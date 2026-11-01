//! Hyperkey tab.

use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, h_flex, v_flex};
use gpui_kit::*;

use super::SettingsWindow;
use super::controls::{divider, hint_text, hyper_keycap, section, setting_row, text_keycap};

impl SettingsWindow {
    pub(super) fn render_hyperkey(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let h = &self.doc.config.hyperkey;
        let (enabled, include_win) = (h.enabled, h.include_win);
        let ro = self.read_only();
        let toggle = Switch::new("hyperkey-enabled")
            .accessibility_label("Enable Hyperkey")
            .checked(enabled)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.hyperkey.enabled = *on;
                this.changed(cx);
            }));
        let win_toggle = Switch::new("hyperkey-include-win")
            .accessibility_label("Include Windows key")
            .checked(include_win)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.hyperkey.include_win = *on;
                this.changed(cx);
            }));
        let combo = if include_win {
            "Ctrl+Shift+Alt+Win"
        } else {
            "Ctrl+Shift+Alt"
        };
        // `[✦] = [Ctrl] [Shift] [Alt] [Win]`: what holding CapsLock presses.
        let mut legend = h_flex()
            .gap_1()
            .items_center()
            .child(text_keycap("CapsLock", cx))
            .child(hint_text("→", cx))
            .child(hyper_keycap())
            .child(hint_text("=", cx));
        for k in ["Ctrl", "Shift", "Alt"]
            .into_iter()
            .chain(include_win.then_some("Win"))
        {
            legend = legend.child(text_keycap(k, cx));
        }
        let mut notes = vec![hint_text(
            "While enabled, CapsLock does not toggle caps. Hyper is shown as the star key.",
            cx,
        )];
        if include_win {
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
        v_flex().gap_4().child(
            section(cx)
                .child(setting_row(
                    "Enable Hyperkey",
                    Some(format!("Hold CapsLock to press {combo}.").into()),
                    toggle,
                    cx,
                ))
                .child(v_flex().px_4().pb_3().gap_2().child(legend).children(notes))
                .child(divider(cx))
                .child(setting_row(
                    "Include Windows key",
                    Some("On: Hyper is Ctrl+Shift+Alt+Win. Off: Ctrl+Shift+Alt.".into()),
                    win_toggle,
                    cx,
                ))
                .child(v_flex().px_4().pb_3().child(hint_text(
                    "On some keyboard layouts (e.g. Spanish) Ctrl+Alt acts as AltGr, so some keys \
                     may type a character when Win is excluded.",
                    cx,
                ))),
        )
    }
}
