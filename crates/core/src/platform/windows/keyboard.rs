//! Keyboard thread: low-level keyboard hook + Hyperkey engine, see `docs/specs/hyperkey.md`.
//!
//! One thread (`kuality-keyboard`) lives for the whole process. It owns a hidden top-level
//! window (so it receives session, power and other broadcasts), the [`Engine`] and the hook.
//! The hook itself is installed only while Hyperkey is active (enabled and not paused).
//!
//! The hook callback follows strict rules: no heap allocation, no locks, no logging, nothing
//! that can block. It only runs the engine, calls `GetKeyState`, one `SendInput` and posts
//! trace messages to its own window.

use std::cell::RefCell;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use ::windows::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use ::windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyState, VK_CAPITAL};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    EVENT_SYSTEM_DESKTOPSWITCH, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
    PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND, PostMessageW, PostQuitMessage, PostThreadMessageW,
    RegisterClassExW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WINDOW_EX_STYLE, WINEVENT_OUTOFCONTEXT, WM_APP, WM_CLOSE, WM_KEYDOWN, WM_KEYUP,
    WM_POWERBROADCAST, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_WTSSESSION_CHANGE, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_POPUP, WTS_CONSOLE_CONNECT, WTS_CONSOLE_DISCONNECT, WTS_REMOTE_CONNECT,
    WTS_REMOTE_DISCONNECT, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};
use ::windows::core::{PCWSTR, w};
use kuality_protocol::Key;

use super::input::{self, KUALITY_TAG};
use crate::hyperkey::KeyboardCommand;
use crate::hyperkey::engine::{Engine, HYPER_RELEASE_ORDER, KeyEvent, Synth, SynthBuf, Transition};
use crate::platform::KeyboardBackend;

const CLASS_NAME: PCWSTR = w!("KualityKeyboard");
/// Commands are waiting in the channel.
const WM_APP_COMMAND: u32 = WM_APP + 1;
/// Posted by the hook callback: `wparam` is one of the `TRACE_*` codes.
const WM_APP_TRACE: u32 = WM_APP + 2;
const TRACE_HYPER_DOWN: usize = 1;
const TRACE_HYPER_UP: usize = 2;
const TRACE_PANIC: usize = 3;

/// The keyboard thread's window, for the hook callback (read with one atomic load).
static KEYBOARD_HWND: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    /// Used by the hook callback; `const` init so first access never allocates.
    static ENGINE: RefCell<Engine> = const { RefCell::new(Engine::new()) };
    /// Thread state outside the callback (commands, hook handles).
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

/// Handle to the keyboard thread, owned by the main thread.
#[derive(Debug)]
pub struct Keyboard {
    tx: Sender<KeyboardCommand>,
    hwnd: isize,
    thread_id: u32,
    thread: JoinHandle<()>,
    done: Receiver<()>,
}

impl KeyboardBackend for Keyboard {
    fn start() -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<io::Result<(isize, u32)>>(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("kuality-keyboard".into())
            .spawn(move || {
                // Signals `join` even if the thread unwinds.
                let _done = DoneGuard(done_tx);
                thread_main(rx, ready_tx);
            })?;
        let (hwnd, thread_id) = ready_rx
            .recv()
            .map_err(|_| io::Error::other("keyboard thread exited during startup"))??;
        Ok(Self {
            tx,
            hwnd,
            thread_id,
            thread,
            done: done_rx,
        })
    }

