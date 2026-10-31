//! Single instance through a named mutex.

use std::io;

use ::windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use ::windows::Win32::System::Threading::CreateMutexW;
use ::windows::core::PCWSTR;

use super::wide;
use crate::platform::SingleInstance;

/// Holds the named mutex for the process lifetime.
#[derive(Debug)]
pub struct Instance(HANDLE);

impl SingleInstance for Instance {
    fn acquire(name: &str) -> io::Result<Option<Self>> {
        let name = wide(name);
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
