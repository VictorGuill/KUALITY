//! Main-thread application state: reacts to tray commands and config reloads.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use kuality_protocol::{CONFIG_VERSION, Config};

use crate::actions::ActionWorker;
use crate::config_store::changed_sections;
use crate::hyperkey::KeyboardController;
use crate::overlay::OverlayController;
use crate::platform::{self, DirWatcher, Event};
use crate::{logging, shortcuts};

/// How long quit waits for a running `launch`/`open_url` action.
const ACTIONS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// File stem of the settings app, shipped next to the core.
const SETTINGS_EXE_STEM: &str = "kuality-settings";

/// Path of the settings app next to the running exe, if it exists.
fn settings_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.with_file_name(format!(
        "{SETTINGS_EXE_STEM}{}",
        std::env::consts::EXE_SUFFIX
    ));
    path.is_file().then_some(path)
}

/// State owned by the main thread.
#[derive(Debug)]
pub struct App<W: DirWatcher> {
    config: Config,
    config_path: PathBuf,
    reloads: Receiver<Box<Config>>,
    watcher: Option<W>,
    keyboard: Option<KeyboardController>,
    actions: Arc<ActionWorker>,
    overlay: Option<OverlayController>,
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
        actions: Arc<ActionWorker>,
        overlay: Option<OverlayController>,
    ) -> Self {
        let app = Self {
            config,
            config_path,
            reloads,
            watcher,
            keyboard,
            actions,
            overlay,
            paused: false,
        };
        app.apply_keyboard(&app.config);
        if let Some(o) = &app.overlay {
            o.apply(&app.config.widgets);
        }
        // Also cleans up after a crash when Hyperkey is now disabled.
        reconcile_office_key(app.config.hyperkey.enabled);
        app
    }

    /// Starts `kuality-settings.exe` from the folder of the running exe; falls back to opening
    /// `config.json` in the default editor if it is missing or cannot be started.
    fn open_settings(&self) {
        match settings_exe() {
            Some(exe) => match platform::spawn_detached(&exe) {
                Ok(()) => {
                    log::info!("started {}", exe.display());
                    return;
                }
                Err(e) => log::error!("could not start {}: {e}", exe.display()),
            },
            None => log::warn!("{SETTINGS_EXE_STEM} not found next to the core"),
        }
        log::info!("opening {}", self.config_path.display());
        if let Err(e) = platform::open_with_default_app(&self.config_path) {
            log::error!("could not open {}: {e}", self.config_path.display());
        }
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
                self.open_settings();
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
                if let Some(mut o) = self.overlay.take() {
                    o.shutdown();
                }
                if let Some(mut k) = self.keyboard.take() {
                    k.shutdown();
                }
                // After the keyboard thread: no new actions can arrive.
                self.actions.shutdown(ACTIONS_SHUTDOWN_TIMEOUT);
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
        // On every reload: the keyboard thread reinstalls the hook, which is harmless.
        self.apply_keyboard(&new);
        if new.hyperkey.enabled != self.config.hyperkey.enabled {
            reconcile_office_key(new.hyperkey.enabled);
        }
        // Only when the section changed: an Apply also brings back widgets hidden from the menu.
        if new.widgets != self.config.widgets
            && let Some(o) = &self.overlay
        {
            o.apply(&new.widgets);
        }
        self.config = new;
    }
}

impl<W: DirWatcher> App<W> {
    /// Sends the Hyperkey settings and the compiled shortcuts to the keyboard thread.
    fn apply_keyboard(&self, config: &Config) {
        let Some(k) = &self.keyboard else {
            return;
        };
        let compiled = shortcuts::compile(&config.shortcuts);
        if config.shortcuts.enabled && !config.hyperkey.enabled {
            log::warn!("shortcuts are enabled but need Hyperkey (hyperkey.enabled) to work");
        }
        k.apply(&config.hyperkey, Arc::new(compiled));
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
