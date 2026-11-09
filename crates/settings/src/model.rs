//! The edited `config.json`: loading, saving, change detection and field validation.
//! No UI types, unit-tested. See `docs/specs/settings.md`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use kuality_protocol::store;
use kuality_protocol::{
    Action, BindingEntry, CONFIG_VERSION, Color, Config, Key, KeyParseError, LaunchAction,
    LaunchArgs, Length, Mods, OpenUrlAction, SendHotkeyAction, Trigger, TriggerError, WidgetEntry,
    parse_chord, parse_trigger, quote, valid_url,
};
use kuality_protocol::{
    Anchor, BarModule, BarModuleEntry, BarWidget, CommonItem, Lenient, TodoModule, WidgetItem,
    Widgets,
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

/// A shortcut hotkey (`hyper+1`, `ctrl+t`; a bare key is Hyper + key).
pub fn validate_trigger(text: &str) -> Result<Trigger, &'static str> {
    parse_trigger(text).map_err(|e| match e {
        TriggerError::Key(KeyParseError::Empty) => "Choose a hotkey",
        TriggerError::CapsLock => "CapsLock is the Hyper key itself",
        TriggerError::ModifierKey => "A modifier cannot be the shortcut key",
        TriggerError::HyperWithModifiers => "Hyper cannot be combined with Ctrl, Shift, Alt or Win",
        TriggerError::Reserved => "Windows reserves this hotkey",
        _ => "Unknown key name",
    })
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

/// What the user typed as a website: `github.com` becomes `https://github.com`. Text that has a
/// scheme (`mailto:…`, `ms-settings:…`) or does not look like a host name is kept as typed.
pub fn normalize_url(text: &str) -> String {
    let text = text.trim();
    // `github.com:443/x` parses as a scheme with dots; real schemes have none.
    let has_scheme = valid_url(text)
        && !text
            .split_once(':')
            .is_some_and(|(head, _)| head.contains('.'));
    let looks_like_host =
        text.contains('.') && !text.starts_with('.') && !text.contains(char::is_whitespace);
    if !has_scheme && looks_like_host {
        format!("https://{text}")
    } else {
        text.to_string()
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

/// Bar `gap` bounds (same clamp as the core).
pub const GAP_RANGE: (f64, f64) = (0.0, 200.0);

pub fn parse_gap(text: &str) -> Result<f64, &'static str> {
    let v = parse_number(text)?;
    if (GAP_RANGE.0..=GAP_RANGE.1).contains(&v) {
        Ok(v)
    } else {
        Err("Between 0 and 200")
    }
}

/// Makes sure an enabled bar shows the to-do module (Settings → To do → Enable): enables the
/// first bar that has one, else adds a new bar at the bottom-left of the taskbar. Returns
/// whether `widgets` changed.
pub fn ensure_todo_bar(widgets: &mut Widgets) -> bool {
    let bars = || {
        widgets.items.iter().filter_map(|e| match e {
            WidgetEntry::Known(WidgetItem::Bar(b)) => Some(b),
            _ => None,
        })
    };
    if bars().any(|b| b.common.enabled && b.todo_module().is_some()) {
        return false;
    }
    let first = widgets.items.iter_mut().find_map(|e| match e {
        WidgetEntry::Known(WidgetItem::Bar(b)) if b.todo_module().is_some() => Some(b),
        _ => None,
    });
    if let Some(bar) = first {
        bar.common.enabled = true;
        return true;
    }
    let bar = BarWidget {
        common: CommonItem {
            id: next_widget_id(&widgets.items, "bar"),
            anchor: Lenient::Valid(Anchor::Left),
            x: Lenient::Valid(Length::Px(12.0)),
            ..CommonItem::default()
        },
        modules: vec![BarModuleEntry::Known(
            BarModule::Todo(TodoModule::default()),
        )],
        ..BarWidget::default()
    };
    widgets.items.push(WidgetEntry::Known(WidgetItem::Bar(bar)));
    true
}

/// The to-do module the To do tab edits: the first one of the first bar that has one.
pub fn todo_module_mut(widgets: &mut Widgets) -> Option<&mut TodoModule> {
    widgets.items.iter_mut().find_map(|e| match e {
        WidgetEntry::Known(WidgetItem::Bar(b)) => b.todo_module_mut(),
        _ => None,
    })
}

/// Read-only [`todo_module_mut`], with whether its bar is enabled.
pub fn todo_module(widgets: &Widgets) -> Option<(&TodoModule, bool)> {
    widgets.items.iter().find_map(|e| match e {
        WidgetEntry::Known(WidgetItem::Bar(b)) => b.todo_module().map(|t| (t, b.common.enabled)),
        _ => None,
    })
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
/// First unused id `<prefix>-<n>` (`text-1`, `bar-1`…).
pub fn next_widget_id(items: &[WidgetEntry], prefix: &str) -> String {
    let used: Vec<&str> = items
        .iter()
        .filter_map(|e| match e {
            WidgetEntry::Known(w) => Some(w.common().id.as_str()),
            WidgetEntry::Unknown(v) => v.get("id").and_then(|v| v.as_str()),
        })
        .collect();
    (1..)
        .map(|n| format!("{prefix}-{n}"))
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

/// Another enabled binding uses the same hotkey as `bindings[index]` (core rule: the first
/// enabled one wins). `"1"` and `"hyper+1"` are the same hotkey.
pub fn duplicate_trigger(bindings: &[BindingEntry], index: usize) -> bool {
    let key_of = |e: &BindingEntry| match e {
        BindingEntry::Valid(b) if b.enabled => parse_trigger(&b.key).ok(),
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

// ---------------------------------------------------------------------------------------------
// Shortcut actions as the Shortcuts tab shows them: one label per action and the picker entry
// that edits it. The config format is unchanged; these only read and build `Action`s.

/// Prefix of a `launch` path that starts an app from the shell's Applications folder.
pub const APPS_FOLDER_PREFIX: &str = r"shell:AppsFolder\";

/// The app id (AppUserModelID or the folder's parsing name) of a `shell:AppsFolder\<id>` path.
pub fn app_id(path: &str) -> Option<&str> {
    let path = path.trim();
    let head = path.get(..APPS_FOLDER_PREFIX.len())?;
    if !head.eq_ignore_ascii_case(APPS_FOLDER_PREFIX) {
        return None;
    }
    let id = path[APPS_FOLDER_PREFIX.len()..].trim();
    (!id.is_empty()).then_some(id)
}

/// The `launch` path for an app of the Applications folder.
pub fn app_path(id: &str) -> String {
    format!("{APPS_FOLDER_PREFIX}{id}")
}

/// A readable name for an app id when the app is not (or not yet) in the app list:
/// `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App` → `WindowsCalculator`,
/// `{GUID}\Microsoft Office\root\Office16\EXCEL.EXE` → `EXCEL`, `Org.App.Name` → `Name`.
pub fn fallback_app_name(id: &str) -> String {
    let id = id.trim();
    if id.contains('\\') || id.contains('/') {
        let file = id.rsplit(['\\', '/']).next().unwrap_or(id);
        let stem = Path::new(file)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        return if stem.is_empty() { id.into() } else { stem };
    }
    // Store apps: `<Name>_<PublisherId>!<AppId>`.
    let package = id.split('!').next().unwrap_or(id);
    let name = package.split('_').next().unwrap_or(package);
    let last = name.rsplit('.').next().unwrap_or(name);
    if last.is_empty() {
        id.into()
    } else {
        last.into()
    }
}

/// What a shortcut's action is, as the Action button and the picker see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// `launch` of `shell:AppsFolder\<id>` without arguments.
    App,
    /// `open_url`.
    Website,
    /// `send_hotkey`.
    SendKeys,
    /// `launch` of a document, folder or shortcut, without arguments.
    OpenFile,
    /// `launch` of a program, or of anything with arguments or a working folder.
    RunProgram,
}

/// File extensions that are run rather than opened.
const PROGRAM_EXTENSIONS: [&str; 4] = ["exe", "com", "bat", "cmd"];

fn is_program(path: &str) -> bool {
    Path::new(path.trim().trim_matches('"'))
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PROGRAM_EXTENSIONS.iter().any(|p| e.eq_ignore_ascii_case(p)))
}

/// No arguments (empty list or blank raw string).
pub fn args_empty(args: &LaunchArgs) -> bool {
    match args {
        LaunchArgs::List(list) => list.is_empty(),
        LaunchArgs::Raw(raw) => raw.trim().is_empty(),
    }
}

pub fn kind_of(action: &Action) -> ActionKind {
    match action {
        Action::SendHotkey(_) => ActionKind::SendKeys,
        Action::OpenUrl(_) => ActionKind::Website,
        Action::Launch(l) => {
            let plain = args_empty(&l.args) && l.cwd.as_deref().is_none_or(|c| c.trim().is_empty());
            if !plain {
                ActionKind::RunProgram
            } else if app_id(&l.path).is_some() {
                ActionKind::App
            } else if is_program(&l.path) {
                ActionKind::RunProgram
            } else {
                ActionKind::OpenFile
            }
        }
    }
}

/// How a key is written in labels: `Win`, `Right Ctrl`, `F5`, `Page Up`, `;`, `Volume Mute`.
pub fn key_display(key: Key) -> String {
    let named = |s: &str| s.to_string();
    match key.0 {
        0x30..=0x39 | 0x41..=0x5A => char::from(key.0 as u8).to_string(),
        0x70..=0x87 => format!("F{}", key.0 - 0x70 + 1),
        0x60..=0x69 => format!("Num {}", key.0 - 0x60),
        0xA2 | 0x11 => named("Ctrl"),
        0xA3 => named("Right Ctrl"),
        0xA0 | 0x10 => named("Shift"),
        0xA1 => named("Right Shift"),
        0xA4 | 0x12 => named("Alt"),
        0xA5 => named("Right Alt"),
        0x5B => named("Win"),
        0x5C => named("Right Win"),
        0x1B => named("Esc"),
        0x21 => named("Page Up"),
        0x22 => named("Page Down"),
        0x2C => named("Print Screen"),
        0x91 => named("Scroll Lock"),
        0x90 => named("Num Lock"),
        0x14 => named("CapsLock"),
        0x5D => named("Menu"),
        0x6A => named("Num *"),
        0x6B => named("Num +"),
        0x6D => named("Num -"),
        0x6E => named("Num ."),
        0x6F => named("Num /"),
        0xBA => named(";"),
        0xBB => named("="),
        0xBC => named(","),
        0xBD => named("-"),
        0xBE => named("."),
        0xBF => named("/"),
        0xC0 => named("`"),
        0xDB => named("["),
        0xDC => named("\\"),
        0xDD => named("]"),
        0xDE => named("'"),
        0xB1 => named("Media Previous"),
        _ => {
            let name = key.name();
            if name.starts_with("vk:") {
                return name;
            }
            // `volume_mute` → `Volume Mute`, `oem_8` → `Oem 8`, `space` → `Space`.
            name.split('_')
                .map(capitalize)
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// A `send_hotkey` chord for labels: `win+1` → `Win + 1`. Unparseable text is shown as typed.
pub fn chord_label(keys: &str) -> String {
    match parse_chord(keys) {
        Ok(chord) => chord
            .mods()
            .iter()
            .copied()
            .chain(chord.key)
            .map(key_display)
            .collect::<Vec<_>>()
            .join(" + "),
        Err(_) => keys.trim().to_string(),
    }
}

/// One keycap of a hotkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keycap {
    /// The Hyper key (drawn as a star).
    Hyper,
    Text(String),
}

/// The keycaps of a hotkey: `[Hyper] [1]`, `[Ctrl] [T]`. Text that is not a valid hotkey is
/// one keycap with the raw text. `None` when no hotkey is set yet.
pub fn hotkey_caps(text: &str) -> Option<Vec<Keycap>> {
    if text.trim().is_empty() {
        return None;
    }
    Some(match parse_trigger(text) {
        Ok(t) => {
            let mut caps = Vec::new();
            if t.hyper {
                caps.push(Keycap::Hyper);
            }
            for (m, name) in Mods::ALL.into_iter().zip(["Ctrl", "Shift", "Alt", "Win"]) {
                if t.mods.contains(m) {
                    caps.push(Keycap::Text(name.into()));
                }
            }
            caps.push(Keycap::Text(key_display(t.key)));
            caps
        }
        Err(_) => vec![Keycap::Text(text.trim().to_string())],
    })
}

/// The hotkey as text (accessibility, tests): `Hyper + 1`, `Ctrl + Alt + K`; `None` when unset.
pub fn hotkey_label(text: &str) -> Option<String> {
    hotkey_caps(text).map(|caps| {
        caps.into_iter()
            .map(|c| match c {
                Keycap::Hyper => "Hyper".to_string(),
                Keycap::Text(t) => t,
            })
            .collect::<Vec<_>>()
            .join(" + ")
    })
}

/// The hotkey needs Hyperkey (a Hyper hotkey, bare keys included).
pub fn is_hyper_hotkey(text: &str) -> bool {
    parse_trigger(text).is_ok_and(|t| t.hyper)
}

/// A URL for labels: scheme, `www.` and a trailing `/` dropped for web addresses.
pub fn url_label(url: &str) -> String {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let rest = ["https://", "http://"]
        .iter()
        .find(|p| lower.starts_with(*p))
        .map(|p| &url[p.len()..]);
    match rest {
        Some(rest) => {
            let rest = rest.trim_end_matches('/');
            let rest = if rest.len() > 4 && rest[..4].eq_ignore_ascii_case("www.") {
                &rest[4..]
            } else {
                rest
            };
            if rest.is_empty() {
                url.into()
            } else {
                rest.into()
            }
        }
        None => url.into(),
    }
}

/// Last component of a path (`C:\Windows\notepad.exe` → `notepad.exe`), or the path itself.
fn file_label(path: &str) -> String {
    let path = path.trim().trim_matches('"');
    let trimmed = path.trim_end_matches(['\\', '/']);
    match trimmed.rsplit(['\\', '/']).next() {
        Some(name) if !name.is_empty() => name
            .strip_suffix(".lnk")
            .or_else(|| name.strip_suffix(".LNK"))
            .unwrap_or(name)
            .to_string(),
        _ => path.to_string(),
    }
}

/// The action as a verb and its object: `("Open", "Helium")`, `("Open", "github.com")`,
/// `("Send", "Win + 1")`, `("Run", "notepad.exe …")`. `app_name` looks an app id up in the
/// installed apps.
pub fn action_parts(
    action: &Action,
    app_name: impl Fn(&str) -> Option<String>,
) -> (&'static str, String) {
    match (kind_of(action), action) {
        (ActionKind::App, Action::Launch(l)) => {
            let id = app_id(&l.path).unwrap_or_default();
            (
                "Open",
                app_name(id).unwrap_or_else(|| fallback_app_name(id)),
            )
        }
        (_, Action::OpenUrl(u)) => ("Open", url_label(&u.url)),
        (_, Action::SendHotkey(h)) => ("Send", chord_label(&h.keys)),
        (ActionKind::RunProgram, Action::Launch(l)) => {
            let name = match app_id(&l.path) {
                Some(id) => app_name(id).unwrap_or_else(|| fallback_app_name(id)),
                None => file_label(&l.path),
            };
            if args_empty(&l.args) {
                ("Run", name)
            } else {
                ("Run", format!("{name} …"))
            }
        }
        (_, Action::Launch(l)) => ("Open", file_label(&l.path)),
    }
}

/// Grey placeholder of an empty Name field: the action with its verb (`Open Helium`).
pub fn name_placeholder(action: &Action, app_name: impl Fn(&str) -> Option<String>) -> String {
    let (verb, object) = action_parts(action, app_name);
    format!("{verb} {object}")
}

/// A choice made in the action picker.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    /// An app of the Applications folder, by id.
    App(String),
    Website(String),
    SendKeys(String),
    OpenFile(String),
    RunProgram {
        path: String,
        args: LaunchArgs,
        cwd: Option<String>,
    },
}

/// Builds the action for a picker choice. Unknown fields of `prev` survive when it has the same
/// `type` (e.g. an app replaced by a program keeps the extra fields of the `launch` object).
pub fn build_action(choice: Choice, prev: Option<&Action>) -> Action {
    let launch_extra = || match prev {
        Some(Action::Launch(l)) => l.extra.clone(),
        _ => Default::default(),
    };
    match choice {
        Choice::App(id) => Action::Launch(LaunchAction {
            path: app_path(id.trim()),
            args: LaunchArgs::List(Vec::new()),
            cwd: None,
            extra: launch_extra(),
        }),
        Choice::OpenFile(path) => Action::Launch(LaunchAction {
            path: path.trim().to_string(),
            args: LaunchArgs::List(Vec::new()),
            cwd: None,
            extra: launch_extra(),
        }),
        Choice::RunProgram { path, args, cwd } => Action::Launch(LaunchAction {
            path: path.trim().to_string(),
            args,
            cwd: cwd.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
            extra: launch_extra(),
        }),
        Choice::Website(url) => Action::OpenUrl(OpenUrlAction {
            url: url.trim().to_string(),
            extra: match prev {
                Some(Action::OpenUrl(u)) => u.extra.clone(),
                _ => Default::default(),
            },
        }),
        Choice::SendKeys(keys) => Action::SendHotkey(SendHotkeyAction {
            keys: keys.trim().to_string(),
            extra: match prev {
                Some(Action::SendHotkey(h)) => h.extra.clone(),
                _ => Default::default(),
            },
        }),
    }
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
        assert_eq!(validate_trigger("g"), Ok(Trigger::hyper(Key(0x47))));
        assert_eq!(validate_trigger(" F5 "), Ok(Trigger::hyper(Key(0x74))));
        assert_eq!(validate_trigger("hyper+g"), Ok(Trigger::hyper(Key(0x47))));
        let t = validate_trigger("ctrl+t").unwrap();
        assert!(!t.hyper && t.mods == Mods::CTRL);
        assert_eq!(validate_trigger(""), Err("Choose a hotkey"));
        assert!(validate_trigger("capslock").is_err());
        assert!(validate_trigger("ctrl").is_err());
        assert!(validate_trigger("nope").is_err());
        assert_eq!(
            validate_trigger("win+l"),
            Err("Windows reserves this hotkey")
        );
        assert!(validate_trigger("hyper+alt+k").is_err());
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
        assert_eq!(next_widget_id(&[], "text"), "text-1");
        assert_eq!(next_widget_id(&[], "bar"), "bar-1");
        let items = vec![
            text("text-1"),
            WidgetEntry::Unknown(json!({ "type": "media", "id": "text-2" })),
            text("text-4"),
        ];
        assert_eq!(next_widget_id(&items, "text"), "text-3");
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
            pass_through: false,
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
            binding("hyper+G", true),
            binding("h", true),
            binding("h", false),
            binding("ctrl+h", true),
            binding("alt+rctrl+k", true),
            binding("ctrl+alt+k", true),
        ];
        assert!(duplicate_trigger(&b, 0));
        assert!(duplicate_trigger(&b, 1));
        assert!(!duplicate_trigger(&b, 2));
        assert!(!duplicate_trigger(&b, 3));
        assert!(!duplicate_trigger(&b, 4), "ctrl+h is not hyper+h");
        assert!(duplicate_trigger(&b, 5) && duplicate_trigger(&b, 6));
    }
    fn launch(path: &str, args: LaunchArgs, cwd: Option<&str>) -> Action {
        Action::Launch(LaunchAction {
            path: path.into(),
            args,
            cwd: cwd.map(Into::into),
            extra: Default::default(),
        })
    }

    fn no_args() -> LaunchArgs {
        LaunchArgs::List(Vec::new())
    }

    #[test]
    fn apps_folder_paths() {
        let id = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";
        assert_eq!(app_path(id), format!(r"shell:AppsFolder\{id}"));
        assert_eq!(app_id(&app_path(id)), Some(id));
        assert_eq!(app_id(r" SHELL:appsfolder\Chrome "), Some("Chrome"));
        assert_eq!(app_id(r"shell:AppsFolder\"), None);
        assert_eq!(app_id(r"C:\Windows\notepad.exe"), None);
        assert_eq!(app_id("shell:"), None);
        // Multi-byte text around the prefix length must not panic.
        assert_eq!(app_id("ññññññññññññññññ"), None);
    }

    #[test]
    fn fallback_app_names() {
        assert_eq!(
            fallback_app_name("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
            "WindowsCalculator"
        );
        assert_eq!(
            fallback_app_name(
                r"{6D809377-6AF0-444B-8957-A3773F02200E}\Microsoft Office\root\Office16\EXCEL.EXE"
            ),
            "EXCEL"
        );
        assert_eq!(fallback_app_name("Chrome"), "Chrome");
        assert_eq!(fallback_app_name("Microsoft.Windows.Explorer"), "Explorer");
        assert_eq!(fallback_app_name("Org.App."), "Org.App.");
    }

    #[test]
    fn action_kinds() {
        let app = app_path("Helium.ABC");
        assert_eq!(kind_of(&launch(&app, no_args(), None)), ActionKind::App);
        assert_eq!(
            kind_of(&launch(&app, LaunchArgs::Raw("--x".into()), None)),
            ActionKind::RunProgram
        );
        assert_eq!(
            kind_of(&launch(r"C:\Windows\notepad.exe", no_args(), None)),
            ActionKind::RunProgram
        );
        assert_eq!(
            kind_of(&launch(r"C:\Tools\build.CMD", no_args(), None)),
            ActionKind::RunProgram
        );
        assert_eq!(
            kind_of(&launch(r"C:\Users\me\Documents", no_args(), None)),
            ActionKind::OpenFile
        );
        assert_eq!(
            kind_of(&launch(
                r"C:\notes.txt",
                LaunchArgs::Raw("  ".into()),
                Some(" ")
            )),
            ActionKind::OpenFile
        );
        assert_eq!(
            kind_of(&launch(r"C:\notes.txt", no_args(), Some(r"C:\"))),
            ActionKind::RunProgram
        );
        let url = build_action(Choice::Website("https://a.b".into()), None);
        assert_eq!(kind_of(&url), ActionKind::Website);
        let keys = build_action(Choice::SendKeys("win+1".into()), None);
        assert_eq!(kind_of(&keys), ActionKind::SendKeys);
    }

    #[test]
    fn key_and_chord_labels() {
        assert_eq!(chord_label("win+1"), "Win + 1");
        assert_eq!(chord_label("ctrl+shift+esc"), "Ctrl + Shift + Esc");
        assert_eq!(chord_label("rctrl + f5"), "Right Ctrl + F5");
        assert_eq!(chord_label("volume_mute"), "Volume Mute");
        assert_eq!(chord_label("win"), "Win");
        assert_eq!(chord_label("ctrl+oem_plus"), "Ctrl + =");
        assert_eq!(chord_label("alt+pageup"), "Alt + Page Up");
        assert_eq!(
            chord_label(" a+b "),
            "a+b",
            "unparseable text is shown as typed"
        );
        assert_eq!(hotkey_label("1").as_deref(), Some("Hyper + 1"));
        assert_eq!(hotkey_label("hyper+1").as_deref(), Some("Hyper + 1"));
        assert_eq!(hotkey_label("G").as_deref(), Some("Hyper + G"));
        assert_eq!(hotkey_label("space").as_deref(), Some("Hyper + Space"));
        assert_eq!(
            hotkey_label("alt+ctrl+k").as_deref(),
            Some("Ctrl + Alt + K")
        );
        assert_eq!(hotkey_label("win+e").as_deref(), Some("Win + E"));
        assert_eq!(hotkey_label("hyper").as_deref(), Some("hyper"));
        assert_eq!(hotkey_label(" "), None);
        assert_eq!(
            hotkey_caps("hyper+pageup"),
            Some(vec![Keycap::Hyper, Keycap::Text("Page Up".into())])
        );
        assert!(is_hyper_hotkey("1") && is_hyper_hotkey("hyper+1"));
        assert!(!is_hyper_hotkey("ctrl+1") && !is_hyper_hotkey("bad key"));
        assert_eq!(key_display(Key(0xE8)), "vk:0xE8");
        assert_eq!(key_display(Key(0xDF)), "Oem 8");
    }

    #[test]
    fn url_labels() {
        assert_eq!(url_label("https://github.com"), "github.com");
        assert_eq!(url_label("https://www.github.com/"), "github.com");
        assert_eq!(url_label("HTTP://example.org/a/b"), "example.org/a/b");
        assert_eq!(url_label("mailto:me@x.y"), "mailto:me@x.y");
        assert_eq!(url_label("ms-settings:display"), "ms-settings:display");
        assert_eq!(url_label("https://"), "https://");
    }

    #[test]
    fn typed_websites_get_https() {
        assert_eq!(normalize_url(" github.com "), "https://github.com");
        assert_eq!(
            normalize_url("github.com:443/x"),
            "https://github.com:443/x"
        );
        assert_eq!(normalize_url("http://a.b"), "http://a.b");
        assert_eq!(normalize_url("mailto:me@x.y"), "mailto:me@x.y");
        assert_eq!(normalize_url("ms-settings:display"), "ms-settings:display");
        assert_eq!(normalize_url("not a url"), "not a url");
        assert_eq!(normalize_url(".hidden"), ".hidden");
    }

    /// The action in words as the Action button shows it: an app is just its name, the rest
    /// `Open github.com`, `Send Win + 1`, `Run notepad.exe`.
    fn action_label(action: &Action, app_name: impl Fn(&str) -> Option<String>) -> String {
        let (verb, object) = action_parts(action, app_name);
        if kind_of(action) == ActionKind::App {
            object
        } else {
            format!("{verb} {object}")
        }
    }

    #[test]
    fn action_labels() {
        let names = |id: &str| (id == "Helium.ABC").then(|| "Helium".to_string());
        let app = |id: &str| launch(&app_path(id), no_args(), None);
        assert_eq!(action_label(&app("Helium.ABC"), names), "Helium");
        assert_eq!(name_placeholder(&app("Helium.ABC"), names), "Open Helium");
        assert_eq!(
            action_parts(&app("Helium.ABC"), names),
            ("Open", "Helium".to_string())
        );
        let keys = build_action(Choice::SendKeys("win+1".into()), None);
        assert_eq!(name_placeholder(&keys, names), "Send Win + 1");
        assert_eq!(
            action_label(&app("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), names),
            "WindowsCalculator",
            "unknown apps fall back to a name derived from the id"
        );
        assert_eq!(
            action_label(
                &launch(
                    &app_path("Helium.ABC"),
                    LaunchArgs::Raw("--new".into()),
                    None
                ),
                names
            ),
            "Run Helium …"
        );
        assert_eq!(
            action_label(&launch(r"C:\Windows\notepad.exe", no_args(), None), names),
            "Run notepad.exe"
        );
        assert_eq!(
            action_label(
                &launch(
                    r"%WINDIR%\notepad.exe",
                    LaunchArgs::List(vec!["a b.txt".into()]),
                    None
                ),
                names
            ),
            "Run notepad.exe …"
        );
        assert_eq!(
            action_label(&launch(r"C:\Users\me\Documents\", no_args(), None), names),
            "Open Documents"
        );
        assert_eq!(
            action_label(&launch(r"C:\Links\Excel.lnk", no_args(), None), names),
            "Open Excel"
        );
        assert_eq!(
            action_label(
                &build_action(Choice::Website("https://github.com".into()), None),
                names
            ),
            "Open github.com"
        );
        assert_eq!(
            action_label(&build_action(Choice::SendKeys("win+1".into()), None), names),
            "Send Win + 1"
        );
    }

    #[test]
    fn picker_choices_build_actions_and_keep_unknown_fields() {
        let mut extra = serde_json::Map::new();
        extra.insert("note".into(), json!(1));
        let prev = Action::Launch(LaunchAction {
            path: "x.exe".into(),
            args: LaunchArgs::Raw("--a".into()),
            cwd: Some(r"C:\".into()),
            extra: extra.clone(),
        });
        let Action::Launch(l) = build_action(Choice::App(" Helium.ABC ".into()), Some(&prev))
        else {
            panic!("launch expected");
        };
        assert_eq!(l.path, r"shell:AppsFolder\Helium.ABC");
        assert_eq!(l.args, no_args());
        assert_eq!(l.cwd, None);
        assert_eq!(l.extra, extra);
        let run = build_action(
            Choice::RunProgram {
                path: r" C:\Windows\notepad.exe ".into(),
                args: LaunchArgs::Raw("a.txt".into()),
                cwd: Some("  ".into()),
            },
            None,
        );
        assert_eq!(
            run,
            launch(
                r"C:\Windows\notepad.exe",
                LaunchArgs::Raw("a.txt".into()),
                None
            )
        );
        assert_eq!(kind_of(&run), ActionKind::RunProgram);
        let file = build_action(Choice::OpenFile(r"C:\a.txt".into()), None);
        assert_eq!(kind_of(&file), ActionKind::OpenFile);
        // Another type starts without the old unknown fields.
        let Action::OpenUrl(u) = build_action(Choice::Website("https://a.b".into()), Some(&prev))
        else {
            panic!("open_url expected");
        };
        assert!(u.extra.is_empty());
        let prev_keys = Action::SendHotkey(SendHotkeyAction {
            keys: "win+d".into(),
            extra: extra.clone(),
        });
        let Action::SendHotkey(h) =
            build_action(Choice::SendKeys("win+1".into()), Some(&prev_keys))
        else {
            panic!("send_hotkey expected");
        };
        assert_eq!((h.keys.as_str(), &h.extra), ("win+1", &extra));
    }

    #[test]
    fn ensure_todo_bar_adds_or_enables_once() {
        let mut w = Widgets::default();
        assert!(ensure_todo_bar(&mut w));
        let WidgetEntry::Known(WidgetItem::Bar(b)) = &w.items[0] else {
            panic!("expected a bar");
        };
        assert_eq!(b.common.id, "bar-1");
        assert!(b.common.enabled);
        assert_eq!(b.common.x, Lenient::Valid(Length::Px(12.0)));
        assert!(b.todo_module().is_some());
        assert!(!ensure_todo_bar(&mut w), "already there");
        assert_eq!(w.items.len(), 1);

        // A disabled bar with the module is turned on instead of adding another.
        if let WidgetEntry::Known(item) = &mut w.items[0] {
            item.common_mut().enabled = false;
        }
        assert!(ensure_todo_bar(&mut w));
        assert_eq!(w.items.len(), 1);
        assert_eq!(todo_module(&w).map(|(_, on)| on), Some(true));
        todo_module_mut(&mut w).unwrap().label = "Tasks".into();
        assert_eq!(todo_module(&w).unwrap().0.label, "Tasks");
    }

    #[test]
    fn gap_rule() {
        assert_eq!(parse_gap("16"), Ok(16.0));
        assert!(parse_gap("-1").is_err());
        assert!(parse_gap("201").is_err());
        assert!(parse_gap("x").is_err());
    }
}
