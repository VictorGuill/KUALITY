//! Win32 side of the startup entry: the HKCU Run value, the scheduled task (Task Scheduler COM
//! API) and the elevated helper started through UAC.

use std::io;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree,
    WIN32_ERROR,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ITaskFolder, ITaskService, TASK_CREATE_OR_UPDATE, TASK_ENUM_HIDDEN,
    TASK_LOGON_INTERACTIVE_TOKEN, TaskScheduler,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VariantClear};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{BSTR, HRESULT, Interface as _, PCWSTR, PWSTR, w};

use super::{
    Entry, Error, RUN_KEY, RunStep, State, TaskState, TaskStep, entry_name, plan, run_command,
    task_name, task_sddl, task_xml,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn check(r: WIN32_ERROR) -> io::Result<()> {
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(r.0 as i32))
    }
}

/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`, `ERROR_PATH_NOT_FOUND`: no such task / folder.
fn is_not_found(e: &windows::core::Error) -> bool {
    const FILE: HRESULT = HRESULT(0x8007_0002_u32 as i32);
    const PATH: HRESULT = HRESULT(0x8007_0003_u32 as i32);
    e.code() == FILE || e.code() == PATH
}

// ---------------------------------------------------------------------------------------------
// Top level.

pub(super) fn read() -> State {
    let name = entry_name();
    let run_command = match RunKey::new(&name).read() {
        Ok(v) => v,
        Err(e) => {
            log::warn!("cannot read the Run value: {e}");
            None
        }
    };
    let task = match current_user_sid() {
        Ok(sid) => TaskLocation::new(&name, &sid).query(),
        Err(e) => TaskState::Unknown(format!("cannot read the user's SID: {e}")),
    };
    State { run_command, task }
}

pub(super) fn set(want: Entry) -> Result<(), Error> {
    let failed = |what: &str, e: &dyn std::fmt::Display| Error::Failed(format!("{what}: {e}"));
    let exe = std::env::current_exe().map_err(|e| failed("cannot find kuality.exe", &e))?;
    let sid = current_user_sid().map_err(|e| failed("cannot read the user's SID", &e))?;
    let name = entry_name();
    let state = read();
    let steps = plan(&state, want, &exe);
    log::info!("startup: {want:?} from {state:?} -> {steps:?}");
    // The task first: a cancelled UAC prompt leaves everything as it was.
    if let Some(step) = steps.task {
        run_elevated(
            &exe,
            &[
                "--startup-task".into(),
                step.as_arg().into(),
                "--user".into(),
                sid,
                "--name".into(),
                name.clone(),
            ],
        )?;
    }
    let key = RunKey::new(&name);
    match steps.run {
        Some(RunStep::Write) => key
            .write(&run_command(&exe))
            .map_err(|e| failed("cannot write the Run value", &e)),
        Some(RunStep::Delete) => key
            .delete()
            .map_err(|e| failed("cannot remove the Run value", &e)),
        None => Ok(()),
    }
}

/// The elevated helper's work: create or delete the task of `user_sid` for this exe, in task
/// folder `\<name>`.
pub(super) fn apply_task_step(step: TaskStep, user_sid: &str, name: &str) -> io::Result<()> {
    let loc = TaskLocation::new(name, user_sid);
    match step {
        TaskStep::Create => {
            let exe = std::env::current_exe()?;
            loc.create(&task_xml(user_sid, &exe), &task_sddl(user_sid))
        }
        TaskStep::Delete => loc.delete(),
    }
}

/// Starts this exe with `args` through UAC ("runas") and waits for it. No prompt when this
/// process is already elevated.
fn run_elevated(exe: &Path, args: &[String]) -> Result<(), Error> {
    let file = wide(&exe.to_string_lossy());
    let params = wide(&kuality_protocol::quote::join_args(args));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` and the strings it points to are alive for the call.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            Error::Cancelled
        } else {
            Error::Failed(format!("cannot start the elevated helper: {e}"))
        });
    }
    let process = info.hProcess;
    if process.is_invalid() {
        return Err(Error::Failed("the elevated helper did not start".into()));
    }
    let mut code = 0u32;
    // SAFETY: `process` is the helper's handle (SEE_MASK_NOCLOSEPROCESS), owned here and
    // closed once.
    let got = unsafe {
        WaitForSingleObject(process, INFINITE);
        let got = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        got
    };
    match (got, code) {
        (Ok(()), 0) => Ok(()),
        (Ok(()), code) => Err(Error::Failed(format!(
            "the elevated helper failed (exit code {code})"
        ))),
        (Err(e), _) => Err(Error::Failed(format!(
            "cannot read the elevated helper's result: {e}"
        ))),
    }
}

