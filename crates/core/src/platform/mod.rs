//! OS-specific code.
//!
//! The rest of the core only talks to the traits and free functions re-exported here, so a
//! future port only has to provide another implementation of this module.

use std::io;
use std::ops::ControlFlow;
use std::path::Path;
use std::time::Duration;

use crate::hyperkey::KeyboardCommand;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{
    Instance, Keyboard, Tray, Watcher, dpi_awareness, init_main_thread, install_crash_guard,
    local_time_now, open_with_default_app, roaming_app_data_dir, set_tray_paused,
};

#[cfg(not(windows))]
compile_error!("Kuality only supports Windows for now (see docs/DECISIONS.md).");

/// Guard proving this is the only running instance; released on drop.
pub trait SingleInstance: Sized {
    /// Returns `Ok(None)` if another instance already holds `name`.
    fn acquire(name: &str) -> io::Result<Option<Self>>;
}

/// Watches a directory (not recursive) and calls `on_change` on its own thread after changes
/// settle for `debounce`.
pub trait DirWatcher: Sized {
    fn start(
        dir: &Path,
        debounce: Duration,
        on_change: Box<dyn FnMut() + Send>,
    ) -> io::Result<Self>;
    /// Stops the thread and waits for it.
    fn stop(self);
}

/// Events delivered to the main-thread handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A [`Waker`] was triggered from another thread.
    Wake,
    /// Tray menu: Settings.
    OpenSettings,
    /// Tray menu: Pause (toggles keyboard features).
    TogglePause,
    /// Tray menu: Quit, or the OS asked the app to close.
    Quit,
}

/// Wakes the main event loop from any thread.
pub trait Waker: Clone + Send + 'static {
    fn wake(&self);
}

/// Tray icon + main-thread event loop.
pub trait TrayIcon: Sized {
    type Waker: Waker;

    /// Creates the (hidden) owner window and adds the tray icon.
    fn create(tooltip: &str) -> io::Result<Self>;

    fn waker(&self) -> Self::Waker;

    /// Runs the event loop until the handler returns `Break`. Removes the icon on exit.
    /// The handler is never called re-entrantly.
    fn run(self, handler: impl FnMut(Event) -> ControlFlow<()> + 'static);
}

/// The keyboard hook thread (Hyperkey, later Shortcuts).
pub trait KeyboardBackend: Sized {
    /// Spawns the thread; it runs until [`KeyboardCommand::Shutdown`].
    fn start() -> io::Result<Self>;
    /// Queues a command and wakes the thread.
    fn send(&self, cmd: KeyboardCommand);
    /// Waits up to `timeout` for the thread to end. Returns `false` on timeout.
    fn join(self, timeout: Duration) -> bool;
}
