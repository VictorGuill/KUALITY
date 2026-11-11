//! Named pipe server for the clipboard popup and the settings window
//! (`\\.\pipe\Kuality.Core.<pid>`, `docs/specs/clipboard.md`).
//!
//! Thread `kuality-ipc` serves one connection at a time, one request per connection. Only the
//! current user can open the pipe (explicit DACL), remote clients are rejected, and a client
//! whose process is not our own exe is disconnected before anything is read.
//!
//! All pipe I/O is overlapped: waiting for a client costs nothing, every read and write has a
//! deadline (a client that hangs never blocks the pipe for the next one) and the stop event
//! ends any wait at once. The next pipe instance is created before the current one is closed,
//! so the name exists for as long as the server runs.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0,
};
use ::windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use ::windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use ::windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use ::windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use ::windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use ::windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};
use ::windows::core::{PCWSTR, PWSTR};
use kuality_protocol::clipboard::{Request, Response};
use kuality_protocol::ipc;

use super::SendHandle;
use crate::clipboard::sealed::Plain;

/// Buffer sizes of the pipe (the system grows them as needed).
const BUFFER: u32 = 64 * 1024;
/// How long a paste waits for the popup to exit before giving the focus back.
const CLIENT_EXIT_WAIT: Duration = Duration::from_millis(1500);
/// Largest request accepted: requests are a few dozen bytes of JSON, without payload.
const MAX_REQUEST: usize = 4096;
/// Pause before trying again after the pipe could not be created or connected.
const RETRY_MS: u32 = 1000;

/// How long the server waits for a client during one exchange.
#[derive(Debug, Clone, Copy)]
struct Timeouts {
    /// For the request to arrive once the client is connected.
    request: Duration,
    /// For the client to read the whole reply and close its end.
    reply: Duration,
}

const TIMEOUTS: Timeouts = Timeouts {
    request: Duration::from_secs(2),
    reply: Duration::from_secs(10),
};

/// The answer to one request.
#[derive(Debug)]
pub struct Reply {
    pub response: Response,
    pub payload: Plain,
    /// After replying: wait for the client to exit, give the focus back to the paste target
    /// and send Ctrl+V.
    pub paste: bool,
}

/// Handles one request (on the IPC thread).
pub type Handler = Box<dyn FnMut(Request) -> Reply + Send>;

/// Handle to the pipe server thread.
#[derive(Debug)]
pub struct IpcServer {
    /// Manual-reset event: set to end the thread.
    stop: SendHandle,
    thread: JoinHandle<()>,
}

impl IpcServer {
    pub fn start(handler: Handler) -> io::Result<Self> {
        Self::start_named(ipc::pipe_name(std::process::id()), handler, TIMEOUTS)
    }

    fn start_named(name: String, handler: Handler, timeouts: Timeouts) -> io::Result<Self> {
        let sddl = pipe_sddl()?;
        // SAFETY: creates an unnamed manual-reset event; no pointers passed.
        let stop = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(SendHandle)
            .map_err(io::Error::other)?;
        let spawned = std::thread::Builder::new()
            .name("kuality-ipc".into())
            .spawn(move || {
                let stop = stop;
                serve(&name, &sddl, stop.0, handler, timeouts);
            });
        match spawned {
            Ok(thread) => Ok(Self { stop, thread }),
            Err(e) => {
                // SAFETY: the thread never started, so nothing else uses the event.
                let _ = unsafe { CloseHandle(stop.0) };
                Err(e)
            }
        }
    }

    /// Stops the thread (waits up to `timeout`).
    pub fn stop(self, timeout: Duration) {
        // SAFETY: the event is valid until closed below. It ends any wait of the thread.
        let _ = unsafe { SetEvent(self.stop.0) };
        let deadline = Instant::now() + timeout;
        while !self.thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
            // SAFETY: the thread no longer uses the event.
            let _ = unsafe { CloseHandle(self.stop.0) };
        } else {
            // The event stays open: the thread still waits on it.
            log::warn!("IPC thread did not stop within {timeout:?}");
        }
    }
}

/// Security descriptor: full access for the current user and SYSTEM only, no inheritance;
/// medium integrity label so a non-elevated settings window can reach an elevated core.
fn pipe_sddl() -> io::Result<Vec<u16>> {
    let sid = current_user_sid()?;
    Ok(super::wide(format!(
        "D:P(A;;GA;;;{sid})(A;;GA;;;SY)S:(ML;;NW;;;ME)"
    )))
}

fn current_user_sid() -> io::Result<String> {
    // SAFETY: the token handle is closed below; the buffer is sized by the first call and holds
    // a TOKEN_USER whose SID points inside it; the SID string is freed with LocalFree.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(io::Error::other)?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        // u64 storage keeps the buffer aligned for TOKEN_USER.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let info = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        );
        let _ = CloseHandle(token);
        info.map_err(io::Error::other)?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).map_err(io::Error::other)?;
        let sid = s.to_string().map_err(io::Error::other);
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        sid
    }
}

