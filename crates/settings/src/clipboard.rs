//! The clipboard history popup (`kuality.exe --clipboard`, see `docs/specs/clipboard.md`):
//! search, list and preview of the history the core keeps in memory, read over the core's pipe.
//! Enter pastes into the window that was in front when the hotkey fired (`--paste`), Shift+Enter
//! only copies. Nothing is written to disk; decrypted data lives only in this process while it
//! is open.
//!
//! The list is gpui-component's `List` (virtualized, with its search field and sections); the
//! entry under the selection is fetched from the core off the UI thread.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::empty::{Empty, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::component::input::Escape;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::label::Label;
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListItem, ListState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, Icon, IndexPath, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::clipboard::{EntryInfo, EntryKind, Request};
use kuality_protocol::unix_now;

use crate::platform::{self, CLIPBOARD_WINDOW_TITLE, ScreenRect};
use crate::ui::window::{self, ACTIVATION_GRACE};

/// Popup size in DIPs.
const WIDTH: f32 = 720.0;
const HEIGHT: f32 = 460.0;
/// While the popup is not in front yet, it retries taking the foreground this often…
const FOREGROUND_RETRY: Duration = Duration::from_millis(100);
/// …this many times (the user may still be releasing the hotkey).
const FOREGROUND_TRIES: u32 = 15;
/// Width of the list column in DIPs.
const LIST_WIDTH: f32 = 300.0;
/// Rows PageUp / PageDown move the selection by.
const PAGE_ROWS: isize = 8;

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

