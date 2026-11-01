//! Start at logon: a `Kuality` value in `HKCU\...\Run` (normal) or a scheduled task with highest
//! privileges (elevated), never both. See `docs/specs/distribution.md`.
//!
//! The system entry is the source of truth (not `config.json`): the installer sets it too, and
//! only an elevated process can create the task. The unelevated side writes HKCU and starts
//! `kuality.exe --startup-task …` through UAC for the task part only.
//!
//! This module is the portable part (plan, command lines, task XML), unit-tested; the Win32
//! calls live in [`windows`].

mod windows;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Where Kuality should start at logon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// Not at all.
    Off,
    /// `HKCU\...\Run` value, normal rights.
    Normal,
    /// Scheduled task, highest privileges.
    Elevated,
}

impl Entry {
    /// Command-line spelling (`--startup <entry>`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "normal" => Some(Self::Normal),
            "elevated" => Some(Self::Elevated),
            _ => None,
        }
    }
}

/// The scheduled task as seen by this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Missing,
    /// `exe`: the program it starts, if it could be read.
    Present {
        exe: Option<PathBuf>,
    },
    /// Could not be checked (e.g. access denied); the message says why.
    Unknown(String),
}

/// What is registered now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// Data of the `Kuality` Run value, if present.
    pub run_command: Option<String>,
    pub task: TaskState,
}

impl State {
    /// The entry in effect (the task wins if both exist).
    pub fn entry(&self) -> Entry {
        match (&self.task, &self.run_command) {
            (TaskState::Present { .. }, _) => Entry::Elevated,
            (_, Some(_)) => Entry::Normal,
            _ => Entry::Off,
        }
    }

    /// The exe the active entry starts, when it is not `exe` (this copy). `None` when it is
    /// this copy, nothing is registered, or it cannot be read.
    pub fn other_copy(&self, exe: &Path) -> Option<PathBuf> {
        let target = match (&self.task, &self.run_command) {
            (TaskState::Present { exe }, _) => exe.clone()?,
            (_, Some(cmd)) => exe_of_command(cmd)?,
            _ => return None,
        };
        (!same_path(&target, exe)).then_some(target)
    }
}

/// One step on the scheduled task (needs elevation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStep {
    Create,
    Delete,
}

impl TaskStep {
    pub fn as_arg(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Delete => "delete",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "create" => Some(Self::Create),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// One step on the Run value (no elevation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStep {
    Write,
    Delete,
}

/// Steps to reach an entry. The task step runs first: if its UAC prompt is cancelled, nothing
/// has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub task: Option<TaskStep>,
    pub run: Option<RunStep>,
}

/// What to do to go from `state` to `want` for the copy at `exe`.
pub fn plan(state: &State, want: Entry, exe: &Path) -> Plan {
    let task_may_exist = !matches!(state.task, TaskState::Missing);
    let has_run = state.run_command.is_some();
    match want {
        Entry::Off => Plan {
            task: task_may_exist.then_some(TaskStep::Delete),
            run: has_run.then_some(RunStep::Delete),
        },
        Entry::Normal => Plan {
            task: task_may_exist.then_some(TaskStep::Delete),
            // Always written: points it to this copy.
            run: Some(RunStep::Write),
        },
        Entry::Elevated => {
            let up_to_date =
                matches!(&state.task, TaskState::Present { exe: Some(t) } if same_path(t, exe));
            Plan {
                task: (!up_to_date).then_some(TaskStep::Create),
                run: has_run.then_some(RunStep::Delete),
            }
        }
    }
}

/// Why a change did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The UAC prompt was cancelled; nothing changed.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Failed(e) => f.write_str(e),
        }
    }
}

/// Value name under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, also the Task
/// Scheduler folder name (`\Kuality`).
pub const RUN_VALUE: &str = "Kuality";
/// `HKCU` subkey holding the Run values.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Run value name and task folder: [`RUN_VALUE`], or in debug builds the `KUALITY_STARTUP_NAME`
/// environment variable, so manual tests never touch the real entry. A UAC ("runas") child does
/// not inherit our environment, so the elevated helper gets the name with `--name`.
pub fn entry_name() -> String {
    #[cfg(debug_assertions)]
    if let Ok(name) = std::env::var("KUALITY_STARTUP_NAME")
        && valid_entry_name(&name)
    {
        return name;
    }
    RUN_VALUE.to_string()
}

