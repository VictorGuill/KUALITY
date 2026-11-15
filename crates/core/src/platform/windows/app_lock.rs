//! App lock, Windows side (`docs/specs/app-lock.md`).
//!
//! Thread `kuality-applock` owns the [`Book`] and an out-of-context `EVENT_OBJECT_SHOW` WinEvent
//! hook (no injection; the callback runs on this thread while it pumps messages). For each
//! top-level, non-tool window of another process it reads the exe path and asks the book: hide
//! the window (`ShowWindowAsync(SW_HIDE)`) and open the prompt, remember the process, or nothing.
//! The processes the book cares about are waited on with `MsgWaitForMultipleObjectsEx`, so an
//! exit is seen at once and nothing polls.
//!
//! Hidden windows are also kept in a lock-free table (shown again by the crash guard) and in
//! `app-lock-hidden.json` (shown again by the next core after it was killed).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use ::windows::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use ::windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadId, GetProcessTimes, INFINITE, OpenProcess,
    OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use ::windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_SHOW, EnumWindows, GA_PARENT, GW_OWNER,
    GWL_EXSTYLE, GWL_STYLE, GetAncestor, GetDesktopWindow, GetWindow, GetWindowLongW,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, OBJID_WINDOW, PM_REMOVE, PeekMessageW, PostThreadMessageW,
    QS_ALLINPUT, SW_HIDE, SW_SHOW, SetForegroundWindow, ShowWindowAsync, TranslateMessage,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP, WM_QUIT, WS_CHILD, WS_EX_APPWINDOW,
    WS_EX_TOOLWINDOW,
};
use ::windows::core::BOOL;
use kuality_protocol::app_lock::normalize_path;
use kuality_protocol::store;

use super::process_image_path;
use crate::app_lock::book::{Book, Decision, Hidden};

/// Commands are waiting in the channel (thread message).
const WM_APP_COMMAND: u32 = WM_APP + 1;
/// `MsgWaitForMultipleObjectsEx` waits on at most this many handles.
const MAX_WAIT_HANDLES: usize = 63;
/// With more processes than that, the rest are checked this often.
const EXTRA_POLL_MS: u32 = 2000;
/// Slots of the crash-time table of hidden windows.
const EMERGENCY_SLOTS: usize = 128;

/// Hidden windows, for [`emergency_restore`] (lock-free: it runs in a panic hook).
static EMERGENCY: [AtomicIsize; EMERGENCY_SLOTS] = [const { AtomicIsize::new(0) }; EMERGENCY_SLOTS];

/// Crash-time: shows every window App lock hid. Never allocates, locks or panics.
pub fn emergency_restore() {
    for slot in &EMERGENCY {
        let h = slot.swap(0, Ordering::SeqCst);
        if h != 0 {
            // SAFETY: posting a show request never blocks; a stale handle only makes it fail.
            let _ = unsafe { ShowWindowAsync(HWND(h as *mut _), SW_SHOW) };
        }
    }
}

/// One program of the list: its normalized path (the book's key) and the path as configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedExe {
    pub key: String,
    pub path: String,
}

/// A command for the App lock thread.
#[derive(Debug)]
pub enum Command {
    /// The list of programs and whether a password exists. `initial`: Kuality just started, so
    /// running programs are locked at once; otherwise programs that start being locked while
    /// they run stay unlocked until they exit.
    Apply {
        apps: Vec<LockedExe>,
        armed: bool,
        initial: bool,
    },
    /// The right password was typed for the program with this key.
    Unlock {
        key: String,
        done: SyncSender<()>,
    },
    /// Tray Pause.
    SetPaused(bool),
    Shutdown,
}

/// Called with the configured path of a program whose window was just hidden.
pub type Prompt = Box<dyn FnMut(&str) + Send>;

/// Sends commands to the App lock thread (cloneable: the pipe handler has one too).
#[derive(Debug, Clone)]
pub struct Commander {
    tx: Sender<Command>,
    thread_id: u32,
}

