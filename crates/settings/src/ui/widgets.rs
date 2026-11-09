//! Widgets tab: the two switches and the widget list (text widgets and bars) with an inline
//! editor shared by both types.

use std::collections::BTreeMap;

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{
    Input, InputEvent, InputState, NumberInput, Textarea, TextareaState,
};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::{
    Anchor, BarModule, BarModuleEntry, Color, CommonItem, Dimension, FontWeight, Lenient, Origin,
    TextAlign, TextWidget, TodoModule, WidgetEntry, WidgetItem,
};

use super::SettingsWindow;
use super::controls::{
    self, INVALID_IN_FILE, add_button, divider, field, hint_text, list_header, section,
    setting_row, warning_text,
};
use crate::model::{self, Unit};

const ANCHORS: [(Anchor, &str); 3] = [
    (Anchor::Left, "Left"),
    (Anchor::Center, "Center"),
    (Anchor::Right, "Right"),
];
const ORIGIN_ROWS: [[(Origin, &str); 3]; 3] = [
    [
        (Origin::TopLeft, "Top left"),
        (Origin::Top, "Top"),
        (Origin::TopRight, "Top right"),
    ],
    [
        (Origin::Left, "Left"),
        (Origin::Center, "Center"),
        (Origin::Right, "Right"),
    ],
    [
        (Origin::BottomLeft, "Bottom left"),
        (Origin::Bottom, "Bottom"),
        (Origin::BottomRight, "Bottom right"),
    ],
];
const WEIGHTS: [(FontWeight, &str); 3] = [
    (FontWeight::Normal, "Normal"),
    (FontWeight::Semibold, "Semibold"),
    (FontWeight::Bold, "Bold"),
];
const ALIGNS: [(TextAlign, &str); 3] = [
    (TextAlign::Left, "Left"),
    (TextAlign::Center, "Center"),
    (TextAlign::Right, "Right"),
];
const UNITS: [(Unit, &str); 2] = [(Unit::Px, "px"), (Unit::Pct, "%")];

/// Text fields of the widget editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WField {
    Id,
    Text,
    X,
    Y,
    Width,
    Height,
    FontFamily,
    FontSize,
    Color,
    Anchor,
    Origin,
    Weight,
    Align,
    Gap,
}

/// What the editor shows, copied from a text widget or a bar.
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    common: CommonItem,
    font_family: String,
    font_size: Lenient<f64>,
    font_weight: Lenient<FontWeight>,
    color: Lenient<Option<Color>>,
    kind: SnapKind,
}

#[derive(Debug, Clone)]
enum SnapKind {
    Text {
        text: String,
        align: Lenient<TextAlign>,
    },
    Bar {
        gap: Lenient<f64>,
        modules: Vec<BarModuleEntry>,
    },
}

fn snapshot(item: &WidgetItem) -> Snapshot {
    match item {
        WidgetItem::Text(t) => Snapshot {
            common: t.common.clone(),
            font_family: t.font_family.clone(),
            font_size: t.font_size.clone(),
            font_weight: t.font_weight.clone(),
            color: t.color.clone(),
            kind: SnapKind::Text {
                text: t.text.clone(),
                align: t.align.clone(),
            },
        },
        WidgetItem::Bar(b) => Snapshot {
            common: b.common.clone(),
            font_family: b.font_family.clone(),
            font_size: b.font_size.clone(),
            font_weight: b.font_weight.clone(),
            color: b.color.clone(),
            kind: SnapKind::Bar {
                gap: b.gap.clone(),
                modules: b.modules.clone(),
            },
        },
    }
}

/// The editable fields of a text widget or a bar (type-specific ones are `None` for the other).
struct StyleMut<'a> {
    common: &'a mut CommonItem,
    font_family: &'a mut String,
    font_size: &'a mut Lenient<f64>,
    font_weight: &'a mut Lenient<FontWeight>,
    color: &'a mut Lenient<Option<Color>>,
    text: Option<&'a mut String>,
    align: Option<&'a mut Lenient<TextAlign>>,
    gap: Option<&'a mut Lenient<f64>>,
}

