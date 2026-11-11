//! Clipboard history (`docs/specs/clipboard.md`).
//!
//! While `clipboard.enabled`, [`ClipboardService`] runs two threads: the clipboard listener
//! (records every change into the [`History`], encrypted in memory) and the IPC pipe server
//! (the popup and the settings window list, paste, pin and delete entries). Pinned entries are
//! saved to `clipboard-pins.bin`, encrypted with DPAPI; nothing else touches the disk.

pub mod history;
pub mod image;
pub mod sealed;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kuality_protocol::clipboard::{
    EntryKind, PinRecord, Request, Response, THUMB_PX, decode_pins, encode_pins,
};
use kuality_protocol::{Clipboard, store};

use crate::platform::{self, ClipboardMonitor, IpcServer, MemoryCipher, Putter, Reply};
use history::{Added, Content, History, Skip};
use sealed::Plain;

/// How long stopping waits for each thread.
const STOP_TIMEOUT: Duration = Duration::from_secs(1);

type SharedHistory = Arc<Mutex<History<MemoryCipher>>>;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn lock(h: &SharedHistory) -> MutexGuard<'_, History<MemoryCipher>> {
    h.lock().unwrap_or_else(|e| e.into_inner())
}

/// The running clipboard history (exists only while the feature is enabled).
#[derive(Debug)]
pub struct ClipboardService {
    history: SharedHistory,
    images: Arc<AtomicBool>,
    monitor: Option<ClipboardMonitor>,
    server: Option<IpcServer>,
}

impl ClipboardService {
    /// Loads the pins and starts the listener and the pipe server.
    pub fn start(config: &Clipboard, pins_path: PathBuf) -> io::Result<Self> {
        let history: SharedHistory = Arc::new(Mutex::new(History::new(
            MemoryCipher,
            config.entries_limit() as usize,
        )));
        match load_pins(&pins_path) {
            Ok(pins) if pins.is_empty() => {}
            Ok(pins) => {
                log::info!("clipboard history: {} pinned entry(ies) loaded", pins.len());
                lock(&history).restore_pins(pins);
            }
            Err(e) => log::warn!(
                "cannot read {}: {e}; no pinned entries (the file is kept until pins change)",
                pins_path.display()
            ),
        }
        let images = Arc::new(AtomicBool::new(config.images));
        let on_content = {
            let history = Arc::clone(&history);
            Box::new(move |content: Content| {
                let kind = match &content {
                    Content::Text(_) => "text",
                    Content::Image { .. } => "image",
                };
                match lock(&history).add(content, now()) {
                    Added::New(id) => log::debug!("clipboard: recorded {kind} #{id}"),
                    Added::Moved(id) => log::debug!("clipboard: {kind} #{id} copied again"),
                    Added::Skipped(Skip::Empty) => {}
                    Added::Skipped(Skip::TooLarge) => {
                        log::info!("clipboard: {kind} too large; not recorded")
                    }
                    Added::Skipped(Skip::Cipher) => {
                        log::warn!("clipboard: encryption failed; {kind} not recorded")
                    }
                }
            })
        };
        let monitor = ClipboardMonitor::start(on_content, Arc::clone(&images))?;
        let handler = {
            let history = Arc::clone(&history);
            let putter = monitor.putter();
            Box::new(move |request| handle(&history, &putter, &pins_path, request))
        };
        let server = match IpcServer::start(handler) {
            Ok(s) => Some(s),
            Err(e) => {
                log::error!("cannot start the IPC pipe: {e}; the history popup cannot read it");
                None
            }
        };
        Ok(Self {
            history,
            images,
            monitor: Some(monitor),
            server,
        })
    }

    /// Applies a changed `clipboard` section (while it stays enabled).
    pub fn apply(&self, config: &Clipboard) {
        lock(&self.history).set_limit(config.entries_limit() as usize);
        self.images.store(config.images, Ordering::Relaxed);
    }

    /// Stops both threads and forgets the history (pins stay on disk).
    pub fn shutdown(&mut self) {
        if let Some(s) = self.server.take() {
            s.stop(STOP_TIMEOUT);
        }
        if let Some(m) = self.monitor.take() {
            m.stop(STOP_TIMEOUT);
        }
        let mut h = lock(&self.history);
        h.clear_unpinned();
        h.clear_pins();
    }
}

