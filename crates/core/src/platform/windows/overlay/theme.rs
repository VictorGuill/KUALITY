//! Light or dark taskbar.
//!
//! The default text color follows what the taskbar actually shows: [`sample`] reads its pixels
//! from the screen. The `SystemUsesLightTheme` registry value is only the fallback, because it
//! can disagree with the real taskbar (seen: value 1 while the taskbar stayed dark).

use ::windows::Win32::Foundation::{HWND, RECT};
use ::windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GdiFlush, GetDC, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromWindow, ReleaseDC, SRCCOPY, SelectObject,
};
use ::windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use ::windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use ::windows::core::w;

use crate::overlay::widgets::{Theme, relative_luminance};

/// Sampled bands, as fractions of the taskbar height (icons sit in the middle).
const BANDS: [f64; 2] = [0.12, 0.88];
/// Fewer usable samples than this → no verdict.
const MIN_SAMPLES: usize = 16;

/// What the taskbar `hwnd` shows: median relative luminance of a pixel grid in two horizontal
/// bands, skipping `exclude` (screen rects of our widgets). `None` when it cannot be sampled
/// (taskbar off its monitor, e.g. auto-hidden, or the capture failed).
pub fn sample(hwnd: HWND, exclude: &[RECT]) -> Option<(Theme, f64)> {
    let mut r = RECT::default();
    // SAFETY: plain queries on Explorer's window and its monitor into locals (non-blocking).
    unsafe {
        GetWindowRect(hwnd, &mut r).ok()?;
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let m = info.rcMonitor;
        if r.left < m.left || r.top < m.top || r.right > m.right || r.bottom > m.bottom {
            return None;
        }
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w < 8 || h < 4 {
        return None;
    }
    let rows: Vec<i32> = BANDS
        .iter()
        .map(|f| ((f64::from(h) * f) as i32).clamp(0, h - 1))
        .collect();
    let pixels = capture_rows(r.left, r.top, w, &rows)?;
    let step = (w / 400).max(4);
    let mut lum = Vec::new();
    for (row_index, &y) in rows.iter().enumerate() {
        for x in (0..w).step_by(step as usize) {
            let (sx, sy) = (r.left + x, r.top + y);
            if exclude
                .iter()
                .any(|e| sx >= e.left && sx < e.right && sy >= e.top && sy < e.bottom)
            {
                continue;
            }
            let p = pixels[row_index * w as usize + x as usize];
            let [b, g, red, _] = p.to_le_bytes();
            lum.push(relative_luminance(red, g, b));
        }
    }
    if lum.len() < MIN_SAMPLES {
        return None;
    }
    lum.sort_by(f64::total_cmp);
    let median = lum[lum.len() / 2];
    Some((Theme::for_background(median), median))
}

/// Copies screen rows `top + rows[i]`, `left .. left + w` into BGRA pixels (one row per entry).
fn capture_rows(left: i32, top: i32, w: i32, rows: &[i32]) -> Option<Vec<u32>> {
    let n = i32::try_from(rows.len()).ok()?;
    // SAFETY: screen DC, memory DC and DIB are created and released here; the DIB holds w*n
    // pixels and is read only after GdiFlush.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return None;
        }
        let mem = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -n,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut out = None;
        if let Ok(bitmap) = CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            let old = SelectObject(mem, bitmap.into());
            let mut ok = !bits.is_null();
            for (i, &y) in rows.iter().enumerate() {
                ok &= BitBlt(mem, 0, i as i32, w, 1, Some(screen), left, top + y, SRCCOPY).is_ok();
            }
            if ok {
                let _ = GdiFlush();
                let len = w as usize * rows.len();
                out = Some(std::slice::from_raw_parts(bits.cast::<u32>(), len).to_vec());
            }
            SelectObject(mem, old);
            let _ = DeleteObject(bitmap.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        out
    }
}

/// Reads the system mode from the registry (fallback); missing or unreadable value → dark.
pub fn current() -> Theme {
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `value`/`size` are locals matching the requested REG_DWORD type.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    if r.is_ok() && value != 0 {
        Theme::Light
    } else {
        Theme::Dark
    }
}
