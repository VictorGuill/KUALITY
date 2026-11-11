//! Clipboard tab: the clipboard history switch, its hotkey and limits, and buttons that talk to
//! the running core over its pipe (`docs/specs/clipboard.md`).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::clipboard::Request;
use kuality_protocol::{DEFAULT_CLIPBOARD_HOTKEY, Lenient};

use super::controls::{
    divider, hint_text, hyper_keycap, keycaps, section, setting_row, warning_text,
};
use super::{RecordTarget, SettingsWindow};
use crate::{model, platform};

/// "Keep the last" choices.
const KEEP: [(u32, &str); 4] = [(50, "50"), (100, "100"), (200, "200"), (500, "500")];
const HOTKEY_WIDTH: f32 = 160.;

/// Result of the last button press, shown under the buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PipeNote {
    Done(&'static str),
    Failed(String),
}

impl SettingsWindow {
    fn clipboard_hotkey(&self) -> String {
        self.doc
            .config
            .clipboard
            .hotkey
            .valid()
            .cloned()
            .unwrap_or_default()
    }

    /// A recorded hotkey for the clipboard history.
    pub(super) fn apply_clipboard_hotkey(&mut self, text: String, cx: &mut Context<Self>) {
        if self.read_only() || model::validate_trigger(&text).is_err() {
            return;
        }
        if self.doc.config.clipboard.hotkey.valid() != Some(&text) {
            self.doc.config.clipboard.hotkey = Lenient::Valid(text);
            self.changed(cx);
        }
    }

