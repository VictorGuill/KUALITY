//! Windows-specific parts of the settings app: known folder, single instance, the link to the
//! core process and the key-recording hook. Everything else is portable.

use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_channel::oneshot;
use kuality_protocol::Key;
use kuality_protocol::cli::{self, ArmedSignal};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{COLORREF, SIZE};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0,
    WPARAM,
};
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_CAPTION_COLOR, DWMWA_CLOAK, DWMWA_CLOAKED,
    DWMWA_TEXT_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmGetWindowAttribute,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetMonitorInfoW, GetObjectW, HBITMAP, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, GetCurrentThreadId, INFINITE, OpenEventW, OpenMutexW,
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, SYNCHRONIZATION_SYNCHRONIZE, WaitForMultipleObjects,
    WaitForSingleObject,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, FOLDERID_RoamingAppData, IEnumShellItems, IShellItem,
    IShellItemImageFactory, KF_FLAG_DEFAULT, SHCreateItemFromParsingName, SHGetKnownFolderItem,
    SHGetKnownFolderPath, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING, SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CWPSTRUCT, CallNextHookEx, DispatchMessageW, EnumWindows,
    FindWindowW, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetCursorPos, GetForegroundWindow,
    GetMessageW, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, HC_ACTION, HHOOK, IsIconic, IsWindowVisible, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, MSG, PostMessageW, PostQuitMessage, PostThreadMessageW, SW_RESTORE,
    SetForegroundWindow, SetTimer, SetWindowsHookExW, ShowWindow, TranslateMessage,
    UnhookWindowsHookEx, WH_CALLWNDPROC, WH_KEYBOARD_LL, WM_APP, WM_CLOSE, WM_CREATE, WM_KEYDOWN,
    WM_SHOWWINDOW, WM_SYSKEYDOWN, WM_TIMER, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, Interface as _, PCWSTR, PWSTR, w};

use crate::apps::{AppInfo, ICON_PX, RawIcon, straight_alpha};
use crate::recorder::{Mode, Outcome, Recording};

/// Window title, also used by a second instance to find the first one.
pub const WINDOW_TITLE: &str = "Kuality Settings";
const INSTANCE_MUTEX: &str = r"Local\Kuality.Settings";
/// Title of the to-do popup window, used by the core to find and close it.
pub const TODO_WINDOW_TITLE: &str = "Kuality To-do";
/// Held by the to-do popup while it runs.
const TODO_MUTEX: &str = r"Local\Kuality.Todo";
/// Held by a to-do popup while it is only shown on hover (until it is clicked).
const TODO_PEEK_MUTEX: &str = r"Local\Kuality.TodoPeek";
/// Title of the clipboard history popup, used by the core to find and close it.
pub const CLIPBOARD_WINDOW_TITLE: &str = "Kuality Clipboard";
/// Held by the clipboard history popup while it runs.
const CLIPBOARD_MUTEX: &str = r"Local\Kuality.Clipboard";
/// Title of the App lock prompt, used by the core to find, focus and close it.
pub const UNLOCK_WINDOW_TITLE: &str = "Kuality unlock";
/// Held by the App lock prompt while it runs.
const UNLOCK_MUTEX: &str = r"Local\Kuality.Unlock";
const CORE_MUTEX: &str = r"Local\Kuality.Core";
/// Class of the core's hidden main window (owns the tray icon).
const CORE_WINDOW_CLASS: PCWSTR = w!("Kuality.Core");

/// File name of the running exe (`kuality.exe`). The core and the settings window are two
/// processes of this same exe, so process checks compare against it (a renamed exe still works).
fn own_exe_name() -> OsString {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_os_string()))
        .unwrap_or_else(|| "kuality.exe".into())
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `%APPDATA%` (roaming app data), or `None` if it cannot be resolved. Same as the core.
pub fn roaming_app_data_dir() -> Option<PathBuf> {
    // SAFETY: on success the returned PWSTR is a valid NUL-terminated string that we own and
    // free with CoTaskMemFree exactly once.
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, None).ok()?;
        let path = PathBuf::from(OsString::from_wide(p.as_wide()));
        CoTaskMemFree(Some(p.as_ptr() as *const _));
        Some(path)
    }
}

/// Holds the settings-app mutex for the process lifetime.
#[derive(Debug)]
pub struct Instance(HANDLE);

impl Instance {
    /// `None` if another settings window is already running.
    pub fn acquire() -> io::Result<Option<Self>> {
        Self::acquire_named(INSTANCE_MUTEX)
    }

    /// `None` if another to-do popup is already running.
    pub fn acquire_todo() -> io::Result<Option<Self>> {
        Self::acquire_named(TODO_MUTEX)
    }

