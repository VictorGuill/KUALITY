//! Actions run by shortcuts. See `docs/specs/shortcuts.md`.
//!
//! `launch` and `open_url` run on the `kuality-actions` worker thread so the keyboard thread
//! never waits for the shell. The thread runs a platform message loop (it is a COM STA and the
//! shell may leave windows on it); jobs travel through a channel and a wake-up message. `send_hotkey` does not come here: it runs on the keyboard thread,
//! which owns the injected Hyper modifiers.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::platform;
use crate::shortcuts::CompiledAction;

/// One action to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// Binding index in `shortcuts.bindings` (for logs).
    pub source: usize,
    pub action: CompiledAction,
}

/// Handle to the action worker thread. The thread is spawned on the first job and runs until
/// [`ActionWorker::shutdown`]. Jobs run one at a time, in order.
#[derive(Debug, Default)]
pub struct ActionWorker {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    tx: Option<Sender<Queued>>,
    thread: Option<Thread>,
    closed: bool,
}

#[derive(Debug)]
struct Thread {
    id: u32,
    handle: JoinHandle<()>,
    done: Receiver<()>,
}

impl ActionWorker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a job, spawning the thread if needed. Never blocks on a running job. Notes the
    /// current foreground window, so a window the job opens is only brought forward if the user
    /// is still there (see [`FOCUS_WAIT`]).
    pub fn submit(&self, job: Job) {
        let before = platform::foreground_window();
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.closed {
            return;
        }
        if inner.tx.is_none() {
            match spawn() {
                Ok((tx, thread)) => {
                    inner.tx = Some(tx);
                    inner.thread = Some(thread);
                }
                Err(e) => {
                    log::error!("cannot start the action thread: {e}");
                    return;
                }
            }
        }
        let sent = inner
            .tx
            .as_ref()
            .is_some_and(|tx| tx.send((job, before)).is_ok())
            && inner
                .thread
                .as_ref()
                .is_some_and(|t| platform::action_thread_wake(t.id));
        if !sent {
            log::error!("action thread is gone; action dropped");
            inner.tx = None;
        }
    }

    /// Stops accepting jobs, lets the thread finish the current one and waits up to `timeout`.
    pub fn shutdown(&self, timeout: Duration) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.closed = true;
        inner.tx = None;
        if let Some(thread) = inner.thread.take() {
            // Queued after any pending wake-ups, so queued jobs still run first.
            platform::action_thread_stop(thread.id);
            match thread.done.recv_timeout(timeout) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = thread.handle.join();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    log::warn!("action thread did not stop within {timeout:?}");
                }
            }
        }
    }
}

type Queued = (Job, platform::Foreground);

fn spawn() -> std::io::Result<(Sender<Queued>, Thread)> {
    let (tx, rx) = mpsc::channel::<Queued>();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let (id_tx, id_rx) = mpsc::sync_channel(1);
    let handle = std::thread::Builder::new()
        .name("kuality-actions".into())
        .spawn(move || {
            let _done = DoneGuard(done_tx);
            let _ = id_tx.send(platform::action_thread_init());
            log::debug!("action thread started");
            platform::action_thread_run(|| {
                while let Ok((job, before)) = rx.try_recv() {
                    run(&job, before);
                }
            });
            log::debug!("action thread stopped");
        })?;
    let id = id_rx
        .recv()
        .map_err(|_| std::io::Error::other("action thread exited during startup"))?;
    Ok((
        tx,
        Thread {
            id,
            handle,
            done: done_rx,
        },
    ))
}

struct DoneGuard(mpsc::SyncSender<()>);

impl Drop for DoneGuard {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

fn run(job: &Job, before: platform::Foreground) {
    let i = job.source;
    match &job.action {
        CompiledAction::Launch { path, params, cwd } => {
            if let Err(e) = platform::launch(path, params, cwd.as_deref(), before) {
                log::error!("shortcut #{i}: launch of {path} failed: {e}");
            }
        }
        CompiledAction::OpenUrl(url) => {
            if let Err(e) = platform::open_url(url, before) {
                log::error!("shortcut #{i}: open_url of {url} failed: {e}");
            }
        }
        CompiledAction::SendHotkey(_) | CompiledAction::OpenClipboard => {
            log::error!("shortcut #{i}: this action must run on the keyboard thread");
        }
    }
}

/// How long the worker watches for the first window of what a `launch` / `open_url` started.
/// The foreground grant given before `ShellExecuteExW` ends at the next input, and releasing
/// Hyper is input: an app whose first window appears after the release (Character Map needs
/// ~250-400 ms) opens behind the current window, so the worker brings it forward itself. Long
/// enough for slow starts, short enough not to surprise the user later on.
pub const FOCUS_WAIT: Duration = Duration::from_secs(3);

/// What the launch watcher does on one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusStep {
    /// Check again later.
    Wait,
    /// Bring the window that was found to the front.
    Raise,
    /// Stop watching.
    Stop,
}

/// One check of the launch watcher, `elapsed` after the launch. Nothing is done once the
/// foreground has changed: the app came forward on its own, or the user moved on.
pub fn focus_step(elapsed: Duration, foreground_unchanged: bool, window_found: bool) -> FocusStep {
    if !foreground_unchanged {
        FocusStep::Stop
    } else if window_found {
        FocusStep::Raise
    } else if elapsed >= FOCUS_WAIT {
        FocusStep::Stop
    } else {
        FocusStep::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_watch_raises_only_while_the_foreground_is_unchanged() {
        let early = Duration::from_millis(300);
        assert_eq!(focus_step(early, true, false), FocusStep::Wait);
        assert_eq!(focus_step(early, true, true), FocusStep::Raise);
        assert_eq!(focus_step(early, false, true), FocusStep::Stop);
        assert_eq!(focus_step(early, false, false), FocusStep::Stop);
        assert_eq!(focus_step(FOCUS_WAIT, true, false), FocusStep::Stop);
        // A window found right at the deadline is still raised.
        assert_eq!(focus_step(FOCUS_WAIT, true, true), FocusStep::Raise);
    }
}
