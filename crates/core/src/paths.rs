//! Data directory resolution (portable mode vs `%APPDATA%\Kuality`). OS-independent.

use std::path::{Path, PathBuf};

/// Marker next to the exe that turns on portable mode (file or directory, any content).
pub const PORTABLE_MARKER: &str = "portable";
/// Folder created under the roaming app data dir.
pub const APP_DIR_NAME: &str = "Kuality";
/// Config file name inside the data dir.
pub const CONFIG_FILE: &str = "config.json";
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

    pub fn logs_dir(&self) -> PathBuf {
        self.path.join(LOGS_DIR)
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh, empty temp directory removed on drop.
    #[derive(Debug)]
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("kuality-test-{}-{tag}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::TempDir;
    use super::*;

    #[test]
    fn uses_app_data_without_marker() {
        let exe = TempDir::new("exe");
        let roaming = PathBuf::from(r"C:\Users\someone\AppData\Roaming");
        let d = DataDir::resolve(&exe.0, Some(roaming.clone()));
        assert_eq!(d.source, DataDirSource::AppData);
        assert_eq!(d.path, roaming.join("Kuality"));
        assert_eq!(d.config_file(), roaming.join("Kuality").join("config.json"));
        assert_eq!(d.logs_dir(), roaming.join("Kuality").join("logs"));
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
