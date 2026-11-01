//! Portable keyboard state machine (no OS types): Hyperkey, see `docs/specs/hyperkey.md`, and
//! the shortcut lookup (Hyper and normal hotkeys), see `docs/specs/shortcuts.md`.
//!
//! The engine is called from the low-level keyboard hook callback, so nothing here allocates,
//! locks, logs or panics on any input.

use std::sync::Arc;

use kuality_protocol::{Chord, HyperModifiers, Key, Mods};

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

    /// The generic modifiers of the sided keys in the set (`RCTRL` → `ctrl`).
    pub fn generic(self) -> Mods {
        let mut m = Mods::NONE;
        for k in Key::SIDED_MODIFIERS {
            if self.contains(k) {
                m |= Mods::of(k);
            }
        }
        m
    }

    /// Keys in the set, in [`Key::SIDED_MODIFIERS`] order.
    pub fn keys(self) -> impl Iterator<Item = Key> {
        Key::SIDED_MODIFIERS
            .into_iter()
            .filter(move |&k| self.contains(k))
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

/// The keyboard state machine.
#[derive(Debug, Clone, PartialEq)]
pub struct Engine {
    /// Hyperkey is on: CapsLock is Hyper. Off: CapsLock is a normal key (the hook may still run
    /// for normal hotkeys).
    hyper_enabled: bool,
    /// Modifiers Hyper injects (none: Hyper only triggers Kuality shortcuts).
    modifiers: HyperModifiers,
    /// Hyper is active (CapsLock physically down).
    held: bool,
    /// Hyper modifiers we have injected down.
    injected: ModSet,
    /// Physical state of the eight sided modifiers.
    physical: ModSet,
    /// Bound keys whose press triggered a shortcut: their repeats and up are swallowed.
    consumed: KeySet,
    /// Keys whose press triggered a pass-through shortcut: repeats pass without firing again.
    passed: KeySet,
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
            hyper_enabled: true,
            // Tests start from the full Ctrl+Shift+Alt+Win; the core always calls `set_hyper`.
            modifiers: HyperModifiers::CtrlShiftAltWin,
            held: false,
            injected: ModSet(0),
            physical: ModSet(0),
            consumed: KeySet::EMPTY,
            passed: KeySet::EMPTY,
            bindings: None,
        }
    }

    /// Hyperkey settings. Call [`Engine::force_release`] first if Hyper may be held.
    pub fn set_hyper(&mut self, enabled: bool, modifiers: HyperModifiers) {
        self.hyper_enabled = enabled;
        self.modifiers = modifiers;
    }

    /// Modifiers Hyper presses, in press order (none in Kuality-only mode).
    fn hyper_press_order(&self) -> impl Iterator<Item = Key> + use<> {
        let m = self.modifiers;
        HYPER_PRESS_ORDER
            .into_iter()
            .filter(move |&k| m.injects() && (m.includes_win() || k != Key::LWIN))
    }

    /// Replaces the shortcut table and returns the previous one (so the caller decides where it
    /// is dropped). Keys already consumed stay consumed, so their up is still swallowed.
    pub fn set_bindings(
        &mut self,
        bindings: Arc<CompiledBindings>,
    ) -> Option<Arc<CompiledBindings>> {
        self.bindings.replace(bindings)
    }

    /// Bound keys currently pressed (consumed, passed), up not seen yet.
    pub fn pressed(&self) -> (KeySet, KeySet) {
        (self.consumed, self.passed)
    }

    /// Restores pressed keys after a hook reinstall (whose forced release cleared them), so
    /// a config reload while a bound key is held still swallows its up.
    pub fn restore_pressed(&mut self, (consumed, passed): (KeySet, KeySet)) {
        self.consumed = consumed;
        self.passed = passed;
    }

    /// Whether the hook should check for missed modifier ups before this event (see
    /// [`Engine::drop_stale_modifiers`]): a key down while no Hyper modifier is injected and
    /// some physical modifier is believed down.
    pub fn wants_modifier_check(&self, ev: KeyEvent) -> bool {
        ev.down
            && ev.key.sided_modifier_index().is_none()
            && self.injected.is_empty()
            && !self.physical.is_empty()
    }

    /// Forgets physical modifiers the system reports up: their up never reached the hook
    /// (hook not installed, elevated window in front). Never marks a key down.
    pub fn drop_stale_modifiers(&mut self, system_down: ModSet) {
        self.physical = ModSet(self.physical.0 & system_down.0);
    }

    /// Takes the physical modifier state from the system (on hook install, when nothing is
    /// injected).
    pub fn sync_physical(&mut self, system_down: ModSet) {
        if self.injected.is_empty() {
            self.physical = system_down;
        }
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

        if ev.key == Key::CAPSLOCK && self.hyper_enabled {
            return match (ev.down, self.held) {
                // Rule 2: Hyper starts.
                (true, false) => {
                    self.held = true;
                    let mut v = Verdict::swallow();
                    if caps_toggled_on {
                        v.synth.push(Synth::Down(Key::CAPSLOCK));
                        v.synth.push(Synth::Up(Key::CAPSLOCK));
                    }
                    for k in self.hyper_press_order() {
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

        // Rule 6: physical modifiers add nothing while Hyper is held. In Kuality-only mode they
        // pass: nothing was injected, so a swallowed up would leave the key stuck.
        if self.held && is_modifier && self.modifiers.injects() {
            return Verdict::swallow();
        }

        // Shortcuts (shortcuts.md): a consumed key stays swallowed until its up; a passed one
        // passes without firing again.
        if self.consumed.contains(ev.key) {
            if !ev.down {
                self.consumed.set(ev.key, false);
            }
            return Verdict::swallow();
        }
        if self.passed.contains(ev.key) {
            if !ev.down {
                self.passed.set(ev.key, false);
            }
            return Verdict::pass();
        }
        if ev.down && !is_modifier {
            if self.held {
                if let Some(index) = self.bindings.as_ref().and_then(|b| b.find_hyper(ev.key)) {
                    return self.fire(ev.key, index, false);
                }
            } else if !self.physical.is_empty() {
                let mods = self.physical.generic();
                if let Some(index) = self
                    .bindings
                    .as_ref()
                    .and_then(|b| b.find_normal(mods, ev.key))
                {
                    return self.fire(ev.key, index, true);
                }
            }
        }
        // Office letters: Explorer owns Ctrl+Shift+Alt+Win+letter for these and opens Office
        // web pages; an unbound one is dropped (until its up) so Explorer never sees it. Without
        // Win in Hyper, Explorer does not see the Office combination: nothing to drop.
        if ev.down && self.held && self.modifiers.includes_win() && OFFICE_LETTERS.contains(&ev.key)
        {
            self.consumed.set(ev.key, true);
            return Verdict::swallow();
        }
        // Kuality-only mode: an unbound key pressed with Hyper is dropped (until its up), so
        // holding CapsLock never types into the app in front.
        if ev.down && self.held && !is_modifier && !self.modifiers.injects() {
            self.consumed.set(ev.key, true);
            return Verdict::swallow();
        }

        // Rule 7: everything else passes.
        Verdict::pass()
    }

    /// Shortcut `index` fires on `key`'s press. Swallowed unless the binding passes through;
    /// a swallowed normal hotkey gets the mask key, so releasing its Win / Alt / Ctrl+Shift
    /// opens no Start menu, menu bar or input-language switch. So does a Hyper hotkey in
    /// Kuality-only mode while the user physically holds a modifier.
    fn fire(&mut self, key: Key, index: u16, normal: bool) -> Verdict {
        let pass_through = self
            .bindings
            .as_ref()
            .is_some_and(|b| b.passes_through(index));
        let mut v = if pass_through {
            self.passed.set(key, true);
            Verdict::pass()
        } else {
            self.consumed.set(key, true);
            let mut v = Verdict::swallow();
            if normal || (self.injected.is_empty() && !self.physical.is_empty()) {
                v.synth.push(Synth::Down(Key::MASK));
                v.synth.push(Synth::Up(Key::MASK));
            }
            v
        };
        v.action = Some(index);
        v
    }

    /// Events for a `send_hotkey` chord, in one batch, so the target sees exactly the chord:
    /// - Hyper held: its injected modifiers are released first (mask key, physically held ones
    ///   kept) and pressed again after the chord.
    /// - Otherwise (or Hyper held in Kuality-only mode) the modifiers the user still holds (e.g. the Ctrl of a `ctrl+t` hotkey) are
    ///   released first (mask key) and pressed again after the chord, followed by the mask key
    ///   when Win or Alt is among them (their later release must not open Start / the menu bar).
    pub fn hotkey(&mut self, chord: &Chord) -> SynthBuf {
        let held = self.held;
        let injecting = held && self.modifiers.injects();
        let user_mods = if injecting {
            ModSet::EMPTY
        } else {
            self.physical
        };
        let mut out = if injecting {
            self.release_injected()
        } else {
            SynthBuf::new()
        };
        if !user_mods.is_empty() {
            out.push(Synth::Down(Key::MASK));
            out.push(Synth::Up(Key::MASK));
            for k in user_mods.keys() {
                out.push(Synth::Up(k));
            }
        }
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
        if injecting {
            for k in self.hyper_press_order() {
                out.push(Synth::Down(k));
                self.injected.set(k, true);
            }
        }
        if !user_mods.is_empty() {
            for k in user_mods.keys() {
                out.push(Synth::Down(k));
            }
            if user_mods.generic().contains(Mods::WIN) || user_mods.generic().contains(Mods::ALT) {
                out.push(Synth::Down(Key::MASK));
                out.push(Synth::Up(Key::MASK));
            }
        }
        out
    }

    /// Releases Hyper if it is held (or modifiers are still injected); empty otherwise.
    /// Also forgets consumed keys (their up then passes through).
    pub fn force_release(&mut self) -> SynthBuf {
        self.consumed = KeySet::EMPTY;
        self.passed = KeySet::EMPTY;
        if self.held || !self.injected.is_empty() {
            self.release()
        } else {
            SynthBuf::new()
        }
    }

    /// Forgets all state without injecting anything (after unlock/resume). Keeps the shortcut
    /// table and the Hyperkey settings.
    pub fn reset(&mut self) {
        let bindings = self.bindings.take();
        let (enabled, modifiers) = (self.hyper_enabled, self.modifiers);
        *self = Self::new();
        self.bindings = bindings;
        self.set_hyper(enabled, modifiers);
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

    /// Mask key, then `Up` for each injected modifier not physically held. Empty when nothing
    /// is injected (Kuality-only mode).
    fn release_injected(&mut self) -> SynthBuf {
        let mut out = SynthBuf::new();
        if self.injected.is_empty() {
            return out;
        }
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
    use kuality_protocol::{Trigger, parse_trigger};

    const K1: Key = Key(0x31);
    const K2: Key = Key(0x32);
    const KK: Key = Key(0x4B);

    fn table(generation: u64, keys: &[Key]) -> Arc<CompiledBindings> {
        let triggers: Vec<(Trigger, bool)> =
            keys.iter().map(|&k| (Trigger::hyper(k), false)).collect();
        table_of(generation, &triggers)
    }

    fn table_of(generation: u64, triggers: &[(Trigger, bool)]) -> Arc<CompiledBindings> {
        Arc::new(CompiledBindings {
            generation,
            entries: triggers
                .iter()
                .enumerate()
                .map(|(i, &(trigger, pass_through))| CompiledBinding {
                    trigger,
                    pass_through,
                    source: i,
                    action: CompiledAction::OpenUrl("https://example.com".into()),
                })
                .collect(),
        })
    }

    /// Engine with bindings given as config text (`"ctrl+t"`, `"hyper+1"`) and pass-through
    /// flags.
    fn engine_of(bindings: &[(&str, bool)]) -> Engine {
        let triggers: Vec<(Trigger, bool)> = bindings
            .iter()
            .map(|&(s, p)| (parse_trigger(s).unwrap(), p))
            .collect();
        let mut e = Engine::new();
        e.set_bindings(table_of(1, &triggers));
        e
    }

    const T: Key = Key(0x54);
    const E: Key = Key(0x45);
    const MASKED: [Synth; 2] = [Down(Key::MASK), Up(Key::MASK)];

    #[test]
    fn normal_hotkey_fires_with_exact_modifiers_and_is_masked() {
        let mut e = engine_of(&[("ctrl+t", false), ("win+e", false)]);
        // Plain T: passes.
        assert_eq!(e.on_event(down(T), false), Verdict::default());
        assert_eq!(e.on_event(up(T), false), Verdict::default());
        // Ctrl passes (it is not Hyper), Ctrl+T fires, swallowed, with the mask key.
        assert_eq!(e.on_event(down(Key::RCTRL), false), Verdict::default());
        let v = e.on_event(down(T), false);
        assert!(v.swallow);
        assert_eq!(v.action, Some(0));
        assert_eq!(v.synth.as_slice(), MASKED);
        // Repeat and up swallowed, no second action.
        let v = e.on_event(down(T), false);
        assert!(v.swallow && v.action.is_none() && v.synth.is_empty());
        assert!(e.on_event(up(T), false).swallow);
        // Ctrl+Shift+T is another hotkey: not bound.
        e.on_event(down(Key::LSHIFT), false);
        assert_eq!(e.on_event(down(T), false), Verdict::default());
        e.on_event(up(T), false);
        e.on_event(up(Key::LSHIFT), false);
        e.on_event(up(Key::RCTRL), false);
        // Win+E.
        e.on_event(down(Key::LWIN), false);
        let v = e.on_event(down(E), false);
        assert!(v.swallow && v.action == Some(1) && v.synth.as_slice() == MASKED);
        // The modifier up passes (the mask already went between Win down and up).
        assert_eq!(e.on_event(up(Key::LWIN), false), Verdict::default());
        assert!(e.on_event(up(E), false).swallow);
    }

    #[test]
    fn pass_through_hotkey_fires_once_and_passes() {
        let mut e = engine_of(&[("ctrl+t", true)]);
        e.on_event(down(Key::LCTRL), false);
        let v = e.on_event(down(T), false);
        assert!(!v.swallow && v.synth.is_empty());
        assert_eq!(v.action, Some(0));
        for _ in 0..3 {
            assert_eq!(e.on_event(down(T), false), Verdict::default());
        }
        assert_eq!(e.on_event(up(T), false), Verdict::default());
        assert_eq!(e.on_event(down(T), false).action, Some(0));
    }

    #[test]
    fn hyper_pass_through_lets_the_hyper_combo_through() {
        let mut e = engine_of(&[("hyper+1", true)]);
        e.on_event(down(Key::CAPSLOCK), false);
        let v = e.on_event(down(K1), false);
        assert!(!v.swallow && v.synth.is_empty() && v.action == Some(0));
        assert_eq!(e.on_event(up(K1), false), Verdict::default());
    }

    #[test]
    fn normal_hotkeys_do_not_fire_while_hyper_is_held() {
        let mut e = engine_of(&[("ctrl+t", false), ("hyper+t", false)]);
        e.on_event(down(Key::LCTRL), false);
        e.on_event(down(Key::CAPSLOCK), false);
        assert_eq!(e.on_event(down(T), false).action, Some(1));
    }

    #[test]
    fn hyperkey_off_leaves_capslock_alone_but_normal_hotkeys_work() {
        let mut e = engine_of(&[("ctrl+t", false), ("hyper+1", false)]);
        e.set_hyper(false, HyperModifiers::CtrlShiftAltWin);
        assert_eq!(e.on_event(down(Key::CAPSLOCK), false), Verdict::default());
        assert_eq!(e.on_event(down(K1), false), Verdict::default());
        assert_eq!(e.on_event(up(K1), false), Verdict::default());
        assert_eq!(e.on_event(up(Key::CAPSLOCK), false), Verdict::default());
        assert!(!e.on_event(down(Key::LCTRL), false).swallow);
        assert_eq!(e.on_event(down(T), false).action, Some(0));
    }

    #[test]
    fn stale_modifiers_are_dropped_and_synced() {
        let mut e = engine_of(&[("alt+k", false)]);
        e.on_event(down(Key::LALT), false);
        // Its up never arrived (elevated window); the system says it is up.
        assert!(e.wants_modifier_check(down(KK)));
        assert!(!e.wants_modifier_check(down(Key::LSHIFT)));
        assert!(!e.wants_modifier_check(up(KK)));
        e.drop_stale_modifiers(ModSet::EMPTY);
        assert!(!e.wants_modifier_check(down(KK)));
        assert_eq!(e.on_event(down(KK), false), Verdict::default());
        e.on_event(up(KK), false);
        // A modifier held when the hook was installed.
        let mut held = ModSet::EMPTY;
        held.set(Key::RALT, true);
        e.sync_physical(held);
        assert_eq!(e.on_event(down(KK), false).action, Some(0));
    }

    #[test]
    fn hyper_without_win() {
        let mut e = engine_of(&[]);
        e.set_hyper(true, HyperModifiers::CtrlShiftAlt);
        let v = e.on_event(down(Key::CAPSLOCK), false);
        assert_eq!(
            v.synth.as_slice(),
            [Down(Key::LCTRL), Down(Key::LSHIFT), Down(Key::LALT)]
        );
        // Office letters pass: Explorer does not see Ctrl+Shift+Alt+letter as Office.
        assert_eq!(e.on_event(down(T), false), Verdict::default());
        assert_eq!(e.on_event(up(T), false), Verdict::default());
        let v = e.on_event(up(Key::CAPSLOCK), false);
        assert_eq!(
            v.synth.as_slice(),
            [
                Down(Key::MASK),
                Up(Key::MASK),
                Up(Key::LALT),
                Up(Key::LSHIFT),
                Up(Key::LCTRL)
            ]
        );
        // send_hotkey while held presses the three again.
        e.on_event(down(Key::CAPSLOCK), false);
        let chord = kuality_protocol::parse_chord("win+1").unwrap();
        let buf = e.hotkey(&chord);
        assert_eq!(
            &buf.as_slice()[buf.len() - 3..],
            [Down(Key::LCTRL), Down(Key::LSHIFT), Down(Key::LALT)]
        );
        // reset keeps the setting.
        e.reset();
        assert_eq!(e.on_event(down(Key::CAPSLOCK), false).synth.len(), 3);
    }

    #[test]
    fn kuality_only_injects_nothing() {
        let mut e = engine_of(&[("hyper+1", false), ("hyper+k", true)]);
        e.set_hyper(true, HyperModifiers::None);
        // CapsLock toggled on is still turned off; no modifier goes down.
        let v = e.on_event(down(Key::CAPSLOCK), true);
        assert!(v.swallow);
        assert_eq!(v.synth.as_slice(), [Down(Key::CAPSLOCK), Up(Key::CAPSLOCK)]);
        assert!(e.is_held() && !e.modifiers_down());
        // Bound key fires and is swallowed, without a mask key.
        let v = e.on_event(down(K1), false);
        assert_eq!(v.action, Some(0));
        assert!(v.swallow && v.synth.is_empty());
        assert!(e.on_event(up(K1), false).swallow);
        // Pass-through binding: the plain key reaches the app.
        let v = e.on_event(down(KK), false);
        assert_eq!(v.action, Some(1));
        assert!(!v.swallow);
        e.on_event(up(KK), false);
        // Unbound keys (Office letters included) are dropped until their up.
        for key in [T, E] {
            assert!(e.on_event(down(key), false).swallow);
            assert!(e.on_event(down(key), false).swallow, "repeat");
            assert!(e.on_event(up(key), false).swallow);
        }
        // Release injects nothing.
        let v = e.on_event(up(Key::CAPSLOCK), false);
        assert!(v.swallow && v.synth.is_empty());
        assert!(!e.is_held());
    }

    #[test]
    fn kuality_only_lets_physical_modifiers_through() {
        let mut e = engine_of(&[("hyper+1", false)]);
        e.set_hyper(true, HyperModifiers::None);
        e.on_event(down(Key::LALT), false);
        e.on_event(down(Key::CAPSLOCK), false);
        // A swallowed Hyper hotkey with Alt physically held gets the mask key.
        let v = e.on_event(down(K1), false);
        assert_eq!(v.action, Some(0));
        assert_eq!(v.synth.as_slice(), [Down(Key::MASK), Up(Key::MASK)]);
        e.on_event(up(K1), false);
        // Its up must reach the system: nothing was injected to release it later.
        assert!(!e.on_event(up(Key::LALT), false).swallow);
        assert!(e.on_event(up(Key::CAPSLOCK), false).synth.is_empty());
    }

    #[test]
    fn kuality_only_send_hotkey_sends_just_the_chord() {
        let mut e = engine_of(&[]);
        e.set_hyper(true, HyperModifiers::None);
        e.on_event(down(Key::CAPSLOCK), false);
        let chord = kuality_protocol::parse_chord("ctrl+alt+f24").unwrap();
        let f24 = chord.key.unwrap();
        assert_eq!(
            e.hotkey(&chord).as_slice(),
            [
                Down(Key::LCTRL),
                Down(Key::LALT),
                Down(f24),
                Up(f24),
                Up(Key::LALT),
                Up(Key::LCTRL),
            ]
        );
        assert!(!e.modifiers_down());
        // reset keeps the setting.
        e.reset();
        assert!(e.on_event(down(Key::CAPSLOCK), false).synth.is_empty());
    }

    #[test]
    fn hotkey_from_a_normal_chord_releases_and_restores_the_users_modifiers() {
        let mut e = engine_of(&[("ctrl+t", false)]);
        e.on_event(down(Key::LCTRL), false);
        e.on_event(down(T), false);
        let chord = kuality_protocol::parse_chord("win+1").unwrap();
        assert_eq!(
            e.hotkey(&chord).as_slice(),
            [
                Down(Key::MASK),
                Up(Key::MASK),
                Up(Key::LCTRL),
                Down(Key::LWIN),
                Down(K1),
                Up(K1),
                Up(Key::LWIN),
                Down(Key::LCTRL),
            ]
        );
        assert!(!e.modifiers_down(), "user modifiers are not Kuality's");
        // Win held by the user: mask after restoring it.
        let mut e = engine_of(&[("win+e", false)]);
        e.on_event(down(Key::RWIN), false);
        e.on_event(down(E), false);
        let chord = kuality_protocol::parse_chord("ctrl+c").unwrap();
        let buf = e.hotkey(&chord);
        assert_eq!(
            &buf.as_slice()[buf.len() - 3..],
            [Down(Key::RWIN), Down(Key::MASK), Up(Key::MASK)]
        );
        assert_eq!(buf.as_slice()[2], Up(Key::RWIN));
        // Longest case still fits: 8 held modifiers + 5-key chord.
        let mut e = engine_of(&[]);
        for k in Key::SIDED_MODIFIERS {
            e.on_event(down(k), false);
        }
        let chord = kuality_protocol::parse_chord("lctrl+rctrl+shift+alt+k").unwrap();
        assert_eq!(e.hotkey(&chord).len(), 2 + 8 + 10 + 8 + 2);
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
