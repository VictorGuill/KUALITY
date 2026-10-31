//! Actions run by shortcuts. See `docs/specs/shortcuts.md`.
//!
//! `launch` and `open_url` run on the `kuality-actions` worker thread so the keyboard thread
//! never waits for the shell. `send_hotkey` does not come here: it runs on the keyboard thread,
//! which owns the injected Hyper modifiers.

pub mod quote;

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
    tx: Option<Sender<Job>>,
    thread: Option<(JoinHandle<()>, Receiver<()>)>,
    closed: bool,
}

impl ActionWorker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a job, spawning the thread if needed. Never blocks on a running job.
    pub fn submit(&self, job: Job) {
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
        if let Some(tx) = &inner.tx
            && tx.send(job).is_err()
        {
            log::error!("action thread is gone; action dropped");
            inner.tx = None;
        }
    }

    /// Stops accepting jobs, lets the thread finish the current one and waits up to `timeout`.
    pub fn shutdown(&self, timeout: Duration) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.closed = true;
        // Dropping the sender ends the thread's loop once the queue is empty.
        inner.tx = None;
        if let Some((thread, done)) = inner.thread.take() {
            match done.recv_timeout(timeout) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = thread.join();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    log::warn!("action thread did not stop within {timeout:?}");
                }
            }
        }
    }
}

type Spawned = (Sender<Job>, (JoinHandle<()>, Receiver<()>));

fn spawn() -> std::io::Result<Spawned> {
    let (tx, rx) = mpsc::channel::<Job>();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("kuality-actions".into())
        .spawn(move || {
            let _done = DoneGuard(done_tx);
            platform::init_action_thread();
            log::debug!("action thread started");
            while let Ok(job) = rx.recv() {
                run(&job);
            }
            log::debug!("action thread stopped");
        })?;
    Ok((tx, (thread, done_rx)))
}

struct DoneGuard(mpsc::SyncSender<()>);

impl Drop for DoneGuard {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

fn run(job: &Job) {
    let i = job.source;
    match &job.action {
        CompiledAction::Launch { path, params, cwd } => {
            if let Err(e) = platform::launch(path, params, cwd.as_deref()) {
                log::error!("shortcut #{i}: launch of {path} failed: {e}");
            }
        }
        CompiledAction::OpenUrl(url) => {
            if let Err(e) = platform::open_url(url) {
                log::error!("shortcut #{i}: open_url of {url} failed: {e}");
            }
        }
        CompiledAction::SendHotkey(_) => {
            log::error!("shortcut #{i}: send_hotkey must run on the keyboard thread");
        }
    }
}
