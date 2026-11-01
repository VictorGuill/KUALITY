//! Kuality key codes and key names.
//!
//! [`Key`] values **equal Windows virtual-key codes** (`VK_*`). A future port maps its native
//! key codes to these values. Key names and the `send_hotkey` chord grammar are described in
//! `docs/specs/shortcuts.md`.

use std::fmt;

/// A keyboard key. The numeric value is the Windows virtual-key code.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Key(pub u16);

impl Key {
    /// Generic Shift (never reported by low-level hooks; they report the sided keys).
    pub const SHIFT: Key = Key(0x10);
    /// Generic Ctrl.
    pub const CTRL: Key = Key(0x11);
    /// Generic Alt.
    pub const ALT: Key = Key(0x12);
    pub const CAPSLOCK: Key = Key(0x14);
    pub const LWIN: Key = Key(0x5B);
    pub const RWIN: Key = Key(0x5C);
    pub const LSHIFT: Key = Key(0xA0);
    pub const RSHIFT: Key = Key(0xA1);
    pub const LCTRL: Key = Key(0xA2);
    pub const RCTRL: Key = Key(0xA3);
    pub const LALT: Key = Key(0xA4);
    pub const RALT: Key = Key(0xA5);
    /// Unassigned virtual-key code, used as the neutral "mask" key before releasing Win/Alt.
    pub const MASK: Key = Key(0xE8);

    /// The eight sided modifiers (L/R Ctrl, Shift, Alt, Win).
    pub const SIDED_MODIFIERS: [Key; 8] = [
        Key::LCTRL,
        Key::RCTRL,
        Key::LSHIFT,
        Key::RSHIFT,
        Key::LALT,
        Key::RALT,
        Key::LWIN,
        Key::RWIN,
    ];

    /// Position of this key in [`Key::SIDED_MODIFIERS`], if it is one of them.
    pub fn sided_modifier_index(self) -> Option<usize> {
        Self::SIDED_MODIFIERS.iter().position(|&k| k == self)
    }

    /// Ctrl, Shift, Alt or Win (sided or generic).
    pub fn is_modifier(self) -> bool {
        matches!(self, Key::SHIFT | Key::CTRL | Key::ALT) || self.sided_modifier_index().is_some()
    }

    /// Can be a shortcut trigger: not a modifier and not CapsLock.
    pub fn is_trigger(self) -> bool {
        self != Key::CAPSLOCK && !self.is_modifier()
    }

