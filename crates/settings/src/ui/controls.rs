//! Glue between the window and gpui-component: setting rows bound to the window's document,
//! and small builders shared by the tabs. Everything drawn here is a kit component; the only
//! exception is the Hyper keycap (a star the kit has no glyph for).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::form::Field;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::label::Label;
use gpui_kit::component::setting::{RenderOptions, SettingField, SettingItem};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use super::SettingsWindow;

/// The window, as captured by the closures of setting items (they outlive a render).
pub type Weak = WeakEntity<SettingsWindow>;

/// Shown under a field whose value in `config.json` is invalid.
pub const INVALID_IN_FILE: &str = "Invalid in config.json, Kuality uses the default";

/// A switch row for a setting of `config.json`. `set` is not called while the window is
/// read-only.
pub fn switch_item(
    title: impl Into<SharedString>,
    weak: &Weak,
    get: impl Fn(&SettingsWindow) -> bool + 'static,
    set: impl Fn(&mut SettingsWindow, bool, &mut Context<SettingsWindow>) + 'static,
) -> SettingItem {
    raw_switch_item(title, weak, get, move |this, on, cx| {
        if !this.read_only() {
            set(this, on, cx);
        }
    })
}

/// A switch row for something that is not in `config.json` (works while it is read-only).
pub fn raw_switch_item(
    title: impl Into<SharedString>,
    weak: &Weak,
    get: impl Fn(&SettingsWindow) -> bool + 'static,
    set: impl Fn(&mut SettingsWindow, bool, &mut Context<SettingsWindow>) + 'static,
) -> SettingItem {
    let (reader, writer) = (weak.clone(), weak.clone());
    SettingItem::new(
        title,
        SettingField::switch(
            move |cx: &App| reader.upgrade().is_some_and(|w| get(w.read(cx))),
            move |on: bool, cx: &mut App| {
                let _ = writer.update(cx, |this, cx| set(this, on, cx));
            },
        ),
    )
}

/// A dropdown row choosing one of `options` (value, label). `get` returning `None` (an invalid
/// value in the file) shows no choice.
pub fn dropdown_item<T: Copy + PartialEq + 'static>(
    title: impl Into<SharedString>,
    weak: &Weak,
    options: &[(T, &'static str)],
    get: impl Fn(&SettingsWindow) -> Option<T> + 'static,
    set: impl Fn(&mut SettingsWindow, T, &mut Context<SettingsWindow>) + 'static,
) -> SettingItem {
    let options: Vec<(T, SharedString)> = options.iter().map(|&(v, l)| (v, l.into())).collect();
    let labels = options
        .iter()
        .map(|(_, l)| (l.clone(), l.clone()))
        .collect();
    let (reader, writer) = (weak.clone(), weak.clone());
    let (for_get, for_set) = (options.clone(), options);
    SettingItem::new(
        title,
        SettingField::dropdown(
            labels,
            move |cx: &App| {
                reader
                    .upgrade()
                    .and_then(|w| get(w.read(cx)))
                    .and_then(|v| for_get.iter().find(|(o, _)| *o == v))
                    .map(|(_, label)| label.clone())
                    .unwrap_or_default()
            },
            move |label: SharedString, cx: &mut App| {
                let Some(&(value, _)) = for_set.iter().find(|(_, l)| *l == label) else {
                    return;
                };
                let _ = writer.update(cx, |this, cx| {
                    if !this.read_only() {
                        set(this, value, cx);
                    }
                });
            },
        ),
    )
}

/// A row whose control (buttons, a segmented choice, a recorder…) is drawn by the window.
pub fn control_item<E: IntoElement>(
    title: impl Into<SharedString>,
    weak: &Weak,
    render: impl Fn(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>) -> E + 'static,
) -> SettingItem {
    let weak = weak.clone();
    SettingItem::new(
        title,
        SettingField::render(
            move |_: &RenderOptions, window: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| render(this, window, cx).into_any_element())
                    .unwrap_or_else(|_| Empty.into_any_element())
            },
        ),
    )
}

