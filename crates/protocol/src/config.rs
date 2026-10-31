//! `config.json` types.
//!
//! Rules (see `docs/specs/skeleton.md`):
//! - Every struct uses `#[serde(default)]`: missing fields fall back to defaults, `{}` is valid.
//! - Every feature is disabled by default.
//! - Unknown fields are kept in a flattened `extra` map so a load → save round trip preserves them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Highest config format version this build understands.
pub const CONFIG_VERSION: u32 = 1;

/// Unknown fields of a config object, preserved on save.
pub type Extra = Map<String, Value>;

/// The whole `config.json` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub general: General,
    pub hyperkey: Hyperkey,
    pub shortcuts: Shortcuts,
    pub widgets: Widgets,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            general: General::default(),
            hyperkey: Hyperkey::default(),
            shortcuts: Shortcuts::default(),
            widgets: Widgets::default(),
            extra: Extra::new(),
        }
    }
}

impl Config {
    /// Parses a config from JSON bytes.
    pub fn from_json(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    /// Serializes the config as pretty JSON (2-space indent) with a trailing newline.
    pub fn to_json_pretty(&self) -> serde_json::Result<Vec<u8>> {
        let mut out = serde_json::to_vec_pretty(self)?;
        out.push(b'\n');
        Ok(out)
    }
}

/// `general` section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Start with Windows (`HKCU\...\Run`). Phase 6.
    pub autostart: bool,
    /// Start elevated through a Task Scheduler task. Phase 6.
    pub run_elevated: bool,
    /// Minimum level written to the log file.
    pub log_level: LogLevel,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Log verbosity, lowercase in JSON.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

/// `hyperkey` section. Fields beyond `enabled` arrive in Phase 1.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hyperkey {
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `shortcuts` section. `bindings` become typed in Phase 2.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    pub enabled: bool,
    pub bindings: Vec<Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `widgets` section. `items` become typed in Phase 3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Widgets {
    pub enabled: bool,
    pub hide_on_fullscreen: bool,
    pub items: Vec<Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Widgets {
    fn default() -> Self {
        Self {
            enabled: false,
            hide_on_fullscreen: true,
            items: Vec::new(),
            extra: Extra::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_all_disabled() {
        let c = Config::default();
        assert_eq!(c.version, CONFIG_VERSION);
        assert!(!c.general.autostart);
        assert!(!c.general.run_elevated);
        assert!(!c.hyperkey.enabled);
        assert!(!c.shortcuts.enabled);
        assert!(c.shortcuts.bindings.is_empty());
        assert!(!c.widgets.enabled);
        assert!(c.widgets.items.is_empty());
    }

    #[test]
    fn empty_object_parses_to_defaults() {
        assert_eq!(Config::from_json(b"{}").unwrap(), Config::default());
    }

    #[test]
    fn partial_config_keeps_given_values() {
        let c = Config::from_json(
            br#"{ "hyperkey": { "enabled": true }, "general": { "log_level": "debug" } }"#,
        )
        .unwrap();
        assert!(c.hyperkey.enabled);
        assert_eq!(c.general.log_level, LogLevel::Debug);
        assert!(!c.general.autostart);
        assert!(c.widgets.hide_on_fullscreen);
        assert_eq!(c.version, CONFIG_VERSION);
    }

    #[test]
    fn unknown_fields_survive_round_trip() {
        let input = json!({
            "version": 1,
            "future_section": { "a": 1 },
            "general": { "autostart": true, "theme": "dark" },
            "hyperkey": { "enabled": false, "double_tap": true },
            "shortcuts": { "enabled": false, "bindings": [{ "key": "1" }], "x": [1, 2] },
            "widgets": { "enabled": true, "hide_on_fullscreen": false, "items": [], "y": null }
        });
        let c: Config = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(c.extra["future_section"], json!({ "a": 1 }));
        assert_eq!(c.general.extra["theme"], json!("dark"));
        let back = serde_json::to_value(&c).unwrap();
        // Known fields missing from the input are written with their defaults.
        let mut expected = input;
        expected["general"]["log_level"] = json!("info");
        expected["general"]["run_elevated"] = json!(false);
        assert_eq!(back, expected);
    }

    #[test]
    fn serialized_defaults_match_documented_json() {
        let expected = json!({
            "version": 1,
            "general": { "autostart": false, "run_elevated": false, "log_level": "info" },
            "hyperkey": { "enabled": false },
            "shortcuts": { "enabled": false, "bindings": [] },
            "widgets": { "enabled": false, "hide_on_fullscreen": true, "items": [] }
        });
        assert_eq!(serde_json::to_value(Config::default()).unwrap(), expected);
    }

    #[test]
    fn pretty_json_ends_with_newline_and_parses_back() {
        let bytes = Config::default().to_json_pretty().unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(Config::from_json(&bytes).unwrap(), Config::default());
    }

    #[test]
    fn invalid_log_level_is_an_error() {
        assert!(Config::from_json(br#"{ "general": { "log_level": "loud" } }"#).is_err());
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(Config::from_json(b"{ \"version\": 1,").is_err());
    }
}
