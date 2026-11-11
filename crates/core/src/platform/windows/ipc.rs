//! Named pipe server for the clipboard popup and the settings window
//! (`\\.\pipe\Kuality.Core.<pid>`, `docs/specs/clipboard.md`).
//!
//! Thread `kuality-ipc` serves one connection at a time, one request per connection. Only the
//! current user can open the pipe (explicit DACL), remote clients are rejected, and a client
//! whose process is not our own exe is disconnected before anything is read.

use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HLOCAL, LocalFree,
};
use ::windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use ::windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use ::windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use ::windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use ::windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
    WaitForSingleObject,
};
use ::windows::core::{PCWSTR, PWSTR};
use kuality_protocol::clipboard::{Request, Response};
use kuality_protocol::ipc;

use crate::clipboard::sealed::Plain;

/// Buffer sizes of the pipe (the system grows them as needed).
const BUFFER: u32 = 64 * 1024;
/// How long a paste waits for the popup to exit before giving the focus back.
const CLIENT_EXIT_WAIT: Duration = Duration::from_millis(1500);

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
    name: String,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl IpcServer {
    pub fn start(handler: Handler) -> io::Result<Self> {
        let name = ipc::pipe_name(std::process::id());
        let sddl = pipe_sddl()?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (name, stop) = (name.clone(), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("kuality-ipc".into())
                .spawn(move || serve(&name, &sddl, &stop, handler))?
        };
        Ok(Self { name, stop, thread })
    }

    /// Stops the thread (waits up to `timeout`).
    pub fn stop(self, timeout: Duration) {
        self.stop.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + timeout;
        while !self.thread.is_finished() && Instant::now() < deadline {
            // Wakes ConnectNamedPipe; the thread sees the flag and ends without reading.
            // Repeated: the pipe may be busy or between two instances.
            let _ = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.name);
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
        } else {
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

fn serve(name: &str, sddl: &[u16], stop: &AtomicBool, mut handler: Handler) {
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
    let wide_name = super::wide(name);
    let own_exe = std::env::current_exe().ok();
    let mut failures = 0u32;
    log::debug!("IPC pipe ready");
    while !stop.load(Ordering::SeqCst) {
        // SAFETY: valid name and security attributes for the call.
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide_name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                BUFFER,
                BUFFER,
                0,
                Some(&sa),
            )
        };
        if pipe.is_invalid() {
            failures += 1;
            if failures == 1 {
                log::error!("cannot create the IPC pipe: {}", io::Error::last_os_error());
            }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        failures = 0;
        // SAFETY: blocking connect on our pipe instance.
        let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok()
            // SAFETY: reads this thread's last error.
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if stop.load(Ordering::SeqCst) || !connected {
            // SAFETY: our pipe handle, closed once.
            let _ = unsafe { CloseHandle(pipe) };
            continue;
        }
        let Some(client) = verify_client(pipe, own_exe.as_ref()) else {
            log::warn!("IPC: a process that is not Kuality connected; disconnected");
            // SAFETY: as above.
            let _ = unsafe { CloseHandle(pipe) };
            continue;
        };
        // SAFETY: the File takes ownership of the pipe handle and closes it on drop.
        let mut file = unsafe { File::from_raw_handle(pipe.0) };
        let paste = match ipc::read_frame::<Request>(&mut file) {
            Ok((request, _)) => {
                // Only the operation and entry id: never content.
                log::debug!("IPC: {request:?}");
                let reply = handler(request);
                let mut response = reply.response;
                let written = ipc::write_frame(&mut file, &response, &reply.payload)
                    // Waits until the client has read everything.
                    .and_then(|()| file.sync_all());
                wipe_response(&mut response);
                if let Err(e) = written {
                    log::debug!("IPC reply not delivered: {e}");
                }
                reply.paste
            }
            Err(e) => {
                log::debug!("IPC: bad request: {e}");
                false
            }
        };
        drop(file);
        if paste {
            client.wait(CLIENT_EXIT_WAIT);
            if super::clipboard::restore_paste_target() {
                super::keyboard::request_paste();
            } else {
                log::info!("clipboard: the window to paste into is gone; entry only copied");
            }
        }
    }
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW above.
    let _ = unsafe { LocalFree(Some(HLOCAL(sd.0))) };
    log::debug!("IPC pipe closed");
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
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` and `len` are valid for the call.
    unsafe {
        QueryFullProcessImageNameW(
            client.0,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    let same = path.eq_ignore_ascii_case(&own_exe.to_string_lossy());
    same.then_some(client)
}
