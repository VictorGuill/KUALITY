//! Key recording logic for the Record buttons (portable, no OS types, unit-tested).
//!
//! The platform hook feeds every non-injected key event to [`Recording::on_key`] and swallows
//! the event when told to. See "Key capture" in `docs/specs/settings.md`.

use kuality_protocol::{Key, Mods, Trigger};

/// What is being recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A shortcut hotkey: CapsLock (Hyper) + key, or modifiers + key.
    Trigger,
    /// A `send_hotkey` chord.
    Chord,
}

/// How a recording ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Key name or chord text, ready for the config.
    Recorded(String),
    /// The keys cannot be a hotkey; the message says why (shown, then recording restarts).
    Rejected(&'static str),
    Cancelled,
}

/// A key without a modifier would be swallowed everywhere.
pub const NEEDS_MODIFIER: &str = "Add Ctrl, Alt, Shift or Win, or hold CapsLock (Hyper).";
/// Windows handles these before any hook.
pub const RESERVED: &str = "Windows reserves Ctrl+Alt+Del and Win+L: choose another hotkey.";

const ESC: Key = Key(0x1B);

/// One recording session plus the keys whose up must still be swallowed after it ends.
#[derive(Debug)]
pub struct Recording {
    mode: Mode,
    active: bool,
    /// Chord modifiers in press order (sided keys).
    mods: Vec<Key>,
    /// Trigger mode: CapsLock is down (Hyper).
    caps: bool,
    /// Trigger mode: sided modifiers down (bit = index in `Key::SIDED_MODIFIERS`).
    held: u8,
    /// Keys whose down was swallowed and whose up has not arrived yet.
    swallowed: [bool; 256],
}

impl Recording {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            active: true,
            mods: Vec::new(),
            caps: false,
            held: 0,
            swallowed: [false; 256],
        }
    }

    /// Still waiting for keys.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// No key is waiting for its swallowed up.
    pub fn is_settled(&self) -> bool {
        !self.swallowed.iter().any(|&s| s)
    }

    /// Ends the recording without a result (Esc, click, timeout, focus loss).
    pub fn cancel(&mut self) -> Option<Outcome> {
        self.finish(Outcome::Cancelled)
    }

    fn finish(&mut self, outcome: Outcome) -> Option<Outcome> {
        if !self.active {
            return None;
        }
        self.active = false;
        Some(outcome)
    }

    /// Feeds one key event. Returns whether to swallow it and the outcome if the recording ended.
    /// `foreground` = the settings window is the foreground window.
    pub fn on_key(&mut self, key: Key, down: bool, foreground: bool) -> (bool, Option<Outcome>) {
        let slot = usize::from(key.0 & 0xFF);
        let key = sided(key);
        if self.mode == Mode::Trigger {
            if key == Key::CAPSLOCK {
                self.caps = down;
            } else if let Some(i) = key.sided_modifier_index() {
                if down {
                    self.held |= 1 << i;
                } else {
                    self.held &= !(1 << i);
                }
            }
        }
        if !down {
            let swallow = std::mem::take(&mut self.swallowed[slot]);
            let outcome = if self.active && self.mode == Mode::Chord && self.mods.contains(&key) {
                // A modifier released before any other key: the chord is the modifiers alone.
                let text = self.chord_text(None);
                self.finish(Outcome::Recorded(text))
            } else {
                None
            };
            return (swallow, outcome);
        }
        if !self.active {
            return (false, None);
        }
        if !foreground {
            return (false, self.cancel());
        }
        self.swallowed[slot] = true;
        if key == ESC && self.mods.is_empty() && self.held == 0 && !self.caps {
            return (true, self.cancel());
        }
        if key == Key::CAPSLOCK {
            return (true, None);
        }
        let outcome = match self.mode {
            Mode::Trigger => {
                if key.is_modifier() {
                    None
                } else {
                    let outcome = self.trigger_outcome(key);
                    self.finish(outcome)
                }
            }
            Mode::Chord => {
                if key.is_modifier() {
                    let key = sided(key);
                    if !self.mods.contains(&key)
                        && self.mods.len() < kuality_protocol::Chord::MAX_MODIFIERS
                    {
                        self.mods.push(key);
                    }
                    None
                } else {
                    let text = self.chord_text(Some(key));
                    self.finish(Outcome::Recorded(text))
                }
            }
        };
        (true, outcome)
    }

    /// The hotkey for `key` pressed with what is held now.
    fn trigger_outcome(&self, key: Key) -> Outcome {
        if self.caps {
            return Outcome::Recorded(Trigger::hyper(key).to_config());
        }
        let mut mods = Mods::NONE;
        for (i, &k) in Key::SIDED_MODIFIERS.iter().enumerate() {
            if self.held & (1 << i) != 0 {
                mods |= Mods::of(k);
            }
        }
        if mods.is_empty() {
            return Outcome::Rejected(NEEDS_MODIFIER);
        }
        let trigger = Trigger {
            hyper: false,
            mods,
            key,
        };
        if trigger.is_reserved() {
            Outcome::Rejected(RESERVED)
        } else {
            Outcome::Recorded(trigger.to_config())
        }
    }

    fn chord_text(&self, key: Option<Key>) -> String {
        let mut parts: Vec<String> = self.mods.iter().map(|&m| modifier_name(m).into()).collect();
        if let Some(k) = key {
            parts.push(k.name());
        }
        parts.join("+")
    }
}

