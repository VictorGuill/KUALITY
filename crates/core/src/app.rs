//! Main-thread application state: reacts to tray commands and config reloads.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use kuality_protocol::{CONFIG_VERSION, Config};

use crate::actions::ActionWorker;
use crate::clipboard::ClipboardService;
use crate::config_store::changed_sections;
use crate::hyperkey::KeyboardController;
use crate::overlay::OverlayController;
use crate::overlay::widgets::LiveData;
use crate::platform::{self, DirWatcher, Event, TodoRequest, TodoTrigger};
use crate::updates::Updates;
use crate::{logging, shortcuts};

/// How long quit waits for a running `launch`/`open_url` action.
const ACTIONS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// Arguments that open the settings window: a second process of this same exe. The pid tells it
/// which core started it (it closes when that process ends).
const SETTINGS_ARG: &str = "--settings";
const CORE_PID_ARG: &str = "--core-pid";
/// Arguments that open the to-do popup (`--at <x>,<y>`: anchor in screen pixels).
const TODO_ARG: &str = "--todo";
const AT_ARG: &str = "--at";
/// `--hover <left>,<top>,<right>,<bottom>`: the popup shows on hover and closes when the mouse
/// leaves this module rectangle (physical px) and the popup.
const HOVER_ARG: &str = "--hover";
/// A hover does not start a second popup this soon after starting one.
const TODO_START_GUARD: Duration = Duration::from_millis(500);
/// Arguments that open the clipboard history popup; `--paste` tells it Enter pastes into the
/// window that was in front (opened by the hotkey).
const CLIPBOARD_ARG: &str = "--clipboard";
const PASTE_ARG: &str = "--paste";
/// The hotkey does not start a second clipboard popup this soon after starting one.
const CLIPBOARD_START_GUARD: Duration = Duration::from_millis(500);

/// State owned by the main thread.
#[derive(Debug)]
pub struct App<W: DirWatcher> {
    config: Config,
    config_path: PathBuf,
    /// `clipboard-pins.bin`.
    pins_path: PathBuf,
    reloads: Receiver<Box<Config>>,
    /// Pending to-do count whenever `todos.json` changes (watcher thread).
    todo_counts: Receiver<Option<usize>>,
    watcher: Option<W>,
    keyboard: Option<KeyboardController>,
    actions: Arc<ActionWorker>,
    overlay: Option<OverlayController>,
    updates: Option<Updates>,
    /// Live widget data last sent to the overlay.
    live: LiveData,
    /// When the to-do popup was last started (it takes a moment to claim its mutex).
    todo_started: Option<Instant>,
    /// Clipboard history, while `clipboard.enabled`.
    clipboard: Option<ClipboardService>,
    /// When the clipboard popup was last started.
    clipboard_started: Option<Instant>,
    /// Tray "Pause" (runtime only, keyboard features only).
    paused: bool,
}

