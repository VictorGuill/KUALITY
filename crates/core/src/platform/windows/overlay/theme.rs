//! Windows mode of the taskbar (`SystemUsesLightTheme`, not the apps mode).

use ::windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use ::windows::core::w;

use crate::overlay::widgets::Theme;

/// Reads the current system mode; missing or unreadable value → dark.
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
