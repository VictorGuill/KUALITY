//! Widget contents: what each widget type measures and draws, through the [`Renderer`] trait.

pub mod bar;
pub mod text;

use kuality_protocol::{Color, FontWeight, TextAlign};

use super::layout::{RectF, SizeF};

/// Windows mode the taskbar follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    /// Default text color on a taskbar in this mode.
    pub fn text_color(self) -> Color {
        match self {
            Self::Dark => Color {
                a: 255,
                r: 0xFF,
                g: 0xFF,
                b: 0xFF,
            },
            Self::Light => Color {
                a: 255,
                r: 0x1B,
                g: 0x1B,
                b: 0x1B,
            },
        }
    }
}

/// Relative luminance (WCAG 2) of an sRGB color, 0 (black) … 1 (white).
pub fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    fn linear(c: u8) -> f64 {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

impl Theme {
    /// Background of a hovered clickable part (like taskbar buttons: a light veil on dark
    /// taskbars, a dark one on light taskbars).
    pub fn hover_color(self) -> Color {
        match self {
            Self::Dark => Color {
                a: 0x1A,
                r: 0xFF,
                g: 0xFF,
                b: 0xFF,
            },
            Self::Light => Color {
                a: 0x12,
                r: 0x00,
                g: 0x00,
                b: 0x00,
            },
        }
    }

    /// The theme whose default text color has the higher contrast ratio on a background of
    /// relative luminance `l` (white text on dark backgrounds, `#1B1B1B` on light ones).
    pub fn for_background(l: f64) -> Self {
        let contrast = |c: Color| {
            let lc = relative_luminance(c.r, c.g, c.b);
            (l.max(lc) + 0.05) / (l.min(lc) + 0.05)
        };
        if contrast(Self::Light.text_color()) > contrast(Self::Dark.text_color()) {
            Self::Light
        } else {
            Self::Dark
        }
    }
}

/// Font and paragraph settings of a piece of text.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    pub family: String,
    /// In DIPs.
    pub size: f32,
    pub weight: FontWeight,
    pub align: TextAlign,
}

/// Drawing backend (Direct2D/DirectWrite on Windows). All sizes are DIPs; drawing calls are only
/// valid while the platform layer renders one widget surface.
pub trait Renderer {
    /// Natural size of `text` without wrapping (trailing whitespace included).
    fn measure_text(&mut self, text: &str, style: &TextStyle) -> SizeF;
    /// Draws `text` inside `rect`: no wrap, vertically centered, horizontal alignment from
    /// `style`, trimmed with an ellipsis if it does not fit.
    fn draw_text(&mut self, text: &str, style: &TextStyle, color: Color, rect: RectF);
    /// Fills a rounded rectangle (hover background).
    fn fill_rounded_rect(&mut self, rect: RectF, radius: f32, color: Color);
}

/// Data that changes without a config change (sent to the overlay thread on its own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveData {
    /// Pending to-do items; `None` when unknown (to-do off, `todos.json` invalid).
    pub todo_pending: Option<usize>,
}

/// What draws a widget: the taskbar theme, live data and the hovered module (bars only).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DrawContext {
    pub theme: Theme,
    pub live: LiveData,
    pub hover: Option<usize>,
}

/// What a widget shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(text::TextContent),
    Bar(bar::BarContent),
}

impl Content {
    /// Natural size in DIPs (used by `auto` width/height).
    pub fn measure(&self, r: &mut dyn Renderer, live: &LiveData) -> SizeF {
        match self {
            Self::Text(t) => t.measure(r),
            Self::Bar(b) => b.measure(r, live),
        }
    }

    /// Clickable parts, left to right, in DIPs from the box's left edge (empty for text).
    pub fn spans(&self, r: &mut dyn Renderer, live: &LiveData) -> Vec<bar::Span> {
        match self {
            Self::Text(_) => Vec::new(),
            Self::Bar(b) => b.spans(r, live),
        }
    }

    /// Draws the content in a box of `size` DIPs.
    pub fn draw(&self, r: &mut dyn Renderer, size: SizeF, cx: &DrawContext) {
        match self {
            Self::Text(t) => t.draw(r, size, cx.theme),
            Self::Bar(b) => b.draw(r, size, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luminance_bounds() {
        assert_eq!(relative_luminance(0, 0, 0), 0.0);
        assert!((relative_luminance(255, 255, 255) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn theme_follows_the_background() {
        let theme = |r, g, b| Theme::for_background(relative_luminance(r, g, b));
        // Windows 11 dark and light taskbars.
        assert_eq!(theme(28, 28, 28), Theme::Dark);
        assert_eq!(theme(238, 238, 238), Theme::Light);
        // Accent-colored taskbar (default blue) and other mid tones.
        assert_eq!(theme(0, 120, 215), Theme::Dark);
        assert_eq!(theme(255, 185, 0), Theme::Light);
        assert_eq!(theme(128, 128, 128), Theme::Light);
        assert_eq!(theme(100, 100, 100), Theme::Dark);
    }
}
