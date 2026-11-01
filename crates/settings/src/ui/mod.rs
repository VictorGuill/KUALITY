//! The settings window (GPUI + gpui-component). One view owns the document, the open editor and
//! the key recorder; each tab lives in its own module.

mod controls;
mod general;
mod hyperkey;
mod picker;
mod shortcuts;
mod widgets;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme as _, IconName, Theme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::apps::{self, Catalog};
use crate::model::{DiskState, Document, PollOutcome, SaveOutcome};
use crate::platform::{self, KeyRecorder, WINDOW_TITLE};
use crate::recorder::{Mode, Outcome};

/// Auto-save delay after the last edit.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);
/// How often the file is checked for outside changes (also on window activation).
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// A recording ends by itself after this long.
const RECORD_TIMEOUT: Duration = Duration::from_secs(10);

/// How this window is tied to the core process.
#[derive(Debug)]
pub enum CoreLink {
    /// The window closes (after flushing a pending save) when this process exits.
    Attached(platform::CoreProcess),
    /// No core could be found or started: the window shows why.
    Missing(String),
}

// Lucide icons beyond gpui-component's default set, embedded one by one.
gpui_kit::assets::icon_assets!(ExtraIcons, [AppWindow, Keyboard]);

/// gpui-component's default icons plus [`ExtraIcons`].
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = ExtraIcons.list(path)?;
        all.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(all)
    }
}

/// Opens the window and runs until it is closed or the core exits.
pub fn run(config_path: PathBuf, core: CoreLink) {
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(920.), px(660.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some(WINDOW_TITLE.into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(720.), px(480.))),
                app_id: Some("Kuality.Settings".into()),
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
                cx.new(|cx| SettingsWindow::new(config_path, core, window, cx))
            });
            if let Err(e) = opened {
                log::error!("cannot open the settings window: {e}");
                cx.quit();
                return;
            }
            cx.on_window_closed(|cx, _| cx.quit()).detach();
        });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    General,
    Widgets,
    Hyperkey,
    Shortcuts,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::General, Tab::Widgets, Tab::Hyperkey, Tab::Shortcuts];

    fn label(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Widgets => "Widgets",
            Tab::Hyperkey => "Hyperkey",
            Tab::Shortcuts => "Shortcuts",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Tab::General => IconName::Settings,
            Tab::Widgets => IconName::PanelBottom,
            Tab::Hyperkey => IconName::ArrowUp,
            Tab::Shortcuts => IconName::SquareTerminal,
        }
    }
}

/// Last save / reload result, shown in the status bar.
#[derive(Debug, Clone, PartialEq)]
enum Status {
    Idle,
    Pending,
    Saved,
    Reloaded,
    Removed,
    Error(String),
}

/// What a running recording fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordTarget {
    /// The hotkey of a shortcut row.
    Trigger(shortcuts::Row),
    /// The chord of the picker.s "Send keys" page.
    Chord,
}

struct ActiveRecording {
    target: RecordTarget,
    _recorder: KeyRecorder,
    _tasks: Vec<Task<()>>,
}

