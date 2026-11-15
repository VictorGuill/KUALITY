//! App lock (`docs/specs/app-lock.md`).
//!
//! While `app_lock.enabled`, [`AppLockService`] runs the App lock thread (hides the windows of
//! locked programs and opens the prompt, see `platform::windows::app_lock`) and gives the core's
//! pipe a handler for the prompt, the settings tab and the tray confirmations. The password is
//! checked here, on the pipe thread ([`password::Vault`]).

pub mod book;
pub mod password;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use kuality_protocol::app_lock::{ConfirmAction, Request, Response, normalize_path};
use kuality_protocol::{AppLock, cli};

use crate::clipboard::sealed::Plain;
use crate::ipc::AppLockHandler;
use crate::platform::{self, AppLockCommand, AppLockThread, Commander, LockedExe};
use password::{Refused, Vault};

/// How long stopping waits for the thread.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// How long an unlock waits for the thread to show the windows again.
const UNLOCK_TIMEOUT: Duration = Duration::from_secs(2);
/// A second prompt is not started this soon after one (it takes a moment to claim its mutex).
const PROMPT_START_GUARD: Duration = Duration::from_millis(1500);

/// What App lock asks the main thread to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppLockEvent {
    /// The password was typed for a tray command.
    Confirmed(ConfirmAction),
}

/// Wakes the main loop (it then reads the events).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The programs of the config, as the thread wants them.
fn locked_exes(config: &AppLock) -> Vec<LockedExe> {
    let mut out: Vec<LockedExe> = Vec::new();
    for app in &config.apps {
        let key = normalize_path(&app.path);
        if !key.is_empty() && !out.iter().any(|a| a.key == key) {
            out.push(LockedExe {
                key,
                path: app.path.trim().to_string(),
            });
        }
    }
    out
}

/// The running App lock (exists only while the feature is enabled).
pub struct AppLockService {
    thread: Option<AppLockThread>,
    commander: Commander,
    vault: Arc<Mutex<Vault>>,
    /// Mirrors `vault.has_password()` without waiting for a password check in progress.
    has_password: Arc<AtomicBool>,
    /// The programs last applied (the pipe handler re-applies them when the first password is
    /// set).
    apps: Arc<Mutex<Vec<LockedExe>>>,
    events: Sender<AppLockEvent>,
    wake: Wake,
}

impl std::fmt::Debug for AppLockService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppLockService")
            .field("has_password", &self.has_password())
            .finish_non_exhaustive()
    }
}

impl AppLockService {
    /// Reads the password file and starts the thread. `initial`: Kuality is starting, so running
    /// locked programs are locked at once (else they stay unlocked until they exit).
    pub fn start(
        config: &AppLock,
        secret_path: PathBuf,
        hidden_path: PathBuf,
        initial: bool,
        events: Sender<AppLockEvent>,
        wake: Wake,
    ) -> io::Result<Self> {
        let vault = Vault::load(secret_path);
        let has_password = Arc::new(AtomicBool::new(vault.has_password()));
        if !vault.has_password() {
            log::info!("App lock is on but has no password yet: nothing is locked");
        }
        let thread = AppLockThread::start(hidden_path, prompt_starter())?;
        let commander = thread.commander();
        let apps = locked_exes(config);
        commander.send(AppLockCommand::Apply {
            apps: apps.clone(),
            armed: vault.has_password(),
            initial,
        });
        log::info!("App lock on ({} program(s))", apps.len());
        Ok(Self {
            thread: Some(thread),
            commander,
            vault: Arc::new(Mutex::new(vault)),
            has_password,
            apps: Arc::new(Mutex::new(apps)),
            events,
            wake,
        })
    }

    /// Applies a changed `app_lock` section (while it stays enabled).
    pub fn apply(&self, config: &AppLock) {
        let apps = locked_exes(config);
        *lock(&self.apps) = apps.clone();
        self.commander.send(AppLockCommand::Apply {
            apps,
            armed: self.has_password(),
            initial: false,
        });
    }

    /// Tray Pause.
    pub fn set_paused(&self, paused: bool) {
        self.commander.send(AppLockCommand::SetPaused(paused));
    }

    /// A password is set: tray Quit / Pause need it.
    pub fn has_password(&self) -> bool {
        self.has_password.load(Ordering::SeqCst)
    }

    /// The pipe handler: status, unlock, verify, confirm, set password.
    pub fn ipc_handler(&self) -> AppLockHandler {
        let vault = Arc::clone(&self.vault);
        let has_password = Arc::clone(&self.has_password);
        let apps = Arc::clone(&self.apps);
        let commander = self.commander.clone();
        let events = self.events.clone();
        let wake = Arc::clone(&self.wake);
        Box::new(move |mut request| {
            let response = handle(
                &vault,
                &commander,
                &apps,
                &events,
                &wake,
                &request,
                Instant::now(),
            );
            has_password.store(lock(&vault).has_password(), Ordering::SeqCst);
            for p in request.passwords_mut() {
                drop(Plain::from_string(std::mem::take(p)));
            }
            response
        })
    }

