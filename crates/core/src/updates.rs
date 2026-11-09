//! Update checks (`docs/specs/updates.md`).
//!
//! The `kuality-update` thread owns the network work ([`kuality_update`]): it checks GitHub
//! [`FIRST_CHECK_DELAY`] after start, then every [`CHECK_INTERVAL`] ([`RETRY_AFTER_ERROR`] after
//! a failure), and on request; it downloads and starts the installer when asked. Between checks
//! it sleeps in a channel wait (no polling); the wall clock is re-read at least every
//! [`MAX_WAIT`], so a check that fell due while the PC slept runs soon after resume. Results go
//! to the main thread, which shows them in the tray ([`Updates`]).

use std::io;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime};

use kuality_update::{Checker, Release, Version};

use crate::platform::{self, Event, Waker};

pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const RETRY_AFTER_ERROR: Duration = Duration::from_secs(30 * 60);
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug)]
enum Command {
    /// Automatic checks on or off (`updates.check`).
    SetEnabled(bool),
    /// Check now (tray "Check for updates"), even when automatic checks are off.
    Check,
    Install(Release),
    Shutdown,
}

#[derive(Debug)]
enum Report {
    Checked {
        result: Result<Option<Release>, String>,
        manual: bool,
    },
    /// The installer is running; it closes Kuality.
    Installing(Version),
    InstallFailed(String),
}

/// How long the thread may wait for a command before looking at the clock again; `None` waits
/// for a command only (automatic checks off).
fn wait_for(next_due: Option<SystemTime>, now: SystemTime) -> Option<Duration> {
    next_due.map(|due| {
        due.duration_since(now)
            .unwrap_or(Duration::ZERO)
            .min(MAX_WAIT)
    })
}

fn run(commands: Receiver<Command>, reports: Sender<Report>, waker: impl Waker) {
    kuality_update::clean_downloads();
    let report = |r: Report| {
        if reports.send(r).is_ok() {
            waker.wake();
        }
    };
    let mut checker = Checker::new();
    let mut next_due: Option<SystemTime> = None;
    loop {
        let cmd = match wait_for(next_due, SystemTime::now()) {
            None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(wait) => commands.recv_timeout(wait),
        };
        let manual = match cmd {
            Ok(Command::SetEnabled(true)) => {
                if next_due.is_none() {
                    next_due = Some(SystemTime::now() + FIRST_CHECK_DELAY);
                }
                continue;
            }
            Ok(Command::SetEnabled(false)) => {
                next_due = None;
                continue;
            }
            Ok(Command::Check) => true,
            Ok(Command::Install(release)) => {
                log::info!("downloading Kuality {}", release.version);
                report(match kuality_update::download_and_install(&release) {
                    Ok(()) => Report::Installing(release.version),
                    Err(e) => Report::InstallFailed(e),
                });
                continue;
            }
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                if next_due.is_none_or(|due| SystemTime::now() < due) {
                    continue;
                }
                false
            }
        };
        let result = checker.check();
        match &result {
            Ok(Some(r)) => log::info!("update available: Kuality {}", r.version),
            Ok(None) => log::info!(
                "no update available (running {})",
                kuality_update::current_version()
            ),
            Err(e) => log::warn!("update check failed: {e}"),
        }
        if next_due.is_some() {
            let after = if result.is_ok() {
                CHECK_INTERVAL
            } else {
                RETRY_AFTER_ERROR
            };
            next_due = Some(SystemTime::now() + after);
        }
        report(Report::Checked { result, manual });
    }
}

/// Main-thread side: owns the thread's channels and what the tray shows.
#[derive(Debug)]
pub struct Updates {
    commands: Sender<Command>,
    reports: Receiver<Report>,
    /// The newest release found by the last check.
    available: Option<Release>,
    /// Last version announced with a notification (one per version and run).
    notified: Option<Version>,
    /// A download/install is running.
    busy: bool,
    /// This is the installed copy: the installer can replace it. Otherwise "install" opens the
    /// release page.
    self_update: bool,
}

