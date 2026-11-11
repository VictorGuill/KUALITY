//! Kuality settings window: edits `config.json`. The core hot-reloads the file, so every saved
//! change applies live. See `docs/specs/settings.md`.
//!
//! Library part of the single `kuality.exe`: the binary runs [`run`] for
//! `kuality.exe --settings`, in its own process, so the core never loads GPUI
//! (`docs/specs/distribution.md`). [`startup`] holds the start-at-logon entry (Run value or
//! elevated scheduled task), shared by the window and the `--startup` command line.

mod apps;
mod clipboard;
mod model;
mod platform;
mod recorder;
pub mod startup;
mod todo;
mod ui;

use std::process::ExitCode;

use kuality_protocol::cli;
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
    let data_dir = data_dir();
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

/// Where `config.json` and `todos.json` live: next to the exe for a portable copy, else in
/// `%APPDATA%\\Kuality` (the same rule as the core).
fn data_dir() -> DataDir {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    DataDir::resolve(&exe_dir, platform::roaming_app_data_dir())
}

/// Runs the to-do popup until it closes (`kuality.exe --todo [--core-pid <n>] [--at <x>,<y>]`,
/// `docs/specs/todo.md`). Never starts the core: the popup only edits `todos.json`.
pub fn run_todo(args: &[String]) -> ExitCode {
    init_logging();
    let _instance = match platform::Instance::acquire_todo() {
        Ok(Some(i)) => Some(i),
        // One is open (or opening): the core toggles it, nothing to do here.
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            log::warn!("single-instance check failed: {e}");
            None
        }
    };
    let data_dir = data_dir();
    let core = match core_pid_arg(args.iter().cloned()) {
        Some(pid) => match platform::CoreProcess::open(pid) {
            Some(core) => Some(core),
            None => {
                log::info!("Kuality (pid {pid}) is no longer running; not opening");
                return ExitCode::SUCCESS;
            }
        },
        None => platform::find_core(),
    };
    // Shown on hover: it closes when the mouse leaves, until it is clicked.
    let peek = todo::hover_arg(args).map(|module| todo::Peek {
        module,
        guard: match platform::Instance::acquire_todo_peek() {
            Ok(g) => g,
            Err(e) => {
                log::warn!("cannot mark the popup as shown on hover: {e}");
                None
            }
        },
    });
    todo::run(
        data_dir.todo_file(),
        data_dir.config_file(),
        todo::anchor_arg(args),
        peek,
        core,
    );
    ExitCode::SUCCESS
}

/// Runs the clipboard history popup until it closes
/// (`kuality.exe --clipboard [--core-pid <n>] [--paste]`, `docs/specs/clipboard.md`). `--paste`:
/// opened by the hotkey, so Enter pastes into the window that was in front. Never starts the core.
pub fn run_clipboard(args: &[String]) -> ExitCode {
    init_logging();
    let _instance = match platform::Instance::acquire_clipboard() {
        Ok(Some(i)) => Some(i),
        // One is open (or opening): the core toggles it, nothing to do here.
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            log::warn!("single-instance check failed: {e}");
            None
        }
    };
    let core = match core_pid_arg(args.iter().cloned()) {
        Some(pid) => match platform::CoreProcess::open(pid) {
            Some(core) => Some(core),
            None => {
                log::info!("Kuality (pid {pid}) is no longer running; not opening");
                return ExitCode::SUCCESS;
            }
        },
        None => platform::find_core(),
    };
    clipboard::run(core, args.iter().any(|a| a == cli::PASTE));
    ExitCode::SUCCESS
}

/// Starts the clipboard history popup from the settings window (Enter copies).
pub(crate) fn open_clipboard_popup(core_pid: Option<u32>) {
    let started = std::env::current_exe().and_then(|exe| {
        let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg(cli::CLIPBOARD).current_dir(dir);
        if let Some(pid) = core_pid {
            cmd.arg(cli::CORE_PID).arg(pid.to_string());
        }
        cmd.spawn()
    });
    if let Err(e) = started {
        log::error!("cannot start the clipboard popup: {e}");
    }
}

/// Closes the clipboard history popup if it runs (the core toggles it with the hotkey).
/// Returns `false` when none runs.
pub fn close_clipboard_popup() -> bool {
    platform::close_clipboard_popup()
}

/// Starts the to-do popup without an anchor (Settings → To do → "Open to-do list"): bottom-left
/// of the primary screen.
pub(crate) fn open_todo_popup() {
    let started = std::env::current_exe().and_then(|exe| {
        let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        std::process::Command::new(&exe)
            .arg(cli::TODO)
            .current_dir(dir)
            .spawn()
    });
    if let Err(e) = started {
        log::error!("cannot start the to-do popup: {e}");
    }
}

/// Whether the to-do popup runs (a mutex lookup: never blocks).
pub fn todo_popup_running() -> bool {
    platform::todo_popup_running()
}

/// Whether the running to-do popup is only shown on hover (a mutex lookup: never blocks).
pub fn todo_popup_peeking() -> bool {
    platform::todo_popup_peeking()
}

/// Brings the to-do popup to the front (a click on the module while it is shown on hover keeps
/// it open). Returns whether it was found.
pub fn focus_todo_popup() -> bool {
    platform::focus_todo_popup()
}

/// Closes the to-do popup if it runs (called by the core when the taskbar module is clicked).
/// Returns `false` when no popup runs, so the core opens one.
pub fn close_todo_popup() -> bool {
    platform::close_todo_popup()
}

/// `--core-pid <n>` from the command line, if present and valid.
fn core_pid_arg(args: impl IntoIterator<Item = String>) -> Option<u32> {
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        if a == cli::CORE_PID {
            return args.next()?.parse().ok();
        }
        if let Some(v) = a
            .strip_prefix(cli::CORE_PID)
            .and_then(|v| v.strip_prefix('='))
        {
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
