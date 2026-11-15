//! App lock tab (`docs/specs/app-lock.md`): the switch, the password (set / change, through the
//! core's pipe), and the locked programs (saved in `config.json`). While App lock is on and a
//! password is set, the tab shows only a password field until the right password is typed.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::setting::{SettingGroup, SettingPage};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use kuality_protocol::LockedApp;
use kuality_protocol::app_lock::{
    Request, Response, normalize_path, password_problem, refused_app,
};
use kuality_protocol::paths::APP_LOCK_FILE;

use super::SettingsWindow;
use super::clipboard::PipeNote;
use super::controls::{Note, Weak, block_item, notes_item, switch_item};
use crate::platform;

/// The password inputs, created on first render.
pub(crate) struct Fields {
    gate: Entity<InputState>,
    new: Entity<InputState>,
    confirm: Entity<InputState>,
    _subs: Vec<Subscription>,
}

/// A program with a window now, offered in the "Add" list.
#[derive(Debug, Clone)]
pub(crate) struct Running {
    path: String,
    name: String,
}

/// State of the App lock tab.
#[derive(Default)]
pub(crate) struct AppLockView {
    /// `app-lock.bin` exists (checked with the 1 s disk poll and after a change).
    pub(crate) has_password: bool,
    /// The right password, typed in this window: it opens the tab and authorizes a change.
    verified: Option<String>,
    fields: Option<Fields>,
    /// Result of the last password action.
    note: Option<PipeNote>,
    /// Why the last program could not be added.
    add_error: Option<String>,
    /// A request is on its way to the core.
    busy: bool,
    /// Programs with a window, once listed.
    running: Option<Vec<Running>>,
    icons: HashMap<String, Option<Arc<RenderImage>>>,
    icons_loading: HashSet<String>,
    /// Exe descriptions of listed programs without a `name`, by normalized path.
    descriptions: HashMap<String, Option<String>>,
}

/// `app-lock.bin` next to `config.json`.
pub(crate) fn secret_file(config: &std::path::Path) -> PathBuf {
    config
        .parent()
        .map_or_else(|| PathBuf::from(APP_LOCK_FILE), |d| d.join(APP_LOCK_FILE))
}

fn own_exe() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

impl AppLockView {
    /// The tab's state for the window editing `config` (reads whether a password exists).
    pub(crate) fn new(config: &std::path::Path) -> Self {
        Self {
            has_password: secret_file(config).exists(),
            ..Self::default()
        }
    }
}

impl SettingsWindow {
    /// Re-checks whether a password exists (disk poll).
    pub(super) fn check_app_lock_secret(&mut self, cx: &mut Context<Self>) {
        let has = secret_file(self.doc.path()).exists();
        if has != self.app_lock.has_password {
            self.app_lock.has_password = has;
            if !has {
                self.app_lock.verified = None;
            }
            cx.notify();
        }
    }

    fn app_lock_locked(&self) -> bool {
        self.doc.config.app_lock.enabled
            && self.app_lock.has_password
            && self.app_lock.verified.is_none()
    }

