//! The clipboard history popup (`kuality.exe --clipboard`, see `docs/specs/clipboard.md`):
//! search, list and preview of the history the core keeps in memory, read over the core's pipe.
//! Enter pastes into the window that was in front when the hotkey fired (`--paste`), Shift+Enter
//! only copies. Nothing is written to disk; decrypted data lives only in this process while it
//! is open.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::clipboard::{EntryInfo, EntryKind, Request};

use crate::platform::{self, CLIPBOARD_WINDOW_TITLE};
use crate::todo::ScreenRect;
use crate::ui::{AppAssets, style};

/// Popup size in DIPs.
const WIDTH: f32 = 720.0;
const HEIGHT: f32 = 460.0;
/// Focus changes right after opening are ignored (the window may not be active yet).
const ACTIVATION_GRACE: Duration = Duration::from_millis(400);
/// While the popup is not in front yet, it retries taking the foreground this often…
const FOREGROUND_RETRY: Duration = Duration::from_millis(100);
/// …this many times (the user may still be releasing the hotkey).
const FOREGROUND_TRIES: u32 = 15;
/// Width of the list column in DIPs.
const LIST_WIDTH: f32 = 300.0;

/// Top-left corner (physical px) of a popup of `size` DIPs centered in `work` at `scale`.
pub fn center_popup(work: ScreenRect, scale: f32, size: (f32, f32)) -> (i32, i32) {
    let (w, h) = (
        (size.0 * scale).round() as i32,
        (size.1 * scale).round() as i32,
    );
    let left = work.left + ((work.right - work.left - w) / 2).max(0);
    let top = work.top + ((work.bottom - work.top - h) / 2).max(0);
    (left, top)
}

/// `"now"`, `"5 min"`, `"3 h"`, `"yesterday"`, `"4 d"`: how long ago `then` was (Unix seconds).
pub fn ago(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{} min", s / 60),
        3600..86_400 => format!("{} h", s / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => format!("{} d", s / 86_400),
    }
}

/// Whether `item` matches the (lowercase) query: a substring of its text; images match "image".
fn matches(item: &Item, query: &str) -> bool {
    query.is_empty() || item.search.contains(query)
}

/// First non-empty line of `text`, trimmed (list rows).
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// One entry of the list.
struct Item {
    info: EntryInfo,
    thumb: Option<Arc<Image>>,
    /// Lowercase text to search in.
    search: String,
}

/// What the core said when the popup asked for the list.
enum Load {
    Ready(Vec<Item>),
    /// No pipe: the core is not running or the history is off.
    Off,
    Error(String),
}

fn load(core_pid: Option<u32>) -> Load {
    let Some(pid) = core_pid else {
        return Load::Off;
    };
    match platform::pipe_request(pid, Request::List) {
        Ok((response, payload)) if response.ok => {
            let mut at = 0usize;
            let items = response
                .entries
                .into_iter()
                .map(|info| {
                    let len = info.thumb_len as usize;
                    let thumb = (len > 0 && at + len <= payload.len()).then(|| {
                        Arc::new(Image::from_bytes(
                            ImageFormat::Png,
                            payload[at..at + len].to_vec(),
                        ))
                    });
                    at += len;
                    let search = match info.kind {
                        EntryKind::Text => info.text.as_deref().unwrap_or("").to_lowercase(),
                        EntryKind::Image => "image".into(),
                    };
                    Item {
                        info,
                        thumb,
                        search,
                    }
                })
                .collect();
            Load::Ready(items)
        }
        Ok((response, _)) => Load::Error(response.error.unwrap_or_default()),
        Err(platform::PipeError::NotRunning) => Load::Off,
        Err(platform::PipeError::Other(e)) => Load::Error(e),
    }
}

/// Opens the popup and runs until it closes (Esc, focus lost, an entry chosen, WM_CLOSE from
/// the core, or the core exiting).
pub fn run(core: Option<platform::CoreProcess>, paste: bool) {
    let core_pid = core.as_ref().map(|c| c.pid);
    let loaded = load(core_pid);
    // Before GPUI takes the focus: the window the user was in.
    let anchor = platform::foreground_center();
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let monitor = platform::monitor_at(anchor.or_else(platform::cursor_pos));
            let (bounds, display_id) = match monitor {
                Some(m) => {
                    let (x, y) = center_popup(m.work, m.scale, (WIDTH, HEIGHT));
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
                    title: Some(CLIPBOARD_WINDOW_TITLE.into()),
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
                app_id: Some("Kuality.Clipboard".into()),
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                style::apply(window, cx);
                platform::popup_window_setup(window, true);
                cx.new(|cx| ClipboardPopup::new(loaded, core_pid, core, paste, window, cx))
            });
            if let Err(e) = opened {
                log::error!("cannot open the clipboard popup: {e}");
                cx.quit();
                return;
            }
            cx.on_window_closed(|cx, _| cx.quit()).detach();
        });
}