impl Commander {
    /// Queues a command and wakes the thread.
    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_ok() {
            // SAFETY: posting a thread message never blocks.
            let _ = unsafe {
                PostThreadMessageW(
                    self.thread_id,
                    WM_APP_COMMAND,
                    Default::default(),
                    LPARAM(0),
                )
            };
        }
    }
}

/// Handle to the App lock thread.
#[derive(Debug)]
pub struct AppLockThread {
    commander: Commander,
    thread: JoinHandle<()>,
}

impl AppLockThread {
    /// Spawns the thread. `hidden_file`: `app-lock-hidden.json`; windows listed there (left by
    /// a core that was killed) are shown first.
    pub fn start(hidden_file: PathBuf, prompt: Prompt) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("kuality-applock".into())
            .spawn(move || thread_main(rx, ready_tx, hidden_file, prompt))?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                commander: Commander { tx, thread_id },
                thread,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::other("the App lock thread ended at start")),
        }
    }

    pub fn commander(&self) -> Commander {
        self.commander.clone()
    }

    /// Asks the thread to show its windows again and end; waits up to `timeout`.
    pub fn stop(self, timeout: Duration) {
        self.commander.send(Command::Shutdown);
        let deadline = Instant::now() + timeout;
        while !self.thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
        } else {
            log::warn!("App lock thread did not stop within {timeout:?}");
            emergency_restore();
        }
    }
}

/// State of the App lock thread.
struct Ctx {
    rx: Receiver<Command>,
    book: Book,
    /// Configured path of each program, by key.
    paths: HashMap<String, String>,
    /// Open handles of the processes the book cares about.
    processes: HashMap<u32, HANDLE>,
    /// Programs already reported as running as administrator.
    elevated_logged: HashSet<String>,
    hidden_file: PathBuf,
    /// Hidden set last written to `hidden_file`.
    written: Vec<Hidden>,
    prompt: Prompt,
    /// This process runs elevated: it can hide any window.
    self_elevated: bool,
    hook: Option<HWINEVENTHOOK>,
    stop: bool,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

fn with_ctx<R>(f: impl FnOnce(&mut Ctx) -> R) -> Option<R> {
    CTX.with(|c| match c.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => {
            log::error!("App lock context re-entered; event ignored");
            None
        }
    })
}

