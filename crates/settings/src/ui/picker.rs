//! The action picker of the Shortcuts tab: a popover with a searchable list (gpui-component's
//! `List`) of the actions that need a value ("Open website…") and the installed apps (with
//! icons). An action with a value opens a small form of its own. The picker only reports a
//! [`Choice`]; the window applies it.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::actions::Cancel;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::empty::{Empty, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::component::form::{Form, v_form};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListItem, ListState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::*;
use kuality_protocol::{Action, LaunchArgs};

use super::controls::{Note, field, hint_text};
use crate::apps::{self, Catalog};
use crate::model::{self, ActionKind, Choice};

/// Width of the popover.
pub const PICKER_WIDTH: f32 = 440.;
/// Height of the list (search field included).
const LIST_HEIGHT: f32 = 380.;

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

/// The actions that need a value, listed before the apps.
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

/// A group of rows of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    /// Indices into [`OTHER`].
    Actions,
    /// Indices into the catalog.
    Apps,
}

/// What a row of the list stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Action(Page),
    /// The app's id.
    App(String),
}

/// The list's content: the actions and apps matching the search.
struct Entries {
    catalog: Entity<Catalog>,
    query: String,
    /// Groups with at least one match (so the first row is always the first section's).
    sections: Vec<(Group, Vec<usize>)>,
    /// The app of the action being replaced (marked in the list).
    current_app: Option<String>,
}

impl Entries {
    fn refilter(&mut self, cx: &App) {
        let query = self.query.trim().to_lowercase();
        let actions: Vec<usize> = (0..OTHER.len())
            .filter(|&i| query.is_empty() || OTHER[i].1.to_lowercase().contains(&query))
            .collect();
        let catalog = self.catalog.read(cx);
        let apps = apps::search(catalog.apps().iter().map(|a| a.name.as_str()), &self.query);
        self.sections = [(Group::Actions, actions), (Group::Apps, apps)]
            .into_iter()
            .filter(|(_, rows)| !rows.is_empty())
            .collect();
    }

    fn row(&self, ix: IndexPath, cx: &App) -> Option<Row> {
        let (group, rows) = self.sections.get(ix.section)?;
        let &index = rows.get(ix.row)?;
        match group {
            Group::Actions => OTHER.get(index).map(|&(page, _, _)| Row::Action(page)),
            Group::Apps => {
                let app = self.catalog.read(cx).apps().get(index)?;
                Some(Row::App(app.id.clone()))
            }
        }
    }

    /// Where the app of the action being replaced is shown.
    fn current_app_path(&self, cx: &App) -> Option<IndexPath> {
        let app = self.catalog.read(cx).find(self.current_app.as_deref()?)?;
        self.sections
            .iter()
            .enumerate()
            .find_map(|(section, (group, rows))| {
                (*group == Group::Apps)
                    .then(|| rows.iter().position(|&r| r == app))
                    .flatten()
                    .map(|row| IndexPath::default().section(section).row(row))
            })
    }
}

