//! The edited `config.json`: loading, saving, change detection and field validation.
//! No UI types, unit-tested. See `docs/specs/settings.md`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use kuality_protocol::store;
use kuality_protocol::{
    BindingEntry, CONFIG_VERSION, Color, Config, Key, LaunchArgs, Length, WidgetEntry, parse_chord,
    parse_key, quote, valid_url,
};

/// What the file on disk looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskState {
    /// No file (yet): the window shows defaults, the first save creates it.
    Missing,
    /// The file parsed; `config` mirrors it plus the user's edits.
    Valid,
    /// The file is not a valid config: the window is read-only until it is fixed.
    Invalid(String),
}

/// Result of [`Document::save`].
#[derive(Debug)]
pub enum SaveOutcome {
    /// Bytes written.
    Written,
    /// Same bytes as on disk: nothing written.
    Unchanged,
    /// The file on disk is invalid: never overwritten.
    Blocked,
}

/// Result of [`Document::poll`].
#[derive(Debug, PartialEq, Eq)]
pub enum PollOutcome {
    /// Nothing changed on disk (or only our own write).
    Unchanged,
    /// The file changed and is valid: `config` was replaced.
    Reloaded,
    /// The file changed and is invalid.
    BecameInvalid,
    /// The file disappeared; the current values stay and the next save recreates it.
    Removed,
}

/// Size and modification time, a cheap "did it change" check before reading the file.
type Stamp = (u64, Option<SystemTime>);

/// The config being edited and what is known about the file.
#[derive(Debug)]
pub struct Document {
    path: PathBuf,
    /// The values shown and saved.
    pub config: Config,
    state: DiskState,
    /// Bytes last read or written, to recognize our own writes and unchanged files.
    disk_bytes: Option<Vec<u8>>,
    stamp: Option<Stamp>,
}

impl Document {
    /// Reads `path` (missing is fine).
    pub fn load(path: PathBuf) -> Self {
        let mut doc = Self {
            path,
            config: Config::default(),
            state: DiskState::Missing,
            disk_bytes: None,
            stamp: None,
        };
        match fs::read(&doc.path) {
            Ok(bytes) => {
                doc.take_bytes(bytes);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => doc.state = DiskState::Invalid(format!("cannot read the file: {e}")),
        }
        doc.stamp = stamp(&doc.path);
        doc
    }

    /// Applies freshly read bytes. Returns whether they parsed.
    fn take_bytes(&mut self, bytes: Vec<u8>) -> bool {
        let ok = match Config::from_json(&bytes) {
            Ok(c) => {
                self.config = c;
                self.state = DiskState::Valid;
                true
            }
            Err(e) => {
                self.state = DiskState::Invalid(e.to_string());
                false
            }
        };
        self.disk_bytes = Some(bytes);
        ok
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn state(&self) -> &DiskState {
        &self.state
    }

    /// Editing is off while the file on disk is invalid.
    pub fn read_only(&self) -> bool {
        matches!(self.state, DiskState::Invalid(_))
    }

    /// The file's `version` is newer than this build understands.
    pub fn newer_version(&self) -> bool {
        self.state == DiskState::Valid && self.config.version > CONFIG_VERSION
    }

    /// Writes `config` if it differs from the file.
    pub fn save(&mut self) -> io::Result<SaveOutcome> {
        if self.read_only() {
            return Ok(SaveOutcome::Blocked);
        }
        let bytes = self.config.to_json_pretty().map_err(io::Error::other)?;
        if self.state == DiskState::Valid && self.disk_bytes.as_deref() == Some(bytes.as_slice()) {
            return Ok(SaveOutcome::Unchanged);
        }
        store::write_atomic(&self.path, &bytes)?;
        self.disk_bytes = Some(bytes);
        self.state = DiskState::Valid;
        self.stamp = stamp(&self.path);
        Ok(SaveOutcome::Written)
    }

    /// Checks the file for changes made by someone else (an editor, another tool).
    pub fn poll(&mut self) -> PollOutcome {
        let now = stamp(&self.path);
        if now == self.stamp {
            return PollOutcome::Unchanged;
        }
        self.stamp = now;
        match fs::read(&self.path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if self.state == DiskState::Missing {
                    return PollOutcome::Unchanged;
                }
                self.disk_bytes = None;
                self.state = DiskState::Missing;
                PollOutcome::Removed
            }
            // Probably locked by the writer: retried on the next poll.
            Err(_) => {
                self.stamp = None;
                PollOutcome::Unchanged
            }
            Ok(bytes) => {
                if self.disk_bytes.as_deref() == Some(bytes.as_slice()) {
                    return PollOutcome::Unchanged;
                }
                if self.take_bytes(bytes) {
                    PollOutcome::Reloaded
                } else {
                    PollOutcome::BecameInvalid
                }
            }
        }
    }
}

fn stamp(path: &Path) -> Option<Stamp> {
    let m = fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()))
}