fn style_mut(item: &mut WidgetItem) -> StyleMut<'_> {
    match item {
        WidgetItem::Text(t) => StyleMut {
            common: &mut t.common,
            font_family: &mut t.font_family,
            font_size: &mut t.font_size,
            font_weight: &mut t.font_weight,
            color: &mut t.color,
            text: Some(&mut t.text),
            align: Some(&mut t.align),
            gap: None,
        },
        WidgetItem::Bar(b) => StyleMut {
            common: &mut b.common,
            font_family: &mut b.font_family,
            font_size: &mut b.font_size,
            font_weight: &mut b.font_weight,
            color: &mut b.color,
            text: None,
            align: None,
            gap: Some(&mut b.gap),
        },
    }
}

/// Label of a bar module type in the UI.
fn module_label(m: &BarModuleEntry) -> String {
    match m {
        BarModuleEntry::Known(BarModule::Todo(_)) => "To do".into(),
        BarModuleEntry::Unknown(v) => format!("\"{}\" (unsupported, kept)", model::unknown_type(v)),
    }
}

/// Inputs of the expanded text widget.
pub(crate) struct WidgetEditor {
    pub index: usize,
    /// Text widgets only.
    text: Option<Entity<TextareaState>>,
    /// Bars only.
    gap: Option<Entity<InputState>>,
    id: Entity<InputState>,
    x: Entity<InputState>,
    y: Entity<InputState>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    font_family: Entity<InputState>,
    font_size: Entity<InputState>,
    color: Entity<InputState>,
    x_unit: Unit,
    y_unit: Unit,
    width_unit: Unit,
    height_unit: Unit,
    /// Messages per field: input errors and invalid values read from the file.
    errors: BTreeMap<WField, String>,
    _subs: Vec<Subscription>,
}

impl WidgetEditor {
    pub fn error_count(&self) -> usize {
        self.errors
            .values()
            .filter(|e| e.as_str() != INVALID_IN_FILE)
            .count()
    }
}

/// Raw JSON of an invalid value, as text for its input.
fn raw_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn length_text(l: &Lenient<kuality_protocol::Length>) -> (String, Unit, bool) {
    match l {
        Lenient::Valid(l) => (
            model::format_number(model::length_value(*l)),
            Unit::of(*l),
            true,
        ),
        Lenient::Invalid(v) => (raw_text(v), Unit::Px, false),
    }
}

fn dimension_text(d: &Lenient<Dimension>) -> (String, Unit, bool) {
    match d {
        Lenient::Valid(Dimension::Auto) => (String::new(), Unit::Px, true),
        Lenient::Valid(Dimension::Length(l)) => (
            model::format_number(model::length_value(*l)),
            Unit::of(*l),
            true,
        ),
        Lenient::Invalid(v) => (raw_text(v), Unit::Px, false),
    }
}

fn color_hex(c: Color) -> String {
    if c.a == 255 {
        format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
    } else {
        format!("#{:02X}{:02X}{:02X}{:02X}", c.a, c.r, c.g, c.b)
    }
}

impl SettingsWindow {
    fn widget_item_mut(&mut self, index: usize) -> Option<&mut WidgetItem> {
        match self.doc.config.widgets.items.get_mut(index) {
            Some(WidgetEntry::Known(item)) => Some(item),
            _ => None,
        }
    }

