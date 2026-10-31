//! Hyperkey tab.

use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, v_flex};
use gpui_kit::*;

use super::SettingsWindow;
use super::controls::{hint_text, section, setting_row};

impl SettingsWindow {
    pub(super) fn render_hyperkey(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let enabled = self.doc.config.hyperkey.enabled;
        let toggle = Switch::new("hyperkey-enabled")
            .accessibility_label("Enable Hyperkey")
            .checked(enabled)
            .disabled(self.read_only())
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.hyperkey.enabled = *on;
                this.changed(cx);
            }));
        v_flex().gap_4().child(
            section(cx)
                .child(setting_row(
                    "Enable Hyperkey",
                    Some("Hold CapsLock to press Ctrl+Shift+Alt+Win.".into()),
                    toggle,
                    cx,
                ))
                .child(v_flex().px_4().pb_3().gap_1().children([
                    hint_text(
                        "While enabled, CapsLock does not toggle caps and the Office key shortcut \
                         (Ctrl+Shift+Alt+Win alone) is disabled.",
                        cx,
                    ),
                    hint_text(
                        "Tray → Pause turns it off temporarily. Shortcuts (Hyper + key) need it.",
                        cx,
                    ),
                ])),
        )
    }
}
