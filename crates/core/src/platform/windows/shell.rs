//! Opening files with their associated program, and the shell side of shortcut actions.

use std::io;
use std::path::Path;

use ::windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use ::windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use ::windows::Win32::System::Threading::{
    CreateProcessW, GetProcessId, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    STARTF_FORCEOFFFEEDBACK, STARTUPINFOW, WaitForSingleObject,
};
use ::windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW, ShellExecuteW,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, SW_SHOWNORMAL,
};
use ::windows::core::{PCWSTR, PWSTR, w};

use super::focus::{self, Foreground};
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
    // The handle is dropped: the process keeps running on its own.
    spawn_quiet(exe, args).map(drop)
}

/// A process started by [`spawn_quiet`]; dropping it closes the handle (the process runs on).
#[derive(Debug)]
pub struct Process {
    handle: HANDLE,
    pub pid: u32,
}

impl Process {
    /// Whether the process still runs.
    pub fn alive(&self) -> bool {
        // SAFETY: `handle` is a live process handle owned by this value.
        unsafe { WaitForSingleObject(self.handle, 0) == WAIT_TIMEOUT }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and closed once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// Starts `exe` with `args` in its own folder, without the "app starting" cursor Windows shows
/// next to the pointer until the new process shows a window (`STARTF_FORCEOFFFEEDBACK`): a
/// popup started hidden (`--armed`) would otherwise show it for its whole start-up.
pub fn spawn_quiet(exe: &Path, args: &[String]) -> io::Result<Process> {
    let mut line = quote_arg(&exe.to_string_lossy());
    for a in args {
        line.push(' ');
        line.push_str(&quote_arg(a));
    }
    let mut line = wide(&line);
    let app = wide(exe);
    let dir = exe.parent().map(wide);
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_FORCEOFFFEEDBACK,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every string is a NUL-terminated UTF-16 buffer alive for the call (`line` is
    // mutable, as CreateProcessW requires); `startup` and `info` are valid for the call.
    unsafe {
        CreateProcessW(
            PCWSTR(app.as_ptr()),
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            &startup,
            &mut info,
        )
    }
    .map_err(io::Error::other)?;
    // SAFETY: the thread handle from CreateProcessW is ours and not needed.
    let _ = unsafe { CloseHandle(info.hThread) };
    Ok(Process {
        handle: info.hProcess,
        pid: info.dwProcessId,
    })
}

/// One command-line argument as the C runtime / `CommandLineToArgvW` read it back.
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// Runs a `launch` action: `%VAR%` expanded in all parts, `cwd` defaults to the parent folder of
/// an absolute `path`, else `%USERPROFILE%`. Supports programs, `.lnk`, documents, folders and
/// `shell:AppsFolder\<AUMID>`. `before`: the foreground window when the shortcut fired (see
/// `focus`).
pub fn launch(path: &str, params: &str, cwd: Option<&str>, before: Foreground) -> io::Result<()> {
    let path = expand_env(path)?;
    let params = expand_env(params)?;
    let cwd = match cwd {
        Some(c) => Some(expand_env(c)?),
        None => default_cwd(&path),
    };
    shell_execute(&path, &params, cwd.as_deref(), before)
}

/// Runs an `open_url` action: opens the URI with its default handler.
pub fn open_url(url: &str, before: Foreground) -> io::Result<()> {
    shell_execute(url, "", None, before)
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

fn shell_execute(
    file: &str,
    params: &str,
    dir: Option<&str>,
    before: Foreground,
) -> io::Result<()> {
    let file = wide(file);
    let params = (!params.is_empty()).then(|| wide(params));
    let dir = dir.map(wide);
    let as_ptr = |s: &Option<Vec<u16>>| s.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS,
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
    let windows = focus::before_launch();
    // SAFETY: `info` is initialized and its strings are NUL-terminated buffers that outlive the
    // call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(io::Error::other)?;
    // No process handle for packaged apps, URLs handed to a running app, DDE…
    let pid = (!info.hProcess.is_invalid()).then(|| {
        // SAFETY: the handle ShellExecuteExW returned (SEE_MASK_NOCLOSEPROCESS); closed once.
        unsafe {
            let pid = GetProcessId(info.hProcess);
            let _ = CloseHandle(info.hProcess);
            pid
        }
    });
    focus::after_launch(before, windows, pid.filter(|&p| p != 0));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::quote_arg;

    #[test]
    fn arguments_are_quoted_only_when_needed() {
        assert_eq!(quote_arg("--todo"), "--todo");
        assert_eq!(quote_arg("10,1032"), "10,1032");
        assert_eq!(quote_arg(""), r#""""#);
        assert_eq!(
            quote_arg(r"C:\Program Files\Kuality\kuality.exe"),
            r#""C:\Program Files\Kuality\kuality.exe""#
        );
        // Backslashes before a quote, and at the end of a quoted argument, are doubled.
        assert_eq!(quote_arg(r#"a "b""#), r#""a \"b\"""#);
        assert_eq!(quote_arg(r"C:\a b\"), r#""C:\a b\\""#);
    }
}