    /// Canonical name, accepted by [`parse_key`]. Codes without a name are written as
    /// `vk:0xNN`.
    pub fn name(self) -> String {
        let c = self.0;
        match c {
            0x30..=0x39 | 0x41..=0x5A => char::from(c as u8).to_ascii_lowercase().to_string(),
            0x60..=0x69 => format!("num{}", c - 0x60),
            0x70..=0x87 => format!("f{}", c - 0x70 + 1),
            _ => NAMES
                .iter()
                .find(|&&(_, code)| code == c)
                .map(|&(name, _)| name.to_string())
                .unwrap_or_else(|| format!("vk:0x{c:02X}")),
        }
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({:#04x})", self.0)
    }
}

/// Named keys besides letters, digits, `f1`…`f24` and `num0`…`num9`. For codes with several
/// names, the first entry is the canonical one.
const NAMES: &[(&str, u16)] = &[
    ("esc", 0x1B),
    ("escape", 0x1B),
    ("tab", 0x09),
    ("space", 0x20),
    ("enter", 0x0D),
    ("return", 0x0D),
    ("backspace", 0x08),
    ("insert", 0x2D),
    ("ins", 0x2D),
    ("delete", 0x2E),
    ("del", 0x2E),
    ("home", 0x24),
    ("end", 0x23),
    ("pageup", 0x21),
    ("pgup", 0x21),
    ("pagedown", 0x22),
    ("pgdn", 0x22),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
    ("printscreen", 0x2C),
    ("prtsc", 0x2C),
    ("pause", 0x13),
    ("scrolllock", 0x91),
    ("numlock", 0x90),
    ("capslock", 0x14),
    ("apps", 0x5D),
    ("menu", 0x5D),
    ("num_multiply", 0x6A),
    ("num_add", 0x6B),
    ("num_subtract", 0x6D),
    ("num_decimal", 0x6E),
    ("num_divide", 0x6F),
    ("oem_1", 0xBA),
    (";", 0xBA),
    ("oem_plus", 0xBB),
    ("=", 0xBB),
    ("oem_comma", 0xBC),
    (",", 0xBC),
    ("oem_minus", 0xBD),
    ("-", 0xBD),
    ("oem_period", 0xBE),
    (".", 0xBE),
    ("oem_2", 0xBF),
    ("/", 0xBF),
    ("oem_3", 0xC0),
    ("`", 0xC0),
    ("oem_4", 0xDB),
    ("[", 0xDB),
    ("oem_5", 0xDC),
    ("\\", 0xDC),
    ("oem_6", 0xDD),
    ("]", 0xDD),
    ("oem_7", 0xDE),
    ("'", 0xDE),
    ("oem_8", 0xDF),
    ("oem_102", 0xE2),
    ("volume_mute", 0xAD),
    ("volume_down", 0xAE),
    ("volume_up", 0xAF),
    ("media_next", 0xB0),
    ("media_prev", 0xB1),
    ("media_stop", 0xB2),
    ("media_play_pause", 0xB3),
    // Generic modifier names are sent as the left keys.
    ("ctrl", 0xA2),
    ("control", 0xA2),
    ("shift", 0xA0),
    ("alt", 0xA4),
    ("win", 0x5B),
    ("super", 0x5B),
    ("meta", 0x5B),
    ("lctrl", 0xA2),
    ("rctrl", 0xA3),
    ("lshift", 0xA0),
    ("rshift", 0xA1),
    ("lalt", 0xA4),
    ("ralt", 0xA5),
    ("lwin", 0x5B),
    ("rwin", 0x5C),
];

/// Why a key name or chord was rejected. Messages never repeat the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyParseError {
    /// Empty name (or an empty token in a chord, e.g. a trailing `+`).
    Empty,
    /// Not a known key name.
    Unknown,
    /// `vk:` code that is not hex or not in `0x01..=0xFE`.
    BadVkCode,
    /// A chord token other than the last is not a modifier.
    NotAModifier,
    /// A chord names the same key twice.
    Duplicate,
    /// A chord has more than [`Chord::MAX_MODIFIERS`] modifiers.
    TooManyModifiers,
}

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "empty key name",
            Self::Unknown => "unknown key name",
            Self::BadVkCode => "vk: code must be hex in 0x01..0xFE",
            Self::NotAModifier => "only the last key of a chord may be a non-modifier",
            Self::Duplicate => "the same key appears twice in the chord",
            Self::TooManyModifiers => "a chord has at most 4 modifiers",
        })
    }
}

impl std::error::Error for KeyParseError {}

/// Parses a key name (case-insensitive, surrounding whitespace ignored).
pub fn parse_key(s: &str) -> Result<Key, KeyParseError> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err(KeyParseError::Empty);
    }
    if let Some(hex) = s.strip_prefix("vk:") {
        let hex = hex.trim();
        let hex = hex.strip_prefix("0x").unwrap_or(hex);
        return match u32::from_str_radix(hex, 16) {
            Ok(code @ 0x01..=0xFE) if !hex.starts_with('+') => Ok(Key(code as u16)),
            _ => Err(KeyParseError::BadVkCode),
        };
    }
    let bytes = s.as_bytes();
    if bytes.len() == 1 && bytes[0].is_ascii_alphanumeric() {
        return Ok(Key(u16::from(bytes[0].to_ascii_uppercase())));
    }
    if let Some(n) = numbered(&s, "f", 1, 24) {
        return Ok(Key(0x70 + n - 1));
    }
    if let Some(n) = numbered(&s, "num", 0, 9) {
        return Ok(Key(0x60 + n));
    }
    NAMES
        .iter()
        .find(|&&(name, _)| name == s)
        .map(|&(_, code)| Key(code))
        .ok_or(KeyParseError::Unknown)
}

/// `prefix` followed by a plain decimal number in `min..=max` (no sign, no leading zero).
fn numbered(s: &str, prefix: &str, min: u16, max: u16) -> Option<u16> {
    let digits = s.strip_prefix(prefix)?;
    if digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return None;
    }
    let n: u16 = digits.parse().ok()?;
    (min..=max).contains(&n).then_some(n)
}