pub struct SettingsWindow {
    pub(crate) doc: Document,
    tab: Tab,
    status: Status,
    core_running: bool,
    /// Why no core is attached (shown as a banner); `None` when attached.
    core_missing: Option<String>,
    save_task: Option<Task<()>>,
    pub(crate) widget_editor: Option<widgets::WidgetEditor>,
    pub(crate) draft: Option<shortcuts::Draft>,
    pub(crate) picker: Option<shortcuts::OpenPicker>,
    /// Name inputs of the shortcut rows, by binding index (created on first render).
    pub(crate) name_fields: HashMap<usize, shortcuts::NameField>,
    /// Why the last recorded hotkey was refused (shown under that row while it records again).
    pub(crate) record_message: Option<(RecordTarget, &'static str)>,
    /// Installed apps for the action picker (loaded in the background).
    pub(crate) catalog: Entity<Catalog>,
    recording: Option<ActiveRecording>,
    /// Start-at-logon switches (General tab).
    pub(crate) startup: general::StartupView,
    _poll: Task<()>,
    _subs: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(
        config_path: PathBuf,
        core: CoreLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let doc = Document::load(config_path);
        let core_missing = match core {
            CoreLink::Attached(core) => {
                match core.watch() {
                    Ok(exited) => cx
                        .spawn(async move |this, cx| {
                            if exited.await.is_ok() {
                                let _ = this.update(cx, |this, cx| this.core_exited(cx));
                            }
                        })
                        .detach(),
                    Err(e) => log::error!("cannot watch the Kuality process: {e}"),
                }
                None
            }
            CoreLink::Missing(why) => Some(why),
        };
        log::info!("editing {} ({:?})", doc.path().display(), doc.state());
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this
                    .update_in(cx, |this, window, cx| this.check_disk(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let catalog = cx.new(|_| Catalog::default());
        match apps::start_loading() {
            Ok(loading) => {
                let catalog = catalog.clone();
                cx.spawn(async move |_, cx| {
                    let Ok(list) = loading.list.await else {
                        return;
                    };
                    catalog.update(cx, |c, cx| {
                        c.take_list(list);
                        cx.notify();
                    });
                    if let Ok(icons) = loading.icons.await {
                        catalog.update(cx, |c, cx| {
                            c.take_icons(icons);
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
            Err(e) => {
                log::error!("cannot start listing the installed apps: {e}");
                catalog.update(cx, |c, _| c.take_list(Err(e.to_string())));
            }
        }
        let subs = vec![
            cx.observe(&catalog, |_, _, cx| cx.notify()),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.check_disk(window, cx);
                } else {
                    this.cancel_recording(cx);
                }
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }),
        ];
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            if let Some(this) = weak.upgrade() {
                this.update(cx, |this, cx| this.flush(cx));
            }
            true
        });
        cx.defer_in(window, |this, _, cx| this.load_startup(cx));
        Self {
            doc,
            tab: Tab::General,
            status: Status::Idle,
            core_running: platform::core_running(),
            core_missing,
            save_task: None,
            widget_editor: None,
            draft: None,
            picker: None,
            name_fields: HashMap::new(),
            record_message: None,
            catalog,
            recording: None,
            startup: general::StartupView::default(),
            _poll: poll,
            _subs: subs,
        }
    }

    pub(crate) fn read_only(&self) -> bool {
        self.doc.read_only()
    }

    /// Call after every valid change of `doc.config`: saves after the debounce.
    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
        if self.read_only() {
            return;
        }
        self.status = Status::Pending;
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.save_now(cx));
        }));
        cx.notify();
    }

    fn save_now(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        match self.doc.save() {
            Ok(SaveOutcome::Written) => {
                log::debug!("saved {}", self.doc.path().display());
                self.status = Status::Saved;
            }
            Ok(SaveOutcome::Unchanged) => self.status = Status::Saved,
            Ok(SaveOutcome::Blocked) => {}
            Err(e) => {
                log::error!("cannot write {}: {e}", self.doc.path().display());
                self.status = Status::Error(e.to_string());
            }
        }
        cx.notify();
    }

    /// The core exited (tray Quit, `taskkill`, crash): save what is pending and close.
    fn core_exited(&mut self, cx: &mut Context<Self>) {
        log::info!("Kuality exited; closing the settings window");
        self.cancel_recording(cx);
        self.flush(cx);
        cx.quit();
    }

    /// Writes a pending change at once (window closing).
    fn flush(&mut self, cx: &mut Context<Self>) {
        if self.save_task.is_some() || matches!(self.status, Status::Error(_)) {
            self.save_now(cx);
        }
    }

    fn check_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = platform::core_running();
        if running != self.core_running {
            self.core_running = running;
            cx.notify();
        }
        match self.doc.poll() {
            PollOutcome::Unchanged => {}
            PollOutcome::Reloaded => {
                log::info!("config.json changed on disk, reloaded");
                // The outside edit wins over a change not saved yet.
                self.save_task = None;
                self.status = Status::Reloaded;
                self.rebuild_editors(window, cx);
                cx.notify();
            }
            PollOutcome::BecameInvalid => {
                log::warn!("config.json on disk is invalid; editing paused");
                self.save_task = None;
                self.cancel_recording(cx);
                self.widget_editor = None;
                self.picker = None;
                cx.notify();
            }
            PollOutcome::Removed => {
                self.status = Status::Removed;
                cx.notify();
            }
        }
    }

    /// Re-creates the open editor from the reloaded config (drafts are kept).
    fn rebuild_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_recording(cx);
        if let Some(index) = self.widget_editor.as_ref().map(|e| e.index) {
            self.widget_editor = None;
            self.open_widget(index, window, cx);
        }
        // Row indices may have changed.
        self.close_picker(cx);
        self.name_fields.clear();
    }

    fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.cancel_recording(cx);
            self.tab = tab;
            cx.notify();
        }
    }

    // --- Recording -----------------------------------------------------------------------

    pub(crate) fn recording_target(&self) -> Option<RecordTarget> {
        self.recording.as_ref().map(|r| r.target)
    }

    pub(crate) fn start_recording(
        &mut self,
        target: RecordTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_recording(cx);
        let mode = match target {
            RecordTarget::Trigger(_) => Mode::Trigger,
            RecordTarget::Chord => Mode::Chord,
        };
        let (recorder, rx) = match KeyRecorder::start(mode) {
            Ok(r) => r,
            Err(e) => {
                log::error!("cannot record keys: {e}");
                self.status = Status::Error(format!("cannot record keys: {e}"));
                cx.notify();
                return;
            }
        };
        let result = cx.spawn_in(window, async move |this, cx| {
            if let Ok(outcome) = rx.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_recording(outcome, window, cx)
                });
            }
        });
        let timeout = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RECORD_TIMEOUT).await;
            let _ = this.update(cx, |this, cx| this.cancel_recording(cx));
        });
        self.recording = Some(ActiveRecording {
            target,
            _recorder: recorder,
            _tasks: vec![result, timeout],
        });
        if target == RecordTarget::Chord {
            self.picker_recording(true, cx);
        }
        cx.notify();
    }

    pub(crate) fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        self.record_message = None;
        if let Some(rec) = self.recording.take() {
            if rec.target == RecordTarget::Chord {
                self.picker_recording(false, cx);
            }
            cx.notify();
        }
    }

    /// Tells the open picker whether its "Send keys" recorder runs.
    fn picker_recording(&self, on: bool, cx: &mut Context<Self>) {
        if let Some(p) = self.picker.as_ref() {
            p.view.update(cx, |p, cx| p.set_recording(on, cx));
        }
    }

    fn finish_recording(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rec) = self.recording.take() else {
            return;
        };
        self.record_message = None;
        match outcome {
            Outcome::Recorded(text) => self.apply_recorded(rec.target, text, window, cx),
            Outcome::Rejected(why) => {
                // Say why and listen again, so the user just presses another hotkey.
                drop(rec._recorder);
                self.start_recording(rec.target, window, cx);
                self.record_message = Some((rec.target, why));
            }
            Outcome::Cancelled if rec.target == RecordTarget::Chord => {
                self.picker_recording(false, cx)
            }
            Outcome::Cancelled => {}
        }
        cx.notify();
    }

    // --- Rendering -----------------------------------------------------------------------

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = Tab::ALL.map(|tab| {
            SidebarMenuItem::new(tab.label())
                .icon(tab.icon())
                .active(self.tab == tab)
                .on_click(cx.listener(move |this, _, _, cx| this.select_tab(tab, cx)))
        });
        Sidebar::new("sidebar")
            .collapsible(false)
            .w(px(200.))
            .header(
                v_flex()
                    .px_2()
                    .py_1()
                    .child(div().text_lg().font_semibold().child("Kuality"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Settings"),
                    ),
            )
            .child(SidebarMenu::new().children(items))
    }

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use gpui_kit::component::alert::Alert;
        match self.doc.state() {
            DiskState::Invalid(msg) => Some(
                v_flex()
                    .gap_2()
                    .child(
                        Alert::error(
                            "invalid-file",
                            format!(
                                "config.json has an error: {msg}. Kuality keeps its last valid \
                                 settings. Fix the file — this window updates when it is valid."
                            ),
                        )
                        .title("Editing paused"),
                    )
                    .child(h_flex().child(controls::open_config_button(
                        "banner-open-config",
                        self,
                        cx,
                    )))
                    .into_any_element(),
            ),
            _ if self.core_missing.is_some() => Some(
                Alert::warning(
                    "core-missing",
                    format!(
                        "{} Changes are still saved to config.json and apply when Kuality starts.",
                        self.core_missing.as_deref().unwrap_or_default()
                    ),
                )
                .title("Kuality is not running")
                .into_any_element(),
            ),
            _ if self.doc.newer_version() => Some(
                Alert::warning(
                    "newer-version",
                    "config.json was written by a newer Kuality. Unknown settings are kept, but \
                     some values may mean something else in that version.",
                )
                .into_any_element(),
            ),
            _ => None,
        }
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let errors = self.widget_editor.as_ref().map_or(0, |e| e.error_count());
        let theme = cx.theme();
        let (text, color) = if self.read_only() {
            (
                "Editing paused: config.json has an error".to_string(),
                theme.danger,
            )
        } else {
            match &self.status {
                Status::Pending => ("Saving…".into(), theme.muted_foreground),
                Status::Error(e) => (format!("Could not write config.json: {e}"), theme.danger),
                _ if errors > 0 => (
                    format!(
                        "{errors} field{} not saved",
                        if errors == 1 { "" } else { "s" }
                    ),
                    theme.warning,
                ),
                Status::Saved => ("Saved".into(), theme.muted_foreground),
                Status::Reloaded => (
                    "Reloaded: config.json changed on disk".into(),
                    theme.muted_foreground,
                ),
                Status::Removed => (
                    "config.json was deleted; the next change creates it again".into(),
                    theme.warning,
                ),
                Status::Idle => (
                    "Changes are saved automatically".into(),
                    theme.muted_foreground,
                ),
            }
        };
        h_flex()
            .id("status")
            .justify_between()
            .gap_4()
            .px_4()
            .py_1p5()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .child(div().text_color(color).child(text))
            .when(!self.core_running, |this| {
                this.child(
                    div()
                        .text_color(theme.warning)
                        .child("Kuality is not running — changes apply when it starts"),
                )
            })
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.tab {
            Tab::General => self.render_general(window, cx).into_any_element(),
            Tab::Widgets => self.render_widgets(window, cx).into_any_element(),
            Tab::Hyperkey => self.render_hyperkey(window, cx).into_any_element(),
            Tab::Shortcuts => self.render_shortcuts(window, cx).into_any_element(),
        };
        let banner = self.render_banner(cx);
        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.cancel_recording(cx)),
            )
            .child(self.render_sidebar(cx))
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .child(
                        div()
                            .id("page")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(
                                v_flex()
                                    .gap_5()
                                    .px_8()
                                    .py_6()
                                    .max_w(px(760.))
                                    .child(div().text_xl().font_semibold().child(self.tab.label()))
                                    .children(banner)
                                    .child(page),
                            ),
                    )
                    .child(self.render_status(cx)),
            )
    }
}