// ---------------------------------------------------------------------------------------------
// Field validation. Every function returns the value to store or a short message for the UI.

/// A shortcut trigger key name.
pub fn validate_trigger(name: &str) -> Result<Key, &'static str> {
    if name.trim().is_empty() {
        return Err("Choose a key");
    }
    let key = parse_key(name).map_err(|_| "Unknown key name")?;
    if key == Key::CAPSLOCK {
        return Err("CapsLock is the Hyper key itself");
    }
    if !key.is_trigger() {
        return Err("A modifier cannot be the shortcut key");
    }
    Ok(key)
}

/// A `send_hotkey` chord.
pub fn validate_chord(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("Record or type a key combination".into());
    }
    parse_chord(text)
        .map(|_| ())
        .map_err(|e| capitalize(&e.to_string()))
}

/// An `open_url` value.
pub fn validate_url(text: &str) -> Result<(), &'static str> {
    if valid_url(text.trim()) {
        Ok(())
    } else {
        Err("Needs a scheme such as https: (at most 2048 characters)")
    }
}

/// A `launch` path.
pub fn validate_path(text: &str) -> Result<(), &'static str> {
    if text.trim().is_empty() {
        Err("Choose a program, file or folder")
    } else {
        Ok(())
    }
}

/// A finite number typed in a number field.
pub fn parse_number(text: &str) -> Result<f64, &'static str> {
    match text.trim().parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err("Enter a number"),
    }
}

/// Unit of a [`Length`] in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Px,
    Pct,
}

impl Unit {
    pub fn of(l: Length) -> Self {
        match l {
            Length::Px(_) => Self::Px,
            Length::Pct(_) => Self::Pct,
        }
    }

    pub fn make(self, v: f64) -> Length {
        match self {
            Self::Px => Length::Px(v),
            Self::Pct => Length::Pct(v),
        }
    }
}

/// The number of a [`Length`].
pub fn length_value(l: Length) -> f64 {
    match l {
        Length::Px(v) | Length::Pct(v) => v,
    }
}

/// A width/height number: finite and not negative.
pub fn parse_size(text: &str) -> Result<f64, &'static str> {
    let v = parse_number(text)?;
    if v < 0.0 {
        Err("Must not be negative")
    } else {
        Ok(v)
    }
}

/// Font size bounds (same clamp as the core).
pub const FONT_SIZE_RANGE: (f64, f64) = (4.0, 200.0);

pub fn parse_font_size(text: &str) -> Result<f64, &'static str> {
    let v = parse_number(text)?;
    if (FONT_SIZE_RANGE.0..=FONT_SIZE_RANGE.1).contains(&v) {
        Ok(v)
    } else {
        Err("Between 4 and 200")
    }
}

pub fn parse_color(text: &str) -> Result<Color, &'static str> {
    Color::parse(text.trim()).ok_or("Use #RRGGBB or #AARRGGBB")
}

pub fn validate_font_family(text: &str) -> Result<(), &'static str> {
    if text.trim().is_empty() {
        Err("Enter a font name")
    } else {
        Ok(())
    }
}

/// A number for display: no trailing `.0`.
pub fn format_number(v: f64) -> String {
    format!("{v}")
}

