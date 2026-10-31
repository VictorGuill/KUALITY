//! Main-thread application state: reacts to tray commands and config reloads.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use kuality_protocol::{CONFIG_VERSION, Config};

use crate::config_store::changed_sections;
use crate::hyperkey::KeyboardController;
use crate::logging;
use crate::platform::{self, DirWatcher, Event};

/// State owned by the main thread.
#[derive(Debug)]
pub struct App<W: DirWatcher> {
    config: Config,
    config_path: PathBuf,
    reloads: Receiver<Box<Config>>,
    watcher: Option<W>,
    keyboard: Option<KeyboardController>,
    /// Tray "Pause" (runtime only, keyboard features only).
    paused: bool,
}

impl<W: DirWatcher> App<W> {
    /// Applies the initial config to the subsystems.
    pub fn new(
        config: Config,
        config_path: PathBuf,
        reloads: Receiver<Box<Config>>,
        watcher: Option<W>,
        keyboard: Option<KeyboardController>,
    ) -> Self {
        let app = Self {
            config,
            config_path,
            reloads,
            watcher,
            keyboard,
            paused: false,
        };
        if let Some(k) = &app.keyboard {
            k.apply(&app.config.hyperkey);
        }
        // Also cleans up after a crash when Hyperkey is now disabled.
        reconcile_office_key(app.config.hyperkey.enabled);
        app
    }

    /// Handles one event from the main loop.
    pub fn handle(&mut self, event: Event) -> ControlFlow<()> {
        match event {
            Event::Wake => {
                while let Ok(new) = self.reloads.try_recv() {
                    self.apply_config(*new);
                }
                ControlFlow::Continue(())
            }
            Event::OpenSettings => {
                // Placeholder until the settings app exists (Phase 5).
                log::info!("opening {}", self.config_path.display());
                if let Err(e) = platform::open_with_default_app(&self.config_path) {
                    log::error!("could not open {}: {e}", self.config_path.display());
                }
                ControlFlow::Continue(())
            }
            Event::TogglePause => {
                self.paused = !self.paused;
                log::info!(
                    "keyboard features {}",
                    if self.paused { "paused" } else { "resumed" }
                );
                if let Some(k) = &self.keyboard {
                    k.set_paused(self.paused);
                }
                platform::set_tray_paused(self.paused);
                ControlFlow::Continue(())
            }
            Event::Quit => {
                log::info!("quitting");
                if let Some(mut k) = self.keyboard.take() {
                    k.shutdown();
                }
                reconcile_office_key(false);
                if let Some(w) = self.watcher.take() {
                    w.stop();
                }
                ControlFlow::Break(())
            }
        }
    }

    fn apply_config(&mut self, new: Config) {
        let changed = changed_sections(&self.config, &new);
        if changed.is_empty() {
            log::info!("config reloaded (no changes)");
            return;
        }
        // Report under the more verbose of the old and new levels, so the line is not lost when
        // `log_level` itself changes (e.g. warn -> info), then switch to the new level.
        log::set_max_level(logging::reload_report_filter(
            log::max_level(),
            new.general.log_level,
        ));
        log::info!("config reloaded, changed: {}", changed.join(", "));
        warn_if_newer(&new);
        logging::set_level(new.general.log_level);
        if let Some(k) = &self.keyboard {
            // On every reload: the keyboard thread reinstalls the hook, which is harmless.
            k.apply(&new.hyperkey);
        }
        if new.hyperkey.enabled != self.config.hyperkey.enabled {
            reconcile_office_key(new.hyperkey.enabled);
        }
        self.config = new;
    }
}

/// Disables the Office key while Hyperkey is enabled, restores it otherwise. Failures are only
/// logged: they never stop Hyperkey.
fn reconcile_office_key(hyperkey_enabled: bool) {
    let (what, result) = if hyperkey_enabled {
        ("disabled", platform::office_key::apply())
    } else {
        ("restored", platform::office_key::restore())
    };
    match result {
        Ok(true) => log::info!("Office key {what}"),
        Ok(false) => {}
        Err(e) => log::warn!("cannot update the Office key override: {e}"),
    }
}

/// Logs a warning if the file was written by a newer Kuality.
pub fn warn_if_newer(config: &Config) {
    if config.version > CONFIG_VERSION {
        log::warn!(
            "config version {} is newer than supported ({CONFIG_VERSION}); loading best-effort, \
             the file will not be written",
            config.version
        );
    }
}
