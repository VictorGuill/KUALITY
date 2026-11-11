//! Windows implementation of the platform layer.

mod actions;
mod clipboard;
mod crash;
mod dpi;
mod focus;
mod folders;
mod hook_watchdog;
mod input;
mod instance;
mod ipc;
mod keyboard;
pub mod office_key;
mod overlay;
mod shell;
mod time;
mod tray;
mod watcher;

pub use actions::{action_thread_init, action_thread_run, action_thread_stop, action_thread_wake};
pub use clipboard::{
    ClipboardMonitor, MemoryCipher, Putter, close_shell_flyout, protect, set_paste_target,
    shell_flyout_in_front, unprotect,
};
pub use crash::install_crash_guard;
pub use dpi::dpi_awareness;
pub use focus::{Foreground, foreground_window};
pub use folders::roaming_app_data_dir;
pub use instance::Instance;
pub use ipc::{IpcServer, Reply};
pub use keyboard::Keyboard;
pub use overlay::Overlay;
pub use shell::{launch, open_url, open_with_default_app, spawn_detached};
pub use time::local_time_now;
pub use tray::{Tray, set_tray_paused, set_tray_update, show_notification};
pub use watcher::Watcher;

use ::windows::Win32::Foundation::HANDLE;
use ::windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use ::windows::Win32::System::Threading::{PROCESS_NAME_WIN32, QueryFullProcessImageNameW};
use ::windows::core::PWSTR;

/// A Win32 kernel handle that may be moved to another thread.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SendHandle(pub HANDLE);
// SAFETY: kernel handles are process-wide and usable from any thread.
unsafe impl Send for SendHandle {}

/// Full path of the exe of `process` (a handle with `PROCESS_QUERY_LIMITED_INFORMATION`).
pub(crate) fn process_image_path(process: HANDLE) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` and `len` are valid for the call.
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Per-thread setup for the main (UI) thread: COM in STA mode, needed by `ShellExecuteW`.
pub fn init_main_thread() {
    init_sta();
}

fn init_sta() {
    // SAFETY: called once per thread before any COM/shell use; never uninitialized because
    // these threads live until the process exits.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if hr.is_err() {
        log::warn!("CoInitializeEx failed: {hr:?}");
    }
}

/// Converts a string to a NUL-terminated UTF-16 buffer.
pub(crate) fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}
