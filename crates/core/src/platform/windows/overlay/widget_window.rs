//! One layered child window per (taskbar, widget).
//!
//! The window procedure never touches the overlay state: it only posts messages to the host
//! window (context menu, external destruction, DPI change), handled later by the thread.

use std::cell::Cell;
use std::io;

use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetParent, HWND_TOP, IDC_ARROW, IsWindow,
    LoadCursorW, MA_NOACTIVATE, PostMessageW, RegisterClassExW, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, WM_CONTEXTMENU, WM_DESTROY,
    WM_DPICHANGED_AFTERPARENT, WM_MOUSEACTIVATE, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOPARENTNOTIFY,
};
use ::windows::core::{PCWSTR, w};

use super::render::Surface;
use super::thread::{WM_APP_MENU, WM_APP_REFRESH, host_hwnd};
use crate::overlay::layout::RectPx;

const CLASS_NAME: PCWSTR = w!("KualityWidget");

thread_local! {
    /// Set while we destroy our own windows, so `WM_DESTROY` can tell them from windows that
    /// died with their taskbar.
    static DESTROYING: Cell<bool> = const { Cell::new(false) };
}

pub fn register_class() -> io::Result<()> {
    // SAFETY: standard class registration with static strings.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(io::Error::other)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// A widget window on one taskbar.
#[derive(Debug)]
pub struct WidgetWindow {
    pub hwnd: HWND,
    pub surface: Option<Surface>,
}

impl WidgetWindow {
    /// Creates a hidden window in `parent` (a taskbar).
    pub fn create(parent: HWND) -> io::Result<Self> {
        #[cfg(debug_assertions)]
        if test_fail_create() {
            return Err(io::Error::from_raw_os_error(5));
        }
        // SAFETY: plain window creation; the parent belongs to Explorer (no messages are sent to
        // it thanks to WS_EX_NOPARENTNOTIFY).
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).map_err(io::Error::other)?;
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_NOPARENTNOTIFY,
                CLASS_NAME,
                w!(""),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                Some(instance.into()),
                None,
            )
        }
        .map_err(io::Error::other)?;
        Ok(Self {
            hwnd,
            surface: None,
        })
    }

    /// Still alive and attached to `parent`.
    pub fn is_alive_in(&self, parent: HWND) -> bool {
        // SAFETY: plain queries; a stale HWND only makes them fail.
        unsafe { IsWindow(Some(self.hwnd)).as_bool() && GetParent(self.hwnd).ok() == Some(parent) }
    }

    /// Moves/resizes the window and puts it on top of its siblings (call in item order so later
    /// items end on top). Shows it unless `hidden`.
    pub fn place(&self, r: RectPx, hidden: bool) {
        let show = if hidden {
            Default::default()
        } else {
            SWP_SHOWWINDOW
        };
        // SAFETY: our own window.
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOP),
                r.x,
                r.y,
                r.w,
                r.h,
                SWP_NOACTIVATE | show,
            )
        };
        if hidden {
            self.show(false);
        }
    }

    /// Puts the window on top of its siblings without moving it.
    pub fn raise(&self) {
        // SAFETY: our own window.
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )
        };
    }

    pub fn show(&self, visible: bool) {
        // SAFETY: our own window.
        let _ = unsafe { ShowWindow(self.hwnd, if visible { SW_SHOWNA } else { SW_HIDE }) };
    }
}

impl Drop for WidgetWindow {
    fn drop(&mut self) {
        DESTROYING.set(true);
        // SAFETY: our own window; fails harmlessly if it already died with its taskbar.
        let _ = unsafe { DestroyWindow(self.hwnd) };
        DESTROYING.set(false);
    }
}

/// Debug builds only: `KUALITY_TEST_FAIL_CREATES=<n>` makes the first `n` window creations
/// fail with "Access is denied", to test the retry path.
#[cfg(debug_assertions)]
fn test_fail_create() -> bool {
    use std::sync::atomic::{AtomicI64, Ordering};
    // -1 = not read yet.
    static LEFT: AtomicI64 = AtomicI64::new(-1);
    if LEFT.load(Ordering::Relaxed) == -1 {
        let n = std::env::var("KUALITY_TEST_FAIL_CREATES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        LEFT.store(n, Ordering::Relaxed);
    }
    if LEFT.load(Ordering::Relaxed) > 0 {
        LEFT.fetch_sub(1, Ordering::Relaxed);
        return true;
    }
    false
}

fn post_host(msg: u32, wparam: WPARAM, lparam: LPARAM) {
    if let Some(host) = host_hwnd() {
        // SAFETY: PostMessageW never blocks.
        let _ = unsafe { PostMessageW(Some(host), msg, wparam, lparam) };
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CONTEXTMENU => {
            // Handled here (never forwarded to the taskbar). lparam = screen position, or
            // (-1, -1) from the keyboard.
            post_host(WM_APP_MENU, WPARAM(hwnd.0 as usize), lparam);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_DESTROY => {
            if !DESTROYING.get() {
                // Its taskbar died (Explorer restart, monitor removed…).
                post_host(WM_APP_REFRESH, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_DPICHANGED_AFTERPARENT => {
            post_host(WM_APP_REFRESH, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        // SAFETY: default handling with the arguments we received.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
