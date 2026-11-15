//! The App lock prompt (`kuality.exe --unlock`, see `docs/specs/app-lock.md`): asks for the
//! Kuality password to unlock a program (`--app <path>`) or to run a tray command
//! (`--confirm quit|pause`). The core checks the password over its pipe; this window never
//! sees the stored hash.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::app_lock::{ConfirmAction, Request, Response, normalize_path};
use kuality_protocol::{Config, cli};

use crate::clipboard::center_popup;
use crate::platform::{self, UNLOCK_WINDOW_TITLE};
use crate::ui::window;

/// Popup size in DIPs.
const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 220.0;
/// While the prompt is not in front yet, it retries taking the foreground this often…
const FOREGROUND_RETRY: Duration = Duration::from_millis(100);
/// …this many times.
const FOREGROUND_TRIES: u32 = 15;

/// What the prompt is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// Unlock the program at this path.
    Unlock(String),
    /// Run a tray command.
    Confirm(ConfirmAction),
}

/// `--app <path>` or `--confirm <action>` from the command line.
pub fn purpose_arg(args: &[String]) -> Option<Purpose> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == cli::APP {
            return it.next().map(|p| Purpose::Unlock(p.clone()));
        }
        if a == cli::CONFIRM {
            return it
                .next()
                .and_then(|v| ConfirmAction::from_arg(v))
                .map(Purpose::Confirm);
        }
    }
    None
}

/// The name shown for a locked program: its `name` in `config.json`, else the exe's
/// description, else its file name.
fn app_name(path: &str, config: Option<&Config>) -> String {
    let key = normalize_path(path);
    let configured = config.and_then(|c| {
        c.app_lock
            .apps
            .iter()
            .find(|a| normalize_path(&a.path) == key)
            .map(|a| a.name.trim().to_string())
    });
    configured
        .filter(|n| !n.is_empty())
        .or_else(|| platform::exe_description(path))
        .unwrap_or_else(|| {
            kuality_protocol::LockedApp {
                path: path.to_string(),
                ..Default::default()
            }
            .display_name()
        })
}

/// Texts of the prompt.
struct Texts {
    title: String,
    body: &'static str,
    action: &'static str,
}

fn texts(purpose: &Purpose, name: &str) -> Texts {
    match purpose {
        Purpose::Unlock(_) => Texts {
            title: format!("{name} is locked"),
            body: "Enter your Kuality password to open it.",
            action: "Unlock",
        },
        Purpose::Confirm(ConfirmAction::Quit) => Texts {
            title: "Quit Kuality?".into(),
            body: "App lock is on. Enter your Kuality password to quit.",
            action: "Quit",
        },
        Purpose::Confirm(ConfirmAction::Pause) => Texts {
            title: "Pause Kuality?".into(),
            body: "App lock pauses too. Enter your Kuality password to pause.",
            action: "Pause",
        },
    }
}

/// Opens the prompt and runs until it closes.
pub fn run(core: platform::CoreProcess, purpose: Purpose, config: Option<Config>) {
    let (name, icon) = match &purpose {
        Purpose::Unlock(path) => (
            app_name(path, config.as_ref()),
            platform::file_icon(path).and_then(crate::apps::to_image),
        ),
        Purpose::Confirm(_) => (String::new(), None),
    };
    let anchor = platform::cursor_pos();
    window::run_window(
        "App lock prompt",
        move |cx| {
            let monitor = platform::monitor_at(anchor);
            let origin = monitor.map(|m| center_popup(m.work, m.scale, (WIDTH, HEIGHT)));
            let (bounds, display_id) =
                window::popup_bounds(monitor, origin, size(px(WIDTH), px(HEIGHT)), cx);
            let mut options = window::popup_options(
                UNLOCK_WINDOW_TITLE,
                "Kuality.Unlock",
                bounds,
                display_id,
                true,
            );
            // A normal window: it shows in the taskbar so it cannot get lost behind others.
            options.kind = WindowKind::Normal;
            Some(options)
        },
        |window| platform::popup_window_setup(window, true),
        move |window, cx| UnlockPrompt::new(core, purpose, name, icon, window, cx),
    );
}

struct UnlockPrompt {
    core_pid: u32,
    purpose: Purpose,
    texts: Texts,
    icon: Option<Arc<RenderImage>>,
    input: Entity<InputState>,
    error: Option<String>,
    /// A request is on its way to the core.
    busy: bool,
    _subs: Vec<Subscription>,
}

