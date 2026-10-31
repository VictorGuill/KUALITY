//! Overlay thread (`kuality-overlay`): widget windows attached to every taskbar.
//!
//! Hard rule: widget windows are children of Explorer's taskbars, so this thread's input queue
//! is attached to Explorer's. It must never block: no `SendMessage` to Explorer windows, no
//! waiting on other threads, no file or registry writes (the theme read is the only registry
//! access). Logging is limited to rare events.
//!
//! Like the keyboard thread, commands arrive through a channel plus a posted `WM_APP_COMMAND`.
//! A hidden top-level window (`KualityOverlayHost`) receives broadcasts (`TaskbarCreated`,
//! display, setting, power and session changes) and owns the refresh timers. Window procedures
//! and WinEvent callbacks only borrow the state when it is free; otherwise they post
//! `WM_APP_REFRESH` so the work happens later.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use ::windows::Win32::Graphics::Gdi::HMONITOR;
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use ::windows::Win32::System::Threading::GetCurrentThreadId;
use ::windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_REORDER, GW_CHILD, GW_HWNDNEXT,
    GetMessageW, GetWindow, KillTimer, MSG, MSGFLT_ALLOW, OBJID_WINDOW, PBT_APMRESUMEAUTOMATIC,
    PostMessageW, PostQuitMessage, PostThreadMessageW, RegisterClassExW, RegisterWindowMessageW,
    SetTimer, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_CLOSE, WM_DISPLAYCHANGE,
    WM_POWERBROADCAST, WM_SETTINGCHANGE, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_POPUP, WTS_SESSION_UNLOCK,
};
use ::windows::core::{PCWSTR, w};

use super::render::D2d;
use super::taskbars;
use super::theme;
use super::widget_window::{self, WidgetWindow};
use crate::overlay::layout::{SizeF, SizePx, layout};
use crate::overlay::widgets::Theme;
use crate::overlay::{OverlayCommand, OverlayConfig};
use crate::platform::OverlayBackend;

const CLASS_NAME: PCWSTR = w!("KualityOverlayHost");
/// Commands are waiting in the channel.
const WM_APP_COMMAND: u32 = WM_APP + 1;
/// Something changed: schedule a (debounced) full refresh.
pub(crate) const WM_APP_REFRESH: u32 = WM_APP + 2;

const TIMER_REFRESH: usize = 1;
const TIMER_LATE_2S: usize = 2;
const TIMER_LATE_5S: usize = 3;
const TIMER_RETRY: usize = 4;
const REFRESH_DEBOUNCE_MS: u32 = 300;
const RETRY_MS: u32 = 1000;
/// Retries while `Shell_TrayWnd` is missing at startup (then rely on `TaskbarCreated`).
const MAX_RETRIES: u32 = 30;

/// The host window (read by widget window procedures from this thread only).
static HOST_HWND: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// The overlay host window, if the thread is running.
pub(crate) fn host_hwnd() -> Option<HWND> {
    let h = HOST_HWND.load(Ordering::Relaxed);
    (h != 0).then_some(HWND(h as *mut _))
}

/// Handle to the overlay thread, owned by the main thread.
#[derive(Debug)]
pub struct Overlay {
    tx: Sender<OverlayCommand>,
    hwnd: isize,
    thread_id: u32,
    thread: JoinHandle<()>,
    done: Receiver<()>,
}

impl OverlayBackend for Overlay {
    fn start() -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<io::Result<(isize, u32)>>(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("kuality-overlay".into())
            .spawn(move || {
                let _done = DoneGuard(done_tx);
                thread_main(rx, ready_tx);
            })?;
        let (hwnd, thread_id) = ready_rx
            .recv()
            .map_err(|_| io::Error::other("overlay thread exited during startup"))??;
        Ok(Self {
            tx,
            hwnd,
            thread_id,
            thread,
            done: done_rx,
        })
    }

