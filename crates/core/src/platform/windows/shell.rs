//! Opening files with their associated program, and the shell side of shortcut actions.

use std::io;
use std::path::Path;

use ::windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use ::windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW, ShellExecuteW,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, SW_SHOWNORMAL,
};
use ::windows::core::{PCWSTR, w};

use super::{input, wide};

/// Opens `path` with its default program; falls back to Notepad if there is no association.
pub fn open_with_default_app(path: &Path) -> io::Result<()> {
    let file = wide(path);
    // SAFETY: all strings are NUL-terminated UTF-16 buffers alive for the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success.
    if result.0 as isize > 32 {
        return Ok(());
    }
    log::debug!(
        "ShellExecuteW(open) failed with {}, falling back to notepad",
        result.0 as isize
    );

    let mut quoted = std::ffi::OsString::from("\"");
    quoted.push(path.as_os_str());
    quoted.push("\"");
    let params = wide(quoted);
    // SAFETY: as above.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("notepad.exe"),
            PCWSTR(params.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "ShellExecuteW failed with code {}",
            result.0 as isize
        )))
    }
}

/// Starts `exe` with `args` (working folder: its own folder) without waiting for it. The new
/// process may take the foreground (it brings its window to the front).
pub fn spawn_detached(exe: &Path, args: &[String]) -> io::Result<()> {
    // SAFETY: plain call without pointers. Failure only means the child may open behind.
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    // The child handle is dropped: the process keeps running on its own.
    cmd.spawn().map(drop)
}

/// Runs a `launch` action: `%VAR%` expanded in all parts, `cwd` defaults to the parent folder of
/// an absolute `path`, else `%USERPROFILE%`. Supports programs, `.lnk`, documents, folders and
/// `shell:AppsFolder\<AUMID>`.
pub fn launch(path: &str, params: &str, cwd: Option<&str>) -> io::Result<()> {
    let path = expand_env(path)?;
    let params = expand_env(params)?;
    let cwd = match cwd {
        Some(c) => Some(expand_env(c)?),
        None => default_cwd(&path),
    };
    shell_execute(&path, &params, cwd.as_deref())
}

/// Runs an `open_url` action: opens the URI with its default handler.
pub fn open_url(url: &str) -> io::Result<()> {
    shell_execute(url, "", None)
}

fn default_cwd(path: &str) -> Option<String> {
    let p = Path::new(path);
    if p.is_absolute()
        && let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty())
    {
        return Some(parent.to_string_lossy().into_owned());
    }
    std::env::var("USERPROFILE").ok()
}

/// `ExpandEnvironmentStringsW`; unknown variables are left as they are.
fn expand_env(s: &str) -> io::Result<String> {
    if !s.contains('%') {
        return Ok(s.to_string());
    }
    let src = wide(s);
    let mut buf = vec![0u16; s.len() + 64];
    loop {
        // SAFETY: `src` is NUL-terminated; `buf` is a writable slice whose length is passed.
        let n = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut buf)) } as usize;
        if n == 0 {
            return Err(io::Error::last_os_error());
        }
        if n <= buf.len() {
            // `n` includes the terminating NUL.
            return Ok(String::from_utf16_lossy(&buf[..n - 1]));
        }
        buf.resize(n, 0);
    }
}

fn shell_execute(file: &str, params: &str, dir: Option<&str>) -> io::Result<()> {
    let file = wide(file);
    let params = (!params.is_empty()).then(|| wide(params));
    let dir = dir.map(wide);
    let as_ptr = |s: &Option<Vec<u16>>| s.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: as_ptr(&params),
        lpDirectory: as_ptr(&dir),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // Lets whatever we start (or the running app a URL is handed to) take the foreground.
    // `AllowSetForegroundWindow` only works if this process may set the foreground itself, which
    // the empty input event grants (see `input::claim_last_input`).
    let claimed = input::claim_last_input();
    // SAFETY: no pointers.
    if let Err(e) = unsafe { AllowSetForegroundWindow(ASFW_ANY) } {
        log::debug!("AllowSetForegroundWindow failed (input claimed: {claimed}): {e}");
    }
    // SAFETY: `info` is initialized and its strings are NUL-terminated buffers that outlive the
    // call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(io::Error::other)
}

/// Renews the foreground grant after our injected Hyper release (see
/// `actions::FOREGROUND_REGRANT_WINDOW`): the release counted as new input and cancelled the
/// grant given before `ShellExecuteExW`. Same two steps as before the launch: claim the last
/// input, then allow any process to take the foreground until the next input.
pub fn regrant_foreground() {
    let claimed = input::claim_last_input();
    // SAFETY: no pointers.
    match unsafe { AllowSetForegroundWindow(ASFW_ANY) } {
        Ok(()) => log::debug!("foreground grant renewed after the Hyper release"),
        Err(e) => log::debug!("foreground grant not renewed (input claimed: {claimed}): {e}"),
    }
}
