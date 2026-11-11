//! Command line of `kuality.exe`. The first argument selects what the process is (nothing: the
//! core); the others are what the core passes to the windows it starts. Defined once, for the
//! side that writes them and the side that reads them.

/// The settings window.
pub const SETTINGS: &str = "--settings";
/// The to-do popup.
pub const TODO: &str = "--todo";
/// The clipboard history popup.
pub const CLIPBOARD: &str = "--clipboard";
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
