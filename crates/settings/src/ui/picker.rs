//! The action picker of the Shortcuts tab: a popover with a search box, the installed apps
//! (with icons) and, pinned below them, the other actions. Each other action opens a small
//! page of its own. The picker only reports a [`Choice`]; the window applies it.

use std::ops::Range;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::actions::Cancel;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::{Action, LaunchArgs};

use super::controls::{field, hint_text};
use crate::apps::{self, Catalog};
use crate::model::{self, ActionKind, Choice};

/// Width of the popover.
pub const PICKER_WIDTH: f32 = 440.;
const LIST_HEIGHT: f32 = 264.;
const ROW_HEIGHT: f32 = 32.;

#[derive(Debug, Clone)]
pub enum PickerEvent {
    Picked(Choice),
    /// The Record button of "Send keys" was pressed (the window owns the recorder).
    RecordKeys,
    /// Record pressed again while recording.
    StopRecording,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    List,
    Website,
    SendKeys,
    OpenFile,
    RunProgram,
}

/// The other actions, pinned under the apps.
const OTHER: [(Page, &str, Lucide); 4] = [
    (Page::Website, "Open website…", Lucide::Globe),
    (Page::SendKeys, "Send keys…", Lucide::Keyboard),
    (Page::OpenFile, "Open file or folder…", Lucide::FolderOpen),
    (Page::RunProgram, "Run program…", Lucide::SquareTerminal),
];

/// The icon of an action kind, for the Action button and the picker.
pub fn kind_icon(kind: ActionKind) -> Lucide {
    match kind {
        ActionKind::App => Lucide::AppWindow,
        ActionKind::Website => Lucide::Globe,
        ActionKind::SendKeys => Lucide::Keyboard,
        ActionKind::OpenFile => Lucide::FolderOpen,
        ActionKind::RunProgram => Lucide::SquareTerminal,
    }
}

