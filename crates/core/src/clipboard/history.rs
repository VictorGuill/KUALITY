//! The clipboard history (`docs/specs/clipboard.md`): ordering, duplicates, limits and pins.
//! Portable and unit-tested; every payload is kept [`Sealed`] (encrypted in memory).

use std::hash::{BuildHasher, RandomState};

use kuality_protocol::clipboard::{EntryInfo, EntryKind, LIST_TEXT_CHARS, PinRecord};

use super::sealed::{Cipher, Plain, Sealed};

/// Text entries larger than this (UTF-8 bytes) are not recorded.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// Unpinned images keep at most this many PNG bytes together; the oldest go first.
pub const IMAGE_BUDGET: usize = 64 * 1024 * 1024;

/// New clipboard content, decrypted (wiped when dropped).
#[derive(Debug)]
pub enum Content {
    Text(Plain),
    Image {
        png: Plain,
        thumb: Plain,
        width: u32,
        height: u32,
    },
}

/// What [`History::add`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    New(u64),
    /// Already in the history: moved to the top.
    Moved(u64),
    Skipped(Skip),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// Empty or whitespace-only text, or invalid UTF-8.
    Empty,
    TooLarge,
    /// Encryption failed: nothing is kept.
    Cipher,
}

struct Entry {
    id: u64,
    kind: EntryKind,
    pinned: bool,
    created: u64,
    /// Keyed (per-process random key) hash of kind + data, for duplicates.
    hash: u64,
    data: Sealed,
    /// Text longer than [`LIST_TEXT_CHARS`]: its first characters, so listing the history never
    /// decrypts a whole large entry.
    preview: Option<Sealed>,
    thumb: Option<Sealed>,
    width: u32,
    height: u32,
    chars: u64,
    lines: u64,
}

/// Entries, newest first (pinned ones are listed first by [`History::list`]).
pub struct History<C: Cipher> {
    cipher: C,
    entries: Vec<Entry>,
    next_id: u64,
    limit: usize,
    hasher: RandomState,
}

impl<C: Cipher> std::fmt::Debug for History<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "History({} entries)", self.entries.len())
    }
}

