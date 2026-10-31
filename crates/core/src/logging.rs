//! Small `log` backend: one file in the logs dir (+ stderr in debug builds), with rotation.
//!
//! Lines are written with a single `write` call each and no buffering, so nothing is lost on
//! a crash. The core logs very little when idle, so this costs nothing measurable.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kuality_protocol::LogLevel;
use log::{LevelFilter, Log, Metadata, Record};

use crate::platform;

/// Current log file name.
pub const LOG_FILE: &str = "kuality.log";
/// Previous log file name (one generation kept).
pub const OLD_LOG_FILE: &str = "kuality.old.log";
/// The file is rotated while running once it grows past this size.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Local wall-clock time, as provided by the platform layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
}

impl fmt::Display for LocalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.millis
        )
    }
}

/// Maps the config level to a `log` filter.
pub fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::Error,
        LogLevel::Warn => LevelFilter::Warn,
        LogLevel::Info => LevelFilter::Info,
        LogLevel::Debug => LevelFilter::Debug,
        LogLevel::Trace => LevelFilter::Trace,
    }
}

/// Filter to use while reporting a config reload: the more verbose of the current filter and
/// the new level, so the report survives a level change in either direction.
pub fn reload_report_filter(current: LevelFilter, new: LogLevel) -> LevelFilter {
    current.max(level_filter(new))
}

/// Applies the config level (also used on hot-reload).
pub fn set_level(level: LogLevel) {
    log::set_max_level(level_filter(level));
}

/// One formatted log line, including the trailing newline.
pub fn format_line(
    time: LocalTime,
    level: log::Level,
    target: &str,
    args: &fmt::Arguments<'_>,
) -> String {
    let target = target
        .strip_prefix("kuality::")
        .unwrap_or(if target == "kuality" { "main" } else { target });
    format!("{time} {level:<5} [{target}] {args}\n")
}

/// Renames `dir/kuality.log` to `dir/kuality.old.log` (replacing it). Missing file is fine.
pub fn rotate(dir: &Path) -> io::Result<()> {
    match fs::rename(dir.join(LOG_FILE), dir.join(OLD_LOG_FILE)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// The open log file and its size.
#[derive(Debug)]
struct LogFile {
    dir: PathBuf,
    file: File,
    size: u64,
}

impl LogFile {
    fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(LOG_FILE))?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            size,
        })
    }

    /// Writes one line, rotating first if the file would pass `max_bytes`.
    fn write_line(&mut self, line: &str, max_bytes: u64) -> io::Result<()> {
        if self.size > 0 && self.size + line.len() as u64 > max_bytes {
            // Reopen through a fresh handle: the old one is closed when replaced.
            let dir = self.dir.clone();
            rotate(&dir)?;
            *self = Self::open(&dir)?;
        }
        self.file.write_all(line.as_bytes())?;
        self.size += line.len() as u64;
        Ok(())
    }
}

#[derive(Debug)]
struct FileLogger {
    file: Mutex<Option<LogFile>>,
}

static LOGGER: FileLogger = FileLogger {
    file: Mutex::new(None),
};

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format_line(
            platform::local_time_now(),
            record.level(),
            record.target(),
            record.args(),
        );
        if cfg!(debug_assertions) {
            let _ = io::stderr().write_all(line.as_bytes());
        }
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = guard.as_mut()
            && f.write_line(&line, MAX_LOG_BYTES).is_err()
        {
            // Stop writing to a broken file rather than failing on every line.
            *guard = None;
        }
    }

    fn flush(&self) {
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = guard.as_mut() {
            let _ = f.file.flush();
        }
    }
}

/// Installs the logger. With `rotate_on_start`, the previous log becomes `kuality.old.log`.
///
/// The logger is always installed (stderr still works in debug builds); the returned error
/// only means the log file could not be opened.
pub fn init(logs_dir: &Path, rotate_on_start: bool) -> io::Result<()> {
    // Ignore the error: it only happens if a logger was already set.
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(LevelFilter::Info);

    let opened = (|| {
        fs::create_dir_all(logs_dir)?;
        if rotate_on_start {
            rotate(logs_dir)?;
        }
        LogFile::open(logs_dir)
    })();
    let mut guard = LOGGER.file.lock().unwrap_or_else(|e| e.into_inner());
    match opened {
        Ok(f) => {
            *guard = Some(f);
            Ok(())
        }
        Err(e) => {
            *guard = None;
            Err(e)
        }
    }
}

/// Logs panics (message + location) before running the default hook.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        log::logger().flush();
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;

    const T: LocalTime = LocalTime {
        year: 2026,
        month: 1,
        day: 2,
        hour: 3,
        minute: 4,
        second: 5,
        millis: 6,
    };

    #[test]
    fn formats_lines() {
        let line = format_line(
            T,
            log::Level::Info,
            "kuality::config_store",
            &format_args!("hi {}", 1),
        );
        assert_eq!(line, "2026-01-02 03:04:05.006 INFO  [config_store] hi 1\n");
        let line = format_line(T, log::Level::Error, "kuality", &format_args!("x"));
        assert_eq!(line, "2026-01-02 03:04:05.006 ERROR [main] x\n");
    }

    #[test]
    fn rotate_renames_and_replaces_old_file() {
        let dir = TempDir::new("rotate");
        assert!(rotate(&dir.0).is_ok(), "missing file is fine");
        fs::write(dir.0.join(LOG_FILE), b"new").unwrap();
        fs::write(dir.0.join(OLD_LOG_FILE), b"older").unwrap();
        rotate(&dir.0).unwrap();
        assert!(!dir.0.join(LOG_FILE).exists());
        assert_eq!(fs::read(dir.0.join(OLD_LOG_FILE)).unwrap(), b"new");
    }

    #[test]
    fn rotates_when_size_limit_is_reached() {
        let dir = TempDir::new("size");
        let mut f = LogFile::open(&dir.0).unwrap();
        f.write_line("0123456789\n", 20).unwrap();
        f.write_line("abcdefghij\n", 20).unwrap();
        assert_eq!(
            fs::read_to_string(dir.0.join(OLD_LOG_FILE)).unwrap(),
            "0123456789\n"
        );
        assert_eq!(
            fs::read_to_string(dir.0.join(LOG_FILE)).unwrap(),
            "abcdefghij\n"
        );
    }

    #[test]
    fn reload_report_survives_level_changes() {
        // warn -> info: the reload line (info) must be written.
        assert_eq!(
            reload_report_filter(LevelFilter::Warn, LogLevel::Info),
            LevelFilter::Info
        );
        // info -> warn: still reported once, before switching to warn.
        assert_eq!(
            reload_report_filter(LevelFilter::Info, LogLevel::Warn),
            LevelFilter::Info
        );
        assert_eq!(
            reload_report_filter(LevelFilter::Error, LogLevel::Error),
            LevelFilter::Error
        );
    }

    #[test]
    fn maps_levels() {
        assert_eq!(level_filter(LogLevel::Warn), LevelFilter::Warn);
        assert_eq!(level_filter(LogLevel::Trace), LevelFilter::Trace);
    }
}