/// Why a wait on the pipe ended without the operation completing.
fn interrupted(stopped: bool) -> io::Error {
    if stopped {
        io::Error::new(io::ErrorKind::Interrupted, "the IPC server is stopping")
    } else {
        io::Error::new(io::ErrorKind::TimedOut, "the client did not answer in time")
    }
}

/// One server instance of the pipe, opened for overlapped I/O. Closed on drop.
struct Pipe {
    handle: HANDLE,
    /// Manual-reset event of the pending operation (one at a time).
    event: HANDLE,
    /// The server's stop event.
    stop: HANDLE,
}

impl Pipe {
    /// Creates an instance. `first`: no other instance of ours exists, so the call must fail
    /// if another process already owns the name.
    fn create(
        name: &[u16],
        sa: &SECURITY_ATTRIBUTES,
        first: bool,
        event: HANDLE,
        stop: HANDLE,
    ) -> io::Result<Self> {
        let first = if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            FILE_FLAGS_AND_ATTRIBUTES(0)
        };
        // SAFETY: valid NUL-terminated name and security attributes for the call.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | first,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER,
                BUFFER,
                0,
                Some(sa),
            )
        };
        if handle.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            handle,
            event,
            stop,
        })
    }

    fn overlapped(&self) -> OVERLAPPED {
        OVERLAPPED {
            hEvent: self.event,
            ..Default::default()
        }
    }

    /// Waits for the pending operation of `ov`: until it completes, `deadline` passes or the
    /// server stops. An operation that did not complete is cancelled. Returns the bytes
    /// transferred.
    fn finish(&self, ov: &OVERLAPPED, deadline: Option<Instant>) -> io::Result<usize> {
        let ms = match deadline {
            // INFINITE is u32::MAX: stay below it.
            Some(d) => d
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u128::from(u32::MAX - 1)) as u32,
            None => INFINITE,
        };
        let mut n = 0u32;
        // SAFETY: both events are valid for the lifetime of `self`.
        let waited = unsafe { WaitForMultipleObjects(&[self.event, self.stop], false, ms) };
        if waited != WAIT_OBJECT_0 {
            // SAFETY: `ov` belongs to an operation on our handle. The kernel uses it until the
            // operation ends, so wait for the cancellation before `ov` can go away.
            unsafe {
                let _ = CancelIoEx(self.handle, Some(ov));
                let _ = GetOverlappedResult(self.handle, ov, &mut n, true);
            }
            return Err(interrupted(waited.0 == WAIT_OBJECT_0.0 + 1));
        }
        // SAFETY: the operation of `ov` on our handle has completed (its event is set).
        unsafe { GetOverlappedResult(self.handle, ov, &mut n, false) }.map_err(io::Error::other)?;
        Ok(n as usize)
    }

    /// Waits for a client. `Ok(false)`: the server is stopping.
    fn connect(&self) -> io::Result<bool> {
        let mut ov = self.overlapped();
        // SAFETY: our pipe instance; `ov` lives until the operation has ended (`finish`).
        match unsafe { ConnectNamedPipe(self.handle, Some(&mut ov)) } {
            Ok(()) => Ok(true),
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => Ok(true),
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => match self.finish(&ov, None) {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(false),
                Err(e) => Err(e),
            },
            Err(e) => Err(io::Error::other(e)),
        }
    }

    fn read(&self, buf: &mut [u8], deadline: Instant) -> io::Result<usize> {
        let mut ov = self.overlapped();
        let mut n = 0u32;
        // SAFETY: `buf` and `ov` live until the operation has ended (`finish`).
        match unsafe { ReadFile(self.handle, Some(buf), Some(&mut n), Some(&mut ov)) } {
            Ok(()) => Ok(n as usize),
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => self.finish(&ov, Some(deadline)),
            Err(e) => Err(io::Error::other(e)),
        }
    }

    fn write(&self, buf: &[u8], deadline: Instant) -> io::Result<usize> {
        let mut ov = self.overlapped();
        let mut n = 0u32;
        // SAFETY: `buf` and `ov` live until the operation has ended (`finish`).
        match unsafe { WriteFile(self.handle, Some(buf), Some(&mut n), Some(&mut ov)) } {
            Ok(()) => Ok(n as usize),
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => self.finish(&ov, Some(deadline)),
            Err(e) => Err(io::Error::other(e)),
        }
    }

    /// Waits until the client has closed its end (it has read the reply) or `deadline`.
    fn wait_client_close(&self, deadline: Instant) {
        let mut byte = [0u8; 1];
        // Anything the client still sends is ignored; a closed pipe ends the read with an error.
        while Instant::now() < deadline && matches!(self.read(&mut byte, deadline), Ok(1..)) {}
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: our pipe handle, closed once. No operation is pending (`finish` waits).
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// `Read` / `Write` on a [`Pipe`] with one deadline for the whole exchange step.
struct Timed<'a> {
    pipe: &'a Pipe,
    deadline: Instant,
}

