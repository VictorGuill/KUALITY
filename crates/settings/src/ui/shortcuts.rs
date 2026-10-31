//! Shortcuts tab: the switch, the binding list and the inline binding editor (with drafts).

use std::collections::BTreeMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::{
    Action, Binding, BindingEntry, Key, LaunchAction, LaunchArgs, OpenUrlAction, SendHotkeyAction,
    parse_key,
};

use super::controls::{
    self, add_button, divider, field, hint_text, list_header, section, setting_row, warning_text,
};
use super::{RecordTarget, SettingsWindow};
use crate::model;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionType {
    Launch,
    SendHotkey,
    OpenUrl,
}

const ACTION_TYPES: [(ActionType, &str); 3] = [
    (ActionType::Launch, "Launch"),
    (ActionType::SendHotkey, "Send hotkey"),
    (ActionType::OpenUrl, "Open URL"),
];

fn type_of(a: &Action) -> ActionType {
    match a {
        Action::Launch(_) => ActionType::Launch,
        Action::SendHotkey(_) => ActionType::SendHotkey,
        Action::OpenUrl(_) => ActionType::OpenUrl,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BField {
    Key,
    Path,
    Keys,
    Url,
}

type KeySelect = SelectState<SearchableVec<SharedString>>;

/// Inputs of the expanded binding. `index == None` is a draft not written yet.
pub(crate) struct BindingEditor {
    pub index: Option<usize>,
    /// The binding as edited: for a saved binding, a copy of the file's entry; for a draft, the
    /// values entered so far.
    work: Binding,
    action_type: ActionType,
    key_text: String,
    args_edited: bool,
    name: Entity<InputState>,
    key_select: Entity<KeySelect>,
    path: Entity<InputState>,
    args: Entity<InputState>,
    cwd: Entity<InputState>,
    keys: Entity<InputState>,
    url: Entity<InputState>,
    errors: BTreeMap<BField, String>,
    _subs: Vec<Subscription>,
}

impl BindingEditor {
    pub fn error_count(&self) -> usize {
        if self.index.is_none() {
            // A draft is not saved until complete; the row says so.
            return 0;
        }
        self.errors.len()
    }
}

/// Canonical names of every key that can be a trigger, in a friendly order.
fn trigger_key_names() -> Vec<SharedString> {
    let mut keys: Vec<(u8, String)> = (1u16..=0xFE)
        .map(Key)
        .filter(|k| k.is_trigger())
        .filter_map(|k| {
            let name = k.name();
            if name.starts_with("vk:") {
                return None;
            }
            let group = match k.0 {
                0x41..=0x5A => 0,
                0x30..=0x39 => 1,
                0x70..=0x87 => 2,
                0x60..=0x6F => 4,
                _ => 3,
            };
            Some((group, name))
        })
        .collect();
    keys.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| natural(&a.1).cmp(&natural(&b.1)))
    });
    keys.into_iter().map(|(_, n)| n.into()).collect()
}

/// Sort key that puts `f2` before `f10`.
fn natural(s: &str) -> (String, u32) {
    let digits: String = s.chars().rev().take_while(char::is_ascii_digit).collect();
    let n = digits
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    (
        s.trim_end_matches(|c: char| c.is_ascii_digit()).to_string(),
        n,
    )
}

fn empty_binding() -> Binding {
    Binding {
        key: String::new(),
        name: String::new(),
        enabled: true,
        action: Action::Launch(LaunchAction::default()),
        extra: Default::default(),
    }
}

fn action_summary(a: &Action) -> String {
    match a {
        Action::Launch(l) => {
            let name = std::path::Path::new(l.path.trim())
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| l.path.clone());
            format!("Launch {name}")
        }
        Action::SendHotkey(h) => format!("Send {}", h.keys),
        Action::OpenUrl(u) => format!("Open {}", u.url),
    }
}

fn key_label(key: &str) -> String {
    match parse_key(key) {
        Ok(k) => k.name().to_uppercase(),
        Err(_) if key.trim().is_empty() => "No key".into(),
        Err(_) => key.to_string(),
    }
}

