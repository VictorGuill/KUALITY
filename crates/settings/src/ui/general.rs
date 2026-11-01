//! General tab: log level and where the config lives.

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use kuality_protocol::LogLevel;

use super::SettingsWindow;
use super::controls::{self, section, setting_row};

const LEVELS: [(LogLevel, &str); 5] = [
    (LogLevel::Error, "Error"),
    (LogLevel::Warn, "Warn"),
    (LogLevel::Info, "Info"),
    (LogLevel::Debug, "Debug"),
    (LogLevel::Trace, "Trace"),
];

impl SettingsWindow {
    pub(super) fn render_general(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ro = self.read_only();
        let level = controls::segmented(
            "log-level",
            &LEVELS,
            Some(self.doc.config.general.log_level),
            ro,
            cx,
            |this, level, _, cx| {
                this.doc.config.general.log_level = level;
                this.changed(cx);
            },
        );
        let path = self.doc.path().to_path_buf();
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        v_flex()
            .gap_4()
            .child(section(cx).child(setting_row(
                "Log level",
                Some(r"How much Kuality writes to logs\kuality.log.".into()),
                level,
                cx,
            )))
            .child(
                section(cx).child(
                    v_flex()
                        .gap_2()
                        .px_4()
                        .py_3()
                        .child(div().text_sm().font_medium().child("Config file"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(path.display().to_string()),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .pt_1()
                                .child(controls::open_config_button("open-config", self, cx))
                                .child(
                                    Button::new("open-folder")
                                        .small()
                                        .outline()
                                        .label("Open data folder")
                                        .on_click(move |_, _, cx| {
                                            let _ = std::fs::create_dir_all(&dir);
                                            cx.open_with_system(&dir);
                                        }),
                                ),
                        )
                        .child(controls::hint_text(
                            "Every change in this window is written to this file and applied by \
                             Kuality right away. You can also edit it by hand.",
                            cx,
                        )),
                ),
            )
    }
}