    fn send(&self, cmd: KeyboardCommand) {
        if self.tx.send(cmd).is_ok() {
            // SAFETY: both calls are thread-safe; a stale HWND or thread id only makes them fail.
            unsafe {
                let posted = PostMessageW(
                    Some(HWND(self.hwnd as *mut _)),
                    WM_APP_COMMAND,
                    WPARAM(0),
                    LPARAM(0),
                );
                // Fallback if the window is gone: the message loop also handles thread messages.
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

/// State of the keyboard thread outside the hook callback.
struct Ctx {
    hwnd: HWND,
    rx: Receiver<KeyboardCommand>,
    enabled: bool,
    paused: bool,
    hook: Option<HHOOK>,
    win_event: Option<HWINEVENTHOOK>,
    session_registered: bool,
    /// A release could not be injected (e.g. secure desktop); clean up on the next switch.
    cleanup_pending: bool,
}

fn thread_main(rx: Receiver<KeyboardCommand>, ready: mpsc::SyncSender<io::Result<(isize, u32)>>) {
    // SAFETY: changes the priority of the current thread only.
    if let Err(e) = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) } {
        log::warn!("cannot raise keyboard thread priority: {e}");
    }
    let hwnd = match create_window() {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    KEYBOARD_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

    // SAFETY: `hwnd` is our window, alive until DestroyWindow at shutdown.
    let session_registered =
        match unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) } {
            Ok(()) => true,
            Err(e) => {
                log::warn!("cannot register for session notifications: {e}");
                false
            }
        };
    // SAFETY: out-of-context WinEvent hook; the callback runs on this thread while it pumps
    // messages.
    let win_event = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_DESKTOPSWITCH,
            EVENT_SYSTEM_DESKTOPSWITCH,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    let win_event = if win_event.is_invalid() {
        log::warn!("cannot watch desktop switches");
        None
    } else {
        Some(win_event)
    };

    CTX.set(Some(Ctx {
        hwnd,
        rx,
        enabled: false,
        paused: false,
        hook: None,
        win_event,
        session_registered,
        cleanup_pending: false,
    }));
    // SAFETY: no arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = ready.send(Ok((hwnd.0 as isize, thread_id)));
    log::debug!("keyboard thread started");

    let mut msg = MSG::default();
    // SAFETY: standard message loop on the thread that owns the window. No modal loops run on
    // this thread, so the hook callback is always serviced promptly.
    unsafe {
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            if msg.hwnd.is_invalid() && msg.message == WM_APP_COMMAND {
                // Thread message (fallback wake-up when the window is gone).
                drain_commands(hwnd);
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    KEYBOARD_HWND.store(0, Ordering::SeqCst);
    log::debug!("keyboard thread stopped");
}

fn create_window() -> io::Result<HWND> {
    // SAFETY: standard class registration and window creation with static strings.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(io::Error::other)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            CLASS_NAME,
            w!("Kuality keyboard"),
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
        .map_err(io::Error::other)
    }
}

/// Runs `f` on the thread context; does nothing if it is missing or already borrowed.
fn with_ctx<R>(f: impl FnOnce(&mut Ctx) -> R) -> Option<R> {
    CTX.with(|c| match c.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => {
            log::error!("keyboard context re-entered; event ignored");
            None
        }
    })
}

impl Ctx {
    fn active(&self) -> bool {
        self.enabled && !self.paused
    }

    fn handle(&mut self, cmd: KeyboardCommand) -> bool {
        match cmd {
            KeyboardCommand::Apply { hyperkey } => {
                self.enabled = hyperkey.enabled;
                self.update(true);
            }
            KeyboardCommand::Shutdown => {
                release_all();
                self.uninstall_hook();
                if let Some(h) = self.win_event.take() {
                    // SAFETY: `h` came from SetWinEventHook on this thread.
                    let _ = unsafe { UnhookWinEvent(h) };
                }
                if self.session_registered {
                    self.session_registered = false;
                    // SAFETY: we registered this window earlier.
                    let _ = unsafe { WTSUnRegisterSessionNotification(self.hwnd) };
                }
                return true;
            }
        }
        false
    }

    /// Installs or removes the hook to match `active()`. With `reinstall`, an installed hook is
    /// replaced (recovers a hook Windows silently removed).
    fn update(&mut self, reinstall: bool) {
        let want = self.active();
        match (self.hook.is_some(), want) {
            (false, true) => {
                self.install_hook();
                caps_toggle_off();
                log::info!("hyperkey active");
            }
            (true, false) => {
                release_all();
                self.uninstall_hook();
                log::info!(
                    "hyperkey inactive ({})",
                    if self.enabled { "paused" } else { "disabled" }
                );
            }
            (true, true) if reinstall => {
                release_all();
                self.uninstall_hook();
                self.install_hook();
                log::debug!("keyboard hook reinstalled");
            }
            _ => {}
        }
    }