/// Index of the entry `delta` rows away from `at` in a list of `len` rows (clamped to its ends;
/// no selection counts as the first row).
fn moved(at: Option<usize>, delta: isize, len: usize) -> Option<usize> {
    let last = len.checked_sub(1)?;
    Some((at.unwrap_or(0) as isize + delta).clamp(0, last as isize) as usize)
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

/// Debug builds only: `KUALITY_CLIPBOARD_SAMPLE` is set, to work on the popup without a core and
/// without copying anything. It then shows made-up entries and stays open when it loses the
/// focus (pin, delete and paste do nothing).
fn sample_mode() -> bool {
    cfg!(debug_assertions) && std::env::var_os("KUALITY_CLIPBOARD_SAMPLE").is_some()
}

fn sample_items() -> Option<Vec<Item>> {
    if !sample_mode() {
        return None;
    }
    let now = unix_now();
    let texts = [
        (
            true,
            86_400 * 3,
            "A pinned snippet: cargo clippy --all-targets -- -D warnings",
        ),
        (true, 7200, "Another pin\nwith a second line"),
        (false, 20, "https://github.com/VictorGuill/KUALITY"),
        (false, 300, "fn main() {\n    println!(\"hello\");\n}"),
        (
            false,
            4000,
            "A plain sentence copied from somewhere, long enough to be cut in the list.",
        ),
    ];
    let more = (0..40).map(|i| (false, 90_000 + i * 600, format!("Older entry number {i}")));
    let all = texts
        .into_iter()
        .map(|(pinned, age, text)| (pinned, age, text.to_string()))
        .chain(more);
    Some(
        all.enumerate()
            .map(|(i, (pinned, age, text))| Item {
                search: text.to_lowercase(),
                thumb: None,
                info: EntryInfo {
                    id: i as u64 + 1,
                    kind: EntryKind::Text,
                    pinned,
                    created: now.saturating_sub(age),
                    truncated: false,
                    chars: text.chars().count() as u64,
                    lines: text.lines().count().max(1) as u64,
                    text: Some(text),
                    width: 0,
                    height: 0,
                    thumb_len: 0,
                },
            })
            .collect(),
    )
}

fn load(core_pid: Option<u32>) -> Load {
    if let Some(items) = sample_items() {
        return Load::Ready(items);
    }
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
    window::run_window(
        "clipboard popup",
        move |cx| {
            let monitor = platform::monitor_at(anchor.or_else(platform::cursor_pos));
            let origin = monitor.map(|m| center_popup(m.work, m.scale, (WIDTH, HEIGHT)));
            let (bounds, display_id) =
                window::popup_bounds(monitor, origin, size(px(WIDTH), px(HEIGHT)), cx);
            Some(window::popup_options(
                CLIPBOARD_WINDOW_TITLE,
                "Kuality.Clipboard",
                bounds,
                display_id,
                true,
            ))
        },
        |window| platform::popup_window_setup(window, true),
        move |window, cx| ClipboardPopup::new(loaded, core_pid, core, paste, window, cx),
    );
}

/// A group of rows of the list.
struct Section {
    pinned: bool,
    /// Indices into [`Entries::items`].
    rows: Vec<usize>,
}

/// The list's content: the entries matching the search, pinned ones in a first section.
struct Entries {
    items: Vec<Item>,
    /// Lowercase search text.
    query: String,
    /// The matching entries: the pinned ones, then the rest. A group without entries is left
    /// out, so the first row of the list is always its first section's.
    sections: Vec<Section>,
    selected: Option<IndexPath>,
    /// No pipe: the core is not running or the history is off.
    off: bool,
}

impl Entries {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            query: String::new(),
            sections: Vec::new(),
            selected: None,
            off: false,
        }
    }

    fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.refilter();
    }

    fn refilter(&mut self) {
        let (mut pinned, mut recent) = (Vec::new(), Vec::new());
        for (i, item) in self.items.iter().enumerate() {
            if self.query.is_empty() || item.search.contains(&self.query) {
                if item.info.pinned {
                    pinned.push(i);
                } else {
                    recent.push(i);
                }
            }
        }
        self.sections = [(true, pinned), (false, recent)]
            .into_iter()
            .filter(|(_, rows)| !rows.is_empty())
            .map(|(pinned, rows)| Section { pinned, rows })
            .collect();
    }

    fn item(&self, ix: IndexPath) -> Option<&Item> {
        self.items
            .get(*self.sections.get(ix.section)?.rows.get(ix.row)?)
    }

    fn by_id(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.info.id == id)
    }

    fn selected_id(&self) -> Option<u64> {
        self.selected
            .and_then(|ix| self.item(ix))
            .map(|i| i.info.id)
    }

    /// Where the entry `id` is shown, if it matches the search.
    fn path_of(&self, id: u64) -> Option<IndexPath> {
        self.sections.iter().enumerate().find_map(|(section, s)| {
            s.rows
                .iter()
                .position(|&i| self.items[i].info.id == id)
                .map(|row| IndexPath::default().section(section).row(row))
        })
    }

    fn len(&self) -> usize {
        self.sections.iter().map(|s| s.rows.len()).sum()
    }

    /// Position of a row counting every section, and back.
    fn flat(&self, ix: IndexPath) -> usize {
        let before: usize = self.sections[..ix.section.min(self.sections.len())]
            .iter()
            .map(|s| s.rows.len())
            .sum();
        before + ix.row
    }

    fn path(&self, mut flat: usize) -> IndexPath {
        for (section, s) in self.sections.iter().enumerate() {
            if flat < s.rows.len() {
                return IndexPath::default().section(section).row(flat);
            }
            flat -= s.rows.len();
        }
        IndexPath::default()
    }
}

