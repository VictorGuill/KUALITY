//! Kuality core (`kuality.exe`): always-running process with the tray icon.
//!
//! Phase 0: config load/create/hot-reload, logging, single instance, tray menu.
//! See `docs/specs/skeleton.md`.

// No console window in release builds; debug builds keep it for log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config_store;
mod logging;
mod paths;
mod platform;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use app::App;
use config_store::{LoadStatus, ReloadOutcome, Reloader};
use paths::{DataDir, DataDirSource};
use platform::{DirWatcher, SingleInstance, TrayIcon, Waker};

/// Named mutex for single instance (per user session, shared by installed and portable copies).
const INSTANCE_MUTEX: &str = r"Local\Kuality.Core";
/// Changes to the config dir are grouped until it has been quiet for this long.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(250);

fn main() -> ExitCode {
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

    let mut app = App::new(loaded.config, config_path, rx, watcher);
    log::info!("ready");
    tray.run(move |event| app.handle(event));

    log::info!("bye");
    log::logger().flush();
    ExitCode::SUCCESS
}
