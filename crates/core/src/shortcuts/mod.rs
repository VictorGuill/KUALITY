//! Shortcuts: a hotkey (Hyper+key or modifiers+key) runs a configured action. See
//! `docs/specs/shortcuts.md`.
//!
//! [`compile`] turns the `shortcuts` config section into a lookup table for the keyboard thread
//! (validated, ready to run). It runs on the main thread on every config load. The clipboard
//! history hotkey (`clipboard.hotkey`, `docs/specs/clipboard.md`) is compiled into the same
//! table as a built-in first entry.

use std::sync::atomic::{AtomicU64, Ordering};

use kuality_protocol::{
    Action, BindingEntry, Chord, Clipboard, Key, LaunchAction, LaunchArgs, Mods, OpenUrlAction,
    SendHotkeyAction, Shortcuts, Trigger, TriggerError, parse_chord, parse_trigger, valid_url,
};

use kuality_protocol::quote;

/// `source` of built-in entries (not from `shortcuts.bindings`).
pub const BUILTIN_SOURCE: usize = usize::MAX;

/// Source of [`CompiledBindings::generation`].
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Bindings ready for the keyboard thread.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompiledBindings {
    /// Incremented on every compile; lets the keyboard thread drop actions that were triggered
    /// against an older table.
    pub generation: u64,
    pub entries: Vec<CompiledBinding>,
}

/// One usable binding.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledBinding {
    pub trigger: Trigger,
    /// The hotkey also reaches the app in front.
    pub pass_through: bool,
    /// Position in `shortcuts.bindings` (for logs); [`BUILTIN_SOURCE`] for the clipboard hotkey.
    pub source: usize,
    pub action: CompiledAction,
}

/// A validated action.
#[derive(Debug, Clone, PartialEq)]
pub enum CompiledAction {
    /// `params` is the joined, quoted argument string. `%VAR%` is expanded at execution time.
    Launch {
        path: String,
        params: String,
        cwd: Option<String>,
    },
    SendHotkey(Chord),
    OpenUrl(String),
    /// Built-in: open (or close) the clipboard history popup.
    OpenClipboard,
}

impl CompiledAction {
    /// Config name of the action type (for logs).
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Launch { .. } => "launch",
            Self::SendHotkey(_) => "send_hotkey",
            Self::OpenUrl(_) => "open_url",
            Self::OpenClipboard => "open_clipboard",
        }
    }
}

impl CompiledBindings {
    /// Index of the Hyper binding of `key`. Linear scan, no allocation (called from the
    /// keyboard hook).
    pub fn find_hyper(&self, key: Key) -> Option<u16> {
        self.find(Trigger::hyper(key))
    }

    /// Index of the binding for exactly `mods` + `key` (not Hyper). Same rules as above.
    pub fn find_normal(&self, mods: Mods, key: Key) -> Option<u16> {
        self.find(Trigger {
            hyper: false,
            mods,
            key,
        })
    }

    fn find(&self, trigger: Trigger) -> Option<u16> {
        self.entries
            .iter()
            .position(|b| b.trigger == trigger)
            .and_then(|i| u16::try_from(i).ok())
    }

    /// Whether binding `index` lets its hotkey reach the app too.
    pub fn passes_through(&self, index: u16) -> bool {
        self.entries
            .get(usize::from(index))
            .is_some_and(|b| b.pass_through)
    }

    /// At least one binding is a normal (non-Hyper) hotkey: the hook is needed even without
    /// Hyperkey.
    pub fn has_normal(&self) -> bool {
        self.entries.iter().any(|b| !b.trigger.hyper)
    }

    /// At least one binding needs Hyperkey.
    pub fn has_hyper(&self) -> bool {
        self.entries.iter().any(|b| b.trigger.hyper)
    }
}

