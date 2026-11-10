//! `bar` widget: modules (to-do, later media…) side by side at one position. Each module is a
//! clickable "pill": text with horizontal padding, highlighted on hover.

use kuality_protocol::{
    BarModule, BarModuleEntry, BarWidget, Color, DEFAULT_BAR_GAP, DEFAULT_FONT_FAMILY,
    DEFAULT_FONT_SIZE, DEFAULT_TODO_LABEL, FontWeight, TextAlign,
};
use serde_json::Value;

use super::text::{FONT_SIZE_RANGE, or_default};
use super::{DrawContext, LiveData, Renderer, TextStyle};
use crate::overlay::layout::{RectF, SizeF};

/// Space left and right of a module's text (DIPs); the hover background covers it.
pub const MODULE_PAD_X: f32 = 8.0;
/// Space above and below the text (DIPs), so the hover background is taller than the text.
pub const PAD_Y: f32 = 6.0;
/// Corner radius of the hover background (DIPs).
pub const HOVER_RADIUS: f32 = 4.0;
/// Fills every module so the whole pill takes the mouse: a layered window lets the mouse
/// through where it is fully transparent (between the letters). Alpha 1/255 is invisible.
pub const HIT_FILL: Color = Color {
    a: 1,
    r: 0,
    g: 0,
    b: 0,
};
/// `gap` is clamped to this range (DIPs).
pub const GAP_RANGE: (f64, f64) = (0.0, 200.0);
/// Longer labels are truncated (in chars).
pub const MAX_LABEL_CHARS: usize = 100;

/// What a module does when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    /// Opens (or closes) the to-do popup; `open_on_hover`: also shows it when the mouse rests
    /// on the module.
    Todo { open_on_hover: bool },
}

impl ModuleKind {
    /// Resting the mouse on the module runs its hover action.
    pub fn opens_on_hover(self) -> bool {
        match self {
            Self::Todo { open_on_hover } => open_on_hover,
        }
    }
}

/// A resolved module.
#[derive(Debug, Clone, PartialEq)]
pub enum Module {
    Todo {
        label: String,
        show_count: bool,
        open_on_hover: bool,
    },
}

impl Module {
    pub fn kind(&self) -> ModuleKind {
        match self {
            Self::Todo { open_on_hover, .. } => ModuleKind::Todo {
                open_on_hover: *open_on_hover,
            },
        }
    }

    /// The text shown for the module.
    pub fn text(&self, live: &LiveData) -> String {
        match self {
            Self::Todo {
                label, show_count, ..
            } => todo_text(label, *show_count, live.todo_pending),
        }
    }
}

/// `TO DO (3)`; just the label when the count is off, zero or unknown.
pub fn todo_text(label: &str, show_count: bool, pending: Option<usize>) -> String {
    match pending {
        Some(n) if show_count && n > 0 => format!("{label} ({n})"),
        _ => label.to_owned(),
    }
}

/// One module's place in the bar: `x`..`x + w` DIPs from the box's left edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub x: f32,
    pub w: f32,
    pub kind: ModuleKind,
}

impl Span {
    pub fn contains(&self, x: f32) -> bool {
        x >= self.x && x < self.x + self.w
    }
}

/// A resolved bar widget.
#[derive(Debug, Clone, PartialEq)]
pub struct BarContent {
    /// Visible modules, left to right (never empty).
    pub modules: Vec<Module>,
    pub style: TextStyle,
    /// `None` = theme color.
    pub color: Option<Color>,
    pub gap: f32,
}