fn thread_main(
    rx: Receiver<Command>,
    ready: SyncSender<io::Result<u32>>,
    hidden_file: PathBuf,
    prompt: Prompt,
) {
    restore_left_over(&hidden_file);
    // SAFETY: out-of-context hook on this thread, unhooked at shutdown; the callback runs while
    // this thread pumps messages.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_SHOW,
            None,
            Some(win_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if hook.is_invalid() {
        let _ = ready.send(Err(io::Error::other(
            "cannot watch windows (SetWinEventHook)",
        )));
        return;
    }
    CTX.set(Some(Ctx {
        rx,
        book: Book::new(),
        paths: HashMap::new(),
        processes: HashMap::new(),
        elevated_logged: HashSet::new(),
        hidden_file,
        written: Vec::new(),
        prompt,
        self_elevated: is_elevated(None),
        hook: Some(hook),
        stop: false,
    }));
    // SAFETY: no arguments.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    log::debug!("App lock thread started");
    run_loop();
    CTX.set(None);
    log::debug!("App lock thread stopped");
}

fn run_loop() {
    loop {
        let Some((pids, handles)) = with_ctx(|c| {
            let pids: Vec<u32> = c.processes.keys().copied().collect();
            let handles: Vec<HANDLE> = pids.iter().map(|p| c.processes[p]).collect();
            (pids, handles)
        }) else {
            return;
        };
        let waited = &handles[..handles.len().min(MAX_WAIT_HANDLES)];
        let timeout = if handles.len() > MAX_WAIT_HANDLES {
            EXTRA_POLL_MS
        } else {
            INFINITE
        };
        // SAFETY: the handles stay open until this thread closes them (after the wait).
        let r = unsafe {
            MsgWaitForMultipleObjectsEx(Some(waited), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        let i = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        if i < waited.len() {
            with_ctx(|c| c.exited(pids[i]));
        } else if r == WAIT_TIMEOUT {
            let gone: Vec<u32> = pids[MAX_WAIT_HANDLES.min(pids.len())..]
                .iter()
                .zip(&handles[MAX_WAIT_HANDLES.min(handles.len())..])
                // SAFETY: an open process handle.
                .filter(|(_, h)| unsafe { WaitForSingleObject(**h, 0) } == WAIT_OBJECT_0)
                .map(|(p, _)| *p)
                .collect();
            with_ctx(|c| gone.into_iter().for_each(|p| c.exited(p)));
        }
        if !pump() || with_ctx(|c| c.stop).unwrap_or(true) {
            return;
        }
    }
}

/// Handles every queued message (WinEvent callbacks run in here). `false` on `WM_QUIT`.
fn pump() -> bool {
    let mut msg = MSG::default();
    // SAFETY: standard message retrieval on this thread.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return false;
            }
            if msg.hwnd.is_invalid() && msg.message == WM_APP_COMMAND {
                with_ctx(Ctx::drain_commands);
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

impl Ctx {
    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Command::Apply {
                    apps,
                    armed,
                    initial,
                } => self.apply(apps, armed, initial),
                Command::Unlock { key, done } => {
                    self.unlock(&key);
                    let _ = done.send(());
                }
                Command::SetPaused(paused) => {
                    let restore = self.book.set_paused(paused);
                    show_again(&restore, None);
                    log::info!("App lock {}", if paused { "paused" } else { "resumed" });
                }
                Command::Shutdown => {
                    show_again(&self.book.clear(), None);
                    if let Some(h) = self.hook.take() {
                        // SAFETY: our hook, unhooked once on this thread.
                        let _ = unsafe { UnhookWinEvent(h) };
                    }
                    self.stop = true;
                }
            }
            self.sync();
        }
    }

    fn apply(&mut self, apps: Vec<LockedExe>, armed: bool, initial: bool) {
        let was_armed = self.book.armed();
        let keys: Vec<String> = apps.iter().map(|a| a.key.clone()).collect();
        let newly: HashSet<String> = keys
            .iter()
            .filter(|k| !self.book.is_listed(k))
            .cloned()
            .collect();
        self.paths = apps.into_iter().map(|a| (a.key, a.path)).collect();
        show_again(&self.book.set_apps(&keys), None);
        show_again(&self.book.set_armed(armed), None);
        if !armed {
            return;
        }
        let just_armed = !was_armed;
        for hwnd in visible_top_level_windows() {
            let Some((pid, key)) = window_program(hwnd) else {
                continue;
            };
            if !self.book.is_listed(&key) {
                continue;
            }
            if initial {
                self.on_window(hwnd, pid, key);
            } else if just_armed || newly.contains(&key) {
                self.book.adopt_running(&key, pid);
            }
        }
    }

    fn unlock(&mut self, key: &str) {
        let Some(windows) = self.book.unlock(key) else {
            return;
        };
        log::info!(
            "App lock: {} unlocked",
            self.paths.get(key).map_or(key, String::as_str)
        );
        // The main window (no owner) comes to the front; the prompt allowed this process to.
        let front = windows
            .iter()
            .rev()
            // SAFETY: plain query; a stale handle returns null.
            .find(|h| unsafe { GetWindow(hwnd(h.window), GW_OWNER) }.is_err())
            .or(windows.last())
            .map(|h| h.window);
        show_again(&windows, front);
    }

    /// A window of `key` (process `pid`) appeared.
    fn on_window(&mut self, window: HWND, pid: u32, key: String) {
        let h = Hidden {
            pid,
            window: window.0 as isize,
        };
        match self.book.on_window(&key, pid, h.window) {
            Decision::Ignore | Decision::Allow => {}
            Decision::HideAndPrompt => {
                if !self.self_elevated && is_elevated(Some(pid)) {
                    if self.elevated_logged.insert(key.clone()) {
                        log::warn!(
                            "App lock: {} runs as administrator; it cannot be locked unless \
                             Kuality runs as administrator too",
                            self.paths.get(&key).map_or(key.as_str(), String::as_str)
                        );
                    }
                    self.book.hide_failed(&key, h);
                    return;
                }
                // SAFETY: posting a hide request never blocks; a stale handle only fails.
                let _ = unsafe { ShowWindowAsync(window, SW_HIDE) };
                log::debug!("App lock: hid a window of {key}");
                let path = self.paths.get(&key).cloned().unwrap_or(key);
                (self.prompt)(&path);
            }
        }
        self.sync();
    }

    fn exited(&mut self, pid: u32) {
        if let Some(h) = self.processes.remove(&pid) {
            // SAFETY: our handle, closed once.
            let _ = unsafe { CloseHandle(h) };
        }
        for key in self.book.on_exit(pid) {
            log::info!(
                "App lock: {} exited; locked again",
                self.paths.get(&key).map_or(key.as_str(), String::as_str)
            );
        }
        self.sync();
    }

    /// Opens / closes process handles to match the book, and saves the hidden windows.
    fn sync(&mut self) {
        loop {
            let wanted: BTreeSet<u32> = self.book.processes();
            self.processes.retain(|pid, h| {
                let keep = wanted.contains(pid);
                if !keep {
                    // SAFETY: our handle, closed once.
                    let _ = unsafe { CloseHandle(*h) };
                }
                keep
            });
            let mut gone = Vec::new();
            for pid in wanted {
                if self.processes.contains_key(&pid) {
                    continue;
                }
                // SAFETY: the handle is kept in `processes` and closed when no longer needed.
                match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
                    Ok(h) => {
                        self.processes.insert(pid, h);
                    }
                    Err(_) => gone.push(pid),
                }
            }
            if gone.is_empty() {
                break;
            }
            for pid in gone {
                self.book.on_exit(pid);
            }
        }
        let hidden = self.book.hidden();
        if hidden != self.written {
            remember_hidden(&hidden);
            save_hidden(&self.hidden_file, &hidden);
            self.written = hidden;
        }
    }
}

