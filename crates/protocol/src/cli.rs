//! Command line of `kuality.exe`. The first argument selects what the process is (nothing: the
//! core); the others are what the core passes to the windows it starts. Defined once, for the
//! side that writes them and the side that reads them.

/// The settings window.
pub const SETTINGS: &str = "--settings";
/// The to-do popup.
pub const TODO: &str = "--todo";
/// The clipboard history popup.
pub const CLIPBOARD: &str = "--clipboard";
/// The App lock prompt: `--unlock --app <path>` or `--unlock --confirm <quit|pause>`.
pub const UNLOCK: &str = "--unlock";
/// Start at logon: `--startup <off|on|normal|elevated>`.
pub const STARTUP: &str = "--startup";
/// The elevated helper `--startup` starts through UAC.
pub const STARTUP_TASK: &str = "--startup-task";

/// `--core-pid <pid>`: the core that started the window (it closes when that process ends).
pub const CORE_PID: &str = "--core-pid";
/// To-do popup, `--at <x>,<y>`: where to open, in physical screen pixels.
pub const AT: &str = "--at";
/// To-do popup, `--hover <left>,<top>,<right>,<bottom>`: opened by the mouse resting on that
/// module rectangle; it closes when the mouse leaves it and the popup.
pub const HOVER: &str = "--hover";
/// Clipboard popup, `--paste`: opened by the hotkey, so Enter pastes into the window in front.
pub const PASTE: &str = "--paste";
/// App lock prompt, `--app <path>`: the locked program to unlock.
pub const APP: &str = "--app";
/// App lock prompt, `--confirm <quit|pause>`: a tray command that needs the password.
pub const CONFIRM: &str = "--confirm";
/// To-do popup, `--armed <name>`: started while the mouse rests on the module, before the hover
/// delay ends. The window is created but cloaked (rendered, never seen) until the core sets one
/// of the events named by [`armed_event`]: show it as a peek, open it with the focus (the
/// module was clicked), or quit (the mouse left).
pub const ARMED: &str = "--armed";

/// What the core tells an armed to-do popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmedSignal {
    /// The mouse rested on the module: show the popup without taking the focus.
    Show,
    /// The module was clicked: show the popup with the focus.
    Activate,
    /// The mouse left the module: quit without showing anything.
    Cancel,
}

impl ArmedSignal {
    /// Every signal, in the order of the events a popup waits on.
    pub const ALL: [ArmedSignal; 3] = [Self::Show, Self::Activate, Self::Cancel];
}

/// Name of the manual-reset event for `signal` of the armed popup `name` (the `--armed` value).
pub fn armed_event(name: &str, signal: ArmedSignal) -> String {
    let suffix = match signal {
        ArmedSignal::Show => "show",
        ArmedSignal::Activate => "activate",
        ArmedSignal::Cancel => "cancel",
    };
    format!(r"Local\{name}.{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_event_names_differ_per_signal() {
        let names: Vec<_> = ArmedSignal::ALL
            .iter()
            .map(|s| armed_event("Kuality.TodoArmed.1.2", *s))
            .collect();
        assert_eq!(names[0], r"Local\Kuality.TodoArmed.1.2.show");
        assert_eq!(names[1], r"Local\Kuality.TodoArmed.1.2.activate");
        assert_eq!(names[2], r"Local\Kuality.TodoArmed.1.2.cancel");
    }
}
