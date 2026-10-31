//! Tray icon and the main-thread message loop.
//!
//! A hidden *top-level* window owns the icon. It must not be a message-only window: those do
//! not receive the broadcast `TaskbarCreated` message used to re-add the icon after Explorer
//! restarts.
//!
//! The window procedure never calls the app handler directly: it queues [`Event`]s, and the
//! message loop delivers them after `DispatchMessageW` returns. This keeps the handler free of
//! re-entrancy (e.g. while `TrackPopupMenu` runs its own modal loop).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use ::windows::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    SetConsoleCtrlHandler,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, ChangeWindowMessageFilterEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, IDI_APPLICATION,
    LoadIconW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, MSGFLT_ALLOW, PostMessageW,
    RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_CLOSE,
    WM_CONTEXTMENU, WM_ENDSESSION, WM_NULL, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use ::windows::core::{BOOL, PCWSTR, w};

use crate::platform::{Event, TrayIcon, Waker};

const CLASS_NAME: PCWSTR = w!("Kuality.Core");
const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_WAKE: u32 = WM_APP + 2;
/// Another thread asks for the Settings action (widget context menu).
const WM_APP_OPEN_SETTINGS: u32 = WM_APP + 3;
const TRAY_ICON_ID: u32 = 1;
/// `NIN_SELECT | NINF_KEY`: icon activated with the keyboard (missing from the `windows` crate).
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const MENU_SETTINGS: usize = 1;
const MENU_QUIT: usize = 2;
const MENU_PAUSE: usize = 3;

/// The hidden window, for the console Ctrl+C handler (runs on another thread).
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

type Handler = Box<dyn FnMut(Event) -> ControlFlow<()>>;

thread_local! {
    static PENDING: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
    static EXIT: Cell<bool> = const { Cell::new(false) };
    static TOOLTIP: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
    static PAUSED: Cell<bool> = const { Cell::new(false) };
}

/// Updates the Pause check mark and the tooltip (`<tooltip> (paused)`). Call on the tray thread.
pub fn set_tray_paused(paused: bool) {
    PAUSED.set(paused);
    let hwnd = MAIN_HWND.load(Ordering::Relaxed);
    if hwnd == 0 {
        return;
    }
    let mut nid = icon_data(HWND(hwnd as *mut _));
    nid.uFlags = NIF_TIP | NIF_SHOWTIP;
    fill_tooltip(&mut nid);
    // SAFETY: `nid` identifies our icon and is alive for the call.
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

/// Copies the tooltip (with the paused suffix if needed) into `nid.szTip`.
fn fill_tooltip(nid: &mut NOTIFYICONDATAW) {
    let suffix = if PAUSED.get() { " (paused)" } else { "" };
    TOOLTIP.with_borrow(|t| {
        let max = nid.szTip.len() - 1;
        let text = t.iter().copied().chain(suffix.encode_utf16()).take(max);
        let mut n = 0;
        for (slot, c) in nid.szTip.iter_mut().zip(text) {
            *slot = c;
            n += 1;
        }
        nid.szTip[n] = 0;
    });
}

/// Owner window of the tray icon. Lives on the thread that created it.
#[derive(Debug)]
pub struct Tray {
    hwnd: HWND,
}

/// Posts a wake-up message to the tray window; usable from any thread.
#[derive(Debug, Clone, Copy)]
pub struct TrayWaker(isize);

impl Waker for TrayWaker {
    fn wake(&self) {
        // SAFETY: PostMessageW is thread-safe; a stale HWND only makes the call fail.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.0 as *mut _)),
                WM_APP_WAKE,
                WPARAM(0),
                LPARAM(0),
            )
        };
    }
}

impl TrayIcon for Tray {
    type Waker = TrayWaker;