fn hwnd(window: isize) -> HWND {
    HWND(window as *mut _)
}

/// `EVENT_OBJECT_SHOW` for any window of another process.
unsafe extern "system" fn win_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    window: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 || window.is_invalid() {
        return;
    }
    let handled = catch_unwind(AssertUnwindSafe(|| {
        if !any_listed() {
            return;
        }
        if !is_candidate(window) {
            return;
        }
        let Some((pid, key)) = window_program(window) else {
            return;
        };
        with_ctx(|c| {
            if c.book.is_listed(&key) {
                c.on_window(window, pid, key);
            }
        });
    }));
    if handled.is_err() {
        emergency_restore();
        log::error!("App lock: the window callback panicked; hidden windows shown again");
    }
}

/// The book lists at least one program (windows of listed programs matter even while paused:
/// they mark the program unlocked until it exits).
fn any_listed() -> bool {
    with_ctx(|c| !c.paths.is_empty()).unwrap_or(false)
}

/// A visible top-level window that is not a tool window (tooltips, menus) nor a child.
fn is_candidate(window: HWND) -> bool {
    // SAFETY: plain queries on a window handle; a stale handle gives zeros / false.
    unsafe {
        if !IsWindowVisible(window).as_bool() {
            return false;
        }
        let style = GetWindowLongW(window, GWL_STYLE) as u32;
        let ex = GetWindowLongW(window, GWL_EXSTYLE) as u32;
        if style & WS_CHILD.0 != 0 {
            return false;
        }
        if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
            return false;
        }
        GetAncestor(window, GA_PARENT) == GetDesktopWindow()
    }
}

/// The process of `window` and its normalized exe path (not for Kuality's own windows).
fn window_program(window: HWND) -> Option<(u32, String)> {
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    if pid == 0 || pid == std::process::id() {
        return None;
    }
    // SAFETY: the handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let path = process_image_path(process);
    // SAFETY: our handle, closed once.
    let _ = unsafe { CloseHandle(process) };
    Some((pid, normalize_path(&path?)))
}