impl BarContent {
    /// Resolves the config fields. `todo_enabled` is the `todo.enabled` switch: the to-do module
    /// only shows while it is on. Invalid values fall back to defaults and unknown module types
    /// are skipped; each problem is reported through `note` (`true` = warning, `false` = info).
    /// Returns `None` when no module is visible.
    pub fn resolve(
        w: &BarWidget,
        todo_enabled: bool,
        mut note: impl FnMut(bool, &str),
    ) -> Option<Self> {
        let mut modules = Vec::new();
        for entry in &w.modules {
            match entry {
                BarModuleEntry::Known(BarModule::Todo(t)) => {
                    if !todo_enabled {
                        continue;
                    }
                    let label: String = t.label.trim().chars().take(MAX_LABEL_CHARS).collect();
                    let label = if label.is_empty() {
                        DEFAULT_TODO_LABEL.to_owned()
                    } else {
                        label
                    };
                    modules.push(Module::Todo {
                        label,
                        show_count: t.show_count,
                        open_on_hover: t.open_on_hover,
                    });
                }
                BarModuleEntry::Unknown(v) => match v.get("type").and_then(Value::as_str) {
                    Some("todo") => note(true, "invalid todo module (wrong field type?), ignored"),
                    Some(t) => {
                        let t: String = t.chars().take(40).collect();
                        note(
                            false,
                            &format!("module type '{t}' is not supported yet, ignored"),
                        );
                    }
                    None => note(true, "module with a missing or invalid \"type\", ignored"),
                },
            }
        }
        if modules.is_empty() {
            return None;
        }
        let family = if w.font_family.trim().is_empty() {
            DEFAULT_FONT_FAMILY.to_owned()
        } else {
            w.font_family.clone()
        };
        let size = match w.font_size.valid() {
            Some(s) if s.is_finite() => s.clamp(FONT_SIZE_RANGE.0, FONT_SIZE_RANGE.1),
            _ => {
                note(true, "invalid font_size, using the default");
                DEFAULT_FONT_SIZE
            }
        };
        let gap = match w.gap.valid() {
            Some(g) if g.is_finite() => g.clamp(GAP_RANGE.0, GAP_RANGE.1),
            _ => {
                note(true, "invalid gap, using the default");
                DEFAULT_BAR_GAP
            }
        };
        let weight = or_default(w.font_weight.valid(), FontWeight::Semibold, &mut || {
            note(true, "invalid font_weight, using the default")
        });
        let color = or_default(w.color.valid(), None, &mut || {
            note(true, "invalid color, using the theme color")
        });
        Some(Self {
            modules,
            style: TextStyle {
                family,
                size: size as f32,
                weight,
                align: TextAlign::Left,
            },
            color,
            gap: gap as f32,
        })
    }

    /// Module texts and their measured sizes.
    fn measured(&self, r: &mut dyn Renderer, live: &LiveData) -> Vec<(String, SizeF)> {
        self.modules
            .iter()
            .map(|m| {
                let text = m.text(live);
                let size = r.measure_text(&text, &self.style);
                (text, size)
            })
            .collect()
    }

    pub fn measure(&self, r: &mut dyn Renderer, live: &LiveData) -> SizeF {
        let measured = self.measured(r, live);
        let spans = spans_of(&self.modules, &measured, self.gap);
        let w = spans.last().map_or(0.0, |s| s.x + s.w);
        let h = measured.iter().map(|(_, s)| s.h).fold(0.0, f32::max) + 2.0 * PAD_Y;
        SizeF { w, h }
    }

    pub fn spans(&self, r: &mut dyn Renderer, live: &LiveData) -> Vec<Span> {
        let measured = self.measured(r, live);
        spans_of(&self.modules, &measured, self.gap)
    }

    pub fn draw(&self, r: &mut dyn Renderer, size: SizeF, cx: &DrawContext) {
        let color = self.color.unwrap_or_else(|| cx.theme.text_color());
        let measured = self.measured(r, &cx.live);
        let spans = spans_of(&self.modules, &measured, self.gap);
        for (i, (span, (text, _))) in spans.iter().zip(&measured).enumerate() {
            let pill = RectF {
                x: span.x,
                y: 0.0,
                w: span.w,
                h: size.h,
            };
            let fill = if cx.hover == Some(i) {
                cx.theme.hover_color()
            } else {
                HIT_FILL
            };
            r.fill_rounded_rect(pill, HOVER_RADIUS, fill);
            let text_rect = RectF {
                x: span.x + MODULE_PAD_X,
                y: 0.0,
                w: (span.w - 2.0 * MODULE_PAD_X).max(0.0),
                h: size.h,
            };
            r.draw_text(text, &self.style, color, text_rect);
        }
    }
}