    fn create(tooltip: &str) -> io::Result<Self> {
        TOOLTIP.with_borrow_mut(|t| *t = tooltip.encode_utf16().collect());
        // SAFETY: standard window class registration and creation; all pointers are static
        // strings or locals alive for the calls.
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
            TASKBAR_CREATED.store(
                RegisterWindowMessageW(w!("TaskbarCreated")),
                Ordering::Relaxed,
            );

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
                CLASS_NAME,
                w!("Kuality"),
                WS_OVERLAPPED,
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
            MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
            let _ = SetConsoleCtrlHandler(Some(console_ctrl_handler), true);

            match add_icon(hwnd) {
                Ok(()) => log::debug!("tray icon added"),
                // Explorer may not be running yet; the icon is added on `TaskbarCreated`.
                Err(e) => log::warn!("could not add tray icon yet: {e}"),
            }
            Ok(Self { hwnd })
        }
    }

    fn waker(&self) -> TrayWaker {
        TrayWaker(self.hwnd.0 as isize)
    }

    fn run(self, handler: impl FnMut(Event) -> ControlFlow<()> + 'static) {
        HANDLER.with_borrow_mut(|h| *h = Some(Box::new(handler)));
        EXIT.set(false);
        let mut msg = MSG::default();
        // SAFETY: standard message loop on the thread that owns the window.
        unsafe {
            loop {
                let r = GetMessageW(&mut msg, None, 0, 0);
                if r.0 == 0 || r.0 == -1 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if deliver_pending() {
                    break;
                }
            }
            remove_icon(self.hwnd);
            MAIN_HWND.store(0, Ordering::Relaxed);
            let _ = DestroyWindow(self.hwnd);
        }
        HANDLER.with_borrow_mut(|h| *h = None);
    }
}

/// Delivers queued events to the handler. Returns `true` once the handler asked to exit.
fn deliver_pending() -> bool {
    if EXIT.get() {
        return true;
    }
    // Take the handler out so a nested call (if any) finds it missing and leaves events queued.
    let Some(mut handler) = HANDLER.with_borrow_mut(Option::take) else {
        return false;
    };
    while let Some(event) = PENDING.with_borrow_mut(VecDeque::pop_front) {
        if handler(event).is_break() {
            EXIT.set(true);
            break;
        }
    }
    HANDLER.with_borrow_mut(|h| *h = Some(handler));
    EXIT.get()
}

fn push(event: Event) {
    PENDING.with_borrow_mut(|q| q.push_back(event));
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        ..Default::default()
    }
}

fn add_icon(hwnd: HWND) -> io::Result<()> {
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_APP_TRAY;
    // SAFETY: loads a shared stock icon; it must not be destroyed.
    nid.hIcon = unsafe { LoadIconW(None, IDI_APPLICATION) }.map_err(io::Error::other)?;
    fill_tooltip(&mut nid);
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    // SAFETY: `nid` is fully initialized and alive for the calls.
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            return Err(io::Error::other("Shell_NotifyIconW(NIM_ADD) failed"));
        }
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
    Ok(())
}

fn remove_icon(hwnd: HWND) {
    let nid = icon_data(hwnd);
    // SAFETY: `nid` identifies our icon; removing a missing icon is harmless.
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
}

/// Shows the tray menu at `(x, y)` and returns the chosen command, if any.
fn show_menu(hwnd: HWND, x: i32, y: i32) -> Option<Event> {
    // SAFETY: creates, shows and destroys a popup menu owned by this function.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("Settings"));
        let check = if PAUSED.get() {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        let _ = AppendMenuW(menu, MF_STRING | check, MENU_PAUSE, w!("Pause"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));
        // Needed so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            x,
            y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            MENU_SETTINGS => Some(Event::OpenSettings),
            MENU_PAUSE => Some(Event::TogglePause),
            MENU_QUIT => Some(Event::Quit),
            _ => None,
        }
    }
}

/// Minimum time between two "open Settings" activations of the icon.
const ACTIVATION_DEBOUNCE: Duration = Duration::from_millis(500);

/// Whether an icon activation follows another one too closely (the shell can report one
/// keyboard activation as two notifications). Main thread only.
fn repeated_activation() -> bool {
    thread_local! {
        static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
    }
    let now = Instant::now();
    LAST.with(|last| {
        let repeated = last
            .get()
            .is_some_and(|t| now.duration_since(t) < ACTIVATION_DEBOUNCE);
        last.set(Some(now));
        repeated
    })
}

