//! `kuality.exe`: the always-running core with the tray icon, and with `--settings` the settings
//! window in a separate process of the same exe (`docs/specs/distribution.md`).
//!
//! Phase 0: config load/create/hot-reload, logging, single instance, tray menu
//! (`docs/specs/skeleton.md`). Phase 1: Hyperkey (`docs/specs/hyperkey.md`). Phase 2: Shortcuts
//! (`docs/specs/shortcuts.md`). Phase 3: taskbar overlay and text widget (`docs/specs/overlay.md`).
//! Updates: `docs/specs/updates.md`.

// No console window in release builds (every mode); debug builds keep it for log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod config_store;
mod hyperkey;
mod logging;
mod overlay;
mod platform;
mod shortcuts;
mod updates;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use actions::ActionWorker;
use app::App;
use config_store::{LoadStatus, ReloadOutcome, Reloader};
use hyperkey::KeyboardController;
use kuality_protocol::paths::{DataDir, DataDirSource};
use overlay::OverlayController;
use platform::{DirWatcher, SingleInstance, TrayIcon, Waker};

/// Named mutex for single instance (per user session, shared by installed and portable copies).
const INSTANCE_MUTEX: &str = r"Local\Kuality.Core";
/// Changes to the config dir are grouped until it has been quiet for this long.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(250);

/// First arguments that run something else than the core (`docs/specs/distribution.md`).
const SETTINGS_ARG: &str = "--settings";
const STARTUP_ARG: &str = "--startup";
const STARTUP_TASK_ARG: &str = "--startup-task";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // Own process: the core never loads GPUI, and the window's memory is freed when it closes.
        Some(SETTINGS_ARG) => kuality_settings::run(&args[1..]),
        // Start at logon: `--startup <off|on|normal|elevated>` (installer, scripts) and the
        // elevated helper it starts through UAC.
        Some(STARTUP_ARG) => kuality_settings::startup::cli(&args[1..]),
        Some(STARTUP_TASK_ARG) => kuality_settings::startup::task_cli(&args[1..]),
        _ => run_core(&args),
    }
}

/// The core: tray, config hot-reload, Hyperkey, shortcuts and taskbar widgets.
fn run_core(args: &[String]) -> ExitCode {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let data_dir = DataDir::resolve(&exe_dir, platform::roaming_app_data_dir());

    // Before logging: a second instance must not rotate the running instance's log.
    let instance = match platform::Instance::acquire(INSTANCE_MUTEX) {
        Ok(Some(guard)) => Ok(guard),
        Ok(None) => {
            let _ = logging::init(&data_dir.logs_dir(), false);
            log::info!(
                "another instance is already running; this one (pid {}) exits",
                std::process::id()
            );
            return ExitCode::SUCCESS;
        }
        Err(e) => Err(e),
    };

    let log_result = logging::init(&data_dir.logs_dir(), true);
    logging::install_panic_hook();
    // Runs before the logging hook: releases any modifiers Hyperkey holds down.
    platform::install_crash_guard();
    log::info!(
        "Kuality {} starting (pid {})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );
    if let Err(e) = log_result {
        log::error!(
            "cannot open log file in {}: {e}",
            data_dir.logs_dir().display()
        );
    }
    let _instance = match instance {
        Ok(guard) => Some(guard),
        Err(e) => {
            log::warn!("single-instance check failed ({e}), continuing anyway");
            None
        }
    };
    log::info!(
        "data dir: {} (portable: {})",
        data_dir.path.display(),
        if data_dir.is_portable() { "yes" } else { "no" }
    );
    if data_dir.source == DataDirSource::Fallback {
        log::warn!("could not resolve %APPDATA%, using the exe directory");
    }
    if !args.is_empty() {
        log::warn!("ignoring unknown arguments: {args:?}");
    }
    log::info!("DPI awareness: {:?}", platform::dpi_awareness());

    platform::init_main_thread();

    // Config.
    let config_path = data_dir.config_file();
    let loaded = config_store::load_or_create(&config_path);
    match &loaded.status {
        LoadStatus::Loaded => log::info!("config loaded from {}", config_path.display()),
        LoadStatus::Created => {
            log::info!("config created with defaults at {}", config_path.display())
        }
        LoadStatus::CreateFailed(e) => {
            log::error!(
                "cannot create {}: {e}; using defaults",
                config_path.display()
            )
        }
        LoadStatus::ReadFailed(e) => {
            log::error!("cannot read {}: {e}; using defaults", config_path.display())
        }
        LoadStatus::Invalid(e) => log::error!(
            "invalid config {}: {e}; using defaults, file left untouched",
            config_path.display()
        ),
    }
    app::warn_if_newer(&loaded.config);
    logging::set_level(loaded.config.general.log_level);

    // Tray + main loop.
    let tray = match platform::Tray::create("Kuality") {
        Ok(t) => t,
        Err(e) => {
            log::error!("cannot create tray window: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Hot-reload: the watcher thread parses the file and hands new configs to the main thread.
    let (tx, rx) = mpsc::channel();
    let waker = tray.waker();
    let mut reloader = Reloader::new(config_path.clone(), loaded.bytes);
    // Never log on `Unchanged`: other writes in the data dir also wake the watcher.
    let on_change = Box::new(move || match reloader.check() {
        ReloadOutcome::Unchanged => {}
        ReloadOutcome::Missing => log::warn!("config file was deleted; keeping the current config"),
        ReloadOutcome::ReadFailed(e) => {
            log::warn!("cannot read config: {e}; will retry on next change")
        }
        ReloadOutcome::Invalid(e) => log::warn!("invalid config: {e}; keeping the current config"),
        ReloadOutcome::Changed(config) => {
            if tx.send(config).is_ok() {
                waker.wake();
            }
        }
    });
    let watcher = match platform::Watcher::start(&data_dir.path, RELOAD_DEBOUNCE, on_change) {
        Ok(w) => Some(w),
        Err(e) => {
            log::error!(
                "cannot watch {}: {e}; hot-reload disabled",
                data_dir.path.display()
            );
            None
        }
    };

    // Spawns its thread on the first shortcut action.
    let actions = Arc::new(ActionWorker::new());
    let keyboard = match KeyboardController::start(Arc::clone(&actions)) {
        Ok(k) => Some(k),
        Err(e) => {
            log::error!(
                "cannot start the keyboard thread: {e}; Hyperkey and shortcuts unavailable"
            );
            None
        }
    };

    let overlay = match OverlayController::start() {
        Ok(o) => Some(o),
        Err(e) => {
            log::error!("cannot start the overlay thread: {e}; taskbar widgets unavailable");
            None
        }
    };

    let updates = match updates::Updates::start(tray.waker(), loaded.config.updates.check) {
        Ok(u) => Some(u),
        Err(e) => {
            log::error!("cannot start the update thread: {e}; updates unavailable");
            None
        }
    };

    let mut app = App::new(
        loaded.config,
        config_path,
        rx,
        watcher,
        keyboard,
        actions,
        overlay,
        updates,
    );
    log::info!("ready");
    tray.run(move |event| app.handle(event));

    log::info!("bye");
    log::logger().flush();
    ExitCode::SUCCESS
}
