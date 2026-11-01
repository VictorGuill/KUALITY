//! Kuality settings window: edits `config.json`. The core hot-reloads the file, so every saved
//! change applies live. See `docs/specs/settings.md`.
//!
//! Library part of the single `kuality.exe`: the binary runs [`run`] for
//! `kuality.exe --settings`, in its own process, so the core never loads GPUI
//! (`docs/specs/distribution.md`).

mod apps;
mod model;
mod platform;
mod recorder;
mod ui;

use std::process::ExitCode;

use kuality_protocol::paths::{DataDir, DataDirSource};

#[cfg(not(windows))]
compile_error!("Kuality only supports Windows for now (see docs/DECISIONS.md).");

/// Runs the settings window until it closes. `args` are the arguments after `--settings`.
pub fn run(args: &[String]) -> ExitCode {
    init_logging();
    let _instance = match platform::Instance::acquire() {
        Ok(Some(i)) => Some(i),
        Ok(None) => {
            if !platform::activate_existing() {
                log::warn!("another settings window is running but was not found");
            }
            return ExitCode::SUCCESS;
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
    let core = match link_core(args) {
        Some(core) => core,
        // The core that opened this window has already exited: nothing to configure live.
        None => return ExitCode::SUCCESS,
    };
    ui::run(data_dir.config_file(), core);
    ExitCode::SUCCESS
}

/// Argument the core passes with its pid when it opens this window (after `--settings`).
const CORE_PID_ARG: &str = "--core-pid";

/// `--core-pid <n>` from the command line, if present and valid.
fn core_pid_arg(args: impl IntoIterator<Item = String>) -> Option<u32> {
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        if a == CORE_PID_ARG {
            return args.next()?.parse().ok();
        }
        if let Some(v) = a.strip_prefix("--core-pid=") {
            return v.parse().ok();
        }
    }
    None
}

/// Finds the core this window belongs to, starting it when none is running. `None` = the core
/// that started us is already gone (quit right away).
fn link_core(args: &[String]) -> Option<ui::CoreLink> {
    if let Some(pid) = core_pid_arg(args.iter().cloned()) {
        return match platform::CoreProcess::open(pid) {
            Some(core) => {
                log::info!("attached to Kuality (pid {pid})");
                Some(ui::CoreLink::Attached(core))
            }
            None => {
                log::info!("Kuality (pid {pid}) is no longer running; not opening");
                None
            }
        };
    }
    if let Some(core) = platform::find_core() {
        log::info!("attached to the running Kuality (pid {})", core.pid);
        return Some(ui::CoreLink::Attached(core));
    }
    if platform::core_running() {
        // Its mutex exists but its window was not found: do not start a second one.
        log::warn!("Kuality seems to be running but its window was not found");
        return Some(ui::CoreLink::Missing(
            "Its process was found but this window could not attach to it.".into(),
        ));
    }
    match platform::start_core() {
        Ok(core) => {
            log::info!("started Kuality (pid {})", core.pid);
            Some(ui::CoreLink::Attached(core))
        }
        Err(e) => {
            log::error!("cannot start Kuality: {e}");
            Some(ui::CoreLink::Missing(format!("Could not start it: {e}.")))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::core_pid_arg;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn core_pid_argument() {
        assert_eq!(core_pid_arg(args(&[])), None);
        assert_eq!(core_pid_arg(args(&["--core-pid", "1234"])), Some(1234));
        assert_eq!(core_pid_arg(args(&["x", "--core-pid=42"])), Some(42));
        assert_eq!(core_pid_arg(args(&["--core-pid"])), None);
        assert_eq!(core_pid_arg(args(&["--core-pid", "abc"])), None);
    }
}