impl Read for Timed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.pipe.read(buf, self.deadline)
    }
}

impl Write for Timed<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pipe.write(buf, self.deadline)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Whether the manual-reset event `stop` is set; with `wait_ms`, waits that long for it.
fn stopping(stop: HANDLE, wait_ms: u32) -> bool {
    // SAFETY: a valid event handle.
    unsafe { WaitForSingleObject(stop, wait_ms) == WAIT_OBJECT_0 }
}

fn serve(name: &str, sddl: &[u16], stop: HANDLE, mut handler: Handler, timeouts: Timeouts) {
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl` is NUL-terminated; `sd` is freed with LocalFree at the end.
    if let Err(e) = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
    } {
        log::error!(
            "cannot build the pipe security descriptor: {e}; clipboard history popup unavailable"
        );
        return;
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    // SAFETY: creates an unnamed manual-reset event; closed at the end.
    match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
        Ok(event) => {
            serve_clients(name, &sa, event, stop, &mut handler, timeouts);
            // SAFETY: every pipe using the event has been dropped.
            let _ = unsafe { CloseHandle(event) };
        }
        Err(e) => log::error!("cannot create the IPC event: {e}; clipboard popup unavailable"),
    }
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW above.
    let _ = unsafe { LocalFree(Some(HLOCAL(sd.0))) };
    log::debug!("IPC pipe closed");
}

fn serve_clients(
    name: &str,
    sa: &SECURITY_ATTRIBUTES,
    event: HANDLE,
    stop: HANDLE,
    handler: &mut Handler,
    timeouts: Timeouts,
) {
    let wide_name = super::wide(name);
    let own_exe = std::env::current_exe().ok();
    // Creates an instance, trying again every second until it works or the server stops.
    let create = |first: bool| {
        let mut logged = false;
        loop {
            match Pipe::create(&wide_name, sa, first, event, stop) {
                Ok(pipe) => return Some(pipe),
                Err(e) if !logged => {
                    logged = true;
                    log::error!("cannot create the IPC pipe: {e}");
                }
                Err(_) => {}
            }
            if stopping(stop, RETRY_MS) {
                return None;
            }
        }
    };
    let Some(mut pipe) = create(true) else {
        return;
    };
    log::debug!("IPC pipe ready");
    loop {
        match pipe.connect() {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                log::debug!("IPC: connecting a client failed: {e}");
                if stopping(stop, RETRY_MS) {
                    return;
                }
                // Go on with a fresh instance (the old one is closed once it is replaced).
                match create(false) {
                    Some(fresh) => pipe = fresh,
                    None => return,
                }
                continue;
            }
        }
        // Before this instance is closed, so the pipe name never disappears between two
        // requests (a client would take that for "Kuality is not running").
        let next = Pipe::create(&wide_name, sa, false, event, stop).ok();
        let paste_client = match verify_client(pipe.handle, own_exe.as_ref()) {
            Some(client) => exchange(&pipe, handler, timeouts).then_some(client),
            None => {
                log::warn!("IPC: a process that is not Kuality connected; disconnected");
                None
            }
        };
        drop(pipe);
        if let Some(client) = paste_client {
            client.wait(CLIENT_EXIT_WAIT);
            if super::clipboard::restore_paste_target() {
                super::keyboard::request_paste();
            } else {
                log::info!("clipboard: the window to paste into is gone; entry only copied");
            }
        }
        pipe = match next {
            Some(p) => p,
            None => match create(true) {
                Some(p) => p,
                None => return,
            },
        };
        if stopping(stop, 0) {
            return;
        }
    }
}

/// Reads the request of a connected client, answers it and waits until the client has read the
/// answer. Returns whether a paste must follow.
fn exchange(pipe: &Pipe, handler: &mut Handler, timeouts: Timeouts) -> bool {
    let mut io = Timed {
        pipe,
        deadline: Instant::now() + timeouts.request,
    };
    let request = match ipc::read_frame_limited::<Request>(&mut io, MAX_REQUEST, 0) {
        Ok((request, _)) => request,
        Err(e) => {
            log::debug!("IPC: bad request: {e}");
            return false;
        }
    };
    // Only the operation and entry id: never content.
    log::debug!("IPC: {request:?}");
    let reply = handler(request);
    let mut response = reply.response;
    io.deadline = Instant::now() + timeouts.reply;
    let written = ipc::write_frame(&mut io, &response, &reply.payload);
    wipe_response(&mut response);
    match written {
        // The client closes its end once it has read everything.
        Ok(()) => pipe.wait_client_close(io.deadline),
        Err(e) => log::debug!("IPC reply not delivered: {e}"),
    }
    reply.paste
}

/// Clears the texts sent in a reply (best effort: they are plain clipboard data).
fn wipe_response(response: &mut Response) {
    for e in &mut response.entries {
        if let Some(t) = e.text.take() {
            drop(Plain::from_string(t));
        }
    }
}

/// The connected client process, opened so a paste can wait for it to exit.
struct Client(HANDLE);

impl Client {
    fn wait(&self, timeout: Duration) {
        // SAFETY: a process handle opened with SYNCHRONIZE.
        unsafe { WaitForSingleObject(self.0, timeout.as_millis() as u32) };
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: our handle, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The client process if it runs our own exe (same full path).
fn verify_client(pipe: HANDLE, own_exe: Option<&PathBuf>) -> Option<Client> {
    let own_exe = own_exe?;
    let mut pid = 0u32;
    // SAFETY: plain query into a local.
    unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) }.ok()?;
    // SAFETY: the handle is owned by `Client` (closed on drop).
    let client = Client(
        unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .ok()?,
    );
    let path = super::process_image_path(client.0)?;
    let same = path.eq_ignore_ascii_case(&own_exe.to_string_lossy());
    same.then_some(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A pipe name of its own for each test (they run in parallel in one process).
    fn test_name() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        format!(
            r"\\.\pipe\Kuality.Test.{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Answers `list` with a payload of `payload` bytes and everything else with an error.
    fn server(name: &str, payload: usize, timeouts: Timeouts) -> IpcServer {
        let handler: Handler = Box::new(move |request| Reply {
            response: match request {
                Request::List => Response::ok(),
                _ => Response::error("no"),
            },
            payload: Plain::new(vec![7u8; payload]),
            paste: false,
        });
        IpcServer::start_named(name.to_string(), handler, timeouts).unwrap()
    }

    /// Opens the pipe as a client, waiting up to 3 s for the server to offer it.
    fn connect(name: &str) -> File {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match OpenOptions::new().read(true).write(true).open(name) {
                Ok(f) => return f,
                Err(e) if Instant::now() >= deadline => panic!("cannot open {name}: {e}"),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    fn ask(name: &str, request: Request) -> (Response, Vec<u8>) {
        let mut pipe = connect(name);
        ipc::write_frame(&mut pipe, &request, &[]).unwrap();
        ipc::read_frame(&mut pipe).unwrap()
    }

    #[test]
    fn answers_requests_one_after_another() {
        let name = test_name();
        // Larger than the pipe buffer: the reply is written while the client reads.
        let server = server(&name, 1_000_000, TIMEOUTS);
        for _ in 0..3 {
            let (response, payload) = ask(&name, Request::List);
            assert!(response.ok);
            assert_eq!(payload.len(), 1_000_000);
            assert!(payload.iter().all(|&b| b == 7));
            let (response, _) = ask(&name, Request::Clear);
            assert_eq!(response.error.as_deref(), Some("no"));
        }
        server.stop(Duration::from_secs(2));
    }

    #[test]
    fn a_silent_client_does_not_block_the_next_one() {
        let name = test_name();
        let timeouts = Timeouts {
            request: Duration::from_millis(150),
            reply: Duration::from_millis(150),
        };
        let server = server(&name, 16, timeouts);
        // Connects and never sends its request.
        let silent = connect(&name);
        // Reads nothing of a reply larger than the pipe buffer.
        let mut deaf = connect(&name);
        ipc::write_frame(&mut deaf, &Request::List, &[]).unwrap();
        let started = Instant::now();
        let (response, payload) = ask(&name, Request::List);
        assert!(response.ok);
        assert_eq!(payload, vec![7u8; 16]);
        assert!(started.elapsed() < Duration::from_secs(2), "timeouts apply");
        drop((silent, deaf));
        server.stop(Duration::from_secs(2));
    }

    #[test]
    fn oversized_requests_are_refused() {
        let name = test_name();
        let server = server(&name, 0, TIMEOUTS);
        let mut pipe = connect(&name);
        // Announces a header far above the limit: the server hangs up without reading it.
        pipe.write_all(&(MAX_REQUEST as u32 + 1).to_le_bytes())
            .unwrap();
        assert!(ipc::read_frame::<Response>(&mut pipe).is_err());
        drop(pipe);
        assert!(ask(&name, Request::List).0.ok, "the server goes on");
        server.stop(Duration::from_secs(2));
    }

    #[test]
    fn stop_ends_a_server_waiting_for_clients() {
        let name = test_name();
        let server = server(&name, 0, TIMEOUTS);
        drop(connect(&name));
        let started = Instant::now();
        server.stop(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(OpenOptions::new().read(true).open(&name).is_err(), "gone");
    }
}