    fn send(&self, cmd: OverlayCommand) {
        if self.tx.send(cmd).is_ok() {
            // SAFETY: both calls are thread-safe; a stale HWND or thread id only makes them fail.
            unsafe {
                let posted = PostMessageW(
                    Some(HWND(self.hwnd as *mut _)),
                    WM_APP_COMMAND,
                    WPARAM(0),
                    LPARAM(0),
                );
                if posted.is_err() {
                    let _ =
                        PostThreadMessageW(self.thread_id, WM_APP_COMMAND, WPARAM(0), LPARAM(0));
                }
            }
        }
    }

    fn join(self, timeout: Duration) -> bool {
        match self.done.recv_timeout(timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = self.thread.join();
                true
            }
            Err(mpsc::RecvTimeoutError::Timeout) => false,
        }
    }
}

struct DoneGuard(mpsc::SyncSender<()>);

impl Drop for DoneGuard {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

/// One taskbar (primary or secondary) and our windows in it.
struct TaskbarHost {
    hwnd: HWND,
    monitor: HMONITOR,
    scale: f64,
    client: SizePx,
    /// A fullscreen window is in the foreground on this taskbar's monitor.
    hidden_by_fullscreen: bool,
    /// Item key → window, in item order (later items on top).
    windows: Vec<(String, WidgetWindow)>,
}

impl TaskbarHost {
    fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            monitor: HMONITOR::default(),
            scale: 1.0,
            client: SizePx::default(),
            hidden_by_fullscreen: false,
            windows: Vec::new(),
        }
    }

    /// Our windows are the topmost children, in item order (last item first).
    fn ours_on_top(&self) -> bool {
        // SAFETY: plain z-order queries; stale handles only make them fail.
        unsafe {
            let mut child = GetWindow(self.hwnd, GW_CHILD).ok();
            for (_, w) in self.windows.iter().rev() {
                let Some(c) = child else {
                    return false;
                };
                if c != w.hwnd {
                    return false;
                }
                child = GetWindow(c, GW_HWNDNEXT).ok();
            }
        }
        true
    }

    fn raise_all(&self) {
        for (_, w) in &self.windows {
            w.raise();
        }
    }
}

/// State of the overlay thread.
struct State {
    host: HWND,
    rx: Receiver<OverlayCommand>,
    config: Arc<OverlayConfig>,
    /// Items hidden from the context menu until the next `Apply` or restart.
    session_hidden: HashSet<String>,
    taskbars: Vec<TaskbarHost>,
    /// Created on the first enable, then kept.
    d2d: Option<D2d>,
    theme: Theme,
    explorer_pid: u32,
    /// `EVENT_OBJECT_REORDER` + `EVENT_OBJECT_LOCATIONCHANGE` limited to Explorer.
    shell_hooks: Vec<HWINEVENTHOOK>,
    retries: u32,
    session_registered: bool,
}

fn thread_main(rx: Receiver<OverlayCommand>, ready: mpsc::SyncSender<io::Result<(isize, u32)>>) {
    let host = match create_host() {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    if let Err(e) = widget_window::register_class() {
        // SAFETY: our own window.
        let _ = unsafe { DestroyWindow(host) };
        let _ = ready.send(Err(e));
        return;
    }
    HOST_HWND.store(host.0 as isize, Ordering::Relaxed);
    // SAFETY: `host` is our window, alive until shutdown.
    let session_registered =
        match unsafe { WTSRegisterSessionNotification(host, NOTIFY_FOR_THIS_SESSION) } {
            Ok(()) => true,
            Err(e) => {
                log::warn!("overlay: cannot register for session notifications: {e}");
                false
            }
        };
    STATE.set(Some(State {
        host,
        rx,
        config: Arc::default(),
        session_hidden: HashSet::new(),
        taskbars: Vec::new(),
        d2d: None,
        theme: Theme::default(),
        explorer_pid: 0,
        shell_hooks: Vec::new(),
        retries: 0,
        session_registered,
    }));
    // SAFETY: no arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = ready.send(Ok((host.0 as isize, thread_id)));
    log::debug!("overlay thread started");

    let mut msg = MSG::default();
    // SAFETY: standard message loop on the thread that owns the windows.
    unsafe {
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            if msg.hwnd.is_invalid() && msg.message == WM_APP_COMMAND {
                drain_commands(host);
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    HOST_HWND.store(0, Ordering::Relaxed);
    STATE.set(None);
    log::debug!("overlay thread stopped");
}

fn create_host() -> io::Result<HWND> {
    // SAFETY: standard class registration and window creation with static strings.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(io::Error::other)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Relaxed,
        );
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            CLASS_NAME,
            w!("Kuality overlay"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(io::Error::other)?;
        // Let `TaskbarCreated` through UIPI when running elevated.
        let msg = TASKBAR_CREATED.load(Ordering::Relaxed);
        if msg != 0 {
            let _ = ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, None);
        }
        Ok(hwnd)
    }
}

/// Runs `f` on the state if it is free. If it is borrowed (re-entered from inside a state
/// call), schedules a refresh instead and returns `None`.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| match s.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => {
            if let Some(h) = host_hwnd() {
                // SAFETY: PostMessageW never blocks.
                let _ = unsafe { PostMessageW(Some(h), WM_APP_REFRESH, WPARAM(0), LPARAM(0)) };
            }
            None
        }
    })
}

