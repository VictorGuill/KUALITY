//! The to-do popup (`kuality.exe --todo`, see `docs/specs/todo.md`): a small borderless window
//! above the taskbar listing `todos.json`. It is the only writer of that file; every change is
//! written at once so the taskbar count follows. It closes on Esc or when it loses focus.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::{Config, NewItems, TodoList, store};

use crate::platform::{self, TODO_WINDOW_TITLE};
use crate::ui::{AppAssets, style};

/// Popup size in DIPs.
const WIDTH: f32 = 340.0;
const HEIGHT: f32 = 440.0;
/// Space between the popup and the taskbar / screen edges, in DIPs.
const MARGIN: f32 = 8.0;
/// Focus changes right after opening are ignored (the window may not be active yet).
const ACTIVATION_GRACE: Duration = Duration::from_millis(400);

/// A screen rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Top-left corner (physical px) of a popup of `size` DIPs on a monitor with work area `work`
/// and `scale` (DPI / 96). `anchor` is where the bar module was clicked: its left edge at the
/// taskbar's top edge. The popup's left edge follows the anchor, its bottom sits `MARGIN` above
/// it, and it stays inside the work area. Without an anchor it goes to the bottom-left corner.
pub fn place_popup(
    anchor: Option<(i32, i32)>,
    work: ScreenRect,
    scale: f32,
    size: (f32, f32),
) -> (i32, i32) {
    let (w, h) = (
        (size.0 * scale).round() as i32,
        (size.1 * scale).round() as i32,
    );
    let margin = (MARGIN * scale).round() as i32;
    let (x, bottom) = match anchor {
        Some((x, y)) => (x, y.min(work.bottom) - margin),
        None => (work.left + margin, work.bottom - margin),
    };
    let max_left = (work.right - margin - w).max(work.left);
    let left = x.clamp(work.left + margin.min(work.right - work.left), max_left);
    let top = (bottom - h).max(work.top + margin);
    (left, top)
}

/// `--at <x>,<y>`: the anchor the core passes (physical screen pixels).
pub fn anchor_arg(args: &[String]) -> Option<(i32, i32)> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let value = match a.strip_prefix("--at=") {
            Some(v) => v,
            None if a == "--at" => it.next()?,
            None => continue,
        };
        let (x, y) = value.split_once(',')?;
        return Some((x.trim().parse().ok()?, y.trim().parse().ok()?));
    }
    None
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Reads `todos.json`: a missing file is an empty list; an invalid one is an error (the popup
/// then never writes, so nothing is lost).
pub fn load(path: &Path) -> Result<TodoList, String> {
    match std::fs::read(path) {
        Ok(bytes) => TodoList::from_json(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TodoList::default()),
        Err(e) => Err(e.to_string()),
    }
}

/// `todo.new_items` from `config.json` (top when unreadable).
fn new_items_at_top(config_path: &Path) -> bool {
    std::fs::read(config_path)
        .ok()
        .and_then(|b| Config::from_json(&b).ok())
        .and_then(|c| c.todo.new_items.valid().copied())
        != Some(NewItems::Bottom)
}

/// Opens the popup and runs until it closes (Esc, focus lost, WM_CLOSE from the core, or the
/// core exiting).
pub fn run(
    todo_path: PathBuf,
    config_path: PathBuf,
    anchor: Option<(i32, i32)>,
    core: Option<platform::CoreProcess>,
) {
    let at_top = new_items_at_top(&config_path);
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let monitor = platform::monitor_at(anchor);
            let (bounds, display_id) = match monitor {
                Some(m) => {
                    let (x, y) = place_popup(anchor, m.work, m.scale, (WIDTH, HEIGHT));
                    let origin = point(px(x as f32 / m.scale), px(y as f32 / m.scale));
                    (
                        Bounds::new(origin, size(px(WIDTH), px(HEIGHT))),
                        Some(DisplayId::new(m.id)),
                    )
                }
                None => (
                    Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx),
                    None,
                ),
            };
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(TODO_WINDOW_TITLE.into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                kind: WindowKind::PopUp,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                focus: true,
                show: true,
                display_id,
                app_id: Some("Kuality.Todo".into()),
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                style::apply(window, cx);
                platform::popup_window_setup(window);
                cx.new(|cx| TodoPopup::new(todo_path, at_top, core, window, cx))
            });
            if let Err(e) = opened {
                log::error!("cannot open the to-do popup: {e}");
                cx.quit();
                return;
            }
            cx.on_window_closed(|cx, _| cx.quit()).detach();
        });
}

