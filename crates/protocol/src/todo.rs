//! `todos.json`: the to-do items (see `docs/specs/todo.md`). OS-independent.
//!
//! Kept apart from `config.json` because it is data, written on every change by the to-do
//! popup (the only writer); the core and the settings window only read it.
//! Same rules as the config: `#[serde(default)]` everywhere, unknown fields kept in `extra`.

use serde::{Deserialize, Serialize};

use crate::config::Extra;

/// Highest `todos.json` format version this build understands.
pub const TODO_VERSION: u32 = 1;
/// Longest item text, in characters.
pub const MAX_TODO_TEXT: usize = 500;

/// The whole `todos.json` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TodoList {
    pub version: u32,
    /// In list order; done items stay where they are (the popup shows them after pending ones).
    pub items: Vec<TodoItem>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for TodoList {
    fn default() -> Self {
        Self {
            version: TODO_VERSION,
            items: Vec::new(),
            extra: Extra::new(),
        }
    }
}

/// One to-do item.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TodoItem {
    /// Unique within the file.
    pub id: u64,
    pub text: String,
    pub done: bool,
    /// Unix seconds.
    pub created: u64,
    /// Unix seconds; only while done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_at: Option<u64>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl TodoList {
    /// Parses a list from JSON bytes.
    pub fn from_json(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    /// Serializes the list as pretty JSON with a trailing newline.
    pub fn to_json_pretty(&self) -> serde_json::Result<Vec<u8>> {
        let mut out = serde_json::to_vec_pretty(self)?;
        out.push(b'\n');
        Ok(out)
    }

    /// Number of items not done yet.
    pub fn pending(&self) -> usize {
        self.items.iter().filter(|i| !i.done).count()
    }

    /// Adds an item (text cleaned with [`clean_text`]) at the top or the bottom of the list.
    /// Returns its id, or `None` when the text is empty.
    pub fn add(&mut self, text: &str, at_top: bool, now: u64) -> Option<u64> {
        let text = clean_text(text)?;
        let id = self.items.iter().map(|i| i.id).max().map_or(1, |m| m + 1);
        let item = TodoItem {
            id,
            text,
            created: now,
            ..TodoItem::default()
        };
        if at_top {
            self.items.insert(0, item);
        } else {
            self.items.push(item);
        }
        Some(id)
    }

    /// Marks item `id` done or not done. Returns whether something changed.
    pub fn set_done(&mut self, id: u64, done: bool, now: u64) -> bool {
        match self.items.iter_mut().find(|i| i.id == id) {
            Some(item) if item.done != done => {
                item.done = done;
                item.done_at = done.then_some(now);
                true
            }
            _ => false,
        }
    }

    /// Removes item `id` for good. Returns whether it existed.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.id != id);
        self.items.len() != before
    }

    /// Items in display order: pending ones first, then done ones, each group in list order.
    pub fn display_order(&self) -> Vec<&TodoItem> {
        let (pending, done): (Vec<_>, Vec<_>) = self.items.iter().partition(|i| !i.done);
        pending.into_iter().chain(done).collect()
    }
}

/// One line, trimmed, at most [`MAX_TODO_TEXT`] characters; `None` when nothing is left.
pub fn clean_text(text: &str) -> Option<String> {
    let one_line: String = text
        .split(['\r', '\n', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let cut: String = one_line.chars().take(MAX_TODO_TEXT).collect();
    let cut = cut.trim_end();
    (!cut.is_empty()).then(|| cut.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_object_is_an_empty_list() {
        let l = TodoList::from_json(b"{}").unwrap();
        assert_eq!(l, TodoList::default());
        assert_eq!(l.pending(), 0);
    }

    #[test]
    fn add_toggle_remove() {
        let mut l = TodoList::default();
        assert_eq!(l.add("  first ", true, 10), Some(1));
        assert_eq!(l.add("second", true, 11), Some(2));
        assert_eq!(l.add("last", false, 12), Some(3));
        assert_eq!(l.add(" \n ", true, 13), None);
        let texts: Vec<_> = l.items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, ["second", "first", "last"]);
        assert_eq!(l.pending(), 3);

        assert!(l.set_done(2, true, 20));
        assert!(!l.set_done(2, true, 21), "already done");
        assert_eq!(l.items[0].done_at, Some(20));
        assert_eq!(l.pending(), 2);
        let shown: Vec<_> = l.display_order().iter().map(|i| i.id).collect();
        assert_eq!(shown, [1, 3, 2], "done items last");

        assert!(l.set_done(2, false, 22));
        assert_eq!(l.items[0].done_at, None);
        assert!(l.remove(1));
        assert!(!l.remove(1));
        assert!(!l.set_done(99, true, 0));
        // Ids are never reused while a higher one exists.
        assert_eq!(l.add("again", true, 30), Some(4));
    }

    #[test]
    fn text_is_cleaned() {
        assert_eq!(clean_text("a\r\nb\tc"), Some("a b c".into()));
        assert_eq!(clean_text("   "), None);
        let long = "x".repeat(MAX_TODO_TEXT + 10);
        assert_eq!(clean_text(&long).unwrap().chars().count(), MAX_TODO_TEXT);
    }

    #[test]
    fn unknown_fields_survive_and_done_at_is_omitted_when_none() {
        let input = json!({ "version": 1, "sort": "x", "items": [
            { "id": 3, "text": "Buy milk", "done": false, "created": 5, "tag": "home" },
            { "id": 2, "text": "Call bank", "done": true, "created": 1, "done_at": 4 }
        ] });
        let l: TodoList = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(l.pending(), 1);
        assert_eq!(l.items[0].extra["tag"], json!("home"));
        assert_eq!(serde_json::to_value(&l).unwrap(), input);
        let bytes = l.to_json_pretty().unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(TodoList::from_json(&bytes).unwrap(), l);
    }

    #[test]
    fn wrong_types_are_an_error() {
        assert!(TodoList::from_json(br#"{ "items": [{ "id": "a" }] }"#).is_err());
        assert!(TodoList::from_json(b"[").is_err());
    }
}