pub struct ActionPicker {
    catalog: Entity<Catalog>,
    /// The action being replaced (its unknown fields are kept by the window).
    page: Page,
    search: Entity<InputState>,
    /// Indices into the catalog matching the search, best first.
    results: Vec<usize>,
    /// Keyboard selection: `0..results.len()` are apps, then the [`OTHER`] entries.
    highlight: usize,
    scroll: UniformListScrollHandle,
    current_app: Option<String>,
    url: Entity<InputState>,
    keys: Entity<InputState>,
    file: Entity<InputState>,
    program: Entity<InputState>,
    args: Entity<InputState>,
    cwd: Entity<InputState>,
    /// Arguments as first shown, and the value they came from: untouched lists stay lists.
    args_initial: String,
    prev_args: LaunchArgs,
    advanced: bool,
    recording: bool,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PickerEvent> for ActionPicker {}

impl ActionPicker {
    pub fn new(
        action: Option<&Action>,
        catalog: Entity<Catalog>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |value: String,
                     placeholder: &'static str,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let (mut url, mut keys, mut file, mut program, mut args, mut cwd) = Default::default();
        let mut prev_args = LaunchArgs::List(Vec::new());
        let mut current_app = None;
        let page = match action {
            None => Page::List,
            Some(a) => match (model::kind_of(a), a) {
                (ActionKind::App, Action::Launch(l)) => {
                    current_app = model::app_id(&l.path).map(str::to_string);
                    Page::List
                }
                (_, Action::OpenUrl(u)) => {
                    url = u.url.clone();
                    Page::Website
                }
                (_, Action::SendHotkey(h)) => {
                    keys = h.keys.clone();
                    Page::SendKeys
                }
                (ActionKind::OpenFile, Action::Launch(l)) => {
                    file = l.path.clone();
                    Page::OpenFile
                }
                (_, Action::Launch(l)) => {
                    program = l.path.clone();
                    args = model::args_display(&l.args);
                    cwd = l.cwd.clone().unwrap_or_default();
                    prev_args = l.args.clone();
                    Page::RunProgram
                }
            },
        };
        let advanced = !args.trim().is_empty() || !cwd.trim().is_empty();
        let args_initial = args.clone();
        let search = input(String::new(), "Search apps and actions", window, cx);
        let url = input(url, "https://github.com", window, cx);
        let keys = input(keys, "win+1", window, cx);
        let file = input(file, r"C:\Users\me\Documents", window, cx);
        let program = input(program, r"C:\Windows\notepad.exe", window, cx);
        let args = input(args, "Arguments (one command line)", window, cx);
        let cwd = input(cwd, "Default: the program's folder", window, cx);

        let mut subs = vec![
            cx.observe(&catalog, |this: &mut Self, _, cx| {
                this.refilter(cx);
                cx.notify();
            }),
            cx.subscribe_in(
                &search,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => {
                        this.highlight = 0;
                        this.refilter(cx);
                        this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.activate(this.highlight, window, cx),
                    _ => {}
                },
            ),
        ];
        for input in [&url, &keys, &file, &program, &args, &cwd] {
            subs.push(cx.subscribe_in(
                input,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    InputEvent::Change if this.error.is_some() => {
                        this.error = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ));
        }
        let mut this = Self {
            catalog,
            page,
            search,
            results: Vec::new(),
            highlight: 0,
            scroll: UniformListScrollHandle::new(),
            current_app,
            url,
            keys,
            file,
            program,
            args,
            cwd,
            args_initial,
            prev_args,
            advanced,
            recording: false,
            error: None,
            _subs: subs,
        };
        this.refilter(cx);
        if let Some(i) = this.current_app_position(cx) {
            this.highlight = i;
            this.scroll.scroll_to_item(i, ScrollStrategy::Center);
        }
        this
    }

    fn current_app_position(&self, cx: &App) -> Option<usize> {
        let id = self.current_app.as_deref()?;
        let index = self.catalog.read(cx).find(id)?;
        self.results.iter().position(|&r| r == index)
    }

    fn refilter(&mut self, cx: &App) {
        let query = self.search.read(cx).value().to_string();
        let catalog = self.catalog.read(cx);
        self.results = apps::search(catalog.apps().iter().map(|a| a.name.as_str()), &query);
        self.highlight = self.highlight.min(self.entry_count().saturating_sub(1));
    }

    fn entry_count(&self) -> usize {
        self.results.len() + OTHER.len()
    }

    /// Moves the keyboard focus to the page's main field.
    pub fn focus_page(&self, window: &mut Window, cx: &mut App) {
        let input = match self.page {
            Page::List => &self.search,
            Page::Website => &self.url,
            Page::SendKeys => &self.keys,
            Page::OpenFile => &self.file,
            Page::RunProgram => &self.program,
        };
        input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// The window's recorder started or stopped.
    pub fn set_recording(&mut self, recording: bool, cx: &mut Context<Self>) {
        self.recording = recording;
        cx.notify();
    }

    /// A chord was recorded for "Send keys".
    pub fn set_keys(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = false;
        self.error = None;
        self.keys.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }

    fn open_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording {
            cx.emit(PickerEvent::StopRecording);
        }
        self.page = page;
        self.error = None;
        self.focus_page(window, cx);
        // "Send keys" listens at once; a click on the field (or Esc) stops it to type instead.
        if page == Page::SendKeys {
            cx.emit(PickerEvent::RecordKeys);
        }
        cx.notify();
    }

    /// Whether the picker shows the "Send keys" page (it then records as soon as it opens).
    pub fn on_send_keys(&self) -> bool {
        self.page == Page::SendKeys
    }

    fn activate(&mut self, entry: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&app) = self.results.get(entry) {
            let id = self.catalog.read(cx).apps().get(app).map(|a| a.id.clone());
            if let Some(id) = id {
                cx.emit(PickerEvent::Picked(Choice::App(id)));
            }
        } else if let Some(&(page, _, _)) = OTHER.get(entry - self.results.len()) {
            self.open_page(page, window, cx);
        }
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == Page::List {
            cx.emit(PickerEvent::Close);
        } else {
            self.open_page(Page::List, window, cx);
        }
    }

