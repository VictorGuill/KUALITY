//! Hook watchdog: notices when Windows silently removed the keyboard hook, see
//! `docs/specs/hyperkey.md` ("Hook watchdog").
//!
//! While the hook is installed, the keyboard window also registers for keyboard Raw Input
//! (`RIDEV_INPUTSINK`, so it gets input while in the background). The hook callback counts the
//! physical CapsLock presses it sees ([`hook_saw_caps_down`]); the window procedure checks each
//! physical CapsLock press that arrives as `WM_INPUT` ([`Watchdog::hook_missed`]). The hook runs
//! before Raw Input is generated, so every physical press reaching Raw Input must have been
//! counted first; if the count did not move, the hook did not see it: Windows removed it
//! (`LowLevelHooksTimeout`) and it must be reinstalled. This works whether or not keys the hook
//! swallows still reach Raw Input. Nothing here runs inside the hook callback except the
//! counter increment.

use std::sync::atomic::{AtomicU32, Ordering};

use ::windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use ::windows::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use ::windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use ::windows::Win32::UI::Input::KeyboardAndMouse::VK_CAPITAL;
use ::windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RIM_TYPEKEYBOARD, RegisterRawInputDevices,
};
use ::windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// HID usage page / usage of keyboards.
const USAGE_PAGE_GENERIC: u16 = 0x01;
const USAGE_KEYBOARD: u16 = 0x06;
/// `RAWKEYBOARD::Flags` bit set on key up.
const RI_KEY_BREAK: u16 = 1;

/// Physical (not injected) CapsLock downs seen by the hook callback, auto-repeat included.
static HOOK_CAPS_DOWNS: AtomicU32 = AtomicU32::new(0);

/// Called by the hook callback for every physical CapsLock down (one relaxed atomic add).
pub fn hook_saw_caps_down() {
    HOOK_CAPS_DOWNS.fetch_add(1, Ordering::Relaxed);
}

/// Raw Input registration and the last hook count checked; owned by the keyboard thread.
#[derive(Debug, Default)]
pub struct Watchdog {
    registered: bool,
    seen: u32,
}

impl Watchdog {
    /// Starts watching for `hwnd` (call right after the hook is installed).
    pub fn start(&mut self, hwnd: HWND) {
        self.seen = HOOK_CAPS_DOWNS.load(Ordering::Relaxed);
        if !self.registered {
            self.registered = register(Some(hwnd));
            if !self.registered {
                log::warn!("hook watchdog: cannot register for keyboard raw input");
            }
        }
    }

    /// Stops watching (call when the hook is removed on purpose).
    pub fn stop(&mut self) {
        if self.registered {
            self.registered = false;
            let _ = register(None);
        }
    }

    /// For a `WM_INPUT` message: whether it is a physical CapsLock press the hook did not see.
    pub fn hook_missed(&mut self, lparam: LPARAM) -> bool {
        if !self.registered || !is_physical_caps_down(lparam) {
            return false;
        }
        let now = HOOK_CAPS_DOWNS.load(Ordering::Relaxed);
        let missed = now == self.seen;
        self.seen = now;
        // UIPI hides keys sent to an elevated window from our hook (not a lost hook).
        missed && !foreground_above_us()
    }
}

/// Starts (`Some(hwnd)`) or stops (`None`) keyboard Raw Input for the keyboard window.
/// Returns whether the call succeeded.
fn register(hwnd: Option<HWND>) -> bool {
    let device = RAWINPUTDEVICE {
        usUsagePage: USAGE_PAGE_GENERIC,
        usUsage: USAGE_KEYBOARD,
        dwFlags: if hwnd.is_some() {
            RIDEV_INPUTSINK
        } else {
            RIDEV_REMOVE
        },
        // RIDEV_REMOVE requires a null target.
        hwndTarget: hwnd.unwrap_or_default(),
    };
    // SAFETY: one valid RAWINPUTDEVICE; the size matches the struct.
    unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) }.is_ok()
}

/// Whether a `WM_INPUT` message is a CapsLock press from a physical keyboard. Injected input
/// (ours or other programs') has no device handle and is ignored.
fn is_physical_caps_down(lparam: LPARAM) -> bool {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    // SAFETY: `raw` is a buffer of `size` bytes; `lparam` is the HRAWINPUT of the WM_INPUT
    // message being handled (a stale handle only makes the call fail).
    let n = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as *mut _),
            RID_INPUT,
            Some((&raw mut raw).cast()),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if n == 0 || n == u32::MAX || raw.header.dwType != RIM_TYPEKEYBOARD.0 {
        return false;
    }
    if raw.header.hDevice.is_invalid() {
        return false;
    }
    // SAFETY: `dwType` says the union holds keyboard data.
    let kb = unsafe { raw.data.keyboard };
    kb.VKey == VK_CAPITAL.0 && kb.Flags & RI_KEY_BREAK == 0
}

/// Whether input to the foreground window may be hidden from our hook by UIPI: we are not
/// elevated and the foreground process is (or cannot be inspected). Our hook is not called
/// for such windows, so a CapsLock reaching Raw Input then does not mean the hook is gone.
fn foreground_above_us() -> bool {
    // SAFETY: pseudo handle of the current process; never closed.
    if process_elevated(unsafe { GetCurrentProcess() }) == Some(true) {
        return false;
    }
    // SAFETY: no pointers besides the out parameter.
    let pid = unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid
    };
    if pid == 0 {
        return false;
    }
    // SAFETY: plain call; the handle is closed below.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return true;
    };
    let elevated = process_elevated(process);
    // SAFETY: handle opened above.
    let _ = unsafe { CloseHandle(process) };
    elevated != Some(false)
}

/// `TokenElevation` of a process, `None` if its token cannot be read.
fn process_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: `token` is closed below when opened.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0;
    // SAFETY: the buffer is a TOKEN_ELEVATION of the size passed.
    let r = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    // SAFETY: token opened above.
    let _ = unsafe { CloseHandle(token) };
    r.ok()?;
    Some(elevation.TokenIsElevated != 0)
}