impl ListDelegate for Entries {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.trim().to_lowercase();
        self.refilter();
        Task::ready(())
    }

    fn sections_count(&self, _: &App) -> usize {
        self.sections.len().max(1)
    }

    fn items_count(&self, section: usize, _: &App) -> usize {
        self.sections.get(section).map_or(0, |s| s.rows.len())
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let item = self.item(ix)?;
        let info = &item.info;
        let muted = cx.theme().muted_foreground;
        let label: SharedString = match info.kind {
            EntryKind::Text => first_line(info.text.as_deref().unwrap_or_default()).into(),
            EntryKind::Image => format!("Image {} × {}", info.width, info.height).into(),
        };
        let leading = match &item.thumb {
            Some(t) => img(t.clone())
                .size_6()
                .object_fit(ObjectFit::Contain)
                .rounded(cx.theme().radius / 2.)
                .into_any_element(),
            None => Icon::new(Lucide::Type)
                .small()
                .text_color(muted)
                .into_any_element(),
        };
        let list = cx.entity().downgrade();
        Some(
            ListItem::new(("clip", info.id))
                .accessibility_label(label.clone())
                // A click only selects (and shows the preview); the list's own click handler,
                // which chooses the entry, is left for a double click.
                .on_click(move |ev: &ClickEvent, window, cx| {
                    if ev.click_count() < 2 {
                        cx.stop_propagation();
                        let _ = list.update(cx, |list, cx| {
                            list.set_selected_index(Some(ix), window, cx);
                            cx.notify();
                        });
                    }
                })
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            h_flex()
                                .size_6()
                                .flex_none()
                                .justify_center()
                                .child(leading),
                        )
                        .child(Label::new(label).flex_1().min_w_0().truncate().text_sm())
                        .when(info.pinned, |row| {
                            row.child(Icon::new(Lucide::Pin).xsmall().text_color(muted))
                        })
                        .child(
                            Label::new(ago(unix_now(), info.created))
                                .text_xs()
                                .text_color(muted),
                        ),
                ),
        )
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        // "Recent" is only worth a header under the pinned entries.
        let with_pins = self.sections.first().is_some_and(|s| s.pinned);
        let pinned = self.sections.get(section)?.pinned;
        with_pins.then(|| {
            Label::new(if pinned { "Pinned" } else { "Recent" })
                .px_3()
                .pt_2()
                .pb_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
        })
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let (title, description) = if self.off {
            (
                "Clipboard history is off",
                "Turn it on in Kuality Settings → Clipboard.",
            )
        } else if self.items.is_empty() {
            ("Nothing copied yet", "What you copy shows up here.")
        } else {
            ("No matches", "Nothing in the history has that text.")
        };
        Empty::new().size_full().header(
            EmptyHeader::new()
                .title(EmptyTitle::new().child(title))
                .description(EmptyDescription::new().child(description)),
        )
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        // The popup observes the list and follows the selection (preview).
        self.selected = ix;
        cx.notify();
    }
}

/// The selected entry's full content (fetched with `get` when the list only has a part).
enum Preview {
    None,
    /// Being fetched from the core.
    Loading(u64),
    Text(u64, SharedString),
    Image(u64, Arc<Image>),
}

impl Preview {
    fn id(&self) -> Option<u64> {
        match self {
            Self::None => None,
            Self::Loading(id) | Self::Text(id, _) | Self::Image(id, _) => Some(*id),
        }
    }
}