    /// `None` if another clipboard popup is already running.
    pub fn acquire_clipboard() -> io::Result<Option<Self>> {
        Self::acquire_named(CLIPBOARD_MUTEX)
    }

    /// `None` if another App lock prompt is already running.
    pub fn acquire_unlock() -> io::Result<Option<Self>> {
        Self::acquire_named(UNLOCK_MUTEX)
    }

    /// Marks the to-do popup as "shown on hover" while held.
    pub fn acquire_todo_peek() -> io::Result<Option<Self>> {
        Self::acquire_named(TODO_PEEK_MUTEX)
    }

    fn acquire_named(mutex: &str) -> io::Result<Option<Self>> {
        let name = wide(mutex);
        // SAFETY: `name` is a NUL-terminated UTF-16 string alive for the call.
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
            .map_err(io::Error::other)?;
        // SAFETY: reads the calling thread's last error, set by CreateMutexW above.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: `handle` is a valid handle we own.
            let _ = unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        Ok(Some(Self(handle)))
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this guard and closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Brings the first instance's window to the front. Retries for up to 2 s because the first
/// instance may still be opening its window. Returns whether a window was found.
pub fn activate_existing() -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(hwnd) = find_own_window(WINDOW_TITLE) {
            // SAFETY: plain calls on a window handle; a stale handle only makes them fail.
            unsafe {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(hwnd);
            }
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Paints the native title bar: caption background and text, as `0xRRGGBB` (Windows 11; older
/// versions ignore it). The caption buttons keep following the window's dark-mode attribute.
pub fn set_caption_colors(window: &impl HasWindowHandle, caption: u32, text: u32) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut std::ffi::c_void);
    for (attr, rgb) in [(DWMWA_CAPTION_COLOR, caption), (DWMWA_TEXT_COLOR, text)] {
        let color = COLORREF(colorref(rgb));
        // SAFETY: `hwnd` is this process's live window; the pointer and size describe `color`.
        let set = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attr,
                &color as *const COLORREF as *const std::ffi::c_void,
                size_of::<COLORREF>() as u32,
            )
        };
        if let Err(e) = set {
            log::debug!("cannot set the title bar color ({attr:?}): {e}");
        }
    }
}

/// `0xRRGGBB` to a Win32 `COLORREF` (`0x00BBGGRR`).
fn colorref(rgb: u32) -> u32 {
    ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)
}

/// What [`enum_proc`] looks for and what it found.
struct WindowSearch {
    title: &'static str,
    found: Option<HWND>,
}

/// A visible window titled `title` owned by another process of this exe.
fn find_own_window(title: &'static str) -> Option<HWND> {
    let mut search = WindowSearch { title, found: None };
    // SAFETY: the callback only runs during EnumWindows and gets a pointer to `search`, which
    // outlives the call.
    let _ = unsafe {
        EnumWindows(
            Some(enum_proc),
            LPARAM(&mut search as *mut WindowSearch as isize),
        )
    };
    search.found
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `&mut WindowSearch` passed by `find_own_window`.
    let search = unsafe { &mut *(lparam.0 as *mut WindowSearch) };
    // SAFETY: plain query on a window handle given by EnumWindows.
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return true.into();
    }
    let mut buf = [0u16; 64];
    // SAFETY: `buf` is a valid buffer for the call.
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if String::from_utf16_lossy(&buf[..n.max(0) as usize]) != search.title {
        return true.into();
    }
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    // SAFETY: plain call.
    if pid == unsafe { GetCurrentProcessId() } || !process_is_settings(pid) || is_cloaked(hwnd) {
        return true.into();
    }
    search.found = Some(hwnd);
    false.into()
}

// ---------------------------------------------------------------------------------------------
// To-do popup.