impl SettingsWindow {
    fn binding(&self, index: usize) -> Option<&Binding> {
        match self.doc.config.shortcuts.bindings.get(index) {
            Some(BindingEntry::Valid(b)) => Some(b),
            _ => None,
        }
    }

    pub(super) fn open_binding(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.binding(index).cloned() else {
            self.binding_editor = None;
            return;
        };
        self.binding_editor = Some(self.make_binding_editor(Some(index), b, window, cx));
        self.binding_changed(window, cx, false);
    }

    fn add_binding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_recording(cx);
        self.binding_editor = Some(self.make_binding_editor(None, empty_binding(), window, cx));
        self.binding_changed(window, cx, false);
    }

    fn make_binding_editor(
        &mut self,
        index: Option<usize>,
        b: Binding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BindingEditor {
        let line = |value: String,
                    placeholder: &'static str,
                    window: &mut Window,
                    cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let (path, args, cwd, keys, url) = match &b.action {
            Action::Launch(l) => (
                l.path.clone(),
                model::args_display(&l.args),
                l.cwd.clone().unwrap_or_default(),
                String::new(),
                String::new(),
            ),
            Action::SendHotkey(h) => (
                String::new(),
                String::new(),
                String::new(),
                h.keys.clone(),
                String::new(),
            ),
            Action::OpenUrl(u) => (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                u.url.clone(),
            ),
        };
        let name = line(b.name.clone(), "Optional label", window, cx);
        let path = line(path, r"C:\Windows\notepad.exe", window, cx);
        let args = line(args, "Arguments (one command line)", window, cx);
        let cwd = line(cwd, "Default: the program's folder", window, cx);
        let keys = line(keys, "ctrl+shift+esc", window, cx);
        let url = line(url, "https://github.com", window, cx);
        let selected = parse_key(&b.key).ok().map(|k| SharedString::from(k.name()));
        let names = trigger_key_names();
        let selected_ix = selected
            .as_ref()
            .and_then(|s| names.iter().position(|n| n == s))
            .map(gpui_kit::component::IndexPath::new);
        let key_select = cx.new(|cx| {
            SelectState::new(SearchableVec::new(names), selected_ix, window, cx).searchable(true)
        });
        let mut subs = vec![cx.subscribe_in(
            &key_select,
            window,
            |this, _, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else {
                    return;
                };
                if let Some(ed) = this.binding_editor.as_mut() {
                    ed.key_text = v.to_string();
                }
                this.binding_changed(window, cx, true);
            },
        )];
        for (input, is_args) in [
            (&name, false),
            (&path, false),
            (&args, true),
            (&cwd, false),
            (&keys, false),
            (&url, false),
        ] {
            subs.push(cx.subscribe_in(
                input,
                window,
                move |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        if is_args && let Some(ed) = this.binding_editor.as_mut() {
                            ed.args_edited = true;
                        }
                        this.binding_changed(window, cx, true);
                    }
                },
            ));
        }
        BindingEditor {
            index,
            action_type: type_of(&b.action),
            key_text: b.key.clone(),
            work: b,
            args_edited: false,
            name,
            key_select,
            path,
            args,
            cwd,
            keys,
            url,
            errors: BTreeMap::new(),
            _subs: subs,
        }
    }

    /// Validates the editor and writes every valid part (or the whole draft once complete).
    /// `edited` = triggered by the user (else only the messages are refreshed).
    fn binding_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>, edited: bool) {
        if self.read_only() {
            return;
        }
        let Some(ed) = self.binding_editor.as_mut() else {
            return;
        };
        let read = |e: &Entity<InputState>| e.read(cx).value().to_string();
        let name = read(&ed.name);
        let path = read(&ed.path).trim().to_string();
        let args = read(&ed.args);
        let cwd = read(&ed.cwd).trim().to_string();
        let keys = read(&ed.keys).trim().to_string();
        let url = read(&ed.url).trim().to_string();

        let mut errors = BTreeMap::new();
        let key_ok = match model::validate_trigger(&ed.key_text) {
            Ok(_) => true,
            Err(e) => {
                errors.insert(BField::Key, e.to_string());
                false
            }
        };
        let required = match ed.action_type {
            ActionType::Launch => {
                model::validate_path(&path).map_err(|e| (BField::Path, e.to_string()))
            }
            ActionType::SendHotkey => model::validate_chord(&keys).map_err(|e| (BField::Keys, e)),
            ActionType::OpenUrl => {
                model::validate_url(&url).map_err(|e| (BField::Url, e.to_string()))
            }
        };
        let action_ok = match required {
            Ok(()) => true,
            Err((f, e)) => {
                errors.insert(f, e);
                false
            }
        };

        // Build the new action, keeping unknown fields and untouched values of the same type.
        let old = ed.work.action.clone();
        let same_type = type_of(&old) == ed.action_type;
        let action = match ed.action_type {
            ActionType::Launch => {
                let prev = match &old {
                    Action::Launch(l) => l.clone(),
                    _ => LaunchAction::default(),
                };
                Action::Launch(LaunchAction {
                    path: if action_ok || !same_type {
                        path
                    } else {
                        prev.path.clone()
                    },
                    args: if ed.args_edited {
                        if args.trim().is_empty() {
                            LaunchArgs::List(Vec::new())
                        } else {
                            LaunchArgs::Raw(args)
                        }
                    } else {
                        prev.args.clone()
                    },
                    cwd: (!cwd.is_empty()).then_some(cwd),
                    extra: prev.extra,
                })
            }
            ActionType::SendHotkey => {
                let prev = match &old {
                    Action::SendHotkey(h) => h.clone(),
                    _ => SendHotkeyAction::default(),
                };
                Action::SendHotkey(SendHotkeyAction {
                    keys: if action_ok || !same_type {
                        keys
                    } else {
                        prev.keys.clone()
                    },
                    extra: prev.extra,
                })
            }
            ActionType::OpenUrl => {
                let prev = match &old {
                    Action::OpenUrl(u) => u.clone(),
                    _ => OpenUrlAction::default(),
                };
                Action::OpenUrl(OpenUrlAction {
                    url: if action_ok || !same_type {
                        url
                    } else {
                        prev.url.clone()
                    },
                    extra: prev.extra,
                })
            }
        };
        ed.errors = errors;
        ed.work.name = name;
        if key_ok {
            ed.work.key = ed.key_text.clone();
        }
        // A new action type replaces the old one only once it is complete.
        if same_type || action_ok {
            ed.work.action = action;
        }

        let mut save = false;
        match ed.index {
            Some(i) => {
                if edited
                    && let Some(BindingEntry::Valid(b)) =
                        self.doc.config.shortcuts.bindings.get_mut(i)
                {
                    let mut work = ed.work.clone();
                    work.enabled = b.enabled;
                    if *b != work {
                        *b = work;
                        save = true;
                    }
                }
            }
            None => {
                if edited && key_ok && action_ok {
                    let mut b = ed.work.clone();
                    b.key = ed.key_text.clone();
                    self.doc
                        .config
                        .shortcuts
                        .bindings
                        .push(BindingEntry::Valid(b));
                    ed.index = Some(self.doc.config.shortcuts.bindings.len() - 1);
                    save = true;
                }
            }
        }
        if save {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }

    fn set_action_type(&mut self, t: ActionType, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ed) = self.binding_editor.as_mut() {
            ed.action_type = t;
        }
        self.binding_changed(window, cx, true);
    }

    pub(super) fn apply_recorded(
        &mut self,
        target: RecordTarget,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ed) = self.binding_editor.as_mut() else {
            return;
        };
        match target {
            RecordTarget::Trigger => {
                ed.key_text = text.clone();
                let value = SharedString::from(text);
                ed.key_select
                    .update(cx, |s, cx| s.set_selected_value(&value, window, cx));
            }
            RecordTarget::Chord => {
                ed.keys.update(cx, |s, cx| s.set_value(text, window, cx));
            }
        }
        self.binding_changed(window, cx, true);
    }

    fn browse(&mut self, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
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
                let Some(ed) = this.binding_editor.as_ref() else {
                    return;
                };
                let input = if folder {
                    ed.cwd.clone()
                } else {
                    ed.path.clone()
                };
                input.update(cx, |s, cx| {
                    s.set_value(path.display().to_string(), window, cx)
                });
                this.binding_changed(window, cx, true);
            });
        })
        .detach();
    }

    fn confirm_delete_binding(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("Delete this shortcut?")
                .description("It is removed from config.json right away.")
                .ok_text("Delete")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.delete_binding(index, cx));
                    true
                })
        });
    }

    fn delete_binding(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.read_only() || index >= self.doc.config.shortcuts.bindings.len() {
            return;
        }
        self.cancel_recording(cx);
        self.doc.config.shortcuts.bindings.remove(index);
        match self.binding_editor.as_ref().and_then(|e| e.index) {
            Some(i) if i == index => self.binding_editor = None,
            Some(i) if i > index => {
                if let Some(ed) = self.binding_editor.as_mut() {
                    ed.index = Some(i - 1);
                }
            }
            _ => {}
        }
        self.changed(cx);
    }

    fn toggle_binding_editor(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_recording(cx);
        if self
            .binding_editor
            .as_ref()
            .is_some_and(|e| e.index == Some(index))
        {
            self.binding_editor = None;
            cx.notify();
        } else {
            self.open_binding(index, window, cx);
        }
    }

    pub(super) fn render_shortcuts(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ro = self.read_only();
        let s = &self.doc.config.shortcuts;
        let hyper_off = !self.doc.config.hyperkey.enabled;
        let enabled = Switch::new("shortcuts-enabled")
            .accessibility_label("Enable shortcuts")
            .checked(s.enabled)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.shortcuts.enabled = *on;
                this.changed(cx);
            }));
        let count = s.bindings.len();
        let section_off = !s.enabled;
        let mut rows: Vec<AnyElement> = (0..count)
            .map(|i| self.render_binding_row(i, window, cx))
            .collect();
        if self
            .binding_editor
            .as_ref()
            .is_some_and(|e| e.index.is_none())
        {
            rows.push(self.render_draft_row(window, cx));
        }
        let has_draft = self
            .binding_editor
            .as_ref()
            .is_some_and(|e| e.index.is_none());
        v_flex()
            .gap_4()
            .child(section(cx).child(setting_row(
                "Enable shortcuts",
                Some("Hold CapsLock (Hyper) and press a key to run an action.".into()),
                enabled,
                cx,
            )))
            .when(hyper_off, |this| {
                this.child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(warning_text(
                            "Shortcuts only work while Hyperkey is enabled.",
                            cx,
                        ))
                        .child(
                            Button::new("enable-hyperkey")
                                .small()
                                .outline()
                                .label("Enable Hyperkey")
                                .disabled(ro)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.doc.config.hyperkey.enabled = true;
                                    this.changed(cx);
                                })),
                        ),
                )
            })
            .child(list_header(
                "Shortcuts",
                add_button(
                    "add-binding",
                    "Add shortcut",
                    ro || has_draft,
                    cx,
                    |this, window, cx| this.add_binding(window, cx),
                ),
                cx,
            ))
            .when(section_off && count > 0, |this| {
                this.child(warning_text(
                    "Shortcuts are off: turn on \"Enable shortcuts\" to use them.",
                    cx,
                ))
            })
            .when(rows.is_empty(), |this| {
                this.child(hint_text("No shortcuts yet. Add one to start.", cx))
            })
            .children(rows)
    }

    fn key_badge(&self, key: &str, cx: &App) -> Div {
        h_flex()
            .gap_1()
            .items_center()
            .text_xs()
            .child(
                div()
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().secondary)
                    .child("Hyper"),
            )
            .child("+")
            .child(
                div()
                    .px_1p5()
                    .py_0p5()
                    .min_w(px(24.))
                    .rounded_sm()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().secondary)
                    .child(key_label(key)),
            )
    }

    fn render_binding_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ro = self.read_only();
        let open = self
            .binding_editor
            .as_ref()
            .is_some_and(|e| e.index == Some(index));
        let delete = Button::new(("delete-binding", index))
            .small()
            .ghost()
            .icon(IconName::Delete)
            .tooltip("Delete")
            .accessibility_label(format!("Delete shortcut {index}"))
            .disabled(ro)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_delete_binding(index, window, cx)
            }));
        let b = match &self.doc.config.shortcuts.bindings[index] {
            BindingEntry::Invalid(_) => {
                return section(cx)
                    .child(
                        h_flex()
                            .px_4()
                            .py_2()
                            .justify_between()
                            .child(hint_text("Unrecognized shortcut (kept as is)", cx))
                            .child(delete),
                    )
                    .into_any_element();
            }
            BindingEntry::Valid(b) => b.clone(),
        };
        let duplicate = model::duplicate_trigger(&self.doc.config.shortcuts.bindings, index);
        let toggle = Switch::new(("binding-enabled", index))
            .accessibility_label(format!("Shortcut {index} enabled"))
            .checked(b.enabled)
            .disabled(ro)
            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                if let Some(BindingEntry::Valid(b)) =
                    this.doc.config.shortcuts.bindings.get_mut(index)
                {
                    b.enabled = *on;
                    if let Some(ed) = this.binding_editor.as_mut()
                        && ed.index == Some(index)
                    {
                        ed.work.enabled = *on;
                    }
                    this.changed(cx);
                }
            }));
        let summary = action_summary(&b.action);
        let title = if b.name.trim().is_empty() {
            summary.clone()
        } else {
            b.name.clone()
        };
        let expand = Button::new(("expand-binding", index))
            .small()
            .ghost()
            .icon(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .disabled(ro)
            .accessibility_label(format!("Edit shortcut {index}"))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_binding_editor(index, window, cx)
            }));
        let header = h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .items_center()
            .child(toggle)
            .child(self.key_badge(&b.key, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(title))
                    .child(hint_text(summary, cx)),
            )
            .when(duplicate && b.enabled, |this| {
                this.child(warning_text("Same key", cx))
            })
            .child(expand)
            .child(delete);
        let mut card = section(cx).child(header);
        if open {
            card = card
                .child(divider(cx))
                .child(self.render_binding_editor(window, cx));
        }
        card.into_any_element()
    }

    fn render_draft_row(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = self
            .binding_editor
            .as_ref()
            .map(|e| e.key_text.clone())
            .unwrap_or_default();
        section(cx)
            .border_color(cx.theme().primary)
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .child(self.key_badge(&key, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().text_sm().child("New shortcut"))
                            .child(hint_text(
                                "Not saved yet: it is written once the key and the action are set.",
                                cx,
                            )),
                    )
                    .child(
                        Button::new("discard-draft")
                            .small()
                            .ghost()
                            .label("Discard")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_recording(cx);
                                this.binding_editor = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(divider(cx))
            .child(self.render_binding_editor(window, cx))
            .into_any_element()
    }

    fn render_binding_editor(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(ed) = self.binding_editor.as_ref() else {
            return div().into_any_element();
        };
        let ro = self.read_only();
        let recording = self.recording_target();
        let errors = ed.errors.clone();
        let err = |f: BField| errors.get(&f).map(String::as_str);
        let duplicate = ed
            .index
            .is_some_and(|i| model::duplicate_trigger(&self.doc.config.shortcuts.bindings, i));
        let (name, key_select, path, args, cwd, keys, url) = (
            ed.name.clone(),
            ed.key_select.clone(),
            ed.path.clone(),
            ed.args.clone(),
            ed.cwd.clone(),
            ed.keys.clone(),
            ed.url.clone(),
        );
        let action_type = ed.action_type;
        let key_text = ed.key_text.clone();
        let record_button = |id: &'static str, target: RecordTarget, cx: &mut Context<Self>| {
            let active = recording == Some(target);
            Button::new(id)
                .small()
                .when(active, |b| b.primary())
                .when(!active, |b| b.outline())
                .label(if active {
                    "Press a key… (Esc to cancel)"
                } else {
                    "Record"
                })
                .disabled(ro)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if active {
                        this.cancel_recording(cx);
                    } else {
                        this.start_recording(target, window, cx);
                    }
                }))
        };
        let type_group = controls::segmented(
            "action-type",
            &ACTION_TYPES,
            Some(action_type),
            ro,
            cx,
            |this, t, window, cx| this.set_action_type(t, window, cx),
        );
        let placeholder = if key_text.trim().is_empty() {
            "Choose a key".to_string()
        } else {
            key_label(&key_text)
        };
        let key_row = h_flex()
            .gap_2()
            .child(
                Select::new(&key_select)
                    .small()
                    .w(px(180.))
                    .placeholder(placeholder)
                    .search_placeholder("Search keys")
                    .disabled(ro),
            )
            .child(record_button("record-key", RecordTarget::Trigger, cx));

        let action_fields =
            match action_type {
                ActionType::Launch => v_flex()
                    .gap_3()
                    .child(field(
                        "Program, file or folder",
                        h_flex()
                            .gap_2()
                            .child(Input::new(&path).small().flex_1().disabled(ro))
                            .child(
                                Button::new("browse-path")
                                    .small()
                                    .outline()
                                    .icon(IconName::FolderOpen)
                                    .label("Browse…")
                                    .disabled(ro)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.browse(false, window, cx)
                                    })),
                            ),
                        err(BField::Path),
                        cx,
                    ))
                    .child(field(
                        "Arguments",
                        Input::new(&args).small().disabled(ro),
                        None,
                        cx,
                    ))
                    .child(field(
                        "Working folder",
                        h_flex()
                            .gap_2()
                            .child(Input::new(&cwd).small().flex_1().disabled(ro))
                            .child(
                                Button::new("browse-cwd")
                                    .small()
                                    .outline()
                                    .icon(IconName::Folder)
                                    .label("Browse…")
                                    .disabled(ro)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.browse(true, window, cx)
                                    })),
                            ),
                        None,
                        cx,
                    ))
                    .child(hint_text(
                        "%VARIABLES% are expanded when the shortcut runs.",
                        cx,
                    )),
                ActionType::SendHotkey => v_flex()
                    .gap_3()
                    .child(field(
                        "Key combination",
                        h_flex()
                            .gap_2()
                            .child(Input::new(&keys).small().w(px(260.)).disabled(ro))
                            .child(record_button("record-chord", RecordTarget::Chord, cx)),
                        err(BField::Keys),
                        cx,
                    ))
                    .child(hint_text(
                        "Like ctrl+shift+esc, win+d or volume_mute. Up to 4 modifiers and one key.",
                        cx,
                    )),
                ActionType::OpenUrl => v_flex()
                    .gap_3()
                    .child(field(
                        "URL",
                        Input::new(&url).small().disabled(ro),
                        err(BField::Url),
                        cx,
                    ))
                    .child(hint_text(
                        "https:, mailto:, ms-settings: and any other registered scheme.",
                        cx,
                    )),
            };

        v_flex()
            .px_4()
            .py_4()
            .gap_4()
            .child(field(
                "Name",
                Input::new(&name).small().w(px(320.)).disabled(ro),
                None,
                cx,
            ))
            .child(field(
                "Key (pressed with Hyper)",
                key_row,
                err(BField::Key),
                cx,
            ))
            .when(recording == Some(RecordTarget::Trigger), |this| {
                this.child(hint_text(
                    "Press the key alone or with CapsLock. A key already used by another \
                     shortcut may be taken by Kuality: choose it in the list instead.",
                    cx,
                ))
            })
            .when(duplicate, |this| {
                this.child(warning_text(
                    "Only the first enabled shortcut with this key works.",
                    cx,
                ))
            })
            .child(field("Action", type_group, None, cx))
            .child(action_fields)
            .into_any_element()
    }
}