/// String SID of the user running this process.
pub(super) fn current_user_sid() -> io::Result<String> {
    // SAFETY: the token handle is checked and closed once; the buffer is large enough per the
    // first GetTokenInformation call and aligned for TOKEN_USER (u64 storage); the SID string
    // from ConvertSidToStringSidW is freed with LocalFree once.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(io::Error::other)?;
        let mut size = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let r = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            size,
            &mut size,
        );
        let _ = CloseHandle(token);
        r.map_err(io::Error::other)?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).map_err(io::Error::other)?;
        let sid = s.to_string().map_err(io::Error::other);
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        sid
    }
}

// ---------------------------------------------------------------------------------------------
// HKCU Run value.

/// A REG_SZ value under HKCU (the real Run value, or a throwaway key in tests).
#[derive(Debug, Clone, Copy)]
pub(super) struct RunKey<'a> {
    subkey: &'a str,
    name: &'a str,
}

impl<'a> RunKey<'a> {
    /// Value `name` of the real Run key.
    fn new(name: &'a str) -> Self {
        Self {
            subkey: RUN_KEY,
            name,
        }
    }

    pub(super) fn read(&self) -> io::Result<Option<String>> {
        let (k, n) = (wide(self.subkey), wide(self.name));
        let mut size = 0u32;
        // SAFETY: the strings are NUL-terminated and alive; `size` is a valid out pointer.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(k.as_ptr()),
                PCWSTR(n.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(r)?;
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `size` bytes.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(k.as_ptr()),
                PCWSTR(n.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(r)?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(Some(String::from_utf16_lossy(&buf[..len])))
    }

    pub(super) fn write(&self, data: &str) -> io::Result<()> {
        let (k, n) = (wide(self.subkey), wide(self.name));
        let mut key = HKEY::default();
        // SAFETY: `k` is NUL-terminated and alive; `key` is a valid out pointer.
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(k.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        })?;
        let bytes: Vec<u8> = wide(data).iter().flat_map(|c| c.to_le_bytes()).collect();
        // SAFETY: `key` is open with KEY_SET_VALUE; closed once below.
        let r = unsafe { RegSetValueExW(key, PCWSTR(n.as_ptr()), None, REG_SZ, Some(&bytes)) };
        // SAFETY: closes the key opened above.
        let _ = unsafe { RegCloseKey(key) };
        check(r)
    }

    pub(super) fn delete(&self) -> io::Result<()> {
        let (k, n) = (wide(self.subkey), wide(self.name));
        let mut key = HKEY::default();
        // SAFETY: `k` is NUL-terminated and alive; `key` is a valid out pointer.
        let r = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(k.as_ptr()),
                None,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(r)?;
        // SAFETY: `key` is open with KEY_SET_VALUE; closed once below.
        let r = unsafe { RegDeleteValueW(key, PCWSTR(n.as_ptr())) };
        // SAFETY: closes the key opened above.
        let _ = unsafe { RegCloseKey(key) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(r)
    }
}

// ---------------------------------------------------------------------------------------------
// Scheduled task.

/// COM initialized on this thread (multithreaded) for the guard's lifetime. If the thread
/// already has another apartment, COM is usable as is and nothing is undone.
struct ComGuard(bool);

impl ComGuard {
    fn new() -> Self {
        // SAFETY: plain call; balanced by CoUninitialize in Drop only when it succeeded.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx in `new` on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// A `VARIANT` holding a BSTR, cleared on drop.
struct BstrVariant(VARIANT);

impl BstrVariant {
    fn new(s: &str) -> Self {
        let mut v = VARIANT::default();
        // SAFETY: writes the type tag and the matching union member of a zeroed VARIANT; the
        // BSTR is freed by VariantClear in Drop.
        unsafe {
            let inner = &mut *v.Anonymous.Anonymous;
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(s));
        }
        Self(v)
    }
}

impl Drop for BstrVariant {
    fn drop(&mut self) {
        // SAFETY: the VARIANT was initialized by `new`.
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

/// Folder and name of a scheduled task.
#[derive(Debug, Clone)]
pub(super) struct TaskLocation {
    folder: String,
    name: String,
}

impl TaskLocation {
    /// Task of user `sid` in folder `\<folder>`.
    fn new(folder: &str, sid: &str) -> Self {
        Self {
            folder: format!(r"\{folder}"),
            name: task_name(sid),
        }
    }

    fn service() -> windows::core::Result<ITaskService> {
        // SAFETY: plain COM calls on an initialized apartment (callers hold a ComGuard).
        unsafe {
            let service: ITaskService =
                CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let empty = VARIANT::default();
            service.Connect(&empty, &empty, &empty, &empty)?;
            Ok(service)
        }
    }

    fn folder(service: &ITaskService, path: &str) -> windows::core::Result<ITaskFolder> {
        // SAFETY: plain COM call.
        unsafe { service.GetFolder(&BSTR::from(path)) }
    }

    pub(super) fn query(&self) -> TaskState {
        let _com = ComGuard::new();
        let found = Self::service()
            .and_then(|s| Self::folder(&s, &self.folder))
            // SAFETY: plain COM call.
            .and_then(|f| unsafe { f.GetTask(&BSTR::from(self.name.as_str())) });
        let task = match found {
            Ok(t) => t,
            Err(e) if is_not_found(&e) => return TaskState::Missing,
            Err(e) => return TaskState::Unknown(e.message()),
        };
        // The program it starts; unreadable details still mean "present".
        // SAFETY: plain COM calls on a live task object.
        let exe = unsafe {
            task.Definition()
                .and_then(|d| d.Actions())
                .and_then(|a| a.get_Item(1))
                .and_then(|a| a.cast::<IExecAction>())
                .and_then(|a| {
                    let mut path = BSTR::new();
                    a.Path(&mut path).map(|()| path)
                })
        };
        TaskState::Present {
            exe: exe
                .ok()
                .map(|p| PathBuf::from(p.to_string().trim_matches('"'))),
        }
    }

    pub(super) fn create(&self, xml: &str, sddl: &str) -> io::Result<()> {
        let _com = ComGuard::new();
        let service = Self::service().map_err(io::Error::other)?;
        let folder = match Self::folder(&service, &self.folder) {
            Ok(f) => f,
            Err(e) if is_not_found(&e) => {
                let root = Self::folder(&service, r"\").map_err(io::Error::other)?;
                // SAFETY: plain COM call; an empty VARIANT keeps the inherited security.
                unsafe { root.CreateFolder(&BSTR::from(self.folder.as_str()), &VARIANT::default()) }
                    .map_err(io::Error::other)?
            }
            Err(e) => return Err(io::Error::other(e)),
        };
        let empty = VARIANT::default();
        let sddl = BstrVariant::new(sddl);
        // SAFETY: plain COM call; all arguments are alive for it.
        unsafe {
            folder.RegisterTask(
                &BSTR::from(self.name.as_str()),
                &BSTR::from(xml),
                TASK_CREATE_OR_UPDATE.0,
                &empty,
                &empty,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &sddl.0,
            )
        }
        .map(drop)
        .map_err(io::Error::other)
    }

    /// Deletes the task (a missing one is fine) and the folder if it is left empty.
    pub(super) fn delete(&self) -> io::Result<()> {
        let _com = ComGuard::new();
        let service = Self::service().map_err(io::Error::other)?;
        let folder = match Self::folder(&service, &self.folder) {
            Ok(f) => f,
            Err(e) if is_not_found(&e) => return Ok(()),
            Err(e) => return Err(io::Error::other(e)),
        };
        // SAFETY: plain COM call.
        match unsafe { folder.DeleteTask(&BSTR::from(self.name.as_str()), 0) } {
            Ok(()) => {}
            Err(e) if is_not_found(&e) => {}
            Err(e) => return Err(io::Error::other(e)),
        }
        // SAFETY: plain COM calls.
        let empty = unsafe {
            folder
                .GetTasks(TASK_ENUM_HIDDEN.0)
                .and_then(|t| t.Count())
                .is_ok_and(|n| n == 0)
                && folder
                    .GetFolders(0)
                    .and_then(|f| f.Count())
                    .is_ok_and(|n| n == 0)
        };
        if empty && let Ok(root) = Self::folder(&service, r"\") {
            // SAFETY: plain COM call; failure only leaves an empty folder behind.
            let _ = unsafe { root.DeleteFolder(&BSTR::from(self.folder.as_str()), 0) };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run-value read/write/delete against a throwaway HKCU key (never the real Run key).
    #[test]
    fn run_value_round_trip() {
        let subkey = format!(r"Software\Kuality-test-{}", std::process::id());
        let key = RunKey {
            subkey: &subkey,
            name: "Kuality",
        };
        assert_eq!(key.read().unwrap(), None);
        key.delete().unwrap();
        key.write(r#""C:\a b\kuality.exe""#).unwrap();
        assert_eq!(
            key.read().unwrap().as_deref(),
            Some(r#""C:\a b\kuality.exe""#)
        );
        key.write("x").unwrap();
        assert_eq!(key.read().unwrap().as_deref(), Some("x"));
        key.delete().unwrap();
        assert_eq!(key.read().unwrap(), None);
        let k = wide(&subkey);
        // SAFETY: deletes the throwaway key created above.
        let _ = unsafe {
            windows::Win32::System::Registry::RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()))
        };
        // SAFETY: as above; RegDeleteTreeW may leave the key itself.
        let _ = unsafe {
            windows::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()))
        };
    }

    #[test]
    fn user_sid_looks_right() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
    }

    /// Creates, reads and deletes a **non-elevated** task in a throwaway folder (the elevated
    /// one needs UAC). Touches the real Task Scheduler, so it only runs on demand:
    /// `cargo test -p kuality-settings -- --ignored task_round_trip`.
    #[test]
    #[ignore]
    fn task_round_trip() {
        let sid = current_user_sid().unwrap();
        let loc = TaskLocation {
            folder: format!(r"\Kuality-test-{}", std::process::id()),
            name: task_name(&sid),
        };
        let exe = PathBuf::from(r"C:\Kuality test\kuality.exe");
        let xml = task_xml(&sid, &exe).replace(
            "<RunLevel>HighestAvailable</RunLevel>",
            "<RunLevel>LeastPrivilege</RunLevel>",
        );
        // Full control for the user too, so this unelevated test can delete it.
        let sddl = format!("D:(A;;FA;;;BA)(A;;FA;;;SY)(A;;FA;;;{sid})");
        assert_eq!(loc.query(), TaskState::Missing);
        // A non-admin may not be allowed to create folders: fall back to the root.
        let loc = match loc.create(&xml, &sddl) {
            Ok(()) => loc,
            Err(e) => {
                eprintln!("folder create failed ({e}); using the root folder");
                let root = TaskLocation {
                    folder: r"\".into(),
                    name: format!("Kuality-test-{}", std::process::id()),
                };
                root.create(&xml, &sddl).unwrap();
                root
            }
        };
        assert_eq!(loc.query(), TaskState::Present { exe: Some(exe) });
        loc.delete().unwrap();
        assert_eq!(loc.query(), TaskState::Missing);
        loc.delete().unwrap();
    }
}
