//! Reading `todos.json` for the taskbar count (`docs/specs/todo.md`). The core never writes the
//! file: the to-do popup (`kuality.exe --todo`) is its only writer.

use std::fs;
use std::io;
use std::path::PathBuf;

use kuality_protocol::TodoList;

/// Number of pending items in a `todos.json` file: a missing file has none, an invalid one is
/// unknown (`None`: the bar shows the label without a count).
pub fn pending(bytes: Option<&[u8]>) -> Option<usize> {
    match bytes {
        None => Some(0),
        Some(b) => TodoList::from_json(b).ok().map(|l| l.pending()),
    }
}

/// Re-reads `todos.json` and reports the pending count when the file changed.
#[derive(Debug)]
pub struct TodoWatch {
    path: PathBuf,
    /// Last bytes seen (`Some(None)`: missing); `None` before the first read.
    last: Option<Option<Vec<u8>>>,
}

impl TodoWatch {
    pub fn new(path: PathBuf) -> Self {
        Self { path, last: None }
    }

    /// `Some(count)` when the file changed since the last call (always on the first call);
    /// `None` when unchanged or unreadable (the previous count stays).
    pub fn check(&mut self) -> Option<Option<usize>> {
        let bytes = match fs::read(&self.path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                log::debug!("cannot read {}: {e}", self.path.display());
                return None;
            }
        };
        if self.last.as_ref() == Some(&bytes) {
            return None;
        }
        let count = pending(bytes.as_deref());
        if count.is_none() {
            log::warn!(
                "{} is invalid; the taskbar shows no count",
                self.path.display()
            );
        }
        self.last = Some(bytes);
        Some(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuality_protocol::test_util::TempDir;

    #[test]
    fn counts_pending_items() {
        assert_eq!(pending(None), Some(0));
        assert_eq!(pending(Some(b"{}")), Some(0));
        assert_eq!(
            pending(Some(
                br#"{ "items": [{ "id": 1, "text": "a" }, { "id": 2, "text": "b", "done": true }] }"#
            )),
            Some(1)
        );
        assert_eq!(pending(Some(b"{ nope")), None);
    }

    #[test]
    fn reports_only_changes() {
        let dir = TempDir::new("todo-watch");
        let path = dir.0.join("todos.json");
        let mut w = TodoWatch::new(path.clone());
        assert_eq!(w.check(), Some(Some(0)), "first read, missing file");
        assert_eq!(w.check(), None);
        fs::write(&path, br#"{ "items": [{ "id": 1, "text": "a" }] }"#).unwrap();
        assert_eq!(w.check(), Some(Some(1)));
        assert_eq!(w.check(), None);
        fs::write(&path, b"{").unwrap();
        assert_eq!(w.check(), Some(None));
        fs::remove_file(&path).unwrap();
        assert_eq!(w.check(), Some(Some(0)));
    }
}
