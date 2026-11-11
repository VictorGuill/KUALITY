//! Data directory resolution (portable mode vs `%APPDATA%\Kuality`). OS-independent.
//!
//! Shared by the core and the settings app so both find the same `config.json`.

use std::path::{Path, PathBuf};

/// Marker next to the exe that turns on portable mode (file or directory, any content).
pub const PORTABLE_MARKER: &str = "portable";
/// Folder created under the roaming app data dir.
pub const APP_DIR_NAME: &str = "Kuality";
/// Config file name inside the data dir.
pub const CONFIG_FILE: &str = "config.json";
/// To-do items file name inside the data dir.
pub const TODO_FILE: &str = "todos.json";
/// Pinned clipboard entries (DPAPI-encrypted) inside the data dir.
pub const CLIPBOARD_PINS_FILE: &str = "clipboard-pins.bin";
/// Logs folder name inside the data dir.
pub const LOGS_DIR: &str = "logs";

/// Why the data dir is where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirSource {
    /// `portable` marker found next to the exe.
    Portable,
    /// `%APPDATA%\Kuality`.
    AppData,
    /// App data folder could not be resolved; using the exe dir.
    Fallback,
}

/// Where config and logs live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    pub path: PathBuf,
    pub source: DataDirSource,
}

impl DataDir {
    /// Resolves the data dir. `roaming_app_data` is the user's roaming app data folder, if known.
    pub fn resolve(exe_dir: &Path, roaming_app_data: Option<PathBuf>) -> Self {
        if exe_dir.join(PORTABLE_MARKER).exists() {
            return Self {
                path: exe_dir.to_path_buf(),
                source: DataDirSource::Portable,
            };
        }
        match roaming_app_data {
            Some(dir) => Self {
                path: dir.join(APP_DIR_NAME),
                source: DataDirSource::AppData,
            },
            None => Self {
                path: exe_dir.to_path_buf(),
                source: DataDirSource::Fallback,
            },
        }
    }

    pub fn is_portable(&self) -> bool {
        self.source == DataDirSource::Portable
    }

    pub fn config_file(&self) -> PathBuf {
        self.path.join(CONFIG_FILE)
    }

    pub fn todo_file(&self) -> PathBuf {
        self.path.join(TODO_FILE)
    }

    pub fn clipboard_pins_file(&self) -> PathBuf {
        self.path.join(CLIPBOARD_PINS_FILE)
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.path.join(LOGS_DIR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn uses_app_data_without_marker() {
        let exe = TempDir::new("exe");
        let roaming = PathBuf::from(r"C:\Users\someone\AppData\Roaming");
        let d = DataDir::resolve(&exe.0, Some(roaming.clone()));
        assert_eq!(d.source, DataDirSource::AppData);
        assert_eq!(d.path, roaming.join("Kuality"));
        assert_eq!(d.config_file(), roaming.join("Kuality").join("config.json"));
        assert_eq!(d.logs_dir(), roaming.join("Kuality").join("logs"));
        assert_eq!(d.todo_file(), roaming.join("Kuality").join("todos.json"));
        assert_eq!(
            d.clipboard_pins_file(),
            roaming.join("Kuality").join("clipboard-pins.bin")
        );
    }

    #[test]
    fn portable_marker_file_wins() {
        let exe = TempDir::new("portable-file");
        std::fs::write(exe.0.join(PORTABLE_MARKER), b"").unwrap();
        let d = DataDir::resolve(&exe.0, Some(PathBuf::from(r"C:\x")));
        assert!(d.is_portable());
        assert_eq!(d.path, exe.0);
    }

    #[test]
    fn portable_marker_directory_also_counts() {
        let exe = TempDir::new("portable-dir");
        std::fs::create_dir(exe.0.join(PORTABLE_MARKER)).unwrap();
        assert!(DataDir::resolve(&exe.0, None).is_portable());
    }

    #[test]
    fn falls_back_to_exe_dir() {
        let exe = TempDir::new("fallback");
        let d = DataDir::resolve(&exe.0, None);
        assert_eq!(d.source, DataDirSource::Fallback);
        assert_eq!(d.path, exe.0);
    }
}