impl<C: Cipher> History<C> {
    /// `limit`: unpinned entries kept.
    pub fn new(cipher: C, limit: usize) -> Self {
        Self {
            cipher,
            entries: Vec::new(),
            next_id: 1,
            limit,
            hasher: RandomState::new(),
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        self.trim();
    }

    fn hash(&self, kind: EntryKind, data: &[u8]) -> u64 {
        self.hasher.hash_one((kind == EntryKind::Image, data))
    }

    fn index(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    /// Records new content at the top, or moves an equal entry there.
    pub fn add(&mut self, content: Content, now: u64) -> Added {
        let (kind, data) = match &content {
            Content::Text(t) => (EntryKind::Text, &**t),
            Content::Image { png, .. } => (EntryKind::Image, &**png),
        };
        match &content {
            Content::Text(t) => match t.as_str() {
                None => return Added::Skipped(Skip::Empty),
                Some(s) if s.trim().is_empty() => return Added::Skipped(Skip::Empty),
                Some(_) if t.len() > MAX_TEXT_BYTES => return Added::Skipped(Skip::TooLarge),
                Some(_) => {}
            },
            Content::Image { png, .. } if png.len() > IMAGE_BUDGET => {
                return Added::Skipped(Skip::TooLarge);
            }
            Content::Image { .. } => {}
        }
        let hash = self.hash(kind, data);
        let same = self.entries.iter().position(|e| {
            e.kind == kind
                && e.hash == hash
                && e.data.len() == data.len()
                && e.data.open(&self.cipher).is_some_and(|p| *p == *data)
        });
        if let Some(i) = same {
            let mut e = self.entries.remove(i);
            e.created = now;
            let id = e.id;
            self.entries.insert(0, e);
            return Added::Moved(id);
        }
        let Some(entry) = self.seal(content, hash, now) else {
            return Added::Skipped(Skip::Cipher);
        };
        let id = entry.id;
        self.entries.insert(0, entry);
        self.trim();
        Added::New(id)
    }

    fn seal(&mut self, content: Content, hash: u64, created: u64) -> Option<Entry> {
        let id = self.next_id;
        let entry = match &content {
            Content::Text(t) => {
                let s = t.as_str().unwrap_or_default();
                let preview = match s.char_indices().nth(LIST_TEXT_CHARS) {
                    Some((cut, _)) => Some(Sealed::seal(&self.cipher, &t[..cut])?),
                    None => None,
                };
                Entry {
                    id,
                    kind: EntryKind::Text,
                    pinned: false,
                    created,
                    hash,
                    data: Sealed::seal(&self.cipher, t)?,
                    preview,
                    thumb: None,
                    width: 0,
                    height: 0,
                    chars: s.chars().count() as u64,
                    lines: s.lines().count().max(1) as u64,
                }
            }
            Content::Image {
                png,
                thumb,
                width,
                height,
            } => Entry {
                id,
                kind: EntryKind::Image,
                pinned: false,
                created,
                hash,
                data: Sealed::seal(&self.cipher, png)?,
                preview: None,
                thumb: Some(Sealed::seal(&self.cipher, thumb)?),
                width: *width,
                height: *height,
                chars: 0,
                lines: 0,
            },
        };
        self.next_id += 1;
        Some(entry)
    }

    /// Drops the oldest unpinned entries above the limit, then the oldest unpinned images above
    /// the image budget.
    fn trim(&mut self) {
        let mut unpinned = 0usize;
        let mut image_bytes = 0usize;
        let limit = self.limit;
        // Newest first: keep while within both limits.
        self.entries.retain(|e| {
            if e.pinned {
                return true;
            }
            if unpinned >= limit {
                return false;
            }
            if e.kind == EntryKind::Image {
                if image_bytes + e.data.len() > IMAGE_BUDGET {
                    return false;
                }
                image_bytes += e.data.len();
            }
            unpinned += 1;
            true
        });
    }

    /// Moves an entry to the top (it was just put on the clipboard again).
    pub fn touch(&mut self, id: u64, now: u64) -> bool {
        let Some(i) = self.index(id) else {
            return false;
        };
        let mut e = self.entries.remove(i);
        e.created = now;
        self.entries.insert(0, e);
        true
    }

    /// Returns whether the pins changed.
    pub fn set_pinned(&mut self, id: u64, pinned: bool) -> bool {
        let Some(i) = self.index(id) else {
            return false;
        };
        if self.entries[i].pinned == pinned {
            return false;
        }
        self.entries[i].pinned = pinned;
        if !pinned {
            self.trim();
        }
        true
    }

    /// Removes an entry. `Some(was_pinned)` when it existed.
    pub fn delete(&mut self, id: u64) -> Option<bool> {
        let i = self.index(id)?;
        Some(self.entries.remove(i).pinned)
    }

    pub fn clear_unpinned(&mut self) {
        self.entries.retain(|e| e.pinned);
    }

    /// Removes the pinned entries. Returns whether there were any.
    pub fn clear_pins(&mut self) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| !e.pinned);
        self.entries.len() != before
    }

    fn info(&self, e: &Entry, full_text: bool) -> EntryInfo {
        let text = (e.kind == EntryKind::Text).then(|| {
            // The preview is the whole text unless it is longer than `LIST_TEXT_CHARS`.
            let sealed = match &e.preview {
                Some(preview) if !full_text => preview,
                _ => &e.data,
            };
            let plain = sealed.open(&self.cipher).unwrap_or_default();
            plain.as_str().unwrap_or_default().to_string()
        });
        EntryInfo {
            id: e.id,
            kind: e.kind,
            pinned: e.pinned,
            created: e.created,
            truncated: !full_text && e.chars > LIST_TEXT_CHARS as u64,
            text,
            chars: e.chars,
            lines: e.lines,
            width: e.width,
            height: e.height,
            thumb_len: 0,
        }
    }

    /// Display order: pinned first, then the rest, each newest first.
    fn ordered(&self) -> impl Iterator<Item = &Entry> {
        let pinned = self.entries.iter().filter(|e| e.pinned);
        pinned.chain(self.entries.iter().filter(|e| !e.pinned))
    }

    /// All entries in display order (text cut to [`LIST_TEXT_CHARS`]) and the image thumbnails
    /// back to back.
    pub fn list(&self) -> (Vec<EntryInfo>, Plain) {
        let mut thumbs = Plain::default();
        let infos = self
            .ordered()
            .map(|e| {
                let mut info = self.info(e, false);
                if let Some(t) = e.thumb.as_ref().and_then(|t| t.open(&self.cipher)) {
                    info.thumb_len = t.len() as u32;
                    thumbs.as_mut_vec().extend_from_slice(&t);
                }
                info
            })
            .collect();
        (infos, thumbs)
    }

