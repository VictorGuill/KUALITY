//! Types shared between the Kuality core, the settings app and (later) extensions.
//!
//! Contains the config file types, Kuality key codes, the data-dir rule, the atomic config
//! write, Windows argument quoting, the `todos.json` types, the clipboard history IPC and pins
//! file formats, and the command line the processes of `kuality.exe` use between them.

pub mod cli;
pub mod clipboard;
pub mod config;
pub mod ipc;
pub mod keys;
pub mod paths;
pub mod quote;
pub mod store;
#[cfg(any(test, feature = "test-util"))]
pub mod test_util;
pub mod todo;

pub use config::{
    Action, Anchor, BarModule, BarModuleEntry, BarWidget, Binding, BindingEntry,
    CLIPBOARD_ENTRIES_RANGE, CONFIG_VERSION, Clipboard, Color, CommonItem, Config, DEFAULT_BAR_GAP,
    DEFAULT_CLIPBOARD_ENTRIES, DEFAULT_CLIPBOARD_HOTKEY, DEFAULT_FONT_FAMILY, DEFAULT_FONT_SIZE,
    DEFAULT_TODO_LABEL, Dimension, FontWeight, General, HyperModifiers, Hyperkey, LaunchAction,
    LaunchArgs, Length, Lenient, LogLevel, MAX_URL_LEN, NewItems, OpenUrlAction, Origin,
    SendHotkeyAction, Shortcuts, TextAlign, TextWidget, Todo, TodoModule, Updates, WidgetEntry,
    WidgetItem, Widgets, valid_url,
};
pub use keys::{
    Chord, Key, KeyParseError, Mods, Trigger, TriggerError, parse_chord, parse_key, parse_trigger,
};
pub use todo::{MAX_TODO_TEXT, TodoItem, TodoList};

/// Seconds since the Unix epoch (0 if the clock is before it).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
