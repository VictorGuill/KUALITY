//! Kuality settings app: a window that edits `config.json`. The core hot-reloads the file, so
//! every saved change applies live. See `docs/specs/settings.md`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod model;
mod platform;
mod recorder;
mod ui;

use kuality_protocol::paths::{DataDir, DataDirSource};

#[cfg(not(windows))]
compile_error!("Kuality only supports Windows for now (see docs/DECISIONS.md).");

fn main() {
    init_logging();
    let _instance = match platform::Instance::acquire() {
        Ok(Some(i)) => Some(i),
        Ok(None) => {
            if !platform::activate_existing() {
                log::warn!("another settings window is running but was not found");
            }
            return;
        }
        Err(e) => {
            // Better two windows than none.
            log::warn!("single-instance check failed: {e}");
            None
        }
    };
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let data_dir = DataDir::resolve(&exe_dir, platform::roaming_app_data_dir());
    if data_dir.source == DataDirSource::Fallback {
        log::warn!("could not resolve %APPDATA%, using the exe directory");
    }
    ui::run(data_dir.config_file());
}

/// stderr logger in debug builds (shares the core's console when started by a debug core);
/// release builds log nothing.
fn init_logging() {
    struct Stderr;
    impl log::Log for Stderr {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.target().starts_with("kuality_settings")
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                eprintln!("[settings {}] {}", r.level(), r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: Stderr = Stderr;
    if cfg!(debug_assertions) && log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}