impl UnlockPrompt {
    fn new(
        core: platform::CoreProcess,
        purpose: Purpose,
        name: String,
        icon: Option<Arc<RenderImage>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let core_pid = core.pid;
        window::watch_core(core, cx, |_, cx| {
            log::info!("Kuality exited; closing the App lock prompt");
            cx.quit();
        });
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Password")
        });
        input.update(cx, |s, cx| s.focus(window, cx));
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
        let subs =
            vec![cx.subscribe_in(
                &input,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change if this.error.is_some() && !this.busy => {
                        this.error = None;
                        cx.notify();
                    }
                    _ => {}
                },
            )];
        Self {
            core_pid,
            texts: texts(&purpose, &name),
            purpose,
            icon,
            input,
            error: None,
            busy: false,
            _subs: subs,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let password = self.input.read(cx).value().to_string();
        if password.is_empty() {
            self.error = Some("Type your Kuality password.".into());
            cx.notify();
            return;
        }
        let request = match &self.purpose {
            Purpose::Unlock(app) => {
                // The core shows the app's windows again and brings it to the front.
                platform::allow_foreground(self.core_pid);
                Request::AppLockUnlock {
                    app: app.clone(),
                    password,
                }
            }
            Purpose::Confirm(action) => Request::AppLockConfirm {
                action: *action,
                password,
            },
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let pid = self.core_pid;
        cx.spawn_in(window, async move |this, cx| {
            let reply = cx
                .background_executor()
                .spawn(async move { platform::pipe_request::<Response>(pid, request) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match reply {
                    Ok((r, _)) if r.ok => {
                        cx.quit();
                        return;
                    }
                    Ok((r, _)) => {
                        this.error = Some(r.error.unwrap_or_else(|| "Wrong password.".into()))
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                this.input.update(cx, |s, cx| {
                    s.set_value("", window, cx);
                    s.focus(window, cx);
                });
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for UnlockPrompt {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let leading = match &self.icon {
            Some(icon) => img(icon.clone())
                .size_8()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => Icon::new(Lucide::Lock)
                .large()
                .text_color(theme.muted_foreground)
                .into_any_element(),
        };
        v_flex()
            .id("unlock-prompt")
            .key_context("UnlockPrompt")
            .capture_action(cx.listener(|_, _: &Escape, _, cx| cx.quit()))
            .size_full()
            .p_5()
            .gap_3()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        h_flex()
                            .size_10()
                            .flex_none()
                            .justify_center()
                            .child(leading),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                Label::new(self.texts.title.clone())
                                    .text_lg()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .truncate(),
                            )
                            .child(
                                Label::new(self.texts.body)
                                    .text_sm()
                                    .text_color(theme.muted_foreground),
                            ),
                    ),
            )
            .child(Input::new(&self.input).mask_toggle().disabled(self.busy))
            .child(
                h_flex()
                    .min_h_5()
                    .when_some(self.error.clone(), |row, e| {
                        row.child(Label::new(e).text_sm().text_color(theme.danger))
                    })
                    .when(self.busy, |row| {
                        row.child(
                            Label::new("Checking…")
                                .text_sm()
                                .text_color(theme.muted_foreground),
                        )
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("unlock-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|_, _, _, cx| cx.quit())),
                    )
                    .child(
                        Button::new("unlock-ok")
                            .primary()
                            .label(self.texts.action)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Purpose, app_name, purpose_arg};
    use kuality_protocol::app_lock::ConfirmAction;
    use kuality_protocol::{Config, LockedApp};

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn purpose_from_arguments() {
        assert_eq!(
            purpose_arg(&args(&["--app", "C:\\b.exe", "--core-pid", "1"])),
            Some(Purpose::Unlock("C:\\b.exe".into()))
        );
        assert_eq!(
            purpose_arg(&args(&["--core-pid", "1", "--confirm", "quit"])),
            Some(Purpose::Confirm(ConfirmAction::Quit))
        );
        assert_eq!(purpose_arg(&args(&["--confirm", "x"])), None);
        assert_eq!(purpose_arg(&args(&["--app"])), None);
        assert_eq!(purpose_arg(&args(&[])), None);
    }

    #[test]
    fn configured_names_win() {
        let mut config = Config::default();
        config.app_lock.apps.push(LockedApp {
            path: "C:\\Apps\\Tool.exe".into(),
            name: "My tool".into(),
            ..Default::default()
        });
        assert_eq!(app_name("c:/apps/tool.EXE", Some(&config)), "My tool");
        // Not in the config and not a real file: the file name.
        assert_eq!(app_name("Z:\\nowhere\\thing.exe", Some(&config)), "thing");
    }
}