    fn style_mut(&mut self, index: usize) -> Option<StyleMut<'_>> {
        self.widget_item_mut(index).map(style_mut)
    }

    /// Expands widget `index` (collapses it if it is already open).
    pub(super) fn open_widget(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(WidgetEntry::Known(item)) = self.doc.config.widgets.items.get(index) else {
            self.widget_editor = None;
            return;
        };
        let t = snapshot(item);
        let mut errors = BTreeMap::new();
        let mut mark = |f: WField, ok: bool| {
            if !ok {
                errors.insert(f, INVALID_IN_FILE.to_string());
            }
        };
        let (x, x_unit, ok) = length_text(&t.common.x);
        mark(WField::X, ok);
        let (y, y_unit, ok) = length_text(&t.common.y);
        mark(WField::Y, ok);
        let (w, width_unit, ok) = dimension_text(&t.common.width);
        mark(WField::Width, ok);
        let (h, height_unit, ok) = dimension_text(&t.common.height);
        mark(WField::Height, ok);
        let font_size = match &t.font_size {
            Lenient::Valid(v) => model::format_number(*v),
            Lenient::Invalid(v) => {
                mark(WField::FontSize, false);
                raw_text(v)
            }
        };
        let color = match &t.color {
            Lenient::Valid(None) => String::new(),
            Lenient::Valid(Some(c)) => color_hex(*c),
            Lenient::Invalid(v) => {
                mark(WField::Color, false);
                raw_text(v)
            }
        };
        mark(WField::Anchor, t.common.anchor.valid().is_some());
        mark(WField::Origin, t.common.origin.valid().is_some());
        mark(WField::Weight, t.font_weight.valid().is_some());
        let gap_text = match &t.kind {
            SnapKind::Text { align, .. } => {
                mark(WField::Align, align.valid().is_some());
                None
            }
            SnapKind::Bar { gap, .. } => Some(match gap {
                Lenient::Valid(v) => model::format_number(*v),
                Lenient::Invalid(v) => {
                    mark(WField::Gap, false);
                    raw_text(v)
                }
            }),
        };

        let line = |value: String,
                    placeholder: &'static str,
                    window: &mut Window,
                    cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let number = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).step(1.0).default_value(value))
        };
        let text = match &t.kind {
            SnapKind::Text { text, .. } => Some(cx.new(|cx| {
                let mut s = TextareaState::new(window, cx).default_value(text.clone());
                s.set_auto_grow(2, 6, cx);
                s
            })),
            SnapKind::Bar { .. } => None,
        };
        let gap = gap_text.map(|g| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .step(1.0)
                    .min(model::GAP_RANGE.0)
                    .max(model::GAP_RANGE.1)
                    .default_value(g)
            })
        });
        let id = line(t.common.id.clone(), "#index", window, cx);
        let x = number(x, window, cx);
        let y = number(y, window, cx);
        let width = number(w, window, cx);
        let height = number(h, window, cx);
        let font_family = line(t.font_family.clone(), "Segoe UI Variable Text", window, cx);
        let font_size = cx.new(|cx| {
            InputState::new(window, cx)
                .step(1.0)
                .min(model::FONT_SIZE_RANGE.0)
                .max(model::FONT_SIZE_RANGE.1)
                .default_value(font_size)
        });
        let color = line(color, "#RRGGBB", window, cx);

        let mut subs = Vec::new();
        if let Some(text) = &text {
            subs.push(
                cx.subscribe_in(text, window, |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.widget_field_changed(WField::Text, window, cx);
                    }
                }),
            );
        }
        if let Some(gap) = &gap {
            subs.push(
                cx.subscribe_in(gap, window, |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.widget_field_changed(WField::Gap, window, cx);
                    }
                }),
            );
        }
        for (input, f) in [
            (&id, WField::Id),
            (&x, WField::X),
            (&y, WField::Y),
            (&width, WField::Width),
            (&height, WField::Height),
            (&font_family, WField::FontFamily),
            (&font_size, WField::FontSize),
            (&color, WField::Color),
        ] {
            subs.push(cx.subscribe_in(
                input,
                window,
                move |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.widget_field_changed(f, window, cx);
                    }
                },
            ));
        }
        self.widget_editor = Some(WidgetEditor {
            index,
            text,
            gap,
            id,
            x,
            y,
            width,
            height,
            font_family,
            font_size,
            color,
            x_unit,
            y_unit,
            width_unit,
            height_unit,
            errors,
            _subs: subs,
        });
        cx.notify();
    }

    /// Reads the changed input, validates it and stores it if valid.
    fn widget_field_changed(&mut self, f: WField, _window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only() {
            return;
        }
        let Some(ed) = self.widget_editor.as_ref() else {
            return;
        };
        let index = ed.index;
        let value = |e: &Entity<InputState>| e.read(cx).value().to_string();
        let text = match f {
            WField::Text => match &ed.text {
                Some(t) => t.read(cx).value().to_string(),
                None => return,
            },
            WField::Gap => match &ed.gap {
                Some(g) => value(g),
                None => return,
            },
            WField::Id => value(&ed.id),
            WField::X => value(&ed.x),
            WField::Y => value(&ed.y),
            WField::Width => value(&ed.width),
            WField::Height => value(&ed.height),
            WField::FontFamily => value(&ed.font_family),
            WField::FontSize => value(&ed.font_size),
            WField::Color => value(&ed.color),
            _ => return,
        };
        let (x_unit, y_unit, w_unit, h_unit) =
            (ed.x_unit, ed.y_unit, ed.width_unit, ed.height_unit);
        let Some(t) = self.style_mut(index) else {
            return;
        };
        let result: Result<(), String> = match f {
            WField::Text => {
                if let Some(s) = t.text {
                    *s = text;
                }
                Ok(())
            }
            WField::Gap => model::parse_gap(&text)
                .map(|v| {
                    if let Some(g) = t.gap {
                        *g = Lenient::Valid(v);
                    }
                })
                .map_err(Into::into),
            WField::Id => {
                t.common.id = text;
                Ok(())
            }
            WField::X => model::parse_number(&text)
                .map(|v| t.common.x = Lenient::Valid(x_unit.make(v)))
                .map_err(Into::into),
            WField::Y => model::parse_number(&text)
                .map(|v| t.common.y = Lenient::Valid(y_unit.make(v)))
                .map_err(Into::into),
            WField::Width => {
                if matches!(t.common.width, Lenient::Valid(Dimension::Auto)) {
                    Ok(())
                } else {
                    model::parse_size(&text)
                        .map(|v| t.common.width = Lenient::Valid(Dimension::Length(w_unit.make(v))))
                        .map_err(Into::into)
                }
            }
            WField::Height => {
                if matches!(t.common.height, Lenient::Valid(Dimension::Auto)) {
                    Ok(())
                } else {
                    model::parse_size(&text)
                        .map(|v| {
                            t.common.height = Lenient::Valid(Dimension::Length(h_unit.make(v)))
                        })
                        .map_err(Into::into)
                }
            }
            WField::FontFamily => model::validate_font_family(&text)
                .map(|()| *t.font_family = text.trim().to_string())
                .map_err(Into::into),
            WField::FontSize => model::parse_font_size(&text)
                .map(|v| *t.font_size = Lenient::Valid(v))
                .map_err(Into::into),
            WField::Color => {
                if matches!(*t.color, Lenient::Valid(None)) {
                    Ok(())
                } else {
                    model::parse_color(&text)
                        .map(|c| *t.color = Lenient::Valid(Some(c)))
                        .map_err(Into::into)
                }
            }
            _ => Ok(()),
        };
        let ok = result.is_ok();
        if let Some(ed) = self.widget_editor.as_mut() {
            match result {
                Ok(()) => {
                    ed.errors.remove(&f);
                }
                Err(e) => {
                    ed.errors.insert(f, e);
                }
            }
        }
        if ok {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }

    /// A choice (segmented control or checkbox) changed a widget field directly.
    fn widget_choice(&mut self, f: WField, cx: &mut Context<Self>, set: impl FnOnce(StyleMut<'_>)) {
        let Some(index) = self.widget_editor.as_ref().map(|e| e.index) else {
            return;
        };
        if let Some(t) = self.style_mut(index) {
            set(t);
        }
        if let Some(ed) = self.widget_editor.as_mut() {
            ed.errors.remove(&f);
        }
        self.changed(cx);
    }

    fn set_unit(&mut self, f: WField, unit: Unit, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ed) = self.widget_editor.as_mut() {
            match f {
                WField::X => ed.x_unit = unit,
                WField::Y => ed.y_unit = unit,
                WField::Width => ed.width_unit = unit,
                WField::Height => ed.height_unit = unit,
                _ => {}
            }
        }
        self.widget_field_changed(f, window, cx);
    }

    fn set_auto(&mut self, f: WField, auto: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.widget_editor.as_ref() else {
            return;
        };
        let index = ed.index;
        let input = if f == WField::Width {
            ed.width.clone()
        } else {
            ed.height.clone()
        };
        if let Some(t) = self.style_mut(index) {
            let dim = if f == WField::Width {
                &mut t.common.width
            } else {
                &mut t.common.height
            };
            if auto {
                *dim = Lenient::Valid(Dimension::Auto);
            } else {
                // Leave Auto with a placeholder value the field then validates.
                *dim = Lenient::Valid(Dimension::Length(kuality_protocol::Length::Px(0.0)));
            }
        }
        if let Some(ed) = self.widget_editor.as_mut() {
            ed.errors.remove(&f);
        }
        if !auto && model::parse_size(&input.read(cx).value()).is_err() {
            input.update(cx, |s, cx| s.set_value("100", window, cx));
        }
        if auto {
            self.changed(cx);
        } else {
            self.widget_field_changed(f, window, cx);
        }
    }

    fn set_theme_color(&mut self, theme: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.widget_editor.as_ref() else {
            return;
        };
        let (index, input) = (ed.index, ed.color.clone());
        if let Some(t) = self.style_mut(index) {
            *t.color = if theme {
                Lenient::Valid(None)
            } else {
                Lenient::Valid(Some(Color {
                    a: 255,
                    r: 255,
                    g: 255,
                    b: 255,
                }))
            };
        }
        if let Some(ed) = self.widget_editor.as_mut() {
            ed.errors.remove(&WField::Color);
        }
        if !theme && model::parse_color(&input.read(cx).value()).is_err() {
            input.update(cx, |s, cx| s.set_value("#FFFFFF", window, cx));
        }
        if theme {
            self.changed(cx);
        } else {
            self.widget_field_changed(WField::Color, window, cx);
        }
    }

    fn add_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = &mut self.doc.config.widgets.items;
        let widget = TextWidget {
            common: CommonItem {
                id: model::next_widget_id(items, "text"),
                // Off until the user turns it on: a placeholder "Hello" should not pop up on
                // the taskbar the moment the item is created.
                enabled: false,
                ..CommonItem::default()
            },
            text: "Hello".into(),
            ..TextWidget::default()
        };
        items.push(WidgetEntry::Known(WidgetItem::Text(widget)));
        let index = items.len() - 1;
        self.changed(cx);
        self.open_widget(index, window, cx);
    }

    /// Bar editor: shows or removes the to-do module.
    fn set_bar_todo(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(index) = self.widget_editor.as_ref().map(|e| e.index) else {
            return;
        };
        let Some(WidgetItem::Bar(b)) = self.widget_item_mut(index) else {
            return;
        };
        let has = b.todo_module().is_some();
        if on && !has {
            b.modules.push(BarModuleEntry::Known(
                BarModule::Todo(TodoModule::default()),
            ));
        } else if !on && has {
            b.modules
                .retain(|m| !matches!(m, BarModuleEntry::Known(BarModule::Todo(_))));
        } else {
            return;
        }
        self.changed(cx);
    }

    fn confirm_delete_widget(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("Delete this widget?")
                .description("It is removed from config.json right away.")
                .ok_text("Delete")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.delete_widget(index, cx));
                    true
                })
        });
    }

    fn delete_widget(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.read_only() || index >= self.doc.config.widgets.items.len() {
            return;
        }
        self.doc.config.widgets.items.remove(index);
        match self.widget_editor.as_ref().map(|e| e.index) {
            Some(i) if i == index => self.widget_editor = None,
            Some(i) if i > index => {
                if let Some(ed) = self.widget_editor.as_mut() {
                    ed.index -= 1;
                }
            }
            _ => {}
        }
        self.changed(cx);
    }

    pub(super) fn render_widgets(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ro = self.read_only();
        let w = &self.doc.config.widgets;
        let enabled = Switch::new("widgets-enabled")
            .accessibility_label("Show taskbar widgets")
            .checked(w.enabled)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.widgets.enabled = *on;
                this.changed(cx);
            }));
        let fullscreen = Switch::new("widgets-fullscreen")
            .accessibility_label("Hide when a fullscreen app is in front")
            .checked(w.hide_on_fullscreen)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.widgets.hide_on_fullscreen = *on;
                this.changed(cx);
            }));
        let section_off = !w.enabled;
        let count = w.items.len();
        let mut rows: Vec<AnyElement> = Vec::new();
        for index in 0..count {
            rows.push(self.render_widget_row(index, window, cx));
        }
        v_flex()
            .gap_4()
            .child(
                section(cx)
                    .child(setting_row(
                        "Show taskbar widgets",
                        Some("Draw the widgets below on top of the Windows taskbar.".into()),
                        enabled,
                        cx,
                    ))
                    .child(divider(cx))
                    .child(setting_row(
                        "Hide when a fullscreen app is in front",
                        None,
                        fullscreen,
                        cx,
                    )),
            )
            .child(list_header(
                "Widgets",
                add_button(
                    "add-widget",
                    "Add text widget",
                    ro,
                    cx,
                    |this, window, cx| this.add_widget(window, cx),
                ),
                cx,
            ))
            .when(section_off && count > 0, |this| {
                this.child(warning_text(
                    "Taskbar widgets are off: turn on \"Show taskbar widgets\" to see them.",
                    cx,
                ))
            })
            .when(count == 0, |this| {
                this.child(hint_text("No widgets yet. Add a text widget to start.", cx))
            })
            .children(rows)
    }

    fn render_widget_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ro = self.read_only();
        let open = self
            .widget_editor
            .as_ref()
            .is_some_and(|e| e.index == index);
        let entry = self.doc.config.widgets.items[index].clone();
        let delete = Button::new(("delete-widget", index))
            .small()
            .ghost()
            .icon(IconName::Delete)
            .disabled(ro)
            .tooltip("Delete")
            .accessibility_label(format!("Delete widget {index}"))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_delete_widget(index, window, cx)
            }));
        let item = match entry {
            WidgetEntry::Unknown(v) => {
                return section(cx)
                    .child(
                        h_flex()
                            .px_5()
                            .py_2()
                            .gap_3()
                            .justify_between()
                            .child(hint_text(
                                format!(
                                    "Unsupported widget \"{}\" (kept as is)",
                                    model::unknown_type(&v)
                                ),
                                cx,
                            ))
                            .child(delete),
                    )
                    .into_any_element();
            }
            WidgetEntry::Known(item) => item,
        };
        let t = snapshot(&item);
        let title = match &t.kind {
            SnapKind::Text { text, .. } => {
                let first_line = text.lines().next().unwrap_or("").to_string();
                if first_line.trim().is_empty() {
                    "(empty)".to_string()
                } else {
                    first_line
                }
            }
            SnapKind::Bar { modules, .. } => {
                let names: Vec<String> = modules.iter().map(module_label).collect();
                if names.is_empty() {
                    "Bar (no modules)".to_string()
                } else {
                    format!("Bar: {}", names.join(", "))
                }
            }
        };
        let duplicate = model::duplicate_widget_id(&self.doc.config.widgets.items, index);
        let toggle = Switch::new(("widget-enabled", index))
            .accessibility_label(format!("Widget {index} enabled"))
            .checked(t.common.enabled)
            .disabled(ro)
            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                if let Some(item) = this.widget_item_mut(index) {
                    item.common_mut().enabled = *on;
                    this.changed(cx);
                }
            }));
        let expand = Button::new(("expand-widget", index))
            .small()
            .ghost()
            .icon(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .disabled(ro)
            .accessibility_label(format!("Edit widget {index}"))
            .on_click(cx.listener(move |this, _, window, cx| {
                if this
                    .widget_editor
                    .as_ref()
                    .is_some_and(|e| e.index == index)
                {
                    this.widget_editor = None;
                    cx.notify();
                } else {
                    this.open_widget(index, window, cx);
                }
            }));
        let header = h_flex()
            .id(("widget-row", index))
            .px_5()
            .py_2()
            .gap_3()
            .items_center()
            .child(toggle)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(title))
                    .child(hint_text(
                        if t.common.id.is_empty() {
                            format!("#{index}")
                        } else {
                            t.common.id.clone()
                        },
                        cx,
                    )),
            )
            .when(duplicate, |this| {
                this.child(warning_text("Duplicate id", cx))
            })
            .child(expand)
            .child(delete);
        let mut card = section(cx).child(header);
        if open {
            card = card
                .child(divider(cx))
                .child(self.render_widget_editor(&t, window, cx));
        }
        card.into_any_element()
    }

    fn render_widget_editor(
        &mut self,
        t: &Snapshot,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(ed) = self.widget_editor.as_ref() else {
            return div().into_any_element();
        };
        let ro = self.read_only();
        let duplicate = model::duplicate_widget_id(&self.doc.config.widgets.items, ed.index);
        let (text, gap, id, x, y, width, height, family, size, color) = (
            ed.text.clone(),
            ed.gap.clone(),
            ed.id.clone(),
            ed.x.clone(),
            ed.y.clone(),
            ed.width.clone(),
            ed.height.clone(),
            ed.font_family.clone(),
            ed.font_size.clone(),
            ed.color.clone(),
        );
        let (xu, yu, wu, hu) = (ed.x_unit, ed.y_unit, ed.width_unit, ed.height_unit);
        let errors: BTreeMap<WField, String> = ed.errors.clone();
        let width_auto = matches!(t.common.width, Lenient::Valid(Dimension::Auto));
        let height_auto = matches!(t.common.height, Lenient::Valid(Dimension::Auto));
        let theme_color = matches!(t.color, Lenient::Valid(None));

        let unit_group = |id: &'static str, f: WField, unit: Unit, cx: &mut Context<Self>| {
            controls::segmented(
                id,
                &UNITS,
                Some(unit),
                ro,
                cx,
                move |this, u, window, cx| this.set_unit(f, u, window, cx),
            )
        };
        let length_field = |label: &'static str,
                            f: WField,
                            input: Entity<InputState>,
                            unit: Unit,
                            unit_id: &'static str,
                            cx: &mut Context<Self>| {
            field(
                label,
                h_flex()
                    .gap_2()
                    .child(NumberInput::new(&input).small().w(px(130.)).disabled(ro))
                    .child(unit_group(unit_id, f, unit, cx)),
                errors.get(&f).map(String::as_str),
                cx,
            )
        };
        let size_field = |label: &'static str,
                          f: WField,
                          input: Entity<InputState>,
                          unit: Unit,
                          auto: bool,
                          ids: (&'static str, &'static str),
                          cx: &mut Context<Self>| {
            field(
                label,
                h_flex()
                    .gap_2()
                    .child(
                        Checkbox::new(ids.0)
                            .label("Auto")
                            .accessibility_label(format!("{label} auto"))
                            .checked(auto)
                            .disabled(ro)
                            .on_click(cx.listener(move |this, on: &bool, window, cx| {
                                this.set_auto(f, *on, window, cx)
                            })),
                    )
                    .when(!auto, |this| {
                        this.child(NumberInput::new(&input).small().w(px(130.)).disabled(ro))
                            .child(unit_group(ids.1, f, unit, cx))
                    }),
                if auto {
                    None
                } else {
                    errors.get(&f).map(String::as_str)
                },
                cx,
            )
        };

        let anchor = controls::segmented(
            "anchor",
            &ANCHORS,
            t.common.anchor.valid().copied(),
            ro,
            cx,
            |this, v, _, cx| {
                this.widget_choice(WField::Anchor, cx, |t| t.common.anchor = Lenient::Valid(v))
            },
        );
        let current_origin = t.common.origin.valid().copied();
        let origin_rows: Vec<_> = ORIGIN_ROWS
            .iter()
            .enumerate()
            .map(|(r, row)| {
                controls::segmented(
                    ["origin-0", "origin-1", "origin-2"][r],
                    row,
                    current_origin,
                    ro,
                    cx,
                    |this, v, _, cx| {
                        this.widget_choice(WField::Origin, cx, |t| {
                            t.common.origin = Lenient::Valid(v)
                        })
                    },
                )
            })
            .collect();
        let weight = controls::segmented(
            "weight",
            &WEIGHTS,
            t.font_weight.valid().copied(),
            ro,
            cx,
            |this, v, _, cx| {
                this.widget_choice(WField::Weight, cx, |t| *t.font_weight = Lenient::Valid(v))
            },
        );
        let align = match &t.kind {
            SnapKind::Text { align, .. } => Some(controls::segmented(
                "align",
                &ALIGNS,
                align.valid().copied(),
                ro,
                cx,
                |this, v, _, cx| {
                    this.widget_choice(WField::Align, cx, |t| {
                        if let Some(a) = t.align {
                            *a = Lenient::Valid(v);
                        }
                    })
                },
            )),
            SnapKind::Bar { .. } => None,
        };
        let modules = match &t.kind {
            SnapKind::Bar { modules, .. } => Some(self.render_bar_modules(modules, cx)),
            SnapKind::Text { .. } => None,
        };
        let empty_text = matches!(&t.kind, SnapKind::Text { text, .. } if text.trim().is_empty());
        let swatch = match t.color {
            Lenient::Valid(Some(c)) => Some(
                div()
                    .size_5()
                    .rounded_sm()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(rgba(u32::from_be_bytes([c.r, c.g, c.b, c.a]))),
            ),
            _ => None,
        };

        v_flex()
            .px_5()
            .py_4()
            .gap_4()
            .when_some(text, |this, text| {
                this.child(field(
                    "Text",
                    Textarea::new(&text).disabled(ro),
                    errors.get(&WField::Text).map(String::as_str),
                    cx,
                ))
            })
            .when(empty_text, |this| {
                this.child(hint_text("Empty text: the widget is not shown.", cx))
            })
            .children(modules)
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .child(
                        field(
                            "Id",
                            Input::new(&id).small().disabled(ro),
                            errors.get(&WField::Id).map(String::as_str),
                            cx,
                        )
                        .w(px(220.)),
                    )
                    .when(duplicate, |this| {
                        this.child(div().pt_5().child(warning_text(
                            "Only the first item with this id is shown",
                            cx,
                        )))
                    }),
            )
            .child(div().text_sm().font_semibold().child("Position"))
            .child(field(
                "Anchor (point of the taskbar)",
                anchor,
                errors.get(&WField::Anchor).map(String::as_str),
                cx,
            ))
            .child(
                h_flex()
                    .gap_6()
                    .flex_wrap()
                    .child(length_field(
                        "X (from the anchor, + = right)",
                        WField::X,
                        x,
                        xu,
                        "x-unit",
                        cx,
                    ))
                    .child(length_field(
                        "Y (from the top edge, + = down)",
                        WField::Y,
                        y,
                        yu,
                        "y-unit",
                        cx,
                    )),
            )
            .child(field(
                "Origin (point of the widget placed at X, Y)",
                v_flex().gap_1().children(origin_rows),
                errors.get(&WField::Origin).map(String::as_str),
                cx,
            ))
            .child(
                h_flex()
                    .gap_6()
                    .flex_wrap()
                    .child(size_field(
                        "Width",
                        WField::Width,
                        width,
                        wu,
                        width_auto,
                        ("width-auto", "width-unit"),
                        cx,
                    ))
                    .child(size_field(
                        "Height",
                        WField::Height,
                        height,
                        hu,
                        height_auto,
                        ("height-auto", "height-unit"),
                        cx,
                    )),
            )
            .child(div().text_sm().font_semibold().child("Text style"))
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .flex_wrap()
                    .child(
                        field(
                            "Font",
                            Input::new(&family).small().disabled(ro),
                            errors.get(&WField::FontFamily).map(String::as_str),
                            cx,
                        )
                        .w(px(240.)),
                    )
                    .child(field(
                        "Size",
                        NumberInput::new(&size).small().w(px(130.)).disabled(ro),
                        errors.get(&WField::FontSize).map(String::as_str),
                        cx,
                    )),
            )
            .child(field(
                "Weight",
                weight,
                errors.get(&WField::Weight).map(String::as_str),
                cx,
            ))
            .when_some(align, |this, align| {
                this.child(field(
                    "Align (inside the widget box)",
                    align,
                    errors.get(&WField::Align).map(String::as_str),
                    cx,
                ))
            })
            .when_some(gap, |this, gap| {
                this.child(field(
                    "Space between modules (px)",
                    NumberInput::new(&gap).small().w(px(130.)).disabled(ro),
                    errors.get(&WField::Gap).map(String::as_str),
                    cx,
                ))
            })
            .child(field(
                "Color",
                h_flex()
                    .gap_2()
                    .child(
                        Checkbox::new("theme-color")
                            .label("Theme color")
                            .accessibility_label("Theme color")
                            .checked(theme_color)
                            .disabled(ro)
                            .on_click(cx.listener(|this, on: &bool, window, cx| {
                                this.set_theme_color(*on, window, cx)
                            })),
                    )
                    .when(!theme_color, |this| {
                        this.child(Input::new(&color).small().w(px(130.)).disabled(ro))
                            .children(swatch)
                    }),
                if theme_color {
                    None
                } else {
                    errors.get(&WField::Color).map(String::as_str)
                },
                cx,
            ))
            .when(theme_color, |this| {
                this.child(hint_text(
                    "Light or dark text, chosen from the taskbar behind the widget.",
                    cx,
                ))
            })
            .into_any_element()
    }

    /// Bar editor: which modules the bar shows (only "To do" exists for now).
    fn render_bar_modules(
        &mut self,
        modules: &[BarModuleEntry],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ro = self.read_only();
        let has_todo = modules
            .iter()
            .any(|m| matches!(m, BarModuleEntry::Known(BarModule::Todo(_))));
        let todo_off = !self.doc.config.todo.enabled;
        let unknown: Vec<String> = modules
            .iter()
            .filter(|m| matches!(m, BarModuleEntry::Unknown(_)))
            .map(module_label)
            .collect();
        field(
            "Modules (left to right)",
            v_flex()
                .gap_2()
                .child(
                    Checkbox::new("bar-module-todo")
                        .label("To do")
                        .accessibility_label("Bar module To do")
                        .checked(has_todo)
                        .disabled(ro)
                        .on_click(cx.listener(|this, on: &bool, _, cx| this.set_bar_todo(*on, cx))),
                )
                .when(has_todo && todo_off, |this| {
                    this.child(warning_text(
                        "To do is off: turn it on in the To do tab to show this module.",
                        cx,
                    ))
                })
                .children(unknown.into_iter().map(|u| hint_text(u, cx))),
            None,
            cx,
        )
        .into_any_element()
    }
}
