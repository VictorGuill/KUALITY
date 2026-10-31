//! Portable Hyperkey state machine (no OS types), see `docs/specs/hyperkey.md`.
//!
//! The engine is called from the low-level keyboard hook callback, so nothing here allocates,
//! locks, logs or panics on any input.

use kuality_protocol::Key;

/// A key event that did **not** come from Kuality itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub down: bool,
}

/// A key event to inject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Synth {
    Down(Key),
    Up(Key),
}

/// Capacity of a [`SynthBuf`].
pub const SYNTH_CAPACITY: usize = 32;

/// Fixed-size list of events to inject (no heap allocation).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SynthBuf {
    items: [Synth; SYNTH_CAPACITY],
    len: usize,
}

impl SynthBuf {
    pub const fn new() -> Self {
        Self {
            items: [Synth::Up(Key(0)); SYNTH_CAPACITY],
            len: 0,
        }
    }

    /// Appends an event. Events past the capacity are dropped (never happens with the fixed
    /// sequences the engine produces).
    pub fn push(&mut self, s: Synth) {
        if self.len < SYNTH_CAPACITY {
            self.items[self.len] = s;
            self.len += 1;
        }
    }

    pub fn as_slice(&self) -> &[Synth] {
        &self.items[..self.len]
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

impl Default for SynthBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SynthBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
    }
}

/// Hyper state change caused by an event (for debug logging; never carries key codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    HyperDown,
    HyperUp,
}

/// What the hook must do with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Verdict {
    /// Do not pass the event on to the system.
    pub swallow: bool,
    /// Events to inject (in one `SendInput` call).
    pub synth: SynthBuf,
    /// Binding index to run (Phase 2).
    pub action: Option<u16>,
    pub transition: Option<Transition>,
}

impl Verdict {
    fn pass() -> Self {
        Self::default()
    }

    fn swallow() -> Self {
        Self {
            swallow: true,
            ..Self::default()
        }
    }
}

/// Bitset over [`Key::SIDED_MODIFIERS`] (bit `i` = `SIDED_MODIFIERS[i]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModSet(u8);

impl ModSet {
    pub const EMPTY: ModSet = ModSet(0);

    pub fn contains(self, key: Key) -> bool {
        key.sided_modifier_index()
            .is_some_and(|i| self.0 & (1 << i) != 0)
    }