    /// One entry with its whole data (text UTF-8 or PNG).
    pub fn get(&self, id: u64) -> Option<(EntryInfo, Plain)> {
        let e = &self.entries[self.index(id)?];
        let data = e.data.open(&self.cipher)?;
        let mut info = self.info(e, true);
        if e.kind == EntryKind::Image {
            info.text = None;
        }
        Some((info, data))
    }

    /// The pinned entries with their data and thumbnail (empty for text), for
    /// `clipboard-pins.bin` (display order).
    pub fn pin_records(&self) -> Vec<(PinRecord, Plain, Plain)> {
        self.ordered()
            .filter(|e| e.pinned)
            .filter_map(|e| {
                let data = e.data.open(&self.cipher)?;
                let thumb = match &e.thumb {
                    Some(t) => t.open(&self.cipher)?,
                    None => Plain::default(),
                };
                let record = PinRecord {
                    kind: e.kind,
                    created: e.created,
                    len: data.len() as u64,
                    width: e.width,
                    height: e.height,
                    thumb_len: thumb.len() as u64,
                };
                Some((record, data, thumb))
            })
            .collect()
    }

    /// Adds pinned entries loaded from disk, keeping their order and times.
    pub fn restore_pins(&mut self, pins: Vec<(u64, Content)>) {
        let mut restored = Vec::new();
        for (created, content) in pins {
            let (kind, data) = match &content {
                Content::Text(t) => (EntryKind::Text, &**t),
                Content::Image { png, .. } => (EntryKind::Image, &**png),
            };
            let hash = self.hash(kind, data);
            if let Some(mut e) = self.seal(content, hash, created) {
                e.pinned = true;
                restored.push(e);
            }
        }
        // Keep them ahead of anything copied since the start.
        self.entries.splice(0..0, restored);
    }
}

#[cfg(test)]
mod tests {
    use super::super::sealed::XorCipher;
    use super::*;

    fn text(s: &str) -> Content {
        Content::Text(Plain::new(s.as_bytes().to_vec()))
    }

    fn image(seed: u8, len: usize) -> Content {
        Content::Image {
            png: Plain::new(vec![seed; len]),
            thumb: Plain::new(vec![seed; 3]),
            width: 10,
            height: 5,
        }
    }

    fn ids(h: &History<XorCipher>) -> Vec<u64> {
        h.list().0.iter().map(|e| e.id).collect()
    }

    #[test]
    fn newest_first_and_duplicates_move_to_top() {
        let mut h = History::new(XorCipher, 100);
        assert_eq!(h.add(text("a"), 1), Added::New(1));
        assert_eq!(h.add(text("b"), 2), Added::New(2));
        assert_eq!(h.add(text("a"), 3), Added::Moved(1));
        assert_eq!(ids(&h), vec![1, 2]);
        let (list, _) = h.list();
        assert_eq!(list[0].created, 3);
        assert_eq!(list[0].text.as_deref(), Some("a"));
        // Same bytes but another kind is not a duplicate.
        assert_eq!(h.add(image(b'a', 1), 4), Added::New(3));
    }