/// Why a binding was skipped. Never carries config text (only the reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    InvalidEntry,
    BadTrigger(TriggerError),
    ModifierTrigger,
    CapsLockTrigger,
    ReservedTrigger,
    EmptyPath,
    BadChord(kuality_protocol::KeyParseError),
    BadUrl,
    Duplicate,
    TooMany,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEntry => {
                f.write_str("not a valid binding (missing field or unknown action type)")
            }
            Self::BadTrigger(e) => write!(f, "bad hotkey: {e}"),
            Self::ModifierTrigger => f.write_str("a modifier cannot be a trigger key"),
            Self::CapsLockTrigger => f.write_str("capslock cannot be a trigger key"),
            Self::ReservedTrigger => {
                f.write_str("Windows reserves this hotkey (Ctrl+Alt+Del, Win+L)")
            }
            Self::EmptyPath => f.write_str("launch path is empty"),
            Self::BadChord(e) => write!(f, "bad send_hotkey keys: {e}"),
            Self::BadUrl => {
                f.write_str("open_url needs a scheme (e.g. https:) and at most 2048 characters")
            }
            Self::Duplicate => f.write_str("hotkey already used by an earlier binding"),
            Self::TooMany => f.write_str("too many bindings"),
        }
    }
}

/// Compiles the config sections, logging a warning for every skipped binding.
pub fn compile(shortcuts: &Shortcuts, clipboard: &Clipboard) -> CompiledBindings {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let (compiled, skipped) = build(shortcuts, clipboard, generation);
    for (index, reason) in skipped {
        if index == BUILTIN_SOURCE {
            log::warn!("clipboard hotkey skipped: {reason}");
        } else {
            log::warn!("shortcut binding #{index} skipped: {reason}");
        }
    }
    if shortcuts.enabled {
        let user = compiled
            .entries
            .iter()
            .filter(|b| b.source != BUILTIN_SOURCE)
            .count();
        log::info!("shortcuts: {user} binding(s) active");
    }
    compiled
}

/// Pure part of [`compile`]: the table plus `(binding index, reason)` for each skipped entry
/// ([`BUILTIN_SOURCE`] for the clipboard hotkey).
fn build(
    shortcuts: &Shortcuts,
    clipboard: &Clipboard,
    generation: u64,
) -> (CompiledBindings, Vec<(usize, SkipReason)>) {
    let mut out = CompiledBindings {
        generation,
        entries: Vec::new(),
    };
    let mut skipped = Vec::new();
    // First, so it wins over a shortcut with the same hotkey; independent of `shortcuts.enabled`.
    if clipboard.enabled {
        match clipboard.hotkey_text() {
            None => skipped.push((BUILTIN_SOURCE, SkipReason::InvalidEntry)),
            Some(key) => match compile_trigger(key) {
                Ok(trigger) => out.entries.push(CompiledBinding {
                    trigger,
                    pass_through: false,
                    source: BUILTIN_SOURCE,
                    action: CompiledAction::OpenClipboard,
                }),
                Err(reason) => skipped.push((BUILTIN_SOURCE, reason)),
            },
        }
    }
    if !shortcuts.enabled {
        return (out, skipped);
    }
    for (index, entry) in shortcuts.bindings.iter().enumerate() {
        let binding = match entry {
            BindingEntry::Valid(b) => b,
            BindingEntry::Invalid(_) => {
                skipped.push((index, SkipReason::InvalidEntry));
                continue;
            }
        };
        if !binding.enabled {
            continue;
        }
        let result = compile_one(&binding.key, &binding.action).and_then(|(trigger, action)| {
            if out.entries.iter().any(|b| b.trigger == trigger) {
                Err(SkipReason::Duplicate)
            } else if out.entries.len() >= usize::from(u16::MAX) {
                Err(SkipReason::TooMany)
            } else {
                Ok(CompiledBinding {
                    trigger,
                    pass_through: binding.pass_through,
                    source: index,
                    action,
                })
            }
        });
        match result {
            Ok(b) => out.entries.push(b),
            Err(reason) => skipped.push((index, reason)),
        }
    }
    (out, skipped)
}

