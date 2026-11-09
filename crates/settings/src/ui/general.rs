//! General tab: start at logon, updates, log level and where the config lives.

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use kuality_protocol::LogLevel;
use kuality_update::Release;

use super::SettingsWindow;
use super::controls::{self, divider, error_text, hint_text, section, setting_row, warning_text};
use crate::startup::{self, Entry, TaskState};

/// Start-at-logon state shown by the General tab. The system entry (Run value or scheduled
/// task) is the source of truth, read off the UI thread when the window opens and after each
/// change; nothing is stored in `config.json`.
#[derive(Debug, Default)]
pub struct StartupView {
    /// `None` until the first read finishes.
    state: Option<startup::State>,
    /// A change is running (maybe waiting for the UAC prompt).
    busy: bool,
    /// Why the last change failed.
    error: Option<String>,
}

/// Runs `work` on its own thread (it may wait for a UAC prompt for a long time) and hands the
/// result to `done` on the UI thread.
fn off_thread<T: Send + 'static>(
    cx: &mut Context<SettingsWindow>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut SettingsWindow, T, &mut Context<SettingsWindow>) + 'static,
) {
    let (tx, rx) = futures_channel::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("kuality-settings-worker".into())
        .spawn(move || {
            let _ = tx.send(work());
        });
    if let Err(e) = spawned {
        log::error!("cannot start a worker thread: {e}");
        return;
    }
    cx.spawn(async move |this, cx| {
        if let Ok(result) = rx.await {
            let _ = this.update(cx, |this, cx| {
                done(this, result, cx);
                cx.notify();
            });
        }
    })
    .detach();
}

impl SettingsWindow {
    /// Reads the current startup entry in the background.
    pub(super) fn load_startup(&mut self, cx: &mut Context<Self>) {
        off_thread(cx, startup::read, |this, state, _| {
            log::info!("startup entry: {state:?}");
            this.startup.state = Some(state);
        });
    }

    /// Changes the startup entry (one UAC prompt when the scheduled task changes).
    fn set_startup(&mut self, want: Entry, cx: &mut Context<Self>) {
        if self.startup.busy {
            return;
        }
        self.startup.busy = true;
        self.startup.error = None;
        cx.notify();
        off_thread(
            cx,
            move || (startup::set(want), startup::read()),
            move |this, (result, state), _| {
                this.startup.busy = false;
                this.startup.state = Some(state);
                match result {
                    Ok(()) => log::info!("startup entry set to {want:?}"),
                    Err(startup::Error::Cancelled) => log::info!("UAC prompt cancelled"),
                    Err(startup::Error::Failed(e)) => {
                        log::error!("cannot set the startup entry: {e}");
                        this.startup.error = Some(e);
                    }
                }
            },
        );
    }

    fn render_startup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = &self.startup;
        let entry = view.state.as_ref().map(startup::State::entry);
        let ready = entry.is_some() && !view.busy;
        let on = entry.is_some_and(|e| e != Entry::Off);
        let elevated = entry == Some(Entry::Elevated);

        let start = Switch::new("startup-enabled")
            .accessibility_label("Start with Windows")
            .checked(on)
            .disabled(!ready)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.set_startup(if *on { Entry::Normal } else { Entry::Off }, cx);
            }));
        let admin = Switch::new("startup-elevated")
            .accessibility_label("Run as administrator")
            .checked(elevated)
            .disabled(!ready || !on)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.set_startup(if *on { Entry::Elevated } else { Entry::Normal }, cx);
            }));

        let exe = std::env::current_exe().ok();
        let other = view
            .state
            .as_ref()
            .zip(exe.as_deref())
            .and_then(|(s, exe)| s.other_copy(exe));
        let unknown = match view.state.as_ref().map(|s| &s.task) {
            Some(TaskState::Unknown(why)) => Some(why.clone()),
            _ => None,
        };
        let mut notes: Vec<Div> = Vec::new();
        if view.busy {
            notes.push(hint_text("Applying… Windows may ask for permission.", cx));
        }
        if let Some(path) = other {
            notes.push(warning_text(
                format!(
                    "Starts another copy of Kuality: {}. Turn this off and on to start this one.",
                    path.display()
                ),
                cx,
            ));
        }
        if let Some(why) = unknown {
            notes.push(warning_text(
                format!("Could not check the administrator task: {why}"),
                cx,
            ));
        }
        if let Some(e) = view.error.clone() {
            notes.push(error_text(e, cx));
        }
        let notes = (!notes.is_empty()).then(|| v_flex().gap_1().px_5().pb_4().children(notes));

        section(cx)
            .child(setting_row(
                "Start with Windows",
                Some("Start Kuality when you sign in.".into()),
                start,
                cx,
            ))
            .child(divider(cx))
            .child(setting_row(
                "Run as administrator",
                Some(
                    "Start it with administrator rights, so Hyperkey and shortcuts also work in \
                     apps running as administrator. Windows asks for permission when you change \
                     this."
                        .into(),
                ),
                admin,
                cx,
            ))
            .children(notes)
    }
}

/// Updates section (`docs/specs/updates.md`). Checks run in the background when the window
/// opens and on "Check now"; installing downloads the installer, which then closes Kuality and
/// this window and starts Kuality again.
#[derive(Debug, Default)]
pub struct UpdatesView {
    checking: bool,
    /// Result of the last check: `Ok(None)` = up to date.
    result: Option<Result<Option<Release>, String>>,
    /// This is the installed copy (else "Download" opens the release page).
    self_update: bool,
    installing: bool,
    install_error: Option<String>,
}

