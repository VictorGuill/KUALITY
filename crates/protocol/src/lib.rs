//! Types shared between the Kuality core, the settings app and (later) extensions.
//!
//! Contains the config file types and Kuality key codes.

pub mod config;
pub mod keys;

pub use config::{
    Action, Binding, BindingEntry, CONFIG_VERSION, Config, General, Hyperkey, LaunchArgs, LogLevel,
    Shortcuts, Widgets,
};
pub use keys::{Chord, Key, KeyParseError, parse_chord, parse_key};
