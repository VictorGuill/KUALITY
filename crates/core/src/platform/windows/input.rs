//! `SendInput` helpers for the keyboard thread, and crash-time emergency release.
//!
//! Everything here may run inside the low-level hook callback or a crash handler: no heap
//! allocation, no locks, no logging.

use std::sync::atomic::{AtomicBool, Ordering};

use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC_EX, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};
use kuality_protocol::Key;

use crate::hyperkey::engine::{HYPER_RELEASE_ORDER, SYNTH_CAPACITY, Synth, SynthBuf};

/// `dwExtraInfo` of every event Kuality injects ("KUAL"), so the hook can skip its own events.
pub const KUALITY_TAG: usize = 0x4B55_414C;

/// Set while Kuality holds injected modifiers down; read by [`emergency_release`].
static MODIFIERS_DOWN: AtomicBool = AtomicBool::new(false);

/// Records whether injected modifiers are currently down.
pub fn set_modifiers_down(down: bool) {
    MODIFIERS_DOWN.store(down, Ordering::SeqCst);
}

/// Keys that need `KEYEVENTF_EXTENDEDKEY` (VK codes).
fn is_extended(vk: u16) -> bool {
    matches!(
        vk,
        0xA3 | 0xA5   // RCTRL, RALT
        | 0x5B..=0x5D // LWIN, RWIN, APPS
        | 0x2D | 0x2E // INSERT, DELETE
        | 0x21..=0x28 // PAGEUP, PAGEDOWN, END, HOME, LEFT, UP, RIGHT, DOWN
        | 0x90        // NUMLOCK
        | 0x2C        // PRINTSCREEN
        | 0x6F        // numpad DIVIDE
        | 0xA6..=0xB7 // browser, volume, media and launch keys
    )
}

fn to_input(s: Synth) -> INPUT {
    let (key, up) = match s {
        Synth::Down(k) => (k, false),
        Synth::Up(k) => (k, true),
    };
    let (scan, mut flags) = if key == Key::MASK {
        (0, KEYBD_EVENT_FLAGS(0))
    } else {
        // SAFETY: pure lookup in the current keyboard layout; no pointers.
        let mapped = unsafe { MapVirtualKeyW(u32::from(key.0), MAPVK_VK_TO_VSC_EX) };
        let ext = is_extended(key.0) || (mapped & 0xFF00) == 0xE000;
        (
            (mapped & 0xFF) as u16,
            if ext {
                KEYEVENTF_EXTENDEDKEY
            } else {
                KEYBD_EVENT_FLAGS(0)
            },
        )
    };
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(key.0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: KUALITY_TAG,
            },
        },
    }
}

/// Injects all events with a single `SendInput` call. Returns `true` if all were inserted
/// (an empty buffer counts as success).
pub fn send(buf: &SynthBuf) -> bool {
    if buf.is_empty() {
        return true;
    }
    let mut inputs = [INPUT::default(); SYNTH_CAPACITY];
    for (slot, &s) in inputs.iter_mut().zip(buf.as_slice()) {
        *slot = to_input(s);
    }
    let inputs = &inputs[..buf.len()];
    // SAFETY: `inputs` is a valid slice of initialized INPUT structs.
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    sent as usize == inputs.len()
}

/// Crash-time release: if Kuality holds modifiers down, injects mask + Up for all Hyper
/// modifiers. Never allocates, locks or panics.
pub fn emergency_release() {
    if !MODIFIERS_DOWN.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut buf = SynthBuf::new();
    buf.push(Synth::Down(Key::MASK));
    buf.push(Synth::Up(Key::MASK));
    for k in HYPER_RELEASE_ORDER {
        buf.push(Synth::Up(k));
    }
    let _ = send(&buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_keys() {
        assert!(is_extended(Key::LWIN.0));
        assert!(is_extended(Key::RWIN.0));
        assert!(is_extended(Key::RCTRL.0));
        assert!(is_extended(Key::RALT.0));
        assert!(is_extended(0x2E)); // DELETE
        assert!(is_extended(0xAD)); // VOLUME_MUTE
        assert!(!is_extended(Key::LCTRL.0));
        assert!(!is_extended(Key::LSHIFT.0));
        assert!(!is_extended(Key::LALT.0));
        assert!(!is_extended(Key::CAPSLOCK.0));
        assert!(!is_extended(0x54)); // T
    }

    #[test]
    fn inputs_are_tagged() {
        let i = to_input(Synth::Up(Key::LWIN));
        // SAFETY: `to_input` always fills the keyboard variant.
        let ki = unsafe { i.Anonymous.ki };
        assert_eq!(ki.dwExtraInfo, KUALITY_TAG);
        assert_eq!(ki.wVk.0, Key::LWIN.0);
        assert!(ki.dwFlags.contains(KEYEVENTF_KEYUP));
        assert!(ki.dwFlags.contains(KEYEVENTF_EXTENDEDKEY));
        let m = to_input(Synth::Down(Key::MASK));
        // SAFETY: as above.
        let ki = unsafe { m.Anonymous.ki };
        assert_eq!(ki.wScan, 0);
        assert!(!ki.dwFlags.contains(KEYEVENTF_KEYUP));
    }
}
