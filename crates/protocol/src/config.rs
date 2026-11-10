//! `config.json` types.
//!
//! Rules (see `docs/specs/skeleton.md`):
//! - Every struct uses `#[serde(default)]`: missing fields fall back to defaults, `{}` is valid.
//! - Every feature is disabled by default (exception: `updates.check`, see `Updates`).
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
    pub updates: Updates,
    pub todo: Todo,
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
            updates: Updates::default(),
            todo: Todo::default(),
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
    // Start at logon is not stored here: the Run value / scheduled task itself is the source of
    // truth (`docs/specs/distribution.md`). Old `autostart` / `run_elevated` keys stay in `extra`.
    /// Minimum level written to the log file.
    pub log_level: LogLevel,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `updates` section (see `docs/specs/updates.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    /// Check GitHub for a newer release (at start, then every few hours). The one feature that
    /// is **on** by default (user decision 2026-11-09): installing still needs a click.
    pub check: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            check: true,
            extra: Extra::new(),
        }
    }
}

/// `todo` section (see `docs/specs/todo.md`). The items themselves live in `todos.json`
/// ([`crate::todo`]), not here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Todo {
    pub enabled: bool,
    /// Where a new item goes in the list.
    pub new_items: Lenient<NewItems>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `"top" | "bottom"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewItems {
    #[default]
    Top,
    Bottom,
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

/// `hyperkey` section (see `docs/specs/hyperkey.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hyperkey {
    pub enabled: bool,
    /// Modifiers apps see while Hyper is held (default: none, Hyper only triggers Kuality
    /// shortcuts). The old `include_win` key is kept in `extra` and ignored.
    pub modifiers: HyperModifiers,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Hyperkey {
    fn default() -> Self {
        Self {
            enabled: false,
            modifiers: HyperModifiers::None,
            extra: Extra::new(),
        }
    }
}

/// What holding Hyper presses for other apps, snake_case in JSON.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HyperModifiers {
    /// Nothing: Hyper only exists for Kuality's own shortcuts.
    #[default]
    None,
    CtrlShiftAlt,
    CtrlShiftAltWin,
}

impl HyperModifiers {
    /// Hyper injects modifiers at all.
    pub fn injects(self) -> bool {
        self != Self::None
    }

    /// Hyper presses Win (the Office key combination).
    pub fn includes_win(self) -> bool {
        self == Self::CtrlShiftAltWin
    }
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
// Invalid entries are rare and lists are short: boxing would only complicate matching.
#[allow(clippy::large_enum_variant)]
pub enum BindingEntry {
    Valid(Binding),
    Invalid(Value),
}

/// The hotkey `key` runs `action`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    /// Trigger chord (see `keys::parse_trigger`): `hyper+1`, `ctrl+t`; a bare key means Hyper +
    /// key. Validated when bindings are compiled.
    pub key: String,
    /// Label for the settings UI; not used by the core.
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The hotkey also reaches the app in front (default: Kuality swallows it).
    #[serde(default)]
    pub pass_through: bool,
    pub action: Action,
    #[serde(flatten)]
    pub extra: Extra,
}

fn default_true() -> bool {
    true
}

/// What a binding does, tagged by `type`. Each variant keeps its own unknown fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Open a program, document, folder or `shell:` path.
    Launch(LaunchAction),
    /// Send one key chord, e.g. `"ctrl+shift+esc"`.
    SendHotkey(SendHotkeyAction),
    /// Open a URI with its default handler (`https:`, `mailto:`, `ms-settings:`…).
    OpenUrl(OpenUrlAction),
}

impl Action {
    /// The `type` value.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Launch(_) => "launch",
            Self::SendHotkey(_) => "send_hotkey",
            Self::OpenUrl(_) => "open_url",
        }
    }
}

