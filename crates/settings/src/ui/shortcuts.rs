//! Shortcuts tab: the switch and a two-column table, one row per shortcut:
//! `[enabled] | Hotkey (recorder button) | Action (button opening the picker) | delete`.
//! A new shortcut is a draft row until it has both a hotkey and an action.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::{Action, Binding, BindingEntry, parse_key};

use super::controls::{
    add_button, divider, hint_text, list_header, section, setting_row, warning_text,
};
use super::picker::{ActionPicker, PickerEvent, kind_icon};
use super::{RecordTarget, SettingsWindow};
use crate::model::{self, ActionKind, Choice};

/// A row of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// `shortcuts.bindings[i]`.
    Saved(usize),
    /// The new shortcut, not written yet.
    Draft,
}

impl Row {
    fn id(self) -> usize {
        match self {
            Row::Saved(i) => i,
            Row::Draft => usize::MAX,
        }
    }

    fn name(self) -> String {
        match self {
            Row::Saved(i) => format!("shortcut {i}"),
            Row::Draft => "new shortcut".into(),
        }
    }
}

/// The new shortcut: written once both parts are set; dropped on close.
#[derive(Debug, Default)]
pub(crate) struct Draft {
    key: String,
    action: Option<Action>,
}

/// The action picker, open for one row.
pub(crate) struct OpenPicker {
    pub row: Row,
    pub view: Entity<ActionPicker>,
    _sub: Subscription,
}

const HOTKEY_WIDTH: f32 = 168.;
const SWITCH_WIDTH: f32 = 40.;

impl SettingsWindow {
    fn binding(&self, index: usize) -> Option<&Binding> {
        match self.doc.config.shortcuts.bindings.get(index) {
            Some(BindingEntry::Valid(b)) => Some(b),
            _ => None,
        }
    }

    fn row_action(&self, row: Row) -> Option<&Action> {
        match row {
            Row::Saved(i) => self.binding(i).map(|b| &b.action),
            Row::Draft => self.draft.as_ref().and_then(|d| d.action.as_ref()),
        }
    }

    fn open_picker(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only() {
            return;
        }
        self.cancel_recording(cx);
        let action = self.row_action(row).cloned();
        let catalog = self.catalog.clone();
        let view = cx.new(|cx| ActionPicker::new(action.as_ref(), catalog, window, cx));
        view.update(cx, |p, cx| p.focus_page(window, cx));
        let sub = cx.subscribe_in(
            &view,
            window,
            move |this, _, ev: &PickerEvent, window, cx| {
                this.on_picker_event(row, ev.clone(), window, cx)
            },
        );
        self.picker = Some(OpenPicker {
            row,
            view,
            _sub: sub,
        });
        cx.notify();
    }