/// Generic modifier codes (rare in low-level hooks) are treated as the left key.
fn sided(key: Key) -> Key {
    match key {
        Key::CTRL => Key::LCTRL,
        Key::SHIFT => Key::LSHIFT,
        Key::ALT => Key::LALT,
        k => k,
    }
}

/// Left modifiers use the generic names (sent as the left keys); right ones keep their side.
fn modifier_name(key: Key) -> &'static str {
    match key {
        Key::LCTRL => "ctrl",
        Key::RCTRL => "rctrl",
        Key::LSHIFT => "shift",
        Key::RSHIFT => "rshift",
        Key::LALT => "alt",
        Key::RALT => "ralt",
        Key::RWIN => "rwin",
        _ => "win",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: Key = Key(0x47);
    const D: Key = Key(0x44);

    fn feed(r: &mut Recording, events: &[(Key, bool)]) -> Vec<(bool, Option<Outcome>)> {
        events.iter().map(|&(k, d)| r.on_key(k, d, true)).collect()
    }

    #[test]
    fn trigger_with_capslock_is_hyper_and_ignores_modifiers() {
        let mut r = Recording::new(Mode::Trigger);
        let out = feed(
            &mut r,
            &[(Key::CAPSLOCK, true), (Key::LSHIFT, true), (G, true)],
        );
        assert_eq!(out[0], (true, None));
        assert_eq!(out[1], (true, None));
        assert_eq!(out[2], (true, Some(Outcome::Recorded("hyper+g".into()))));
        assert!(!r.is_active());
        // Ups of swallowed downs are swallowed after the end too; then it is settled.
        assert!(!r.is_settled());
        assert_eq!(r.on_key(G, false, true), (true, None));
        assert_eq!(r.on_key(Key::LSHIFT, false, true), (true, None));
        assert_eq!(r.on_key(Key::CAPSLOCK, false, true), (true, None));
        assert!(r.is_settled());
        // Keys after the end pass through.
        assert_eq!(r.on_key(G, true, true), (false, None));
        assert_eq!(r.on_key(G, false, true), (false, None));
    }

    #[test]
    fn trigger_records_modifier_chords() {
        let mut r = Recording::new(Mode::Trigger);
        let out = feed(
            &mut r,
            &[(Key::RCTRL, true), (Key::LALT, true), (Key(0x4B), true)],
        );
        assert_eq!(out[2].1, Some(Outcome::Recorded("ctrl+alt+k".into())));
        let mut r = Recording::new(Mode::Trigger);
        let out = feed(&mut r, &[(Key::LWIN, true), (Key(0x45), true)]);
        assert_eq!(out[1].1, Some(Outcome::Recorded("win+e".into())));
        // A modifier pressed and released before the key does not count.
        let mut r = Recording::new(Mode::Trigger);
        let out = feed(
            &mut r,
            &[
                (Key::LSHIFT, true),
                (Key::LSHIFT, false),
                (Key::LCTRL, true),
                (G, true),
            ],
        );
        assert_eq!(out[3].1, Some(Outcome::Recorded("ctrl+g".into())));
        // Ctrl+Esc is a hotkey, not a cancel.
        let mut r = Recording::new(Mode::Trigger);
        let out = feed(&mut r, &[(Key::LCTRL, true), (ESC, true)]);
        assert_eq!(out[1].1, Some(Outcome::Recorded("ctrl+esc".into())));
    }

    #[test]
    fn trigger_rejects_lone_keys_and_reserved_combinations() {
        let mut r = Recording::new(Mode::Trigger);
        assert_eq!(
            r.on_key(G, true, true),
            (true, Some(Outcome::Rejected(NEEDS_MODIFIER)))
        );
        let mut r = Recording::new(Mode::Trigger);
        let out = feed(&mut r, &[(Key::LWIN, true), (Key(0x4C), true)]);
        assert_eq!(out[1].1, Some(Outcome::Rejected(RESERVED)));
        let mut r = Recording::new(Mode::Trigger);
        let out = feed(
            &mut r,
            &[(Key::LCTRL, true), (Key::LALT, true), (Key(0x2E), true)],
        );
        assert_eq!(out[2].1, Some(Outcome::Rejected(RESERVED)));
        // Hyper + L is fine.
        let mut r = Recording::new(Mode::Trigger);
        let out = feed(&mut r, &[(Key::CAPSLOCK, true), (Key(0x4C), true)]);
        assert_eq!(out[1].1, Some(Outcome::Recorded("hyper+l".into())));
    }

    #[test]
    fn up_of_a_key_held_before_recording_passes() {
        let mut r = Recording::new(Mode::Trigger);
        assert_eq!(r.on_key(Key::LCTRL, false, true), (false, None));
    }

    #[test]
    fn esc_cancels_and_focus_loss_cancels_without_swallowing() {
        let mut r = Recording::new(Mode::Trigger);
        assert_eq!(r.on_key(ESC, true, true), (true, Some(Outcome::Cancelled)));
        let mut r = Recording::new(Mode::Chord);
        assert_eq!(r.on_key(G, true, false), (false, Some(Outcome::Cancelled)));
        assert!(r.is_settled());
        let mut r = Recording::new(Mode::Chord);
        assert_eq!(r.cancel(), Some(Outcome::Cancelled));
        assert_eq!(r.cancel(), None);
    }

    #[test]
    fn chord_with_modifiers_and_key() {
        let mut r = Recording::new(Mode::Chord);
        let out = feed(
            &mut r,
            &[
                (Key::LCTRL, true),
                (Key::LCTRL, true), // auto-repeat
                (Key::RSHIFT, true),
                (ESC, true),
            ],
        );
        assert_eq!(
            out[3],
            (true, Some(Outcome::Recorded("ctrl+rshift+esc".into())))
        );
    }

    #[test]
    fn chord_of_modifiers_alone_ends_on_first_release() {
        let mut r = Recording::new(Mode::Chord);
        feed(&mut r, &[(Key::LWIN, true)]);
        assert_eq!(
            r.on_key(Key::LWIN, false, true),
            (true, Some(Outcome::Recorded("win".into())))
        );
        assert!(r.is_settled());
    }

    #[test]
    fn chord_ignores_capslock_and_caps_at_four_modifiers() {
        let mut r = Recording::new(Mode::Chord);
        let out = feed(
            &mut r,
            &[
                (Key::CAPSLOCK, true),
                (Key::LCTRL, true),
                (Key::LSHIFT, true),
                (Key::LALT, true),
                (Key::LWIN, true),
                (Key::RCTRL, true),
                (D, true),
            ],
        );
        assert_eq!(
            out[6].1,
            Some(Outcome::Recorded("ctrl+shift+alt+win+d".into()))
        );
        assert!(kuality_protocol::parse_chord("ctrl+shift+alt+win+d").is_ok());
    }
}
