//! Main-thread application state: reacts to tray commands and config reloads.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use kuality_protocol::app_lock::ConfirmAction;
use kuality_protocol::cli::ArmedSignal;
use kuality_protocol::{CONFIG_VERSION, Config, cli};

use crate::actions::ActionWorker;
use crate::app_lock::{self, AppLockEvent, AppLockService, Wake};
use crate::clipboard::ClipboardService;
use crate::config_store::changed_sections;
use crate::hyperkey::KeyboardController;
use crate::ipc::IpcHub;
use crate::overlay::OverlayController;
use crate::overlay::layout::RectPx;
use crate::overlay::widgets::LiveData;
use crate::platform::{self, ArmedPopup, DirWatcher, Event, TodoRequest, TodoTrigger};
use crate::updates::Updates;
use crate::{logging, shortcuts};

/// How long quit waits for a running `launch`/`open_url` action.
const ACTIONS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// A hover does not start a second popup this soon after starting one.
const TODO_START_GUARD: Duration = Duration::from_millis(500);
/// The hotkey does not start a second clipboard popup this soon after starting one.
const CLIPBOARD_START_GUARD: Duration = Duration::from_millis(500);

/// Files next to `config.json` the core writes.
#[derive(Debug, Clone)]
pub struct DataFiles {
    pub config: PathBuf,
    /// `clipboard-pins.bin`.
    pub pins: PathBuf,
    /// `app-lock.bin`.
    pub app_lock: PathBuf,
    /// `app-lock-hidden.json`.
    pub app_lock_hidden: PathBuf,
}

/// State owned by the main thread.
pub struct App<W: DirWatcher> {
    config: Config,
    files: DataFiles,
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
    /// The to-do popup started hidden when the mouse entered this module (`TodoTrigger::Arm`).
    todo_armed: Option<(ArmedPopup, RectPx)>,
    /// The last armed popup told to show: kept until the next one, so its events live until
    /// the process has opened them.
    todo_signaled: Option<ArmedPopup>,
    /// Clipboard history, while `clipboard.enabled`.
    clipboard: Option<ClipboardService>,
    /// When the clipboard popup was last started.
    clipboard_started: Option<Instant>,
    /// App lock, while `app_lock.enabled`.
    app_lock: Option<AppLockService>,
    app_lock_events: (Sender<AppLockEvent>, Receiver<AppLockEvent>),
    /// Wakes the main loop from another thread.
    wake: Wake,
    /// The pipe shared by the clipboard history and App lock.
    ipc: IpcHub,
    /// Tray "Pause" (runtime only: keyboard features and App lock).
    paused: bool,
}

impl<W: DirWatcher> std::fmt::Debug for App<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("paused", &self.paused)
            .field("clipboard", &self.clipboard.is_some())
            .field("app_lock", &self.app_lock.is_some())
            .finish_non_exhaustive()
    }
}