/// A full-width block drawn by the window (a list, a table, an editor).
pub fn block_item<E: IntoElement>(
    weak: &Weak,
    render: impl Fn(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>) -> E + 'static,
) -> SettingItem {
    let weak = weak.clone();
    SettingItem::render(
        move |_: &RenderOptions, window: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| render(this, window, cx).into_any_element())
                .unwrap_or_else(|_| Empty.into_any_element())
        },
    )
}

/// One line of explanation under a setting.
#[derive(Debug, Clone)]
pub enum Note {
    Hint(SharedString),
    Warning(SharedString),
    Error(SharedString),
}

impl Note {
    pub fn hint(text: impl Into<SharedString>) -> Self {
        Self::Hint(text.into())
    }

    pub fn warning(text: impl Into<SharedString>) -> Self {
        Self::Warning(text.into())
    }

    pub fn error(text: impl Into<SharedString>) -> Self {
        Self::Error(text.into())
    }

    pub fn render(&self, cx: &App) -> Label {
        match self {
            Self::Hint(t) => hint_text(t.clone(), cx),
            Self::Warning(t) => warning_text(t.clone(), cx),
            Self::Error(t) => error_text(t.clone(), cx),
        }
    }
}

/// A full-width block of notes; `None` when there are none.
pub fn notes_item(notes: Vec<Note>) -> Option<SettingItem> {
    (!notes.is_empty()).then(|| {
        SettingItem::render(move |_: &RenderOptions, _: &mut Window, cx: &mut App| {
            v_flex()
                .gap_1()
                .children(notes.iter().map(|n| n.render(cx)))
        })
    })
}

pub fn error_text(text: impl Into<SharedString>, cx: &App) -> Label {
    Label::new(text).text_xs().text_color(cx.theme().danger)
}

pub fn warning_text(text: impl Into<SharedString>, cx: &App) -> Label {
    Label::new(text).text_xs().text_color(cx.theme().warning)
}

pub fn hint_text(text: impl Into<SharedString>, cx: &App) -> Label {
    Label::new(text)
        .text_xs()
        .text_color(cx.theme().muted_foreground)
}

/// A labelled form field (for a kit `Form`) with an optional note below it.
pub fn field(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    note: Option<Note>,
) -> Field {
    let field = Field::new().label(label.into()).child(control);
    match note {
        Some(note) => field.description_fn(move |_, cx| note.render(cx)),
        None => field,
    }
}