    fn move_highlight(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.entry_count();
        if n == 0 {
            return;
        }
        self.highlight = if down {
            (self.highlight + 1) % n
        } else {
            (self.highlight + n - 1) % n
        };
        if self.highlight < self.results.len() {
            self.scroll
                .scroll_to_item(self.highlight, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn value(&self, input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    /// Validates the open page and reports its choice.
    fn save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let result = match self.page {
            Page::List => return,
            Page::Website => {
                let url = model::normalize_url(&self.value(&self.url, cx));
                model::validate_url(&url)
                    .map(|_| Choice::Website(url))
                    .map_err(str::to_string)
            }
            Page::SendKeys => {
                let keys = self.value(&self.keys, cx);
                model::validate_chord(&keys).map(|_| Choice::SendKeys(keys))
            }
            Page::OpenFile => {
                let path = self.value(&self.file, cx);
                model::validate_path(&path)
                    .map(|_| Choice::OpenFile(path))
                    .map_err(str::to_string)
            }
            Page::RunProgram => {
                let path = self.value(&self.program, cx);
                let args_text = self.args.read(cx).value().to_string();
                let args = if args_text == self.args_initial {
                    self.prev_args.clone()
                } else if args_text.trim().is_empty() {
                    LaunchArgs::List(Vec::new())
                } else {
                    LaunchArgs::Raw(args_text)
                };
                let cwd = Some(self.value(&self.cwd, cx));
                model::validate_path(&path)
                    .map(|_| Choice::RunProgram { path, args, cwd })
                    .map_err(|_| "Choose a program".to_string())
            }
        };
        match result {
            Ok(choice) => {
                if self.recording {
                    cx.emit(PickerEvent::StopRecording);
                }
                cx.emit(PickerEvent::Picked(choice));
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn browse(&mut self, target: Browse, window: &mut Window, cx: &mut Context<Self>) {
        let folder = matches!(target, Browse::Folder | Browse::Cwd);
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: !folder,
            directories: folder,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let input = match target {
                    Browse::File | Browse::Folder => this.file.clone(),
                    Browse::Program => this.program.clone(),
                    Browse::Cwd => this.cwd.clone(),
                };
                input.update(cx, |s, cx| {
                    s.set_value(path.display().to_string(), window, cx)
                });
                this.error = None;
                cx.notify();
            });
        })
        .detach();
    }

    // --- Rendering -----------------------------------------------------------------------

    fn render_app_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let catalog = self.catalog.read(cx);
        let current = self.current_app.as_deref().and_then(|id| catalog.find(id));
        let theme = cx.theme();
        let (accent, muted) = (theme.accent, theme.muted_foreground);
        let rows: Vec<_> = range
            .filter_map(|entry| {
                let &app = self.results.get(entry)?;
                let info = catalog.apps().get(app)?;
                Some((entry, app, info.name.clone(), catalog.icon(app)))
            })
            .collect();
        rows.into_iter()
            .map(|(entry, app, name, icon)| {
                let icon = match icon {
                    Some(image) => img(image).size(px(20.)).flex_none().into_any_element(),
                    None => Icon::new(Lucide::AppWindow)
                        .size(px(20.))
                        .text_color(muted)
                        .into_any_element(),
                };
                h_flex()
                    .id(("app", app))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .gap_2()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .when(entry == self.highlight, |this| this.bg(accent))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.highlight != entry {
                            this.highlight = entry;
                            cx.notify();
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.activate(entry, window, cx)),
                    )
                    .child(icon)
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .when(current == Some(app), |this| {
                        this.child(Icon::new(IconName::Check).small().text_color(muted))
                    })
                    .into_any_element()
            })
            .collect()
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let catalog = self.catalog.read(cx);
        let (loaded, error, any_apps) = (
            catalog.loaded,
            catalog.error.clone(),
            !catalog.apps().is_empty(),
        );
        let theme = cx.theme();
        let (accent, muted, border) = (theme.accent, theme.muted_foreground, theme.border);
        let apps: AnyElement = if !self.results.is_empty() {
            uniform_list(
                "apps",
                self.results.len(),
                cx.processor(|this, range, _window, cx| this.render_app_rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .h(px(LIST_HEIGHT))
            .into_any_element()
        } else {
            let text = match (&error, loaded, any_apps) {
                (Some(e), _, _) => format!("Could not list the installed apps: {e}"),
                (None, false, _) => "Loading apps…".to_string(),
                (None, true, false) => "No apps found.".to_string(),
                (None, true, true) => "No app matches.".to_string(),
            };
            v_flex()
                .h(px(LIST_HEIGHT))
                .items_center()
                .justify_center()
                .child(hint_text(text, cx))
                .into_any_element()
        };
        let first_other = self.results.len();
        let others = OTHER.iter().enumerate().map(|(i, &(_, label, icon))| {
            let entry = first_other + i;
            h_flex()
                .id(("other", i))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px_2()
                .gap_2()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .when(entry == self.highlight, |this| this.bg(accent))
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.highlight != entry {
                        this.highlight = entry;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| this.activate(entry, window, cx)))
                .child(Icon::new(icon).size(px(20.)).text_color(muted))
                .child(div().flex_1().child(label))
                .child(Icon::new(IconName::ChevronRight).small().text_color(muted))
        });
        v_flex()
            .gap_1()
            .child(
                Input::new(&self.search)
                    .prefix(Icon::new(IconName::Search).small().text_color(muted))
                    .cleanable(true),
            )
            .child(section_label("Applications", cx))
            .child(apps)
            .child(div().h(px(1.)).bg(border).my_1())
            .child(section_label("Other actions", cx))
            .children(others)
            .into_any_element()
    }

    fn page_header(&self, title: &'static str, cx: &mut Context<Self>) -> Div {
        h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("picker-back")
                    .small()
                    .ghost()
                    .icon(IconName::ArrowLeft)
                    .tooltip("Back to the list")
                    .accessibility_label("Back")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_page(Page::List, window, cx)),
                    ),
            )
            .child(div().text_sm().font_semibold().child(title))
    }

    fn save_row(&self, cx: &mut Context<Self>) -> Div {
        h_flex().justify_end().child(
            Button::new("picker-save")
                .small()
                .primary()
                .label("Save")
                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
        )
    }

    fn render_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let error = self.error.clone();
        let body = match self.page {
            Page::List => return self.render_list(cx),
            Page::Website => v_flex()
                .gap_3()
                .child(self.page_header("Open website", cx))
                .child(field(
                    "Address",
                    Input::new(&self.url),
                    error.as_deref(),
                    cx,
                ))
                .child(hint_text(
                    "https:, mailto:, ms-settings: or any other link. https:// is added when \
                     missing.",
                    cx,
                )),
            Page::SendKeys => {
                let recording = self.recording;
                let keys = self.keys.read(cx).value().to_string();
                let preview = if model::validate_chord(&keys).is_ok() {
                    model::chord_label(&keys)
                } else {
                    String::new()
                };
                v_flex()
                    .gap_3()
                    .child(self.page_header("Send keys", cx))
                    .child(
                        Button::new("record-chord")
                            .when(recording, |b| b.primary())
                            .when(!recording, |b| b.outline())
                            .icon(Lucide::Keyboard)
                            .label(if recording {
                                "Press the keys… (Esc to cancel)"
                            } else {
                                "Record keys"
                            })
                            .accessibility_label("Record keys")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(if recording {
                                    PickerEvent::StopRecording
                                } else {
                                    PickerEvent::RecordKeys
                                })
                            })),
                    )
                    .when(!preview.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .font_medium()
                                .child(format!("Sends {preview}")),
                        )
                    })
                    .child(field(
                        "Or type them",
                        // Clicking the field to type stops the listening recorder.
                        div()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if this.recording {
                                        cx.emit(PickerEvent::StopRecording);
                                    }
                                }),
                            )
                            .child(Input::new(&self.keys)),
                        error.as_deref(),
                        cx,
                    ))
                    .child(hint_text(
                        "Like win+1, ctrl+shift+esc or volume_mute: up to 4 modifiers and one key.",
                        cx,
                    ))
            }
            Page::OpenFile => v_flex()
                .gap_3()
                .child(self.page_header("Open file or folder", cx))
                .child(field("Path", Input::new(&self.file), error.as_deref(), cx))
                .child(
                    h_flex()
                        .gap_2()
                        .child(browse_button(
                            "browse-file",
                            "Choose file…",
                            Browse::File,
                            cx,
                        ))
                        .child(browse_button(
                            "browse-folder",
                            "Choose folder…",
                            Browse::Folder,
                            cx,
                        )),
                )
                .child(hint_text(
                    "Opens with its default app. %VARIABLES% are expanded when the shortcut runs.",
                    cx,
                )),
            Page::RunProgram => {
                let advanced = self.advanced;
                v_flex()
                    .gap_3()
                    .child(self.page_header("Run program", cx))
                    .child(field(
                        "Program",
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.program)))
                            .child(browse_button(
                                "browse-program",
                                "Browse…",
                                Browse::Program,
                                cx,
                            )),
                        error.as_deref(),
                        cx,
                    ))
                    // In a row so the toggle sits on the left instead of stretching.
                    .child(
                        h_flex().child(
                            Button::new("advanced")
                                .small()
                                .ghost()
                                .icon(if advanced {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .label("Advanced")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.advanced = !this.advanced;
                                    cx.notify();
                                })),
                        ),
                    )
                    .when(advanced, |this| {
                        this.child(field("Arguments", Input::new(&self.args), None, cx))
                            .child(field(
                                "Working folder",
                                h_flex()
                                    .gap_2()
                                    .child(div().flex_1().child(Input::new(&self.cwd)))
                                    .child(browse_button("browse-cwd", "Browse…", Browse::Cwd, cx)),
                                None,
                                cx,
                            ))
                            .child(hint_text(
                                "%VARIABLES% are expanded when the shortcut runs.",
                                cx,
                            ))
                    })
            }
        };
        body.child(self.save_row(cx)).into_any_element()
    }
}