/// Closes the running to-do popup (the core toggles it from the taskbar). Returns `false` when
/// none runs. While a popup is still starting (its mutex exists, no window yet), nothing is
/// done and `true` is returned, so a double click never opens two.
pub fn close_todo_popup() -> bool {
    if !todo_popup_running() {
        return false;
    }
    if let Some(hwnd) = find_own_window(TODO_WINDOW_TITLE) {
        // SAFETY: posting never blocks; a stale handle only makes it fail.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
    true
}

/// Brings the running to-do popup to the front (the core calls it on a click, so Windows allows
/// it). Returns whether its window was found and activated.
pub fn focus_todo_popup() -> bool {
    match find_own_window(TODO_WINDOW_TITLE) {
        // SAFETY: plain call; a stale handle only makes it fail.
        Some(hwnd) => unsafe { SetForegroundWindow(hwnd) }.as_bool(),
        None => false,
    }
}

/// Whether a to-do popup runs in this session (its mutex exists).
pub fn todo_popup_running() -> bool {
    mutex_exists(TODO_MUTEX)
}

/// Whether the running to-do popup is only shown on hover (not clicked yet).
pub fn todo_popup_peeking() -> bool {
    mutex_exists(TODO_PEEK_MUTEX)
}

// ---------------------------------------------------------------------------------------------
// Armed popup (`--armed`): started hidden, shown when the core says so.

/// How long an armed popup waits for the core before quitting by itself.
const ARMED_WAIT: Duration = Duration::from_secs(10);

/// The three events of an armed popup ([`ArmedSignal::ALL`] order), created by the core.
#[derive(Debug)]
pub struct ArmedEvents(Vec<HANDLE>);

impl ArmedEvents {
    /// Opens the events of `name` (the `--armed` value). `None` if one is missing: the core
    /// already dropped this popup.
    pub fn open(name: &str) -> Option<Self> {
        let mut opened = Self(Vec::with_capacity(ArmedSignal::ALL.len()));
        for signal in ArmedSignal::ALL {
            let event = wide(&cli::armed_event(name, signal));
            // SAFETY: `event` is a NUL-terminated UTF-16 string alive for the call.
            let h =
                unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(event.as_ptr())) };
            // On failure, dropping `opened` closes what was opened.
            opened.0.push(h.ok()?);
        }
        Some(opened)
    }

    /// Waits on a background thread for the core's signal; the receiver gets it, or `Cancel`
    /// after [`ARMED_WAIT`] without one.
    pub fn wait(self) -> io::Result<oneshot::Receiver<ArmedSignal>> {
        let (tx, rx) = oneshot::channel();
        // The thread takes over the handles; raw values cross the thread boundary.
        let raw: Vec<usize> = self.0.iter().map(|h| h.0 as usize).collect();
        std::mem::forget(self);
        std::thread::Builder::new()
            .name("kuality-settings-armed".into())
            .spawn(move || {
                let events = Self(
                    raw.into_iter()
                        .map(|h| HANDLE(h as *mut std::ffi::c_void))
                        .collect(),
                );
                // SAFETY: the handles are owned by `events`, alive until it drops after the wait.
                let r = unsafe {
                    WaitForMultipleObjects(&events.0, false, ARMED_WAIT.as_millis() as u32)
                };
                let i = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
                let signal = ArmedSignal::ALL
                    .get(i)
                    .copied()
                    .unwrap_or(ArmedSignal::Cancel);
                let _ = tx.send(signal);
            })?;
        Ok(rx)
    }
}

impl Drop for ArmedEvents {
    fn drop(&mut self) {
        for h in &self.0 {
            // SAFETY: each handle is owned by this value and closed once.
            let _ = unsafe { CloseHandle(*h) };
        }
    }
}

/// GPUI's window class: only those windows get cloaked.
const GPUI_WINDOW_CLASS: &str = "Zed::Window";

/// While alive, every GPUI window this thread creates is cloaked (`DWMWA_CLOAK`) before it is
/// shown: it renders normally but is never seen until [`set_cloaked`]`(…, false)`. A
/// thread-local `WH_CALLWNDPROC` hook on our own thread (no injection) cloaks it on `WM_CREATE`
/// and again on `WM_SHOWWINDOW` (DWM refuses it earlier, at `HCBT_CREATEWND`).
pub struct CloakNewWindows(HHOOK);

impl CloakNewWindows {
    pub fn install() -> io::Result<Self> {
        // SAFETY: thread-local hook on the calling thread; the procedure only touches the new
        // window. Removed on drop, on this same thread (the value is not `Send`).
        let hook = unsafe {
            SetWindowsHookExW(WH_CALLWNDPROC, Some(cloak_proc), None, GetCurrentThreadId())
        }
        .map_err(io::Error::other)?;
        Ok(Self(hook))
    }
}

impl Drop for CloakNewWindows {
    fn drop(&mut self) {
        // SAFETY: the hook was installed by `install` and is removed once.
        let _ = unsafe { UnhookWindowsHookEx(self.0) };
    }
}

unsafe extern "system" fn cloak_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: for HC_ACTION, `lparam` points to the message's CWPSTRUCT, valid for the call.
    let msg = (code == HC_ACTION as i32).then(|| unsafe { &*(lparam.0 as *const CWPSTRUCT) });
    if let Some(m) = msg
        && (m.message == WM_CREATE || (m.message == WM_SHOWWINDOW && m.wParam.0 != 0))
    {
        let hwnd = m.hwnd;
        let mut class = [0u16; 32];
        // SAFETY: `class` is a valid buffer for the call; `hwnd` is the window being created.
        let n = unsafe { GetClassNameW(hwnd, &mut class) };
        if String::from_utf16_lossy(&class[..n.max(0) as usize]) == GPUI_WINDOW_CLASS {
            cloak(hwnd, true);
        }
    }
    // SAFETY: passes the call on unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Cloaks (hides from the screen while it keeps rendering) or uncloaks this process's window.
