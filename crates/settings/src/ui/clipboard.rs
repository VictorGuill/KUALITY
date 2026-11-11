//! Clipboard tab: the clipboard history switch, its hotkey and limits, and buttons that talk to
//! the running core over its pipe (`docs/specs/clipboard.md`).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingPage};
use gpui_kit::component::{Disableable as _, Sizable as _, WindowExt as _, h_flex};
use gpui_kit::*;
use kuality_protocol::clipboard::Request;
use kuality_protocol::{DEFAULT_CLIPBOARD_HOTKEY, Lenient};

use super::controls::{
    self, Note, Weak, block_item, control_item, hotkey_button, notes_item, switch_item,
};
use super::{RecordTarget, SettingsWindow};
use crate::{model, platform};

/// "Keep the last" choices.
const KEEP: [(u32, &str); 4] = [(50, "50"), (100, "100"), (200, "200"), (500, "500")];

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

    /// Asks before removing every pinned entry (they are deleted from disk too).
    fn confirm_clear_pins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("Remove all pins?")
                .description("Every pinned clipboard entry is deleted. This cannot be undone.")
                .ok_text("Remove")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.clipboard_request(Request::ClearPins, "Pins removed.", cx)
                    });
                    true
                })
        });
    }

    pub(super) fn clipboard_page(&self, weak: &Weak, _cx: &mut Context<Self>) -> SettingPage {
        let ro = self.read_only();
        let config = &self.doc.config;
        let enabled = config.clipboard.enabled;
        let key = self.clipboard_hotkey();
        let recording = self.recording_target() == Some(RecordTarget::ClipboardHotkey);
        let hyper_off = enabled && !config.hyperkey.enabled && model::is_hyper_hotkey(&key);
        let invalid_key = !recording && model::validate_trigger(&key).is_err();
        let duplicate =
            enabled && !invalid_key && model::clipboard_hotkey_conflict(config).is_some();

        let mut hotkey_notes = Vec::new();
        if recording {
            if let Some((_, why)) = self
                .record_message
                .filter(|(t, _)| *t == RecordTarget::ClipboardHotkey)
            {
                hotkey_notes.push(Note::warning(why));
            }
            hotkey_notes.push(Note::hint(
                "Hold CapsLock (Hyper) or Ctrl / Alt / Shift / Win and press a key. Esc cancels.",
            ));
        } else if invalid_key {
            hotkey_notes.push(Note::warning("This hotkey is not valid: record a new one."));
        } else {
            hotkey_notes.push(Note::hint(
                "Record Win + V to replace the Windows clipboard history.",
            ));
        }
        if duplicate {
            hotkey_notes.push(Note::warning(
                "A shortcut uses this hotkey too: the clipboard history wins.",
            ));
        }

        let mut history_notes = Vec::new();
        if !enabled {
            history_notes.push(Note::hint(
                "Turn the history on to open, clear or unpin it.",
            ));
        }
        match &self.clipboard_note {
            Some(PipeNote::Done(t)) => history_notes.push(Note::hint(*t)),
            Some(PipeNote::Failed(e)) => history_notes.push(Note::warning(e.clone())),
            None => {}
        }
        history_notes.push(Note::hint(
            "Contents that password managers mark as private are never recorded. With an app \
             running as administrator in front, the hotkey needs Kuality to run as administrator \
             too.",
        ));

        let mut main = SettingGroup::new()
            .item(
                switch_item(
                    "Enable clipboard history",
                    weak,
                    |w| w.doc.config.clipboard.enabled,
                    |w, on, cx| {
                        w.doc.config.clipboard.enabled = on;
                        w.clipboard_note = None;
                        w.changed(cx);
                    },
                )
                .description(
                    "Keeps what you copy (text and images) and opens it with a hotkey. The \
                     history stays in memory only, encrypted; pinned entries are saved encrypted \
                     for your Windows account.",
                )
                .disabled(ro),
            )
            .item(
                control_item("Hotkey", weak, |this, _, cx| {
                    let recording = this.recording_target() == Some(RecordTarget::ClipboardHotkey);
                    hotkey_button(
                        "clipboard-hotkey",
                        &this.clipboard_hotkey(),
                        recording,
                        "Clipboard history hotkey",
                        cx,
                    )
                    .disabled(this.read_only())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_clipboard_recording(window, cx)
                    }))
                })
                .description(format!(
                    "Opens the history (default Hyper + V; config: {DEFAULT_CLIPBOARD_HOTKEY})."
                ))
                .disabled(ro),
            )
            .items(notes_item(hotkey_notes));
        if hyper_off {
            main = main.item(block_item(weak, |_, _, cx| {
                controls::hyper_off_row(
                    "clipboard-enable-hyperkey",
                    "A Hyper hotkey only works while Hyperkey is enabled.",
                    cx,
                )
            }));
        }
        main = main
            .item(
                control_item("Keep the last", weak, |this, _, cx| {
                    controls::segmented(
                        "clipboard-keep",
                        &KEEP,
                        this.doc.config.clipboard.max_entries.valid().copied(),
                        this.read_only(),
                        cx,
                        |this, v, _, cx| {
                            this.doc.config.clipboard.max_entries = Lenient::Valid(v);
                            this.changed(cx);
                        },
                    )
                })
                .description("Entries kept, newest first. Pinned entries do not count.")
                .disabled(ro),
            )
            .item(
                switch_item(
                    "Record images",
                    weak,
                    |w| w.doc.config.clipboard.images,
                    |w, on, cx| {
                        w.doc.config.clipboard.images = on;
                        w.changed(cx);
                    },
                )
                .description("Screenshots and copied pictures (up to 64 MB of images in memory).")
                .disabled(ro),
            );

        let history = SettingGroup::new()
            .title("History")
            .item(
                block_item(weak, |this, _, cx| {
                    // They talk to the running core: nothing to do without it or with the history off.
                    let ready = this.core_pid.is_some()
                        && this.core_running
                        && this.doc.config.clipboard.enabled;
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("clipboard-open")
                                .small()
                                .label("Open clipboard history")
                                .disabled(!ready)
                                .on_click(cx.listener(|this, _, _, _| {
                                    crate::open_clipboard_popup(this.core_pid)
                                })),
                        )
                        .child(
                            Button::new("clipboard-clear")
                                .small()
                                .label("Clear history")
                                .disabled(!ready)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.clipboard_request(Request::Clear, "History cleared.", cx)
                                })),
                        )
                        .child(
                            Button::new("clipboard-clear-pins")
                                .small()
                                .danger()
                                .label("Remove all pins")
                                .disabled(!ready)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clipboard_note = None;
                                    this.confirm_clear_pins(window, cx);
                                })),
                        )
                })
                .keywords(["history", "clear", "pins", "open"]),
            )
            .items(notes_item(history_notes));

        SettingPage::new("Clipboard")
            .icon(gpui_kit::assets::IconName::Clipboard)
            .group(main)
            .group(history)
    }
}