impl SettingsWindow {
    /// Checks GitHub for a newer release in the background.
    pub(super) fn check_updates(&mut self, cx: &mut Context<Self>) {
        if self.updates.checking || self.updates.installing {
            return;
        }
        self.updates.checking = true;
        cx.notify();
        off_thread(
            cx,
            || {
                let self_update = kuality_update::can_self_update();
                (self_update, kuality_update::Checker::new().check())
            },
            |this, (self_update, result), _| {
                match &result {
                    Ok(Some(r)) => log::info!("update available: Kuality {}", r.version),
                    Ok(None) => log::info!("Kuality is up to date"),
                    Err(e) => log::warn!("update check failed: {e}"),
                }
                this.updates.checking = false;
                this.updates.self_update = self_update;
                this.updates.result = Some(result);
            },
        );
    }

    /// Installs `release` (installed copy) or opens its page (portable / dev copy).
    fn install_update(&mut self, release: Release, cx: &mut Context<Self>) {
        if !self.updates.self_update {
            cx.open_url(&release.page_url);
            return;
        }
        if self.updates.installing {
            return;
        }
        self.updates.installing = true;
        self.updates.install_error = None;
        cx.notify();
        off_thread(
            cx,
            move || kuality_update::download_and_install(&release),
            |this, result, _| {
                // On success stay "installing": the installer closes this window.
                if let Err(e) = result {
                    log::error!("update failed: {e}");
                    this.updates.installing = false;
                    this.updates.install_error = Some(e);
                }
            },
        );
    }

    fn render_updates(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = &self.updates;
        let ro = self.read_only();
        let auto = Switch::new("updates-check")
            .accessibility_label("Check for updates automatically")
            .checked(self.doc.config.updates.check)
            .disabled(ro)
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.doc.config.updates.check = *on;
                this.changed(cx);
            }));

        let available = match &view.result {
            Some(Ok(Some(r))) => Some(r.clone()),
            _ => None,
        };
        let status: SharedString = if view.installing {
            "Downloading the update… Kuality restarts when it is installed.".into()
        } else if view.checking {
            "Checking for updates…".into()
        } else {
            match &view.result {
                Some(Ok(Some(r))) => format!("Kuality {} is available.", r.version).into(),
                Some(Ok(None)) => "Kuality is up to date.".into(),
                Some(Err(_)) | None => "".into(),
            }
        };
        let busy = view.checking || view.installing;
        let buttons = match available {
            Some(release) => {
                let page = release.page_url.clone();
                let label = if view.self_update {
                    format!("Install {}", release.version)
                } else {
                    format!("Download {}", release.version)
                };
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("update-notes")
                            .small()
                            .ghost()
                            .label("What's new")
                            .on_click(move |_, _, cx| cx.open_url(&page)),
                    )
                    .child(
                        Button::new("update-install")
                            .small()
                            .primary()
                            .label(label)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.install_update(release.clone(), cx);
                            })),
                    )
            }
            None => h_flex().child(
                Button::new("update-check")
                    .small()
                    .secondary()
                    .label("Check now")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.check_updates(cx))),
            ),
        };

        let mut notes: Vec<Div> = Vec::new();
        if let Some(Err(e)) = &view.result
            && !view.checking
        {
            notes.push(error_text(format!("Could not check for updates: {e}"), cx));
        }
        if let Some(e) = view.install_error.clone() {
            notes.push(error_text(format!("Update failed: {e}"), cx));
        }
        let notes = (!notes.is_empty()).then(|| v_flex().gap_1().px_5().pb_4().children(notes));

        section(cx)
            .child(setting_row(
                "Check for updates automatically",
                Some(
                    "When Kuality starts and every 6 hours. Nothing is installed without asking."
                        .into(),
                ),
                auto,
                cx,
            ))
            .child(divider(cx))
            .child(setting_row(
                format!("Version {}", kuality_update::current_version()),
                (!status.is_empty()).then_some(status),
                buttons,
                cx,
            ))
            .children(notes)
    }
}

const LEVELS: [(LogLevel, &str); 5] = [
    (LogLevel::Error, "Error"),
    (LogLevel::Warn, "Warn"),
    (LogLevel::Info, "Info"),
    (LogLevel::Debug, "Debug"),
    (LogLevel::Trace, "Trace"),
];

impl SettingsWindow {
    pub(super) fn render_general(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ro = self.read_only();
        let level = controls::segmented(
            "log-level",
            &LEVELS,
            Some(self.doc.config.general.log_level),
            ro,
            cx,
            |this, level, _, cx| {
                this.doc.config.general.log_level = level;
                this.changed(cx);
            },
        );
        let path = self.doc.path().to_path_buf();
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        v_flex()
            .gap_4()
            .child(self.render_startup(cx))
            .child(self.render_updates(cx))
            .child(section(cx).child(setting_row(
                "Log level",
                Some(r"How much Kuality writes to logs\kuality.log.".into()),
                level,
                cx,
            )))
            .child(
                section(cx).child(
                    v_flex()
                        .gap_2()
                        .px_5()
                        .py_4()
                        .child(div().text_sm().font_medium().child("Config file"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(path.display().to_string()),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .pt_1()
                                .child(controls::open_config_button("open-config", self, cx))
                                .child(
                                    Button::new("open-folder")
                                        .small()
                                        .secondary()
                                        .label("Open data folder")
                                        .on_click(move |_, _, cx| {
                                            let _ = std::fs::create_dir_all(&dir);
                                            cx.open_with_system(&dir);
                                        }),
                                ),
                        )
                        .child(controls::hint_text(
                            "Every change in this window is written to this file and applied by \
                             Kuality right away. You can also edit it by hand.",
                            cx,
                        )),
                ),
            )
    }
}