/// Runs all queued commands. On `Shutdown`, destroys the host window and ends the loop.
fn drain_commands(host: HWND) {
    while let Some(Ok(cmd)) = with_state(|s| s.rx.try_recv()) {
        if with_state(|s| s.handle(cmd)) == Some(true) {
            // SAFETY: our own window, destroyed once.
            unsafe {
                let _ = DestroyWindow(host);
                PostQuitMessage(0);
            }
            break;
        }
    }
}

impl State {
    /// Returns `true` on shutdown.
    fn handle(&mut self, cmd: OverlayCommand) -> bool {
        match cmd {
            OverlayCommand::Apply(config) => {
                self.apply(config);
                false
            }
            OverlayCommand::Shutdown => {
                self.teardown();
                if self.session_registered {
                    self.session_registered = false;
                    // SAFETY: we registered this window earlier.
                    let _ = unsafe { WTSUnRegisterSessionNotification(self.host) };
                }
                true
            }
        }
    }

    fn apply(&mut self, config: Arc<OverlayConfig>) {
        let was_enabled = self.config.enabled;
        // The section changed: widgets hidden from the menu come back.
        self.session_hidden.clear();
        self.config = config;
        if !self.config.enabled {
            if was_enabled {
                self.teardown();
                log::info!("widgets disabled: windows destroyed, hooks removed");
            }
            return;
        }
        self.retries = 0;
        self.refresh();
    }

    /// Destroys every widget window, removes the hooks and stops the timers.
    fn teardown(&mut self) {
        self.taskbars.clear();
        self.unhook_explorer();
        for id in [TIMER_REFRESH, TIMER_LATE_2S, TIMER_LATE_5S, TIMER_RETRY] {
            self.kill_timer(id);
        }
        self.retries = 0;
    }

    fn kill_timer(&self, id: usize) {
        // SAFETY: our own window; killing a missing timer is harmless.
        let _ = unsafe { KillTimer(Some(self.host), id) };
    }

    fn set_timer(&self, id: usize, ms: u32) {
        // SAFETY: our own window; re-arming an existing timer resets it.
        unsafe { SetTimer(Some(self.host), id, ms, None) };
    }

    /// Debounced full refresh.
    fn schedule_refresh(&self) {
        if self.config.enabled {
            self.set_timer(TIMER_REFRESH, REFRESH_DEBOUNCE_MS);
        }
    }

    /// Extra refreshes after display changes and Explorer restarts (secondary taskbars appear
    /// asynchronously).
    fn schedule_late_refreshes(&self) {
        if self.config.enabled {
            self.set_timer(TIMER_LATE_2S, 2000);
            self.set_timer(TIMER_LATE_5S, 5000);
        }
    }

    fn on_timer(&mut self, id: usize) {
        self.kill_timer(id);
        if matches!(
            id,
            TIMER_REFRESH | TIMER_LATE_2S | TIMER_LATE_5S | TIMER_RETRY
        ) {
            self.refresh();
        }
    }