/// A segmented choice between a few values. `current == None` selects nothing (invalid value).
pub fn segmented<T: Copy + PartialEq + 'static>(
    id: impl Into<ElementId>,
    options: &[(T, &'static str)],
    current: Option<T>,
    disabled: bool,
    cx: &mut Context<SettingsWindow>,
    on_pick: impl Fn(&mut SettingsWindow, T, &mut Window, &mut Context<SettingsWindow>) + 'static,
) -> Div {
    let values: Vec<T> = options.iter().map(|&(v, _)| v).collect();
    let selected = current.and_then(|c| values.iter().position(|&v| v == c));
    let bar = TabBar::new(id)
        .segmented()
        .small()
        .children(
            options
                .iter()
                .map(|&(_, label)| Tab::new().label(label).disabled(disabled)),
        )
        .on_click(cx.listener(move |this, ix: &usize, window, cx| {
            if let Some(&value) = values.get(*ix)
                && !this.read_only()
            {
                on_pick(this, value, window, cx);
            }
        }));
    // In a row of its own so it keeps its natural width instead of stretching.
    h_flex().child(match selected {
        Some(ix) => bar.selected_index(ix),
        None => bar,
    })
}

/// "Open config file": creates the file first if it does not exist yet.
/// `id` must be unique in the window: the banner and the General tab can both show one.
pub fn open_config_button(id: &'static str, this: &SettingsWindow) -> Button {
    let path = this.doc.path().to_path_buf();
    Button::new(id)
        .small()
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

/// A warning with the button that fixes it ("… is off" + "Turn on").
pub fn fix_row(warning: impl Into<SharedString>, button: Button, cx: &App) -> Div {
    h_flex()
        .gap_3()
        .items_center()
        .child(warning_text(warning, cx))
        .child(button.small())
}

/// "Hyper … only works while Hyperkey is enabled", with the button that enables it.
pub fn hyper_off_row(
    id: &'static str,
    warning: &'static str,
    cx: &mut Context<SettingsWindow>,
) -> Div {
    let read_only = cx.entity().read(cx).read_only();
    let button = Button::new(id)
        .label("Enable Hyperkey")
        .disabled(read_only)
        .on_click(cx.listener(|this, _, _, cx| {
            this.doc.config.hyperkey.enabled = true;
            this.changed(cx);
        }));
    h_flex()
        .gap_3()
        .items_center()
        .child(hyper_keycap(cx))
        .child(warning_text(warning, cx))
        .child(button.small())
}

/// Width of a hotkey recorder button.
pub const HOTKEY_WIDTH: f32 = 160.;

/// The button that shows a hotkey as keycaps and records a new one when clicked (the caller
/// adds the click handler). While `recording` it reads "Press a hotkey…".
pub fn hotkey_button(
    id: impl Into<ElementId>,
    key: &str,
    recording: bool,
    accessibility_label: impl Into<SharedString>,
    cx: &App,
) -> Button {
    use crate::model;
    let button = Button::new(id)
        .w(px(HOTKEY_WIDTH))
        .flex_none()
        .accessibility_label(accessibility_label);
    let button = if recording {
        button.primary()
    } else {
        button.outline()
    };
    let button = match model::hotkey_label(key).filter(|_| !recording) {
        Some(label) => button.tooltip(label),
        None => button,
    };
    match (!recording).then(|| model::hotkey_caps(key)).flatten() {
        Some(caps) => button.child(keycaps(&caps, cx)),
        None => button
            .icon(gpui_kit::component::Icon::new(
                gpui_kit::assets::IconName::Keyboard,
            ))
            .label(if recording {
                "Press a hotkey…"
            } else {
                "Record hotkey"
            }),
    }
}

/// The Hyper glyph: a four-pointed star, drawn from a path (no font glyph needed).
pub const HYPER_STAR_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" d="M12 1C12.8 7.4 16.6 11.2 23 12C16.6 12.8 12.8 16.6 12 23C11.2 16.6 7.4 12.8 1 12C7.4 11.2 11.2 7.4 12 1Z"/></svg>"#;

/// The Hyper keycap: a white star on a dark key, in light and dark mode alike.
pub fn hyper_keycap(cx: &App) -> Div {
    h_flex()
        .flex_none()
        .size_5()
        .justify_center()
        .items_center()
        .rounded(cx.theme().radius / 2.)
        .bg(rgb(0x3A3A3C))
        .child(
            gpui_kit::component::Icon::default()
                .data(HYPER_STAR_SVG)
                .size_3()
                .text_color(white()),
        )
}

/// One keycap with text (`Ctrl`, `T`, `Page Up`).
pub fn text_keycap(text: impl Into<String>) -> Kbd {
    Kbd::new(Keystroke {
        modifiers: Modifiers::default(),
        key: text.into(),
        key_char: None,
    })
}

/// A hotkey as keycaps joined by `+`: `[✦] + [1]`, `[Ctrl] + [T]`.
pub fn keycaps(caps: &[crate::model::Keycap], cx: &App) -> Div {
    let mut row = h_flex().gap_1().items_center().min_w_0();
    for (i, cap) in caps.iter().enumerate() {
        if i > 0 {
            row = row.child(hint_text("+", cx));
        }
        row = match cap {
            crate::model::Keycap::Hyper => row.child(hyper_keycap(cx)),
            crate::model::Keycap::Text(t) => row.child(text_keycap(t.to_string())),
        };
    }
    row
}