struct ClipboardPopup {
    core_pid: Option<u32>,
    /// Opened by the hotkey: Enter pastes into the previous window (else it copies).
    paste: bool,
    list: Entity<ListState<Entries>>,
    error: Option<String>,
    preview: Preview,
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
        let list = cx.new(|cx| ListState::new(Entries::new(), window, cx).searchable(true));
        list.update(cx, |list, cx| list.focus(window, cx));
        if let Some(core) = core {
            window::watch_core(core, cx, |_, cx| {
                log::info!("Kuality exited; closing the clipboard popup");
                cx.quit();
            });
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
            // The selection moved (keys, search, click): show that entry.
            cx.observe(&list, |this, _, cx| this.load_preview(cx)),
            cx.subscribe(&list, |this, _, ev: &ListEvent, cx| {
                // Enter or a double click.
                if let ListEvent::Confirm(_) = ev
                    && let Some(id) = this.selected_id(cx)
                {
                    this.choose(id, true, cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active()
                    && this.opened.elapsed() >= ACTIVATION_GRACE
                    && !sample_mode()
                {
                    log::debug!("clipboard popup lost focus; closing");
                    cx.quit();
                }
            }),
        ];
        let mut this = Self {
            core_pid,
            paste,
            list,
            error: None,
            preview: Preview::None,
            opened: Instant::now(),
            _subs: subs,
        };
        this.set_loaded(loaded, window, cx);
        this
    }

    fn selected_id(&self, cx: &App) -> Option<u64> {
        self.list.read(cx).delegate().selected_id()
    }

    /// Shows a list read from the core, keeping the selection on the same entry if it is still
    /// there (else on the first one).
    fn set_loaded(&mut self, loaded: Load, window: &mut Window, cx: &mut Context<Self>) {
        let keep = self.selected_id(cx);
        self.list.update(cx, |list, cx| {
            let entries = list.delegate_mut();
            match loaded {
                Load::Ready(items) => {
                    entries.off = false;
                    entries.set_items(items);
                }
                Load::Off => {
                    entries.off = true;
                    entries.set_items(Vec::new());
                }
                Load::Error(e) => self.error = Some(e),
            }
            let entries = list.delegate();
            let ix = keep
                .and_then(|id| entries.path_of(id))
                .or_else(|| (entries.len() > 0).then(|| entries.path(0)));
            list.set_selected_index(ix, window, cx);
            cx.notify();
        });
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let loaded = load(self.core_pid);
        self.set_loaded(loaded, window, cx);
        cx.notify();
    }

    /// Shows the selected entry: at once when the list has all of it, else fetched from the
    /// core on a background thread (an image, or a text longer than the list carries).
    fn load_preview(&mut self, cx: &mut Context<Self>) {
        let entries = self.list.read(cx).delegate();
        let Some(item) = entries.selected_id().and_then(|id| entries.by_id(id)) else {
            if self.preview.id().is_some() {
                self.preview = Preview::None;
                cx.notify();
            }
            return;
        };
        let id = item.info.id;
        if self.preview.id() == Some(id) {
            return;
        }
        let complete = item.info.kind == EntryKind::Text && !item.info.truncated;
        let pid = match self.core_pid {
            Some(pid) if !complete => pid,
            _ => {
                let text = item.info.text.clone().unwrap_or_default();
                self.preview = Preview::Text(id, text.into());
                cx.notify();
                return;
            }
        };
        self.preview = Preview::Loading(id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_executor()
                .spawn(async move { platform::pipe_request(pid, Request::Get { id }) })
                .await;
            let _ = this.update(cx, |this, cx| {
                // The selection may have moved on while this was read.
                if !matches!(this.preview, Preview::Loading(p) if p == id) {
                    return;
                }
                this.preview = match fetched {
                    Ok((r, payload)) if r.ok => match r.entries.first().map(|e| e.kind) {
                        Some(EntryKind::Image) => Preview::Image(
                            id,
                            Arc::new(Image::from_bytes(ImageFormat::Png, payload)),
                        ),
                        _ => {
                            Preview::Text(id, String::from_utf8_lossy(&payload).into_owned().into())
                        }
                    },
                    _ => Preview::None,
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Moves the selection `delta` rows (PageUp / PageDown; the list itself handles the arrows).
    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.list.update(cx, |list, cx| {
            let entries = list.delegate();
            let at = entries.selected.map(|ix| entries.flat(ix));
            if let Some(next) = moved(at, delta, entries.len()) {
                let ix = entries.path(next);
                list.set_selected_index(Some(ix), window, cx);
                list.scroll_to_item(ix, ScrollStrategy::Center, window, cx);
            }
        });
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
            Ok((r, _)) => self.error = r.error,
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// Pin / unpin / delete, then reload the list.
    fn command(&mut self, request: Request, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pid) = self.core_pid else {
            return;
        };
        self.error = match platform::pipe_request(pid, request) {
            Ok((r, _)) if r.ok => None,
            Ok((r, _)) => r.error,
            Err(e) => Some(e.to_string()),
        };
        self.reload(window, cx);
    }

    fn toggle_pin(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let pinned = self
            .list
            .read(cx)
            .delegate()
            .by_id(id)
            .is_some_and(|i| i.info.pinned);
        let request = if pinned {
            Request::Unpin { id }
        } else {
            Request::Pin { id }
        };
        self.command(request, window, cx);
    }

    /// Keys the list does not handle itself (it has the arrows, Enter and Ctrl+Enter).
    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = k.modifiers;
        let selected = self.selected_id(cx);
        match (k.key.as_str(), selected) {
            ("escape", _) => cx.quit(),
            ("pageup", _) => self.move_selection(-PAGE_ROWS, window, cx),
            ("pagedown", _) => self.move_selection(PAGE_ROWS, window, cx),
            // Shift+Enter: copy only.
            ("enter", Some(id)) if m.shift => self.choose(id, false, cx),
            ("p", Some(id)) if m.control => self.toggle_pin(id, window, cx),
            ("delete" | "backspace", Some(id)) if m.control => {
                self.command(Request::Delete { id }, window, cx)
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let entries = self.list.read(cx).delegate();
        let Some(item) = entries.selected_id().and_then(|id| entries.by_id(id)) else {
            return div().flex_1().h_full().into_any_element();
        };
        let info = &item.info;
        let id = info.id;
        let body = match &self.preview {
            Preview::Image(p, image) if *p == id => h_flex()
                .flex_1()
                .min_h_0()
                .justify_center()
                .child(
                    img(image.clone())
                        .max_w_full()
                        .max_h_full()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            Preview::Text(p, text) if *p == id => div()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .size_full()
                        .overflow_y_scrollbar()
                        .font_family(theme.mono_font_family.clone())
                        .text_sm()
                        .child(text.clone()),
                )
                .into_any_element(),
            Preview::Loading(p) if *p == id => div().flex_1().into_any_element(),
            _ => Label::new("Preview not available.")
                .flex_1()
                .text_sm()
                .text_color(muted)
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
        let copied = match ago(unix_now(), info.created).as_str() {
            "now" => "Copied just now".to_string(),
            "yesterday" => "Copied yesterday".to_string(),
            a => format!("Copied {a} ago"),
        };
        let pinned = info.pinned;
        v_flex()
            .flex_1()
            .h_full()
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
                            .child(Label::new(meta).text_xs().text_color(muted))
                            .child(Label::new(copied).text_xs().text_color(muted)),
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_pin(id, window, cx)
                            })),
                    )
                    .child(
                        Button::new("clip-delete")
                            .ghost()
                            .small()
                            .icon(Icon::new(Lucide::Trash))
                            .tooltip("Delete (Ctrl+Delete)")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.command(Request::Delete { id }, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// One entry of the key legend: `[Enter] Paste`.
fn key_hint(keys: &'static str, label: &'static str) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .children(Keystroke::parse(keys).ok().map(Kbd::new))
        .child(Label::new(label).text_xs())
}

impl Render for ClipboardPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = self.render_preview(cx);
        let theme = cx.theme();
        v_flex()
            .id("clipboard-popup")
            .key_context("ClipboardPopup")
            .capture_action(cx.listener(|_, _: &Escape, _, cx| cx.quit()))
            .capture_key_down(
                cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)),
            )
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .when_some(self.error.clone(), |this, e| {
                this.child(
                    Label::new(e)
                        .px_4()
                        .pt_2()
                        .text_xs()
                        .text_color(theme.danger),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(LIST_WIDTH))
                            .h_full()
                            .flex_none()
                            .border_r_1()
                            .border_color(theme.border)
                            .child(List::new(&self.list).search_placeholder("Search clipboard…")),
                    )
                    .child(preview),
            )
            .child(
                StatusBar::new().left(
                    h_flex()
                        .gap_4()
                        .text_color(theme.muted_foreground)
                        .child(key_hint("enter", if self.paste { "Paste" } else { "Copy" }))
                        .when(self.paste, |row| row.child(key_hint("shift-enter", "Copy")))
                        .child(key_hint("ctrl-p", "Pin"))
                        .child(key_hint("ctrl-delete", "Delete"))
                        .child(key_hint("escape", "Close")),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it would bring GPUI's `test` attribute in place of the built-in one.
    use super::{ScreenRect, ago, center_popup, first_line, moved};

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

    #[test]
    fn page_keys_stay_inside_the_list() {
        assert_eq!(moved(Some(3), 8, 20), Some(11));
        assert_eq!(moved(Some(3), -8, 20), Some(0));
        assert_eq!(moved(Some(15), 8, 20), Some(19));
        assert_eq!(moved(None, 8, 20), Some(8));
        assert_eq!(moved(None, 1, 0), None);
    }
}
