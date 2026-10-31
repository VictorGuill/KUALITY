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

/// `shortcuts` section (see `docs/specs/shortcuts.md`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    pub enabled: bool,
    pub bindings: Vec<BindingEntry>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// One entry of `shortcuts.bindings`. Entries that do not parse as a [`Binding`] (unknown action
/// type, missing field…) are kept as raw JSON so they survive a save, and skipped at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BindingEntry {
    Valid(Binding),
    Invalid(Value),
}

/// Hyper+`key` runs `action`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    /// Trigger key name (see `keys::parse_key`). Validated when bindings are compiled.
    pub key: String,
    /// Label for the settings UI; not used by the core.
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub action: Action,
    #[serde(flatten)]
    pub extra: Extra,
}

fn default_true() -> bool {
    true
}

/// What a binding does, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Open a program, document, folder or `shell:` path.
    Launch {
        path: String,
        #[serde(default)]
        args: LaunchArgs,
        #[serde(default)]
        cwd: Option<String>,
    },
    /// Send one key chord, e.g. `"ctrl+shift+esc"`.
    SendHotkey { keys: String },
    /// Open a URI with its default handler (`https:`, `mailto:`, `ms-settings:`…).
    OpenUrl { url: String },
}

/// `launch.args`: a list (quoted and joined with Windows rules) or one raw string (verbatim).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LaunchArgs {
    List(Vec<String>),
    Raw(String),
}

impl Default for LaunchArgs {
    fn default() -> Self {
        Self::List(Vec::new())
    }
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
    fn shortcut_example_parses() {
        let c = Config::from_json(
            br#"{ "shortcuts": { "enabled": true, "bindings": [
                { "key": "1", "name": "Notepad", "enabled": true,
                  "action": { "type": "launch", "path": "C:\\Windows\\notepad.exe", "args": [], "cwd": null } },
                { "key": "2", "action": { "type": "send_hotkey", "keys": "ctrl+shift+esc" } },
                { "key": "G", "action": { "type": "open_url", "url": "https://github.com" } }
            ] } }"#,
        )
        .unwrap();
        assert!(c.shortcuts.enabled);
        let b: Vec<&Binding> = c
            .shortcuts
            .bindings
            .iter()
            .map(|e| match e {
                BindingEntry::Valid(b) => b,
                BindingEntry::Invalid(v) => panic!("invalid entry {v}"),
            })
            .collect();
        assert_eq!(b[0].name, "Notepad");
        assert_eq!(
            b[0].action,
            Action::Launch {
                path: r"C:\Windows\notepad.exe".into(),
                args: LaunchArgs::List(vec![]),
                cwd: None
            }
        );
        // Optional fields default.
        assert_eq!(b[1].name, "");
        assert!(b[1].enabled);
        assert_eq!(
            b[1].action,
            Action::SendHotkey {
                keys: "ctrl+shift+esc".into()
            }
        );
        assert_eq!(
            b[2].action,
            Action::OpenUrl {
                url: "https://github.com".into()
            }
        );
    }

    #[test]
    fn launch_args_list_raw_and_defaults() {
        let parse = |action: Value| -> Action {
            let v = json!({ "key": "a", "action": action });
            match serde_json::from_value::<BindingEntry>(v).unwrap() {
                BindingEntry::Valid(b) => b.action,
                BindingEntry::Invalid(v) => panic!("invalid {v}"),
            }
        };
        assert_eq!(
            parse(json!({ "type": "launch", "path": "x", "args": ["a b", "c"] })),
            Action::Launch {
                path: "x".into(),
                args: LaunchArgs::List(vec!["a b".into(), "c".into()]),
                cwd: None
            }
        );
        assert_eq!(
            parse(json!({ "type": "launch", "path": "x", "args": "--raw \"q\"", "cwd": "C:\\" })),
            Action::Launch {
                path: "x".into(),
                args: LaunchArgs::Raw("--raw \"q\"".into()),
                cwd: Some("C:\\".into())
            }
        );
        assert_eq!(
            parse(json!({ "type": "launch", "path": "x" })),
            Action::Launch {
                path: "x".into(),
                args: LaunchArgs::default(),
                cwd: None
            }
        );
    }

    #[test]
    fn invalid_bindings_are_kept_verbatim() {
        let input = json!({ "enabled": true, "bindings": [
            { "key": "1", "action": { "type": "run_script", "script": "x.ps1" } },
            { "key": "2" },
            { "action": { "type": "open_url", "url": "https://a.b" } },
            { "key": 3, "action": { "type": "open_url", "url": "https://a.b" } },
            { "key": "4", "action": { "type": "launch" } },
            "not an object",
            { "key": "5", "note": "kept", "action": { "type": "open_url", "url": "https://a.b" } }
        ] });
        let s: Shortcuts = serde_json::from_value(input.clone()).unwrap();
        for e in &s.bindings[..6] {
            assert!(matches!(e, BindingEntry::Invalid(_)), "{e:?}");
        }
        let BindingEntry::Valid(b) = &s.bindings[6] else {
            panic!("expected a valid entry");
        };
        assert_eq!(b.extra["note"], json!("kept"));
        let back = serde_json::to_value(&s).unwrap();
        for i in 0..6 {
            assert_eq!(back["bindings"][i], input["bindings"][i]);
        }
        // Known optional fields are written with their defaults.
        assert_eq!(
            back["bindings"][6],
            json!({ "key": "5", "name": "", "enabled": true, "note": "kept",
                    "action": { "type": "open_url", "url": "https://a.b" } })
        );
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(Config::from_json(b"{ \"version\": 1,").is_err());
    }
}