/// Signed 16-bit coordinates packed in a `WPARAM` (NOTIFYICON_VERSION_4 anchor point).
fn point_from_wparam(wparam: WPARAM) -> (i32, i32) {
    let x = (wparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            // Left click (NIN_SELECT) and Enter/Space on the focused icon (NIN_KEYSELECT) open
            // Settings; right click / Shift+F10 / menu key (WM_CONTEXTMENU) show the menu.
            if matches!(event, NIN_SELECT | NIN_KEYSELECT) {
                if !repeated_activation() {
                    push(Event::OpenSettings);
                    // Explorer *sends* this callback, so it runs inside GetMessageW and no
                    // DispatchMessageW follows: post a no-op so the loop delivers the event now.
                    // SAFETY: posting to our own window.
                    let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
                }
            } else if event == WM_CONTEXTMENU {
                let (mut x, mut y) = point_from_wparam(wparam);
                if x == 0 && y == 0 {
                    let mut p = POINT::default();
                    // SAFETY: writes into a local POINT.
                    if unsafe { GetCursorPos(&mut p) }.is_ok() {
                        (x, y) = (p.x, p.y);
                    }
                }
                if let Some(cmd) = show_menu(hwnd, x, y) {
                    push(cmd);
                }
            }
            LRESULT(0)
        }
        WM_APP_OPEN_SETTINGS => {
            push(Event::OpenSettings);
            LRESULT(0)
        }
        WM_APP_WAKE => {
            push(Event::Wake);
            LRESULT(0)
        }
        WM_CLOSE => {
            push(Event::Quit);
            LRESULT(0)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                // The process may be terminated as soon as we return: shut down synchronously.
                log::info!("session is ending");
                push(Event::Quit);
                if deliver_pending() {
                    remove_icon(hwnd);
                }
            }
            LRESULT(0)
        }
        _ if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            log::info!("taskbar (re)created, re-adding tray icon");
            // Remove first in case the icon survived (e.g. the message was not from a restart).
            remove_icon(hwnd);
            if let Err(e) = add_icon(hwnd) {
                log::warn!("could not re-add tray icon: {e}");
            }
            LRESULT(0)
        }
        // SAFETY: default handling with the arguments we received.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Asks the main loop to run the Settings action (same as the tray item). Any thread.
pub(crate) fn request_open_settings() {
    let hwnd = MAIN_HWND.load(Ordering::Relaxed);
    if hwnd != 0 {
        // SAFETY: PostMessageW is thread-safe.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(hwnd as *mut _)),
                WM_APP_OPEN_SETTINGS,
                WPARAM(0),
                LPARAM(0),
            )
        };
    }
}

/// Asks the main loop to quit cleanly (as if `WM_CLOSE` reached the tray window). Any thread.
pub(crate) fn request_quit() {
    let hwnd = MAIN_HWND.load(Ordering::Relaxed);
    if hwnd != 0 {
        // SAFETY: PostMessageW is thread-safe.
        let _ = unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
}

/// Id of the thread that owns the main window (0 if it does not exist). Any thread.
pub(crate) fn main_thread_id() -> u32 {
    let hwnd = MAIN_HWND.load(Ordering::Relaxed);
    if hwnd == 0 {
        return 0;
    }
    // SAFETY: plain window query; a stale HWND only returns 0.
    unsafe {
        ::windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            HWND(hwnd as *mut _),
            None,
        )
    }
}

/// Debug console events → clean shutdown through `WM_CLOSE`.
///
/// Ctrl+C / Ctrl+Break: the handler returns and the main loop quits. Console closed (e.g.
/// `taskkill` without `/F` picked the console window, which reports our process as its owner),
/// logoff or shutdown: Windows terminates the process as soon as the handler returns, so it
/// blocks until `main` finishes and exits the process (Windows allows about 5 s).
unsafe extern "system" fn console_ctrl_handler(ctrl_type: u32) -> BOOL {
    let hwnd = MAIN_HWND.load(Ordering::Relaxed);
    if hwnd == 0 {
        return false.into();
    }
    let post_close = || {
        // SAFETY: PostMessageW is thread-safe.
        let _ = unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    };
    match ctrl_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT => {
            post_close();
            true.into()
        }
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            post_close();
            std::thread::sleep(std::time::Duration::from_secs(10));
            true.into()
        }
        _ => false.into(),
    }
}