    #[test]
    fn empty_large_and_invalid_text_is_skipped() {
        let mut h = History::new(XorCipher, 100);
        assert_eq!(h.add(text(" \n\t"), 1), Added::Skipped(Skip::Empty));
        assert_eq!(
            h.add(Content::Text(Plain::new(vec![0xFF, 0xFE])), 1),
            Added::Skipped(Skip::Empty)
        );
        let big = "x".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(h.add(text(&big), 1), Added::Skipped(Skip::TooLarge));
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn limit_ignores_pins() {
        let mut h = History::new(XorCipher, 2);
        h.add(text("1"), 1);
        h.add(text("2"), 2);
        assert!(h.set_pinned(1, true));
        assert!(!h.set_pinned(1, true), "no change");
        h.add(text("3"), 3);
        h.add(text("4"), 4);
        // Pinned "1" first, then the two newest unpinned.
        assert_eq!(ids(&h), vec![1, 4, 3]);
        h.set_limit(1);
        assert_eq!(ids(&h), vec![1, 4]);
        // Unpinning trims again.
        assert!(h.set_pinned(1, false));
        assert_eq!(ids(&h), vec![4]);
    }

    #[test]
    fn image_budget_drops_oldest_unpinned_images() {
        let third = IMAGE_BUDGET / 3 + 1;
        let mut h = History::new(XorCipher, 100);
        h.add(image(1, third), 1);
        h.add(text("t"), 2);
        h.add(image(2, third), 3);
        h.set_pinned(3, true);
        h.add(image(3, third), 4);
        h.add(image(4, third), 5);
        // 1, 3 (pinned), 4 and 5 are images; unpinned 4 + 5 fit, 1 does not.
        assert_eq!(ids(&h), vec![3, 5, 4, 2]);
        assert_eq!(
            h.add(image(9, IMAGE_BUDGET + 1), 6),
            Added::Skipped(Skip::TooLarge)
        );
    }

    #[test]
    fn list_cuts_text_and_get_returns_all() {
        let mut h = History::new(XorCipher, 100);
        let long: String = "é".repeat(LIST_TEXT_CHARS + 5);
        h.add(text(&long), 1);
        h.add(image(7, 8), 2);
        h.add(text("two\nlines"), 3);
        let (list, thumbs) = h.list();
        assert_eq!(list[0].lines, 2);
        assert_eq!(list[1].kind, EntryKind::Image);
        assert_eq!(
            (list[1].width, list[1].height, list[1].thumb_len),
            (10, 5, 3)
        );
        assert_eq!(&*thumbs, &[7, 7, 7]);
        let cut = &list[2];
        assert!(cut.truncated);
        assert_eq!(cut.chars, LIST_TEXT_CHARS as u64 + 5);
        assert_eq!(cut.text.as_ref().unwrap().chars().count(), LIST_TEXT_CHARS);
        let (info, data) = h.get(1).unwrap();
        assert!(!info.truncated);
        assert_eq!(info.text.as_deref(), Some(long.as_str()));
        assert_eq!(data.as_str(), Some(long.as_str()));
        let (info, data) = h.get(2).unwrap();
        assert_eq!(info.text, None);
        assert_eq!(&*data, &[7; 8]);
        assert!(h.get(99).is_none());
    }

    #[test]
    fn delete_touch_and_clear() {
        let mut h = History::new(XorCipher, 100);
        h.add(text("a"), 1);
        h.add(text("b"), 2);
        h.add(text("c"), 3);
        h.set_pinned(1, true);
        assert!(h.touch(2, 9));
        assert_eq!(ids(&h), vec![1, 2, 3]);
        assert_eq!(h.delete(3), Some(false));
        assert_eq!(h.delete(3), None);
        h.clear_unpinned();
        assert_eq!(ids(&h), vec![1]);
        assert!(h.clear_pins());
        assert!(!h.clear_pins());
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn pins_round_trip_through_records() {
        let mut h = History::new(XorCipher, 100);
        h.add(text("keep"), 5);
        h.add(image(4, 6), 6);
        h.add(text("drop"), 7);
        h.set_pinned(1, true);
        h.set_pinned(2, true);
        let records = h.pin_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0.kind, EntryKind::Image);
        assert_eq!(records[0].0.len, 6);
        assert_eq!((records[0].0.thumb_len, &*records[0].2), (3, &[4u8; 3][..]));
        assert_eq!(records[1].1.as_str(), Some("keep"));
        assert!(records[1].2.is_empty());

        let mut fresh = History::new(XorCipher, 100);
        fresh.add(text("new"), 10);
        fresh.restore_pins(
            records
                .into_iter()
                .map(|(r, data, thumb)| {
                    let content = match r.kind {
                        EntryKind::Text => Content::Text(data),
                        EntryKind::Image => Content::Image {
                            png: data,
                            thumb,
                            width: r.width,
                            height: r.height,
                        },
                    };
                    (r.created, content)
                })
                .collect(),
        );
        let (list, thumbs) = fresh.list();
        assert!(list[0].pinned && list[1].pinned && !list[2].pinned);
        assert_eq!((list[0].thumb_len, &*thumbs), (3, &[4u8; 3][..]));
        assert_eq!(list[1].text.as_deref(), Some("keep"));
        assert_eq!(list[1].created, 5);
        // A pinned entry copied again is a duplicate of the pin, which stays pinned.
        assert!(matches!(fresh.add(text("keep"), 11), Added::Moved(_)));
        assert!(fresh.list().0[0].pinned);
    }
}
