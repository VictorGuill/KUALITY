//! Kuality key codes.
//!
//! [`Key`] values **equal Windows virtual-key codes** (`VK_*`). A future port maps its native
//! key codes to these values. Phase 2 adds the key-name table (see `docs/specs/shortcuts.md`).

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
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({:#04x})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
