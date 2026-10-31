//! Portable Hyperkey state machine (no OS types), see `docs/specs/hyperkey.md` and, for the
//! binding lookup, `docs/specs/shortcuts.md`.
//!
//! The engine is called from the low-level keyboard hook callback, so nothing here allocates,
//! locks, logs or panics on any input.

use std::sync::Arc;

use kuality_protocol::{Chord, Key};

use crate::shortcuts::CompiledBindings;

/// Letters Windows' Office key (Ctrl+Shift+Alt+Win) combines with: Explorer registers them as
/// hotkeys and opens Office / Teams / LinkedIn web pages. While Hyper is held they are dropped
/// unless bound to a shortcut. Apps never receive these combos anyway (Explorer takes them).
pub const OFFICE_LETTERS: [Key; 9] = [
    Key(b'W' as u16),
    Key(b'X' as u16),
    Key(b'P' as u16),
    Key(b'O' as u16),
    Key(b'T' as u16),
    Key(b'D' as u16),
    Key(b'N' as u16),
    Key(b'L' as u16),
    Key(b'Y' as u16),
];

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
    /// Index into the bindings table of the shortcut to run.
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

/// Set of key codes `0..256` (no allocation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeySet([u64; 4]);

impl KeySet {
    pub const EMPTY: KeySet = KeySet([0; 4]);

    pub fn contains(&self, key: Key) -> bool {
        let c = usize::from(key.0);
        c < 256 && self.0[c / 64] & (1 << (c % 64)) != 0
    }

    pub fn set(&mut self, key: Key, on: bool) {
        let c = usize::from(key.0);
        if c < 256 {
            if on {
                self.0[c / 64] |= 1 << (c % 64);
            } else {
                self.0[c / 64] &= !(1 << (c % 64));
            }
        }
    }
}