/// `launch.args` as one command line.
pub fn args_display(args: &LaunchArgs) -> String {
    match args {
        LaunchArgs::List(list) => quote::join_args(list),
        LaunchArgs::Raw(raw) => raw.clone(),
    }
}

/// Smallest unused `text-<n>` id.
pub fn next_widget_id(items: &[WidgetEntry]) -> String {
    let used: Vec<&str> = items
        .iter()
        .filter_map(|e| match e {
            WidgetEntry::Known(w) => Some(w.common().id.as_str()),
            WidgetEntry::Unknown(v) => v.get("id").and_then(|v| v.as_str()),
        })
        .collect();
    (1..)
        .map(|n| format!("text-{n}"))
        .find(|id| !used.contains(&id.as_str()))
        .expect("an unused id exists")
}

/// Another known item has the same non-empty id as `items[index]` (core rule: the first wins).
pub fn duplicate_widget_id(items: &[WidgetEntry], index: usize) -> bool {
    let id_of = |e: &WidgetEntry| match e {
        WidgetEntry::Known(w) => Some(w.common().id.clone()),
        WidgetEntry::Unknown(_) => None,
    };
    let Some(id) = items.get(index).and_then(id_of).filter(|id| !id.is_empty()) else {
        return false;
    };
    items
        .iter()
        .enumerate()
        .any(|(i, e)| i != index && id_of(e).as_deref() == Some(id.as_str()))
}

/// Another enabled binding uses the same trigger key as `bindings[index]` (core rule: the first
/// enabled one wins).
pub fn duplicate_trigger(bindings: &[BindingEntry], index: usize) -> bool {
    let key_of = |e: &BindingEntry| match e {
        BindingEntry::Valid(b) if b.enabled => parse_key(&b.key).ok(),
        _ => None,
    };
    let Some(key) = bindings.get(index).and_then(key_of) else {
        return false;
    };
    bindings
        .iter()
        .enumerate()
        .any(|(i, e)| i != index && key_of(e) == Some(key))
}