#[derive(Debug, Clone, Copy)]
enum Browse {
    File,
    Folder,
    Program,
    Cwd,
}

fn browse_button(
    id: &'static str,
    label: &'static str,
    target: Browse,
    cx: &mut Context<ActionPicker>,
) -> Button {
    Button::new(id)
        .small()
        .outline()
        .icon(match target {
            Browse::File | Browse::Program => IconName::File,
            Browse::Folder | Browse::Cwd => IconName::Folder,
        })
        .label(label)
        .on_click(cx.listener(move |this, _, window, cx| this.browse(target, window, cx)))
}

fn section_label(text: &'static str, cx: &App) -> Div {
    div()
        .px_2()
        .pt_1()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

impl Render for ActionPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("action-picker")
            .w(px(PICKER_WIDTH))
            // Up/Down move the list selection instead of the search caret.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.page == Page::List {
                    cx.stop_propagation();
                    this.move_highlight(false, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.page == Page::List {
                    cx.stop_propagation();
                    this.move_highlight(true, cx);
                }
            }))
            // Esc: back to the list from a page, else close. The input binds Escape, the popover
            // binds Cancel; whichever arrives, it is handled here first.
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.escape(window, cx)
            }))
            .capture_action(cx.listener(|this, _: &Cancel, window, cx| {
                cx.stop_propagation();
                this.escape(window, cx)
            }))
            .child(self.render_page(cx))
    }
}