    fn toggle_clipboard_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording_target() == Some(RecordTarget::ClipboardHotkey) {
            self.cancel_recording(cx);
        } else {
            self.start_recording(RecordTarget::ClipboardHotkey, window, cx);
        }
    }

    /// Sends a request to the running core's clipboard history.
    fn clipboard_request(&mut self, request: Request, done: &'static str, cx: &mut Context<Self>) {
        self.clipboard_confirm_pins = false;
        self.clipboard_note = Some(match self.core_pid {
            None => PipeNote::Failed("Kuality is not running.".into()),
            Some(pid) => match platform::pipe_request(pid, request) {
                Ok((r, _)) if r.ok => PipeNote::Done(done),
                Ok((r, _)) => PipeNote::Failed(r.error.unwrap_or_default()),
                Err(e) => PipeNote::Failed(e.to_string()),
            },
        });
        cx.notify();
    }

    fn clipboard_hotkey_button(&self, recording: bool, cx: &mut Context<Self>) -> Div {
        let key = self.clipboard_hotkey();
        let caps = (!recording).then(|| model::hotkey_caps(&key)).flatten();
        let button = Button::new("clipboard-hotkey")
            .w_full()
            .when(recording, |b| b.primary())
            .when(!recording, |b| b.outline())
            .accessibility_label("Clipboard history hotkey")
            .when_some(model::hotkey_label(&key).filter(|_| !recording), |b, l| {
                b.tooltip(l)
            })
            .disabled(self.read_only())
            .on_click(
                cx.listener(|this, _, window, cx| this.toggle_clipboard_recording(window, cx)),
            );
        let button = match caps {
            Some(caps) => button.child(keycaps(&caps, cx)),
            None => button
                .icon(Icon::new(gpui_kit::assets::IconName::Keyboard))
                .label(if recording {
                    "Press a hotkey…"
                } else {
                    "Record hotkey"
                }),
        };
        div().w(px(HOTKEY_WIDTH)).flex_none().child(button)
    }

    pub(super) fn render_clipboard(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let ro = self.read_only();
        let c = &self.doc.config.clipboard;
        let enabled = c.enabled;
        let images = c.images;
        let keep = c.max_entries.valid().copied();
        let key = self.clipboard_hotkey();
        let recording = self.recording_target() == Some(RecordTarget::ClipboardHotkey);
        let hyper_off =
            enabled && !self.doc.config.hyperkey.enabled && model::is_hyper_hotkey(&key);
        let invalid_key = !recording && model::validate_trigger(&key).is_err();
        let duplicate =
            enabled && !invalid_key && model::clipboard_hotkey_conflict(&self.doc.config).is_some();
        let core = self.core_pid.is_some() && self.core_running;
        let pipe_ready = core && enabled;

        let toggle = Switch::new("clipboard-enabled")
            .accessibility_label("Enable clipboard history")
            .checked(enabled)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.clipboard.enabled = *on;
                this.clipboard_note = None;
                this.changed(cx);
            }));
        let keep_control =
            super::controls::segmented("clipboard-keep", &KEEP, keep, ro, cx, |this, v, _, cx| {
                this.doc.config.clipboard.max_entries = Lenient::Valid(v);
                this.changed(cx);
            });
        let images_switch = Switch::new("clipboard-images")
            .accessibility_label("Record images")
            .checked(images)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.clipboard.images = *on;
                this.changed(cx);
            }));

        let mut hotkey_notes: Vec<Div> = Vec::new();
        if recording {
            if let Some((_, why)) = self
                .record_message
                .filter(|(t, _)| *t == RecordTarget::ClipboardHotkey)
            {
                hotkey_notes.push(warning_text(why, cx));
            }
            hotkey_notes.push(hint_text(
                "Hold CapsLock (Hyper) or Ctrl / Alt / Shift / Win and press a key. Esc cancels.",
                cx,
            ));
        } else if invalid_key {
            hotkey_notes.push(warning_text(
                "This hotkey is not valid: record a new one.",
                cx,
            ));
        } else {
            hotkey_notes.push(hint_text(
                "Record Win + V to replace the Windows clipboard history.",
                cx,
            ));
        }
        if duplicate {
            hotkey_notes.push(warning_text(
                "A shortcut uses this hotkey too: the clipboard history wins.",
                cx,
            ));
        }

        let hotkey_button = self
            .clipboard_hotkey_button(recording, cx)
            .into_any_element();
        let confirm = self.clipboard_confirm_pins;
        let buttons = h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("clipboard-open")
                    .small()
                    .secondary()
                    .label("Open clipboard history")
                    .disabled(!pipe_ready)
                    .on_click(
                        cx.listener(|this, _, _, _| crate::open_clipboard_popup(this.core_pid)),
                    ),
            )
            .child(
                Button::new("clipboard-clear")
                    .small()
                    .secondary()
                    .label("Clear history")
                    .disabled(!pipe_ready)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clipboard_request(Request::Clear, "History cleared.", cx)
                    })),
            )
            .child(
                Button::new("clipboard-clear-pins")
                    .small()
                    .when(confirm, |b| b.danger())
                    .when(!confirm, |b| b.secondary())
                    .label(if confirm {
                        "Click again to remove all pins"
                    } else {
                        "Remove all pins"
                    })
                    .disabled(!pipe_ready)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if confirm {
                            this.clipboard_request(Request::ClearPins, "Pins removed.", cx);
                        } else {
                            this.clipboard_confirm_pins = true;
                            this.clipboard_note = None;
                            cx.notify();
                        }
                    })),
            );

        let main = section(cx)
            .child(setting_row(
                "Enable clipboard history",
                Some(
                    "Keeps what you copy (text and images) and opens it with a hotkey. The \
                     history stays in memory only, encrypted; pinned entries are saved encrypted \
                     for your Windows account."
                        .into(),
                ),
                toggle,
                cx,
            ))
            .child(divider(cx))
            .child(
                v_flex()
                    .child(setting_row(
                        "Hotkey",
                        Some(format!("Opens the history (default Hyper + V; config: {DEFAULT_CLIPBOARD_HOTKEY}).").into()),
                        hotkey_button,
                        cx,
                    ))
                    .child(v_flex().px_5().pb_3().gap_1().children(hotkey_notes)),
            )
            .child(divider(cx))
            .child(setting_row(
                "Keep the last",
                Some("Entries kept, newest first. Pinned entries do not count.".into()),
                keep_control,
                cx,
            ))
            .child(divider(cx))
            .child(setting_row(
                "Record images",
                Some("Screenshots and copied pictures (up to 64 MB of images in memory).".into()),
                images_switch,
                cx,
            ));

        v_flex()
            .gap_4()
            .child(main)
            .when(hyper_off, |this| {
                this.child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(hyper_keycap())
                        .child(warning_text(
                            "A Hyper hotkey only works while Hyperkey is enabled.",
                            cx,
                        ))
                        .child(
                            Button::new("clipboard-enable-hyperkey")
                                .small()
                                .secondary()
                                .label("Enable Hyperkey")
                                .disabled(ro)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.doc.config.hyperkey.enabled = true;
                                    this.changed(cx);
                                })),
                        ),
                )
            })
            .child(buttons)
            .when(!enabled, |this| {
                this.child(hint_text(
                    "Turn the history on to open, clear or unpin it.",
                    cx,
                ))
            })
            .when_some(self.clipboard_note.clone(), |this, note| {
                this.child(match note {
                    PipeNote::Done(t) => hint_text(t, cx),
                    PipeNote::Failed(e) => warning_text(e, cx),
                })
            })
            .child(hint_text(
                "Contents that password managers mark as private are never recorded. With an \
                 app running as administrator in front, the hotkey needs Kuality to run as \
                 administrator too.",
                cx,
            ))
    }
}