/// A name usable as a Run value and a Task Scheduler folder.
fn valid_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Task names are machine-wide: one task per user.
pub fn task_name(user_sid: &str) -> String {
    format!("Start elevated {user_sid}")
}

/// Full control for administrators and SYSTEM, read for the user: the unelevated settings
/// window and uninstaller can see it, but an unelevated program cannot change what it starts.
pub fn task_sddl(user_sid: &str) -> String {
    format!("D:(A;;FA;;;BA)(A;;FA;;;SY)(A;;GR;;;{user_sid})")
}

/// Run value data: the quoted exe path.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// The program of a command line (quoted or up to the first space).
pub fn exe_of_command(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    let path = match cmd.strip_prefix('"') {
        Some(rest) => &rest[..rest.find('"')?],
        None => cmd.split(' ').next()?,
    };
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Case-insensitive path comparison (Windows file names).
fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

/// Task Scheduler XML of the elevated logon task for `user_sid`, starting `exe`.
pub fn task_xml(user_sid: &str, exe: &Path) -> String {
    let dir = exe.parent().unwrap_or(Path::new(""));
    let exe = xml_escape(&exe.display().to_string());
    let dir = xml_escape(&dir.display().to_string());
    let sid = xml_escape(user_sid);
    // Priority 4 = normal (the default 7 is below normal, bad for the keyboard hook).
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>Kuality</Author>
    <Description>Starts Kuality with administrator rights when you sign in.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{sid}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{sid}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>false</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>"{exe}"</Command>
      <WorkingDirectory>{dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------------------------
// Entry points used by the window and the command line.

/// Reads the current state (Run value and task).
pub fn read() -> State {
    windows::read()
}

/// Sets the entry for this copy of Kuality. Shows one UAC prompt when the task changes (none
/// when this process is already elevated). Blocks: call it off the UI thread.
pub fn set(want: Entry) -> Result<(), Error> {
    windows::set(want)
}

/// Exit codes of `--startup`.
const EXIT_FAILED: u8 = 1;
const EXIT_CANCELLED: u8 = 2;

/// What `--startup <arg>` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    Set(Entry),
    /// `on`: start at logon, keeping an elevated entry if there is one (the installer's
    /// "Start with Windows" must not undo "Run as administrator" chosen later in the window).
    On,
}

fn parse_request(arg: &str) -> Option<Request> {
    match arg {
        "on" => Some(Request::On),
        other => Entry::parse(other).map(Request::Set),
    }
}

/// The entry `request` resolves to when `current` is in effect.
fn resolve(request: Request, current: Entry) -> Entry {
    match request {
        Request::Set(e) => e,
        Request::On if current == Entry::Elevated => Entry::Elevated,
        Request::On => Entry::Normal,
    }
}

/// `kuality.exe --startup <off|on|normal|elevated>` (installer, uninstaller, scripts).
pub fn cli(args: &[String]) -> ExitCode {
    let Some(request) = args.first().and_then(|a| parse_request(a)) else {
        eprintln!("usage: kuality.exe --startup <off|on|normal|elevated>");
        return ExitCode::from(EXIT_FAILED);
    };
    let want = match request {
        Request::Set(e) => e,
        Request::On => resolve(request, read().entry()),
    };
    match set(want) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Cancelled) => ExitCode::from(EXIT_CANCELLED),
        Err(Error::Failed(e)) => {
            eprintln!("cannot set the startup entry: {e}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}

/// Parsed `--startup-task <create|delete> --user <SID> [--name <entry name>]`.
#[derive(Debug, PartialEq, Eq)]
struct TaskArgs {
    step: TaskStep,
    user_sid: String,
    /// Task folder name ([`entry_name`] of the caller).
    name: String,
}

fn parse_task_args(args: &[String]) -> Option<TaskArgs> {
    let step = TaskStep::parse(args.first()?)?;
    let mut rest = args[1..].iter();
    let (mut user_sid, mut name) = (None, None);
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--user" => user_sid = rest.next().cloned(),
            "--name" => name = rest.next().cloned(),
            _ => {}
        }
    }
    let user_sid = user_sid.filter(|s| s.starts_with("S-1-") && !s.contains(['"', ' ']))?;
    let name = match name {
        Some(n) => Some(n).filter(|n| valid_entry_name(n))?,
        None => RUN_VALUE.to_string(),
    };
    Some(TaskArgs {
        step,
        user_sid,
        name,
    })
}

/// `kuality.exe --startup-task <create|delete> --user <SID> [--name <name>]`: the elevated
/// helper. Only touches the scheduled task of that user.
pub fn task_cli(args: &[String]) -> ExitCode {
    let Some(a) = parse_task_args(args) else {
        eprintln!("usage: kuality.exe --startup-task <create|delete> --user <SID> [--name <name>]");
        return ExitCode::from(EXIT_FAILED);
    };
    match windows::apply_task_step(a.step, &a.user_sid, &a.name) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cannot {} the startup task: {e}", a.step.as_arg());
            ExitCode::from(EXIT_FAILED)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\me\AppData\Local\Programs\Kuality\kuality.exe";

    fn state(run: Option<&str>, task: TaskState) -> State {
        State {
            run_command: run.map(str::to_string),
            task,
        }
    }

    fn present(exe: &str) -> TaskState {
        TaskState::Present {
            exe: Some(PathBuf::from(exe)),
        }
    }

    #[test]
    fn entry_in_effect() {
        assert_eq!(state(None, TaskState::Missing).entry(), Entry::Off);
        assert_eq!(
            state(Some("\"x\""), TaskState::Missing).entry(),
            Entry::Normal
        );
        assert_eq!(state(None, present(EXE)).entry(), Entry::Elevated);
        assert_eq!(state(Some("\"x\""), present(EXE)).entry(), Entry::Elevated);
        assert_eq!(
            state(None, TaskState::Unknown("denied".into())).entry(),
            Entry::Off
        );
    }

    #[test]
    fn plans_never_leave_both_entries() {
        let exe = Path::new(EXE);
        let off = state(None, TaskState::Missing);
        let normal = state(Some(&run_command(exe)), TaskState::Missing);
        let elevated = state(None, present(EXE));

        assert_eq!(
            plan(&off, Entry::Normal, exe),
            Plan {
                task: None,
                run: Some(RunStep::Write)
            }
        );
        assert_eq!(
            plan(&off, Entry::Elevated, exe),
            Plan {
                task: Some(TaskStep::Create),
                run: None
            }
        );
        assert_eq!(
            plan(&normal, Entry::Elevated, exe),
            Plan {
                task: Some(TaskStep::Create),
                run: Some(RunStep::Delete)
            }
        );
        assert_eq!(
            plan(&normal, Entry::Off, exe),
            Plan {
                task: None,
                run: Some(RunStep::Delete)
            }
        );
        assert_eq!(
            plan(&elevated, Entry::Normal, exe),
            Plan {
                task: Some(TaskStep::Delete),
                run: Some(RunStep::Write)
            }
        );
        assert_eq!(
            plan(&elevated, Entry::Off, exe),
            Plan {
                task: Some(TaskStep::Delete),
                run: None
            }
        );
        // Already elevated for this copy: no UAC prompt.
        assert_eq!(
            plan(&elevated, Entry::Elevated, exe),
            Plan {
                task: None,
                run: None
            }
        );
        // Elevated for another copy: re-created for this one.
        let other = state(None, present(r"D:\portable\kuality.exe"));
        assert_eq!(
            plan(&other, Entry::Elevated, exe).task,
            Some(TaskStep::Create)
        );
        // A task we cannot read is deleted when leaving elevated mode (the helper ignores a
        // missing one).
        let unknown = state(None, TaskState::Unknown("denied".into()));
        assert_eq!(plan(&unknown, Entry::Off, exe).task, Some(TaskStep::Delete));
    }

    #[test]
    fn other_copy_detection() {
        let exe = Path::new(EXE);
        assert_eq!(state(None, TaskState::Missing).other_copy(exe), None);
        let same = state(Some(&run_command(exe).to_uppercase()), TaskState::Missing);
        assert_eq!(same.other_copy(exe), None);
        let other = state(Some(r#""D:\tools\kuality.exe""#), TaskState::Missing);
        assert_eq!(
            other.other_copy(exe),
            Some(PathBuf::from(r"D:\tools\kuality.exe"))
        );
        let task = state(None, present(r"D:\tools\kuality.exe"));
        assert_eq!(
            task.other_copy(exe),
            Some(PathBuf::from(r"D:\tools\kuality.exe"))
        );
    }

    #[test]
    fn command_lines() {
        assert_eq!(run_command(Path::new(r"C:\a b\k.exe")), r#""C:\a b\k.exe""#);
        assert_eq!(
            exe_of_command(r#""C:\a b\k.exe" --x"#),
            Some(PathBuf::from(r"C:\a b\k.exe"))
        );
        assert_eq!(
            exe_of_command(r"C:\k.exe --x"),
            Some(PathBuf::from(r"C:\k.exe"))
        );
        assert_eq!(exe_of_command("\"unterminated"), None);
        assert_eq!(exe_of_command("  "), None);
    }

    #[test]
    fn task_xml_content() {
        let xml = task_xml("S-1-5-21-1-2-3-1001", Path::new(r"C:\K&Co\kuality.exe"));
        assert!(xml.contains("<UserId>S-1-5-21-1-2-3-1001</UserId>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<Priority>4</Priority>"));
        assert!(xml.contains(r#"<Command>"C:\K&amp;Co\kuality.exe"</Command>"#));
        assert!(xml.contains(r"<WorkingDirectory>C:\K&amp;Co</WorkingDirectory>"));
        assert_eq!(
            task_sddl("S-1-5-21-9"),
            "D:(A;;FA;;;BA)(A;;FA;;;SY)(A;;GR;;;S-1-5-21-9)"
        );
        assert_eq!(task_name("S-1-5-21-9"), "Start elevated S-1-5-21-9");
    }

    #[test]
    fn on_keeps_elevated() {
        assert_eq!(parse_request("on"), Some(Request::On));
        assert_eq!(parse_request("off"), Some(Request::Set(Entry::Off)));
        assert_eq!(parse_request("yes"), None);
        assert_eq!(resolve(Request::On, Entry::Off), Entry::Normal);
        assert_eq!(resolve(Request::On, Entry::Normal), Entry::Normal);
        assert_eq!(resolve(Request::On, Entry::Elevated), Entry::Elevated);
        assert_eq!(
            resolve(Request::Set(Entry::Normal), Entry::Elevated),
            Entry::Normal
        );
    }

    #[test]
    fn arguments() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(Entry::parse("elevated"), Some(Entry::Elevated));
        assert_eq!(Entry::parse("on"), None);
        assert_eq!(
            parse_task_args(&a(&["create", "--user", "S-1-5-21-7"])),
            Some(TaskArgs {
                step: TaskStep::Create,
                user_sid: "S-1-5-21-7".into(),
                name: "Kuality".into(),
            })
        );
        assert_eq!(
            parse_task_args(&a(&[
                "delete",
                "--user",
                "S-1-5-21-7",
                "--name",
                "Kuality-test"
            ]))
            .map(|t| t.name),
            Some("Kuality-test".into())
        );
        assert_eq!(
            parse_task_args(&a(&["delete", "--user", "S-1-5-21-7", "--name", r"..\x"])),
            None
        );
        assert_eq!(parse_task_args(&a(&["delete"])), None);
        assert_eq!(parse_task_args(&a(&["delete", "--user", "bob"])), None);
        assert_eq!(parse_task_args(&a(&["move", "--user", "S-1-5-21-7"])), None);
    }
}