pub fn set_cloaked(window: &impl HasWindowHandle, cloaked: bool) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    cloak(HWND(h.hwnd.get() as *mut std::ffi::c_void), cloaked);
}

fn cloak(hwnd: HWND, cloaked: bool) {
    let value = BOOL::from(cloaked);
    // SAFETY: `hwnd` is a window of this process; the pointer and size describe `value`.
    let r = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            &value as *const BOOL as *const std::ffi::c_void,
            size_of::<BOOL>() as u32,
        )
    };
    if let Err(e) = r {
        let what = if cloaked { "cloak" } else { "uncloak" };
        log::warn!("cannot {what} the window: {e}");
    }
}

/// Whether `hwnd` is cloaked (an armed popup not shown yet, or a window on another desktop).
fn is_cloaked(hwnd: HWND) -> bool {
    let mut value = 0u32;
    // SAFETY: the pointer and size describe `value`.
    let r = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut value as *mut u32 as *mut std::ffi::c_void,
            size_of::<u32>() as u32,
        )
    };
    r.is_ok() && value != 0
}

// ---------------------------------------------------------------------------------------------
// Clipboard history popup.

/// Closes the running clipboard popup. Returns `false` when none runs. While one is still
/// starting (mutex, no window yet), nothing is done and `true` is returned.
pub fn close_clipboard_popup() -> bool {
    if !mutex_exists(CLIPBOARD_MUTEX) {
        return false;
    }
    if let Some(hwnd) = find_own_window(CLIPBOARD_WINDOW_TITLE) {
        // SAFETY: posting never blocks; a stale handle only makes it fail.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
    true
}

// ---------------------------------------------------------------------------------------------
// App lock prompt and tab.

/// Whether an App lock prompt runs in this session (its mutex exists).
pub fn unlock_popup_running() -> bool {
    mutex_exists(UNLOCK_MUTEX)
}

/// Brings the running App lock prompt to the front. Returns whether it was found.
pub fn focus_unlock_popup() -> bool {
    match find_own_window(UNLOCK_WINDOW_TITLE) {
        Some(hwnd) => {
            // SAFETY: plain calls; a stale handle only makes them fail.
            unsafe {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
            }
            force_foreground(hwnd)
        }
        None => false,
    }
}

/// Closes the running App lock prompt (App lock turned off, Kuality quitting).
pub fn close_unlock_popup() -> bool {
    if !unlock_popup_running() {
        return false;
    }
    if let Some(hwnd) = find_own_window(UNLOCK_WINDOW_TITLE) {
        // SAFETY: posting never blocks; a stale handle only makes it fail.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
    true
}

/// Lets the core bring an unlocked app to the front (this prompt is in front now).
pub fn allow_foreground(pid: u32) {
    // SAFETY: plain call.
    let _ = unsafe { AllowSetForegroundWindow(pid) };
}

/// The description of an exe (`FileDescription` of its version resource, e.g. "Brave Browser"),
/// or `None`.
pub fn exe_description(path: &str) -> Option<String> {
    let wpath = wide(&path.replace('/', "\\"));
    // SAFETY: the buffer is sized by the first call; VerQueryValueW returns pointers into it,
    // read while it is alive.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wpath.as_ptr()), None, size, data.as_mut_ptr().cast()).ok()?;
        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let mut langs: Vec<(u16, u16)> = Vec::new();
        if VerQueryValueW(
            data.as_ptr().cast(),
            w!("\\VarFileInfo\\Translation"),
            &mut ptr,
            &mut len,
        )
        .as_bool()
            && !ptr.is_null()
        {
            let pairs = std::slice::from_raw_parts(ptr as *const u16, (len / 2) as usize);
            langs.extend(pairs.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])));
        }
        langs.extend([(0x0409, 0x04b0), (0x0409, 0x04e4), (0x0000, 0x04b0)]);
        for (lang, cp) in langs {
            let key = wide(&format!(
                "\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"
            ));
            if VerQueryValueW(
                data.as_ptr().cast(),
                PCWSTR(key.as_ptr()),
                &mut ptr,
                &mut len,
            )
            .as_bool()
                && !ptr.is_null()
                && len > 1
            {
                let text = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let text = String::from_utf16_lossy(text);
                let text = text.trim_end_matches('\0').trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
        None
    }
}

