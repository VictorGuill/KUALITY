//! The windows' look: the gpui-component theme as it comes, following the Windows light / dark
//! mode. Nothing is overridden (colors, radius and font are the theme's), so a theme chosen
//! later changes everything at once. The only thing done here is painting the native title bar
//! in the theme's colors.

use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::*;

/// Follows the Windows light / dark mode and paints the title bar to match. Call it again on
/// every appearance change.
pub fn apply(window: &mut Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
    let theme = cx.theme();
    crate::platform::set_caption_colors(window, rgb24(theme.background), rgb24(theme.foreground));
}

/// A theme color as `0xRRGGBB` (alpha dropped).
fn rgb24(color: Hsla) -> u32 {
    let c = Rgba::from(color);
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    (byte(c.r) << 16) | (byte(c.g) << 8) | byte(c.b)
}