impl<W: DirWatcher> App<W> {
    /// Applies the initial config to the subsystems.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        config_path: PathBuf,
        pins_path: PathBuf,
        reloads: Receiver<Box<Config>>,
        todo_counts: Receiver<Option<usize>>,
        watcher: Option<W>,
        keyboard: Option<KeyboardController>,
        actions: Arc<ActionWorker>,
        overlay: Option<OverlayController>,
        updates: Option<Updates>,
    ) -> Self {
        let mut app = Self {
            config,
            config_path,
            pins_path,
            reloads,
            todo_counts,
            watcher,
            keyboard,
            actions,
            overlay,
            updates,
            live: LiveData::default(),
            todo_started: None,
            clipboard: None,
            clipboard_started: None,
            paused: false,
        };
        app.apply_clipboard(None);
        app.apply_keyboard(&app.config);
        app.update_live();
        if let Some(o) = &app.overlay {
            o.apply(&app.config.widgets, app.config.todo.enabled);
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

    /// A click on, or the mouse resting on, a bar's to-do module (`kuality.exe --todo`, this
    /// same exe, is the popup):
    /// - click while the list is only shown on hover: keep it open (bring it to the front);
    /// - click while it is (or was, at the mouse press) open: close it;
    /// - click otherwise: open it above the module;
    /// - hover: show it until the mouse leaves the module and the list, unless one is open.
    fn on_todo(&mut self, r: TodoRequest) {
        match r.trigger {
            TodoTrigger::Click {
                was_peeking: true, ..
            } if kuality_settings::focus_todo_popup() => {
                log::debug!("to-do popup kept open");
                return;
            }
            TodoTrigger::Click {
                was_open,
                was_peeking: false,
            } if kuality_settings::close_todo_popup() || was_open => {
                log::debug!("to-do popup closed");
                return;
            }
            TodoTrigger::Hover
                if kuality_settings::todo_popup_running()
                    || self
                        .todo_started
                        .is_some_and(|t| t.elapsed() < TODO_START_GUARD) =>
            {
                return;
            }
            _ => {}
        }
        let m = r.module;
        let mut args = vec![
            TODO_ARG.into(),
            CORE_PID_ARG.into(),
            std::process::id().to_string(),
            AT_ARG.into(),
            format!("{},{}", m.x, r.taskbar_top),
        ];
        if r.trigger == TodoTrigger::Hover {
            args.push(HOVER_ARG.into());
            args.push(format!("{},{},{},{}", m.x, m.y, m.x + m.w, m.y + m.h));
        }
        let started = std::env::current_exe().and_then(|exe| platform::spawn_detached(&exe, &args));
        match started {
            Ok(()) => {
                self.todo_started = Some(Instant::now());
                log::debug!("started the to-do popup");
            }
            Err(e) => log::error!("could not start the to-do popup: {e}"),
        }
    }

    /// Starts, updates or stops the clipboard history to match the config (`old`: the section
    /// before a reload, `None` at start).
    fn apply_clipboard(&mut self, old: Option<&kuality_protocol::Clipboard>) {
        let new = &self.config.clipboard;
        match (&mut self.clipboard, new.enabled) {
            (None, true) => match ClipboardService::start(new, self.pins_path.clone()) {
                Ok(s) => self.clipboard = Some(s),
                Err(e) => log::error!("cannot start the clipboard history: {e}"),
            },
            (Some(_), false) => {
                if let Some(mut s) = self.clipboard.take() {
                    s.shutdown();
                }
                kuality_settings::close_clipboard_popup();
                log::info!("clipboard history off; history cleared");
            }
            (Some(s), true) if old != Some(new) => s.apply(new),
            _ => {}
        }
    }

    /// The clipboard hotkey: close the popup if it is open, else open it (Enter pastes into the
    /// window in front now).
    fn on_clipboard(&mut self) {
        if self.clipboard.is_none() {
            return;
        }
        if kuality_settings::close_clipboard_popup()
            || self
                .clipboard_started
                .is_some_and(|t| t.elapsed() < CLIPBOARD_START_GUARD)
        {
            log::debug!("clipboard popup closed");
            return;
        }
        // The Start menu and other shell flyouts sit above every topmost window: close them
        // first, so the popup is on top and Enter pastes into the app behind them.
        platform::close_shell_flyout();
        platform::set_paste_target(platform::foreground_window());
        let args = [
            CLIPBOARD_ARG.into(),
            CORE_PID_ARG.into(),
            std::process::id().to_string(),
            PASTE_ARG.into(),
        ];
        let started = std::env::current_exe().and_then(|exe| platform::spawn_detached(&exe, &args));
        match started {
            Ok(()) => {
                self.clipboard_started = Some(Instant::now());
                log::debug!("started the clipboard popup");
            }
            Err(e) => log::error!("could not start the clipboard popup: {e}"),
        }
    }

    /// Takes the latest to-do count from the watcher and sends it to the overlay if it changed.
    fn update_live(&mut self) {
        let mut live = self.live;
        while let Ok(count) = self.todo_counts.try_recv() {
            live.todo_pending = count;
        }
        if live != self.live {
            self.live = live;
            if let Some(o) = &self.overlay {
                o.set_data(live);
            }
        }
    }

    /// Handles one event from the main loop.
    pub fn handle(&mut self, event: Event) -> ControlFlow<()> {
        match event {
            Event::Wake => {
                while let Ok(new) = self.reloads.try_recv() {
                    self.apply_config(*new);
                }
                self.update_live();
                if let Some(u) = &mut self.updates {
                    u.poll();
                }
                ControlFlow::Continue(())
            }
            Event::OpenSettings => {
                self.open_settings();
                ControlFlow::Continue(())
            }
            Event::Todo(r) => {
                self.on_todo(r);
                ControlFlow::Continue(())
            }
            Event::Clipboard => {
                self.on_clipboard();
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
                if let Some(mut c) = self.clipboard.take() {
                    c.shutdown();
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
        // Only when it matters to the keyboard thread: an `Apply` releases held Hyper modifiers
        // and reinstalls the hook, and the settings window saves on every edit.
        if keyboard_config_changed(&self.config, &new) {
            self.apply_keyboard(&new);
        }
        if office_override_wanted(&new) != office_override_wanted(&self.config) {
            reconcile_office_key(office_override_wanted(&new));
        }
        // Only when the section changed: an Apply also brings back widgets hidden from the menu.
        // `todo.enabled` shows or hides the bars' to-do modules.
        if (new.widgets != self.config.widgets || new.todo.enabled != self.config.todo.enabled)
            && let Some(o) = &self.overlay
        {
            o.apply(&new.widgets, new.todo.enabled);
        }
        if new.updates.check != self.config.updates.check
            && let Some(u) = &self.updates
        {
            u.set_enabled(new.updates.check);
        }
        let old = std::mem::replace(&mut self.config, new);
        self.apply_clipboard(Some(&old.clipboard));
    }
}

impl<W: DirWatcher> App<W> {
    /// Sends the Hyperkey settings and the compiled shortcuts to the keyboard thread.
    fn apply_keyboard(&self, config: &Config) {
        let Some(k) = &self.keyboard else {
            return;
        };
        let compiled = shortcuts::compile(&config.shortcuts, &config.clipboard);
        if compiled.has_hyper() && !config.hyperkey.enabled {
            log::warn!("Hyper shortcuts need Hyperkey (hyperkey.enabled) to work");
        }
        k.apply(&config.hyperkey, Arc::new(compiled));
    }
}

/// Whether a reload changes what the keyboard thread works with: Hyperkey, the shortcuts, or
/// the clipboard hotkey (compiled into the shortcut table while the history is enabled).
fn keyboard_config_changed(old: &Config, new: &Config) -> bool {
    old.hyperkey != new.hyperkey
        || old.shortcuts != new.shortcuts
        || old.clipboard.enabled != new.clipboard.enabled
        || old.clipboard.hotkey != new.clipboard.hotkey
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

#[cfg(test)]
mod tests {
    use super::*;
    use kuality_protocol::{Lenient, LogLevel};

    #[test]
    fn only_keyboard_settings_reach_the_keyboard_thread() {
        let old = Config::default();
        assert!(!keyboard_config_changed(&old, &old.clone()));

        // Edits made all the time from the settings window: no hook reinstall.
        let mut new = old.clone();
        new.general.log_level = LogLevel::Debug;
        new.widgets.enabled = true;
        new.todo.enabled = true;
        new.updates.check = !old.updates.check;
        new.clipboard.images = !old.clipboard.images;
        new.clipboard.max_entries = Lenient::Valid(500);
        assert!(!keyboard_config_changed(&old, &new));

        let mut new = old.clone();
        new.hyperkey.enabled = true;
        assert!(keyboard_config_changed(&old, &new));

        let mut new = old.clone();
        new.shortcuts.enabled = true;
        assert!(keyboard_config_changed(&old, &new));

        // The clipboard hotkey is part of the shortcut table.
        let mut new = old.clone();
        new.clipboard.enabled = true;
        assert!(keyboard_config_changed(&old, &new));
        let mut new = old.clone();
        new.clipboard.hotkey = Lenient::Valid("win+v".into());
        assert!(keyboard_config_changed(&old, &new));
    }
}
