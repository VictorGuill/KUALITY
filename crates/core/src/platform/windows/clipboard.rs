//! Clipboard history, Windows side (`docs/specs/clipboard.md`):
//! - [`ClipboardMonitor`]: thread `kuality-clipboard` with a message-only window registered with
//!   `AddClipboardFormatListener`. It reads every change (text, else an image), skips contents
//!   marked private, and also writes entries back to the clipboard ([`Putter`]).
//! - [`MemoryCipher`]: `CryptProtectMemory` (key only valid inside this process).
//! - [`protect`] / [`unprotect`]: DPAPI for `clipboard-pins.bin` (current user).
//! - The window to paste into, remembered when the hotkey fires.

use std::cell::RefCell;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HLOCAL, HWND, LPARAM, LRESULT, LocalFree, WPARAM,
};
use ::windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CRYPTPROTECTMEMORY_SAME_PROCESS,
    CryptProtectData, CryptProtectMemory, CryptUnprotectData, CryptUnprotectMemory,
};
use ::windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use ::windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_UNICODETEXT};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    GetMessageW, HWND_MESSAGE, IsWindow, KillTimer, MSG, PostMessageW, PostQuitMessage,
    RegisterClassExW, SetForegroundWindow, SetTimer, TranslateMessage, WINDOW_EX_STYLE, WM_APP,
    WM_CLIPBOARDUPDATE, WM_TIMER, WNDCLASSEXW, WS_POPUP,
};
use ::windows::core::{PCWSTR, w};
use kuality_protocol::clipboard::{EntryKind, THUMB_PX};

use super::input;
use crate::clipboard::history::{Content, MAX_TEXT_BYTES};
use crate::clipboard::image::{self, Rgba};
use crate::clipboard::sealed::{Cipher, Plain, wipe};
use crate::platform::Foreground;

const CLASS_NAME: PCWSTR = w!("KualityClipboard");
/// Jobs are waiting in the channel.
const WM_APP_PUT: u32 = WM_APP + 1;
const WM_APP_STOP: u32 = WM_APP + 2;
const TIMER_RETRY: usize = 1;
/// Reading retries while another app holds the clipboard: 50, 100, 200, 400 ms.
const RETRIES: u32 = 4;
/// How long a write waits for the clipboard thread.
const PUT_TIMEOUT: Duration = Duration::from_secs(3);
/// DPAPI entropy for the pins file (not a secret; it ties the blob to this use).
const PINS_ENTROPY: &[u8] = b"Kuality.ClipboardPins.v1";

// ---------------------------------------------------------------------------------------------
// Encryption.

/// `CryptProtectMemory` with `CRYPTPROTECTMEMORY_SAME_PROCESS`: the data can only be decrypted
/// by this process.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryCipher;

impl Cipher for MemoryCipher {
    fn encrypt(&self, buf: &mut [u8]) -> bool {
        // SAFETY: `buf` is a valid writable buffer of the given length (a multiple of 16).
        unsafe {
            CryptProtectMemory(
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                CRYPTPROTECTMEMORY_SAME_PROCESS,
            )
        }
        .is_ok()
    }

    fn decrypt(&self, buf: &mut [u8]) -> bool {
        // SAFETY: as above.
        unsafe {
            CryptUnprotectMemory(
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                CRYPTPROTECTMEMORY_SAME_PROCESS,
            )
        }
        .is_ok()
    }
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Encrypts `data` for the current Windows user (DPAPI).
pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = blob(data);
    let entropy = blob(PINS_ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the blobs point to live buffers for the call; `out` is freed below with LocalFree.
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(io::Error::other)?;
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        Ok(bytes)
    }
}