/// A `send_hotkey` chord: up to four modifiers, then at most one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    mods: [Key; Chord::MAX_MODIFIERS],
    mods_len: u8,
    /// The non-modifier key, or `None` if the chord is only modifiers (e.g. `"win"`).
    pub key: Option<Key>,
}

impl Chord {
    pub const MAX_MODIFIERS: usize = 4;

    /// Modifiers in press order.
    pub fn mods(&self) -> &[Key] {
        &self.mods[..usize::from(self.mods_len)]
    }
}

/// Parses a chord such as `"ctrl+shift+esc"`, `"win+d"`, `"volume_mute"` or `"win"`.
///
/// Tokens are separated by `+` (the `+`/`=` key is `oem_plus`). All tokens except the last
/// must be modifiers; if every token is a modifier, they are all pressed and released.
pub fn parse_chord(s: &str) -> Result<Chord, KeyParseError> {
    let mut keys = [Key(0); Chord::MAX_MODIFIERS + 1];
    let mut n = 0;
    for token in s.split('+') {
        let key = parse_key(token)?;
        if keys[..n].contains(&key) {
            return Err(KeyParseError::Duplicate);
        }
        if n > 0 && !keys[n - 1].is_modifier() {
            return Err(KeyParseError::NotAModifier);
        }
        if n == keys.len() {
            return Err(KeyParseError::TooManyModifiers);
        }
        keys[n] = key;
        n += 1;
    }
    let (last, mods) = (keys[n - 1], &keys[..n]);
    let (mods, key) = if last.is_modifier() {
        (mods, None)
    } else {
        (&mods[..n - 1], Some(last))
    };
    if mods.len() > Chord::MAX_MODIFIERS {
        return Err(KeyParseError::TooManyModifiers);
    }
    let mut chord = Chord {
        mods: [Key(0); Chord::MAX_MODIFIERS],
        mods_len: mods.len() as u8,
        key,
    };
    chord.mods[..mods.len()].copy_from_slice(mods);
    Ok(chord)
}

/// Side-agnostic modifier set of a shortcut [`Trigger`] (`ctrl` matches Left or Right Ctrl).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const SHIFT: Mods = Mods(2);
    pub const ALT: Mods = Mods(4);
    pub const WIN: Mods = Mods(8);
    /// In the order they are written and shown.
    pub const ALL: [Mods; 4] = [Mods::CTRL, Mods::SHIFT, Mods::ALT, Mods::WIN];

    /// The generic modifier of a (sided or generic) modifier key; `NONE` for other keys.
    pub fn of(key: Key) -> Mods {
        match key {
            Key::CTRL | Key::LCTRL | Key::RCTRL => Mods::CTRL,
            Key::SHIFT | Key::LSHIFT | Key::RSHIFT => Mods::SHIFT,
            Key::ALT | Key::LALT | Key::RALT => Mods::ALT,
            Key::LWIN | Key::RWIN => Mods::WIN,
            _ => Mods::NONE,
        }
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Config name (`ctrl`, `shift`, `alt`, `win`) of a single modifier.
    fn name(self) -> &'static str {
        match self {
            Mods::CTRL => "ctrl",
            Mods::SHIFT => "shift",
            Mods::ALT => "alt",
            _ => "win",
        }
    }
}

impl std::ops::BitOr for Mods {
    type Output = Mods;
    fn bitor(self, rhs: Mods) -> Mods {
        Mods(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Mods) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for Mods {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = Mods::ALL
            .iter()
            .filter(|&&m| self.contains(m))
            .map(|m| m.name())
            .collect();
        write!(f, "Mods({})", names.join("+"))
    }
}

/// What starts a shortcut: Hyper + key, or modifiers + key (see `docs/specs/shortcuts.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Trigger {
    /// Hyper (CapsLock) + `key`; `mods` is then empty.
    pub hyper: bool,
    pub mods: Mods,
    /// Never a modifier or CapsLock.
    pub key: Key,
}

