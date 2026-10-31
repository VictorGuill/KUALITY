//! Loading, creating, saving and reloading `config.json`. OS-independent.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use kuality_protocol::Config;
use kuality_protocol::store;

/// Result of the startup load.
#[derive(Debug)]
pub enum LoadStatus {
    /// File existed and parsed.
    Loaded,
    /// File was missing and has been created with defaults.
    Created,
    /// File was missing and could not be created; running with in-memory defaults.
    CreateFailed(io::Error),
    /// File exists but could not be read; running with in-memory defaults.
    ReadFailed(io::Error),
    /// File exists but is not a valid config; running with in-memory defaults, file untouched.
    Invalid(String),
}

/// What [`load_or_create`] returns.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    /// Bytes of the file as last read or written (for change detection), if any.
    pub bytes: Option<Vec<u8>>,
    pub status: LoadStatus,
}

/// Loads `path`, creating it with defaults if it does not exist.
/// Never overwrites an existing file.
pub fn load_or_create(path: &Path) -> Loaded {
    match fs::read(path) {
        Ok(bytes) => match Config::from_json(&bytes) {
            Ok(config) => Loaded {
                config,
                bytes: Some(bytes),
                status: LoadStatus::Loaded,
            },
            Err(e) => Loaded {
                config: Config::default(),
                bytes: Some(bytes),
                status: LoadStatus::Invalid(e.to_string()),
            },
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let config = Config::default();
            match save(path, &config) {
                Ok(bytes) => Loaded {
                    config,
                    bytes: Some(bytes),
                    status: LoadStatus::Created,
                },
                Err(e) => Loaded {
                    config,
                    bytes: None,
                    status: LoadStatus::CreateFailed(e),
                },
            }
        }
        Err(e) => Loaded {
            config: Config::default(),
            bytes: None,
            status: LoadStatus::ReadFailed(e),
        },
    }
}

/// Writes `config` to `path` atomically (temp file + rename). Returns the bytes written.
pub fn save(path: &Path, config: &Config) -> io::Result<Vec<u8>> {
    let bytes = config.to_json_pretty().map_err(io::Error::other)?;
    store::write_atomic(path, &bytes)?;
    Ok(bytes)
}

/// Outcome of a reload check.
#[derive(Debug)]
pub enum ReloadOutcome {
    /// Same bytes as last time (or still missing): nothing to do.
    Unchanged,
    /// File disappeared; keep the current config.
    Missing,
    /// File could not be read (e.g. locked by an editor); retried on the next change.
    ReadFailed(io::Error),
    /// File changed but is not a valid config; keep the current config.
    Invalid(String),
    /// File changed and parsed.
    Changed(Box<Config>),
}

/// Re-reads the config file and reports whether it changed since the last read.
#[derive(Debug)]
pub struct Reloader {
    path: PathBuf,
    last: Option<Vec<u8>>,
}

impl Reloader {
    pub fn new(path: PathBuf, initial_bytes: Option<Vec<u8>>) -> Self {
        Self {
            path,
            last: initial_bytes,
        }
    }

    pub fn check(&mut self) -> ReloadOutcome {
        match fs::read(&self.path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if self.last.take().is_some() {
                    ReloadOutcome::Missing
                } else {
                    ReloadOutcome::Unchanged
                }
            }
            Err(e) => ReloadOutcome::ReadFailed(e),
            Ok(bytes) => {
                if self.last.as_deref() == Some(bytes.as_slice()) {
                    return ReloadOutcome::Unchanged;
                }
                let parsed = Config::from_json(&bytes);
                self.last = Some(bytes);
                match parsed {
                    Ok(c) => ReloadOutcome::Changed(Box::new(c)),
                    Err(e) => ReloadOutcome::Invalid(e.to_string()),
                }
            }
        }
    }
}