impl<W: DirWatcher> App<W> {
    /// Applies the initial config to the subsystems.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        files: DataFiles,
        wake: Wake,
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
            files,
            reloads,
            todo_counts,
            watcher,
            keyboard,
            actions,
            overlay,
            updates,
            live: LiveData::default(),
            todo_started: None,
            todo_armed: None,
            todo_signaled: None,
            clipboard: None,
            clipboard_started: None,
            app_lock: None,
            app_lock_events: mpsc::channel(),
            wake,
            ipc: IpcHub::new(),
            paused: false,
        };
        app.apply_clipboard(None);
        app.apply_app_lock(None);
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
                    cli::SETTINGS.into(),
                    cli::CORE_PID.into(),
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
        log::info!("opening {}", self.files.config.display());
        if let Err(e) = platform::open_with_default_app(&self.files.config) {
            log::error!("could not open {}: {e}", self.files.config.display());
        }
    }

    /// A click on, or the mouse resting on, a bar's to-do module (`kuality.exe --todo`, this
    /// same exe, is the popup):
    /// - click while the list is only shown on hover: keep it open (bring it to the front);
    /// - click while it is (or was, at the mouse press) open: close it;
    /// - click otherwise: open it above the module;
    /// - hover: show it until the mouse leaves the module and the list, unless one is open;
    /// - arm / disarm: the mouse entered / left a module that opens on hover: start the popup
    ///   hidden, so the hover or click that follows only has to show it, or drop it.
    fn on_todo(&mut self, r: TodoRequest) {
        match r.trigger {
            TodoTrigger::Arm => return self.arm_todo(r),
            TodoTrigger::Disarm => {
                if self.todo_armed.take().is_some() {
                    log::debug!("hidden to-do popup dropped");
                }
                return;
            }
            TodoTrigger::Hover | TodoTrigger::Click { .. } => {}
        }
        // The popup started hidden when the mouse arrived: show it (hover) or open it with the
        // focus (click). Without one (still starting, or gone), the usual paths below.
        if let Some((mut armed, _)) = self.todo_armed.take()
            && armed.alive()
        {
            let signal = match r.trigger {
                TodoTrigger::Hover => ArmedSignal::Show,
                _ => ArmedSignal::Activate,
            };
            armed.signal(signal);
            self.todo_signaled = Some(armed);
            self.todo_started = Some(Instant::now());
            log::debug!("hidden to-do popup told to {signal:?}");
            return;
        }
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
            TodoTrigger::Hover if self.todo_popup_busy() => return,
            _ => {}
        }
        let args = Self::todo_args(&r);
        let started = std::env::current_exe().and_then(|exe| platform::spawn_detached(&exe, &args));
        match started {
            Ok(()) => {
                self.todo_started = Some(Instant::now());
                log::debug!("started the to-do popup");
            }
            Err(e) => log::error!("could not start the to-do popup: {e}"),
        }
    }

    /// Whether a to-do popup runs, or was started too recently to have claimed its mutex.
    fn todo_popup_busy(&self) -> bool {
        kuality_settings::todo_popup_running()
            || self
                .todo_started
                .is_some_and(|t| t.elapsed() < TODO_START_GUARD)
    }

    /// The mouse entered a to-do module that opens on hover: starts its popup hidden (unless one
    /// runs, or a hidden one for this module is still alive).
    fn arm_todo(&mut self, r: TodoRequest) {
        if let Some((armed, module)) = &mut self.todo_armed
            && *module == r.module
            && armed.alive()
        {
            return;
        }
        self.todo_armed = None;
        if self.todo_popup_busy() {
            return;
        }
        // Its events can go now: that popup has long opened them.
        self.todo_signaled = None;
        let args = Self::todo_args(&TodoRequest {
            trigger: TodoTrigger::Hover,
            ..r
        });
        match std::env::current_exe().and_then(|exe| ArmedPopup::spawn(&exe, &args, "Todo")) {
            Ok(armed) => {
                self.todo_armed = Some((armed, r.module));
                log::debug!("started the to-do popup hidden");
            }
            Err(e) => log::error!("could not start the hidden to-do popup: {e}"),
        }
    }

    /// The to-do popup's command line for request `r`.
    fn todo_args(r: &TodoRequest) -> Vec<String> {
        let m = r.module;
        let mut args = vec![
            cli::TODO.into(),
            cli::CORE_PID.into(),
            std::process::id().to_string(),
            cli::AT.into(),
            format!("{},{}", m.x, r.taskbar_top),
        ];
        if r.trigger == TodoTrigger::Hover {
            args.push(cli::HOVER.into());
            args.push(format!("{},{},{},{}", m.x, m.y, m.x + m.w, m.y + m.h));
        }
        args
    }

    /// Starts, updates or stops the clipboard history to match the config (`old`: the section
    /// before a reload, `None` at start).
    fn apply_clipboard(&mut self, old: Option<&kuality_protocol::Clipboard>) {
        let new = &self.config.clipboard;
        match (&mut self.clipboard, new.enabled) {
            (None, true) => match ClipboardService::start(new, self.files.pins.clone()) {
                Ok(s) => {
                    self.ipc.set_clipboard(s.ipc_handler());
                    self.clipboard = Some(s);
                }
                Err(e) => log::error!("cannot start the clipboard history: {e}"),
            },
            (Some(_), false) => {
                self.ipc.set_clipboard(None);
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

    /// Starts, updates or stops App lock to match the config (`old`: the section before a
    /// reload, `None` at start: running locked programs are then locked at once).
    fn apply_app_lock(&mut self, old: Option<&kuality_protocol::AppLock>) {
        let new = &self.config.app_lock;
        match (&self.app_lock, new.enabled) {
            (None, true) => match AppLockService::start(
                new,
                self.files.app_lock.clone(),
                self.files.app_lock_hidden.clone(),
                old.is_none(),
                self.app_lock_events.0.clone(),
                Arc::clone(&self.wake),
            ) {
                Ok(s) => {
                    if self.paused {
                        s.set_paused(true);
                    }
                    self.ipc.set_app_lock(Some(s.ipc_handler()));
                    self.app_lock = Some(s);
                }
                Err(e) => log::error!("cannot start App lock: {e}"),
            },
            (Some(_), false) => {
                self.ipc.set_app_lock(None);
                if let Some(mut s) = self.app_lock.take() {
                    s.shutdown();
                }
                kuality_settings::close_unlock_popup();
            }
            (Some(s), true) if old != Some(new) => s.apply(new),
            _ => {}
        }
    }

    /// Tray Quit / Pause need the App lock password while one is set.
    fn guarded(&self) -> bool {
        self.app_lock
            .as_ref()
            .is_some_and(AppLockService::has_password)
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        log::info!(
            "keyboard features and App lock {}",
            if self.paused { "paused" } else { "resumed" }
        );
        if let Some(k) = &self.keyboard {
            k.set_paused(self.paused);
        }
        if let Some(a) = &self.app_lock {
            a.set_paused(self.paused);
        }
        platform::set_tray_paused(self.paused);
    }

    /// Stops everything; the main loop ends after this.
    fn quit(&mut self) -> ControlFlow<()> {
        log::info!("quitting");
        if let Some(u) = &self.updates {
            u.shutdown();
        }
        // First: shows the windows it hid.
        if let Some(mut a) = self.app_lock.take() {
            a.shutdown();
        }
        kuality_settings::close_unlock_popup();
        self.ipc.shutdown();
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
        self.clipboard_started = Some(Instant::now());
        // The Start menu and other shell flyouts sit above every topmost window: close them
        // first, so the popup is on top and Enter pastes into the app behind them. Closing
        // waits for the shell (up to 400 ms), so it does not run on the main thread.
        if platform::shell_flyout_in_front() {
            let spawned = std::thread::Builder::new()
                .name("kuality-clipboard-open".into())
                .spawn(|| {
                    platform::close_shell_flyout();
                    start_clipboard_popup();
                });
            match spawned {
                Ok(_) => return,
                Err(e) => log::warn!("cannot close the shell flyout first: {e}"),
            }
        }
        start_clipboard_popup();
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
                while let Ok(event) = self.app_lock_events.1.try_recv() {
                    match event {
                        AppLockEvent::Confirmed(ConfirmAction::Quit) => return self.quit(),
                        AppLockEvent::Confirmed(ConfirmAction::Pause) => {
                            if !self.paused {
                                self.toggle_pause();
                            }
                        }
                    }
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
                if !self.paused && self.guarded() {
                    app_lock::start_confirm_prompt(ConfirmAction::Pause);
                } else {
                    self.toggle_pause();
                }
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
            Event::QuitFromMenu if self.guarded() => {
                app_lock::start_confirm_prompt(ConfirmAction::Quit);
                ControlFlow::Continue(())
            }
            Event::QuitFromMenu | Event::Quit => self.quit(),
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
        if let Some(u) = &self.updates {
            // Enabled first: a channel change then schedules its check on the new state.
            if new.updates.check != self.config.updates.check {
                u.set_enabled(new.updates.check);
            }
            if new.updates.channel != self.config.updates.channel {
                u.set_channel(new.updates.channel.includes_prereleases());
            }
        }
        let old = std::mem::replace(&mut self.config, new);
        self.apply_clipboard(Some(&old.clipboard));
        self.apply_app_lock(Some(&old.app_lock));
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

/// Remembers the window in front as the paste target and starts the clipboard popup
/// (`kuality.exe --clipboard --paste`, this same exe). Any thread.
fn start_clipboard_popup() {
    platform::set_paste_target(platform::foreground_window());
    let args = [
        cli::CLIPBOARD.into(),
        cli::CORE_PID.into(),
        std::process::id().to_string(),
        cli::PASTE.into(),
    ];
    match std::env::current_exe().and_then(|exe| platform::spawn_detached(&exe, &args)) {
        Ok(()) => log::debug!("started the clipboard popup"),
        Err(e) => log::error!("could not start the clipboard popup: {e}"),
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