/// The icon of a file (an exe) at [`ICON_PX`], or `None`. Initializes COM for the call, so it
/// can run on any thread.
pub fn file_icon(path: &str) -> Option<RawIcon> {
    // The shell does not parse `/` as a separator.
    let wpath = wide(&path.replace('/', "\\"));
    // SAFETY: balanced CoInitializeEx / CoUninitialize; the item is dropped before.
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let icon = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wpath.as_ptr()), None)
            .ok()
            .and_then(|item| item_icon(&item));
        if com.is_ok() {
            CoUninitialize();
        }
        icon
    }
}

/// A program with a visible window now: its exe path and the title of one of its windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningApp {
    pub path: String,
    pub title: String,
}

/// Programs with a visible, titled top-level window (not tool windows), one entry per exe, in
/// window z-order. This process is left out.
pub fn running_apps() -> Vec<RunningApp> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut Vec<HWND>` passed below, alive during EnumWindows.
        let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        list.push(hwnd);
        true.into()
    }
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: the callback only runs during the call and gets a pointer to `windows`.
    let _ = unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut windows as *mut Vec<HWND> as isize),
        )
    };
    let own = std::process::id();
    let mut out: Vec<RunningApp> = Vec::new();
    for hwnd in windows {
        // SAFETY: plain queries on handles from EnumWindows; stale handles give zeros.
        let (visible, ex, owner, pid, title) = unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(hwnd, &mut buf);
            (
                IsWindowVisible(hwnd).as_bool(),
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
                GetWindow(hwnd, GW_OWNER).is_ok(),
                pid,
                String::from_utf16_lossy(&buf[..n.max(0) as usize]),
            )
        };
        let tool = ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0;
        if !visible || tool || owner || pid == 0 || pid == own || title.trim().is_empty() {
            continue;
        }
        // SAFETY: the handle is checked and closed once.
        let path = unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                continue;
            };
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                h,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
            .is_ok();
            let _ = CloseHandle(h);
            if !ok {
                continue;
            }
            String::from_utf16_lossy(&buf[..len as usize])
        };
        if !out.iter().any(|a| a.path.eq_ignore_ascii_case(&path)) {
            out.push(RunningApp {
                path,
                title: title.trim().to_string(),
            });
        }
    }
    out
}

/// Center of the foreground window (physical px): the popup opens on its monitor.
pub fn foreground_center() -> Option<(i32, i32)> {
    let mut r = RECT::default();
    // SAFETY: plain queries into a local.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        GetWindowRect(hwnd, &mut r).ok()?;
    }
    Some(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
}

/// Why a pipe request failed.
#[derive(Debug)]
pub enum PipeError {
    /// No pipe: the core is not running, or neither the clipboard history nor App lock is on.
    NotRunning,
    Other(String),
}

impl std::fmt::Display for PipeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => f.write_str("Kuality is not running, or this feature is off."),
            Self::Other(e) => f.write_str(e),
        }
    }
}

/// Sends one request to the core's pipe and reads the reply. Checks that the pipe is served by
/// process `core_pid`.
pub fn pipe_request<Resp: serde::de::DeserializeOwned>(
    core_pid: u32,
    request: impl serde::Serialize,
) -> Result<(Resp, Vec<u8>), PipeError> {
    use std::os::windows::io::AsRawHandle;
    const ERROR_PIPE_BUSY: i32 = 231;
    let name = kuality_protocol::ipc::pipe_name(core_pid);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut file = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
        {
            Ok(f) => break f,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(PipeError::NotRunning),
            Err(e) => return Err(PipeError::Other(e.to_string())),
        }
    };
    let mut server = 0u32;
    // SAFETY: the handle belongs to `file` and stays open for the call.
    let checked = unsafe { GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut server) };
    if checked.is_err() || server != core_pid {
        return Err(PipeError::Other(
            "The pipe is not served by Kuality.".into(),
        ));
    }
    let io_error = |e: io::Error| PipeError::Other(e.to_string());
    kuality_protocol::ipc::write_frame(&mut file, &request, &[]).map_err(io_error)?;
    kuality_protocol::ipc::read_frame(&mut file).map_err(io_error)
}

/// The mouse position in physical screen pixels.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY: writes into a local.
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some((p.x, p.y))
}

fn mutex_exists(mutex: &str) -> bool {
    let name = wide(mutex);
    // SAFETY: `name` is NUL-terminated and alive for the call; the handle is closed at once.
    unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

/// A screen rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ScreenRect {
    pub fn contains(&self, (x, y): (i32, i32)) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// A monitor for the popup: GPUI display id (the `HMONITOR`), work area and scale.
#[derive(Debug, Clone, Copy)]
pub struct Monitor {
    pub id: u64,
    pub work: ScreenRect,
    pub scale: f32,
}

/// The monitor at `anchor` (nearest), or the primary one.
pub fn monitor_at(anchor: Option<(i32, i32)>) -> Option<Monitor> {
    // SAFETY: plain monitor queries into locals.
    unsafe {
        let monitor = match anchor {
            Some((x, y)) => MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST),
            None => MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY),
        };
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let w = info.rcWork;
        Some(Monitor {
            id: monitor.0 as u64,
            work: ScreenRect {
                left: w.left,
                top: w.top,
                right: w.right,
                bottom: w.bottom,
            },
            scale: dpi_x as f32 / 96.0,
        })
    }
}

/// Rounded corners (Windows 11) and the foreground for the popup window.
pub fn popup_window_setup(window: &impl HasWindowHandle, activate: bool) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut std::ffi::c_void);
    let corners: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
    // SAFETY: `hwnd` is this process's live window; the pointer and size describe `corners`.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corners as *const _ as *const std::ffi::c_void,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
    if activate {
        force_foreground(hwnd);
    }
}

/// Brings this process's popup to the front. Returns whether it is in front.
///
/// The core's `AllowSetForegroundWindow` grant ends at the next input, and the user releases
/// the hotkey while this process starts, so it is often gone by now. An empty mouse event makes
/// this process the source of the last input, which lets it set the foreground itself (same
/// technique as the core's launched apps, `docs/DECISIONS.md` "Foreground for launched apps").
pub fn bring_to_front(window: &impl HasWindowHandle) -> bool {
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return false;
    };
    force_foreground(HWND(h.hwnd.get() as *mut std::ffi::c_void))
}

fn force_foreground(hwnd: HWND) -> bool {
    // SAFETY: plain calls; a stale handle only makes them fail. The input has no movement and
    // no buttons.
    unsafe {
        if GetForegroundWindow() == hwnd {
            return true;
        }
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT::default(),
            },
        };
        SendInput(&[input], size_of::<INPUT>() as i32);
        let _ = SetForegroundWindow(hwnd);
        GetForegroundWindow() == hwnd
    }
}

/// Whether `pid` runs this exe (the window title check already tells settings from the core).
fn process_is_settings(pid: u32) -> bool {
    // SAFETY: the handle is checked and closed once.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let ok = image_is(h, &own_exe_name());
        let _ = CloseHandle(h);
        ok
    }
}

/// Whether the process behind `h` (opened with at least query-limited rights) runs an exe named
/// `exe_name`.
fn image_is(h: HANDLE, exe_name: &std::ffi::OsStr) -> bool {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` and `len` are valid for the call; `h` is a live process handle.
    let ok = unsafe {
        QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
    }
    .is_ok();
    ok && PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case(exe_name))
}