    /// Explorer (re)started: our windows died with the old taskbars.
    fn on_taskbar_created(&mut self) {
        if !self.config.enabled {
            return;
        }
        log::info!("taskbar (re)created; re-attaching widgets");
        self.taskbars.clear();
        self.unhook_explorer();
        self.retries = 0;
        self.schedule_refresh();
        self.schedule_late_refreshes();
    }

    /// Re-enumerates taskbars, reconciles windows, lays out and renders everything.
    fn refresh(&mut self) {
        self.kill_timer(TIMER_REFRESH);
        if !self.config.enabled {
            return;
        }
        if self.d2d.is_none() {
            match D2d::new() {
                Ok(d) => self.d2d = Some(d),
                Err(e) => {
                    log::error!("cannot initialize Direct2D/DirectWrite: {e}; no widgets");
                    return;
                }
            }
        }
        self.theme = theme::current();

        let found = taskbars::enumerate();
        let Some(&primary) = found.first() else {
            self.taskbars.clear();
            self.unhook_explorer();
            if self.retries < MAX_RETRIES {
                if self.retries == 0 {
                    log::info!("taskbar not found yet; retrying every second for 30 s");
                }
                self.retries += 1;
                self.set_timer(TIMER_RETRY, RETRY_MS);
            } else if self.retries == MAX_RETRIES {
                self.retries += 1;
                log::warn!("taskbar still not found; waiting for TaskbarCreated");
            }
            return;
        };
        self.retries = 0;
        let pid = taskbars::window_pid(primary);
        if pid != self.explorer_pid || self.shell_hooks.is_empty() {
            self.hook_explorer(pid);
        }

        let mut old = std::mem::take(&mut self.taskbars);
        for hwnd in found {
            let mut tb = match old.iter().position(|t| t.hwnd == hwnd) {
                Some(i) => old.swap_remove(i),
                None => TaskbarHost::new(hwnd),
            };
            (tb.monitor, tb.scale) = taskbars::monitor_scale(hwnd);
            tb.client = taskbars::client_size(hwnd).unwrap_or_default();
            self.taskbars.push(tb);
        }
        // Dropping the hosts of vanished taskbars destroys their windows.
        drop(old);
        self.layout_all();
        log::debug!(
            "overlay refreshed: {} taskbar(s), {} widget window(s)",
            self.taskbars.len(),
            self.taskbars.iter().map(|t| t.windows.len()).sum::<usize>()
        );
    }

    /// Creates, moves, renders and destroys widget windows to match the config on every
    /// taskbar.
    fn layout_all(&mut self) {
        let Some(d2d) = self.d2d.as_mut() else {
            return;
        };
        let theme = self.theme;
        // Natural sizes in DIPs do not depend on the monitor: measure once.
        let items: Vec<_> = self
            .config
            .items
            .iter()
            .filter(|i| !self.session_hidden.contains(&i.key))
            .map(|i| (i, i.content.measure(d2d)))
            .collect();
        for tb in &mut self.taskbars {
            let mut old = std::mem::take(&mut tb.windows);
            for &(item, content) in &items {
                let existing = old
                    .iter()
                    .position(|(k, _)| *k == item.key)
                    .map(|i| old.swap_remove(i).1);
                // Not shown (zero size): an existing window is dropped (destroyed).
                let Some(rect) = layout(&item.placement, tb.client, tb.scale, content) else {
                    continue;
                };
                let mut win = match existing.filter(|w| w.is_alive_in(tb.hwnd)) {
                    Some(w) => w,
                    None => match WidgetWindow::create(tb.hwnd) {
                        Ok(w) => w,
                        Err(e) => {
                            log::warn!("cannot create a widget window: {e}");
                            continue;
                        }
                    },
                };
                let box_dip = SizeF {
                    w: (f64::from(rect.w) / tb.scale) as f32,
                    h: (f64::from(rect.h) / tb.scale) as f32,
                };
                let rendered = d2d.render(&mut win.surface, rect.size(), tb.scale, |r| {
                    item.content.draw(r, box_dip, theme)
                });
                let presented = rendered.and_then(|()| match &win.surface {
                    Some(s) => s.present(win.hwnd),
                    None => Ok(()),
                });
                if let Err(e) = presented {
                    log::warn!("cannot render widget '{}': {e}", item.key);
                    continue;
                }
                win.place(rect, tb.hidden_by_fullscreen);
                tb.windows.push((item.key.clone(), win));
            }
        }
    }

