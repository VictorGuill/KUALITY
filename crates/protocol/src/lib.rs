//! Types shared between the Kuality core, the settings app and (later) extensions.
//!
//! Contains the config file types, Kuality key codes, the data-dir rule, the atomic config
//! write and Windows argument quoting.

pub mod config;
pub mod keys;
pub mod paths;
pub mod quote;
pub mod store;
#[cfg(any(test, feature = "test-util"))]
pub mod test_util;

pub use config::{
    Action, Anchor, Binding, BindingEntry, CONFIG_VERSION, Color, CommonItem, Config,
    DEFAULT_FONT_FAMILY, DEFAULT_FONT_SIZE, Dimension, FontWeight, General, HyperModifiers,
    Hyperkey, LaunchAction, LaunchArgs, Length, Lenient, LogLevel, MAX_URL_LEN, OpenUrlAction,
    Origin, SendHotkeyAction, Shortcuts, TextAlign, TextWidget, WidgetEntry, WidgetItem, Widgets,
    valid_url,
};
pub use keys::{
    Chord, Key, KeyParseError, Mods, Trigger, TriggerError, parse_chord, parse_key, parse_trigger,
};