    pub(super) fn close_picker(&mut self, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            if self.recording_target() == Some(RecordTarget::Chord) {
                self.cancel_recording(cx);
            }
            cx.notify();
        }
    }

    fn on_picker_event(
        &mut self,
        row: Row,
        ev: PickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            PickerEvent::Picked(choice) => {
                self.close_picker(cx);
                self.apply_choice(row, choice, cx);
            }
            PickerEvent::RecordKeys => {
                self.start_recording(RecordTarget::Chord, window, cx);
            }
            PickerEvent::StopRecording => self.cancel_recording(cx),
            PickerEvent::Close => self.close_picker(cx),
        }
    }

    fn apply_choice(&mut self, row: Row, choice: Choice, cx: &mut Context<Self>) {
        if self.read_only() {
            return;
        }
        match row {
            Row::Saved(i) => {
                if let Some(BindingEntry::Valid(b)) = self.doc.config.shortcuts.bindings.get_mut(i)
                {
                    let action = model::build_action(choice, Some(&b.action));
                    if b.action != action {
                        b.action = action;
                        self.changed(cx);
                    }
                }
            }
            Row::Draft => {
                if let Some(d) = self.draft.as_mut() {
                    d.action = Some(model::build_action(choice, None));
                }
                self.commit_draft(cx);
            }
        }
        cx.notify();
    }

    /// A recording finished with `text` (a key name or a chord).
    pub(super) fn apply_recorded(
        &mut self,
        target: RecordTarget,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            RecordTarget::Chord => {
                if let Some(p) = self.picker.as_ref() {
                    p.view.update(cx, |p, cx| p.set_keys(text, window, cx));
                }
            }
            RecordTarget::Trigger(row) => {
                if self.read_only() || model::validate_trigger(&text).is_err() {
                    return;
                }
                match row {
                    Row::Saved(i) => {
                        if let Some(BindingEntry::Valid(b)) =
                            self.doc.config.shortcuts.bindings.get_mut(i)
                            && b.key != text
                        {
                            b.key = text;
                            self.changed(cx);
                        }
                    }
                    Row::Draft => {
                        let Some(d) = self.draft.as_mut() else {
                            return;
                        };
                        d.key = text;
                        let needs_action = d.action.is_none();
                        self.commit_draft(cx);
                        // Raycast-style: hotkey first, then straight to "what does it do".
                        if needs_action && self.draft.is_some() {
                            self.open_picker(Row::Draft, window, cx);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// Writes the draft once it has a valid hotkey and an action.
    fn commit_draft(&mut self, cx: &mut Context<Self>) {
        let ready = self
            .draft
            .as_ref()
            .is_some_and(|d| d.action.is_some() && model::validate_trigger(&d.key).is_ok());
        if !ready || self.read_only() {
            return;
        }
        let Some(Draft {
            key,
            action: Some(action),
        }) = self.draft.take()
        else {
            return;
        };
        self.doc
            .config
            .shortcuts
            .bindings
            .push(BindingEntry::Valid(Binding {
                key,
                name: String::new(),
                enabled: true,
                action,
                extra: Default::default(),
            }));
        if self.picker.as_ref().is_some_and(|p| p.row == Row::Draft) {
            self.close_picker(cx);
        }
        self.changed(cx);
    }

    fn add_binding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_picker(cx);
        self.draft = Some(Draft::default());
        self.start_recording(RecordTarget::Trigger(Row::Draft), window, cx);
    }

    fn discard_draft(&mut self, cx: &mut Context<Self>) {
        if self.recording_target() == Some(RecordTarget::Trigger(Row::Draft)) {
            self.cancel_recording(cx);
        }
        if self.picker.as_ref().is_some_and(|p| p.row == Row::Draft) {
            self.close_picker(cx);
        }
        self.draft = None;
        cx.notify();
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
        if self
            .picker
            .as_ref()
            .is_some_and(|p| matches!(p.row, Row::Saved(_)))
        {
            self.close_picker(cx);
        }
        self.doc.config.shortcuts.bindings.remove(index);
        self.changed(cx);
    }

    fn toggle_hotkey_recording(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording_target() == Some(RecordTarget::Trigger(row)) {
            self.cancel_recording(cx);
        } else {
            self.close_picker(cx);
            self.start_recording(RecordTarget::Trigger(row), window, cx);
        }
    }

    // --- Rendering -----------------------------------------------------------------------

    pub(super) fn render_shortcuts(
        &mut self,
        _window: &mut Window,
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
        let mut rows: Vec<Row> = (0..count).map(Row::Saved).collect();
        if self.draft.is_some() {
            rows.push(Row::Draft);
        }
        let has_draft = self.draft.is_some();
        let table = (!rows.is_empty()).then(|| self.render_table(&rows, cx));
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
                this.child(hint_text(
                    "No shortcuts yet. Add one: press its hotkey, then choose what it does.",
                    cx,
                ))
            })
            .children(table)
    }

    fn render_table(&mut self, rows: &[Row], cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let header = h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .text_xs()
            .font_medium()
            .text_color(muted)
            .child(div().w(px(SWITCH_WIDTH)).flex_none())
            .child(div().w(px(HOTKEY_WIDTH)).flex_none().child("Hotkey"))
            .child(div().flex_1().child("Action"));
        let mut table = section(cx).child(header);
        for &row in rows {
            table = table.child(divider(cx)).child(self.render_row(row, cx));
        }
        table.into_any_element()
    }

    fn render_row(&mut self, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let ro = self.read_only();
        let id = row.id();
        let delete = Button::new(("delete-binding", id))
            .small()
            .ghost()
            .icon(IconName::Delete)
            .tooltip(if row == Row::Draft {
                "Discard"
            } else {
                "Delete"
            })
            .accessibility_label(format!("Delete {}", row.name()))
            .disabled(ro)
            .on_click(cx.listener(move |this, _, window, cx| match row {
                Row::Saved(i) => this.confirm_delete_binding(i, window, cx),
                Row::Draft => this.discard_draft(cx),
            }));
        let (key, enabled, action) = match row {
            Row::Saved(i) => match &self.doc.config.shortcuts.bindings[i] {
                BindingEntry::Invalid(_) => {
                    return h_flex()
                        .px_4()
                        .py_2()
                        .gap_3()
                        .items_center()
                        .child(div().w(px(SWITCH_WIDTH)).flex_none())
                        .child(
                            div()
                                .flex_1()
                                .child(hint_text("Unrecognized shortcut (kept as is)", cx)),
                        )
                        .child(delete)
                        .into_any_element();
                }
                BindingEntry::Valid(b) => (b.key.clone(), b.enabled, Some(b.action.clone())),
            },
            Row::Draft => {
                let d = self.draft.as_ref();
                (
                    d.map(|d| d.key.clone()).unwrap_or_default(),
                    true,
                    d.and_then(|d| d.action.clone()),
                )
            }
        };
        let toggle = match row {
            Row::Saved(i) => Switch::new(("binding-enabled", i))
                .accessibility_label(format!("Shortcut {i} enabled"))
                .checked(enabled)
                .disabled(ro)
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    if let Some(BindingEntry::Valid(b)) =
                        this.doc.config.shortcuts.bindings.get_mut(i)
                    {
                        b.enabled = *on;
                        this.changed(cx);
                    }
                }))
                .into_any_element(),
            Row::Draft => div().into_any_element(),
        };
        let recording = self.recording_target() == Some(RecordTarget::Trigger(row));
        let messages = self.row_messages(row, &key, action.as_ref(), recording, cx);
        h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .items_start()
            .child(
                div()
                    .w(px(SWITCH_WIDTH))
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(toggle),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .when(!enabled, |this| this.opacity(0.55))
                            .child(self.hotkey_button(row, &key, recording, cx))
                            .child(self.action_button(row, action.as_ref(), cx)),
                    )
                    .children(messages),
            )
            .child(div().h(px(32.)).flex().items_center().child(delete))
            .into_any_element()
    }

    /// Hints and warnings shown under a row.
    fn row_messages(
        &self,
        row: Row,
        key: &str,
        action: Option<&Action>,
        recording: bool,
        cx: &App,
    ) -> Vec<Div> {
        let mut out = Vec::new();
        if recording {
            out.push(hint_text(
                "Hold CapsLock and press a key (or just press the key). Esc cancels.",
                cx,
            ));
        } else if !key.trim().is_empty() && model::validate_trigger(key).is_err() {
            out.push(warning_text(
                "This key is not valid in config.json: record a new hotkey.",
                cx,
            ));
        } else {
            let duplicate = match row {
                Row::Saved(i) => model::duplicate_trigger(&self.doc.config.shortcuts.bindings, i),
                Row::Draft => parse_key(key).is_ok_and(|k| {
                    self.doc
                        .config
                        .shortcuts
                        .bindings
                        .iter()
                        .any(|e| matches!(e, BindingEntry::Valid(b) if b.enabled && parse_key(&b.key) == Ok(k)))
                }),
            };
            let enabled = match row {
                Row::Saved(i) => self.binding(i).is_some_and(|b| b.enabled),
                Row::Draft => true,
            };
            if duplicate && enabled {
                out.push(warning_text(
                    "Another shortcut uses this hotkey: only the first one works.",
                    cx,
                ));
            }
        }
        if let Some(problem) = action.and_then(action_problem) {
            out.push(warning_text(problem, cx));
        }
        if row == Row::Draft {
            out.push(hint_text(
                "New shortcut: it is saved once it has a hotkey and an action.",
                cx,
            ));
        }
        out
    }

    fn hotkey_button(
        &self,
        row: Row,
        key: &str,
        recording: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = if recording {
            "Press Hyper + a key…".to_string()
        } else {
            model::hotkey_label(key).unwrap_or_else(|| "Record hotkey".into())
        };
        div().w(px(HOTKEY_WIDTH)).flex_none().child(
            Button::new(("hotkey", row.id()))
                .w_full()
                .when(recording, |b| b.primary())
                .when(!recording, |b| b.outline())
                .icon(Icon::new(gpui_kit::assets::IconName::Keyboard))
                .label(label)
                .accessibility_label(format!("Hotkey of {}", row.name()))
                .disabled(self.read_only())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_hotkey_recording(row, window, cx)
                })),
        )
    }

    fn action_button(
        &self,
        row: Row,
        action: Option<&Action>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let catalog = self.catalog.read(cx);
        let muted = cx.theme().muted_foreground;
        let (label, icon) = match action {
            None => ("Choose an action".to_string(), None),
            Some(a) => {
                let label = model::action_label(a, |id| catalog.name_of(id));
                let kind = model::kind_of(a);
                let app_icon = match a {
                    Action::Launch(l) => model::app_id(&l.path)
                        .and_then(|id| catalog.find(id))
                        .and_then(|i| catalog.icon(i)),
                    _ => None,
                };
                let icon = match app_icon {
                    Some(image) => img(image).size(px(18.)).flex_none().into_any_element(),
                    None => Icon::new(kind_icon(kind))
                        .size(px(18.))
                        .text_color(muted)
                        .into_any_element(),
                };
                (label, Some(icon))
            }
        };
        let is_empty = action.is_none();
        let open = self.picker.as_ref().is_some_and(|p| p.row == row);
        let view = self
            .picker
            .as_ref()
            .filter(|p| p.row == row)
            .map(|p| p.view.clone());
        let trigger = Button::new(("action", row.id()))
            .outline()
            .w_full()
            .disabled(self.read_only())
            .accessibility_label(format!("Action of {}", row.name()))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .children(icon)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_left()
                            .when(is_empty, |d| d.text_color(muted))
                            .child(label),
                    )
                    .child(Icon::new(IconName::ChevronDown).small().text_color(muted)),
            );
        let weak = cx.entity().downgrade();
        // The trigger fills what the hotkey button leaves.
        let mut trigger_style = StyleRefinement {
            flex_grow: Some(1.),
            flex_shrink: Some(1.),
            flex_basis: Some(relative(0.).into()),
            ..Default::default()
        };
        trigger_style.min_size.width = Some(px(0.).into());
        Popover::new(("action-picker", row.id()))
            .anchor(Anchor::TopRight)
            .open(open)
            .trigger(trigger)
            .trigger_style(trigger_style)
            .on_open_change(move |open, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if *open {
                        this.open_picker(row, window, cx);
                    } else {
                        this.close_picker(cx);
                    }
                });
            })
            .content(move |_, _, _| match &view {
                Some(view) => view.clone().into_any_element(),
                None => div().into_any_element(),
            })
    }
}

/// Why the core would skip this action, if it would (hand-edited values).
fn action_problem(action: &Action) -> Option<String> {
    let problem = match (model::kind_of(action), action) {
        (_, Action::SendHotkey(h)) => model::validate_chord(&h.keys).err(),
        (_, Action::OpenUrl(u)) => model::validate_url(&u.url).err().map(str::to_string),
        (ActionKind::App, _) => None,
        (_, Action::Launch(l)) => model::validate_path(&l.path).err().map(str::to_string),
    };
    problem.map(|p| format!("{p}: choose the action again."))
}