impl Updates {
    /// Spawns the thread. `enabled` = `updates.check`.
    pub fn start(waker: impl Waker, enabled: bool) -> io::Result<Self> {
        let (commands, commands_rx) = mpsc::channel();
        let (reports_tx, reports) = mpsc::channel();
        // Never joined: a download may be running at quit; the process exit ends it.
        std::thread::Builder::new()
            .name("kuality-update".into())
            .spawn(move || run(commands_rx, reports_tx, waker))?;
        let self_update = kuality_update::can_self_update();
        log::info!(
            "updates: automatic checks {}, {}",
            if enabled { "on" } else { "off" },
            if self_update {
                "installed copy (self-update)"
            } else {
                "not the installed copy (release page only)"
            }
        );
        let updates = Self {
            commands,
            reports,
            available: None,
            notified: None,
            busy: false,
            self_update,
        };
        updates.set_enabled(enabled);
        Ok(updates)
    }

    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.commands.send(Command::SetEnabled(enabled));
    }

    /// Tray "Check for updates".
    pub fn check_now(&self) {
        let _ = self.commands.send(Command::Check);
    }

    /// Tray "Install update" or a click on the notification.
    pub fn install(&mut self) {
        let Some(release) = self.available.clone() else {
            self.check_now();
            return;
        };
        if !self.self_update {
            log::info!("opening {}", release.page_url);
            if let Err(e) = platform::open_with_default_app(Path::new(&release.page_url)) {
                log::error!("cannot open {}: {e}", release.page_url);
            }
            return;
        }
        if self.busy {
            return;
        }
        self.busy = true;
        platform::show_notification(
            &format!("Updating to Kuality {}", release.version),
            "Downloading… Kuality restarts when the update is installed.",
            None,
        );
        let _ = self.commands.send(Command::Install(release));
    }

    /// Handles the thread's reports (call on every main-loop wake).
    pub fn poll(&mut self) {
        while let Ok(report) = self.reports.try_recv() {
            match report {
                Report::Checked { result, manual } => self.checked(result, manual),
                Report::Installing(v) => log::info!("installer for Kuality {v} started"),
                Report::InstallFailed(e) => {
                    log::error!("update failed: {e}");
                    self.busy = false;
                    platform::show_notification("Kuality update failed", &e, None);
                }
            }
        }
    }

    fn checked(&mut self, result: Result<Option<Release>, String>, manual: bool) {
        match result {
            Ok(Some(release)) => {
                platform::set_tray_update(Some(&release.version.to_string()));
                if manual || self.notified.as_ref() != Some(&release.version) {
                    let action = if self.self_update {
                        "Click to install it."
                    } else {
                        "Click to download it."
                    };
                    platform::show_notification(
                        &format!("Kuality {} is available", release.version),
                        &format!("You have {}. {action}", kuality_update::current_version()),
                        Some(Event::InstallUpdate),
                    );
                    self.notified = Some(release.version.clone());
                }
                self.available = Some(release);
            }
            Ok(None) => {
                platform::set_tray_update(None);
                self.available = None;
                if manual {
                    platform::show_notification(
                        "Kuality is up to date",
                        &format!("Version {}.", kuality_update::current_version()),
                        None,
                    );
                }
            }
            Err(e) => {
                if manual {
                    platform::show_notification("Could not check for updates", &e, None);
                }
            }
        }
    }

    pub fn shutdown(&self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_commands_only_when_disabled() {
        assert_eq!(wait_for(None, SystemTime::now()), None);
    }

    #[test]
    fn wait_is_capped_and_never_negative() {
        let now = SystemTime::now();
        assert_eq!(wait_for(Some(now + CHECK_INTERVAL), now), Some(MAX_WAIT));
        assert_eq!(
            wait_for(Some(now + Duration::from_secs(5)), now),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            wait_for(Some(now - Duration::from_secs(5)), now),
            Some(Duration::ZERO)
        );
    }
}
