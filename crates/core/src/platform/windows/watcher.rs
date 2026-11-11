//! Directory change watcher using `FindFirstChangeNotificationW`.
//!
//! The thread blocks in `WaitForMultipleObjects` on the change handle and a stop event, so it
//! costs nothing while idle and shuts down promptly.

use std::io;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

use ::windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use ::windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindCloseChangeNotification,
    FindFirstChangeNotificationW, FindNextChangeNotification,
};
use ::windows::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects,
};
use ::windows::core::PCWSTR;

use super::{SendHandle, wide};
use crate::platform::DirWatcher;

/// Running watcher; call [`DirWatcher::stop`] to end it.
#[derive(Debug)]
pub struct Watcher {
    stop_event: SendHandle,
    thread: Option<JoinHandle<()>>,
}

impl DirWatcher for Watcher {
    fn start(
        dir: &Path,
        debounce: Duration,
        mut on_change: Box<dyn FnMut() + Send>,
    ) -> io::Result<Self> {
        let dir_w = wide(dir);
        // SAFETY: `dir_w` is a NUL-terminated UTF-16 path alive for the call.
        let change = unsafe {
            FindFirstChangeNotificationW(
                PCWSTR(dir_w.as_ptr()),
                false,
                FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME,
            )
        }
        .map_err(io::Error::other)?;
        // SAFETY: creates an unnamed manual-reset event; no pointers passed.
        let stop = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
            Ok(h) => h,
            Err(e) => {
                // SAFETY: `change` is a valid change-notification handle we own.
                let _ = unsafe { FindCloseChangeNotification(change) };
                return Err(io::Error::other(e));
            }
        };

        let change = SendHandle(change);
        let stop_h = SendHandle(stop);
        let debounce_ms = u32::try_from(debounce.as_millis()).unwrap_or(u32::MAX);
        let thread = std::thread::Builder::new()
            .name("config-watcher".into())
            .spawn(move || {
                let (change, stop) = (change, stop_h);
                watch_loop(change.0, stop.0, debounce_ms, &mut *on_change);
                // SAFETY: both handles are owned by this thread from here on and closed once.
                unsafe {
                    let _ = FindCloseChangeNotification(change.0);
                }
            })?;
        Ok(Self {
            stop_event: SendHandle(stop),
            thread: Some(thread),
        })
    }

    fn stop(mut self) {
        self.shutdown();
    }
}

impl Watcher {
    fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
            // SAFETY: the stop event is valid until closed below, after the thread has exited.
            let _ = unsafe { SetEvent(self.stop_event.0) };
            let _ = thread.join();
            // SAFETY: the thread no longer uses the event.
            let _ = unsafe { CloseHandle(self.stop_event.0) };
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn watch_loop(change: HANDLE, stop: HANDLE, debounce_ms: u32, on_change: &mut dyn FnMut()) {
    let handles = [change, stop];
    let mut pending = false;
    loop {
        let timeout = if pending { debounce_ms } else { INFINITE };
        // SAFETY: both handles stay valid while this loop runs.
        let r = unsafe { WaitForMultipleObjects(&handles, false, timeout) };
        if r == WAIT_OBJECT_0 {
            pending = true;
            // Re-arm before reading so changes made while we read are not lost.
            // SAFETY: valid change-notification handle.
            if let Err(e) = unsafe { FindNextChangeNotification(change) } {
                log::error!("config watcher stopped: FindNextChangeNotification failed: {e}");
                return;
            }
        } else if r.0 == WAIT_OBJECT_0.0 + 1 {
            return;
        } else if r == WAIT_TIMEOUT {
            pending = false;
            on_change();
        } else {
            log::error!(
                "config watcher stopped: WaitForMultipleObjects returned {:#x}",
                r.0
            );
            return;
        }
    }
}
