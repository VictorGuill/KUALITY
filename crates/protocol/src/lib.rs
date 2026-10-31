//! Types shared between the Kuality core, the settings app and (later) extensions.
//!
//! Phase 0 only contains the config file types.

pub mod config;

pub use config::{CONFIG_VERSION, Config, General, Hyperkey, LogLevel, Shortcuts, Widgets};
