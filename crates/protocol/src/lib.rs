//! Types shared between the Kuality core, the settings app and (later) extensions.
//!
//! Contains the config file types and Kuality key codes.

pub mod config;
pub mod keys;

pub use config::{CONFIG_VERSION, Config, General, Hyperkey, LogLevel, Shortcuts, Widgets};
pub use keys::Key;