    /// Shows every hidden window again and stops the thread.
    pub fn shutdown(&mut self) {
        if let Some(t) = self.thread.take() {
            t.stop(STOP_TIMEOUT);
            log::info!("App lock off");
        }
    }
}

impl Drop for AppLockService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Answers one App lock request.
fn handle(
    vault: &Mutex<Vault>,
    commander: &Commander,
    apps: &Mutex<Vec<LockedExe>>,
    events: &Sender<AppLockEvent>,
    wake: &Wake,
    request: &Request,
    now: Instant,
) -> Response {
    let mut vault = lock(vault);
    let status = |vault: &Vault, refused: Option<Refused>| Response {
        ok: refused.is_none(),
        error: refused.as_ref().map(Refused::message),
        has_password: vault.has_password(),
        retry_after_secs: vault.wait(now).map_or(0, |d| d.as_secs().max(1)),
    };
    let checked = |vault: &mut Vault, password: &str| vault.check(password, now).err();
    match request {
        Request::AppLockStatus => status(&vault, None),
        Request::AppLockVerify { password } => {
            let refused = checked(&mut vault, password);
            status(&vault, refused)
        }
        Request::AppLockUnlock { app, password } => {
            let refused = checked(&mut vault, password);
            if refused.is_none() {
                let (done_tx, done_rx) = mpsc::sync_channel(1);
                commander.send(AppLockCommand::Unlock {
                    key: normalize_path(app),
                    done: done_tx,
                });
                if done_rx.recv_timeout(UNLOCK_TIMEOUT).is_err() {
                    log::warn!("App lock: the thread did not confirm the unlock in time");
                }
            } else {
                log::info!("App lock: a wrong password was typed to unlock {app}");
            }
            status(&vault, refused)
        }
        Request::AppLockConfirm { action, password } => {
            let refused = checked(&mut vault, password);
            if refused.is_none() && events.send(AppLockEvent::Confirmed(*action)).is_ok() {
                wake();
            }
            status(&vault, refused)
        }
        Request::AppLockSetPassword { current, new } => match vault.set(current, new, now) {
            Ok(first) => {
                if first {
                    // Locking starts now; programs running at this moment stay unlocked.
                    commander.send(AppLockCommand::Apply {
                        apps: lock(apps).clone(),
                        armed: true,
                        initial: false,
                    });
                }
                status(&vault, None)
            }
            Err(refused) => status(&vault, Some(refused)),
        },
    }
}

/// What the thread calls when it hid a window: bring the open prompt to the front, or start one
/// (`kuality.exe --unlock --app <path> --core-pid <pid>`).
fn prompt_starter() -> platform::Prompt {
    let mut started: Option<Instant> = None;
    Box::new(move |path: &str| {
        if kuality_settings::unlock_popup_running() {
            kuality_settings::focus_unlock_popup();
            return;
        }
        if started.is_some_and(|t| t.elapsed() < PROMPT_START_GUARD) {
            return;
        }
        started = Some(Instant::now());
        let args = [
            cli::UNLOCK.into(),
            cli::APP.into(),
            path.to_string(),
            cli::CORE_PID.into(),
            std::process::id().to_string(),
        ];
        match std::env::current_exe().and_then(|exe| platform::spawn_detached(&exe, &args)) {
            Ok(()) => log::debug!("started the App lock prompt"),
            Err(e) => log::error!("could not start the App lock prompt: {e}"),
        }
    })
}

/// Starts the prompt for a tray command (`kuality.exe --unlock --confirm <action>`).
pub fn start_confirm_prompt(action: ConfirmAction) {
    if kuality_settings::unlock_popup_running() {
        kuality_settings::focus_unlock_popup();
        return;
    }
    let args = [
        cli::UNLOCK.into(),
        cli::CONFIRM.into(),
        action.as_arg().into(),
        cli::CORE_PID.into(),
        std::process::id().to_string(),
    ];
    match std::env::current_exe().and_then(|exe| platform::spawn_detached(&exe, &args)) {
        Ok(()) => log::debug!("started the App lock prompt ({})", action.as_arg()),
        Err(e) => log::error!("could not start the App lock prompt: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuality_protocol::LockedApp;

    #[test]
    fn configured_programs_are_normalized_and_deduplicated() {
        let app = |path: &str| LockedApp {
            path: path.into(),
            ..LockedApp::default()
        };
        let config = AppLock {
            enabled: true,
            apps: vec![
                app(" C:/B/Brave.exe "),
                app("c:\\b\\brave.EXE"),
                app("  "),
                app("D:\\n.exe"),
            ],
            ..AppLock::default()
        };
        let exes = locked_exes(&config);
        assert_eq!(
            exes,
            vec![
                LockedExe {
                    key: "c:\\b\\brave.exe".into(),
                    path: "C:/B/Brave.exe".into()
                },
                LockedExe {
                    key: "d:\\n.exe".into(),
                    path: "D:\\n.exe".into()
                },
            ]
        );
    }
}
