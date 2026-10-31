//! `text` widget: a static text, possibly several lines (`\n`), never wrapped.

use kuality_protocol::{
    Color, DEFAULT_FONT_FAMILY, DEFAULT_FONT_SIZE, FontWeight, TextAlign, TextWidget,
};

use super::{Renderer, TextStyle, Theme};
use crate::overlay::layout::SizeF;

/// Longer texts are truncated (in chars).
pub const MAX_TEXT_CHARS: usize = 1000;
/// `font_size` is clamped to this range (DIPs).
pub const FONT_SIZE_RANGE: (f64, f64) = (4.0, 200.0);

/// A resolved text widget.
#[derive(Debug, Clone, PartialEq)]
pub struct TextContent {
    pub text: String,
    pub style: TextStyle,
    /// `None` = theme color.
    pub color: Option<Color>,
}

impl TextContent {
    /// Resolves the config fields. Invalid values fall back to defaults; each problem is
    /// reported through `warn` as a short phrase. Returns `None` for an empty text.
    pub fn resolve(w: &TextWidget, mut warn: impl FnMut(&str)) -> Option<Self> {
        let text: String = w.text.chars().take(MAX_TEXT_CHARS).collect();
        if text.len() < w.text.len() {
            warn("text longer than 1000 characters, truncated");
        }
        if text.is_empty() {
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
                warn("invalid font_size, using the default");
                DEFAULT_FONT_SIZE
            }
        };
        let weight = or_default(w.font_weight.valid(), FontWeight::Normal, &mut || {
            warn("invalid font_weight, using the default")
        });
        let align = or_default(w.align.valid(), TextAlign::Left, &mut || {
            warn("invalid align, using the default")
        });
        let color = or_default(w.color.valid(), None, &mut || {
            warn("invalid color, using the theme color")
        });
        Some(Self {
            text,
            style: TextStyle {
                family,
                size: size as f32,
                weight,
                align,
            },
            color,
        })
    }

    pub fn measure(&self, r: &mut dyn Renderer) -> SizeF {
        r.measure_text(&self.text, &self.style)
    }

    pub fn draw(&self, r: &mut dyn Renderer, size: SizeF, theme: Theme) {
        let color = self.color.unwrap_or_else(|| theme.text_color());
        r.draw_text(&self.text, &self.style, color, size);
    }
}

/// `v`, or `default` after calling `invalid`.
pub fn or_default<T: Copy>(v: Option<&T>, default: T, invalid: &mut dyn FnMut()) -> T {
    v.copied().unwrap_or_else(|| {
        invalid();
        default
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuality_protocol::Lenient;
    use serde_json::json;

    fn resolve(w: &TextWidget) -> (Option<TextContent>, Vec<String>) {
        let mut warnings = Vec::new();
        let c = TextContent::resolve(w, |f| warnings.push(f.to_owned()));
        (c, warnings)
    }

    #[test]
    fn defaults() {
        let w = TextWidget {
            text: "Hello".into(),
            ..TextWidget::default()
        };
        let (c, warnings) = resolve(&w);
        let c = c.unwrap();
        assert!(warnings.is_empty());
        assert_eq!(c.text, "Hello");
        assert_eq!(c.style.family, "Segoe UI Variable Text");
        assert_eq!(c.style.size, 12.0);
        assert_eq!(c.style.weight, FontWeight::Normal);
        assert_eq!(c.style.align, TextAlign::Left);
        assert_eq!(c.color, None);
    }

    #[test]
    fn empty_text_is_not_shown() {
        assert_eq!(resolve(&TextWidget::default()).0, None);
    }

    #[test]
    fn long_text_is_truncated() {
        let w = TextWidget {
            text: "é".repeat(1500),
            ..TextWidget::default()
        };
        let (c, warnings) = resolve(&w);
        assert_eq!(c.unwrap().text.chars().count(), 1000);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn font_size_is_clamped_and_invalid_values_fall_back() {
        let w = TextWidget {
            text: "x".into(),
            font_size: Lenient::Valid(1000.0),
            font_weight: Lenient::Invalid(json!("black")),
            align: Lenient::Invalid(json!(1)),
            color: Lenient::Invalid(json!("red")),
            font_family: "  ".into(),
            ..TextWidget::default()
        };
        let (c, warnings) = resolve(&w);
        let c = c.unwrap();
        assert_eq!(c.style.size, 200.0);
        assert_eq!(c.style.weight, FontWeight::Normal);
        assert_eq!(c.style.align, TextAlign::Left);
        assert_eq!(c.color, None);
        assert_eq!(c.style.family, "Segoe UI Variable Text");
        assert_eq!(warnings.len(), 3);
        assert!(warnings[0].contains("font_weight"));
        assert!(warnings[1].contains("align"));
        assert!(warnings[2].contains("color"));

        let small = TextWidget {
            text: "x".into(),
            font_size: Lenient::Valid(1.0),
            ..TextWidget::default()
        };
        assert_eq!(resolve(&small).0.unwrap().style.size, 4.0);
        let bad = TextWidget {
            text: "x".into(),
            font_size: Lenient::Invalid(json!("big")),
            ..TextWidget::default()
        };
        let (c, warnings) = resolve(&bad);
        assert_eq!(c.unwrap().style.size, 12.0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("font_size"));
    }

    #[test]
    fn theme_color_unless_set() {
        struct Spy(Vec<Color>);
        impl Renderer for Spy {
            fn measure_text(&mut self, _: &str, _: &TextStyle) -> SizeF {
                SizeF::default()
            }
            fn draw_text(&mut self, _: &str, _: &TextStyle, color: Color, _: SizeF) {
                self.0.push(color);
            }
        }
        let mut spy = Spy(Vec::new());
        let w = TextWidget {
            text: "x".into(),
            ..TextWidget::default()
        };
        let c = resolve(&w).0.unwrap();
        c.draw(&mut spy, SizeF::default(), Theme::Dark);
        c.draw(&mut spy, SizeF::default(), Theme::Light);
        let red = Color {
            a: 255,
            r: 255,
            g: 0,
            b: 0,
        };
        let custom = TextContent {
            color: Some(red),
            ..c
        };
        custom.draw(&mut spy, SizeF::default(), Theme::Light);
        assert_eq!(
            spy.0,
            [Theme::Dark.text_color(), Theme::Light.text_color(), red]
        );
        assert_eq!(spy.0[0].r, 0xFF);
        assert_eq!(spy.0[1].r, 0x1B);
    }
}
