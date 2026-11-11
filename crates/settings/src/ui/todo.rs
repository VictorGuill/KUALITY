//! To do tab: the feature switch and how the taskbar bar shows it. The items themselves are
//! edited in the to-do popup (`kuality.exe --todo`), not here.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::setting::{SettingGroup, SettingPage};
use gpui_kit::component::{Disableable as _, Sizable as _, v_flex};
use gpui_kit::*;
use kuality_protocol::{DEFAULT_TODO_LABEL, NewItems};

use super::SettingsWindow;
use super::controls::{
    self, Note, Weak, block_item, control_item, fix_row, notes_item, switch_item,
};
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

    pub(super) fn todo_page(&self, weak: &Weak, _cx: &mut Context<Self>) -> SettingPage {
        let ro = self.read_only();
        let config = &self.doc.config;
        let enabled = config.todo.enabled;
        let module = model::todo_module(&config.widgets).map(|(_, on)| on);
        let bar_on = module == Some(true);
        let widgets_off = enabled && !config.widgets.enabled;
        let no_bar = enabled && !bar_on;

        let mut main = SettingGroup::new()
            .item(
                switch_item(
                    "Enable to-do",
                    weak,
                    |w| w.doc.config.todo.enabled,
                    |w, on, cx| w.set_todo_enabled(on, cx),
                )
                .description(
                    "A small \"TO DO\" on the taskbar; click it to add items, check them off and delete them.",
                )
                .disabled(ro),
            )
            .item(
                control_item("New items go to", weak, |this, _, cx| {
                    controls::segmented(
                        "todo-new-items",
                        &NEW_ITEMS,
                        this.doc.config.todo.new_items.valid().copied(),
                        this.read_only(),
                        cx,
                        |this, v, _, cx| {
                            this.doc.config.todo.new_items = kuality_protocol::Lenient::Valid(v);
                            this.changed(cx);
                        },
                    )
                })
                .description("Where an item you add appears in the list.")
                .disabled(ro),
            )
            .item(
                control_item("To-do list", weak, |_, _, _| {
                    Button::new("todo-open")
                        .small()
                        .label("Open to-do list")
                        .on_click(|_, _, _| crate::open_todo_popup())
                })
                .description("Items are stored on this PC only (todos.json next to config.json)."),
            );
        if widgets_off || no_bar {
            main = main.item(block_item(weak, move |this, _, cx| {
                let ro = this.read_only();
                v_flex()
                    .gap_2()
                    .children(widgets_off.then(|| {
                        fix_row(
                            "Taskbar widgets are off, so the to-do list is not on the taskbar.",
                            Button::new("todo-widgets-on")
                                .label("Turn on")
                                .disabled(ro)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.doc.config.widgets.enabled = true;
                                    this.changed(cx);
                                })),
                            cx,
                        )
                    }))
                    .children(no_bar.then(|| {
                        fix_row(
                            "No taskbar bar shows the to-do list.",
                            Button::new("todo-add-bar")
                                .label("Add to the taskbar")
                                .disabled(ro)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if model::ensure_todo_bar(&mut this.doc.config.widgets) {
                                        this.changed(cx);
                                    }
                                })),
                            cx,
                        )
                    }))
            }));
        }
        let page = SettingPage::new("To do")
            .icon(gpui_kit::assets::IconName::ListTodo)
            .group(main);
        if module.is_none() {
            return page;
        }
        page.group(
            SettingGroup::new()
                .title("On the taskbar")
                .item(
                    control_item("Taskbar label", weak, |this, window, cx| {
                        let label = this.label_field(window, cx);
                        Input::new(&label)
                            .small()
                            .w(px(180.))
                            .disabled(this.read_only())
                    })
                    .disabled(ro),
                )
                .item(
                    switch_item(
                        "Show the number of pending items",
                        weak,
                        |w| model::todo_module(&w.doc.config.widgets).is_some_and(|(m, _)| m.show_count),
                        |w, on, cx| {
                            if let Some(m) = model::todo_module_mut(&mut w.doc.config.widgets) {
                                m.show_count = on;
                                w.changed(cx);
                            }
                        },
                    )
                    .description("\"TO DO (3)\"; nothing is added when the list is done.")
                    .disabled(ro),
                )
                .item(
                    switch_item(
                        "Open when hovered",
                        weak,
                        |w| {
                            model::todo_module(&w.doc.config.widgets)
                                .is_some_and(|(m, _)| m.open_on_hover)
                        },
                        |w, on, cx| {
                            if let Some(m) = model::todo_module_mut(&mut w.doc.config.widgets) {
                                m.open_on_hover = on;
                                w.changed(cx);
                            }
                        },
                    )
                    .description(
                        "Resting the mouse on it shows the list; it closes when the mouse leaves, unless you click it.",
                    )
                    .disabled(ro),
                )
                .items(notes_item(vec![Note::hint(
                    "Its position and font are set in Widgets, on the bar that holds it.",
                )])),
        )
    }
}
