//! Known folders.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use ::windows::Win32::System::Com::CoTaskMemFree;
use ::windows::Win32::UI::Shell::{FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

/// `%APPDATA%` (roaming app data), or `None` if it cannot be resolved.
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
