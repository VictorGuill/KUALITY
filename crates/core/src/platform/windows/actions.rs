//! Message loop of the `kuality-actions` worker thread, and clean quit when the shell leaves
//! windows in our process.
//!
//! The thread is a COM single-threaded apartment, so it pumps messages. Jobs arrive as a thread
//! message (`WM_APP_JOB`) that wakes the loop; the job data itself travels through a channel owned
//! by the portable layer.
//!
//! `ShellExecuteExW` can leave hidden top-level windows in our process, on this thread or on shell
//! helper threads (e.g. a `WorkerW` that lives until the helper thread times out).
//! `taskkill` without `/F` delivers `WM_CLOSE` to one top-level window of the process, which can
//! be one of those, so the app would not quit. Therefore:
//! - a `WM_CLOSE` to any window of this thread (posted, seen by the loop, or sent, seen by a
//!   thread-local `WH_CALLWNDPROC` hook) requests a clean app quit;
//! - an out-of-context `EVENT_OBJECT_CREATE` WinEvent hook (our process only, delivered to this
//!   thread) spots top-level windows created by threads that are not Kuality's, and installs
//!   `WH_CALLWNDPROC` + `WH_GETMESSAGE` hooks on those threads (same process, no injection) that
//!   do the same.
//!
//! Kuality's own threads (main thread, threads named `kuality-*`) are never hooked: they handle
//! `WM_CLOSE` themselves.

use std::cell::RefCell;

use ::windows::Win32::Foundation::{CloseHandle, HLOCAL, HWND, LPARAM, LRESULT, LocalFree, WPARAM};
use ::windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, GetThreadDescription, OpenThread,
    THREAD_QUERY_LIMITED_INFORMATION,
};
use ::windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CWPSTRUCT, CallNextHookEx, DispatchMessageW, EVENT_OBJECT_CREATE, GA_PARENT, GetAncestor,
    GetDesktopWindow, GetMessageW, MSG, OBJID_WINDOW, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, WH_CALLWNDPROC, WH_GETMESSAGE,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNTHREAD, WM_APP, WM_CLOSE, WM_QUIT, WM_USER,
};

/// Thread message: jobs are waiting in the channel.
const WM_APP_JOB: u32 = WM_APP + 1;

thread_local! {
    /// Foreign threads already hooked (only touched on the action thread).
    static HOOKED: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

/// Per-thread setup (COM STA, message queue, close watchers). Returns the thread id used to wake
/// or stop the loop.
pub fn action_thread_init() -> u32 {
    super::init_sta();
    let mut msg = MSG::default();
    // SAFETY: creates this thread's message queue so PostThreadMessageW works right away.
    let thread_id = unsafe {
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
        GetCurrentThreadId()
    };
    // SAFETY: hook limited to this thread; removed by Windows when the thread ends.
    let hook = unsafe { SetWindowsHookExW(WH_CALLWNDPROC, Some(call_wnd_proc), None, thread_id) };
    if let Err(e) = hook {
        log::warn!("cannot watch the action thread's windows: {e}");
    }
    // SAFETY: out-of-context hook for our own process; callbacks run on this thread while it
    // pumps messages. Removed by Windows when the thread ends.
    let win_event = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_CREATE,
            EVENT_OBJECT_CREATE,
            None,
            Some(on_window_created),
            GetCurrentProcessId(),
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNTHREAD,
        )
    };
    if win_event.is_invalid() {
        log::warn!("cannot watch windows created by the shell");
    }
    thread_id
}

