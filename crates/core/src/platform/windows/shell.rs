//! Opening files with their associated program.

use std::io;
use std::path::Path;

use ::windows::Win32::UI::Shell::ShellExecuteW;
use ::windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use ::windows::core::{PCWSTR, w};

use super::wide;

/// Opens `path` with its default program; falls back to Notepad if there is no association.
pub fn open_with_default_app(path: &Path) -> io::Result<()> {
    let file = wide(path);
    // SAFETY: all strings are NUL-terminated UTF-16 buffers alive for the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success.
    if result.0 as isize > 32 {
        return Ok(());
    }
    log::debug!(
        "ShellExecuteW(open) failed with {}, falling back to notepad",
        result.0 as isize
    );

    let mut quoted = std::ffi::OsString::from("\"");
    quoted.push(path.as_os_str());
    quoted.push("\"");
    let params = wide(quoted);
    // SAFETY: as above.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("notepad.exe"),
            PCWSTR(params.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "ShellExecuteW failed with code {}",
            result.0 as isize
        )))
    }
}