/// The selected entry's full content (fetched with `get` when the list only has a part).
enum Preview {
    None,
    Text(u64, String),
    Image(u64, Arc<Image>),
}

struct ClipboardPopup {
    core_pid: Option<u32>,
    /// Opened by the hotkey: Enter pastes into the previous window (else it copies).
    paste: bool,
    items: Vec<Item>,
    off: bool,
    error: Option<String>,
    input: Entity<InputState>,
    query: String,
    selected: Option<u64>,
    preview: Preview,
    scroll: ScrollHandle,
    opened: Instant,
    _subs: Vec<Subscription>,
}

impl ClipboardPopup {
    fn new(
        loaded: Load,
        core_pid: Option<u32>,
        core: Option<platform::CoreProcess>,
        paste: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search clipboard…"));
        input.update(cx, |s, cx| s.focus(window, cx));
        if let Some(core) = core {
            match core.watch() {
                Ok(exited) => cx
                    .spawn(async move |_, cx| {
                        if exited.await.is_ok() {
                            log::info!("Kuality exited; closing the clipboard popup");
                            cx.update(|cx| cx.quit());
                        }
                    })
                    .detach(),
                Err(e) => log::error!("cannot watch the Kuality process: {e}"),
            }
        }
        // Opened by a hotkey, the window may appear without the focus (Windows' foreground
        // lock): keep asking for it until it is in front.
        cx.spawn_in(window, async move |_, cx| {
            for _ in 0..FOREGROUND_TRIES {
                cx.background_executor().timer(FOREGROUND_RETRY).await;
                let active = cx.update(|window, _| {
                    window.is_window_active() || platform::bring_to_front(window)
                });
                if active.unwrap_or(true) {
                    break;
                }
            }
        })
        .detach();
        let subs = vec![
            cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.query = input.read(cx).value().trim().to_lowercase();
                    this.selected = this.visible().first().map(|i| i.info.id);
                    this.scroll.scroll_to_item(0);
                    this.load_preview();
                    cx.notify();
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() && this.opened.elapsed() >= ACTIVATION_GRACE {
                    log::debug!("clipboard popup lost focus; closing");
                    cx.quit();
                }
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                style::apply(window, cx);
            }),
        ];
        let mut this = Self {
            core_pid,
            paste,
            items: Vec::new(),
            off: false,
            error: None,
            input,
            query: String::new(),
            selected: None,
            preview: Preview::None,
            scroll: ScrollHandle::new(),
            opened: Instant::now(),
            _subs: subs,
        };
        this.set_loaded(loaded);
        this
    }

    fn set_loaded(&mut self, loaded: Load) {
        match loaded {
            Load::Ready(items) => {
                self.items = items;
                self.off = false;
            }
            Load::Off => self.off = true,
            Load::Error(e) => self.error = Some(e),
        }
        let visible: Vec<u64> = self.visible().iter().map(|i| i.info.id).collect();
        if self.selected.is_none_or(|id| !visible.contains(&id)) {
            self.selected = visible.first().copied();
        }
        self.load_preview();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let loaded = load(self.core_pid);
        self.set_loaded(loaded);
        cx.notify();
    }

    fn visible(&self) -> Vec<&Item> {
        self.items
            .iter()
            .filter(|i| matches(i, &self.query))
            .collect()
    }

    fn selected_item(&self) -> Option<&Item> {
        let id = self.selected?;
        self.items.iter().find(|i| i.info.id == id)
    }

    /// Fetches the selected entry's full text or image when the list does not have it.
    fn load_preview(&mut self) {
        let Some(item) = self.selected_item() else {
            self.preview = Preview::None;
            return;
        };
        let id = item.info.id;
        let have = match &self.preview {
            Preview::Text(p, _) | Preview::Image(p, _) => *p == id,
            Preview::None => false,
        };
        if have {
            return;
        }
        let needs_get = item.info.kind == EntryKind::Image || item.info.truncated;
        if !needs_get {
            self.preview = Preview::Text(id, item.info.text.clone().unwrap_or_default());
            return;
        }
        let Some(pid) = self.core_pid else {
            return;
        };
        self.preview = match platform::pipe_request(pid, Request::Get { id }) {
            Ok((r, payload)) if r.ok => match r.entries.first().map(|e| e.kind) {
                Some(EntryKind::Image) => {
                    Preview::Image(id, Arc::new(Image::from_bytes(ImageFormat::Png, payload)))
                }
                _ => Preview::Text(id, String::from_utf8_lossy(&payload).into_owned()),
            },
            _ => Preview::None,
        };
    }

    fn select(&mut self, id: u64, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.load_preview();
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self.visible().iter().map(|i| i.info.id).collect();
        if ids.is_empty() {
            return;
        }
        let at = self
            .selected
            .and_then(|id| ids.iter().position(|&i| i == id))
            .unwrap_or(0) as isize;
        let next = (at + delta).clamp(0, ids.len() as isize - 1) as usize;
        self.scroll.scroll_to_item(next);
        self.select(ids[next], cx);
    }

    /// Puts the entry on the clipboard; pastes it into the previous window when `paste`.
    fn choose(&mut self, id: u64, paste: bool, cx: &mut Context<Self>) {
        let Some(pid) = self.core_pid else {
            return;
        };
        let request = if paste && self.paste {
            Request::Paste { id }
        } else {
            Request::Copy { id }
        };
        match platform::pipe_request(pid, request) {
            Ok((r, _)) if r.ok => cx.quit(),
            Ok((r, _)) => {
                self.error = r.error;
                self.reload(cx);
            }
            Err(e) => {
                self.error = Some(e.to_string());
                cx.notify();
            }
        }
    }

    /// Pin / unpin / delete, then reload the list.
    fn command(&mut self, request: Request, cx: &mut Context<Self>) {
        let Some(pid) = self.core_pid else {
            return;
        };
        self.error = match platform::pipe_request(pid, request) {
            Ok((r, _)) if r.ok => None,
            Ok((r, _)) => r.error,
            Err(e) => Some(e.to_string()),
        };
        self.reload(cx);
    }

    fn toggle_pin(&mut self, id: u64, cx: &mut Context<Self>) {
        let pinned = self
            .items
            .iter()
            .find(|i| i.info.id == id)
            .is_some_and(|i| i.info.pinned);
        let request = if pinned {
            Request::Unpin { id }
        } else {
            Request::Pin { id }
        };
        self.command(request, cx);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = k.modifiers;
        match k.key.as_str() {
            "escape" => cx.quit(),
            "up" => self.move_selection(-1, cx),
            "down" => self.move_selection(1, cx),
            "pageup" => self.move_selection(-8, cx),
            "pagedown" => self.move_selection(8, cx),
            "enter" => {
                if let Some(id) = self.selected {
                    self.choose(id, !m.shift, cx);
                }
            }
            "p" if m.control => {
                if let Some(id) = self.selected {
                    self.toggle_pin(id, cx);
                }
            }
            "delete" | "backspace" if m.control => {
                if let Some(id) = self.selected {
                    self.command(Request::Delete { id }, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, hover, active) = (theme.muted_foreground, theme.secondary, theme.accent);
        let now = now();
        let mut rows = Vec::new();
        let mut pinned_header = false;
        let mut recent_header = false;
        for item in self.visible() {
            let info = &item.info;
            let id = info.id;
            let selected = self.selected == Some(id);
            let header = if info.pinned && !pinned_header {
                pinned_header = true;
                Some("Pinned")
            } else if !info.pinned && !recent_header && pinned_header {
                recent_header = true;
                Some("Recent")
            } else {
                None
            };
            let label: SharedString = match info.kind {
                EntryKind::Text => first_line(info.text.as_deref().unwrap_or_default()).into(),
                EntryKind::Image => format!("Image {} × {}", info.width, info.height).into(),
            };
            let leading = match &item.thumb {
                Some(t) => img(t.clone())
                    .size(px(24.))
                    .object_fit(ObjectFit::Contain)
                    .rounded(px(4.))
                    .into_any_element(),
                None => Icon::new(Lucide::Type)
                    .small()
                    .text_color(muted)
                    .into_any_element(),
            };
            rows.push(
                v_flex()
                    .id(("clip-row", id))
                    .when_some(header, |d, h| {
                        d.child(
                            div()
                                .px_2()
                                .pt_2()
                                .pb_1()
                                .text_xs()
                                .text_color(muted)
                                .child(h),
                        )
                    })
                    .child(
                        h_flex()
                            .id(("clip", id))
                            .gap_2()
                            .px_2()
                            .h(px(34.))
                            .rounded(px(style::RADIUS))
                            .when(selected, |d| d.bg(active))
                            .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                if ev.click_count() >= 2 {
                                    this.choose(id, true, cx);
                                } else {
                                    this.select(id, cx);
                                }
                            }))
                            .child(div().w(px(24.)).flex().justify_center().child(leading))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_sm()
                                    .child(label),
                            )
                            .when(info.pinned, |d| {
                                d.child(Icon::new(Lucide::Pin).xsmall().text_color(muted))
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(ago(now, info.created)),
                            ),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let Some(item) = self.selected_item() else {
            return div().into_any_element();
        };
        let info = &item.info;
        let id = info.id;
        let body = match &self.preview {
            Preview::Image(p, image) if *p == id => div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(image.clone())
                        .max_w_full()
                        .max_h_full()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            Preview::Text(p, text) if *p == id => div()
                .id("clip-preview-text")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .font_family("Cascadia Mono")
                .text_sm()
                .child(text.clone())
                .into_any_element(),
            _ => div()
                .flex_1()
                .text_sm()
                .text_color(muted)
                .child("Preview not available.")
                .into_any_element(),
        };
        let meta = match info.kind {
            EntryKind::Text => format!(
                "Text · {} character{} · {} line{}",
                info.chars,
                if info.chars == 1 { "" } else { "s" },
                info.lines,
                if info.lines == 1 { "" } else { "s" }
            ),
            EntryKind::Image => format!("Image · {} × {}", info.width, info.height),
        };
        let copied = match ago(now(), info.created).as_str() {
            "now" => "Copied just now".to_string(),
            "yesterday" => "Copied yesterday".to_string(),
            a => format!("Copied {a} ago"),
        };
        let pinned = info.pinned;
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .p_3()
            .child(body)
            .child(
                h_flex()
                    .gap_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        v_flex()
                            .flex_1()
                            .text_xs()
                            .text_color(muted)
                            .child(meta)
                            .child(copied),
                    )
                    .child(
                        Button::new("clip-pin")
                            .ghost()
                            .small()
                            .icon(Icon::new(if pinned { Lucide::PinOff } else { Lucide::Pin }))
                            .tooltip(if pinned {
                                "Unpin (Ctrl+P)"
                            } else {
                                "Pin (Ctrl+P)"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_pin(id, cx))),
                    )
                    .child(
                        Button::new("clip-delete")
                            .ghost()
                            .small()
                            .icon(Icon::new(Lucide::Trash))
                            .tooltip("Delete (Ctrl+Delete)")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.command(Request::Delete { id }, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ClipboardPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.render_rows(cx);
        let preview = self.render_preview(cx);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let empty_message = if self.off {
            Some("Clipboard history is off. Turn it on in Kuality Settings → Clipboard.")
        } else if self.items.is_empty() {
            Some("Nothing copied yet.")
        } else if rows.is_empty() {
            Some("No matches.")
        } else {
            None
        };
        let enter_hint = if self.paste { "↵ Paste" } else { "↵ Copy" };
        v_flex()
            .id("clipboard-popup")
            .key_context("ClipboardPopup")
            .capture_action(cx.listener(|_, _: &Escape, _, cx| cx.quit()))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div().p_3().child(
                    Input::new(&self.input)
                        .prefix(Icon::new(IconName::Search).small().text_color(muted)),
                ),
            )
            .when_some(self.error.clone(), |this, e| {
                this.child(
                    div()
                        .px_4()
                        .pb_2()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(e),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .id("clip-list")
                            .w(px(LIST_WIDTH))
                            .h_full()
                            .p_2()
                            .border_r_1()
                            .border_color(theme.border)
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .when_some(empty_message, |d, m| {
                                d.child(div().p_4().text_sm().text_color(muted).child(m))
                            })
                            .children(rows),
                    )
                    .child(preview),
            )
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .gap_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(muted)
                    .child(enter_hint)
                    .when(self.paste, |d| d.child("⇧↵ Copy"))
                    .child("Ctrl+P Pin")
                    .child("Ctrl+Del Delete")
                    .child("Esc Close"),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it would bring GPUI's `test` attribute in place of the built-in one.
    use super::{ScreenRect, ago, center_popup, first_line};

    #[test]
    fn popup_is_centered() {
        let work = ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032,
        };
        assert_eq!(center_popup(work, 1.0, (720.0, 460.0)), (600, 286));
        assert_eq!(center_popup(work, 1.5, (720.0, 460.0)), (420, 171));
        let second = ScreenRect {
            left: 1920,
            right: 2400,
            ..work
        };
        assert_eq!(
            center_popup(second, 1.0, (720.0, 460.0)).0,
            1920,
            "too wide"
        );
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(1000, 1000), "now");
        assert_eq!(ago(1000, 2000), "now", "clock went back");
        assert_eq!(ago(1000 + 59, 1000), "now");
        assert_eq!(ago(1000 + 300, 1000), "5 min");
        assert_eq!(ago(1000 + 7200, 1000), "2 h");
        assert_eq!(ago(1000 + 90_000, 1000), "yesterday");
        assert_eq!(ago(1000 + 4 * 86_400, 1000), "4 d");
    }

    #[test]
    fn first_non_empty_line() {
        assert_eq!(first_line("\n  \n  hello world \nnext"), "hello world");
        assert_eq!(first_line(""), "");
        assert_eq!(first_line(&"x".repeat(500)).len(), 200);
    }
}