/// `type: "launch"`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LaunchAction {
    pub path: String,
    #[serde(default)]
    pub args: LaunchArgs,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `type: "send_hotkey"`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SendHotkeyAction {
    pub keys: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `type: "open_url"`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenUrlAction {
    pub url: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Longest accepted `open_url` value, in characters.
pub const MAX_URL_LEN: usize = 2048;

/// `open_url` rule: `^[A-Za-z][A-Za-z0-9+.-]*:` and at most [`MAX_URL_LEN`] characters. The
/// caller trims surrounding whitespace first.
pub fn valid_url(url: &str) -> bool {
    if url.chars().count() > MAX_URL_LEN {
        return false;
    }
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
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

/// `widgets` section (see `docs/specs/overlay.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Widgets {
    pub enabled: bool,
    pub hide_on_fullscreen: bool,
    pub items: Vec<WidgetEntry>,
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

/// One entry of `widgets.items`. Entries that do not parse as a [`WidgetItem`] (unknown `type`
/// such as `media` before Phase 4, missing `type`, wrong field types…) are kept as raw JSON so
/// they survive a save, and ignored at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
// Unknown entries are rare and lists are short: boxing would only complicate matching.
#[allow(clippy::large_enum_variant)]
pub enum WidgetEntry {
    Known(WidgetItem),
    Unknown(Value),
}

/// A widget of a known type, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WidgetItem {
    /// Static text.
    Text(TextWidget),
    /// A row of modules (to-do, later media…) sharing one position.
    Bar(BarWidget),
}

impl WidgetItem {
    /// Fields shared by every widget type.
    pub fn common(&self) -> &CommonItem {
        match self {
            Self::Text(t) => &t.common,
            Self::Bar(b) => &b.common,
        }
    }

    /// Mutable [`Self::common`].
    pub fn common_mut(&mut self) -> &mut CommonItem {
        match self {
            Self::Text(t) => &mut t.common,
            Self::Bar(b) => &mut b.common,
        }
    }

    /// The `type` value, for logs.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Bar(_) => "bar",
        }
    }
}

/// Fields shared by every widget type (flattened into the item object).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommonItem {
    /// Stable identifier; empty means "use `#<index>`".
    pub id: String,
    pub enabled: bool,
    pub anchor: Lenient<Anchor>,
    pub x: Lenient<Length>,
    pub y: Lenient<Length>,
    pub origin: Lenient<Origin>,
    pub width: Lenient<Dimension>,
    pub height: Lenient<Dimension>,
}

impl Default for CommonItem {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
            anchor: Lenient::Valid(Anchor::Left),
            x: Lenient::Valid(Length::Px(0.0)),
            y: Lenient::Valid(Length::Pct(50.0)),
            origin: Lenient::Valid(Origin::Left),
            width: Lenient::Valid(Dimension::Auto),
            height: Lenient::Valid(Dimension::Auto),
        }
    }
}

/// `type: "text"`: a static text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextWidget {
    #[serde(flatten)]
    pub common: CommonItem,
    /// `\n` makes extra lines; no word wrap.
    pub text: String,
    pub font_family: String,
    /// In DIPs.
    pub font_size: Lenient<f64>,
    pub font_weight: Lenient<FontWeight>,
    /// `None` = theme color.
    pub color: Lenient<Option<Color>>,
    pub align: Lenient<TextAlign>,
    /// Unknown fields; declared last so known fields are taken first.
    #[serde(flatten)]
    pub extra: Extra,
}

/// Default `font_family` of a text widget.
pub const DEFAULT_FONT_FAMILY: &str = "Segoe UI Variable Text";
/// Default `font_size` of a text widget, in DIPs.
pub const DEFAULT_FONT_SIZE: f64 = 12.0;

impl Default for TextWidget {
    fn default() -> Self {
        Self {
            common: CommonItem::default(),
            text: String::new(),
            font_family: DEFAULT_FONT_FAMILY.into(),
            font_size: Lenient::Valid(DEFAULT_FONT_SIZE),
            font_weight: Lenient::Valid(FontWeight::Normal),
            color: Lenient::Valid(None),
            align: Lenient::Valid(TextAlign::Left),
            extra: Extra::new(),
        }
    }
}

/// `type: "bar"`: modules laid out side by side, left to right, at one position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BarWidget {
    #[serde(flatten)]
    pub common: CommonItem,
    pub modules: Vec<BarModuleEntry>,
    pub font_family: String,
    /// In DIPs.
    pub font_size: Lenient<f64>,
    pub font_weight: Lenient<FontWeight>,
    /// `None` = theme color.
    pub color: Lenient<Option<Color>>,
    /// Space between modules, in DIPs.
    pub gap: Lenient<f64>,
    /// Unknown fields; declared last so known fields are taken first.
    #[serde(flatten)]
    pub extra: Extra,
}

/// Default `gap` of a bar, in DIPs.
pub const DEFAULT_BAR_GAP: f64 = 16.0;

