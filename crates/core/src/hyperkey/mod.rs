//! Hyperkey: CapsLock acts as Ctrl+Shift+Alt+Win while held. See `docs/specs/hyperkey.md`.
//!
//! [`engine`] is the portable state machine; the hook thread lives in the platform layer and is
//! driven through [`KeyboardController`].

pub mod engine;

use std::io;
use std::time::Duration;

use kuality_protocol::Hyperkey;

use crate::platform::{self, KeyboardBackend};

/// How long quit waits for the keyboard thread to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// Commands sent to the keyboard thread.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyboardCommand {
    /// New config (sent on startup and on every reload).
    Apply { hyperkey: Hyperkey },
    /// Release everything, unhook and end the thread.
    Shutdown,
}

/// Main-thread handle to the keyboard thread.
#[derive(Debug)]
pub struct KeyboardController {
    backend: Option<platform::Keyboard>,
}

impl KeyboardController {
    /// Spawns the keyboard thread. No hook is installed until a config enables Hyperkey.
    pub fn start() -> io::Result<Self> {
        Ok(Self {
            backend: Some(platform::Keyboard::start()?),
        })
    }

    pub fn apply(&self, hyperkey: &Hyperkey) {
        self.send(KeyboardCommand::Apply {
            hyperkey: hyperkey.clone(),
        });
    }

    /// Releases any held modifiers, removes the hook and joins the thread.
    pub fn shutdown(&mut self) {
        if let Some(backend) = self.backend.take() {
            backend.send(KeyboardCommand::Shutdown);
            if !backend.join(SHUTDOWN_TIMEOUT) {
                log::warn!("keyboard thread did not stop within {SHUTDOWN_TIMEOUT:?}");
            }
        }
    }

    fn send(&self, cmd: KeyboardCommand) {
        if let Some(b) = &self.backend {
            b.send(cmd);
        }
    }
}

impl Drop for KeyboardController {
    fn drop(&mut self) {
        self.shutdown();
    }
}
