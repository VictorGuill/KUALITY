//! Small layout helpers shared by the tabs.

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::SettingsWindow;

/// Shown under a field whose value in `config.json` is invalid.
pub const INVALID_IN_FILE: &str = "Invalid in config.json, Kuality uses the default";

/// A bordered group of settings.
pub fn section(cx: &App) -> Div {
    v_flex()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().secondary.opacity(0.35))
}

/// One line of a [`section`]: label and description on the left, control on the right.
pub fn setting_row(
    label: impl Into<SharedString>,
    description: Option<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> Div {
    h_flex()
        .w_full()
        .gap_4()
        .px_4()
        .py_3()
        .justify_between()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().font_medium().child(label.into()))
                .children(description.map(|d| {
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(d)
                })),
        )
        .child(control)
}

/// A labelled field with an optional error or warning line below.
pub fn field(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    error: Option<&str>,
    cx: &App,
) -> Div {
    v_flex()
        .gap_1()
        .min_w_0()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(label.into()),
        )
        .child(control)
        .children(error.map(|e| error_text(e.to_string(), cx)))
}

pub fn error_text(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().danger)
        .child(text.into())
}

pub fn warning_text(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().warning)
        .child(text.into())
}

pub fn hint_text(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A row of mutually exclusive buttons. `current == None` selects nothing (invalid value).
pub fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, &'static str)],
    current: Option<T>,
    disabled: bool,
    cx: &mut Context<SettingsWindow>,
    on_pick: impl Fn(&mut SettingsWindow, T, &mut Window, &mut Context<SettingsWindow>) + 'static,
) -> ButtonGroup {
    let values: Vec<T> = options.iter().map(|(v, _)| *v).collect();
    ButtonGroup::new(id)
        .small()
        .disabled(disabled)
        .children(options.iter().enumerate().map(|(i, (v, label))| {
            let selected = current == Some(*v);
            Button::new((id, i))
                .label(*label)
                .when(selected, |b| b.primary())
                .when(!selected, |b| b.outline())
                .selected(selected)
        }))
        .on_click(cx.listener(move |this, picked: &Vec<usize>, window, cx| {
            if this.read_only() {
                return;
            }
            if let Some(&v) = picked.first().and_then(|&i| values.get(i)) {
                on_pick(this, v, window, cx);
            }
        }))
}

/// "Open config file": creates the file first if it does not exist yet.
/// `id` must be unique in the window: the banner and the General tab can both show one.
pub fn open_config_button(
    id: &'static str,
    this: &SettingsWindow,
    _cx: &mut Context<SettingsWindow>,
) -> Button {
    let path = this.doc.path().to_path_buf();
    Button::new(id)
        .small()
        .outline()
        .label("Open config file")
        .on_click(move |_, _, cx| {
            if !path.exists() {
                let mut doc = crate::model::Document::load(path.clone());
                if let Err(e) = doc.save() {
                    log::error!("cannot create {}: {e}", path.display());
                }
            }
            cx.open_with_system(&path);
        })
}

/// A primary "Add …" button.
pub fn add_button(
    id: &'static str,
    label: &'static str,
    disabled: bool,
    cx: &mut Context<SettingsWindow>,
    on_click: impl Fn(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>) + 'static,
) -> Button {
    Button::new(id)
        .small()
        .primary()
        .icon(gpui_kit::component::IconName::Plus)
        .label(label)
        .disabled(disabled)
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
}

/// Header line of a list: title on the left, buttons on the right.
pub fn list_header(title: &'static str, right: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .justify_between()
        .items_center()
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(title),
        )
        .child(right)
}

/// Visual separator line inside a section.
pub fn divider(cx: &App) -> Div {
    div().h(px(1.)).w_full().bg(cx.theme().border)
}

/// The Hyper glyph: a four-pointed star, drawn from a path (no font glyph needed).
pub const HYPER_STAR_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" d="M12 1C12.8 7.4 16.6 11.2 23 12C16.6 12.8 12.8 16.6 12 23C11.2 16.6 7.4 12.8 1 12C7.4 11.2 11.2 7.4 12 1Z"/></svg>"#;

/// Keycap height.
const KEYCAP_H: f32 = 20.;

/// The Hyper keycap: a white star on a dark key, in light and dark mode alike.
pub fn hyper_keycap() -> Div {
    h_flex()
        .flex_none()
        .h(px(KEYCAP_H))
        .w(px(KEYCAP_H + 2.))
        .justify_center()
        .items_center()
        .rounded(px(4.))
        .bg(rgb(0x3A3A3C))
        .child(
            gpui_kit::component::Icon::default()
                .data(HYPER_STAR_SVG)
                .size(px(12.))
                .text_color(white()),
        )
}

/// One keycap with text (`Ctrl`, `T`, `Page Up`).
pub fn text_keycap(text: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .h(px(KEYCAP_H))
        .min_w(px(KEYCAP_H + 2.))
        .px_1p5()
        .justify_center()
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().secondary)
        .text_xs()
        .font_medium()
        .text_color(cx.theme().foreground)
        .child(text.into())
}

/// A hotkey as keycaps joined by `+`: `[✦] + [1]`, `[Ctrl] + [T]`.
pub fn keycaps(caps: &[crate::model::Keycap], cx: &App) -> Div {
    let mut row = h_flex().gap_1().items_center().min_w_0();
    for (i, cap) in caps.iter().enumerate() {
        if i > 0 {
            row = row.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("+"),
            );
        }
        row = row.child(match cap {
            crate::model::Keycap::Hyper => hyper_keycap(),
            crate::model::Keycap::Text(t) => text_keycap(t.clone(), cx),
        });
    }
    row
}