impl Default for BarWidget {
    fn default() -> Self {
        Self {
            common: CommonItem::default(),
            modules: Vec::new(),
            font_family: DEFAULT_FONT_FAMILY.into(),
            font_size: Lenient::Valid(DEFAULT_FONT_SIZE),
            font_weight: Lenient::Valid(FontWeight::Semibold),
            color: Lenient::Valid(None),
            gap: Lenient::Valid(DEFAULT_BAR_GAP),
            extra: Extra::new(),
        }
    }
}

impl BarWidget {
    /// The first to-do module, if any.
    pub fn todo_module(&self) -> Option<&TodoModule> {
        self.modules.iter().find_map(|m| match m {
            BarModuleEntry::Known(BarModule::Todo(t)) => Some(t),
            BarModuleEntry::Unknown(_) => None,
        })
    }

    /// Mutable [`Self::todo_module`].
    pub fn todo_module_mut(&mut self) -> Option<&mut TodoModule> {
        self.modules.iter_mut().find_map(|m| match m {
            BarModuleEntry::Known(BarModule::Todo(t)) => Some(t),
            BarModuleEntry::Unknown(_) => None,
        })
    }
}

/// One entry of a bar's `modules`. Unknown module types (e.g. `media` before Phase 4) are kept
/// as raw JSON so they survive a save, and ignored at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BarModuleEntry {
    Known(BarModule),
    Unknown(Value),
}

/// A bar module of a known type, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BarModule {
    /// `TO DO (3)`: opens the to-do list on click.
    Todo(TodoModule),
}

/// `type: "todo"` bar module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TodoModule {
    pub label: String,
    /// Append ` (n)` with the number of pending items (nothing when 0).
    pub show_count: bool,
    /// Resting the mouse on the module shows the list (it closes again when the mouse leaves,
    /// unless it was clicked).
    pub open_on_hover: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Default to-do module label.
pub const DEFAULT_TODO_LABEL: &str = "TO DO";

impl Default for TodoModule {
    fn default() -> Self {
        Self {
            label: DEFAULT_TODO_LABEL.into(),
            show_count: true,
            open_on_hover: true,
            extra: Extra::new(),
        }
    }
}

/// A field value that may be invalid. Invalid values are kept verbatim (so a save does not lose
/// them) and the reader falls back to the field default with a warning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Lenient<T> {
    Valid(T),
    Invalid(Value),
}

impl<T: Default> Default for Lenient<T> {
    fn default() -> Self {
        Self::Valid(T::default())
    }
}

impl<T> Lenient<T> {
    /// The value, or `None` if it was invalid.
    pub fn valid(&self) -> Option<&T> {
        match self {
            Self::Valid(v) => Some(v),
            Self::Invalid(_) => None,
        }
    }
}

/// Horizontal reference point of the taskbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Left,
    Center,
    Right,
}

impl Anchor {
    /// Fraction of the taskbar width: 0, 0.5 or 1.
    pub fn fraction(self) -> f64 {
        match self {
            Self::Left => 0.0,
            Self::Center => 0.5,
            Self::Right => 1.0,
        }
    }
}

/// Which point of the widget is placed at (x, y).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Origin {
    /// `(ox, oy)` fractions of the widget size.
    pub fn fractions(self) -> (f64, f64) {
        match self {
            Self::TopLeft => (0.0, 0.0),
            Self::Top => (0.5, 0.0),
            Self::TopRight => (1.0, 0.0),
            Self::Left => (0.0, 0.5),
            Self::Center => (0.5, 0.5),
            Self::Right => (1.0, 0.5),
            Self::BottomLeft => (0.0, 1.0),
            Self::Bottom => (0.5, 1.0),
            Self::BottomRight => (1.0, 1.0),
        }
    }
}

/// `"normal" | "semibold" | "bold"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontWeight {
    Normal,
    Semibold,
    Bold,
}

/// Horizontal text alignment inside the widget box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// A length: `"<n>px"` (DIPs), `"<n>%"`, or a JSON number (= px). Written back as `"<n>px"` /
/// `"<n>%"`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f64),
    Pct(f64),
}

impl Length {
    /// Parses `"<n>px"` or `"<n>%"` (surrounding spaces allowed; the number must be finite).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (num, make): (&str, fn(f64) -> Self) = if let Some(n) = s.strip_suffix("px") {
            (n, Self::Px)
        } else {
            (s.strip_suffix('%')?, Self::Pct)
        };
        // `f64::from_str` also accepts "inf" / "NaN": rejected by the finiteness check.
        let v: f64 = num.trim_end().parse().ok()?;
        v.is_finite().then(|| make(v))
    }
}

