//! Windows-specific parts of the settings app: known folder, single instance, the link to the
//! core process and the key-recording hook. Everything else is portable.

use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_channel::oneshot;
use kuality_protocol::Key;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, GetCurrentThreadId, INFINITE, OpenMutexW, OpenProcess,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, FOLDERID_RoamingAppData, IEnumShellItems, IShellItem,
    IShellItemImageFactory, KF_FLAG_DEFAULT, SHGetKnownFolderItem, SHGetKnownFolderPath, SIGDN,
    SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING, SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, EnumWindows, FindWindowW, GetForegroundWindow, GetMessageW,
    GetWindowTextW, GetWindowThreadProcessId, HC_ACTION, IsIconic, IsWindowVisible,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PostQuitMessage, PostThreadMessageW, SW_RESTORE,
    SetForegroundWindow, SetTimer, SetWindowsHookExW, ShowWindow, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_SYSKEYDOWN, WM_TIMER,
};
use windows::core::{BOOL, Interface as _, PCWSTR, PWSTR, w};

use crate::apps::{AppInfo, ICON_PX, RawIcon, straight_alpha};
use crate::recorder::{Mode, Outcome, Recording};

/// Window title, also used by a second instance to find the first one.
pub const WINDOW_TITLE: &str = "Kuality Settings";
const INSTANCE_MUTEX: &str = r"Local\Kuality.Settings";
const CORE_MUTEX: &str = r"Local\Kuality.Core";
const EXE_NAME: &str = "kuality-settings.exe";
/// The core's exe, shipped next to this one.
pub const CORE_EXE_NAME: &str = "kuality.exe";
/// Class of the core's hidden main window (owns the tray icon).
const CORE_WINDOW_CLASS: PCWSTR = w!("Kuality.Core");

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `%APPDATA%` (roaming app data), or `None` if it cannot be resolved. Same as the core.
pub fn roaming_app_data_dir() -> Option<PathBuf> {
    // SAFETY: on success the returned PWSTR is a valid NUL-terminated string that we own and
    // free with CoTaskMemFree exactly once.
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, None).ok()?;
        let path = PathBuf::from(OsString::from_wide(p.as_wide()));
        CoTaskMemFree(Some(p.as_ptr() as *const _));
        Some(path)
    }
}

/// Holds the settings-app mutex for the process lifetime.
#[derive(Debug)]
pub struct Instance(HANDLE);