/// Why a trigger was rejected. Messages never repeat the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerError {
    Key(KeyParseError),
    /// A token other than the last is not a modifier or `hyper`.
    NotAModifier,
    /// The same modifier twice (`ctrl+rctrl` counts: sides are ignored).
    Duplicate,
    /// The last token is a modifier.
    ModifierKey,
    /// CapsLock is the Hyper key itself.
    CapsLock,
    /// `hyper` plus another modifier (physical modifiers are ignored while Hyper is held).
    HyperWithModifiers,
    /// Windows keeps this combination (Ctrl+Alt+Del, Win+L).
    Reserved,
}

impl fmt::Display for TriggerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(e) => write!(f, "{e}"),
            Self::NotAModifier => f.write_str("only the last key may be a non-modifier"),
            Self::Duplicate => f.write_str("the same modifier appears twice"),
            Self::ModifierKey => f.write_str("a modifier cannot be the shortcut key"),
            Self::CapsLock => f.write_str("capslock is the Hyper key itself"),
            Self::HyperWithModifiers => {
                f.write_str("hyper cannot be combined with other modifiers")
            }
            Self::Reserved => f.write_str("Windows reserves this combination"),
        }
    }
}

impl std::error::Error for TriggerError {}

impl Trigger {
    pub fn hyper(key: Key) -> Self {
        Self {
            hyper: true,
            mods: Mods::NONE,
            key,
        }
    }

    /// Ctrl+Alt+Del (with any other modifiers) and Win+L: Windows handles them before any hook.
    pub fn is_reserved(&self) -> bool {
        const DELETE: Key = Key(0x2E);
        const L: Key = Key(0x4C);
        !self.hyper
            && ((self.mods.contains(Mods::CTRL | Mods::ALT) && self.key == DELETE)
                || (self.mods == Mods::WIN && self.key == L))
    }

    /// Canonical config text: `hyper+1`, `ctrl+alt+k` (modifiers in the order ctrl, shift,
    /// alt, win).
    pub fn to_config(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.hyper {
            parts.push("hyper".into());
        }
        for m in Mods::ALL {
            if self.mods.contains(m) {
                parts.push(m.name().into());
            }
        }
        parts.push(self.key.name());
        parts.join("+")
    }
}