impl ListDelegate for Entries {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_string();
        self.refilter(cx);
        Task::ready(())
    }

    fn sections_count(&self, _: &App) -> usize {
        self.sections.len().max(1)
    }

    fn items_count(&self, section: usize, _: &App) -> usize {
        self.sections.get(section).map_or(0, |(_, rows)| rows.len())
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let (group, rows) = self.sections.get(ix.section)?;
        let &index = rows.get(ix.row)?;
        let muted = cx.theme().muted_foreground;
        let (icon, label, trailing) = match group {
            Group::Actions => {
                let &(_, label, icon) = OTHER.get(index)?;
                (
                    Icon::new(icon)
                        .size_5()
                        .text_color(muted)
                        .into_any_element(),
                    SharedString::from(label),
                    Some(IconName::ChevronRight),
                )
            }
            Group::Apps => {
                let catalog = self.catalog.read(cx);
                let app = catalog.apps().get(index)?;
                let icon = match catalog.icon(index) {
                    Some(image) => img(image).size_5().flex_none().into_any_element(),
                    None => Icon::new(Lucide::AppWindow)
                        .size_5()
                        .text_color(muted)
                        .into_any_element(),
                };
                let current = self.current_app.as_deref() == Some(app.id.as_str());
                (
                    icon,
                    SharedString::from(app.name.clone()),
                    current.then_some(IconName::Check),
                )
            }
        };
        Some(
            ListItem::new(("picker-row", ix.section * 100_000 + ix.row))
                .accessibility_label(label.clone())
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(icon)
                        .child(Label::new(label).flex_1().min_w_0().truncate().text_sm())
                        .children(trailing.map(|icon| Icon::new(icon).small().text_color(muted))),
                ),
        )
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let (group, _) = self.sections.get(section)?;
        Some(
            Label::new(match group {
                Group::Actions => "Actions",
                Group::Apps => "Applications",
            })
            .px_3()
            .pt_2()
            .pb_1()
            .text_xs()
            .text_color(cx.theme().muted_foreground),
        )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let catalog = self.catalog.read(cx);
        let description = match &catalog.error {
            Some(e) => format!("Could not list the installed apps: {e}"),
            None if !catalog.loaded => "Loading apps…".to_string(),
            None => "No app or action has that name.".to_string(),
        };
        Empty::new().size_full().header(
            EmptyHeader::new()
                .title(EmptyTitle::new().child("No matches"))
                .description(EmptyDescription::new().child(description)),
        )
    }

    fn set_selected_index(
        &mut self,
        _: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        cx.notify();
    }
}

pub struct ActionPicker {
    page: Page,
    list: Entity<ListState<Entries>>,
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
    /// `action`: the one being replaced (its unknown fields are kept by the window).
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
        let url = input(url, "https://github.com", window, cx);
        let keys = input(keys, "win+1", window, cx);
        let file = input(file, r"C:\Users\me\Documents", window, cx);
        let program = input(program, r"C:\Windows\notepad.exe", window, cx);
        let args = input(args, "Arguments (one command line)", window, cx);
        let cwd = input(cwd, "Default: the program's folder", window, cx);

        let list = cx.new(|cx| {
            let mut entries = Entries {
                catalog: catalog.clone(),
                query: String::new(),
                sections: Vec::new(),
                current_app,
            };
            entries.refilter(cx);
            ListState::new(entries, window, cx).searchable(true)
        });
        // Start on the app the shortcut opens now, else on the first row.
        list.update(cx, |list, cx| {
            let current = list.delegate().current_app_path(cx);
            list.set_selected_index(Some(current.unwrap_or_default()), window, cx);
            if let Some(ix) = current {
                list.scroll_to_item(ix, ScrollStrategy::Center, window, cx);
            }
        });

        let mut subs = vec![
            // The apps (then their icons) arrive in the background.
            cx.observe(&catalog, |this: &mut Self, _, cx| {
                this.list.update(cx, |list, cx| {
                    list.delegate_mut().refilter(cx);
                    cx.notify();
                });
            }),
            cx.subscribe_in(&list, window, |this, list, ev: &ListEvent, window, cx| {
                // Enter or a click on a row.
                if let ListEvent::Confirm(ix) = ev {
                    match list.read(cx).delegate().row(*ix, cx) {
                        Some(Row::App(id)) => cx.emit(PickerEvent::Picked(Choice::App(id))),
                        Some(Row::Action(page)) => this.open_page(page, window, cx),
                        None => {}
                    }
                }
            }),
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
        Self {
            page,
            list,
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
        }
    }

