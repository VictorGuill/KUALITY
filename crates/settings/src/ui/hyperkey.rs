//! Hyperkey tab.

use gpui_kit::component::setting::{SettingGroup, SettingPage};
use gpui_kit::component::{IconName, h_flex};
use gpui_kit::*;
use kuality_protocol::HyperModifiers;

use super::SettingsWindow;
use super::controls::{
    self, Note, Weak, block_item, control_item, hint_text, hyper_keycap, notes_item, switch_item,
    text_keycap,
};

/// Options of the "Apps see" control.
const MODIFIERS: [(HyperModifiers, &str); 3] = [
    (HyperModifiers::None, "Nothing"),
    (HyperModifiers::CtrlShiftAlt, "Ctrl+Shift+Alt"),
    (HyperModifiers::CtrlShiftAltWin, "Ctrl+Shift+Alt+Win"),
];

/// The modifier keys Hyper presses for apps.
fn modifier_names(modifiers: HyperModifiers) -> &'static [&'static str] {
    match modifiers {
        HyperModifiers::None => &[],
        HyperModifiers::CtrlShiftAlt => &["Ctrl", "Shift", "Alt"],
        HyperModifiers::CtrlShiftAltWin => &["Ctrl", "Shift", "Alt", "Win"],
    }
}

impl SettingsWindow {
    pub(super) fn hyperkey_page(&self, weak: &Weak, _cx: &mut Context<Self>) -> SettingPage {
        let modifiers = self.doc.config.hyperkey.modifiers;
        let ro = self.read_only();
        let names = modifier_names(modifiers);
        let description = if modifiers.injects() {
            format!("Hold CapsLock to press {}.", names.join("+"))
        } else {
            "Hold CapsLock + a key to run a Kuality shortcut.".to_owned()
        };
        let mut notes = vec![Note::hint(
            "While enabled, CapsLock does not toggle caps. Hyper is shown as the star key.",
        )];
        if modifiers.includes_win() {
            notes.push(Note::hint(
                "The Office key shortcut (Ctrl+Shift+Alt+Win alone) is disabled while Hyperkey is \
                 on.",
            ));
        }
        notes.push(Note::hint(
            "Tray → Pause turns it off temporarily. Hyper shortcuts need it; shortcuts with \
             Ctrl, Alt, Shift or Win do not.",
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

        let enable = switch_item(
            "Enable Hyperkey",
            weak,
            |w| w.doc.config.hyperkey.enabled,
            |w, on, cx| {
                w.doc.config.hyperkey.enabled = on;
                w.changed(cx);
            },
        )
        .description(description)
        .disabled(ro);
        // `[CapsLock] → [✦] = [Ctrl] [Shift] [Alt] [Win]`: what holding CapsLock presses.
        let legend = block_item(weak, |this, _, cx| {
            let names = modifier_names(this.doc.config.hyperkey.modifiers);
            h_flex()
                .gap_1()
                .items_center()
                .child(text_keycap("CapsLock"))
                .child(hint_text("→", cx))
                .child(hyper_keycap(cx))
                .children((!names.is_empty()).then(|| hint_text("=", cx)))
                .children(names.iter().map(|k| text_keycap(*k)))
        });
        let apps_see = control_item("Apps see", weak, |this, _, cx| {
            controls::segmented(
                "hyperkey-modifiers",
                &MODIFIERS,
                Some(this.doc.config.hyperkey.modifiers),
                this.read_only(),
                cx,
                |this, m, _, cx| {
                    this.doc.config.hyperkey.modifiers = m;
                    this.changed(cx);
                },
            )
        })
        .description("What other apps receive while CapsLock is held.")
        .disabled(ro);

        SettingPage::new("Hyperkey")
            .icon(IconName::ArrowUp)
            .group(
                SettingGroup::new()
                    .item(enable)
                    .item(legend)
                    .items(notes_item(notes)),
            )
            .group(
                SettingGroup::new()
                    .title("Modifiers")
                    .item(apps_see)
                    .items(notes_item(vec![Note::hint(modifiers_hint)])),
            )
    }
}