/// Visible top-level candidate windows, front to back.
fn visible_top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(window: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut Vec<HWND>` passed below, alive during EnumWindows.
        let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        if is_candidate(window) {
            list.push(window);
        }
        true.into()
    }
    let mut list: Vec<HWND> = Vec::new();
    // SAFETY: the callback only runs during the call and gets a pointer to `list`.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut list as *mut Vec<HWND> as isize)) };
    list
}

/// Shows hidden windows again (those that still exist and still belong to their process), then
/// brings `front` to the front.
fn show_again(windows: &[Hidden], front: Option<isize>) {
    for h in windows {
        let w = hwnd(h.window);
        let mut pid = 0u32;
        // SAFETY: plain queries and a non-blocking show request; stale handles only fail.
        unsafe {
            if !IsWindow(Some(w)).as_bool() {
                continue;
            }
            GetWindowThreadProcessId(w, Some(&mut pid));
            if pid != h.pid {
                continue;
            }
            let _ = ShowWindowAsync(w, SW_SHOW);
        }
    }
    if let Some(f) = front {
        // SAFETY: plain call; allowed because the prompt granted it (AllowSetForegroundWindow).
        let _ = unsafe { SetForegroundWindow(hwnd(f)) };
    }
}

/// Fills the crash-time table with `hidden`.
fn remember_hidden(hidden: &[Hidden]) {
    for (i, slot) in EMERGENCY.iter().enumerate() {
        slot.store(hidden.get(i).map_or(0, |h| h.window), Ordering::SeqCst);
    }
}

/// Whether process `pid` (`None`: this process) runs elevated. A process whose token cannot be
/// read from here counts as elevated (that is what a medium-integrity process sees).
fn is_elevated(pid: Option<u32>) -> bool {
    // SAFETY: handles are checked and closed once; the token query writes into a local.
    unsafe {
        let (process, owned) = match pid {
            None => (GetCurrentProcess(), false),
            Some(pid) => match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => (h, true),
                Err(_) => return true,
            },
        };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token);
        if owned {
            let _ = CloseHandle(process);
        }
        if opened.is_err() {
            return pid.is_some();
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        ok.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// Creation time of process `pid` (100 ns units), to tell it from a later process with the same
/// id.
fn process_created(pid: u32) -> Option<u64> {
    // SAFETY: the handle is closed below; the times are written into locals.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut created, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let ok = GetProcessTimes(h, &mut created, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(h);
        ok.ok()?;
        Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

/// Writes `app-lock-hidden.json` (removes it when nothing is hidden).
fn save_hidden(path: &PathBuf, hidden: &[Hidden]) {
    if hidden.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("cannot remove {}: {e}", path.display()),
        }
        return;
    }
    let records: Vec<serde_json::Value> = hidden
        .iter()
        .map(|h| {
            serde_json::json!({
                "pid": h.pid,
                "created": process_created(h.pid).unwrap_or(0),
                "hwnd": h.window as i64,
            })
        })
        .collect();
    let bytes = serde_json::to_vec(&records).unwrap_or_default();
    if let Err(e) = store::write_atomic(path, &bytes) {
        log::warn!("cannot write {}: {e}", path.display());
    }
}

/// Shows the windows a killed core left hidden, then removes the file.
fn restore_left_over(path: &PathBuf) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let records: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap_or_default();
    let mut shown = 0;
    for r in records {
        let (Some(pid), Some(created), Some(window)) =
            (r["pid"].as_u64(), r["created"].as_u64(), r["hwnd"].as_i64())
        else {
            continue;
        };
        let pid = pid as u32;
        if created == 0 || process_created(pid) != Some(created) {
            continue;
        }
        show_again(
            &[Hidden {
                pid,
                window: window as isize,
            }],
            None,
        );
        shown += 1;
    }
    if shown > 0 {
        log::info!("App lock: {shown} window(s) left hidden by a previous run shown again");
    }
    let _ = std::fs::remove_file(path);
}
