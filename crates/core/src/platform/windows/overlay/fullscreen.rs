//! Foreground fullscreen detection (see `docs/specs/overlay.md`, "Fullscreen").
//!
//! Only non-blocking queries on foreign windows: no messages are sent to them.

use ::windows::Win32::Foundation::{HWND, RECT};
use ::windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromWindow,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetClassNameW, GetShellWindow, GetWindowLongW, GetWindowRect, IsZoomed, WS_CAPTION,
};

use super::taskbars;

/// Window classes of the shell itself (desktop and taskbars): never "fullscreen".
const SHELL_CLASSES: [&str; 4] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

/// The monitor `fg` covers in fullscreen, or `None` if it is not a fullscreen window.
pub fn fullscreen_monitor(fg: HWND) -> Option<HMONITOR> {
    if fg.is_invalid() || taskbars::window_pid(fg) == std::process::id() {
        return None;
    }
    // SAFETY: plain queries on a window handle; out-params are locals. A stale handle only makes
    // them fail.
    unsafe {
        if GetShellWindow() == fg || is_shell_class(fg) {
            return None;
        }
        let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONULL);
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
        let mut rc = RECT::default();
        GetWindowRect(fg, &mut rc).ok()?;
        if !contains(&rc, &info.rcMonitor) {
            return None;
        }
        // A maximized normal window also covers the monitor when the taskbar auto-hides.
        let style = GetWindowLongW(fg, GWL_STYLE) as u32;
        let captioned = style & WS_CAPTION.0 == WS_CAPTION.0;
        if IsZoomed(fg).as_bool() && captioned {
            return None;
        }
        Some(monitor)
    }
}

fn contains(outer: &RECT, inner: &RECT) -> bool {
    outer.left <= inner.left
        && outer.top <= inner.top
        && outer.right >= inner.right
        && outer.bottom >= inner.bottom
}

fn is_shell_class(hwnd: HWND) -> bool {
    let mut buf = [0u16; 64];
    // SAFETY: writes at most `buf.len()` chars into a local buffer.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    let Ok(n) = usize::try_from(n) else {
        return false;
    };
    let class = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    SHELL_CLASSES.contains(&class.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_containment() {
        let mon = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert!(contains(&mon, &mon));
        let bigger = RECT {
            left: -8,
            top: -8,
            right: 1928,
            bottom: 1088,
        };
        assert!(contains(&bigger, &mon));
        let smaller = RECT {
            bottom: 1040,
            ..mon
        };
        assert!(!contains(&smaller, &mon));
    }
}
