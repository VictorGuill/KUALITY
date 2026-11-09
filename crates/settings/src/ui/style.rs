//! The window's look: code-side overrides of the gpui-component theme (radius, font, palette).
//! Still follows the Windows light/dark mode; there are no user themes.

use std::sync::OnceLock;

use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::*;

/// Corner radius of buttons, inputs and other small controls.
pub(crate) const RADIUS: f32 = 8.;
/// Corner radius of the content panel, cards, dialogs and popovers.
pub const RADIUS_LG: f32 = 12.;

/// Fonts tried in order; the first installed one is used (Windows 11 ships the first).
const FONTS: [&str; 2] = ["Segoe UI Variable Text", "Segoe UI Variable"];

/// One color mode.
struct Palette {
    /// Window and sidebar background, behind the content panel; also the title bar.
    window: u32,
    /// Title bar text.
    caption_text: u32,
    /// Content panel; also the theme background (inputs, lists).
    panel: u32,
    /// Cards inside the panel.
    card: u32,
    /// Active sidebar item.
    sidebar_active: u32,
    /// Hairlines: panel and card outlines, dividers (RGBA).
    border: u32,
    /// Input and default button outlines (RGBA).
    input: u32,
    /// Filled (secondary) buttons: normal, hover, pressed.
    button: [u32; 3],
    /// Segmented control: track and selected item.
    segment: [u32; 2],
}

const DARK: Palette = Palette {
    window: 0x0f0f0f,
    caption_text: 0xffffff,
    panel: 0x171717,
    card: 0x1e1e1e,
    sidebar_active: 0x242424,
    border: 0xffffff12,
    input: 0xffffff1f,
    button: [0x2a2a2a, 0x333333, 0x252525],
    segment: [0x141414, 0x303030],
};

const LIGHT: Palette = Palette {
    window: 0xf3f3f3,
    caption_text: 0x1a1a1a,
    panel: 0xffffff,
    card: 0xf7f7f7,
    sidebar_active: 0xe3e3e3,
    border: 0x00000012,
    input: 0x0000001c,
    button: [0xececec, 0xe4e4e4, 0xdcdcdc],
    segment: [0xe9e9e9, 0xffffff],
};

fn palette(cx: &App) -> &'static Palette {
    if cx.theme().mode.is_dark() {
        &DARK
    } else {
        &LIGHT
    }
}

/// Window and sidebar background.
pub fn window_bg(cx: &App) -> Hsla {
    rgb(palette(cx).window).into()
}

/// Background of a card ([`super::controls::section`]).
pub fn card_bg(cx: &App) -> Hsla {
    rgb(palette(cx).card).into()
}

/// Segmented control colors: (track, selected item).
pub fn segment_colors(cx: &App) -> (Hsla, Hsla) {
    let [track, thumb] = palette(cx).segment;
    (rgb(track).into(), rgb(thumb).into())
}

/// Follows the Windows light/dark mode, then applies Kuality's overrides on top and paints the
/// title bar to match. Call it again on every appearance change: a mode switch reloads the stock
/// colors.
pub fn apply(window: &mut Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
    let font = font_family(cx);
    let p = palette(cx);
    crate::platform::set_caption_colors(window, p.window, p.caption_text);
    Theme::update(cx, |t| {
        t.radius = px(RADIUS);
        t.radius_lg = px(RADIUS_LG);
        t.shadow = false;
        if let Some(font) = font {
            t.font_family = font;
        }

        let window: Hsla = rgb(p.window).into();
        let panel: Hsla = rgb(p.panel).into();
        let card: Hsla = rgb(p.card).into();
        let border: Hsla = rgba(p.border).into();
        let [button, button_hover, button_active] = p.button.map(|c| Hsla::from(rgb(c)));

        t.background = panel;
        t.popover = card;
        t.border = border;
        t.input = rgba(p.input).into();
        t.sidebar = window;
        t.sidebar_border = transparent_black();
        t.sidebar_accent = rgb(p.sidebar_active).into();
        t.secondary = button;
        t.secondary_hover = button_hover;
        t.secondary_active = button_active;
        t.button_secondary = button;
        t.button_secondary_hover = button_hover;
        t.button_secondary_active = button_active;
    });
}

/// The first installed family of [`FONTS`], looked up once.
fn font_family(cx: &App) -> Option<SharedString> {
    static FONT: OnceLock<Option<SharedString>> = OnceLock::new();
    FONT.get_or_init(|| {
        let installed = cx.text_system().all_font_names();
        let found = FONTS
            .iter()
            .find(|f| installed.iter().any(|i| i == *f))
            .map(|f| SharedString::from(*f));
        match &found {
            Some(f) => log::debug!("UI font: {f}"),
            None => log::info!("Segoe UI Variable not installed; using the system UI font"),
        }
        found
    })
    .clone()
}
