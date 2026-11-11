//! Brings the first window of a launched app to the front (runs on the `kuality-actions` thread).
//!
//! The grant given before `ShellExecuteExW` (`AllowSetForegroundWindow(ASFW_ANY)`) ends at the
//! next input, and releasing Hyper is input, so an app whose first window appears after the
//! release opens behind the current window. After a launch the action thread therefore checks
//! every [`TICK_MS`] for up to [`crate::actions::FOCUS_WAIT`] for the app's first window: a visible, unowned,
//! non-tool top-level window of the started process, or, when there is no process (packaged
//! apps, `shell:AppsFolder`, URLs, a running single-instance app) or it shows no window of its
//! own (launcher stubs), any such window that was not there before the launch. It brings that
//! window forward only while the foreground is still the window that was in front when the
//! shortcut fired; once the foreground changes (the app came forward itself, or the user moved
//! on) it stops. A timer on the action thread drives the checks, so jobs never wait for it.

use std::cell::RefCell;
use std::time::Instant;

use ::windows::Win32::Foundation::{HWND, LPARAM};
use ::windows::Win32::System::Threading::GetCurrentProcessId;
use ::windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongPtrW,
    GetWindowThreadProcessId, IsWindowVisible, KillTimer, SetForegroundWindow, SetTimer,
    WS_EX_TOOLWINDOW,
};
use ::windows::core::BOOL;

use super::input;
use crate::actions::{FocusStep, focus_step};

/// Check interval.
const TICK_MS: u32 = 50;

/// The window in front when a shortcut fired (compared by value only, never dereferenced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Foreground(pub(super) isize);

/// The current foreground window. Callable from any thread.
pub fn foreground_window() -> Foreground {
    // SAFETY: no arguments.
    Foreground(unsafe { GetForegroundWindow() }.0 as isize)
}

/// Candidate windows that exist before a launch (so a new one can be told apart).
pub struct Before(Vec<isize>);

/// Call right before `ShellExecuteExW`.
pub fn before_launch() -> Before {
    Before(app_windows(|_, _| true))
}

struct Watch {
    timer: usize,
    started: Instant,
    foreground: Foreground,
    pid: Option<u32>,
    known: Vec<isize>,
}

thread_local! {
    /// The launch being watched (one at a time: a new launch replaces it).
    static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
}

/// Starts watching after a successful `ShellExecuteExW` (`pid`: the started process, if the
/// shell returned one). Must run on a thread that pumps messages.
pub fn after_launch(foreground: Foreground, before: Before, pid: Option<u32>) {
    stop();
    // SAFETY: thread timer (no window); `tick` runs on this thread from its message loop.
    let timer = unsafe { SetTimer(None, 0, TICK_MS, Some(tick)) };
    if timer == 0 {
        log::debug!("cannot start the launch watcher timer");
        return;
    }
    WATCH.set(Some(Watch {
        timer,
        started: Instant::now(),
        foreground,
        pid,
        known: before.0,
    }));
}

fn stop() {
    if let Some(w) = WATCH.take() {
        // SAFETY: a thread timer created by `after_launch` on this thread.
        let _ = unsafe { KillTimer(None, w.timer) };
    }
}

unsafe extern "system" fn tick(_hwnd: HWND, _msg: u32, _id: usize, _time: u32) {
    let Some((step, window)) = WATCH.with_borrow(|w| w.as_ref().map(check)) else {
        return;
    };
    match step {
        FocusStep::Wait => {}
        FocusStep::Stop => stop(),
        FocusStep::Raise => {
            stop();
            raise(window);
        }
    }
}

fn check(w: &Watch) -> (FocusStep, HWND) {
    let unchanged = foreground_window() == w.foreground;
    let window = if unchanged { find(w) } else { None };
    let step = focus_step(w.started.elapsed(), unchanged, window.is_some());
    (step, window.unwrap_or_default())
}

/// The launched app's first window: one of its process if it has one, else a new one.
fn find(w: &Watch) -> Option<HWND> {
    let own = w
        .pid
        .and_then(|pid| app_windows(|_, p| p == pid).first().copied());
    own.or_else(|| app_windows(|h, _| !w.known.contains(&h)).first().copied())
        .map(|h| HWND(h as *mut _))
}

fn raise(window: HWND) {
    // Releasing Hyper (injected ups) or the user's own keys may have ended the grant; the empty
    // tagged event makes us the last input source, which lets us set the foreground ourselves.
    let claimed = input::claim_last_input();
    // SAFETY: a destroyed window only makes the call fail.
    let ok = unsafe { SetForegroundWindow(window) }.as_bool();
    log::debug!("launched app's window brought to the front: {ok} (input claimed: {claimed})");
}

/// Visible, unowned, non-tool top-level windows of other processes, in Z order, filtered by
/// `keep(hwnd, pid)`.
fn app_windows(keep: impl Fn(isize, u32) -> bool) -> Vec<isize> {
    struct Ctx<'a> {
        keep: &'a dyn Fn(isize, u32) -> bool,
        own_pid: u32,
        out: Vec<isize>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Ctx` below, alive for the whole `EnumWindows` call.
        let ctx = unsafe { &mut *(lparam.0 as *mut Ctx) };
        let mut pid = 0;
        // SAFETY: plain window queries; a destroyed window only makes them fail.
        let candidate = unsafe {
            IsWindowVisible(hwnd).as_bool()
                && GetWindow(hwnd, GW_OWNER).is_err()
                && GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW.0 as isize == 0
                && GetWindowThreadProcessId(hwnd, Some(&mut pid)) != 0
        };
        let h = hwnd.0 as isize;
        if candidate && pid != ctx.own_pid && (ctx.keep)(h, pid) {
            ctx.out.push(h);
        }
        true.into()
    }
    let mut ctx = Ctx {
        keep: &keep,
        // SAFETY: no arguments.
        own_pid: unsafe { GetCurrentProcessId() },
        out: Vec::new(),
    };
    // SAFETY: `each` only uses `ctx` during the call.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut ctx as *mut Ctx as isize)) };
    ctx.out
}
