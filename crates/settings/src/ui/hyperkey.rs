//! Hyperkey tab.

use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, h_flex, v_flex};
use gpui_kit::*;
use kuality_protocol::HyperModifiers;

use super::SettingsWindow;
use super::controls::{self, divider, hint_text, hyper_keycap, section, setting_row, text_keycap};

/// Options of the "Apps see" control.
const MODIFIERS: [(HyperModifiers, &str); 3] = [
    (HyperModifiers::None, "Nothing"),
    (HyperModifiers::CtrlShiftAlt, "Ctrl+Shift+Alt"),
    (HyperModifiers::CtrlShiftAltWin, "Ctrl+Shift+Alt+Win"),
];

impl SettingsWindow {
    pub(super) fn render_hyperkey(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let h = &self.doc.config.hyperkey;
        let (enabled, modifiers) = (h.enabled, h.modifiers);
        let ro = self.read_only();
        let toggle = Switch::new("hyperkey-enabled")
            .accessibility_label("Enable Hyperkey")
            .checked(enabled)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.hyperkey.enabled = *on;
                this.changed(cx);
            }));
        let picker = controls::segmented(
            "hyperkey-modifiers",
            &MODIFIERS,
            Some(modifiers),
            ro,
            cx,
            |this, m, _, cx| {
                this.doc.config.hyperkey.modifiers = m;
                this.changed(cx);
            },
        );
        let names: &[&str] = match modifiers {
            HyperModifiers::None => &[],
            HyperModifiers::CtrlShiftAlt => &["Ctrl", "Shift", "Alt"],
            HyperModifiers::CtrlShiftAltWin => &["Ctrl", "Shift", "Alt", "Win"],
        };
        let description = if modifiers.injects() {
            format!("Hold CapsLock to press {}.", names.join("+"))
        } else {
            "Hold CapsLock + a key to run a Kuality shortcut.".to_owned()
        };
        // `[CapsLock] → [✦] = [Ctrl] [Shift] [Alt] [Win]`: what holding CapsLock presses.
        let mut legend = h_flex()
            .gap_1()
            .items_center()
            .child(text_keycap("CapsLock", cx))
            .child(hint_text("→", cx))
            .child(hyper_keycap());
        if modifiers.injects() {
            legend = legend.child(hint_text("=", cx));
            for k in names {
                legend = legend.child(text_keycap(*k, cx));
            }
        }
        let mut notes = vec![hint_text(
            "While enabled, CapsLock does not toggle caps. Hyper is shown as the star key.",
            cx,
        )];
        if modifiers.includes_win() {
            notes.push(hint_text(
                "The Office key shortcut (Ctrl+Shift+Alt+Win alone) is disabled while Hyperkey is \
                 on.",
                cx,
            ));
        }
        notes.push(hint_text(
            "Tray → Pause turns it off temporarily. Hyper shortcuts need it; shortcuts with \
             Ctrl, Alt, Shift or Win do not.",
            cx,
        ));
        let modifiers_hint = match modifiers {
            HyperModifiers::None => {
                "Apps never see Hyper, so nothing reacts to a stray Alt, Win or Ctrl+Shift. \
                 Keys without a shortcut do nothing while CapsLock is held. To reach an app, \
                 use a \"Send hotkey\" shortcut (e.g. Hyper+K → Ctrl+Alt+F24)."
            }
            HyperModifiers::CtrlShiftAlt => {
                "Apps can bind Hyper themselves. On some keyboard layouts (e.g. Spanish) \
                 Ctrl+Alt acts as AltGr, so some keys may type a character."
            }
            HyperModifiers::CtrlShiftAltWin => "Apps can bind Hyper themselves.",
        };
        v_flex().gap_4().child(
            section(cx)
                .child(setting_row(
                    "Enable Hyperkey",
                    Some(description.into()),
                    toggle,
                    cx,
                ))
                .child(v_flex().px_4().pb_3().gap_2().child(legend).children(notes))
                .child(divider(cx))
                .child(setting_row(
                    "Apps see",
                    Some("What other apps receive while CapsLock is held.".into()),
                    picker,
                    cx,
                ))
                .child(v_flex().px_4().pb_3().child(hint_text(modifiers_hint, cx))),
        )
    }
}