/// Decrypts a [`protect`] blob (fails for another user or PC).
pub fn unprotect(data: &[u8]) -> io::Result<Plain> {
    let input = blob(data);
    let entropy = blob(PINS_ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`; the plain output is copied, then wiped before it is freed.
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(io::Error::other)?;
        let plain = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let bytes = Plain::new(plain.to_vec());
        wipe(plain);
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        Ok(bytes)
    }
}

// ---------------------------------------------------------------------------------------------
// Paste target.

/// The window in front when the clipboard hotkey fired.
static PASTE_TARGET: AtomicIsize = AtomicIsize::new(0);

/// Remembers the window to paste into (called when the hotkey opens the popup).
pub fn set_paste_target(window: Foreground) {
    PASTE_TARGET.store(window.0, Ordering::SeqCst);
}

/// Gives the focus back to the remembered window. Returns whether it is in front.
pub fn restore_paste_target() -> bool {
    let hwnd = HWND(PASTE_TARGET.load(Ordering::SeqCst) as *mut _);
    // SAFETY: plain window queries / calls; a stale handle only makes them fail.
    unsafe {
        if hwnd.is_invalid() || !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        if GetForegroundWindow() == hwnd {
            return true;
        }
        // Makes this process the last input source, so Windows lets it set the foreground.
        input::claim_last_input();
        let _ = SetForegroundWindow(hwnd);
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if GetForegroundWindow() == hwnd {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        GetForegroundWindow() == hwnd
    }
}

// ---------------------------------------------------------------------------------------------
// Listener thread.

/// Called with each new clipboard content (on the clipboard thread).
pub type OnContent = Box<dyn FnMut(Content) + Send>;

/// One write to the clipboard, done by the clipboard thread (it owns the window the clipboard
/// needs as owner).
struct PutJob {
    kind: EntryKind,
    data: Plain,
    done: Sender<bool>,
}

/// Puts entries on the clipboard from any thread.
#[derive(Clone)]
pub struct Putter {
    tx: Sender<PutJob>,
    hwnd: isize,
}

impl std::fmt::Debug for Putter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Putter")
    }
}

impl Putter {
    /// Puts text (UTF-8) or a PNG on the clipboard. Returns whether it worked.
    pub fn put(&self, kind: EntryKind, data: Plain) -> bool {
        let (done, result) = mpsc::channel();
        if self.tx.send(PutJob { kind, data, done }).is_err() {
            return false;
        }
        // SAFETY: posting is thread-safe; a stale handle only makes it fail.
        let posted = unsafe {
            PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_APP_PUT,
                WPARAM(0),
                LPARAM(0),
            )
        };
        posted.is_ok() && result.recv_timeout(PUT_TIMEOUT).unwrap_or(false)
    }
}

/// Handle to the clipboard thread.
pub struct ClipboardMonitor {
    hwnd: isize,
    thread: JoinHandle<()>,
    putter: Putter,
}

impl std::fmt::Debug for ClipboardMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClipboardMonitor")
    }
}

impl ClipboardMonitor {
    /// Starts listening. `images`: whether images are recorded (read on every change).
    pub fn start(on_content: OnContent, images: Arc<AtomicBool>) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<io::Result<isize>>(1);
        let thread = std::thread::Builder::new()
            .name("kuality-clipboard".into())
            .spawn(move || thread_main(rx, ready_tx, on_content, images))?;
        let hwnd = ready_rx
            .recv()
            .map_err(|_| io::Error::other("clipboard thread exited during startup"))??;
        Ok(Self {
            hwnd,
            thread,
            putter: Putter { tx, hwnd },
        })
    }

    pub fn putter(&self) -> Putter {
        self.putter.clone()
    }

    /// Stops the thread (waits up to `timeout`).
    pub fn stop(self, timeout: Duration) {
        // SAFETY: posting is thread-safe; a stale handle only makes it fail.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_APP_STOP,
                WPARAM(0),
                LPARAM(0),
            )
        };
        let deadline = Instant::now() + timeout;
        while !self.thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
        } else {
            log::warn!("clipboard thread did not stop within {timeout:?}");
        }
    }
}

/// Registered clipboard formats we look at.
struct Formats {
    /// Set by apps that do not want monitors to record the content (password managers).
    exclude: u32,
    /// Older equivalent of `exclude`.
    viewer_ignore: u32,
    /// DWORD; 0 = keep out of clipboard history.
    can_include: u32,
    png: u32,
}

struct Ctx {
    hwnd: HWND,
    rx: Receiver<PutJob>,
    on_content: OnContent,
    images: Arc<AtomicBool>,
    formats: Formats,
    /// Clipboard sequence number of the last change handled.
    last_seq: u32,
    /// Retry count of the change waiting for the clipboard to be free.
    attempt: u32,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

fn register(name: &str) -> u32 {
    let wide = super::wide(name);
    // SAFETY: `wide` is NUL-terminated and alive for the call.
    unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) }
}

fn thread_main(
    rx: Receiver<PutJob>,
    ready: mpsc::SyncSender<io::Result<isize>>,
    on_content: OnContent,
    images: Arc<AtomicBool>,
) {
    let hwnd = match create_window() {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // SAFETY: `hwnd` is our window.
    if let Err(e) = unsafe { AddClipboardFormatListener(hwnd) } {
        // SAFETY: our window, destroyed once.
        let _ = unsafe { DestroyWindow(hwnd) };
        let _ = ready.send(Err(io::Error::other(e)));
        return;
    }
    let formats = Formats {
        exclude: register("ExcludeClipboardContentFromMonitorProcessing"),
        viewer_ignore: register("Clipboard Viewer Ignore"),
        can_include: register("CanIncludeInClipboardHistory"),
        png: register("PNG"),
    };
    CTX.set(Some(Ctx {
        hwnd,
        rx,
        on_content,
        images,
        formats,
        // SAFETY: no arguments. What is on the clipboard at start is not recorded.
        last_seq: unsafe { GetClipboardSequenceNumber() },
        attempt: 0,
    }));
    let _ = ready.send(Ok(hwnd.0 as isize));
    log::info!("clipboard history: listening");
    let mut msg = MSG::default();
    // SAFETY: standard message loop on the thread that owns the window.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    CTX.set(None);
    log::info!("clipboard history: stopped");
}

fn create_window() -> io::Result<HWND> {
    // SAFETY: standard class registration (ignored if it exists from a previous start) and
    // creation of a message-only window with static strings.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(io::Error::other)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        let _ = RegisterClassExW(&class);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("Kuality clipboard"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(io::Error::other)
    }
}

fn with_ctx<R>(f: impl FnOnce(&mut Ctx) -> R) -> Option<R> {
    CTX.with(|c| c.try_borrow_mut().ok()?.as_mut().map(f))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            with_ctx(|c| {
                c.attempt = 0;
                capture(c);
            });
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_RETRY => {
            // SAFETY: our timer on our window.
            let _ = unsafe { KillTimer(Some(hwnd), TIMER_RETRY) };
            with_ctx(|c| {
                c.attempt += 1;
                capture(c);
            });
            LRESULT(0)
        }
        WM_APP_PUT => {
            while let Some(Ok(job)) = with_ctx(|c| c.rx.try_recv()) {
                let ok = with_ctx(|c| put(c, &job)).unwrap_or(false);
                let _ = job.done.send(ok);
            }
            LRESULT(0)
        }
        WM_APP_STOP => {
            // SAFETY: our window; the listener was added in `thread_main`.
            unsafe {
                let _ = RemoveClipboardFormatListener(hwnd);
                let _ = DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        // SAFETY: default handling with the arguments we received.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// What was read while the clipboard was open (copied out, so it can be closed at once).
enum Raw {
    Text(Plain),
    Png(Plain),
    Dib(Plain),
}

fn available(format: u32) -> bool {
    // SAFETY: plain query, no pointers.
    unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

/// Reads the current clipboard change, if new and not private, and reports it.
fn capture(c: &mut Ctx) {
    // SAFETY: plain queries, no pointers.
    let (seq, owner) = unsafe { (GetClipboardSequenceNumber(), GetClipboardOwner()) };
    if seq == c.last_seq {
        return;
    }
    if owner.is_ok_and(|o| o == c.hwnd) {
        // Our own write (paste / copy from the history): the entry was already moved up.
        c.last_seq = seq;
        return;
    }
    if available(c.formats.exclude) || available(c.formats.viewer_ignore) {
        c.last_seq = seq;
        log::debug!("clipboard change marked private; not recorded");
        return;
    }
    // SAFETY: opens the clipboard with our window as owner; closed below on every path.
    if unsafe { OpenClipboard(Some(c.hwnd)) }.is_err() {
        if c.attempt < RETRIES {
            // SAFETY: a timer on our own window.
            unsafe { SetTimer(Some(c.hwnd), TIMER_RETRY, 50 << c.attempt, None) };
        } else {
            c.last_seq = seq;
            log::debug!("clipboard busy; change not recorded");
        }
        return;
    }
    let raw = read_open(c);
    // SAFETY: we opened it above.
    let _ = unsafe { CloseClipboard() };
    c.last_seq = seq;
    let content = match raw {
        Ok(Some(raw)) => convert(raw),
        Ok(None) => return,
        Err(reason) => {
            log::info!("clipboard change not recorded: {reason}");
            return;
        }
    };
    match content {
        Ok(content) => (c.on_content)(content),
        Err(e) => log::info!("clipboard image not recorded: {e}"),
    }
}

/// Reads the data while the clipboard is open. `Ok(None)`: nothing to record.
fn read_open(c: &Ctx) -> Result<Option<Raw>, &'static str> {
    if available(c.formats.can_include)
        && let Some(v) = global_bytes(c.formats.can_include, 4)
        && v.len() >= 4
        && u32::from_le_bytes([v[0], v[1], v[2], v[3]]) == 0
    {
        log::debug!("clipboard change excluded from history by its app");
        return Ok(None);
    }
    if available(u32::from(CF_UNICODETEXT.0)) {
        return read_text().map(|t| t.map(Raw::Text));
    }
    if !c.images.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if available(c.formats.png)
        && let Some(png) = global_bytes(c.formats.png, usize::MAX)
    {
        return Ok(Some(Raw::Png(png)));
    }
    for format in [CF_DIBV5, CF_DIB] {
        if available(u32::from(format.0))
            && let Some(dib) = global_bytes(u32::from(format.0), usize::MAX)
        {
            return Ok(Some(Raw::Dib(dib)));
        }
    }
    Ok(None)
}

/// Locks the clipboard data of `format` and runs `f` on its bytes.
fn with_global<R>(format: u32, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
    // SAFETY: the clipboard is open; the handle belongs to the clipboard and stays valid until
    // it is closed. The slice is only used while the memory is locked.
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        let global = HGLOBAL(handle.0);
        let size = GlobalSize(global);
        let ptr = GlobalLock(global) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let result = f(std::slice::from_raw_parts(ptr, size));
        let _ = GlobalUnlock(global);
        Some(result)
    }
}

/// A copy of at most `max` bytes of the clipboard data of `format`.
fn global_bytes(format: u32, max: usize) -> Option<Plain> {
    with_global(format, |b| Plain::new(b[..b.len().min(max)].to_vec()))
}

fn read_text() -> Result<Option<Plain>, &'static str> {
    with_global(u32::from(CF_UNICODETEXT.0), |bytes| {
        let units = bytes.len() / 2;
        // UTF-16 data: up to the first NUL. Each unit is at least one UTF-8 byte.
        // SAFETY: GlobalLock memory is suitably aligned for u16, `units` fit in `bytes`.
        let wide = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, units) };
        let len = wide.iter().position(|&u| u == 0).unwrap_or(units);
        if len > MAX_TEXT_BYTES {
            return Err("text too large");
        }
        Ok(Some(Plain::from_string(String::from_utf16_lossy(
            &wide[..len],
        ))))
    })
    .unwrap_or(Ok(None))
}

fn convert(raw: Raw) -> Result<Content, image::ImageError> {
    let (png, img) = match raw {
        Raw::Text(t) => return Ok(Content::Text(t)),
        Raw::Png(png) => {
            let img = image::decode_png(&png)?;
            (png, img)
        }
        Raw::Dib(dib) => {
            let img = image::from_dib(&dib)?;
            (image::encode_png(&img)?, img)
        }
    };
    let thumb = image::encode_png(&image::thumbnail(&img, THUMB_PX))?;
    Ok(Content::Image {
        png,
        thumb,
        width: img.width,
        height: img.height,
    })
}

// ---------------------------------------------------------------------------------------------
// Writing.

/// Puts the job's data on the clipboard (we become its owner, so the listener skips it).
fn put(c: &Ctx, job: &PutJob) -> bool {
    // Prepare everything before opening the clipboard (holding it blocks other apps).
    let formats: Vec<(u32, Plain)> = match job.kind {
        EntryKind::Text => {
            let Some(text) = job.data.as_str() else {
                return false;
            };
            let mut wide: Vec<u8> = Vec::with_capacity(text.len() * 2 + 2);
            for u in text.encode_utf16().chain(std::iter::once(0)) {
                wide.extend_from_slice(&u.to_le_bytes());
            }
            vec![(u32::from(CF_UNICODETEXT.0), Plain::new(wide))]
        }
        EntryKind::Image => {
            let img: Rgba = match image::decode_png(&job.data) {
                Ok(i) => i,
                Err(e) => {
                    log::warn!("cannot decode a stored image: {e}");
                    return false;
                }
            };
            vec![
                (u32::from(CF_DIB.0), image::to_dib(&img)),
                (c.formats.png, Plain::new(job.data.to_vec())),
            ]
        }
    };
    let mut opened = false;
    for _ in 0..10 {
        // SAFETY: opens the clipboard with our window as owner; closed below.
        if unsafe { OpenClipboard(Some(c.hwnd)) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !opened {
        log::warn!("clipboard busy; entry not put on it");
        return false;
    }
    // SAFETY: the clipboard is open by us.
    let mut ok = unsafe { EmptyClipboard() }.is_ok();
    for (format, bytes) in &formats {
        ok &= set_global(*format, bytes);
    }
    // SAFETY: we opened it above.
    let _ = unsafe { CloseClipboard() };
    ok
}

/// Copies `bytes` into a movable global block and hands it to the open clipboard.
fn set_global(format: u32, bytes: &[u8]) -> bool {
    // SAFETY: the block is allocated with the right size, filled while locked, and either owned
    // by the clipboard after SetClipboardData or freed here.
    unsafe {
        let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else {
            return false;
        };
        let ptr = GlobalLock(global) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(global);
        if SetClipboardData(format, Some(HANDLE(global.0))).is_err() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::sealed::Sealed;

    #[test]
    fn memory_cipher_round_trip() {
        let data = b"secret clipboard text";
        let sealed = Sealed::seal(&MemoryCipher, data).unwrap();
        assert_eq!(&*sealed.open(&MemoryCipher).unwrap(), data);
    }

    #[test]
    fn dpapi_round_trip_and_tampering() {
        let blob = protect(b"pinned entry").unwrap();
        assert!(!blob.windows(6).any(|w| w == b"pinned"), "stored encrypted");
        assert_eq!(&*unprotect(&blob).unwrap(), b"pinned entry");
        let mut bad = blob.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert!(unprotect(&bad).is_err());
        assert!(unprotect(b"not a blob").is_err());
    }
}