/// Names of the top-level sections that differ between `old` and `new`.
pub fn changed_sections(old: &Config, new: &Config) -> Vec<&'static str> {
    let checks = [
        ("version", old.version != new.version),
        ("general", old.general != new.general),
        ("hyperkey", old.hyperkey != new.hyperkey),
        ("shortcuts", old.shortcuts != new.shortcuts),
        ("widgets", old.widgets != new.widgets),
        ("other", old.extra != new.extra),
    ];
    checks
        .into_iter()
        .filter_map(|(name, changed)| changed.then_some(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuality_protocol::test_util::TempDir;

    #[test]
    fn creates_file_with_defaults_when_missing() {
        let dir = TempDir::new("create");
        let path = dir.0.join("sub").join("config.json");
        let loaded = load_or_create(&path);
        assert!(matches!(loaded.status, LoadStatus::Created));
        assert_eq!(loaded.config, Config::default());
        let on_disk = fs::read(&path).unwrap();
        assert_eq!(Some(on_disk.clone()), loaded.bytes);
        assert_eq!(Config::from_json(&on_disk).unwrap(), Config::default());
        assert!(!store::tmp_path(&path).exists());
    }

    #[test]
    fn loads_existing_file() {
        let dir = TempDir::new("load");
        let path = dir.0.join("config.json");
        fs::write(&path, br#"{ "hyperkey": { "enabled": true } }"#).unwrap();
        let loaded = load_or_create(&path);
        assert!(matches!(loaded.status, LoadStatus::Loaded));
        assert!(loaded.config.hyperkey.enabled);
    }

    #[test]
    fn invalid_file_is_not_overwritten() {
        let dir = TempDir::new("invalid");
        let path = dir.0.join("config.json");
        let broken = b"{ \"hyperkey\": { \"enabled\": tru }".to_vec();
        fs::write(&path, &broken).unwrap();
        let loaded = load_or_create(&path);
        assert!(matches!(loaded.status, LoadStatus::Invalid(_)));
        assert_eq!(loaded.config, Config::default());
        assert_eq!(fs::read(&path).unwrap(), broken);
    }

    #[test]
    fn save_replaces_existing_file() {
        let dir = TempDir::new("save");
        let path = dir.0.join("config.json");
        fs::write(&path, b"old").unwrap();
        let mut c = Config::default();
        c.widgets.enabled = true;
        save(&path, &c).unwrap();
        assert_eq!(Config::from_json(&fs::read(&path).unwrap()).unwrap(), c);
    }

    #[test]
    fn reloader_detects_changes_only_once() {
        let dir = TempDir::new("reload");
        let path = dir.0.join("config.json");
        let loaded = load_or_create(&path);
        let mut r = Reloader::new(path.clone(), loaded.bytes);
        assert!(matches!(r.check(), ReloadOutcome::Unchanged));

        fs::write(&path, br#"{ "hyperkey": { "enabled": true } }"#).unwrap();
        match r.check() {
            ReloadOutcome::Changed(c) => assert!(c.hyperkey.enabled),
            other => panic!("expected Changed, got {other:?}"),
        }
        assert!(matches!(r.check(), ReloadOutcome::Unchanged));

        fs::write(&path, b"{ broken").unwrap();
        assert!(matches!(r.check(), ReloadOutcome::Invalid(_)));
        assert!(
            matches!(r.check(), ReloadOutcome::Unchanged),
            "same invalid bytes are reported once"
        );

        fs::write(&path, b"{}").unwrap();
        assert!(
            matches!(r.check(), ReloadOutcome::Changed(_)),
            "fixed file is loaded"
        );

        fs::remove_file(&path).unwrap();
        assert!(matches!(r.check(), ReloadOutcome::Missing));
        assert!(matches!(r.check(), ReloadOutcome::Unchanged));
    }

    #[test]
    fn changed_sections_reports_differences() {
        let a = Config::default();
        assert!(changed_sections(&a, &a).is_empty());
        let mut b = a.clone();
        b.hyperkey.enabled = true;
        b.general.log_level = kuality_protocol::LogLevel::Debug;
        b.extra.insert("x".into(), serde_json::json!(1));
        assert_eq!(
            changed_sections(&a, &b),
            vec!["general", "hyperkey", "other"]
        );
    }
}
