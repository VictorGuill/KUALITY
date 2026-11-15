//! The core's pipe, shared by the clipboard history and App lock (`docs/specs/clipboard.md`,
//! `docs/specs/app-lock.md`).
//!
//! [`IpcHub`] runs the pipe server while at least one of the two features is on and hands each
//! request to that feature's handler. A request for a feature that is off gets an error reply
//! (the clipboard one says the history is off, which clients show as such).

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use kuality_protocol::ipc::{Request, Response};
use kuality_protocol::{app_lock, clipboard};

use crate::clipboard::sealed::Plain;
use crate::platform::{IpcServer, Reply};

/// How long stopping waits for the pipe thread (it may be checking a password).
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Answers clipboard requests (on the IPC thread).
pub type ClipboardHandler = Box<dyn FnMut(clipboard::Request) -> Reply + Send>;
/// Answers App lock requests (on the IPC thread).
pub type AppLockHandler = Box<dyn FnMut(app_lock::Request) -> app_lock::Response + Send>;

type Shared<T> = Arc<Mutex<T>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The handlers of the features that are on. Each one has its own lock, so a slow request
/// (a password check) never blocks the main thread changing the routes.
#[derive(Default)]
struct Routes {
    clipboard: Option<Shared<ClipboardHandler>>,
    app_lock: Option<Shared<AppLockHandler>>,
}

impl std::fmt::Debug for Routes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Routes")
            .field("clipboard", &self.clipboard.is_some())
            .field("app_lock", &self.app_lock.is_some())
            .finish()
    }
}

/// The pipe server and the features it serves.
#[derive(Debug, Default)]
pub struct IpcHub {
    routes: Shared<Routes>,
    server: Option<IpcServer>,
}

impl IpcHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serves the clipboard history with `handler` (`None`: the history is off).
    pub fn set_clipboard(&mut self, handler: Option<ClipboardHandler>) {
        lock(&self.routes).clipboard = handler.map(|h| Arc::new(Mutex::new(h)));
        self.reconcile();
    }

    /// Serves App lock with `handler` (`None`: App lock is off).
    pub fn set_app_lock(&mut self, handler: Option<AppLockHandler>) {
        lock(&self.routes).app_lock = handler.map(|h| Arc::new(Mutex::new(h)));
        self.reconcile();
    }

    /// Starts the server when a feature needs it, stops it when none does.
    fn reconcile(&mut self) {
        let needed = {
            let r = lock(&self.routes);
            r.clipboard.is_some() || r.app_lock.is_some()
        };
        match (&self.server, needed) {
            (None, true) => {
                let routes = Arc::clone(&self.routes);
                match IpcServer::start(Box::new(move |request| dispatch(&routes, request))) {
                    Ok(s) => self.server = Some(s),
                    Err(e) => log::error!(
                        "cannot start the IPC pipe: {e}; the clipboard popup and the App lock \
                         prompt cannot reach Kuality"
                    ),
                }
            }
            (Some(_), false) => self.stop(),
            _ => {}
        }
    }

    fn stop(&mut self) {
        if let Some(s) = self.server.take() {
            s.stop(STOP_TIMEOUT);
        }
    }

    /// Stops the server and forgets every handler.
    pub fn shutdown(&mut self) {
        *lock(&self.routes) = Routes::default();
        self.stop();
    }
}

impl Drop for IpcHub {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Hands one request to the handler of its feature.
fn dispatch(routes: &Mutex<Routes>, request: Request) -> Reply {
    let plain = |response: Response| Reply {
        response,
        payload: Plain::default(),
        paste: false,
    };
    match request {
        Request::Clipboard(r) => {
            let handler = lock(routes).clipboard.clone();
            match handler {
                Some(h) => lock(&h)(r),
                None => plain(Response::Clipboard(clipboard::Response::error(
                    clipboard::HISTORY_OFF,
                ))),
            }
        }
        Request::AppLock(mut r) => {
            let handler = lock(routes).app_lock.clone();
            let response = match handler {
                Some(h) => lock(&h)(r.clone()),
                None => app_lock::Response::error("App lock is off."),
            };
            // The handler got a copy; wipe the passwords of this one too (best effort).
            for p in r.passwords_mut() {
                drop(Plain::from_string(std::mem::take(p)));
            }
            plain(Response::AppLock(response))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_reach_the_feature_that_is_on() {
        let routes = Mutex::new(Routes::default());
        let reply = dispatch(&routes, Request::Clipboard(clipboard::Request::List));
        assert_eq!(
            reply.response,
            Response::Clipboard(clipboard::Response::error(clipboard::HISTORY_OFF))
        );
        let reply = dispatch(&routes, Request::AppLock(app_lock::Request::AppLockStatus));
        assert!(matches!(reply.response, Response::AppLock(r) if !r.ok));

        let handler: AppLockHandler = Box::new(|r| app_lock::Response {
            ok: matches!(r, app_lock::Request::AppLockStatus),
            has_password: true,
            ..Default::default()
        });
        lock(&routes).app_lock = Some(Arc::new(Mutex::new(handler)));
        let reply = dispatch(&routes, Request::AppLock(app_lock::Request::AppLockStatus));
        assert!(matches!(reply.response, Response::AppLock(r) if r.ok && r.has_password));
    }
}
