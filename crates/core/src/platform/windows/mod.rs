//! Windows implementation of the platform layer.

mod actions;
mod crash;
mod dpi;
mod focus;
mod folders;
mod hook_watchdog;
mod input;
mod instance;
mod keyboard;
pub mod office_key;
mod overlay;
mod shell;
mod time;
mod tray;
mod watcher;

pub use actions::{action_thread_init, action_thread_run, action_thread_stop, action_thread_wake};
pub use crash::install_crash_guard;
pub use dpi::dpi_awareness;
pub use focus::{Foreground, foreground_window};
pub use folders::roaming_app_data_dir;
pub use instance::Instance;
pub use keyboard::Keyboard;
pub use overlay::Overlay;
pub use shell::{launch, open_url, open_with_default_app, spawn_detached};
pub use time::local_time_now;
pub use tray::{Tray, set_tray_paused, set_tray_update, show_notification};
pub use watcher::Watcher;

use ::windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};

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