impl Drop for ClipboardService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Answers one pipe request.
fn handle(history: &SharedHistory, putter: &Putter, pins_path: &Path, request: Request) -> Reply {
    let reply = |response: Response| Reply {
        response,
        payload: Plain::default(),
        paste: false,
    };
    let not_found = || reply(Response::error("This entry is no longer in the history."));
    match request {
        Request::List => {
            let (entries, thumbs) = lock(history).list();
            Reply {
                response: Response {
                    ok: true,
                    error: None,
                    entries,
                },
                payload: thumbs,
                paste: false,
            }
        }
        Request::Get { id } => match lock(history).get(id) {
            Some((info, data)) => Reply {
                response: Response {
                    ok: true,
                    error: None,
                    entries: vec![info],
                },
                payload: data,
                paste: false,
            },
            None => not_found(),
        },
        Request::Paste { id } | Request::Copy { id } => {
            // Not under the lock: the clipboard thread records changes with it.
            let Some((info, data)) = lock(history).get(id) else {
                return not_found();
            };
            if !putter.put(info.kind, data) {
                return reply(Response::error("Could not put the entry on the clipboard."));
            }
            lock(history).touch(id, now());
            log::info!("clipboard: entry #{id} put on the clipboard");
            Reply {
                response: Response::ok(),
                payload: Plain::default(),
                paste: matches!(request, Request::Paste { .. }),
            }
        }
        Request::Pin { id } | Request::Unpin { id } => {
            let pinned = matches!(request, Request::Pin { .. });
            let changed = lock(history).set_pinned(id, pinned);
            if changed {
                save_pins(history, pins_path);
            }
            reply(Response::ok())
        }
        Request::Delete { id } => match lock(history).delete(id) {
            Some(was_pinned) => {
                if was_pinned {
                    save_pins(history, pins_path);
                }
                reply(Response::ok())
            }
            None => not_found(),
        },
        Request::Clear => {
            lock(history).clear_unpinned();
            log::info!("clipboard history cleared");
            reply(Response::ok())
        }
        Request::ClearPins => {
            if lock(history).clear_pins() {
                save_pins(history, pins_path);
            }
            log::info!("clipboard pins removed");
            reply(Response::ok())
        }
    }
}

/// Writes the pinned entries, encrypted for the current user.
fn save_pins(history: &SharedHistory, path: &Path) {
    let records = lock(history).pin_records();
    let count = records.len();
    let payload = Plain::new(encode_pins(
        records.iter().map(|(r, data)| (r.clone(), &data[..])),
    ));
    let written = platform::protect(&payload).and_then(|blob| store::write_atomic(path, &blob));
    match written {
        Ok(()) => log::info!("clipboard: {count} pinned entry(ies) saved"),
        Err(e) => log::error!("cannot save {}: {e}", path.display()),
    }
}

/// Reads and decrypts the pins file. Missing file: no pins.
fn load_pins(path: &Path) -> Result<Vec<(u64, Content)>, String> {
    let blob = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let plain = platform::unprotect(&blob).map_err(|e| format!("cannot decrypt it ({e})"))?;
    let records = decode_pins(&plain)?;
    let mut out = Vec::with_capacity(records.len());
    for (record, range) in records {
        let data = Plain::new(plain[range].to_vec());
        match pin_content(&record, data) {
            Ok(content) => out.push((record.created, content)),
            Err(e) => log::warn!("a pinned clipboard entry could not be read: {e}"),
        }
    }
    Ok(out)
}

fn pin_content(record: &PinRecord, data: Plain) -> Result<Content, image::ImageError> {
    Ok(match record.kind {
        EntryKind::Text => Content::Text(data),
        EntryKind::Image => {
            let img = image::decode_png(&data)?;
            let thumb = image::encode_png(&image::thumbnail(&img, THUMB_PX))?;
            Content::Image {
                png: data,
                thumb,
                width: img.width,
                height: img.height,
            }
        }
    })
}
