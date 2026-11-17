//! A popup process started before it is needed (`--armed`, `docs/specs/todo.md` "Armed
//! popup"): the core creates three named events, starts the process, then sets one of them.

use std::io;
use std::path::Path;
use std::process::Child;
use std::sync::atomic::{AtomicU32, Ordering};

use ::windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use ::windows::Win32::System::Threading::{CreateEventW, SetEvent};
use ::windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use ::windows::core::PCWSTR;
use kuality_protocol::cli::{self, ArmedSignal};

use super::wide;

/// Makes every armed popup's event names unique within this core.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// A started, still hidden popup process. Dropping it without [`ArmedPopup::signal`] cancels it
/// (the process quits without showing anything); a missing event means the same to it.
#[derive(Debug)]
pub struct ArmedPopup {
    child: Child,
    /// Manual-reset events, in [`ArmedSignal::ALL`] order.
    events: Vec<HANDLE>,
    signaled: bool,
}

impl ArmedPopup {
    /// Creates the events and starts `exe args… --armed <name>` (`what` names the events, e.g.
    /// `Todo`).
    pub fn spawn(exe: &Path, args: &[String], what: &str) -> io::Result<Self> {
        let name = format!(
            "Kuality.{what}Armed.{}.{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut events = Vec::with_capacity(ArmedSignal::ALL.len());
        for signal in ArmedSignal::ALL {
            match create_event(&cli::armed_event(&name, signal)) {
                Ok(h) => events.push(h),
                Err(e) => {
                    close_all(&events);
                    return Err(e);
                }
            }
        }
        let mut cmd = std::process::Command::new(exe);
        cmd.args(args).arg(cli::ARMED).arg(&name);
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        match cmd.spawn() {
            Ok(child) => Ok(Self {
                child,
                events,
                signaled: false,
            }),
            Err(e) => {
                close_all(&events);
                Err(e)
            }
        }
    }

    /// Whether the process still runs (it quits by itself only when the core is gone or after a
    /// long wait without any signal).
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Tells the process what to do. `Activate` also lets it take the foreground. Keep the value
    /// a while afterwards: closing the last handle of an event the process has not opened yet
    /// deletes it (the process then quits, as for `Cancel`).
    pub fn signal(&mut self, signal: ArmedSignal) {
        if !self.signaled {
            self.set(signal);
        }
    }

    fn set(&mut self, signal: ArmedSignal) {
        if signal == ArmedSignal::Activate {
            // SAFETY: plain call. Failure only means the popup may open behind.
            let _ = unsafe { AllowSetForegroundWindow(self.child.id()) };
        }
        let i = ArmedSignal::ALL
            .iter()
            .position(|s| *s == signal)
            .unwrap_or(0);
        if let Some(h) = self.events.get(i) {
            // SAFETY: `h` is an event handle owned by this value.
            if let Err(e) = unsafe { SetEvent(*h) } {
                log::warn!("cannot signal the armed popup ({signal:?}): {e}");
            }
        }
        self.signaled = true;
    }
}

impl Drop for ArmedPopup {
    fn drop(&mut self) {
        if !self.signaled {
            self.set(ArmedSignal::Cancel);
        }
        close_all(&self.events);
    }
}

/// A new manual-reset, non-signaled event; fails if the name already exists (never ours).
fn create_event(name: &str) -> io::Result<HANDLE> {
    let name = wide(name);
    // SAFETY: `name` is a NUL-terminated UTF-16 string alive for the call.
    let h = unsafe { CreateEventW(None, true, false, PCWSTR(name.as_ptr())) }
        .map_err(io::Error::other)?;
    // SAFETY: reads the calling thread's last error, set by CreateEventW above.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: `h` is a valid handle we own.
        let _ = unsafe { CloseHandle(h) };
        return Err(io::Error::other("the event name is already taken"));
    }
    Ok(h)
}

fn close_all(events: &[HANDLE]) {
    for h in events {
        // SAFETY: each handle is owned by the caller and closed once.
        let _ = unsafe { CloseHandle(*h) };
    }
}