impl Instance {
    /// `None` if another settings window is already running.
    pub fn acquire() -> io::Result<Option<Self>> {
        let name = wide(INSTANCE_MUTEX);
        // SAFETY: `name` is a NUL-terminated UTF-16 string alive for the call.
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
            .map_err(io::Error::other)?;
        // SAFETY: reads the calling thread's last error, set by CreateMutexW above.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: `handle` is a valid handle we own.
            let _ = unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        Ok(Some(Self(handle)))
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this guard and closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Brings the first instance's window to the front. Retries for up to 2 s because the first
/// instance may still be opening its window. Returns whether a window was found.
pub fn activate_existing() -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(hwnd) = find_existing_window() {
            // SAFETY: plain calls on a window handle; a stale handle only makes them fail.
            unsafe {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(hwnd);
            }
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn find_existing_window() -> Option<HWND> {
    let mut found: Option<HWND> = None;
    // SAFETY: the callback only runs during EnumWindows and gets a pointer to `found`, which
    // outlives the call.
    let _ = unsafe {
        EnumWindows(
            Some(enum_proc),
            LPARAM(&mut found as *mut Option<HWND> as isize),
        )
    };
    found
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `&mut Option<HWND>` passed by `find_existing_window`.
    let found = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
    // SAFETY: plain query on a window handle given by EnumWindows.
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return true.into();
    }
    let mut buf = [0u16; 64];
    // SAFETY: `buf` is a valid buffer for the call.
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if String::from_utf16_lossy(&buf[..n.max(0) as usize]) != WINDOW_TITLE {
        return true.into();
    }
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    // SAFETY: plain call.
    if pid == unsafe { GetCurrentProcessId() } || !process_is_settings(pid) {
        return true.into();
    }
    *found = Some(hwnd);
    false.into()
}

fn process_is_settings(pid: u32) -> bool {
    // SAFETY: the handle is checked and closed once.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let ok = image_is(h, EXE_NAME);
        let _ = CloseHandle(h);
        ok
    }
}

/// Whether the process behind `h` (opened with at least query-limited rights) runs an exe named
/// `exe_name`.
fn image_is(h: HANDLE, exe_name: &str) -> bool {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` and `len` are valid for the call; `h` is a live process handle.
    let ok = unsafe {
        QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
    }
    .is_ok();
    ok && PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case(exe_name))
}

/// Whether the Kuality core is running in this session (its single-instance mutex exists).
pub fn core_running() -> bool {
    let name = wide(CORE_MUTEX);
    // SAFETY: `name` is NUL-terminated and alive for the call; the handle is closed at once.
    unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Link to the core: the settings window lives only as long as the core process.

/// The running core process, opened so we can wait for it to exit.
#[derive(Debug)]
pub struct CoreProcess {
    handle: HANDLE,
    pub pid: u32,
}

impl CoreProcess {
    /// Opens process `pid` if it is a running `kuality.exe` (guards against a reused pid).
    pub fn open(pid: u32) -> Option<Self> {
        // SAFETY: the handle is checked, and owned by the returned value or closed here.
        unsafe {
            let h = OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
            .ok()?;
            if image_is(h, CORE_EXE_NAME) {
                Some(Self { handle: h, pid })
            } else {
                let _ = CloseHandle(h);
                None
            }
        }
    }

    /// Waits for the process to exit on a background thread; the receiver fires then (for any
    /// exit: tray Quit, `taskkill`, crash).
    pub fn watch(self) -> io::Result<oneshot::Receiver<()>> {
        let (tx, rx) = oneshot::channel();
        // The thread takes over the handle; a raw value crosses the thread boundary.
        let raw = self.handle.0 as usize;
        std::mem::forget(self);
        std::thread::Builder::new()
            .name("kuality-settings-core-watch".into())
            .spawn(move || {
                let h = HANDLE(raw as *mut core::ffi::c_void);
                // SAFETY: `h` is the process handle opened in `open` and owned by this thread
                // from here on; it is closed once after the wait.
                unsafe {
                    WaitForSingleObject(h, INFINITE);
                    let _ = CloseHandle(h);
                }
                let _ = tx.send(());
            })?;
        Ok(rx)
    }
}

impl Drop for CoreProcess {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and closed once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// The running core of this session, found through its main window. Waits up to 3 s when the
/// core's mutex exists but its window is not created yet (core still starting).
pub fn find_core() -> Option<CoreProcess> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // SAFETY: plain lookup by a static class name.
        if let Ok(hwnd) = unsafe { FindWindowW(CORE_WINDOW_CLASS, PCWSTR::null()) } {
            let mut pid = 0u32;
            // SAFETY: `pid` is a valid out pointer.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if let Some(core) = CoreProcess::open(pid) {
                return Some(core);
            }
        }
        if !core_running() || Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Starts `kuality.exe` from `exe_dir` (it applies the same data-dir rule as this app).
pub fn start_core(exe_dir: &Path) -> io::Result<CoreProcess> {
    let exe = exe_dir.join(CORE_EXE_NAME);
    if !exe.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{CORE_EXE_NAME} was not found in {}", exe_dir.display()),
        ));
    }
    // The child handle is dropped: the core keeps running on its own.
    let child = std::process::Command::new(&exe)
        .current_dir(exe_dir)
        .spawn()?;
    CoreProcess::open(child.id())
        .ok_or_else(|| io::Error::other(format!("{CORE_EXE_NAME} exited right after starting")))
}

// ---------------------------------------------------------------------------------------------
// Key recording: a `WH_KEYBOARD_LL` hook on its own thread, alive only while recording (plus
// until the ups of swallowed keys arrive, at most 1 s).

/// Posted to the recorder thread to cancel.
const WM_APP_CANCEL: u32 = WM_APP + 1;
/// How long the hook stays after the recording ends, waiting for swallowed ups.
const SETTLE_MS: u32 = 1000;

struct HookState {
    recording: Recording,
    result: Option<oneshot::Sender<Outcome>>,
    our_pid: u32,
    settle_timer: bool,
}

thread_local! {
    static HOOK: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

/// A running recording. Dropping it cancels the recording.
#[derive(Debug)]
pub struct KeyRecorder {
    thread_id: u32,
}

impl KeyRecorder {
    /// Starts recording; the outcome arrives on the returned receiver.
    pub fn start(mode: Mode) -> io::Result<(Self, oneshot::Receiver<Outcome>)> {
        let (tx, rx) = oneshot::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("kuality-settings-recorder".into())
            .spawn(move || run_hook_thread(mode, tx, ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok((Self { thread_id }, rx)),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::other("recorder thread ended early")),
        }
    }

    /// Ends the recording without a result (if it is still running).
    pub fn cancel(&self) {
        // SAFETY: plain call; failure means the thread already ended.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_APP_CANCEL, WPARAM(0), LPARAM(0)) };
    }
}

impl Drop for KeyRecorder {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run_hook_thread(
    mode: Mode,
    result: oneshot::Sender<Outcome>,
    ready: std::sync::mpsc::Sender<io::Result<u32>>,
) {
    HOOK.with(|h| {
        *h.borrow_mut() = Some(HookState {
            recording: Recording::new(mode),
            result: Some(result),
            // SAFETY: plain call.
            our_pid: unsafe { GetCurrentProcessId() },
            settle_timer: false,
        })
    });
    // SAFETY: the hook procedure is a valid `extern "system"` fn for the process lifetime; the
    // module handle of our own exe is valid.
    let hook = unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).ok();
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module.map(Into::into), 0)
    };
    let hook = match hook {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(io::Error::other(e)));
            return;
        }
    };
    // SAFETY: plain call.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    let mut msg = MSG::default();
    // SAFETY: standard message loop on this thread's queue; `msg` is a valid out pointer.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            match msg.message {
                WM_APP_CANCEL => with_state(|s| {
                    if let Some(o) = s.recording.cancel() {
                        send(s, o);
                    }
                    after_end(s);
                }),
                WM_TIMER => break,
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        let _ = UnhookWindowsHookEx(hook);
    }
    HOOK.with(|h| h.borrow_mut().take());
}

fn with_state(f: impl FnOnce(&mut HookState)) {
    HOOK.with(|h| {
        if let Some(s) = h.borrow_mut().as_mut() {
            f(s);
        }
    });
}

fn send(s: &mut HookState, outcome: Outcome) {
    if let Some(tx) = s.result.take() {
        let _ = tx.send(outcome);
    }
}

/// Once the recording ended: quit when no swallowed up is pending, else wait at most 1 s.
fn after_end(s: &mut HookState) {
    if s.recording.is_active() {
        return;
    }
    if s.recording.is_settled() {
        // SAFETY: plain call on this thread.
        unsafe { PostQuitMessage(0) };
    } else if !s.settle_timer {
        s.settle_timer = true;
        // SAFETY: thread timer (no window); WM_TIMER ends the loop.
        unsafe { SetTimer(None, 0, SETTLE_MS, None) };
    }
}

fn foreground_pid() -> u32 {
    let mut pid = 0u32;
    // SAFETY: plain calls; `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
    pid
}

/// Low-level keyboard hook: feeds the state machine and returns at once.
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL with HC_ACTION, lparam points to a KBDLLHOOKSTRUCT.
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if (kb.flags.0 & LLKHF_INJECTED.0) == 0 {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let mut swallow = false;
            with_state(|s| {
                let fg = down && foreground_pid() == s.our_pid;
                let (sw, outcome) = s.recording.on_key(Key(kb.vkCode as u16), down, fg);
                swallow = sw;
                if let Some(o) = outcome {
                    send(s, o);
                }
                after_end(s);
            });
            if swallow {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: forwards the event to the next hook with the arguments we received.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

// ---------------------------------------------------------------------------------------------
// Installed apps: the shell's Applications folder (`shell:AppsFolder`), read on the
// `kuality-settings-apps` thread (see `apps.rs`).

/// Reads every app of the Applications folder and sends the list (in the order `order` gives),
/// then their icons in the same order. Runs on the calling thread; COM is initialised for it.
pub fn read_apps_folder(
    order: impl FnOnce(&[AppInfo]) -> Vec<usize>,
    list_tx: oneshot::Sender<Result<Vec<AppInfo>, String>>,
    icons_tx: oneshot::Sender<Vec<Option<RawIcon>>>,
) {
    // SAFETY: plain call; balanced by CoUninitialize below when it succeeded.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    {
        let items = match apps_folder_items() {
            Ok(items) => items,
            Err(e) => {
                log::warn!("cannot list the installed apps: {e}");
                let _ = list_tx.send(Err(e.message()));
                return;
            }
        };
        let infos: Vec<AppInfo> = items.iter().map(|(info, _)| info.clone()).collect();
        let order = order(&infos);
        log::debug!("{} apps in the Applications folder", order.len());
        let list = order.iter().map(|&i| infos[i].clone()).collect();
        if list_tx.send(Ok(list)).is_err() {
            return;
        }
        let icons = order.iter().map(|&i| item_icon(&items[i].1)).collect();
        let _ = icons_tx.send(icons);
    }
    if com.is_ok() {
        // SAFETY: matches the successful CoInitializeEx above; every COM object of this thread
        // was dropped at the end of the block.
        unsafe { CoUninitialize() };
    }
}

fn apps_folder_items() -> windows::core::Result<Vec<(AppInfo, IShellItem)>> {
    // SAFETY: plain COM calls on interfaces we own; out values are checked.
    unsafe {
        let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
        let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
        let mut out = Vec::new();
        loop {
            let mut batch: [Option<IShellItem>; 32] = Default::default();
            let mut fetched = 0u32;
            items.Next(&mut batch, Some(&mut fetched))?;
            if fetched == 0 {
                break;
            }
            for item in batch.into_iter().take(fetched as usize).flatten() {
                let name = display_name(&item, SIGDN_NORMALDISPLAY);
                let id = display_name(&item, SIGDN_PARENTRELATIVEPARSING);
                if let (Some(name), Some(id)) = (name, id) {
                    out.push((AppInfo { name, id }, item));
                }
            }
        }
        Ok(out)
    }
}

fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    // SAFETY: on success the PWSTR is a NUL-terminated string we own and free once.
    unsafe {
        let p = item.GetDisplayName(kind).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.as_ptr() as *const _));
        s
    }
}