/// Whether the Kuality core is running in this session (its single-instance mutex exists).
pub fn core_running() -> bool {
    let name = wide(CORE_MUTEX);
    // SAFETY: `name` is NUL-terminated and alive for the call; the handle is closed at once.
    unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Link to the core: the settings window lives only as long as the core process.

/// The running core process, opened so we can wait for it to exit.
#[derive(Debug)]
pub struct CoreProcess {
    handle: HANDLE,
    pub pid: u32,
}

impl CoreProcess {
    /// Opens process `pid` if it runs this exe (guards against a reused pid).
    pub fn open(pid: u32) -> Option<Self> {
        // SAFETY: the handle is checked, and owned by the returned value or closed here.
        unsafe {
            let h = OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
            .ok()?;
            if image_is(h, &own_exe_name()) {
                Some(Self { handle: h, pid })
            } else {
                let _ = CloseHandle(h);
                None
            }
        }
    }

    /// Waits for the process to exit on a background thread; the receiver fires then (for any
    /// exit: tray Quit, `taskkill`, crash).
    pub fn watch(self) -> io::Result<oneshot::Receiver<()>> {
        let (tx, rx) = oneshot::channel();
        // The thread takes over the handle; a raw value crosses the thread boundary.
        let raw = self.handle.0 as usize;
        std::mem::forget(self);
        std::thread::Builder::new()
            .name("kuality-settings-core-watch".into())
            .spawn(move || {
                let h = HANDLE(raw as *mut core::ffi::c_void);
                // SAFETY: `h` is the process handle opened in `open` and owned by this thread
                // from here on; it is closed once after the wait.
                unsafe {
                    WaitForSingleObject(h, INFINITE);
                    let _ = CloseHandle(h);
                }
                let _ = tx.send(());
            })?;
        Ok(rx)
    }
}

impl Drop for CoreProcess {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and closed once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// The running core of this session, found through its main window. Waits up to 3 s when the
/// core's mutex exists but its window is not created yet (core still starting).
pub fn find_core() -> Option<CoreProcess> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // SAFETY: plain lookup by a static class name.
        if let Ok(hwnd) = unsafe { FindWindowW(CORE_WINDOW_CLASS, PCWSTR::null()) } {
            let mut pid = 0u32;
            // SAFETY: `pid` is a valid out pointer.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if let Some(core) = CoreProcess::open(pid) {
                return Some(core);
            }
        }
        if !core_running() || Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Starts the core: this same exe without arguments (it applies the same data-dir rule).
pub fn start_core() -> io::Result<CoreProcess> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    // The child handle is dropped: the core keeps running on its own.
    let child = std::process::Command::new(&exe).current_dir(dir).spawn()?;
    CoreProcess::open(child.id())
        .ok_or_else(|| io::Error::other("Kuality exited right after starting"))
}