impl std::fmt::Display for Length {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Px(v) => write!(f, "{v}px"),
            Self::Pct(v) => write!(f, "{v}%"),
        }
    }
}

impl Serialize for Length {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Length {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match d.deserialize_any(LengthVisitor { allow_auto: false })? {
            Some(l) => Ok(l),
            None => Err(serde::de::Error::custom("\"auto\" is not allowed here")),
        }
    }
}

/// A width or height: `"auto"` (content size) or a [`Length`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dimension {
    Auto,
    Length(Length),
}

impl Serialize for Dimension {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => s.serialize_str("auto"),
            Self::Length(l) => l.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Dimension {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(LengthVisitor { allow_auto: true })
            .map(|v| v.map_or(Self::Auto, Self::Length))
    }
}

/// Visits a length; `Ok(None)` is `"auto"` (only when allowed).
struct LengthVisitor {
    allow_auto: bool,
}

impl serde::de::Visitor<'_> for LengthVisitor {
    type Value = Option<Length>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(r#"a length like "10px", "50%" or a number"#)?;
        if self.allow_auto {
            f.write_str(r#", or "auto""#)?;
        }
        Ok(())
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        if self.allow_auto && v.trim() == "auto" {
            return Ok(None);
        }
        Length::parse(v)
            .map(Some)
            .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Str(v), &self))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
        if v.is_finite() {
            Ok(Some(Length::Px(v)))
        } else {
            Err(E::invalid_value(serde::de::Unexpected::Float(v), &self))
        }
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(Some(Length::Px(v as f64)))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(Some(Length::Px(v as f64)))
    }
}

/// A color `"#RRGGBB"` or `"#AARRGGBB"` (straight alpha). Written back uppercase, as `#RRGGBB`
/// when fully opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// Parses `#RRGGBB` or `#AARRGGBB` (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.strip_prefix('#')?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) || !matches!(hex.len(), 6 | 8) {
            return None;
        }
        let [a, r, g, b] = u32::from_str_radix(hex, 16).ok()?.to_be_bytes();
        let a = if hex.len() == 6 { 255 } else { a };
        Some(Self { a, r, g, b })
    }
}

impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let text = if self.a == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.a, self.r, self.g, self.b)
        };
        s.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| {
            serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(&s),
                &r##"a color "#RRGGBB" or "#AARRGGBB""##,
            )
        })
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
        assert!(!c.hyperkey.enabled);
        assert_eq!(c.hyperkey.modifiers, HyperModifiers::None);
        assert!(!c.shortcuts.enabled);
        assert!(c.shortcuts.bindings.is_empty());
        assert!(!c.widgets.enabled);
        assert!(c.widgets.items.is_empty());
        assert!(!c.todo.enabled);
        assert_eq!(c.todo.new_items, Lenient::Valid(NewItems::Top));
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
        assert_eq!(
            c.hyperkey.modifiers,
            HyperModifiers::None,
            "missing modifiers is none"
        );
        assert_eq!(c.general.log_level, LogLevel::Debug);
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
        // Phase 0 placeholder, now an unknown key: kept as is.
        assert_eq!(c.general.extra["autostart"], json!(true));
        let back = serde_json::to_value(&c).unwrap();
        // Known fields missing from the input are written with their defaults.
        let mut expected = input;
        expected["general"]["log_level"] = json!("info");
        expected["hyperkey"]["modifiers"] = json!("none");
        expected["updates"] = json!({ "check": true });
        expected["todo"] = json!({ "enabled": false, "new_items": "top" });
        assert_eq!(back, expected);
    }

    #[test]
    fn serialized_defaults_match_documented_json() {
        let expected = json!({
            "version": 1,
            "general": { "log_level": "info" },
            "hyperkey": { "enabled": false, "modifiers": "none" },
            "shortcuts": { "enabled": false, "bindings": [] },
            "widgets": { "enabled": false, "hide_on_fullscreen": true, "items": [] },
            "updates": { "check": true },
            "todo": { "enabled": false, "new_items": "top" }
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
            Action::Launch(LaunchAction {
                path: r"C:\Windows\notepad.exe".into(),
                args: LaunchArgs::List(vec![]),
                cwd: None,
                extra: Extra::new()
            })
        );
        // Optional fields default.
        assert_eq!(b[1].name, "");
        assert!(b[1].enabled);
        assert!(!b[1].pass_through, "pass_through defaults to false");
        assert_eq!(
            b[1].action,
            Action::SendHotkey(SendHotkeyAction {
                keys: "ctrl+shift+esc".into(),
                extra: Extra::new()
            })
        );
        assert_eq!(
            b[2].action,
            Action::OpenUrl(OpenUrlAction {
                url: "https://github.com".into(),
                extra: Extra::new()
            })
        );
    }

    #[test]
    fn modifiers_and_pass_through_are_read() {
        let c = Config::from_json(
            br#"{ "hyperkey": { "enabled": true, "modifiers": "ctrl_shift_alt" },
                  "shortcuts": { "bindings": [
                    { "key": "ctrl+t", "pass_through": true,
                      "action": { "type": "open_url", "url": "https://a.b" } } ] } }"#,
        )
        .unwrap();
        assert_eq!(c.hyperkey.modifiers, HyperModifiers::CtrlShiftAlt);
        let BindingEntry::Valid(b) = &c.shortcuts.bindings[0] else {
            panic!("invalid");
        };
        assert!(b.pass_through);
        assert_eq!(b.key, "ctrl+t");
    }

    #[test]
    fn old_include_win_is_kept_but_ignored() {
        let c = Config::from_json(br#"{ "hyperkey": { "enabled": true, "include_win": true } }"#)
            .unwrap();
        assert_eq!(c.hyperkey.modifiers, HyperModifiers::None);
        assert_eq!(c.hyperkey.extra["include_win"], json!(true));
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["hyperkey"]["modifiers"], json!("none"));
        assert!(
            Config::from_json(br#"{ "hyperkey": { "modifiers": "win" } }"#).is_err(),
            "unknown modifiers value is an error"
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
            Action::Launch(LaunchAction {
                path: "x".into(),
                args: LaunchArgs::List(vec!["a b".into(), "c".into()]),
                ..LaunchAction::default()
            })
        );
        assert_eq!(
            parse(json!({ "type": "launch", "path": "x", "args": "--raw \"q\"", "cwd": "C:\\" })),
            Action::Launch(LaunchAction {
                path: "x".into(),
                args: LaunchArgs::Raw("--raw \"q\"".into()),
                cwd: Some("C:\\".into()),
                extra: Extra::new()
            })
        );
        assert_eq!(
            parse(json!({ "type": "launch", "path": "x" })),
            Action::Launch(LaunchAction {
                path: "x".into(),
                ..LaunchAction::default()
            })
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
            json!({ "key": "5", "name": "", "enabled": true, "pass_through": false, "note": "kept",
                    "action": { "type": "open_url", "url": "https://a.b" } })
        );
    }

    #[test]
    fn unknown_action_fields_survive_round_trip() {
        let input = json!({ "key": "a", "name": "", "enabled": true, "pass_through": false, "action":
            { "type": "launch", "path": "x", "args": [], "cwd": null, "elevated": true } });
        let e: BindingEntry = serde_json::from_value(input.clone()).unwrap();
        let BindingEntry::Valid(b) = &e else {
            panic!("expected a valid entry");
        };
        let Action::Launch(l) = &b.action else {
            panic!("expected launch");
        };
        assert_eq!(l.extra["elevated"], json!(true));
        assert_eq!(b.action.type_name(), "launch");
        assert_eq!(serde_json::to_value(&e).unwrap(), input);
        for action in [
            json!({ "type": "send_hotkey", "keys": "win+d", "note": 1 }),
            json!({ "type": "open_url", "url": "https://a.b", "note": [2] }),
        ] {
            let v = json!({ "key": "b", "name": "", "enabled": true, "pass_through": false,
                            "action": action });
            let e: BindingEntry = serde_json::from_value(v.clone()).unwrap();
            assert!(matches!(e, BindingEntry::Valid(_)));
            assert_eq!(serde_json::to_value(&e).unwrap(), v);
        }
    }

    #[test]
    fn url_rule() {
        for ok in [
            "https://a.b",
            "mailto:x@y",
            "ms-settings:display",
            "a+b.c-d:x",
        ] {
            assert!(valid_url(ok), "{ok}");
        }
        for bad in ["", "github.com", "1http://x", "ht tp://x", ":x", "-a:b"] {
            assert!(!valid_url(bad), "{bad}");
        }
        assert!(valid_url(&format!("a:{}", "x".repeat(MAX_URL_LEN - 2))));
        assert!(!valid_url(&format!("a:{}", "x".repeat(MAX_URL_LEN - 1))));
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(Config::from_json(b"{ \"version\": 1,").is_err());
    }

    fn length(v: Value) -> Option<Length> {
        serde_json::from_value(v).ok()
    }

    #[test]
    fn length_parsing() {
        assert_eq!(length(json!("10px")), Some(Length::Px(10.0)));
        assert_eq!(length(json!("-200px")), Some(Length::Px(-200.0)));
        assert_eq!(length(json!("35%")), Some(Length::Pct(35.0)));
        assert_eq!(length(json!(" -12.5% ")), Some(Length::Pct(-12.5)));
        assert_eq!(length(json!(12)), Some(Length::Px(12.0)));
        assert_eq!(length(json!(-3)), Some(Length::Px(-3.0)));
        assert_eq!(length(json!(1.5)), Some(Length::Px(1.5)));
        assert_eq!(length(json!("1.5px")), Some(Length::Px(1.5)));
        for bad in [
            json!("auto"),
            json!("10"),
            json!("px"),
            json!("10 em"),
            json!("infpx"),
            json!("NaN%"),
            json!(""),
            json!(true),
            json!(null),
        ] {
            assert_eq!(length(bad.clone()), None, "{bad}");
        }
        // Written back with its unit; whole numbers without a fraction.
        assert_eq!(
            serde_json::to_value(Length::Px(-200.0)).unwrap(),
            json!("-200px")
        );
        assert_eq!(
            serde_json::to_value(Length::Pct(35.5)).unwrap(),
            json!("35.5%")
        );
    }

    #[test]
    fn dimension_parsing() {
        let dim = |v: Value| serde_json::from_value::<Dimension>(v).ok();
        assert_eq!(dim(json!("auto")), Some(Dimension::Auto));
        assert_eq!(
            dim(json!("320px")),
            Some(Dimension::Length(Length::Px(320.0)))
        );
        assert_eq!(
            dim(json!("10%")),
            Some(Dimension::Length(Length::Pct(10.0)))
        );
        assert_eq!(dim(json!(40)), Some(Dimension::Length(Length::Px(40.0))));
        assert_eq!(dim(json!("big")), None);
        assert_eq!(
            serde_json::to_value(Dimension::Auto).unwrap(),
            json!("auto")
        );
    }

    #[test]
    fn anchor_and_origin_parsing_and_fractions() {
        let origin = |s: &str| serde_json::from_value::<Origin>(json!(s)).unwrap();
        assert_eq!(origin("top_left").fractions(), (0.0, 0.0));
        assert_eq!(origin("top").fractions(), (0.5, 0.0));
        assert_eq!(origin("top_right").fractions(), (1.0, 0.0));
        assert_eq!(origin("left").fractions(), (0.0, 0.5));
        assert_eq!(origin("center").fractions(), (0.5, 0.5));
        assert_eq!(origin("right").fractions(), (1.0, 0.5));
        assert_eq!(origin("bottom_left").fractions(), (0.0, 1.0));
        assert_eq!(origin("bottom").fractions(), (0.5, 1.0));
        assert_eq!(origin("bottom_right").fractions(), (1.0, 1.0));
        assert!(serde_json::from_value::<Origin>(json!("middle")).is_err());
        let anchor = |s: &str| serde_json::from_value::<Anchor>(json!(s)).unwrap();
        assert_eq!(anchor("left").fraction(), 0.0);
        assert_eq!(anchor("center").fraction(), 0.5);
        assert_eq!(anchor("right").fraction(), 1.0);
    }

    #[test]
    fn color_parsing() {
        assert_eq!(
            Color::parse("#FF8000"),
            Some(Color {
                a: 255,
                r: 255,
                g: 128,
                b: 0
            })
        );
        assert_eq!(
            Color::parse("#80ff8000"),
            Some(Color {
                a: 128,
                r: 255,
                g: 128,
                b: 0
            })
        );
        for bad in [
            "FF8000", "#FF800", "#FF80000", "#GG8000", "#+F8000", "", "#",
        ] {
            assert_eq!(Color::parse(bad), None, "{bad}");
        }
        let c = Color::parse("#80ff8000").unwrap();
        assert_eq!(serde_json::to_value(c).unwrap(), json!("#80FF8000"));
        let c = Color::parse("#ff8000").unwrap();
        assert_eq!(serde_json::to_value(c).unwrap(), json!("#FF8000"));
    }

    fn text_item(entry: &WidgetEntry) -> &TextWidget {
        match entry {
            WidgetEntry::Known(WidgetItem::Text(t)) => t,
            other => panic!("expected a text item, got {other:?}"),
        }
    }

    #[test]
    fn architecture_example_parses_and_media_round_trips() {
        let input = json!({ "enabled": false, "hide_on_fullscreen": true, "items": [
            { "id": "media-1", "type": "media", "enabled": true, "anchor": "left", "x": "35%",
              "y": "50%", "origin": "center", "width": "320px" },
            { "id": "text-1", "type": "text", "enabled": true, "text": "Hello", "anchor": "right",
              "x": "-200px", "y": "50%", "origin": "center" }
        ] });
        let w: Widgets = serde_json::from_value(input.clone()).unwrap();
        assert!(matches!(&w.items[0], WidgetEntry::Unknown(v) if *v == input["items"][0]));
        let t = text_item(&w.items[1]);
        assert_eq!(t.common.id, "text-1");
        assert_eq!(t.common.anchor, Lenient::Valid(Anchor::Right));
        assert_eq!(t.common.x, Lenient::Valid(Length::Px(-200.0)));
        assert_eq!(t.common.y, Lenient::Valid(Length::Pct(50.0)));
        assert_eq!(t.common.origin, Lenient::Valid(Origin::Center));
        assert_eq!(t.text, "Hello");
        let back = serde_json::to_value(&w).unwrap();
        assert_eq!(back["items"][0], input["items"][0]);
        assert_eq!(back["items"][1]["type"], json!("text"));
        assert_eq!(back["items"][1]["x"], json!("-200px"));
    }

    #[test]
    fn minimal_text_item_gets_defaults() {
        let e: WidgetEntry =
            serde_json::from_value(json!({ "type": "text", "text": "x" })).unwrap();
        let t = text_item(&e);
        let expected = TextWidget {
            text: "x".into(),
            ..TextWidget::default()
        };
        assert_eq!(*t, expected);
        assert_eq!(t.common.id, "");
        assert!(t.common.enabled);
        assert_eq!(t.common.anchor, Lenient::Valid(Anchor::Left));
        assert_eq!(t.common.x, Lenient::Valid(Length::Px(0.0)));
        assert_eq!(t.common.y, Lenient::Valid(Length::Pct(50.0)));
        assert_eq!(t.common.origin, Lenient::Valid(Origin::Left));
        assert_eq!(t.common.width, Lenient::Valid(Dimension::Auto));
        assert_eq!(t.font_family, "Segoe UI Variable Text");
        assert_eq!(t.font_size, Lenient::Valid(12.0));
        assert_eq!(t.color, Lenient::Valid(None));
        // Written back with every known field once, defaults included.
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({ "type": "text", "id": "", "enabled": true, "anchor": "left", "x": "0px",
                    "y": "50%", "origin": "left", "width": "auto", "height": "auto",
                    "text": "x", "font_family": "Segoe UI Variable Text", "font_size": 12.0,
                    "font_weight": "normal", "color": null, "align": "left" })
        );
    }

    #[test]
    fn invalid_text_fields_are_kept_and_unknown_fields_survive() {
        let input = json!({ "type": "text", "id": "t", "text": "hi", "x": "10 em", "y": "auto",
            "anchor": "middle", "origin": 3, "width": "wide", "height": "20px", "font_size": "big",
            "font_weight": "black", "color": "#12345", "align": "justify", "note": { "k": 1 } });
        let e: WidgetEntry = serde_json::from_value(input.clone()).unwrap();
        let t = text_item(&e);
        assert_eq!(t.common.x, Lenient::Invalid(json!("10 em")));
        assert_eq!(t.common.y, Lenient::Invalid(json!("auto")));
        assert_eq!(t.common.anchor, Lenient::Invalid(json!("middle")));
        assert_eq!(t.common.origin, Lenient::Invalid(json!(3)));
        assert_eq!(t.common.width, Lenient::Invalid(json!("wide")));
        assert_eq!(
            t.common.height,
            Lenient::Valid(Dimension::Length(Length::Px(20.0)))
        );
        assert_eq!(t.font_size, Lenient::Invalid(json!("big")));
        assert_eq!(t.font_weight, Lenient::Invalid(json!("black")));
        assert_eq!(t.color, Lenient::Invalid(json!("#12345")));
        assert_eq!(t.align, Lenient::Invalid(json!("justify")));
        assert_eq!(t.extra.len(), 1);
        assert_eq!(t.extra["note"], json!({ "k": 1 }));
        let mut back = serde_json::to_value(&e).unwrap();
        // Known fields missing from the input are added with their defaults; nothing else changes.
        let obj = back.as_object_mut().unwrap();
        assert_eq!(obj.remove("enabled"), Some(json!(true)));
        assert_eq!(
            obj.remove("font_family"),
            Some(json!("Segoe UI Variable Text"))
        );
        assert_eq!(back, input);
    }

    #[test]
    fn items_that_do_not_parse_are_kept_verbatim() {
        let input = json!([
            { "type": "text", "text": 5 },
            { "text": "no type" },
            { "type": "media" },
            { "type": "text", "enabled": "yes" },
            "not an object"
        ]);
        let items: Vec<WidgetEntry> = serde_json::from_value(input.clone()).unwrap();
        for e in &items {
            assert!(matches!(e, WidgetEntry::Unknown(_)), "{e:?}");
        }
        assert_eq!(serde_json::to_value(&items).unwrap(), input);
    }

    #[test]
    fn color_null_hex_and_alpha() {
        let color = |v: Value| {
            let e: WidgetEntry =
                serde_json::from_value(json!({ "type": "text", "color": v })).unwrap();
            text_item(&e).color.clone()
        };
        assert_eq!(color(json!(null)), Lenient::Valid(None));
        assert_eq!(
            color(json!("#102030")),
            Lenient::Valid(Some(Color {
                a: 255,
                r: 0x10,
                g: 0x20,
                b: 0x30
            }))
        );
        assert_eq!(color(json!("red")), Lenient::Invalid(json!("red")));
    }

    #[test]
    fn bar_with_todo_and_unknown_module_round_trips() {
        let input = json!({ "id": "bar-1", "type": "bar", "anchor": "left", "x": "12px",
            "modules": [
                { "type": "todo", "label": "Tasks", "show_count": false, "open_on_hover": false,
                  "note": 1 },
                { "type": "media", "width": "200px" }
            ] });
        let e: WidgetEntry = serde_json::from_value(input.clone()).unwrap();
        let WidgetEntry::Known(WidgetItem::Bar(b)) = &e else {
            panic!("expected a bar, got {e:?}");
        };
        assert_eq!(b.common.id, "bar-1");
        assert_eq!(b.common.x, Lenient::Valid(Length::Px(12.0)));
        assert_eq!(b.gap, Lenient::Valid(DEFAULT_BAR_GAP));
        assert_eq!(b.font_weight, Lenient::Valid(FontWeight::Semibold));
        let t = b.todo_module().unwrap();
        assert_eq!(t.label, "Tasks");
        assert!(!t.show_count && !t.open_on_hover);
        assert_eq!(t.extra["note"], json!(1));
        assert!(matches!(&b.modules[1], BarModuleEntry::Unknown(v) if *v == input["modules"][1]));
        let back = serde_json::to_value(&e).unwrap();
        assert_eq!(back["type"], json!("bar"));
        assert_eq!(back["modules"], input["modules"]);
    }

    #[test]
    fn todo_module_and_section_defaults() {
        let e: WidgetEntry =
            serde_json::from_value(json!({ "type": "bar", "modules": [{ "type": "todo" }] }))
                .unwrap();
        let WidgetEntry::Known(WidgetItem::Bar(b)) = &e else {
            panic!("expected a bar");
        };
        assert_eq!(b.todo_module(), Some(&TodoModule::default()));
        assert_eq!(b.todo_module().unwrap().label, "TO DO");
        assert!(b.todo_module().unwrap().show_count && b.todo_module().unwrap().open_on_hover);
        let c = Config::from_json(br#"{ "todo": { "enabled": true, "new_items": "sideways" } }"#)
            .unwrap();
        assert!(c.todo.enabled);
        assert_eq!(c.todo.new_items, Lenient::Invalid(json!("sideways")));
        let c = Config::from_json(br#"{ "todo": { "new_items": "bottom" } }"#).unwrap();
        assert_eq!(c.todo.new_items, Lenient::Valid(NewItems::Bottom));
    }
}
