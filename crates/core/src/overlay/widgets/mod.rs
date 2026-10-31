//! Widget contents: what each widget type measures and draws, through the [`Renderer`] trait.

pub mod text;

use kuality_protocol::{Color, FontWeight, TextAlign};

use super::layout::SizeF;

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
    /// Draws `text` inside a box of `size` at (0, 0): no wrap, vertically centered, horizontal
    /// alignment from `style`, trimmed with an ellipsis if it does not fit.
    fn draw_text(&mut self, text: &str, style: &TextStyle, color: Color, size: SizeF);
}

/// What a widget shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(text::TextContent),
}

impl Content {
    /// Natural size in DIPs (used by `auto` width/height).
    pub fn measure(&self, r: &mut dyn Renderer) -> SizeF {
        match self {
            Self::Text(t) => t.measure(r),
        }
    }

    /// Draws the content in a box of `size` DIPs.
    pub fn draw(&self, r: &mut dyn Renderer, size: SizeF, theme: Theme) {
        match self {
            Self::Text(t) => t.draw(r, size, theme),
        }
    }
}
