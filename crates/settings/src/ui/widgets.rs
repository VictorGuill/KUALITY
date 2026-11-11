//! Widgets tab: the two switches and the widget list (text widgets and bars) with an inline
//! editor shared by both types.

use std::collections::BTreeMap;

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::form::{Field, v_form};
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants as _};
use gpui_kit::component::input::{
    Input, InputEvent, InputState, NumberInput, Textarea, TextareaState,
};
use gpui_kit::component::label::Label;
use gpui_kit::component::separator::Separator;
use gpui_kit::component::setting::{SettingGroup, SettingPage};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    Disableable as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::{
    Anchor, BarModule, BarModuleEntry, Color, CommonItem, Dimension, FontWeight, Lenient, Origin,
    TextAlign, TextWidget, TodoModule, WidgetEntry, WidgetItem,
};

use super::SettingsWindow;
use super::controls::{
    self, INVALID_IN_FILE, Note, Weak, add_button, block_item, field, hint_text, switch_item,
    warning_text,
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

/// Inputs of the expanded widget.
pub(crate) struct WidgetEditor {
    pub index: usize,
    /// Text widgets only (multi-line).
    text: Option<Entity<TextareaState>>,
    /// The single-line inputs, by field (`Gap` only for bars).
    inputs: BTreeMap<WField, Entity<InputState>>,
    /// Unit of `X`, `Y`, `Width` and `Height`.
    units: BTreeMap<WField, Unit>,
    /// The color picker next to the `Color` input (it writes its choice into that input).
    color_picker: Entity<ColorPickerState>,
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

    fn input(&self, f: WField) -> Option<&Entity<InputState>> {
        self.inputs.get(&f)
    }

    fn unit(&self, f: WField) -> Unit {
        self.units.get(&f).copied().unwrap_or(Unit::Px)
    }

    /// The note under field `f`, if any.
    fn note(&self, f: WField) -> Option<Note> {
        self.errors.get(&f).map(|e| Note::error(e.clone()))
    }
}

/// How a single-line input of the editor is set up.
enum InputKind {
    Line(&'static str),
    /// A number with +/- buttons, optionally limited to a range.
    Number(Option<(f64, f64)>),
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

/// A config color for the color picker, and back (8 bits per channel).
fn to_hsla(c: Color) -> Hsla {
    let f = |v: u8| f32::from(v) / 255.;
    Rgba {
        r: f(c.r),
        g: f(c.g),
        b: f(c.b),
        a: f(c.a),
    }
    .into()
}

fn from_hsla(color: Hsla) -> Color {
    let c = Rgba::from(color);
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    Color {
        a: byte(c.a),
        r: byte(c.r),
        g: byte(c.g),
        b: byte(c.b),
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

    /// Expands widget `index`: creates its inputs from the config.
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
        let number = |f: WField, v: &Lenient<f64>, mark: &mut dyn FnMut(WField, bool)| match v {
            Lenient::Valid(v) => model::format_number(*v),
            Lenient::Invalid(v) => {
                mark(f, false);
                raw_text(v)
            }
        };
        let mut units = BTreeMap::new();
        // Field, initial text and kind of every single-line input.
        let mut lines: Vec<(WField, String, InputKind)> =
            vec![(WField::Id, t.common.id.clone(), InputKind::Line("#index"))];
        for (f, (text, unit, ok)) in [
            (WField::X, length_text(&t.common.x)),
            (WField::Y, length_text(&t.common.y)),
            (WField::Width, dimension_text(&t.common.width)),
            (WField::Height, dimension_text(&t.common.height)),
        ] {
            mark(f, ok);
            units.insert(f, unit);
            lines.push((f, text, InputKind::Number(None)));
        }
        lines.push((
            WField::FontFamily,
            t.font_family.clone(),
            InputKind::Line("Segoe UI Variable Text"),
        ));
        lines.push((
            WField::FontSize,
            number(WField::FontSize, &t.font_size, &mut mark),
            InputKind::Number(Some(model::FONT_SIZE_RANGE)),
        ));
        let color = match &t.color {
            Lenient::Valid(None) => String::new(),
            Lenient::Valid(Some(c)) => color_hex(*c),
            Lenient::Invalid(v) => {
                mark(WField::Color, false);
                raw_text(v)
            }
        };
        lines.push((WField::Color, color, InputKind::Line("#RRGGBB")));
        mark(WField::Anchor, t.common.anchor.valid().is_some());
        mark(WField::Origin, t.common.origin.valid().is_some());
        mark(WField::Weight, t.font_weight.valid().is_some());
        match &t.kind {
            SnapKind::Text { align, .. } => mark(WField::Align, align.valid().is_some()),
            SnapKind::Bar { gap, .. } => lines.push((
                WField::Gap,
                number(WField::Gap, gap, &mut mark),
                InputKind::Number(Some(model::GAP_RANGE)),
            )),
        }

        let mut subs = Vec::new();
        let text = match &t.kind {
            SnapKind::Text { text, .. } => Some(cx.new(|cx| {
                let mut s = TextareaState::new(window, cx).default_value(text.clone());
                s.set_auto_grow(2, 6, cx);
                s
            })),
            SnapKind::Bar { .. } => None,
        };
        if let Some(text) = &text {
            subs.push(
                cx.subscribe_in(text, window, |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.widget_field_changed(WField::Text, window, cx);
                    }
                }),
            );
        }
        let mut inputs = BTreeMap::new();
        for (f, value, kind) in lines {
            let input = cx.new(|cx| {
                let state = InputState::new(window, cx).default_value(value);
                match kind {
                    InputKind::Line(placeholder) => state.placeholder(placeholder),
                    InputKind::Number(None) => state.step(1.0),
                    InputKind::Number(Some((min, max))) => state.step(1.0).min(min).max(max),
                }
            });
            subs.push(cx.subscribe_in(
                &input,
                window,
                move |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.widget_field_changed(f, window, cx);
                    }
                },
            ));
            inputs.insert(f, input);
        }
        let color_picker = cx.new(|cx| {
            let state = ColorPickerState::new(window, cx);
            match t.color {
                Lenient::Valid(Some(c)) => state.default_value(to_hsla(c)),
                _ => state,
            }
        });
        // A color chosen in the picker goes through the hex input, like a typed one.
        subs.push(cx.subscribe_in(
            &color_picker,
            window,
            |this, _, ev: &ColorPickerEvent, window, cx| {
                let ColorPickerEvent::Change(Some(color)) = ev else {
                    return;
                };
                let hex = color_hex(from_hsla(*color));
                let input = this
                    .widget_editor
                    .as_ref()
                    .and_then(|ed| ed.input(WField::Color).cloned());
                if let Some(input) = input {
                    input.update(cx, |s, cx| s.set_value(hex, window, cx));
                    this.widget_field_changed(WField::Color, window, cx);
                }
            },
        ));
        self.widget_editor = Some(WidgetEditor {
            index,
            text,
            inputs,
            units,
            color_picker,
            errors,
            _subs: subs,
        });
        cx.notify();
    }

    /// Reads the changed input, validates it and stores it if valid.
    fn widget_field_changed(&mut self, f: WField, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only() {
            return;
        }
        let Some(ed) = self.widget_editor.as_ref() else {
            return;
        };
        let (index, unit) = (ed.index, ed.unit(f));
        let text = match (f, &ed.text, ed.input(f)) {
            (WField::Text, Some(text), _) => text.read(cx).value().to_string(),
            (_, _, Some(input)) => input.read(cx).value().to_string(),
            _ => return,
        };
        let Some(t) = self.style_mut(index) else {
            return;
        };
        // A valid color typed in the input, to show in the color picker.
        let mut picked = None;
        let result: Result<(), &'static str> = match f {
            WField::Text => {
                if let Some(s) = t.text {
                    *s = text;
                }
                Ok(())
            }
            WField::Id => {
                t.common.id = text;
                Ok(())
            }
            WField::X | WField::Y => model::parse_number(&text).map(|v| {
                let length = if f == WField::X {
                    &mut t.common.x
                } else {
                    &mut t.common.y
                };
                *length = Lenient::Valid(unit.make(v));
            }),
            WField::Width | WField::Height => {
                let size = if f == WField::Width {
                    &mut t.common.width
                } else {
                    &mut t.common.height
                };
                // "Auto" ignores the number until it is unticked.
                if matches!(size, Lenient::Valid(Dimension::Auto)) {
                    Ok(())
                } else {
                    model::parse_size(&text)
                        .map(|v| *size = Lenient::Valid(Dimension::Length(unit.make(v))))
                }
            }
            WField::FontFamily => model::validate_font_family(&text)
                .map(|()| *t.font_family = text.trim().to_string()),
            WField::FontSize => {
                model::parse_font_size(&text).map(|v| *t.font_size = Lenient::Valid(v))
            }
            WField::Gap => model::parse_gap(&text).map(|v| {
                if let Some(g) = t.gap {
                    *g = Lenient::Valid(v);
                }
            }),
            // "Theme color" ignores the hex value until it is unticked.
            WField::Color if matches!(*t.color, Lenient::Valid(None)) => Ok(()),
            WField::Color => model::parse_color(&text).map(|c| {
                *t.color = Lenient::Valid(Some(c));
                picked = Some(c);
            }),
            // Choices are set by `widget_choice`.
            WField::Anchor | WField::Origin | WField::Weight | WField::Align => Ok(()),
        };
        if let Some(ed) = self.widget_editor.as_mut() {
            match result {
                Ok(()) => ed.errors.remove(&f),
                Err(e) => ed.errors.insert(f, e.to_string()),
            };
            if let Some(color) = picked {
                // `set_value` does not report a change back, so there is no loop.
                ed.color_picker
                    .update(cx, |p, cx| p.set_value(to_hsla(color), window, cx));
            }
        }
        if result.is_ok() {
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
            ed.units.insert(f, unit);
        }
        self.widget_field_changed(f, window, cx);
    }

    /// Ticks or unticks "Auto" of `Width` / `Height`, or "Theme color" of `Color`: `value` is
    /// what the field holds while it is on, `fallback` the text put in its input when it is
    /// turned off with nothing valid there.
    fn set_automatic(
        &mut self,
        f: WField,
        on: bool,
        fallback: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ed) = self.widget_editor.as_ref() else {
            return;
        };
        let (index, input) = (ed.index, ed.input(f).cloned());
        if let Some(t) = self.style_mut(index) {
            // Off: a placeholder value the input then replaces with what it holds.
            match f {
                WField::Width | WField::Height => {
                    let size = if f == WField::Width {
                        &mut t.common.width
                    } else {
                        &mut t.common.height
                    };
                    *size = Lenient::Valid(if on {
                        Dimension::Auto
                    } else {
                        Dimension::Length(kuality_protocol::Length::Px(0.0))
                    });
                }
                WField::Color => {
                    let white = Color {
                        a: 255,
                        r: 255,
                        g: 255,
                        b: 255,
                    };
                    *t.color = Lenient::Valid((!on).then_some(white));
                }
                _ => return,
            }
        }
        if let Some(ed) = self.widget_editor.as_mut() {
            ed.errors.remove(&f);
        }
        if on {
            self.changed(cx);
            return;
        }
        if let Some(input) = input {
            let text = input.read(cx).value();
            let valid = match f {
                WField::Color => model::parse_color(&text).is_ok(),
                _ => model::parse_size(&text).is_ok(),
            };
            if !valid {
                input.update(cx, |s, cx| s.set_value(fallback, window, cx));
            }
        }
        self.widget_field_changed(f, window, cx);
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

    pub(super) fn widgets_page(&self, weak: &Weak, _cx: &mut Context<Self>) -> SettingPage {
        let ro = self.read_only();
        SettingPage::new("Widgets")
            .icon(IconName::PanelBottom)
            .group(
                SettingGroup::new()
                    .item(
                        switch_item(
                            "Show taskbar widgets",
                            weak,
                            |w| w.doc.config.widgets.enabled,
                            |w, on, cx| {
                                w.doc.config.widgets.enabled = on;
                                w.changed(cx);
                            },
                        )
                        .description("Draw the widgets below on top of the Windows taskbar.")
                        .disabled(ro),
                    )
                    .item(
                        switch_item(
                            "Hide when a fullscreen app is in front",
                            weak,
                            |w| w.doc.config.widgets.hide_on_fullscreen,
                            |w, on, cx| {
                                w.doc.config.widgets.hide_on_fullscreen = on;
                                w.changed(cx);
                            },
                        )
                        .disabled(ro),
                    ),
            )
            .group(SettingGroup::new().title("Items").item(
                block_item(weak, |this, window, cx| this.render_widget_list(window, cx)).keywords(
                    [
                        "widget", "text", "bar", "position", "anchor", "font", "color", "size",
                    ],
                ),
            ))
    }

    /// The "Add" button, the notes and one card per widget.
    fn render_widget_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let ro = self.read_only();
        let widgets = &self.doc.config.widgets;
        let (off, count) = (!widgets.enabled, widgets.items.len());
        let rows: Vec<AnyElement> = (0..count)
            .map(|index| self.render_widget_row(index, window, cx))
            .collect();
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(match (count, off) {
                        (0, _) => hint_text("No widgets yet. Add a text widget to start.", cx),
                        (_, true) => warning_text(
                            "Taskbar widgets are off: turn on \"Show taskbar widgets\" to see \
                             them.",
                            cx,
                        ),
                        _ => hint_text("Shown on every taskbar.", cx),
                    })
                    .child(add_button(
                        "add-widget",
                        "Add text widget",
                        ro,
                        cx,
                        |this, window, cx| this.add_widget(window, cx),
                    )),
            )
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
                return GroupBox::new()
                    .outline()
                    .child(
                        h_flex()
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
            .gap_3()
            .items_center()
            .child(toggle)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(Label::new(title).text_sm().truncate())
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
        // The editor exists only while the card is open (its inputs belong to that widget).
        let editor = open.then(|| {
            v_flex()
                .gap_4()
                .child(Separator::horizontal())
                .child(self.render_widget_editor(&t, window, cx))
        });
        GroupBox::new()
            .outline()
            .child(
                Collapsible::new()
                    .open(open)
                    .child(header)
                    .when_some(editor, |c, editor| c.content(div().pt_4().child(editor))),
            )
            .into_any_element()
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
        let width_auto = matches!(t.common.width, Lenient::Valid(Dimension::Auto));
        let height_auto = matches!(t.common.height, Lenient::Valid(Dimension::Auto));
        let theme_color = matches!(t.color, Lenient::Valid(None));
        let number = |f: WField| {
            ed.input(f)
                .map(|input| NumberInput::new(input).small().w(px(130.)).disabled(ro))
        };
        let line = |f: WField| {
            ed.input(f)
                .map(|input| Input::new(input).small().disabled(ro))
        };
        let unit_choice = |id: &'static str, f: WField, cx: &mut Context<Self>| {
            controls::segmented(
                id,
                &UNITS,
                Some(ed.unit(f)),
                ro,
                cx,
                move |this, u, window, cx| this.set_unit(f, u, window, cx),
            )
        };
        // "X" / "Y": a number and its unit.
        let length_field = |label: &'static str, f: WField, id, cx: &mut Context<Self>| {
            field(
                label,
                h_flex()
                    .gap_2()
                    .children(number(f))
                    .child(unit_choice(id, f, cx)),
                ed.note(f),
            )
        };
        // "Width" / "Height": "Auto", or a number and its unit.
        let size_field = |label: &'static str,
                          f: WField,
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
                                this.set_automatic(f, *on, "100", window, cx)
                            })),
                    )
                    .when(!auto, |row| {
                        row.children(number(f)).child(unit_choice(ids.1, f, cx))
                    }),
                ed.note(f).filter(|_| !auto),
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

        // What the widget shows: its text, or the modules of a bar.
        let content = match &t.kind {
            SnapKind::Text { text, .. } => field(
                "Text",
                div().children(ed.text.as_ref().map(|s| Textarea::new(s).disabled(ro))),
                ed.note(WField::Text).or_else(|| {
                    text.trim()
                        .is_empty()
                        .then(|| Note::hint("Empty text: the widget is not shown."))
                }),
            ),
            SnapKind::Bar { modules, .. } => self.bar_modules_field(modules, cx),
        };
        // Text widgets align their text; bars space their modules.
        let last_style_field = match &t.kind {
            SnapKind::Text { align, .. } => field(
                "Align (inside the widget box)",
                controls::segmented(
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
                ),
                ed.note(WField::Align),
            ),
            SnapKind::Bar { .. } => field(
                "Space between modules (px)",
                div().children(number(WField::Gap)),
                ed.note(WField::Gap),
            ),
        };
        let color = field(
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
                            this.set_automatic(WField::Color, *on, "#FFFFFF", window, cx)
                        })),
                )
                .when(!theme_color, |row| {
                    row.children(line(WField::Color).map(|i| i.w(px(130.))))
                        .child(
                            ColorPicker::new(&ed.color_picker)
                                .small()
                                .accessibility_label("Pick a color"),
                        )
                }),
            if theme_color {
                Some(Note::hint(
                    "Light or dark text, chosen from the taskbar behind the widget.",
                ))
            } else {
                ed.note(WField::Color)
            },
        );
        let id = field(
            "Id",
            div().children(line(WField::Id)),
            ed.note(WField::Id).or_else(|| {
                duplicate.then(|| Note::warning("Only the first item with this id is shown"))
            }),
        );
        let heading = |text: &'static str| Label::new(text).text_sm().font_semibold();

        v_flex()
            .gap_4()
            .child(v_form().columns(2).child(content.col_span(2)).child(id))
            .child(heading("Position"))
            .child(
                v_form()
                    .columns(2)
                    .child(
                        field(
                            "Anchor (point of the taskbar)",
                            anchor,
                            ed.note(WField::Anchor),
                        )
                        .col_span(2),
                    )
                    .child(length_field(
                        "X (from the anchor, + = right)",
                        WField::X,
                        "x-unit",
                        cx,
                    ))
                    .child(length_field(
                        "Y (from the top edge, + = down)",
                        WField::Y,
                        "y-unit",
                        cx,
                    ))
                    .child(
                        field(
                            "Origin (point of the widget placed at X, Y)",
                            v_flex().gap_1().children(origin_rows),
                            ed.note(WField::Origin),
                        )
                        .col_span(2),
                    )
                    .child(size_field(
                        "Width",
                        WField::Width,
                        width_auto,
                        ("width-auto", "width-unit"),
                        cx,
                    ))
                    .child(size_field(
                        "Height",
                        WField::Height,
                        height_auto,
                        ("height-auto", "height-unit"),
                        cx,
                    )),
            )
            .child(heading("Text style"))
            .child(
                v_form()
                    .columns(2)
                    .child(field(
                        "Font",
                        div().children(line(WField::FontFamily)),
                        ed.note(WField::FontFamily),
                    ))
                    .child(field(
                        "Size",
                        div().children(number(WField::FontSize)),
                        ed.note(WField::FontSize),
                    ))
                    .child(field("Weight", weight, ed.note(WField::Weight)))
                    .child(last_style_field)
                    .child(color.col_span(2)),
            )
            .into_any_element()
    }

    /// Bar editor: which modules the bar shows (only "To do" exists for now).
    fn bar_modules_field(&self, modules: &[BarModuleEntry], cx: &mut Context<Self>) -> Field {
        let ro = self.read_only();
        let has_todo = modules
            .iter()
            .any(|m| matches!(m, BarModuleEntry::Known(BarModule::Todo(_))));
        let unknown = modules
            .iter()
            .filter(|m| matches!(m, BarModuleEntry::Unknown(_)))
            .map(|m| hint_text(module_label(m), cx));
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
                .children(unknown),
            (has_todo && !self.doc.config.todo.enabled).then(|| {
                Note::warning("To do is off: turn it on in the To do tab to show this module.")
            }),
        )
    }
}