/// Runs the message loop until [`action_thread_stop`]; calls `on_jobs` for every wake-up.
pub fn action_thread_run(mut on_jobs: impl FnMut()) {
    let mut msg = MSG::default();
    loop {
        // SAFETY: standard message loop on the current thread.
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 == 0 || r.0 == -1 {
            break;
        }
        if msg.hwnd.is_invalid() && msg.message == WM_APP_JOB {
            on_jobs();
            continue;
        }
        if msg.message == WM_CLOSE {
            log::debug!("WM_CLOSE posted to a window of the action thread; quitting");
            super::tray::request_quit();
            continue;
        }
        // SAFETY: dispatches a message we just received.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Wakes the loop to run queued jobs. Returns `false` if the thread is gone.
pub fn action_thread_wake(thread_id: u32) -> bool {
    // SAFETY: thread-safe; a stale thread id only makes it fail.
    unsafe { PostThreadMessageW(thread_id, WM_APP_JOB, WPARAM(0), LPARAM(0)) }.is_ok()
}

/// Ends the loop after the messages already queued (so queued jobs still run).
pub fn action_thread_stop(thread_id: u32) {
    // SAFETY: as above.
    let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
}

/// `EVENT_OBJECT_CREATE` in our process (runs on the action thread).
unsafe extern "system" fn on_window_created(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    thread_id: u32,
    _time: u32,
) {
    if id_object != OBJID_WINDOW.0 || id_child != 0 || hwnd.is_invalid() {
        return;
    }
    // SAFETY: plain window queries; a destroyed window only makes them fail.
    let top_level = unsafe { GetAncestor(hwnd, GA_PARENT) == GetDesktopWindow() };
    if !top_level || is_kuality_thread(thread_id) {
        return;
    }
    let new = HOOKED.with_borrow_mut(|hooked| {
        if hooked.contains(&thread_id) {
            false
        } else {
            hooked.push(thread_id);
            true
        }
    });
    if !new {
        return;
    }
    // SAFETY: thread hooks on a thread of our own process with procedures in our own code
    // (`hmod` must be None for that); Windows removes them when that thread ends.
    let ok = unsafe {
        SetWindowsHookExW(WH_CALLWNDPROC, Some(call_wnd_proc), None, thread_id).is_ok()
            && SetWindowsHookExW(WH_GETMESSAGE, Some(get_msg_proc), None, thread_id).is_ok()
    };
    log::debug!("watching a shell thread's windows for WM_CLOSE (hooked: {ok})");
}

/// The main thread or a thread named `kuality-*` (they handle `WM_CLOSE` themselves).
fn is_kuality_thread(thread_id: u32) -> bool {
    if thread_id == super::tray::main_thread_id() {
        return true;
    }
    // SAFETY: opens a thread of our process for a name query; the handle is closed below.
    let Ok(handle) = (unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread_id) })
    else {
        return false;
    };
    // SAFETY: valid handle; the returned buffer is freed with LocalFree as documented.
    let name = unsafe {
        let name = GetThreadDescription(handle).ok().map(|p| {
            let s = p.to_string().unwrap_or_default();
            let _ = LocalFree(Some(HLOCAL(p.0.cast())));
            s
        });
        let _ = CloseHandle(handle);
        name
    };
    name.is_some_and(|n| n.starts_with("kuality-"))
}

/// Messages sent to the hooked thread's windows (before their window procedure).
unsafe extern "system" fn call_wnd_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for `code >= 0`, `lparam` points to a valid CWPSTRUCT for this call.
        let cwp = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        if cwp.message == WM_CLOSE {
            log::debug!("WM_CLOSE sent to a shell or action-thread window; quitting");
            super::tray::request_quit();
        }
    }
    // SAFETY: forwards the arguments we received.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Messages posted to the hooked thread, as its loop retrieves them.
unsafe extern "system" fn get_msg_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 == PM_REMOVE.0 as usize {
        // SAFETY: for `code >= 0`, `lparam` points to a valid MSG for this call.
        let msg = unsafe { &*(lparam.0 as *const MSG) };
        if msg.message == WM_CLOSE && !msg.hwnd.is_invalid() {
            log::debug!("WM_CLOSE posted to a shell window; quitting");
            super::tray::request_quit();
        }
    }
    // SAFETY: forwards the arguments we received.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