// ---------------------------------------------------------------------------------------------
// Key recording: a `WH_KEYBOARD_LL` hook on its own thread, alive only while recording (plus
// until the ups of swallowed keys arrive, at most 1 s).

/// Posted to the recorder thread to cancel.
const WM_APP_CANCEL: u32 = WM_APP + 1;
/// How long the hook stays after the recording ends, waiting for swallowed ups.
const SETTLE_MS: u32 = 1000;

struct HookState {
    recording: Recording,
    result: Option<oneshot::Sender<Outcome>>,
    our_pid: u32,
    settle_timer: bool,
}

thread_local! {
    static HOOK: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

/// A running recording. Dropping it cancels the recording.
#[derive(Debug)]
pub struct KeyRecorder {
    thread_id: u32,
}

impl KeyRecorder {
    /// Starts recording; the outcome arrives on the returned receiver.
    pub fn start(mode: Mode) -> io::Result<(Self, oneshot::Receiver<Outcome>)> {
        let (tx, rx) = oneshot::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("kuality-settings-recorder".into())
            .spawn(move || run_hook_thread(mode, tx, ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok((Self { thread_id }, rx)),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::other("recorder thread ended early")),
        }
    }

    /// Ends the recording without a result (if it is still running).
    pub fn cancel(&self) {
        // SAFETY: plain call; failure means the thread already ended.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_APP_CANCEL, WPARAM(0), LPARAM(0)) };
    }
}

impl Drop for KeyRecorder {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run_hook_thread(
    mode: Mode,
    result: oneshot::Sender<Outcome>,
    ready: std::sync::mpsc::Sender<io::Result<u32>>,
) {
    HOOK.with(|h| {
        *h.borrow_mut() = Some(HookState {
            recording: Recording::new(mode),
            result: Some(result),
            // SAFETY: plain call.
            our_pid: unsafe { GetCurrentProcessId() },
            settle_timer: false,
        })
    });
    // SAFETY: the hook procedure is a valid `extern "system"` fn for the process lifetime; the
    // module handle of our own exe is valid.
    let hook = unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).ok();
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module.map(Into::into), 0)
    };
    let hook = match hook {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(io::Error::other(e)));
            return;
        }
    };
    // SAFETY: plain call.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    let mut msg = MSG::default();
    // SAFETY: standard message loop on this thread's queue; `msg` is a valid out pointer.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            match msg.message {
                WM_APP_CANCEL => with_state(|s| {
                    if let Some(o) = s.recording.cancel() {
                        send(s, o);
                    }
                    after_end(s);
                }),
                WM_TIMER => break,
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        let _ = UnhookWindowsHookEx(hook);
    }
    HOOK.with(|h| h.borrow_mut().take());
}

fn with_state(f: impl FnOnce(&mut HookState)) {
    HOOK.with(|h| {
        if let Some(s) = h.borrow_mut().as_mut() {
            f(s);
        }
    });
}

fn send(s: &mut HookState, outcome: Outcome) {
    if let Some(tx) = s.result.take() {
        let _ = tx.send(outcome);
    }
}

/// Once the recording ended: quit when no swallowed up is pending, else wait at most 1 s.
fn after_end(s: &mut HookState) {
    if s.recording.is_active() {
        return;
    }
    if s.recording.is_settled() {
        // SAFETY: plain call on this thread.
        unsafe { PostQuitMessage(0) };
    } else if !s.settle_timer {
        s.settle_timer = true;
        // SAFETY: thread timer (no window); WM_TIMER ends the loop.
        unsafe { SetTimer(None, 0, SETTLE_MS, None) };
    }
}

fn foreground_pid() -> u32 {
    let mut pid = 0u32;
    // SAFETY: plain calls; `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
    pid
}