    pub fn set(&mut self, key: Key, on: bool) {
        if let Some(i) = key.sided_modifier_index() {
            if on {
                self.0 |= 1 << i;
            } else {
                self.0 &= !(1 << i);
            }
        }
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Modifiers that make up Hyper, in press order.
pub const HYPER_PRESS_ORDER: [Key; 4] = [Key::LCTRL, Key::LSHIFT, Key::LALT, Key::LWIN];
/// Same modifiers, in release order.
pub const HYPER_RELEASE_ORDER: [Key; 4] = [Key::LWIN, Key::LALT, Key::LSHIFT, Key::LCTRL];

/// The Hyperkey state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Engine {
    /// Hyper is active (CapsLock physically down).
    held: bool,
    /// Hyper modifiers we have injected down.
    injected: ModSet,
    /// Physical state of the eight sided modifiers.
    physical: ModSet,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub const fn new() -> Self {
        Self {
            held: false,
            injected: ModSet(0),
            physical: ModSet(0),
        }
    }

    /// Processes one event. `caps_toggled_on` is the current CapsLock toggle state (only read
    /// on a CapsLock press that starts Hyper).
    pub fn on_event(&mut self, ev: KeyEvent, caps_toggled_on: bool) -> Verdict {
        // Rule 1: track the physical state of the sided modifiers.
        let is_modifier = ev.key.sided_modifier_index().is_some();
        if is_modifier {
            self.physical.set(ev.key, ev.down);
        }

        if ev.key == Key::CAPSLOCK {
            return match (ev.down, self.held) {
                // Rule 2: Hyper starts.
                (true, false) => {
                    self.held = true;
                    let mut v = Verdict::swallow();
                    if caps_toggled_on {
                        v.synth.push(Synth::Down(Key::CAPSLOCK));
                        v.synth.push(Synth::Up(Key::CAPSLOCK));
                    }
                    for k in HYPER_PRESS_ORDER {
                        v.synth.push(Synth::Down(k));
                        self.injected.set(k, true);
                    }
                    v.transition = Some(Transition::HyperDown);
                    v
                }
                // Rule 3: auto-repeat.
                (true, true) => Verdict::swallow(),
                // Rule 4: Hyper ends.
                (false, true) => {
                    let mut v = Verdict::swallow();
                    v.synth = self.release();
                    v.transition = Some(Transition::HyperUp);
                    v
                }
                // Rule 5: up without a down we saw.
                (false, false) => Verdict::swallow(),
            };
        }

        // Rule 6: physical modifiers add nothing while Hyper is held.
        if self.held && is_modifier {
            return Verdict::swallow();
        }

        // Rule 7: everything else passes (Phase 2 adds the binding check here).
        Verdict::pass()
    }

    /// Releases Hyper if it is held (or modifiers are still injected); empty otherwise.
    pub fn force_release(&mut self) -> SynthBuf {
        if self.held || !self.injected.is_empty() {
            self.release()
        } else {
            SynthBuf::new()
        }
    }

    /// Forgets all state without injecting anything (after unlock/resume).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Whether Kuality currently holds injected modifiers down.
    pub fn modifiers_down(&self) -> bool {
        !self.injected.is_empty()
    }

    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Mask key, then `Up` for each injected modifier not physically held.
    fn release(&mut self) -> SynthBuf {
        let mut out = SynthBuf::new();
        out.push(Synth::Down(Key::MASK));
        out.push(Synth::Up(Key::MASK));
        for k in HYPER_RELEASE_ORDER {
            if self.injected.contains(k) && !self.physical.contains(k) {
                out.push(Synth::Up(k));
            }
        }
        self.injected = ModSet::EMPTY;
        self.held = false;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Synth::{Down, Up};

    fn down(key: Key) -> KeyEvent {
        KeyEvent { key, down: true }
    }

    fn up(key: Key) -> KeyEvent {
        KeyEvent { key, down: false }
    }

    const HYPER_DOWN: [Synth; 4] = [
        Down(Key::LCTRL),
        Down(Key::LSHIFT),
        Down(Key::LALT),
        Down(Key::LWIN),
    ];
    const HYPER_UP: [Synth; 6] = [
        Down(Key::MASK),
        Up(Key::MASK),
        Up(Key::LWIN),
        Up(Key::LALT),
        Up(Key::LSHIFT),
        Up(Key::LCTRL),
    ];

    #[test]
    fn caps_press_repeat_release() {
        let mut e = Engine::new();
        let v = e.on_event(down(Key::CAPSLOCK), false);
        assert!(v.swallow);
        assert_eq!(v.synth.as_slice(), HYPER_DOWN);
        assert_eq!(v.transition, Some(Transition::HyperDown));
        assert!(e.is_held() && e.modifiers_down());

        for _ in 0..5 {
            let v = e.on_event(down(Key::CAPSLOCK), false);
            assert!(v.swallow);
            assert!(v.synth.is_empty(), "auto-repeat must not inject");
            assert_eq!(v.transition, None);
        }

        let v = e.on_event(up(Key::CAPSLOCK), false);
        assert!(v.swallow);
        assert_eq!(v.synth.as_slice(), HYPER_UP);
        assert_eq!(v.transition, Some(Transition::HyperUp));
        assert!(!e.is_held() && !e.modifiers_down());
    }

    #[test]
    fn caps_toggled_on_is_turned_off_first() {
        let mut e = Engine::new();
        let v = e.on_event(down(Key::CAPSLOCK), true);
        let mut expected = vec![Down(Key::CAPSLOCK), Up(Key::CAPSLOCK)];
        expected.extend(HYPER_DOWN);
        assert_eq!(v.synth.as_slice(), expected);
        // Only read on the first press.
        let v = e.on_event(down(Key::CAPSLOCK), true);
        assert!(v.synth.is_empty());
    }

    #[test]
    fn physical_shift_still_held_is_not_released() {
        let mut e = Engine::new();
        // Held before Hyper: passes through.
        assert!(!e.on_event(down(Key::LSHIFT), false).swallow);
        e.on_event(down(Key::CAPSLOCK), false);
        // Released and pressed again during Hyper: swallowed.
        let v = e.on_event(up(Key::LSHIFT), false);
        assert!(v.swallow && v.synth.is_empty());
        let v = e.on_event(down(Key::LSHIFT), false);
        assert!(v.swallow && v.synth.is_empty());

        let v = e.on_event(up(Key::CAPSLOCK), false);
        assert_eq!(
            v.synth.as_slice(),
            [
                Down(Key::MASK),
                Up(Key::MASK),
                Up(Key::LWIN),
                Up(Key::LALT),
                Up(Key::LCTRL)
            ]
        );
        // After Hyper, the physical up passes through.
        assert!(!e.on_event(up(Key::LSHIFT), false).swallow);
    }

    #[test]
    fn shift_released_during_hyper_is_released() {
        let mut e = Engine::new();
        e.on_event(down(Key::LSHIFT), false);
        e.on_event(down(Key::CAPSLOCK), false);
        e.on_event(up(Key::LSHIFT), false);
        assert_eq!(
            e.on_event(up(Key::CAPSLOCK), false).synth.as_slice(),
            HYPER_UP
        );
    }

    #[test]
    fn all_physical_modifiers_are_swallowed_while_held() {
        let mut e = Engine::new();
        e.on_event(down(Key::CAPSLOCK), false);
        for k in Key::SIDED_MODIFIERS {
            for d in [true, false] {
                let v = e.on_event(KeyEvent { key: k, down: d }, false);
                assert!(v.swallow, "{k:?}");
                assert!(v.synth.is_empty());
            }
        }
    }

    #[test]
    fn right_modifier_does_not_block_left_release() {
        let mut e = Engine::new();
        e.on_event(down(Key::CAPSLOCK), false);
        e.on_event(down(Key::RCTRL), false);
        assert_eq!(
            e.on_event(up(Key::CAPSLOCK), false).synth.as_slice(),
            HYPER_UP
        );
    }

    #[test]
    fn caps_up_without_down_is_swallowed() {
        let mut e = Engine::new();
        let v = e.on_event(up(Key::CAPSLOCK), false);
        assert!(v.swallow);
        assert!(v.synth.is_empty());
        assert_eq!(v.transition, None);
    }

    #[test]
    fn force_release_when_held_and_not_held() {
        let mut e = Engine::new();
        assert!(e.force_release().is_empty());
        e.on_event(down(Key::CAPSLOCK), false);
        assert_eq!(e.force_release().as_slice(), HYPER_UP);
        assert!(!e.is_held() && !e.modifiers_down());
        assert!(e.force_release().is_empty());
        // The real CapsLock up after a forced release is swallowed without injecting.
        let v = e.on_event(up(Key::CAPSLOCK), false);
        assert!(v.swallow && v.synth.is_empty());
    }

    #[test]
    fn reset_clears_state() {
        let mut e = Engine::new();
        e.on_event(down(Key::LSHIFT), false);
        e.on_event(down(Key::CAPSLOCK), false);
        e.reset();
        assert_eq!(e, Engine::new());
        assert!(e.force_release().is_empty());
    }

    #[test]
    fn other_keys_pass_through() {
        let mut e = Engine::new();
        let t = Key(0x54);
        assert_eq!(e.on_event(down(t), false), Verdict::default());
        assert_eq!(e.on_event(up(t), false), Verdict::default());
        e.on_event(down(Key::CAPSLOCK), false);
        assert_eq!(e.on_event(down(t), false), Verdict::default());
        assert_eq!(e.on_event(up(t), false), Verdict::default());
        // Generic modifiers never arrive from LL hooks; they are not treated as modifiers.
        assert!(!e.on_event(down(Key::CTRL), false).swallow);
    }

    #[test]
    fn modifiers_pass_when_not_held() {
        let mut e = Engine::new();
        for k in Key::SIDED_MODIFIERS {
            assert!(!e.on_event(down(k), false).swallow);
            assert!(!e.on_event(up(k), false).swallow);
        }
    }

    #[test]
    fn synth_buf_capacity() {
        let mut b = SynthBuf::new();
        for _ in 0..SYNTH_CAPACITY + 5 {
            b.push(Down(Key::MASK));
        }
        assert_eq!(b.len(), SYNTH_CAPACITY);
    }
}