    /// Moves the keyboard focus to the page's main field.
    pub fn focus_page(&self, window: &mut Window, cx: &mut App) {
        let input = match self.page {
            Page::List => return self.list.update(cx, |list, cx| list.focus(window, cx)),
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

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == Page::List {
            cx.emit(PickerEvent::Close);
        } else {
            self.open_page(Page::List, window, cx);
        }
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
            .child(Label::new(title).text_sm().font_semibold())
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

    /// An input with a "Browse…" button next to it.
    fn with_browse(
        &self,
        input: &Entity<InputState>,
        id: &'static str,
        target: Browse,
        cx: &mut Context<Self>,
    ) -> Div {
        h_flex()
            .gap_2()
            .child(div().flex_1().child(Input::new(input)))
            .child(browse_button(id, "Browse…", target, cx))
    }

    /// The form of an action that needs a value.
    fn render_form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let error = self.error.clone().map(Note::error);
        let (title, form, hint) = match self.page {
            Page::List => return div().into_any_element(),
            Page::Website => (
                "Open website",
                v_form().child(field("Address", Input::new(&self.url), error)),
                "https:, mailto:, ms-settings: or any other link. https:// is added when missing.",
            ),
            Page::SendKeys => (
                "Send keys",
                self.send_keys_form(error, cx),
                "Like win+1, ctrl+shift+esc or volume_mute: up to 4 modifiers and one key.",
            ),
            Page::OpenFile => (
                "Open file or folder",
                v_form()
                    .child(field("Path", Input::new(&self.file), error))
                    .child(
                        field(
                            "",
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
                            None,
                        )
                        .label_indent(false),
                    ),
                "Opens with its default app. %VARIABLES% are expanded when the shortcut runs.",
            ),
            Page::RunProgram => (
                "Run program",
                v_form().child(field(
                    "Program",
                    self.with_browse(&self.program, "browse-program", Browse::Program, cx),
                    error,
                )),
                "%VARIABLES% are expanded when the shortcut runs.",
            ),
        };
        let advanced = (self.page == Page::RunProgram).then(|| self.render_advanced(cx));
        v_flex()
            .gap_3()
            .p_3()
            .child(self.page_header(title, cx))
            .child(form)
            .children(advanced)
            .child(hint_text(hint, cx))
            .child(self.save_row(cx))
            .into_any_element()
    }

    /// "Send keys": the recorder button, what it recorded and the field to type them instead.
    fn send_keys_form(&self, error: Option<Note>, cx: &mut Context<Self>) -> Form {
        let recording = self.recording;
        let keys = self.keys.read(cx).value().to_string();
        let preview = model::validate_chord(&keys)
            .is_ok()
            .then(|| Note::hint(format!("Sends {}", model::chord_label(&keys))));
        let record = Button::new("record-chord")
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
            }));
        let record = if recording {
            record.primary()
        } else {
            record.outline()
        };
        v_form()
            .child(field("", record, preview).label_indent(false))
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
                error,
            ))
    }

    /// "Run program": arguments and working folder, folded away until needed.
    fn render_advanced(&self, cx: &mut Context<Self>) -> Collapsible {
        let open = self.advanced;
        Collapsible::new()
            .open(open)
            // In a row so the toggle sits on the left instead of stretching.
            .child(
                h_flex().child(
                    Button::new("advanced")
                        .small()
                        .ghost()
                        .icon(if open {
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
            .content(
                v_form()
                    .pt_3()
                    .child(field("Arguments", Input::new(&self.args), None))
                    .child(field(
                        "Working folder",
                        self.with_browse(&self.cwd, "browse-cwd", Browse::Cwd, cx),
                        None,
                    )),
            )
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
        .icon(match target {
            Browse::File | Browse::Program => IconName::File,
            Browse::Folder | Browse::Cwd => IconName::Folder,
        })
        .label(label)
        .on_click(cx.listener(move |this, _, window, cx| this.browse(target, window, cx)))
}

impl Render for ActionPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.page {
            Page::List => div()
                .h(px(LIST_HEIGHT))
                .child(List::new(&self.list).search_placeholder("Search apps and actions"))
                .into_any_element(),
            _ => self.render_form(cx),
        };
        v_flex()
            .id("action-picker")
            .w(px(PICKER_WIDTH))
            // Esc: back to the list from a form, else close. The input binds Escape, the list
            // and the popover bind Cancel; whichever arrives, it is handled here first.
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.escape(window, cx)
            }))
            .capture_action(cx.listener(|this, _: &Cancel, window, cx| {
                cx.stop_propagation();
                this.escape(window, cx)
            }))
            .child(content)
    }
}