/// The Hyperkey state machine.
#[derive(Debug, Clone, PartialEq)]
pub struct Engine {
    /// Hyper is active (CapsLock physically down).
    held: bool,
    /// Hyper modifiers we have injected down.
    injected: ModSet,
    /// Physical state of the eight sided modifiers.
    physical: ModSet,
    /// Bound keys whose press triggered a shortcut: their repeats and up are swallowed.
    consumed: KeySet,
    /// Shortcut table (`None` until the first config is applied).
    bindings: Option<Arc<CompiledBindings>>,
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
            consumed: KeySet::EMPTY,
            bindings: None,
        }
    }

    /// Replaces the shortcut table and returns the previous one (so the caller decides where it
    /// is dropped). Keys already consumed stay consumed, so their up is still swallowed.
    pub fn set_bindings(
        &mut self,
        bindings: Arc<CompiledBindings>,
    ) -> Option<Arc<CompiledBindings>> {
        self.bindings.replace(bindings)
    }

    /// Bound keys currently consumed (pressed, up not seen yet).
    pub fn consumed(&self) -> KeySet {
        self.consumed
    }

    /// Restores consumed keys after a hook reinstall (whose forced release cleared them), so
    /// a config reload while a bound key is held still swallows its up.
    pub fn restore_consumed(&mut self, keys: KeySet) {
        self.consumed = keys;
    }

    /// Generation of the current shortcut table (0 if none).
    pub fn bindings_generation(&self) -> u64 {
        self.bindings.as_ref().map_or(0, |b| b.generation)
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

        // Shortcuts (shortcuts.md): a consumed key stays swallowed until its up.
        if self.consumed.contains(ev.key) {
            if !ev.down {
                self.consumed.set(ev.key, false);
            }
            return Verdict::swallow();
        }
        if ev.down
            && self.held
            && !is_modifier
            && let Some(index) = self.bindings.as_ref().and_then(|b| b.find(ev.key))
        {
            self.consumed.set(ev.key, true);
            let mut v = Verdict::swallow();
            v.action = Some(index);
            return v;
        }
        // Office letters: Explorer owns Hyper+letter for these and opens Office web pages; an
        // unbound one is dropped (until its up) so Explorer never sees it.
        if ev.down && self.held && OFFICE_LETTERS.contains(&ev.key) {
            self.consumed.set(ev.key, true);
            return Verdict::swallow();
        }

        // Rule 7: everything else passes.
        Verdict::pass()
    }

    /// Events for a `send_hotkey` chord, in one batch: if Hyper is held, its injected modifiers
    /// are released first (mask key, physically held ones kept) and pressed again after the
    /// chord, so the target sees exactly the chord.
    pub fn hotkey(&mut self, chord: &Chord) -> SynthBuf {
        let held = self.held;
        let mut out = if held {
            self.release_injected()
        } else {
            SynthBuf::new()
        };
        for &m in chord.mods() {
            out.push(Synth::Down(m));
        }
        if let Some(k) = chord.key {
            out.push(Synth::Down(k));
            out.push(Synth::Up(k));
        }
        for &m in chord.mods().iter().rev() {
            out.push(Synth::Up(m));
        }
        if held {
            for k in HYPER_PRESS_ORDER {
                out.push(Synth::Down(k));
                self.injected.set(k, true);
            }
        }
        out
    }

    /// Releases Hyper if it is held (or modifiers are still injected); empty otherwise.
    /// Also forgets consumed keys (their up then passes through).
    pub fn force_release(&mut self) -> SynthBuf {
        self.consumed = KeySet::EMPTY;
        if self.held || !self.injected.is_empty() {
            self.release()
        } else {
            SynthBuf::new()
        }
    }

    /// Forgets all state without injecting anything (after unlock/resume). Keeps the shortcut
    /// table.
    pub fn reset(&mut self) {
        let bindings = self.bindings.take();
        *self = Self::new();
        self.bindings = bindings;
    }

    /// Whether Kuality currently holds injected modifiers down.
    pub fn modifiers_down(&self) -> bool {
        !self.injected.is_empty()
    }

    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Ends Hyper and releases the injected modifiers. Consumed keys stay consumed: a bound
    /// key released after CapsLock is still swallowed.
    fn release(&mut self) -> SynthBuf {
        let out = self.release_injected();
        self.held = false;
        out
    }

    /// Mask key, then `Up` for each injected modifier not physically held.
    fn release_injected(&mut self) -> SynthBuf {
        let mut out = SynthBuf::new();
        out.push(Synth::Down(Key::MASK));
        out.push(Synth::Up(Key::MASK));
        for k in HYPER_RELEASE_ORDER {
            if self.injected.contains(k) && !self.physical.contains(k) {
                out.push(Synth::Up(k));
            }
        }
        self.injected = ModSet::EMPTY;
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
        let t = Key(0x4B);
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

    use crate::shortcuts::{CompiledAction, CompiledBinding};

    const K1: Key = Key(0x31);
    const K2: Key = Key(0x32);
    const KK: Key = Key(0x4B);

    fn table(generation: u64, keys: &[Key]) -> Arc<CompiledBindings> {
        Arc::new(CompiledBindings {
            generation,
            entries: keys
                .iter()
                .enumerate()
                .map(|(i, &key)| CompiledBinding {
                    key,
                    source: i,
                    action: CompiledAction::OpenUrl("https://example.com".into()),
                })
                .collect(),
        })
    }

    fn engine_with(keys: &[Key]) -> Engine {
        let mut e = Engine::new();
        assert!(e.set_bindings(table(5, keys)).is_none());
        e
    }

    #[test]
    fn bound_key_fires_once_and_is_consumed() {
        let mut e = engine_with(&[K1, K2]);
        e.on_event(down(Key::CAPSLOCK), false);
        let v = e.on_event(down(K2), false);
        assert!(v.swallow && v.synth.is_empty());
        assert_eq!(v.action, Some(1));
        // Auto-repeat: swallowed, no action.
        for _ in 0..3 {
            let v = e.on_event(down(K2), false);
            assert!(v.swallow);
            assert_eq!(v.action, None);
        }
        let v = e.on_event(up(K2), false);
        assert!(v.swallow && v.action.is_none());
        // A new press fires again.
        assert_eq!(e.on_event(down(K2), false).action, Some(1));
        e.on_event(up(K2), false);
        assert_eq!(e.bindings_generation(), 5);
    }

    #[test]
    fn unbound_office_letters_are_dropped_while_held() {
        for k in OFFICE_LETTERS {
            let mut e = engine_with(&[K1]);
            // Not held: a normal key.
            assert_eq!(e.on_event(down(k), false), Verdict::default());
            assert_eq!(e.on_event(up(k), false), Verdict::default());
            e.on_event(down(Key::CAPSLOCK), false);
            let v = e.on_event(down(k), false);
            assert!(v.swallow && v.synth.is_empty() && v.action.is_none());
            // Auto-repeat and up: swallowed too.
            assert!(e.on_event(down(k), false).swallow);
            assert!(e.on_event(up(k), false).swallow);
            // Pressed during Hyper, released after it: the up is still swallowed.
            e.on_event(down(k), false);
            e.on_event(up(Key::CAPSLOCK), false);
            assert!(e.on_event(up(k), false).swallow);
            assert_eq!(e.on_event(down(k), false), Verdict::default());
            assert_eq!(e.on_event(up(k), false), Verdict::default());
        }
    }

    #[test]
    fn bound_office_letter_fires_its_shortcut() {
        let t = Key(b'T' as u16);
        let mut e = engine_with(&[K1, t]);
        e.on_event(down(Key::CAPSLOCK), false);
        let v = e.on_event(down(t), false);
        assert!(v.swallow && v.synth.is_empty());
        assert_eq!(v.action, Some(1));
        assert!(e.on_event(up(t), false).swallow);
    }

    #[test]
    fn office_letter_pressed_before_hyper_passes() {
        let t = Key(b'T' as u16);
        let mut e = Engine::new();
        assert_eq!(e.on_event(down(t), false), Verdict::default());
        e.on_event(down(Key::CAPSLOCK), false);
        assert_eq!(e.on_event(up(t), false), Verdict::default());
    }

    #[test]
    fn up_after_caps_released_is_still_swallowed() {
        let mut e = engine_with(&[K1]);
        e.on_event(down(Key::CAPSLOCK), false);
        assert_eq!(e.on_event(down(K1), false).action, Some(0));
        assert_eq!(
            e.on_event(up(Key::CAPSLOCK), false).synth.as_slice(),
            HYPER_UP
        );
        // Repeat and up after Hyper ended: swallowed, no action.
        let v = e.on_event(down(K1), false);
        assert!(v.swallow && v.action.is_none());
        assert!(e.on_event(up(K1), false).swallow);
        // Afterwards, the key is a normal key again.
        assert_eq!(e.on_event(down(K1), false), Verdict::default());
        assert_eq!(e.on_event(up(K1), false), Verdict::default());
    }

    #[test]
    fn unbound_and_unheld_keys_pass() {
        let mut e = engine_with(&[K1]);
        // Not held: a bound key passes.
        assert_eq!(e.on_event(down(K1), false), Verdict::default());
        assert_eq!(e.on_event(up(K1), false), Verdict::default());
        e.on_event(down(Key::CAPSLOCK), false);
        // Held, unbound: passes (the system sees the Hyper combo).
        assert_eq!(e.on_event(down(KK), false), Verdict::default());
        assert_eq!(e.on_event(up(KK), false), Verdict::default());
        // Key pressed before CapsLock, released while held: passes.
        let mut e = engine_with(&[K1]);
        e.on_event(down(K1), false);
        e.on_event(down(Key::CAPSLOCK), false);
        assert_eq!(e.on_event(up(K1), false), Verdict::default());
    }

    #[test]
    fn no_bindings_means_phase_1_behavior() {
        let mut e = Engine::new();
        e.on_event(down(Key::CAPSLOCK), false);
        assert_eq!(e.on_event(down(K1), false), Verdict::default());
        let mut e = engine_with(&[]);
        e.on_event(down(Key::CAPSLOCK), false);
        assert_eq!(e.on_event(down(K1), false), Verdict::default());
    }

    #[test]
    fn physical_modifiers_do_not_change_the_binding() {
        let mut e = engine_with(&[K1]);
        e.on_event(down(Key::CAPSLOCK), false);
        assert!(e.on_event(down(Key::RSHIFT), false).swallow);
        assert_eq!(e.on_event(down(K1), false).action, Some(0));
    }

    #[test]
    fn reset_and_force_release_clear_consumed_but_keep_bindings() {
        let mut e = engine_with(&[K1]);
        e.on_event(down(Key::CAPSLOCK), false);
        e.on_event(down(K1), false);
        e.reset();
        assert_eq!(e, engine_with(&[K1]));
        assert_eq!(e.on_event(up(K1), false), Verdict::default());

        let mut e = engine_with(&[K1]);
        e.on_event(down(Key::CAPSLOCK), false);
        e.on_event(down(K1), false);
        assert_eq!(e.force_release().as_slice(), HYPER_UP);
        assert_eq!(e.on_event(up(K1), false), Verdict::default());
        assert_eq!(e.bindings_generation(), 5);
    }

    #[test]
    fn apply_keeps_consumed_keys() {
        let mut e = engine_with(&[K1]);
        e.on_event(down(Key::CAPSLOCK), false);
        e.on_event(down(K1), false);
        let old = e.set_bindings(table(6, &[K2]));
        assert_eq!(old.map(|b| b.generation), Some(5));
        assert_eq!(e.bindings_generation(), 6);
        // K1 is no longer bound, but its press was consumed: the up is still swallowed.
        assert!(e.on_event(up(K1), false).swallow);
        assert_eq!(e.on_event(down(K2), false).action, Some(0));
    }

    #[test]
    fn hotkey_while_held_releases_and_presses_hyper_again() {
        let mut e = Engine::new();
        e.on_event(down(Key::CAPSLOCK), false);
        let chord = kuality_protocol::parse_chord("ctrl+shift+esc").unwrap();
        let esc = Key(0x1B);
        let mut expected = HYPER_UP.to_vec();
        expected.extend([
            Down(Key::LCTRL),
            Down(Key::LSHIFT),
            Down(esc),
            Up(esc),
            Up(Key::LSHIFT),
            Up(Key::LCTRL),
        ]);
        expected.extend(HYPER_DOWN);
        assert_eq!(e.hotkey(&chord).as_slice(), expected);
        assert!(e.is_held() && e.modifiers_down());
        // Hyper release afterwards is the normal sequence.
        assert_eq!(
            e.on_event(up(Key::CAPSLOCK), false).synth.as_slice(),
            HYPER_UP
        );
    }

    #[test]
    fn hotkey_keeps_physically_held_modifiers_down() {
        let mut e = Engine::new();
        e.on_event(down(Key::CAPSLOCK), false);
        e.on_event(down(Key::LSHIFT), false);
        let chord = kuality_protocol::parse_chord("win").unwrap();
        assert_eq!(
            e.hotkey(&chord).as_slice(),
            [
                Down(Key::MASK),
                Up(Key::MASK),
                Up(Key::LWIN),
                Up(Key::LALT),
                Up(Key::LCTRL),
                Down(Key::LWIN),
                Up(Key::LWIN),
                Down(Key::LCTRL),
                Down(Key::LSHIFT),
                Down(Key::LALT),
                Down(Key::LWIN),
            ]
        );
    }

    #[test]
    fn hotkey_when_not_held_is_just_the_chord() {
        let mut e = Engine::new();
        let chord = kuality_protocol::parse_chord("volume_mute").unwrap();
        assert_eq!(
            e.hotkey(&chord).as_slice(),
            [Down(Key(0xAD)), Up(Key(0xAD))]
        );
        assert!(!e.modifiers_down());
        // Longest chord fits in the buffer.
        e.on_event(down(Key::CAPSLOCK), false);
        let chord = kuality_protocol::parse_chord("lctrl+rctrl+shift+alt+k").unwrap();
        assert_eq!(e.hotkey(&chord).len(), 6 + 4 + 2 + 4 + 4);
    }

    #[test]
    fn key_set() {
        let mut s = KeySet::EMPTY;
        for c in [0u16, 63, 64, 200, 255] {
            s.set(Key(c), true);
            assert!(s.contains(Key(c)));
        }
        s.set(Key(300), true);
        assert!(!s.contains(Key(300)));
        s.set(Key(64), false);
        assert!(!s.contains(Key(64)) && s.contains(Key(63)));
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
