//! Types shared between the Kuality core, the settings app and (later) extensions.
//!
//! Contains the config file types and Kuality key codes.

pub mod config;
pub mod keys;

pub use config::{
    Action, Anchor, Binding, BindingEntry, CONFIG_VERSION, Color, CommonItem, Config,
    DEFAULT_FONT_FAMILY, DEFAULT_FONT_SIZE, Dimension, FontWeight, General, Hyperkey, LaunchArgs,
    Length, Lenient, LogLevel, Origin, Shortcuts, TextAlign, TextWidget, WidgetEntry, WidgetItem,
    Widgets,
};
pub use keys::{Chord, Key, KeyParseError, parse_chord, parse_key};