fn compile_trigger(key: &str) -> Result<Trigger, SkipReason> {
    parse_trigger(key).map_err(|e| match e {
        TriggerError::CapsLock => SkipReason::CapsLockTrigger,
        TriggerError::ModifierKey => SkipReason::ModifierTrigger,
        TriggerError::Reserved => SkipReason::ReservedTrigger,
        e => SkipReason::BadTrigger(e),
    })
}

fn compile_one(key: &str, action: &Action) -> Result<(Trigger, CompiledAction), SkipReason> {
    let trigger = compile_trigger(key)?;
    let action = match action {
        Action::Launch(LaunchAction {
            path, args, cwd, ..
        }) => {
            if path.trim().is_empty() {
                return Err(SkipReason::EmptyPath);
            }
            let params = match args {
                LaunchArgs::List(list) => quote::join_args(list),
                LaunchArgs::Raw(raw) => raw.clone(),
            };
            CompiledAction::Launch {
                path: path.trim().to_string(),
                params,
                cwd: cwd
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .map(String::from),
            }
        }
        Action::SendHotkey(SendHotkeyAction { keys, .. }) => {
            CompiledAction::SendHotkey(parse_chord(keys).map_err(SkipReason::BadChord)?)
        }
        Action::OpenUrl(OpenUrlAction { url, .. }) => {
            let url = url.trim();
            if !valid_url(url) {
                return Err(SkipReason::BadUrl);
            }
            CompiledAction::OpenUrl(url.to_string())
        }
    };
    Ok((trigger, action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shortcuts(enabled: bool, bindings: serde_json::Value) -> Shortcuts {
        serde_json::from_value(json!({ "enabled": enabled, "bindings": bindings })).unwrap()
    }

    fn url(u: &str) -> serde_json::Value {
        json!({ "type": "open_url", "url": u })
    }

    #[test]
    fn compiles_the_spec_example() {
        let s = shortcuts(
            true,
            json!([
                { "key": "1", "name": "Notepad", "enabled": true,
                  "action": { "type": "launch", "path": "C:\\Windows\\notepad.exe", "args": [], "cwd": null } },
                { "key": "2", "action": { "type": "send_hotkey", "keys": "ctrl+shift+esc" } },
                { "key": "G", "action": url("https://github.com") }
            ]),
        );
        let (c, skipped) = build(&s, &Clipboard::default(), 7);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(c.generation, 7);
        assert_eq!(c.entries.len(), 3);
        assert_eq!(c.entries[0].trigger, Trigger::hyper(Key(0x31)));
        assert!(!c.entries[0].pass_through);
        assert_eq!(
            c.entries[0].action,
            CompiledAction::Launch {
                path: r"C:\Windows\notepad.exe".into(),
                params: String::new(),
                cwd: None
            }
        );
        assert_eq!(
            c.entries[1].action,
            CompiledAction::SendHotkey(parse_chord("ctrl+shift+esc").unwrap())
        );
        assert_eq!(c.entries[2].trigger, Trigger::hyper(Key(0x47)));
        assert_eq!(
            c.entries[2].action,
            CompiledAction::OpenUrl("https://github.com".into())
        );
        assert_eq!(c.find_hyper(Key(0x47)), Some(2));
        assert_eq!(c.find_hyper(Key(0x48)), None);
        assert_eq!(c.find_normal(Mods::NONE, Key(0x47)), None);
        assert!(c.has_hyper() && !c.has_normal());
    }

    #[test]
    fn disabled_section_compiles_to_nothing() {
        let s = shortcuts(
            false,
            json!([{ "key": "1", "action": url("https://a.b") }, "junk"]),
        );
        let (c, skipped) = build(&s, &Clipboard::default(), 1);
        assert!(c.entries.is_empty());
        assert!(skipped.is_empty());
    }

    #[test]
    fn duplicates_first_enabled_wins() {
        let s = shortcuts(
            true,
            json!([
                { "key": "a", "enabled": false, "action": url("https://disabled") },
                { "key": "A", "action": url("https://first") },
                { "key": " a ", "action": url("https://second") },
                { "key": "vk:0x41", "action": url("https://third") },
                { "key": "hyper+a", "action": url("https://fourth") }
            ]),
        );
        let (c, skipped) = build(&s, &Clipboard::default(), 1);
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.entries[0].source, 1);
        assert_eq!(
            c.entries[0].action,
            CompiledAction::OpenUrl("https://first".into())
        );
        assert_eq!(
            skipped,
            vec![
                (2, SkipReason::Duplicate),
                (3, SkipReason::Duplicate),
                (4, SkipReason::Duplicate)
            ]
        );
    }

    #[test]
    fn normal_hotkeys_compile_with_their_modifiers() {
        let s = shortcuts(
            true,
            json!([
                { "key": "ctrl+t", "action": url("https://a") },
                { "key": "win+e", "pass_through": true, "action": url("https://b") },
                { "key": "alt+ctrl+k", "action": url("https://c") },
                { "key": "rctrl+lalt+k", "action": url("https://dup") },
                { "key": "t", "action": url("https://hyper-t") },
                { "key": "ctrl+alt+del", "action": url("https://x") },
                { "key": "win+l", "action": url("https://x") },
                { "key": "hyper+ctrl+k", "action": url("https://x") }
            ]),
        );
        let (c, skipped) = build(&s, &Clipboard::default(), 1);
        assert_eq!(
            skipped,
            vec![
                (3, SkipReason::Duplicate),
                (5, SkipReason::ReservedTrigger),
                (6, SkipReason::ReservedTrigger),
                (7, SkipReason::BadTrigger(TriggerError::HyperWithModifiers)),
            ]
        );
        let t = Key(0x54);
        assert_eq!(c.find_normal(Mods::CTRL, t), Some(0));
        assert_eq!(c.find_normal(Mods::CTRL | Mods::SHIFT, t), None);
        assert_eq!(c.find_hyper(t), Some(3));
        assert_eq!(c.find_normal(Mods::WIN, Key(0x45)), Some(1));
        assert!(c.passes_through(1) && !c.passes_through(0));
        assert_eq!(c.find_normal(Mods::CTRL | Mods::ALT, Key(0x4B)), Some(2));
        assert!(c.has_normal() && c.has_hyper());
    }

    #[test]
    fn invalid_entries_are_skipped_and_the_rest_kept() {
        let s = shortcuts(
            true,
            json!([
                { "key": "hyper", "action": url("https://a.b") },
                { "key": "shift", "action": url("https://a.b") },
                { "key": "capslock", "action": url("https://a.b") },
                { "key": "1", "action": { "type": "launch", "path": "  " } },
                { "key": "2", "action": { "type": "send_hotkey", "keys": "a+b" } },
                { "key": "3", "action": url("github.com") },
                { "key": "4", "action": url("1http://x") },
                { "key": "5", "action": url(&format!("https://{}", "x".repeat(2041))) },
                { "key": "6", "action": { "type": "run_script" } },
                { "key": "7", "action": url("mailto:someone@example.com") },
                { "key": "8", "action": url("ms-settings:display") },
                { "key": "9", "action": url(&format!("https://{}", "x".repeat(2040))) }
            ]),
        );
        let (c, skipped) = build(&s, &Clipboard::default(), 1);
        assert_eq!(
            skipped,
            vec![
                (
                    0,
                    SkipReason::BadTrigger(TriggerError::Key(
                        kuality_protocol::KeyParseError::Unknown
                    ))
                ),
                (1, SkipReason::ModifierTrigger),
                (2, SkipReason::CapsLockTrigger),
                (3, SkipReason::EmptyPath),
                (
                    4,
                    SkipReason::BadChord(kuality_protocol::KeyParseError::NotAModifier)
                ),
                (5, SkipReason::BadUrl),
                (6, SkipReason::BadUrl),
                (7, SkipReason::BadUrl),
                (8, SkipReason::InvalidEntry),
            ]
        );
        let sources: Vec<usize> = c.entries.iter().map(|b| b.source).collect();
        assert_eq!(sources, vec![9, 10, 11]);
    }

    #[test]
    fn launch_args_are_quoted_or_kept_raw() {
        let s = shortcuts(
            true,
            json!([
                { "key": "1", "action": { "type": "launch", "path": " %WINDIR%\\notepad.exe ",
                                          "args": ["C:\\Temp\\a b.txt"], "cwd": " " } },
                { "key": "2", "action": { "type": "launch", "path": "x.exe",
                                          "args": "--raw \"a b\"", "cwd": "%TEMP%" } }
            ]),
        );
        let (c, _) = build(&s, &Clipboard::default(), 1);
        assert_eq!(
            c.entries[0].action,
            CompiledAction::Launch {
                path: r"%WINDIR%\notepad.exe".into(),
                params: r#""C:\Temp\a b.txt""#.into(),
                cwd: None
            }
        );
        assert_eq!(
            c.entries[1].action,
            CompiledAction::Launch {
                path: "x.exe".into(),
                params: r#"--raw "a b""#.into(),
                cwd: Some("%TEMP%".into())
            }
        );
    }

    #[test]
    fn generation_increases_on_every_compile() {
        let s = Shortcuts::default();
        let a = compile(&s, &Clipboard::default()).generation;
        let b = compile(&s, &Clipboard::default()).generation;
        assert!(b > a);
    }

    fn clipboard(enabled: bool, hotkey: serde_json::Value) -> Clipboard {
        serde_json::from_value(json!({ "enabled": enabled, "hotkey": hotkey })).unwrap()
    }

    #[test]
    fn clipboard_hotkey_is_a_first_builtin_entry() {
        let s = shortcuts(
            true,
            json!([
                { "key": "hyper+v", "action": url("https://loses") },
                { "key": "1", "action": url("https://one") }
            ]),
        );
        let (c, skipped) = build(&s, &clipboard(true, json!("hyper+v")), 1);
        assert_eq!(skipped, vec![(0, SkipReason::Duplicate)]);
        assert_eq!(c.entries.len(), 2);
        assert_eq!(c.entries[0].source, BUILTIN_SOURCE);
        assert_eq!(c.entries[0].action, CompiledAction::OpenClipboard);
        assert_eq!(c.find_hyper(Key(0x56)), Some(0));
        // Works with shortcuts off; Win+V is a normal hotkey.
        let off = shortcuts(false, json!([]));
        let (c, skipped) = build(&off, &clipboard(true, json!("win+v")), 1);
        assert!(skipped.is_empty());
        assert_eq!(c.find_normal(Mods::WIN, Key(0x56)), Some(0));
        assert!(c.has_normal() && !c.has_hyper());
        // Disabled feature: nothing.
        let (c, _) = build(&off, &clipboard(false, json!("win+v")), 1);
        assert!(c.entries.is_empty());
    }

    #[test]
    fn bad_clipboard_hotkeys_are_skipped() {
        let off = shortcuts(false, json!([]));
        for (hotkey, reason) in [
            (json!(3), SkipReason::InvalidEntry),
            (json!(""), SkipReason::InvalidEntry),
            (json!("win+l"), SkipReason::ReservedTrigger),
            (json!("capslock"), SkipReason::CapsLockTrigger),
        ] {
            let (c, skipped) = build(&off, &clipboard(true, hotkey), 1);
            assert!(c.entries.is_empty());
            assert_eq!(skipped, vec![(BUILTIN_SOURCE, reason)]);
        }
    }

    #[test]
    fn skip_reasons_never_echo_config_text() {
        let s = shortcuts(
            true,
            json!([{ "key": "secret-key-name", "action": url("https://a.b") }]),
        );
        let (_, skipped) = build(&s, &Clipboard::default(), 1);
        assert!(!skipped[0].1.to_string().contains("secret"));
    }
}