    fn install_hook(&mut self) {
        // SAFETY: global LL hook served by this thread's message loop; `hook_proc` follows the
        // callback rules.
        let r = unsafe {
            GetModuleHandleW(None)
                .and_then(|m| SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(m.into()), 0))
        };
        match r {
            Ok(h) => self.hook = Some(h),
            Err(e) => log::error!("cannot install keyboard hook: {e}"),
        }
    }

    fn uninstall_hook(&mut self) {
        if let Some(h) = self.hook.take() {
            // SAFETY: `h` is our hook handle.
            if let Err(e) = unsafe { UnhookWindowsHookEx(h) } {
                log::warn!("UnhookWindowsHookEx failed: {e}");
            }
        }
    }

    /// Lock, disconnect, suspend or desktop switch: release and forget everything.
    fn on_leave(&mut self, why: &str) {
        let had = ENGINE.with_borrow(Engine::modifiers_down);
        let ok = release_all();
        ENGINE.with_borrow_mut(Engine::reset);
        if had && !ok {
            self.cleanup_pending = true;
        }
        if had {
            log::debug!("{why}: forced release (injected: {ok})");
        }
    }

    /// Unlock, connect or resume: clear leftovers, reinstall the hook, CapsLock toggle off.
    fn on_return(&mut self, why: &str) {
        ENGINE.with_borrow_mut(Engine::reset);
        clear_leftover_modifiers();
        self.cleanup_pending = false;
        if self.active() {
            self.uninstall_hook();
            self.install_hook();
            caps_toggle_off();
        }
        log::debug!("{why}: state reset");
    }
}

/// Forces Hyper up and injects the release. Returns `false` if the injection failed.
fn release_all() -> bool {
    let buf = ENGINE.with_borrow_mut(Engine::force_release);
    let ok = input::send(&buf);
    if ok {
        input::set_modifiers_down(ENGINE.with_borrow(Engine::modifiers_down));
    }
    ok
}

/// Injects `Up` for any Hyper modifier the system still reports down (mask key first).
fn clear_leftover_modifiers() {
    let mut buf = SynthBuf::new();
    for k in HYPER_RELEASE_ORDER {
        // SAFETY: reads the async key state; no pointers.
        if unsafe { GetAsyncKeyState(i32::from(k.0)) } < 0 {
            if buf.is_empty() {
                buf.push(Synth::Down(Key::MASK));
                buf.push(Synth::Up(Key::MASK));
            }
            buf.push(Synth::Up(k));
        }
    }
    if !buf.is_empty() {
        log::info!("released modifiers left down");
        let _ = input::send(&buf);
    }
    input::set_modifiers_down(false);
}

/// Real CapsLock is not available while Hyperkey is active: turn the toggle off if it is on.
fn caps_toggle_off() {
    if caps_toggled_on() {
        let mut buf = SynthBuf::new();
        buf.push(Synth::Down(Key::CAPSLOCK));
        buf.push(Synth::Up(Key::CAPSLOCK));
        if input::send(&buf) {
            log::info!("CapsLock was on; turned it off");
        }
    }
}

fn caps_toggled_on() -> bool {
    // SAFETY: reads the key state; no pointers.
    unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) & 1 != 0 }
}

