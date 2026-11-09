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
use crate::updates::Updates;
use crate::{logging, shortcuts};

/// How long quit waits for a running `launch`/`open_url` action.
const ACTIONS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// Arguments that open the settings window: a second process of this same exe. The pid tells it
/// which core started it (it closes when that process ends).
const SETTINGS_ARG: &str = "--settings";
const CORE_PID_ARG: &str = "--core-pid";

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
    updates: Option<Updates>,
    /// Tray "Pause" (runtime only, keyboard features only).
    paused: bool,
}

impl<W: DirWatcher> App<W> {
    /// Applies the initial config to the subsystems.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        config_path: PathBuf,
        reloads: Receiver<Box<Config>>,
        watcher: Option<W>,
        keyboard: Option<KeyboardController>,
        actions: Arc<ActionWorker>,
        overlay: Option<OverlayController>,
        updates: Option<Updates>,
    ) -> Self {
        let app = Self {
            config,
            config_path,
            reloads,
            watcher,
            keyboard,
            actions,
            overlay,
            updates,
            paused: false,
        };
        app.apply_keyboard(&app.config);
        if let Some(o) = &app.overlay {
            o.apply(&app.config.widgets);
        }
        // Also cleans up after a crash when Hyperkey is now disabled.
        reconcile_office_key(office_override_wanted(&app.config));
        app
    }

    /// Starts the settings window (`kuality.exe --settings --core-pid <pid>`, this same exe),
    /// so it closes when the core exits; falls back to opening `config.json` in the default
    /// editor if it cannot be started.
    fn open_settings(&self) {
        match std::env::current_exe() {
            Ok(exe) => match platform::spawn_detached(
                &exe,
                &[
                    SETTINGS_ARG.into(),
                    CORE_PID_ARG.into(),
                    std::process::id().to_string(),
                ],
            ) {
                Ok(()) => {
                    log::info!("started the settings window");
                    return;
                }
                Err(e) => log::error!("could not start the settings window: {e}"),
            },
            Err(e) => log::error!("cannot find our own exe: {e}"),
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
                if let Some(u) = &mut self.updates {
                    u.poll();
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
            Event::CheckUpdates => {
                if let Some(u) = &self.updates {
                    u.check_now();
                }
                ControlFlow::Continue(())
            }
            Event::InstallUpdate => {
                if let Some(u) = &mut self.updates {
                    u.install();
                }
                ControlFlow::Continue(())
            }
            Event::Quit => {
                log::info!("quitting");
                if let Some(u) = &self.updates {
                    u.shutdown();
                }
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
        if office_override_wanted(&new) != office_override_wanted(&self.config) {
            reconcile_office_key(office_override_wanted(&new));
        }
        // Only when the section changed: an Apply also brings back widgets hidden from the menu.
        if new.widgets != self.config.widgets
            && let Some(o) = &self.overlay
        {
            o.apply(&new.widgets);
        }
        if new.updates.check != self.config.updates.check
            && let Some(u) = &self.updates
        {
            u.set_enabled(new.updates.check);
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
        if compiled.has_hyper() && !config.hyperkey.enabled {
            log::warn!("Hyper shortcuts need Hyperkey (hyperkey.enabled) to work");
        }
        k.apply(&config.hyperkey, Arc::new(compiled));
    }
}

/// The Office key override is only needed when Hyper presses Ctrl+Shift+Alt+Win (the Office
/// key combination); without Win, Hyper never produces it.
fn office_override_wanted(config: &Config) -> bool {
    config.hyperkey.enabled && config.hyperkey.modifiers.includes_win()
}

/// Disables the Office key while Hyperkey (with Win) is enabled, restores it otherwise. Failures are only
/// logged: they never stop Hyperkey.
fn reconcile_office_key(wanted: bool) {
    let (what, result) = if wanted {
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
