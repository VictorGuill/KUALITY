//! What the three GPUI windows (settings, to-do popup, clipboard popup) share: the asset source,
//! opening the one window of the process, the popup window options and following the core.

use std::time::Duration;

use gpui_kit::*;

use super::style;
use crate::platform;

/// Focus changes right after a popup opens are ignored (the window may not be active yet).
pub(crate) const ACTIVATION_GRACE: Duration = Duration::from_millis(400);

// Lucide icons beyond gpui-component's default set, embedded one by one.
gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        AppWindow, Clipboard, Keyboard, ListTodo, Pin, PinOff, Trash, Type
    ]
);

/// gpui-component's default icons plus [`ExtraIcons`].
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = ExtraIcons.list(path)?;
        all.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(all)
    }
}

/// Runs the GPUI app of this process: one window, until it closes.
///
/// `options` runs once GPUI is up (screen queries then see physical pixels); `None` quits without
/// a window. `prepare` runs on the new window before its view is built; the theme is applied and
/// kept in sync with the Windows light / dark mode for every window.
pub(crate) fn run_window<V: Render + 'static>(
    what: &'static str,
    options: impl FnOnce(&mut App) -> Option<WindowOptions> + 'static,
    prepare: impl FnOnce(&mut Window) + 'static,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
) {
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let Some(options) = options(cx) else {
                cx.quit();
                return;
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                style::apply(window, cx);
                window.observe_window_appearance(style::apply).detach();
                prepare(window);
                cx.new(|cx| build(window, cx))
            });
            if let Err(e) = opened {
                log::error!("cannot open the {what}: {e}");
                cx.quit();
                return;
            }
            cx.on_window_closed(|cx, _| cx.quit()).detach();
        });
}

/// Options of a popup window (to-do, clipboard): borderless, fixed, placed at `bounds` on the
/// display `display_id`; `focus`: whether it takes the keyboard focus when it opens.
pub(crate) fn popup_options(
    title: &'static str,
    app_id: &'static str,
    bounds: Bounds<Pixels>,
    display_id: Option<DisplayId>,
    focus: bool,
) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: true,
            ..Default::default()
        }),
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        focus,
        show: true,
        display_id,
        app_id: Some(app_id.into()),
        ..Default::default()
    }
}

/// Bounds (DIPs) and display for a popup of `size` DIPs whose top-left corner is at `origin`
/// (physical pixels) on `monitor`; centered on the primary display when the monitor is unknown.
pub(crate) fn popup_bounds(
    monitor: Option<platform::Monitor>,
    origin: Option<(i32, i32)>,
    size: Size<Pixels>,
    cx: &App,
) -> (Bounds<Pixels>, Option<DisplayId>) {
    match (monitor, origin) {
        (Some(m), Some((x, y))) => (
            Bounds::new(point(px(x as f32 / m.scale), px(y as f32 / m.scale)), size),
            Some(DisplayId::new(m.id)),
        ),
        _ => (Bounds::centered(None, size, cx), None),
    }
}

/// Calls `on_exit` on the view once the core process ends (tray Quit, `taskkill`, crash): no
/// window outlives the core.
pub(crate) fn watch_core<V: 'static>(
    core: platform::CoreProcess,
    cx: &mut Context<V>,
    on_exit: impl FnOnce(&mut V, &mut Context<V>) + 'static,
) {
    match core.watch() {
        Ok(exited) => cx
            .spawn(async move |this, cx| {
                if exited.await.is_ok() {
                    let _ = this.update(cx, on_exit);
                }
            })
            .detach(),
        Err(e) => log::error!("cannot watch the Kuality process: {e}"),
    }
}