/// Lays modules out left to right: each one is its text width plus padding, `gap` apart.
fn spans_of(modules: &[Module], measured: &[(String, SizeF)], gap: f32) -> Vec<Span> {
    let mut x = 0.0;
    modules
        .iter()
        .zip(measured)
        .map(|(m, (_, size))| {
            let span = Span {
                x,
                w: size.w.ceil() + 2.0 * MODULE_PAD_X,
                kind: m.kind(),
            };
            x += span.w + gap;
            span
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::widgets::Theme;
    use kuality_protocol::{Lenient, TodoModule};
    use serde_json::json;

    /// Every character is 10 x 16 DIPs; records draw calls.
    #[derive(Default)]
    struct Fake {
        texts: Vec<(String, RectF)>,
        fills: Vec<(RectF, Color)>,
    }

    impl Renderer for Fake {
        fn measure_text(&mut self, text: &str, _: &TextStyle) -> SizeF {
            SizeF {
                w: 10.0 * text.chars().count() as f32,
                h: 16.0,
            }
        }
        fn draw_text(&mut self, text: &str, _: &TextStyle, _: Color, rect: RectF) {
            self.texts.push((text.to_owned(), rect));
        }
        fn fill_rounded_rect(&mut self, rect: RectF, _: f32, color: Color) {
            self.fills.push((rect, color));
        }
    }

    fn todo_bar(modules: serde_json::Value) -> BarWidget {
        serde_json::from_value(json!({ "modules": modules })).unwrap()
    }

    fn resolve(w: &BarWidget, todo: bool) -> (Option<BarContent>, Vec<(bool, String)>) {
        let mut notes = Vec::new();
        let c = BarContent::resolve(w, todo, |warn, m| notes.push((warn, m.to_owned())));
        (c, notes)
    }

    #[test]
    fn todo_text_formats() {
        assert_eq!(todo_text("TO DO", true, Some(3)), "TO DO (3)");
        assert_eq!(todo_text("TO DO", true, Some(0)), "TO DO");
        assert_eq!(todo_text("TO DO", true, None), "TO DO");
        assert_eq!(todo_text("TO DO", false, Some(3)), "TO DO");
    }

    #[test]
    fn todo_module_follows_the_switch_and_unknown_modules_are_skipped() {
        let w = todo_bar(json!([{ "type": "media" }, { "type": "todo", "label": "  " }]));
        let (c, notes) = resolve(&w, false);
        assert!(c.is_none(), "nothing visible while to-do is off");
        assert_eq!(notes.len(), 1);
        assert!(!notes[0].0 && notes[0].1.contains("'media'"));

        let (c, _) = resolve(&w, true);
        let c = c.unwrap();
        assert_eq!(
            c.modules,
            [Module::Todo {
                label: "TO DO".into(),
                show_count: true,
                open_on_hover: true
            }]
        );
        assert_eq!(c.style.weight, FontWeight::Semibold);
        assert_eq!(c.gap, 16.0);
    }

    #[test]
    fn invalid_fields_fall_back() {
        let mut w = todo_bar(json!([{ "type": "todo" }, { "type": "todo", "label": 5 }]));
        w.gap = Lenient::Invalid(json!("wide"));
        w.font_size = Lenient::Valid(1000.0);
        let (c, notes) = resolve(&w, true);
        let c = c.unwrap();
        assert_eq!(c.modules.len(), 1);
        assert_eq!(c.gap, 16.0);
        assert_eq!(c.style.size, 200.0);
        let warnings: Vec<_> = notes.iter().filter(|(w, _)| *w).collect();
        assert_eq!(warnings.len(), 2, "{notes:?}");
    }

    #[test]
    fn layout_spans_and_hover() {
        let w = BarWidget {
            modules: vec![
                BarModuleEntry::Known(BarModule::Todo(TodoModule::default())),
                BarModuleEntry::Known(BarModule::Todo(TodoModule {
                    label: "B".into(),
                    show_count: false,
                    open_on_hover: false,
                    ..TodoModule::default()
                })),
            ],
            ..BarWidget::default()
        };
        let c = resolve(&w, true).0.unwrap();
        let live = LiveData {
            todo_pending: Some(3),
        };
        let mut r = Fake::default();
        // "TO DO (3)" = 9 chars = 90 + 16 padding; "B" = 10 + 16; gap 16.
        let spans = c.spans(&mut r, &live);
        assert_eq!(
            spans,
            [
                Span {
                    x: 0.0,
                    w: 106.0,
                    kind: ModuleKind::Todo {
                        open_on_hover: true
                    }
                },
                Span {
                    x: 122.0,
                    w: 26.0,
                    kind: ModuleKind::Todo {
                        open_on_hover: false
                    }
                }
            ]
        );
        assert!(spans[0].contains(0.0) && !spans[0].contains(106.0));
        assert!(
            !spans.iter().any(|s| s.contains(110.0)),
            "gap is not clickable"
        );
        assert_eq!(c.measure(&mut r, &live), SizeF { w: 148.0, h: 28.0 });

        let size = SizeF { w: 148.0, h: 28.0 };
        let cx = DrawContext {
            theme: Theme::Dark,
            live,
            hover: Some(1),
        };
        c.draw(&mut r, size, &cx);
        assert_eq!(r.texts[0].0, "TO DO (3)");
        assert_eq!(r.texts[0].1.x, 8.0);
        assert_eq!(r.texts[1].0, "B");
        let pill = |x, w| RectF {
            x,
            y: 0.0,
            w,
            h: 28.0,
        };
        assert_eq!(
            r.fills,
            [
                (pill(0.0, 106.0), HIT_FILL),
                (pill(122.0, 26.0), Theme::Dark.hover_color())
            ],
            "every pill takes the mouse; the hovered one is highlighted"
        );
    }
}
