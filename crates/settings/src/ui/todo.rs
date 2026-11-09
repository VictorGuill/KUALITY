//! To do tab: the feature switch and how the taskbar bar shows it. The items themselves are
//! edited in the to-do popup (`kuality.exe --todo`), not here.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::{DEFAULT_TODO_LABEL, NewItems};

use super::SettingsWindow;
use super::controls::{self, divider, hint_text, section, setting_row, warning_text};
use crate::model;

const NEW_ITEMS: [(NewItems, &str); 2] = [(NewItems::Top, "Top"), (NewItems::Bottom, "Bottom")];

/// The label input of the To do tab, created on first render.
pub(crate) struct LabelField {
    input: Entity<InputState>,
    _sub: Subscription,
}

impl SettingsWindow {
    fn set_todo_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let config = &mut self.doc.config;
        config.todo.enabled = on;
        if on {
            // Turning the feature on also puts it on the taskbar (opt-in happens here).
            model::ensure_todo_bar(&mut config.widgets);
        }
        self.changed(cx);
    }

    fn label_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(f) = &self.todo_label {
            return f.input.clone();
        }
        let value = model::todo_module(&self.doc.config.widgets)
            .map(|(m, _)| m.label.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_TODO_LABEL)
                .default_value(value)
        });
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _, cx| {
            if !matches!(ev, InputEvent::Change) || this.read_only() {
                return;
            }
            let text = input.read(cx).value().trim().to_string();
            if let Some(m) = model::todo_module_mut(&mut this.doc.config.widgets)
                && m.label != text
            {
                m.label = text;
                this.changed(cx);
            }
        });
        self.todo_label = Some(LabelField {
            input: input.clone(),
            _sub: sub,
        });
        input
    }

    pub(super) fn render_todo(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ro = self.read_only();
        let enabled = self.doc.config.todo.enabled;
        let widgets_on = self.doc.config.widgets.enabled;
        let module = model::todo_module(&self.doc.config.widgets).map(|(m, on)| (m.clone(), on));
        let new_items = self.doc.config.todo.new_items.valid().copied();
        let label = module.as_ref().map(|_| self.label_field(window, cx));

        let toggle = Switch::new("todo-enabled")
            .accessibility_label("Enable to-do")
            .checked(enabled)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| this.set_todo_enabled(*on, cx)));
        let order = controls::segmented(
            "todo-new-items",
            &NEW_ITEMS,
            new_items,
            ro,
            cx,
            |this, v, _, cx| {
                this.doc.config.todo.new_items = kuality_protocol::Lenient::Valid(v);
                this.changed(cx);
            },
        );
        let open_list = Button::new("todo-open")
            .small()
            .secondary()
            .label("Open to-do list")
            .on_click(|_, _, _| crate::open_todo_popup());

        let mut main = section(cx)
            .child(setting_row(
                "Enable to-do",
                Some(
                    "A small \"TO DO\" on the taskbar; click it to add items, check them off and \
                     delete them."
                        .into(),
                ),
                toggle,
                cx,
            ))
            .child(divider(cx))
            .child(setting_row(
                "New items go to",
                Some("Where an item you add appears in the list.".into()),
                order,
                cx,
            ))
            .child(divider(cx))
            .child(setting_row(
                "To-do list",
                Some("Items are stored on this PC only (todos.json next to config.json).".into()),
                open_list,
                cx,
            ));
        if let (Some((m, _)), Some(label)) = (&module, label) {
            let show_count = m.show_count;
            main = main
                .child(divider(cx))
                .child(setting_row(
                    "Taskbar label",
                    None,
                    Input::new(&label).small().w(px(180.)).disabled(ro),
                    cx,
                ))
                .child(divider(cx))
                .child(setting_row(
                    "Show the number of pending items",
                    Some("\"TO DO (3)\"; nothing is added when the list is done.".into()),
                    Switch::new("todo-count")
                        .accessibility_label("Show the number of pending items")
                        .checked(show_count)
                        .disabled(ro)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            if let Some(m) = model::todo_module_mut(&mut this.doc.config.widgets) {
                                m.show_count = *on;
                                this.changed(cx);
                            }
                        })),
                    cx,
                ));
        }

        let bar_on = module.as_ref().is_some_and(|(_, on)| *on);
        v_flex()
            .gap_4()
            .child(main)
            .when(enabled && !widgets_on, |this| {
                this.child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(warning_text(
                            "Taskbar widgets are off, so the to-do list is not on the taskbar.",
                            cx,
                        ))
                        .child(
                            Button::new("todo-widgets-on")
                                .small()
                                .secondary()
                                .label("Turn on")
                                .disabled(ro)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.doc.config.widgets.enabled = true;
                                    this.changed(cx);
                                })),
                        ),
                )
            })
            .when(enabled && !bar_on, |this| {
                this.child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(warning_text("No taskbar bar shows the to-do list.", cx))
                        .child(
                            Button::new("todo-add-bar")
                                .small()
                                .secondary()
                                .label("Add to the taskbar")
                                .disabled(ro)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if model::ensure_todo_bar(&mut this.doc.config.widgets) {
                                        this.changed(cx);
                                    }
                                })),
                        ),
                )
            })
            .child(hint_text(
                "Its position and font are set in Widgets, on the bar that holds it.",
                cx,
            ))
    }
}