/// Parses a shortcut trigger: `"hyper+1"`, `"ctrl+t"`, `"win+e"`, `"ctrl+alt+k"`. A bare key
/// (`"1"`) means Hyper + that key (the Phase 2 format). Case-insensitive, spaces around tokens
/// ignored, modifier sides ignored (`rctrl` = `ctrl`).
pub fn parse_trigger(s: &str) -> Result<Trigger, TriggerError> {
    let tokens: Vec<&str> = s.split('+').collect();
    let (last, mods_tokens) = tokens
        .split_last()
        .expect("split yields at least one token");
    let mut hyper = false;
    let mut mods = Mods::NONE;
    for token in mods_tokens {
        if token.trim().eq_ignore_ascii_case("hyper") {
            if hyper {
                return Err(TriggerError::Duplicate);
            }
            hyper = true;
            continue;
        }
        let key = parse_key(token).map_err(TriggerError::Key)?;
        let m = Mods::of(key);
        if m.is_empty() {
            return Err(TriggerError::NotAModifier);
        }
        if mods.contains(m) {
            return Err(TriggerError::Duplicate);
        }
        mods |= m;
    }
    let key = parse_key(last).map_err(TriggerError::Key)?;
    if key == Key::CAPSLOCK {
        return Err(TriggerError::CapsLock);
    }
    if key.is_modifier() {
        return Err(TriggerError::ModifierKey);
    }
    if hyper && !mods.is_empty() {
        return Err(TriggerError::HyperWithModifiers);
    }
    let trigger = Trigger {
        hyper: hyper || mods.is_empty(),
        mods,
        key,
    };
    if trigger.is_reserved() {
        return Err(TriggerError::Reserved);
    }
    Ok(trigger)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trig(s: &str) -> Trigger {
        parse_trigger(s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
    }

    #[test]
    fn bare_key_and_hyper_prefix_are_hyper() {
        assert_eq!(trig("1"), Trigger::hyper(Key(0x31)));
        assert_eq!(trig(" G "), Trigger::hyper(Key(0x47)));
        assert_eq!(trig("hyper+1"), Trigger::hyper(Key(0x31)));
        assert_eq!(trig(" HYPER + pageup"), Trigger::hyper(Key(0x21)));
        assert_eq!(trig("1").to_config(), "hyper+1");
    }

    #[test]
    fn normal_chords_ignore_sides_and_order() {
        let t = trig("ctrl+t");
        assert!(!t.hyper);
        assert_eq!(t.mods, Mods::CTRL);
        assert_eq!(t.key, Key(0x54));
        assert_eq!(trig("win+e").mods, Mods::WIN);
        assert_eq!(trig("alt+ctrl+k"), trig("ctrl+alt+k"));
        assert_eq!(trig("rctrl+ralt+k"), trig("ctrl+alt+k"));
        assert_eq!(
            trig("Ctrl + Shift + Alt + Win + F13").to_config(),
            "ctrl+shift+alt+win+f13"
        );
        assert_eq!(trig("alt+ctrl+k").to_config(), "ctrl+alt+k");
        assert_eq!(trig("shift+oem_plus").to_config(), "shift+oem_plus");
    }

    #[test]
    fn bad_triggers() {
        use TriggerError::*;
        assert_eq!(parse_trigger("ctrl"), Err(ModifierKey));
        assert_eq!(parse_trigger("win"), Err(ModifierKey));
        assert_eq!(parse_trigger("capslock"), Err(CapsLock));
        assert_eq!(parse_trigger("ctrl+capslock"), Err(CapsLock));
        assert_eq!(parse_trigger("a+b"), Err(NotAModifier));
        assert_eq!(parse_trigger("ctrl+rctrl+k"), Err(Duplicate));
        assert_eq!(parse_trigger("hyper+hyper+k"), Err(Duplicate));
        assert_eq!(parse_trigger("hyper+ctrl+k"), Err(HyperWithModifiers));
        assert_eq!(parse_trigger("ctrl+hyper+k"), Err(HyperWithModifiers));
        assert_eq!(parse_trigger("hyper"), Err(Key(KeyParseError::Unknown)));
        assert_eq!(parse_trigger(""), Err(Key(KeyParseError::Empty)));
        assert_eq!(parse_trigger("ctrl+"), Err(Key(KeyParseError::Empty)));
        assert_eq!(parse_trigger("+k"), Err(Key(KeyParseError::Empty)));
    }

    #[test]
    fn reserved_combinations() {
        use TriggerError::Reserved;
        assert_eq!(parse_trigger("ctrl+alt+del"), Err(Reserved));
        assert_eq!(parse_trigger("ctrl+alt+shift+delete"), Err(Reserved));
        assert_eq!(parse_trigger("win+l"), Err(Reserved));
        // Not reserved: other modifiers or Hyper.
        assert!(parse_trigger("win+shift+l").is_ok());
        assert!(parse_trigger("ctrl+del").is_ok());
        assert!(parse_trigger("hyper+l").is_ok());
        assert!(parse_trigger("l").is_ok());
    }

    #[test]
    fn mods_of_keys() {
        assert_eq!(Mods::of(Key::RCTRL), Mods::CTRL);
        assert_eq!(Mods::of(Key::LSHIFT), Mods::SHIFT);
        assert_eq!(Mods::of(Key::RALT), Mods::ALT);
        assert_eq!(Mods::of(Key::RWIN), Mods::WIN);
        assert_eq!(Mods::of(Key::CAPSLOCK), Mods::NONE);
        assert_eq!(format!("{:?}", Mods::CTRL | Mods::WIN), "Mods(ctrl+win)");
    }

    #[test]
    fn trigger_keys_exclude_modifiers_and_capslock() {
        for k in ["a", "1", "f5", "space", "esc", "vk:0xE2"] {
            assert!(parse_key(k).unwrap().is_trigger(), "{k}");
        }
        for k in ["capslock", "ctrl", "rshift", "alt", "win", "rwin"] {
            assert!(!parse_key(k).unwrap().is_trigger(), "{k}");
        }
    }

    #[test]
    fn values_match_windows_vk_codes() {
        assert_eq!(Key::CAPSLOCK.0, 0x14);
        assert_eq!(Key::LCTRL.0, 0xA2);
        assert_eq!(Key::LWIN.0, 0x5B);
        assert_eq!(Key::MASK.0, 0xE8);
    }

    #[test]
    fn sided_modifier_index() {
        assert_eq!(Key::LCTRL.sided_modifier_index(), Some(0));
        assert_eq!(Key::RWIN.sided_modifier_index(), Some(7));
        assert_eq!(Key::CTRL.sided_modifier_index(), None);
        assert_eq!(Key::CAPSLOCK.sided_modifier_index(), None);
    }

    fn k(s: &str) -> u16 {
        parse_key(s).unwrap_or_else(|e| panic!("{s:?}: {e}")).0
    }

    #[test]
    fn letters_digits_function_and_numpad_keys() {
        for (i, c) in ('a'..='z').enumerate() {
            assert_eq!(k(&c.to_string()), 0x41 + i as u16);
            assert_eq!(k(&c.to_ascii_uppercase().to_string()), 0x41 + i as u16);
        }
        for d in 0..=9u16 {
            assert_eq!(k(&d.to_string()), 0x30 + d);
            assert_eq!(k(&format!("num{d}")), 0x60 + d);
        }
        for n in 1..=24u16 {
            assert_eq!(k(&format!("f{n}")), 0x70 + n - 1);
            assert_eq!(k(&format!("F{n}")), 0x70 + n - 1);
        }
        for bad in ["f0", "f25", "f01", "num10", "num", "f+1"] {
            assert_eq!(parse_key(bad), Err(KeyParseError::Unknown), "{bad}");
        }
    }

    #[test]
    fn named_keys_match_the_spec_table() {
        let table: &[(&[&str], u16)] = &[
            (&["esc", "escape"], 0x1B),
            (&["tab"], 0x09),
            (&["space"], 0x20),
            (&["enter", "return"], 0x0D),
            (&["backspace"], 0x08),
            (&["insert", "ins"], 0x2D),
            (&["delete", "del"], 0x2E),
            (&["home"], 0x24),
            (&["end"], 0x23),
            (&["pageup", "pgup"], 0x21),
            (&["pagedown", "pgdn"], 0x22),
            (&["left"], 0x25),
            (&["up"], 0x26),
            (&["right"], 0x27),
            (&["down"], 0x28),
            (&["printscreen", "prtsc"], 0x2C),
            (&["pause"], 0x13),
            (&["scrolllock"], 0x91),
            (&["numlock"], 0x90),
            (&["capslock"], 0x14),
            (&["apps", "menu"], 0x5D),
            (&["num_multiply"], 0x6A),
            (&["num_add"], 0x6B),
            (&["num_subtract"], 0x6D),
            (&["num_decimal"], 0x6E),
            (&["num_divide"], 0x6F),
            (&["oem_1", ";"], 0xBA),
            (&["oem_plus", "="], 0xBB),
            (&["oem_comma", ","], 0xBC),
            (&["oem_minus", "-"], 0xBD),
            (&["oem_period", "."], 0xBE),
            (&["oem_2", "/"], 0xBF),
            (&["oem_3", "`"], 0xC0),
            (&["oem_4", "["], 0xDB),
            (&["oem_5", "\\"], 0xDC),
            (&["oem_6", "]"], 0xDD),
            (&["oem_7", "'"], 0xDE),
            (&["oem_8"], 0xDF),
            (&["oem_102"], 0xE2),
            (&["volume_mute"], 0xAD),
            (&["volume_down"], 0xAE),
            (&["volume_up"], 0xAF),
            (&["media_next"], 0xB0),
            (&["media_prev"], 0xB1),
            (&["media_stop"], 0xB2),
            (&["media_play_pause"], 0xB3),
            (&["ctrl", "control", "lctrl"], 0xA2),
            (&["shift", "lshift"], 0xA0),
            (&["alt", "lalt"], 0xA4),
            (&["win", "super", "meta", "lwin"], 0x5B),
            (&["rctrl"], 0xA3),
            (&["rshift"], 0xA1),
            (&["ralt"], 0xA5),
            (&["rwin"], 0x5C),
        ];
        for &(names, code) in table {
            for name in names {
                assert_eq!(k(name), code, "{name}");
                assert_eq!(k(&format!("  {}\t", name.to_ascii_uppercase())), code);
            }
            assert_eq!(Key(code).name(), names[0], "canonical name of {code:#x}");
        }
    }

    #[test]
    fn vk_forms() {
        assert_eq!(k("vk:0xE2"), 0xE2);
        assert_eq!(k("vk:E2"), 0xE2);
        assert_eq!(k("VK:0Xe2"), 0xE2);
        assert_eq!(k("vk:1"), 0x01);
        assert_eq!(k("vk:0xFE"), 0xFE);
        for bad in [
            "vk:0x00", "vk:0", "vk:0x1FF", "vk:FF", "vk:", "vk:0x", "vk:zz", "vk:+1",
        ] {
            assert_eq!(parse_key(bad), Err(KeyParseError::BadVkCode), "{bad}");
        }
    }

    #[test]
    fn rejects_empty_and_unknown() {
        assert_eq!(parse_key(""), Err(KeyParseError::Empty));
        assert_eq!(parse_key("   "), Err(KeyParseError::Empty));
        for bad in ["hyper", "ñ", "+", "ctrl+a", "a b", "€"] {
            assert_eq!(parse_key(bad), Err(KeyParseError::Unknown), "{bad}");
        }
    }

    #[test]
    fn name_round_trips_for_every_code() {
        for code in 0x01..=0xFEu16 {
            let name = Key(code).name();
            assert_eq!(parse_key(&name), Ok(Key(code)), "{name}");
        }
        assert_eq!(Key(0x07).name(), "vk:0x07");
        assert_eq!(Key(0xE8).name(), "vk:0xE8");
    }

    #[test]
    fn modifiers() {
        for m in Key::SIDED_MODIFIERS {
            assert!(m.is_modifier());
        }
        assert!(Key::CTRL.is_modifier() && Key::SHIFT.is_modifier() && Key::ALT.is_modifier());
        assert!(!Key::CAPSLOCK.is_modifier());
        assert!(!Key(0x41).is_modifier());
    }

    fn chord(s: &str) -> (Vec<u16>, Option<u16>) {
        let c = parse_chord(s).unwrap_or_else(|e| panic!("{s:?}: {e}"));
        (c.mods().iter().map(|k| k.0).collect(), c.key.map(|k| k.0))
    }

    #[test]
    fn chords() {
        assert_eq!(chord("ctrl+shift+esc"), (vec![0xA2, 0xA0], Some(0x1B)));
        assert_eq!(chord("win"), (vec![0x5B], None));
        assert_eq!(chord("win+d"), (vec![0x5B], Some(0x44)));
        assert_eq!(chord("ctrl+oem_plus"), (vec![0xA2], Some(0xBB)));
        assert_eq!(chord(" Ctrl + Alt + Del "), (vec![0xA2, 0xA4], Some(0x2E)));
        assert_eq!(chord("volume_mute"), (vec![], Some(0xAD)));
        assert_eq!(chord("ctrl+-"), (vec![0xA2], Some(0xBD)));
        assert_eq!(chord("ctrl+shift"), (vec![0xA2, 0xA0], None));
        assert_eq!(
            chord("lctrl+rctrl+shift+alt+k"),
            (vec![0xA2, 0xA3, 0xA0, 0xA4], Some(0x4B))
        );
        assert_eq!(
            chord("lctrl+rctrl+lshift+rshift"),
            (vec![0xA2, 0xA3, 0xA0, 0xA1], None)
        );
    }

    #[test]
    fn bad_chords() {
        use KeyParseError::*;
        assert_eq!(parse_chord("a+b"), Err(NotAModifier));
        assert_eq!(parse_chord("ctrl+ctrl+a"), Err(Duplicate));
        assert_eq!(parse_chord("ctrl+lctrl"), Err(Duplicate));
        assert_eq!(
            parse_chord("lctrl+rctrl+shift+alt+win+k"),
            Err(TooManyModifiers)
        );
        assert_eq!(
            parse_chord("lctrl+rctrl+shift+alt+win"),
            Err(TooManyModifiers)
        );
        assert_eq!(parse_chord("ctrl+"), Err(Empty));
        assert_eq!(parse_chord("ctrl++"), Err(Empty));
        assert_eq!(parse_chord("+a"), Err(Empty));
        assert_eq!(parse_chord(""), Err(Empty));
        assert_eq!(parse_chord("ctrl+hyper"), Err(Unknown));
    }
}