/// Runs all queued commands. On `Shutdown`, destroys the window and ends the message loop.
fn drain_commands(hwnd: HWND) {
    while let Some(Ok(cmd)) = with_ctx(|c| c.rx.try_recv()) {
        if with_ctx(|c| c.handle(cmd)) == Some(true) {
            // Outside the context borrow: DestroyWindow sends messages to the window proc.
            // SAFETY: our own window, destroyed once (failure is harmless if already gone).
            unsafe {
                let _ = DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            break;
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_COMMAND => {
            drain_commands(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            // e.g. `taskkill` without /F picked this window: quit the whole app cleanly instead
            // of destroying the keyboard window.
            super::tray::request_quit();
            LRESULT(0)
        }
        WM_APP_TRACE => {
            match wparam.0 {
                TRACE_HYPER_DOWN => log::debug!("hyper down"),
                TRACE_HYPER_UP => log::debug!("hyper up"),
                TRACE_PANIC => log::error!("keyboard hook panicked; modifiers released"),
                _ => {}
            }
            LRESULT(0)
        }
        WM_WTSSESSION_CHANGE => {
            match wparam.0 as u32 {
                WTS_SESSION_LOCK => with_ctx(|c| c.on_leave("session lock")),
                WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT => {
                    with_ctx(|c| c.on_leave("session disconnect"))
                }
                WTS_SESSION_UNLOCK => with_ctx(|c| c.on_return("session unlock")),
                WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => {
                    with_ctx(|c| c.on_return("session connect"))
                }
                _ => None,
            };
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            match wparam.0 as u32 {
                PBT_APMSUSPEND => with_ctx(|c| c.on_leave("suspend")),
                PBT_APMRESUMEAUTOMATIC => with_ctx(|c| c.on_return("resume")),
                _ => None,
            };
            LRESULT(1)
        }
        // SAFETY: default handling with the arguments we received.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// `EVENT_SYSTEM_DESKTOPSWITCH` (lock screen, Ctrl+Alt+Del, UAC secure desktop, and back).
unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    with_ctx(|c| {
        let pending = c.cleanup_pending;
        c.on_leave("desktop switch");
        if pending {
            // A release failed on the secure desktop; we may be back on the normal one now.
            clear_leftover_modifiers();
            c.cleanup_pending = false;
        }
    });
}

/// The low-level keyboard hook. See the module docs for the rules it follows.
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for `code >= 0`, `lparam` points to a valid KBDLLHOOKSTRUCT for this call.
        match catch_unwind(AssertUnwindSafe(|| unsafe { on_key(wparam, lparam) })) {
            Ok(true) => return LRESULT(1),
            Ok(false) => {}
            Err(_) => {
                input::emergency_release();
                ENGINE.with(|e| {
                    if let Ok(mut e) = e.try_borrow_mut() {
                        e.reset();
                    }
                });
                post_trace(TRACE_PANIC);
            }
        }
    }
    // SAFETY: forwards the arguments we received.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Handles one hook event. Returns `true` to swallow it.
///
/// # Safety
/// `lparam` must point to a valid `KBDLLHOOKSTRUCT`.
unsafe fn on_key(wparam: WPARAM, lparam: LPARAM) -> bool {
    // SAFETY: guaranteed by the caller.
    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    if info.flags.0 & LLKHF_INJECTED.0 != 0 && info.dwExtraInfo == KUALITY_TAG {
        return false;
    }
    let down = match wparam.0 as u32 {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return false,
    };
    let Ok(vk) = u16::try_from(info.vkCode) else {
        return false;
    };
    let ev = KeyEvent { key: Key(vk), down };

    let result = ENGINE.with(|e| {
        let mut e = e.try_borrow_mut().ok()?;
        let caps_on = ev.key == Key::CAPSLOCK && down && !e.is_held() && caps_toggled_on();
        let verdict = e.on_event(ev, caps_on);
        Some((verdict, e.modifiers_down()))
    });
    // Borrow dropped before injecting.
    let Some((verdict, mods_down)) = result else {
        return false;
    };
    if !verdict.synth.is_empty() {
        let ok = input::send(&verdict.synth);
        // Stay conservative: only clear the flag once a release was really injected.
        if ok || mods_down {
            input::set_modifiers_down(mods_down);
        }
    }
    match verdict.transition {
        Some(Transition::HyperDown) => post_trace(TRACE_HYPER_DOWN),
        Some(Transition::HyperUp) => post_trace(TRACE_HYPER_UP),
        None => {}
    }
    verdict.swallow
}

fn post_trace(code: usize) {
    let hwnd = KEYBOARD_HWND.load(Ordering::Relaxed);
    if hwnd != 0 {
        // SAFETY: PostMessageW does not block; a stale HWND only makes it fail.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(hwnd as *mut _)),
                WM_APP_TRACE,
                WPARAM(code),
                LPARAM(0),
            )
        };
    }
}
