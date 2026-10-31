//! Finding Explorer's taskbars and watching them with WinEvent hooks.
//!
//! Only non-blocking calls are made on Explorer windows: `FindWindowExW`, `GetClientRect`,
//! `GetWindowThreadProcessId`, `MonitorFromWindow`, `GetDpiForMonitor` (overlay thread rule).

use ::windows::Win32::Foundation::{HWND, RECT};
use ::windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use ::windows::Win32::UI::Accessibility::{
    HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent, WINEVENTPROC,
};
use ::windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use ::windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GetClientRect, GetWindowThreadProcessId, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS,
};
use ::windows::core::w;

use crate::overlay::layout::SizePx;

/// Primary taskbar first, then the secondary ones (one per extra monitor).
pub fn enumerate() -> Vec<HWND> {
    let mut out = Vec::new();
    // SAFETY: plain window lookups by class name.
    unsafe {
        let Ok(primary) = FindWindowW(w!("Shell_TrayWnd"), None) else {
            return out;
        };
        out.push(primary);
        let mut prev = None;
        while let Ok(next) = FindWindowExW(None, prev, w!("Shell_SecondaryTrayWnd"), None) {
            out.push(next);
            prev = Some(next);
            if out.len() > 64 {
                break; // Defensive: never loop forever.
            }
        }
    }
    out
}

/// Client-area size in physical px, or `None` if the window is gone.
pub fn client_size(hwnd: HWND) -> Option<SizePx> {
    let mut rc = RECT::default();
    // SAFETY: writes into a local RECT; fails for a stale HWND.
    unsafe { GetClientRect(hwnd, &mut rc) }.ok()?;
    Some(SizePx {
        w: rc.right - rc.left,
        h: rc.bottom - rc.top,
    })
}

/// The monitor a window is on and its scale (effective DPI / 96).
pub fn monitor_scale(hwnd: HWND) -> (HMONITOR, f64) {
    // SAFETY: plain queries; out-params are locals.
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let (mut x, mut y) = (96, 96);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_err() || x == 0 {
            x = 96;
        }
        (monitor, f64::from(x) / 96.0)
    }
}

/// Process id owning a window (0 if gone).
pub fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    // SAFETY: writes into a local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// An out-of-context WinEvent hook for `event` (a single event id) limited to `pid` (0 = all
/// processes), skipping our own process. Served by this thread's message loop.
pub fn hook(event: u32, pid: u32, callback: WINEVENTPROC) -> Option<HWINEVENTHOOK> {
    // SAFETY: out-of-context hook; the callback runs on this thread while it pumps messages.
    let h = unsafe {
        SetWinEventHook(
            event,
            event,
            None,
            callback,
            pid,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if h.is_invalid() {
        log::warn!("SetWinEventHook({event:#x}) failed");
        None
    } else {
        Some(h)
    }
}

pub fn unhook(h: HWINEVENTHOOK) {
    // SAFETY: `h` came from `hook` on this thread.
    let _ = unsafe { UnhookWinEvent(h) };
}