struct TodoPopup {
    path: PathBuf,
    list: TodoList,
    /// `todos.json` is invalid: shown, and nothing is written.
    load_error: Option<String>,
    write_error: Option<String>,
    at_top: bool,
    input: Entity<InputState>,
    opened: Instant,
    _subs: Vec<Subscription>,
}

impl TodoPopup {
    fn new(
        path: PathBuf,
        at_top: bool,
        core: Option<platform::CoreProcess>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (list, load_error) = match load(&path) {
            Ok(l) => (l, None),
            Err(e) => {
                log::warn!("todos.json is invalid: {e}; read-only");
                (TodoList::default(), Some(e))
            }
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Add a task…"));
        input.update(cx, |s, cx| s.focus(window, cx));
        if let Some(core) = core {
            match core.watch() {
                Ok(exited) => cx
                    .spawn(async move |_, cx| {
                        if exited.await.is_ok() {
                            log::info!("Kuality exited; closing the to-do popup");
                            cx.update(|cx| cx.quit());
                        }
                    })
                    .detach(),
                Err(e) => log::error!("cannot watch the Kuality process: {e}"),
            }
        }
        let subs = vec![
            cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.add(window, cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() && this.opened.elapsed() >= ACTIVATION_GRACE {
                    log::debug!("to-do popup lost focus; closing");
                    cx.quit();
                }
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                style::apply(window, cx);
            }),
        ];
        Self {
            path,
            list,
            load_error,
            write_error: None,
            at_top,
            input,
            opened: Instant::now(),
            _subs: subs,
        }
    }

    fn read_only(&self) -> bool {
        self.load_error.is_some()
    }

    /// Writes the list at once (atomic); the core's watcher updates the taskbar count.
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.read_only() {
            return;
        }
        let written = self
            .list
            .to_json_pretty()
            .map_err(std::io::Error::other)
            .and_then(|bytes| store::write_atomic(&self.path, &bytes));
        self.write_error = written.err().map(|e| {
            log::error!("cannot write {}: {e}", self.path.display());
            e.to_string()
        });
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only() {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if self.list.add(&text, self.at_top, now()).is_some() {
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
            self.save(cx);
        }
    }