    fn hook_explorer(&mut self, pid: u32) {
        self.unhook_explorer();
        self.explorer_pid = pid;
        if pid == 0 {
            return;
        }
        for event in [EVENT_OBJECT_REORDER, EVENT_OBJECT_LOCATIONCHANGE] {
            if let Some(h) = taskbars::hook(event, pid, Some(shell_event)) {
                self.shell_hooks.push(h);
            }
        }
        log::debug!("watching taskbars of explorer pid {pid}");
    }

    fn unhook_explorer(&mut self) {
        let had = !self.shell_hooks.is_empty();
        for h in self.shell_hooks.drain(..) {
            taskbars::unhook(h);
        }
        self.explorer_pid = 0;
        if had {
            log::debug!("taskbar hooks removed");
        }
    }

    fn on_shell_event(&mut self, event: u32, hwnd: HWND, id_object: i32, id_child: i32) {
        let Some(tb) = self.taskbars.iter().find(|t| t.hwnd == hwnd) else {
            return;
        };
        match event {
            EVENT_OBJECT_REORDER => {
                if !tb.ours_on_top() {
                    tb.raise_all();
                }
            }
            // Pure moves (auto-hide slide) need nothing: children move with the parent.
            EVENT_OBJECT_LOCATIONCHANGE
                if id_object == OBJID_WINDOW.0
                    && id_child == CHILDID_SELF as i32
                    && taskbars::client_size(hwnd).is_some_and(|s| s != tb.client) =>
            {
                self.schedule_refresh();
            }
            _ => {}
        }
    }
}

/// WinEvent callback for the Explorer hooks (runs on this thread while it pumps messages).
unsafe extern "system" fn shell_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Cheap pre-filter: location changes of anything but whole windows are frequent.
    if event == EVENT_OBJECT_LOCATIONCHANGE && id_object != OBJID_WINDOW.0 {
        return;
    }
    with_state(|s| s.on_shell_event(event, hwnd, id_object, id_child));
}

/// `WM_SETTINGCHANGE` with this `lParam` string: the light/dark mode changed.
fn is_color_set_change(lparam: LPARAM) -> bool {
    if lparam.0 == 0 {
        return false;
    }
    // SAFETY: for WM_SETTINGCHANGE a non-null lParam is a NUL-terminated UTF-16 string.
    let s = unsafe { PCWSTR(lparam.0 as *const u16).to_string() };
    s.is_ok_and(|s| s == "ImmersiveColorSet")
}

unsafe extern "system" fn host_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_APP_COMMAND => {
            drain_commands(hwnd);
            LRESULT(0)
        }
        WM_APP_REFRESH => {
            with_state(|s| s.schedule_refresh());
            LRESULT(0)
        }
        WM_TIMER => {
            with_state(|s| s.on_timer(wparam.0));
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            with_state(|s| {
                s.schedule_refresh();
                s.schedule_late_refreshes();
            });
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            // Covers scale changes, "show taskbar on all displays" and light/dark switches (the
            // theme is re-read on every refresh).
            if is_color_set_change(lparam) {
                log::debug!("Windows color mode changed");
            }
            with_state(|s| s.schedule_refresh());
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            if wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC {
                with_state(|s| s.schedule_refresh());
            }
            LRESULT(1)
        }
        WM_WTSSESSION_CHANGE => {
            if wparam.0 as u32 == WTS_SESSION_UNLOCK {
                with_state(|s| s.schedule_refresh());
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // e.g. `taskkill` without /F picked this window: quit the whole app cleanly.
            super::super::tray::request_quit();
            LRESULT(0)
        }
        _ if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            with_state(State::on_taskbar_created);
            LRESULT(0)
        }
        // SAFETY: default handling with the arguments we received.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
