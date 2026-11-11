//! General tab: start at logon, updates, log level and where the config lives.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingPage};
use gpui_kit::component::{Disableable as _, IconName, Sizable as _, h_flex};
use gpui_kit::*;
use kuality_protocol::LogLevel;
use kuality_update::Release;

use super::SettingsWindow;
use super::controls::{
    self, Note, Weak, control_item, dropdown_item, notes_item, raw_switch_item, switch_item,
};
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

    /// The entry read from the system (`None` until the first read finishes).
    fn startup_entry(&self) -> Option<Entry> {
        self.startup.state.as_ref().map(startup::State::entry)
    }

    fn startup_group(&self, weak: &Weak) -> SettingGroup {
        let view = &self.startup;
        let entry = self.startup_entry();
        let ready = entry.is_some() && !view.busy;
        let on = entry.is_some_and(|e| e != Entry::Off);

        let exe = std::env::current_exe().ok();
        let other = view
            .state
            .as_ref()
            .zip(exe.as_deref())
            .and_then(|(s, exe)| s.other_copy(exe));
        let mut notes = Vec::new();
        if view.busy {
            notes.push(Note::hint("Applying… Windows may ask for permission."));
        }
        if let Some(path) = other {
            notes.push(Note::warning(format!(
                "Starts another copy of Kuality: {}. Turn this off and on to start this one.",
                path.display()
            )));
        }
        if let Some(TaskState::Unknown(why)) = view.state.as_ref().map(|s| &s.task) {
            notes.push(Note::warning(format!(
                "Could not check the administrator task: {why}"
            )));
        }
        if let Some(e) = view.error.clone() {
            notes.push(Note::error(e));
        }

        // Not part of config.json: these work while the file is read-only too.
        SettingGroup::new()
            .title("Startup")
            .item(
                raw_switch_item(
                    "Start with Windows",
                    weak,
                    |w| w.startup_entry().is_some_and(|e| e != Entry::Off),
                    |w, on, cx| w.set_startup(if on { Entry::Normal } else { Entry::Off }, cx),
                )
                .description("Start Kuality when you sign in.")
                .disabled(!ready),
            )
            .item(
                raw_switch_item(
                    "Run as administrator",
                    weak,
                    |w| w.startup_entry() == Some(Entry::Elevated),
                    |w, on, cx| w.set_startup(if on { Entry::Elevated } else { Entry::Normal }, cx),
                )
                .description(
                    "Start it with administrator rights, so Hyperkey and shortcuts also work in \
                     apps running as administrator. Windows asks for permission when you change \
                     this.",
                )
                .disabled(!ready || !on),
            )
            .items(notes_item(notes))
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

    fn updates_group(&self, weak: &Weak) -> SettingGroup {
        let view = &self.updates;
        let ro = self.read_only();
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
        let mut notes = Vec::new();
        if let Some(Err(e)) = &view.result
            && !view.checking
        {
            notes.push(Note::error(format!("Could not check for updates: {e}")));
        }
        if let Some(e) = view.install_error.clone() {
            notes.push(Note::error(format!("Update failed: {e}")));
        }

        let version = control_item(
            format!("Version {}", kuality_update::current_version()),
            weak,
            |this, _, cx| this.render_update_buttons(cx),
        );
        let version = if status.is_empty() {
            version
        } else {
            version.description(status)
        };
        SettingGroup::new()
            .title("Updates")
            .item(
                switch_item(
                    "Check for updates automatically",
                    weak,
                    |w| w.doc.config.updates.check,
                    |w, on, cx| {
                        w.doc.config.updates.check = on;
                        w.changed(cx);
                    },
                )
                .description(
                    "When Kuality starts and every 6 hours. Nothing is installed without asking.",
                )
                .disabled(ro),
            )
            .item(version)
            .items(notes_item(notes))
    }

    /// "Check now", or "What's new" + "Install / Download" when a release is available.
    fn render_update_buttons(&self, cx: &mut Context<Self>) -> Div {
        let view = &self.updates;
        let busy = view.checking || view.installing;
        let Some(Ok(Some(release))) = view.result.clone() else {
            return h_flex().child(
                Button::new("update-check")
                    .small()
                    .label("Check now")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.check_updates(cx))),
            );
        };
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

    pub(super) fn general_page(&self, weak: &Weak, _cx: &mut Context<Self>) -> SettingPage {
        let ro = self.read_only();
        let path = self.doc.path().to_path_buf();
        let logging = SettingGroup::new().title("Logging").item(
            dropdown_item(
                "Log level",
                weak,
                &LEVELS,
                |w| Some(w.doc.config.general.log_level),
                |w, level, cx| {
                    w.doc.config.general.log_level = level;
                    w.changed(cx);
                },
            )
            .description(r"How much Kuality writes to logs\kuality.log.")
            .disabled(ro),
        );
        let config_file = SettingGroup::new()
            .title("Config file")
            .item(
                control_item("config.json", weak, |this, _, _| {
                    let dir = this
                        .doc
                        .path()
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_default();
                    h_flex()
                        .gap_2()
                        .child(controls::open_config_button("open-config", this))
                        .child(
                            Button::new("open-folder")
                                .small()
                                .label("Open data folder")
                                .on_click(move |_, _, cx| {
                                    let _ = std::fs::create_dir_all(&dir);
                                    cx.open_with_system(&dir);
                                }),
                        )
                })
                .description(path.display().to_string()),
            )
            .items(notes_item(vec![Note::hint(
                "Every change in this window is written to this file and applied by Kuality \
                 right away. You can also edit it by hand.",
            )]));
        SettingPage::new("General")
            .icon(IconName::Settings)
            .group(self.startup_group(weak))
            .group(self.updates_group(weak))
            .group(logging)
            .group(config_file)
    }
}

const LEVELS: [(LogLevel, &str); 5] = [
    (LogLevel::Error, "Error"),
    (LogLevel::Warn, "Warn"),
    (LogLevel::Info, "Info"),
    (LogLevel::Debug, "Debug"),
    (LogLevel::Trace, "Trace"),
];