    fn set_done(&mut self, id: u64, done: bool, cx: &mut Context<Self>) {
        if !self.read_only() && self.list.set_done(id, done, now()) {
            self.save(cx);
        }
    }

    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.read_only() && self.list.remove(id) {
            self.save(cx);
        }
    }

    fn render_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ro = self.read_only();
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.secondary);
        self.list
            .display_order()
            .into_iter()
            .map(|item| {
                let (id, done) = (item.id, item.done);
                h_flex()
                    .id(("todo", id))
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded(px(style::RADIUS))
                    .hover(move |s| s.bg(hover))
                    .child(
                        Checkbox::new(("todo-done", id))
                            .accessibility_label(if done { "Mark as not done" } else { "Done" })
                            .checked(done)
                            .disabled(ro)
                            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                this.set_done(id, *on, cx)
                            })),
                    )
                    .child(
                        div()
                            .id(("todo-text", id))
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .when(done, |d| d.line_through().text_color(muted))
                            .when(!ro, |d| {
                                d.cursor_pointer().on_click(
                                    cx.listener(move |this, _, _, cx| this.set_done(id, !done, cx)),
                                )
                            })
                            .child(item.text.clone()),
                    )
                    .child(
                        Button::new(("todo-delete", id))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::Trash).text_color(muted))
                            .tooltip("Delete")
                            .disabled(ro)
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(id, cx))),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for TodoPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.render_rows(cx);
        let theme = cx.theme();
        let pending = self.list.pending();
        let ro = self.read_only();
        let error = self
            .load_error
            .as_ref()
            .map(|e| format!("todos.json has an error: {e}. Fix the file to edit the list."))
            .or_else(|| {
                self.write_error
                    .as_ref()
                    .map(|e| format!("Could not save: {e}"))
            });
        v_flex()
            .id("todo-popup")
            .key_context("TodoPopup")
            .capture_action(cx.listener(|_, _: &Escape, _, cx| cx.quit()))
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.quit();
                }
            }))
            .size_full()
            .gap_3()
            .p_3()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .justify_between()
                    .px_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("To do"),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).child(
                        match pending {
                            _ if ro => String::new(),
                            0 => "All done".to_string(),
                            1 => "1 left".to_string(),
                            n => format!("{n} left"),
                        },
                    )),
            )
            .child(
                Input::new(&self.input)
                    .prefix(
                        Icon::new(IconName::Plus)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .disabled(ro),
            )
            .when_some(error, |this, e| {
                this.child(div().px_1().text_xs().text_color(theme.danger).child(e))
            })
            .child(
                div()
                    .id("todo-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(if rows.is_empty() && !ro {
                        div()
                            .pt_8()
                            .w_full()
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Nothing to do")
                            .into_any_element()
                    } else {
                        v_flex().gap_0p5().children(rows).into_any_element()
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it would bring GPUI's `test` attribute in place of the built-in one.
    use super::{ScreenRect, TodoList, anchor_arg, load, new_items_at_top, place_popup};

    const WORK: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1032,
    };

    #[test]
    fn popup_sits_above_the_anchor() {
        // 100 %: 340 x 440, 8 px margin.
        assert_eq!(
            place_popup(Some((12, 1032)), WORK, 1.0, (340.0, 440.0)),
            (12, 1032 - 8 - 440)
        );
        // 150 %: 510 x 660, 12 px margin.
        assert_eq!(
            place_popup(Some((100, 1032)), WORK, 1.5, (340.0, 440.0)),
            (100, 1032 - 12 - 660)
        );
    }

    #[test]
    fn popup_stays_inside_the_work_area() {
        assert_eq!(
            place_popup(Some((1800, 1032)), WORK, 1.0, (340.0, 440.0)).0,
            1920 - 8 - 340
        );
        assert_eq!(
            place_popup(Some((-50, 1032)), WORK, 1.0, (340.0, 440.0)).0,
            8
        );
        let short = ScreenRect {
            bottom: 300,
            ..WORK
        };
        assert_eq!(place_popup(Some((0, 300)), short, 1.0, (340.0, 440.0)).1, 8);
        // Anchor below the work area (taskbar top) is clamped to it.
        assert_eq!(
            place_popup(Some((0, 1080)), WORK, 1.0, (340.0, 440.0)).1,
            1032 - 8 - 440
        );
    }

    #[test]
    fn without_anchor_bottom_left() {
        assert_eq!(
            place_popup(None, WORK, 1.0, (340.0, 440.0)),
            (8, 1032 - 8 - 440)
        );
    }

    #[test]
    fn anchor_argument() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(anchor_arg(&args(&["--at", "12,1032"])), Some((12, 1032)));
        assert_eq!(
            anchor_arg(&args(&["--core-pid", "1", "--at=-5, 7"])),
            Some((-5, 7))
        );
        assert_eq!(anchor_arg(&args(&["--at", "x,1"])), None);
        assert_eq!(anchor_arg(&args(&["--at"])), None);
        assert_eq!(anchor_arg(&args(&[])), None);
    }

    #[test]
    fn missing_file_is_empty_and_invalid_is_an_error() {
        let dir = kuality_protocol::test_util::TempDir::new("todo-load");
        let path = dir.0.join("todos.json");
        assert_eq!(load(&path), Ok(TodoList::default()));
        std::fs::write(&path, b"{ nope").unwrap();
        assert!(load(&path).is_err());
        std::fs::write(&path, br#"{ "items": [{ "id": 1, "text": "a" }] }"#).unwrap();
        assert_eq!(load(&path).unwrap().pending(), 1);
    }

    #[test]
    fn new_items_setting() {
        let dir = kuality_protocol::test_util::TempDir::new("todo-config");
        let path = dir.0.join("config.json");
        assert!(new_items_at_top(&path), "missing file: top");
        std::fs::write(&path, br#"{ "todo": { "new_items": "bottom" } }"#).unwrap();
        assert!(!new_items_at_top(&path));
        std::fs::write(&path, br#"{ "todo": { "new_items": "side" } }"#).unwrap();
        assert!(new_items_at_top(&path));
    }
}