/// The `type` of an unknown widget entry, for its placeholder row.
pub fn unknown_type(v: &serde_json::Value) -> String {
    v.get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("?")
        .to_string()
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuality_protocol::test_util::TempDir;
    use kuality_protocol::{Action, Binding, OpenUrlAction, TextWidget, WidgetItem};
    use serde_json::{Value, json};

    fn write(path: &Path, v: &Value) {
        fs::write(path, serde_json::to_vec_pretty(v).unwrap()).unwrap();
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn missing_file_shows_defaults_and_first_save_creates_it() {
        let dir = TempDir::new("missing");
        let path = dir.0.join("sub").join("config.json");
        let mut doc = Document::load(path.clone());
        assert_eq!(doc.state(), &DiskState::Missing);
        assert_eq!(doc.config, Config::default());
        assert!(!path.exists(), "loading never writes");
        doc.config.hyperkey.enabled = true;
        assert!(matches!(doc.save().unwrap(), SaveOutcome::Written));
        assert_eq!(read(&path)["hyperkey"]["enabled"], json!(true));
        assert_eq!(doc.state(), &DiskState::Valid);
    }

    #[test]
    fn unknown_fields_survive_an_edit_at_every_level() {
        let dir = TempDir::new("unknown");
        let path = dir.0.join("config.json");
        let input = json!({
            "version": 1,
            "future": { "a": 1 },
            "general": { "autostart": true, "run_elevated": false, "log_level": "info", "g": 2 },
            "hyperkey": { "enabled": false, "double_tap": true },
            "shortcuts": { "enabled": true, "s": 3, "bindings": [
                { "key": "1", "name": "", "enabled": true, "b": 4,
                  "action": { "type": "open_url", "url": "https://a.b", "inner": 5 } },
                { "key": "2", "action": { "type": "run_script", "script": "x" } }
            ] },
            "widgets": { "enabled": false, "hide_on_fullscreen": true, "w": 6, "items": [
                { "type": "media", "id": "m" },
                { "type": "text", "id": "t", "text": "hi", "x": "10 em", "note": 7 }
            ] }
        });
        write(&path, &input);
        let mut doc = Document::load(path.clone());
        assert_eq!(doc.state(), &DiskState::Valid);
        doc.config.hyperkey.enabled = true;
        doc.save().unwrap();
        let out = read(&path);
        assert_eq!(out["hyperkey"]["enabled"], json!(true));
        assert_eq!(out["future"], input["future"]);
        assert_eq!(out["general"]["g"], json!(2));
        assert_eq!(out["general"]["autostart"], json!(true));
        assert_eq!(out["hyperkey"]["double_tap"], json!(true));
        assert_eq!(out["shortcuts"]["s"], json!(3));
        assert_eq!(out["shortcuts"]["bindings"][0]["b"], json!(4));
        assert_eq!(out["shortcuts"]["bindings"][0]["action"]["inner"], json!(5));
        assert_eq!(
            out["shortcuts"]["bindings"][1],
            input["shortcuts"]["bindings"][1]
        );
        assert_eq!(out["widgets"]["w"], json!(6));
        assert_eq!(out["widgets"]["items"][0], input["widgets"]["items"][0]);
        assert_eq!(out["widgets"]["items"][1]["x"], json!("10 em"));
        assert_eq!(out["widgets"]["items"][1]["note"], json!(7));
    }

    #[test]
    fn unchanged_config_is_not_rewritten() {
        let dir = TempDir::new("unchanged");
        let path = dir.0.join("config.json");
        fs::write(&path, Config::default().to_json_pretty().unwrap()).unwrap();
        let mut doc = Document::load(path.clone());
        assert!(matches!(doc.save().unwrap(), SaveOutcome::Unchanged));
        // A hand-formatted file is only rewritten when something changes.
        fs::write(&path, br#"{"hyperkey":{"enabled":true}}"#).unwrap();
        let mut doc = Document::load(path.clone());
        assert!(matches!(doc.save().unwrap(), SaveOutcome::Written));
        assert!(matches!(doc.save().unwrap(), SaveOutcome::Unchanged));
    }

    #[test]
    fn invalid_file_blocks_writes_until_fixed() {
        let dir = TempDir::new("invalid");
        let path = dir.0.join("config.json");
        fs::write(&path, b"{ \"version\": 1,").unwrap();
        let mut doc = Document::load(path.clone());
        assert!(doc.read_only());
        assert!(matches!(doc.state(), DiskState::Invalid(_)));
        doc.config.hyperkey.enabled = true;
        assert!(matches!(doc.save().unwrap(), SaveOutcome::Blocked));
        assert_eq!(fs::read(&path).unwrap(), b"{ \"version\": 1,");
        // Fixed by hand → reloaded and writable again.
        write(&path, &json!({ "widgets": { "enabled": true } }));
        assert_eq!(doc.poll(), PollOutcome::Reloaded);
        assert!(!doc.read_only());
        assert!(doc.config.widgets.enabled);
        assert!(!doc.config.hyperkey.enabled, "values come from the file");
    }

    #[test]
    fn poll_detects_external_changes_but_not_our_own_writes() {
        let dir = TempDir::new("poll");
        let path = dir.0.join("config.json");
        let mut doc = Document::load(path.clone());
        assert_eq!(doc.poll(), PollOutcome::Unchanged);
        doc.config.shortcuts.enabled = true;
        doc.save().unwrap();
        assert_eq!(doc.poll(), PollOutcome::Unchanged);
        write(&path, &json!({ "hyperkey": { "enabled": true } }));
        assert_eq!(doc.poll(), PollOutcome::Reloaded);
        assert!(doc.config.hyperkey.enabled);
        assert!(!doc.config.shortcuts.enabled);
        fs::write(&path, b"nope").unwrap();
        assert_eq!(doc.poll(), PollOutcome::BecameInvalid);
        assert!(doc.read_only());
        fs::remove_file(&path).unwrap();
        assert_eq!(doc.poll(), PollOutcome::Removed);
        assert!(!doc.read_only());
        assert_eq!(doc.poll(), PollOutcome::Unchanged);
    }

    #[test]
    fn newer_version_is_reported_and_kept() {
        let dir = TempDir::new("newer");
        let path = dir.0.join("config.json");
        write(&path, &json!({ "version": CONFIG_VERSION + 1 }));
        let mut doc = Document::load(path.clone());
        assert!(doc.newer_version());
        assert!(!doc.read_only());
        doc.config.hyperkey.enabled = true;
        doc.save().unwrap();
        assert_eq!(read(&path)["version"], json!(CONFIG_VERSION + 1));
    }

    #[test]
    fn trigger_validation() {
        assert_eq!(validate_trigger("g"), Ok(Key(0x47)));
        assert_eq!(validate_trigger(" F5 "), Ok(Key(0x74)));
        assert!(validate_trigger("").is_err());
        assert!(validate_trigger("capslock").is_err());
        assert!(validate_trigger("ctrl").is_err());
        assert!(validate_trigger("nope").is_err());
    }

    #[test]
    fn chord_url_path_validation() {
        assert!(validate_chord("ctrl+shift+esc").is_ok());
        assert!(validate_chord("win").is_ok());
        assert!(validate_chord("").is_err());
        assert!(validate_chord("a+b").is_err());
        assert!(validate_url(" https://x.y ").is_ok());
        assert!(validate_url("x.y").is_err());
        assert!(validate_path("  ").is_err());
        assert!(validate_path("notepad.exe").is_ok());
    }

    #[test]
    fn number_fields() {
        assert_eq!(parse_number(" -12.5 "), Ok(-12.5));
        assert!(parse_number("1e999").is_err());
        assert!(parse_number("NaN").is_err());
        assert!(parse_number("").is_err());
        assert!(parse_size("-1").is_err());
        assert_eq!(parse_size("0"), Ok(0.0));
        assert_eq!(parse_font_size("4"), Ok(4.0));
        assert!(parse_font_size("3.9").is_err());
        assert!(parse_font_size("201").is_err());
        assert_eq!(format_number(12.0), "12");
        assert_eq!(format_number(-0.5), "-0.5");
        assert_eq!(Unit::Pct.make(5.0), Length::Pct(5.0));
        assert_eq!(Unit::of(Length::Px(1.0)), Unit::Px);
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("#80FF8000").is_ok());
    }

    #[test]
    fn args_are_shown_as_one_command_line() {
        assert_eq!(
            args_display(&LaunchArgs::List(vec!["a b".into(), "c".into()])),
            "\"a b\" c"
        );
        assert_eq!(
            args_display(&LaunchArgs::Raw("--x \"y\"".into())),
            "--x \"y\""
        );
    }

    fn text(id: &str) -> WidgetEntry {
        WidgetEntry::Known(WidgetItem::Text(TextWidget {
            common: kuality_protocol::CommonItem {
                id: id.into(),
                ..Default::default()
            },
            ..TextWidget::default()
        }))
    }

    #[test]
    fn widget_ids() {
        assert_eq!(next_widget_id(&[]), "text-1");
        let items = vec![
            text("text-1"),
            WidgetEntry::Unknown(json!({ "type": "media", "id": "text-2" })),
            text("text-4"),
        ];
        assert_eq!(next_widget_id(&items), "text-3");
        let items = vec![text("a"), text("b"), text("a"), text("")];
        assert!(duplicate_widget_id(&items, 0));
        assert!(duplicate_widget_id(&items, 2));
        assert!(!duplicate_widget_id(&items, 1));
        assert!(!duplicate_widget_id(&items, 3));
    }

    fn binding(key: &str, enabled: bool) -> BindingEntry {
        BindingEntry::Valid(Binding {
            key: key.into(),
            name: String::new(),
            enabled,
            action: Action::OpenUrl(OpenUrlAction {
                url: "https://a.b".into(),
                extra: Default::default(),
            }),
            extra: Default::default(),
        })
    }

    #[test]
    fn duplicate_triggers_count_enabled_bindings_and_aliases() {
        let b = vec![
            binding("g", true),
            binding("G", true),
            binding("h", true),
            binding("h", false),
        ];
        assert!(duplicate_trigger(&b, 0));
        assert!(duplicate_trigger(&b, 1));
        assert!(!duplicate_trigger(&b, 2));
        assert!(!duplicate_trigger(&b, 3));
    }
}