    fn app_lock_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &Fields {
        if self.app_lock.fields.is_none() {
            let masked =
                |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
                    cx.new(|cx| {
                        InputState::new(window, cx)
                            .masked(true)
                            .placeholder(placeholder)
                    })
                };
            let gate = masked("Kuality password", window, cx);
            let new = masked("New password", window, cx);
            let confirm = masked("Type it again", window, cx);
            let subs = vec![
                cx.subscribe_in(&gate, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.verify_app_lock(window, cx);
                    }
                }),
                cx.subscribe_in(&confirm, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.set_app_lock_password(window, cx);
                    }
                }),
            ];
            self.app_lock.fields = Some(Fields {
                gate,
                new,
                confirm,
                _subs: subs,
            });
        }
        self.app_lock.fields.as_ref().expect("created above")
    }

    /// Sends a request to the core on a background thread; `done` runs with the reply.
    fn app_lock_request(
        &mut self,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<Response, String>, &mut Window, &mut Context<Self>)
        + 'static,
    ) {
        let Some(pid) = self.core_pid.filter(|_| self.core_running) else {
            self.app_lock.note = Some(PipeNote::Failed("Kuality is not running.".into()));
            cx.notify();
            return;
        };
        self.app_lock.busy = true;
        self.app_lock.note = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let reply = cx
                .background_executor()
                .spawn(async move { platform::pipe_request::<Response>(pid, request) })
                .await
                .map(|(r, _)| r)
                .map_err(|e| e.to_string());
            let _ = this.update_in(cx, |this, window, cx| {
                this.app_lock.busy = false;
                done(this, reply, window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The gate's Unlock button.
    fn verify_app_lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = &self.app_lock.fields else {
            return;
        };
        let password = f.gate.read(cx).value().to_string();
        if password.is_empty() || self.app_lock.busy {
            return;
        }
        let request = Request::AppLockVerify {
            password: password.clone(),
        };
        self.app_lock_request(request, window, cx, move |this, reply, window, cx| {
            match reply {
                Ok(r) if r.ok => this.app_lock.verified = Some(password),
                Ok(r) => this.app_lock.note = Some(PipeNote::Failed(r.error.unwrap_or_default())),
                Err(e) => this.app_lock.note = Some(PipeNote::Failed(e)),
            }
            if let Some(f) = &this.app_lock.fields {
                f.gate.update(cx, |s, cx| s.set_value("", window, cx));
            }
        });
    }

    /// The Set / Change password button.
    fn set_app_lock_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = &self.app_lock.fields else {
            return;
        };
        if self.app_lock.busy {
            return;
        }
        let new = f.new.read(cx).value().to_string();
        let confirm = f.confirm.read(cx).value().to_string();
        if let Some(why) = password_problem(&new, &confirm) {
            self.app_lock.note = Some(PipeNote::Failed(why.into()));
            cx.notify();
            return;
        }
        let first = !self.app_lock.has_password;
        let request = Request::AppLockSetPassword {
            current: self.app_lock.verified.clone().unwrap_or_default(),
            new: new.clone(),
        };
        self.app_lock_request(
            request,
            window,
            cx,
            move |this, reply, window, cx| match reply {
                Ok(r) if r.ok => {
                    this.app_lock.has_password = true;
                    this.app_lock.verified = Some(new);
                    this.app_lock.note = Some(PipeNote::Done(if first {
                        "Password set. Locked apps now ask for it when they start."
                    } else {
                        "Password changed."
                    }));
                    if let Some(f) = &this.app_lock.fields {
                        f.new.update(cx, |s, cx| s.set_value("", window, cx));
                        f.confirm.update(cx, |s, cx| s.set_value("", window, cx));
                    }
                }
                Ok(r) => this.app_lock.note = Some(PipeNote::Failed(r.error.unwrap_or_default())),
                Err(e) => this.app_lock.note = Some(PipeNote::Failed(e)),
            },
        );
    }

    /// Adds a program to the list (`config.json`).
    fn add_locked_app(&mut self, path: String, name: Option<String>, cx: &mut Context<Self>) {
        if self.read_only() {
            return;
        }
        if let Some(why) = refused_app(&path, &own_exe()) {
            self.app_lock.add_error = Some(why.into());
            cx.notify();
            return;
        }
        let key = normalize_path(&path);
        let apps = &mut self.doc.config.app_lock.apps;
        if apps.iter().any(|a| normalize_path(&a.path) == key) {
            self.app_lock.add_error = Some("This program is already in the list.".into());
            cx.notify();
            return;
        }
        let name = name
            .or_else(|| platform::exe_description(&path))
            .unwrap_or_default();
        apps.push(LockedApp {
            path,
            name,
            ..LockedApp::default()
        });
        self.app_lock.add_error = None;
        self.changed(cx);
    }

    fn remove_locked_app(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.read_only() || index >= self.doc.config.app_lock.apps.len() {
            return;
        }
        self.doc.config.app_lock.apps.remove(index);
        self.changed(cx);
    }

    fn browse_locked_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Lock this program".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.add_locked_app(path.display().to_string(), None, cx)
            });
        })
        .detach();
    }

    /// Lists the programs that have a window now.
    fn list_running_apps(&mut self, cx: &mut Context<Self>) {
        let own = own_exe();
        let list = platform::running_apps()
            .into_iter()
            .filter(|a| refused_app(&a.path, &own).is_none())
            .map(|a| Running {
                name: platform::exe_description(&a.path).unwrap_or(a.title),
                path: a.path,
            })
            .collect();
        self.app_lock.running = Some(list);
        cx.notify();
    }

    /// The name shown for a listed program: its `name`, else the exe's description (read once),
    /// else its file name.
    fn listed_name(&mut self, app: &LockedApp) -> String {
        if !app.name.trim().is_empty() {
            return app.display_name();
        }
        self.app_lock
            .descriptions
            .entry(normalize_path(&app.path))
            .or_insert_with(|| platform::exe_description(&app.path))
            .clone()
            .unwrap_or_else(|| app.display_name())
    }

    /// The icon of `path`, loaded in the background the first time it is asked for.
    fn app_icon(&mut self, path: &str, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let key = normalize_path(path);
        if let Some(icon) = self.app_lock.icons.get(&key) {
            return icon.clone();
        }
        if self.app_lock.icons_loading.insert(key.clone()) {
            let path = path.to_string();
            cx.spawn(async move |this, cx| {
                let icon = cx
                    .background_executor()
                    .spawn(
                        async move { platform::file_icon(&path).and_then(crate::apps::to_image) },
                    )
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.app_lock.icons_loading.remove(&key);
                    this.app_lock.icons.insert(key, icon);
                    cx.notify();
                });
            })
            .detach();
        }
        None
    }

    fn app_row(
        &mut self,
        name: String,
        path: String,
        button: Button,
        cx: &mut Context<Self>,
    ) -> Div {
        let icon = self.app_icon(&path, cx);
        let theme = cx.theme();
        let leading = match icon {
            Some(i) => img(i)
                .size_6()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => Icon::new(gpui_kit::assets::IconName::AppWindow)
                .small()
                .text_color(theme.muted_foreground)
                .into_any_element(),
        };
        h_flex()
            .gap_3()
            .items_center()
            .py_1()
            .child(
                h_flex()
                    .size_6()
                    .flex_none()
                    .justify_center()
                    .child(leading),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(Label::new(name).text_sm().truncate())
                    .child(
                        Label::new(path)
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate(),
                    ),
            )
            .child(button)
    }

    pub(super) fn app_lock_page(&self, weak: &Weak, _cx: &mut Context<Self>) -> SettingPage {
        let page = SettingPage::new("App lock").icon(gpui_kit::assets::IconName::Lock);
        if self.app_lock_locked() {
            return page.group(
                SettingGroup::new()
                    .title("Protected")
                    .item(
                        block_item(weak, |this, window, cx| {
                            let busy = this.app_lock.busy;
                            let note = this.app_lock.note.clone();
                            let gate = this.app_lock_fields(window, cx).gate.clone();
                            let theme = cx.theme();
                            v_flex()
                                .gap_2()
                                .child(
                                    Label::new(
                                        "App lock is on. Enter your Kuality password to change it.",
                                    )
                                    .text_sm(),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .w(px(260.))
                                                .child(Input::new(&gate).mask_toggle().small()),
                                        )
                                        .child(
                                            Button::new("app-lock-gate")
                                                .small()
                                                .primary()
                                                .label("Unlock")
                                                .disabled(busy)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.verify_app_lock(window, cx)
                                                })),
                                        ),
                                )
                                .when_some(note, |col, n| {
                                    col.child(match n {
                                        PipeNote::Done(t) => Label::new(t)
                                            .text_xs()
                                            .text_color(theme.muted_foreground),
                                        PipeNote::Failed(e) => {
                                            Label::new(e).text_xs().text_color(theme.danger)
                                        }
                                    })
                                })
                        })
                        .keywords(["password", "unlock", "lock"]),
                    )
                    .items(notes_item(vec![Note::hint(
                        "Forgot it? Quit Kuality, delete app-lock.bin next to config.json, start \
                         Kuality again and set a new password.",
                    )])),
            );
        }

        let ro = self.read_only();
        let config = &self.doc.config.app_lock;
        let enabled = config.enabled;
        let has_password = self.app_lock.has_password;

        let mut main_notes = Vec::new();
        if enabled && !has_password {
            main_notes.push(Note::warning(
                "No password yet: nothing is locked until you set one below.",
            ));
        }
        let main = SettingGroup::new()
            .item(
                switch_item(
                    "Enable App lock",
                    weak,
                    |w| w.doc.config.app_lock.enabled,
                    |w, on, cx| {
                        w.doc.config.app_lock.enabled = on;
                        w.app_lock.note = None;
                        w.changed(cx);
                    },
                )
                .description(
                    "The programs below ask for your Kuality password when they start. Once \
                     unlocked, a program stays unlocked until it is closed.",
                )
                .disabled(ro),
            )
            .items(notes_item(main_notes));

        let mut page = page.group(main);
        if enabled {
            page = page.group(
                SettingGroup::new()
                    .title("Password")
                    .item(
                        block_item(weak, |this, window, cx| {
                            let busy = this.app_lock.busy;
                            let first = !this.app_lock.has_password;
                            let note = this.app_lock.note.clone();
                            let f = this.app_lock_fields(window, cx);
                            let (new, confirm) = (f.new.clone(), f.confirm.clone());
                            let theme = cx.theme();
                            v_flex()
                                .gap_2()
                                .child(
                                    Label::new(if first {
                                        "Set a password (at least 4 characters)."
                                    } else {
                                        "Change the password."
                                    })
                                    .text_sm(),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .flex_wrap()
                                        .child(div().w(px(200.)).child(Input::new(&new).small()))
                                        .child(
                                            div().w(px(200.)).child(Input::new(&confirm).small()),
                                        )
                                        .child(
                                            Button::new("app-lock-set")
                                                .small()
                                                .label(if first {
                                                    "Set password"
                                                } else {
                                                    "Change password"
                                                })
                                                .disabled(busy)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.set_app_lock_password(window, cx)
                                                })),
                                        ),
                                )
                                .when_some(note, |col, n| {
                                    col.child(match n {
                                        PipeNote::Done(t) => Label::new(t)
                                            .text_xs()
                                            .text_color(theme.muted_foreground),
                                        PipeNote::Failed(e) => {
                                            Label::new(e).text_xs().text_color(theme.danger)
                                        }
                                    })
                                })
                        })
                        .keywords(["password", "change", "set"]),
                    )
                    .items(notes_item(vec![Note::hint(
                        "Stored only as a salted hash, encrypted for your Windows account \
                         (app-lock.bin). Forgot it? Quit Kuality, delete app-lock.bin, start \
                         Kuality again and set a new one.",
                    )])),
            );
        }

        let mut apps_notes = Vec::new();
        if let Some(e) = &self.app_lock.add_error {
            apps_notes.push(Note::warning(e.clone()));
        }
        page.group(
            SettingGroup::new()
                .title("Locked apps")
                .item(
                    block_item(weak, |this, _, cx| {
                        let ro = this.read_only();
                        let apps: Vec<(String, String)> = this
                            .doc
                            .config
                            .app_lock
                            .apps
                            .clone()
                            .iter()
                            .map(|a| (this.listed_name(a), a.path.clone()))
                            .collect();
                        let mut list = v_flex().gap_1();
                        if apps.is_empty() {
                            list = list.child(
                                Label::new("No apps yet: add one below.")
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground),
                            );
                        }
                        for (i, (name, path)) in apps.into_iter().enumerate() {
                            let remove =
                                Button::new(("app-lock-remove", i))
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                                    .tooltip("Remove")
                                    .disabled(ro)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_locked_app(i, cx)
                                    }));
                            list = list.child(this.app_row(name, path, remove, cx));
                        }
                        list
                    })
                    .keywords(["apps", "programs", "remove"]),
                )
                .item(
                    block_item(weak, |this, _, cx| {
                        let ro = this.read_only();
                        let listed: HashSet<String> = this
                            .doc
                            .config
                            .app_lock
                            .apps
                            .iter()
                            .map(|a| normalize_path(&a.path))
                            .collect();
                        let running = this.app_lock.running.clone();
                        let mut col =
                            v_flex().gap_2().child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("app-lock-running")
                                            .small()
                                            .label(if running.is_some() {
                                                "Refresh running apps"
                                            } else {
                                                "Add a running app…"
                                            })
                                            .disabled(ro)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.list_running_apps(cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("app-lock-browse")
                                            .small()
                                            .label("Browse…")
                                            .disabled(ro)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.browse_locked_app(window, cx)
                                            })),
                                    ),
                            );
                        if let Some(running) = running {
                            if running.is_empty() {
                                col = col.child(
                                    Label::new("No other program has a window open.")
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground),
                                );
                            }
                            for (i, r) in running.into_iter().enumerate() {
                                let added = listed.contains(&normalize_path(&r.path));
                                let (path, name) = (r.path.clone(), r.name.clone());
                                let add = Button::new(("app-lock-add", i))
                                    .small()
                                    .label(if added { "Added" } else { "Add" })
                                    .disabled(ro || added)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.add_locked_app(path.clone(), Some(name.clone()), cx)
                                    }));
                                col = col.child(this.app_row(r.name, r.path, add, cx));
                            }
                        }
                        col
                    })
                    .keywords(["add", "browse", "running"]),
                )
                .items(notes_item(apps_notes))
                .items(notes_item(vec![
                    Note::hint(
                        "This keeps out someone using your PC while it is unlocked; it is not \
                         real security: anyone on your Windows account can end Kuality in Task \
                         Manager or copy the program elsewhere.",
                    ),
                    Note::hint(
                        "Apps running as administrator can only be locked while Kuality runs as \
                         administrator too (General → Start with Windows). Store apps cannot be \
                         locked yet.",
                    ),
                ])),
        )
    }
}