/// The item's icon at [`ICON_PX`], straight-alpha BGRA.
fn item_icon(item: &IShellItem) -> Option<RawIcon> {
    // SAFETY: COM calls on a live item; the returned bitmap is owned here and deleted once.
    unsafe {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let size = SIZE {
            cx: ICON_PX as i32,
            cy: ICON_PX as i32,
        };
        let hbm = factory.GetImage(size, SIIGBF_ICONONLY).ok()?;
        let icon = bitmap_pixels(hbm);
        let _ = DeleteObject(hbm.into());
        icon
    }
}

fn bitmap_pixels(hbm: HBITMAP) -> Option<RawIcon> {
    let mut bm = BITMAP::default();
    // SAFETY: `bm` is a valid out buffer of the size given.
    let n = unsafe {
        GetObjectW(
            hbm.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut core::ffi::c_void),
        )
    };
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    if n == 0 || w <= 0 || h <= 0 || w > 256 || h > 256 {
        return None;
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Negative: rows top-down.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    // SAFETY: `bgra` holds exactly `h` rows of `w` 32-bit pixels as described by `info`; the
    // memory DC is created and deleted here.
    let lines = unsafe {
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(
            dc,
            hbm,
            0,
            h as u32,
            Some(bgra.as_mut_ptr() as *mut core::ffi::c_void),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        lines
    };
    if lines != h {
        return None;
    }
    straight_alpha(&mut bgra);
    Some(RawIcon {
        width: w as u32,
        height: h as u32,
        bgra,
    })
}