/// Low-level keyboard hook: feeds the state machine and returns at once.
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL with HC_ACTION, lparam points to a KBDLLHOOKSTRUCT.
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if (kb.flags.0 & LLKHF_INJECTED.0) == 0 {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let mut swallow = false;
            with_state(|s| {
                let fg = down && foreground_pid() == s.our_pid;
                let (sw, outcome) = s.recording.on_key(Key(kb.vkCode as u16), down, fg);
                swallow = sw;
                if let Some(o) = outcome {
                    send(s, o);
                }
                after_end(s);
            });
            if swallow {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: forwards the event to the next hook with the arguments we received.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

// ---------------------------------------------------------------------------------------------
// Installed apps: the shell's Applications folder (`shell:AppsFolder`), read on the
// `kuality-settings-apps` thread (see `apps.rs`).

/// Reads every app of the Applications folder and sends the list (in the order `order` gives),
/// then their icons in the same order. Runs on the calling thread; COM is initialised for it.
pub fn read_apps_folder(
    order: impl FnOnce(&[AppInfo]) -> Vec<usize>,
    list_tx: oneshot::Sender<Result<Vec<AppInfo>, String>>,
    icons_tx: oneshot::Sender<Vec<Option<RawIcon>>>,
) {
    // SAFETY: plain call; balanced by CoUninitialize below when it succeeded.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    {
        let items = match apps_folder_items() {
            Ok(items) => items,
            Err(e) => {
                log::warn!("cannot list the installed apps: {e}");
                let _ = list_tx.send(Err(e.message()));
                return;
            }
        };
        let infos: Vec<AppInfo> = items.iter().map(|(info, _)| info.clone()).collect();
        let order = order(&infos);
        log::debug!("{} apps in the Applications folder", order.len());
        let list = order.iter().map(|&i| infos[i].clone()).collect();
        if list_tx.send(Ok(list)).is_err() {
            return;
        }
        let icons = order.iter().map(|&i| item_icon(&items[i].1)).collect();
        let _ = icons_tx.send(icons);
    }
    if com.is_ok() {
        // SAFETY: matches the successful CoInitializeEx above; every COM object of this thread
        // was dropped at the end of the block.
        unsafe { CoUninitialize() };
    }
}

fn apps_folder_items() -> windows::core::Result<Vec<(AppInfo, IShellItem)>> {
    // SAFETY: plain COM calls on interfaces we own; out values are checked.
    unsafe {
        let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
        let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
        let mut out = Vec::new();
        loop {
            let mut batch: [Option<IShellItem>; 32] = Default::default();
            let mut fetched = 0u32;
            items.Next(&mut batch, Some(&mut fetched))?;
            if fetched == 0 {
                break;
            }
            for item in batch.into_iter().take(fetched as usize).flatten() {
                let name = display_name(&item, SIGDN_NORMALDISPLAY);
                let id = display_name(&item, SIGDN_PARENTRELATIVEPARSING);
                if let (Some(name), Some(id)) = (name, id) {
                    out.push((AppInfo { name, id }, item));
                }
            }
        }
        Ok(out)
    }
}

fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    // SAFETY: on success the PWSTR is a NUL-terminated string we own and free once.
    unsafe {
        let p = item.GetDisplayName(kind).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.as_ptr() as *const _));
        s
    }
}

/// The item's icon at [`ICON_PX`], straight-alpha BGRA.
fn item_icon(item: &IShellItem) -> Option<RawIcon> {
    // SAFETY: COM calls on a live item; the returned bitmap is owned here and deleted once.
    unsafe {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let size = SIZE {
            cx: ICON_PX as i32,
            cy: ICON_PX as i32,
        };
        let hbm = factory.GetImage(size, SIIGBF_ICONONLY).ok()?;
        let icon = bitmap_pixels(hbm);
        let _ = DeleteObject(hbm.into());
        icon
    }
}

fn bitmap_pixels(hbm: HBITMAP) -> Option<RawIcon> {
    let mut bm = BITMAP::default();
    // SAFETY: `bm` is a valid out buffer of the size given.
    let n = unsafe {
        GetObjectW(
            hbm.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut core::ffi::c_void),
        )
    };
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    if n == 0 || w <= 0 || h <= 0 || w > 256 || h > 256 {
        return None;
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Negative: rows top-down.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    // SAFETY: `bgra` holds exactly `h` rows of `w` 32-bit pixels as described by `info`; the
    // memory DC is created and deleted here.
    let lines = unsafe {
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(
            dc,
            hbm,
            0,
            h as u32,
            Some(bgra.as_mut_ptr() as *mut core::ffi::c_void),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        lines
    };
    if lines != h {
        return None;
    }
    straight_alpha(&mut bgra);
    Some(RawIcon {
        width: w as u32,
        height: h as u32,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::colorref;

    #[test]
    fn colorref_swaps_red_and_blue() {
        assert_eq!(colorref(0x112233), 0x332211);
        assert_eq!(colorref(0x0f0f0f), 0x0f0f0f);
        assert_eq!(colorref(0xff0000), 0x0000ff);
    }
}
