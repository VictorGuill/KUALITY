//! Main-thread application state: reacts to tray commands and config reloads.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use kuality_protocol::{CONFIG_VERSION, Config};

use crate::config_store::changed_sections;
use crate::logging;
use crate::platform::{self, DirWatcher, Event};

/// State owned by the main thread.
#[derive(Debug)]
pub struct App<W: DirWatcher> {
    config: Config,
    config_path: PathBuf,
    reloads: Receiver<Box<Config>>,
    watcher: Option<W>,
}

impl<W: DirWatcher> App<W> {
    pub fn new(
        config: Config,
        config_path: PathBuf,
        reloads: Receiver<Box<Config>>,
        watcher: Option<W>,
    ) -> Self {
        Self {
            config,
            config_path,
            reloads,
            watcher,
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
                // Placeholder until the settings app exists (Phase 5).
                log::info!("opening {}", self.config_path.display());
                if let Err(e) = platform::open_with_default_app(&self.config_path) {
                    log::error!("could not open {}: {e}", self.config_path.display());
                }
                ControlFlow::Continue(())
            }
            Event::Quit => {
                log::info!("quitting");
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
        log::info!("config reloaded, changed: {}", changed.join(", "));
        warn_if_newer(&new);
        logging::set_level(new.general.log_level);
        self.config = new;
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
