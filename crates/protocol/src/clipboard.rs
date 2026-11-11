//! Clipboard history: the requests the popup / settings window send to the core over the pipe
//! ([`crate::ipc`]), and the decrypted layout of `clipboard-pins.bin` (`docs/specs/clipboard.md`).
//! OS-independent; the core encrypts and decrypts, these types never touch the disk.

use serde::{Deserialize, Serialize};

/// Characters of an entry's text sent with `list` (search and list rows); `get` sends all.
pub const LIST_TEXT_CHARS: usize = 2000;
/// Longest side of an image thumbnail (pixels).
pub const THUMB_PX: u32 = 96;

/// One request (JSON header of a frame with an empty payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// All entries (pinned first, then newest first). Payload of the reply: the thumbnails of
    /// image entries, back to back in entry order (`EntryInfo::thumb_len` bytes each).
    List,
    /// One entry in full. Payload: its text (UTF-8) or PNG.
    Get {
        id: u64,
    },
    /// Puts the entry on the clipboard; once the requesting process exits, the core gives the
    /// focus back to the window that was in front when the hotkey fired and sends Ctrl+V.
    Paste {
        id: u64,
    },
    /// Puts the entry on the clipboard only.
    Copy {
        id: u64,
    },
    Pin {
        id: u64,
    },
    Unpin {
        id: u64,
    },
    Delete {
        id: u64,
    },
    /// Removes every unpinned entry.
    Clear,
    /// Unpins and removes every pinned entry.
    ClearPins,
}

/// What an entry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Text,
    Image,
}

/// An entry as listed to clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryInfo {
    pub id: u64,
    pub kind: EntryKind,
    pub pinned: bool,
    /// When it was (last) copied, Unix seconds.
    pub created: u64,
    /// Text entries: the first [`LIST_TEXT_CHARS`] characters (all of it from `get`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `text` is cut: `get` returns the rest.
    #[serde(default)]
    pub truncated: bool,
    /// Text: characters and lines of the whole text.
    #[serde(default)]
    pub chars: u64,
    #[serde(default)]
    pub lines: u64,
    /// Image: size in pixels.
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Image: bytes of its PNG thumbnail in the `list` payload.
    #[serde(default)]
    pub thumb_len: u32,
}

/// Reply header.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `list`: all entries; `get`: the one entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<EntryInfo>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            ok: true,
            ..Self::default()
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            entries: Vec::new(),
        }
    }
}

/// First bytes of a decrypted pins payload.
pub const PINS_MAGIC: &[u8; 4] = b"KCLP";
/// Version written. 1 had no thumbnails (still read: they are rebuilt from the images).
pub const PINS_VERSION: u32 = 2;

/// One pinned entry in `clipboard-pins.bin`; its data, then its thumbnail, follow the header in
/// header order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinRecord {
    pub kind: EntryKind,
    pub created: u64,
    /// Bytes of data (UTF-8 text or PNG).
    pub len: u64,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Image: bytes of its PNG thumbnail (0: none stored), so loading the pins never decodes
    /// the images.
    #[serde(default)]
    pub thumb_len: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PinsHeader {
    entries: Vec<PinRecord>,
}

/// A record of a pins payload with where its bytes are in that payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedPin {
    pub record: PinRecord,
    pub data: std::ops::Range<usize>,
    /// Empty when no thumbnail is stored.
    pub thumb: std::ops::Range<usize>,
}

/// Builds the plain pins payload from `(record, data, thumbnail)` (the caller encrypts it, then
/// wipes the buffer).
pub fn encode_pins<'a>(pins: impl IntoIterator<Item = (PinRecord, &'a [u8], &'a [u8])>) -> Vec<u8> {
    let (records, parts): (Vec<PinRecord>, Vec<[&[u8]; 2]>) = pins
        .into_iter()
        .map(|(mut r, data, thumb)| {
            r.len = data.len() as u64;
            r.thumb_len = thumb.len() as u64;
            (r, [data, thumb])
        })
        .unzip();
    let header = serde_json::to_vec(&PinsHeader { entries: records }).unwrap_or_default();
    let total = 12 + header.len() + parts.iter().flatten().map(|d| d.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(PINS_MAGIC);
    out.extend_from_slice(&PINS_VERSION.to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    for part in parts.iter().flatten() {
        out.extend_from_slice(part);
    }
    out
}

/// Parses a plain pins payload: each record with the ranges of its data and thumbnail in
/// `bytes`.
pub fn decode_pins(bytes: &[u8]) -> Result<Vec<DecodedPin>, String> {
    if bytes.len() < 12 || &bytes[..4] != PINS_MAGIC {
        return Err("not a pins file".into());
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap_or_default());
    if !(1..=PINS_VERSION).contains(&version) {
        return Err(format!("unsupported pins version {version}"));
    }
    let header_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap_or_default()) as usize;
    let header_end = 12usize
        .checked_add(header_len)
        .filter(|&e| e <= bytes.len())
        .ok_or("truncated header")?;
    let header: PinsHeader =
        serde_json::from_slice(&bytes[12..header_end]).map_err(|e| e.to_string())?;
    let mut at = header_end;
    let mut take = |len: u64| {
        let end = usize::try_from(len)
            .ok()
            .and_then(|l| at.checked_add(l))
            .filter(|&e| e <= bytes.len())
            .ok_or("truncated data")?;
        let range = at..end;
        at = end;
        Ok::<_, String>(range)
    };
    let mut out = Vec::with_capacity(header.entries.len());
    for mut record in header.entries {
        if version < 2 {
            // No thumbnails in that layout, whatever the header says.
            record.thumb_len = 0;
        }
        let data = take(record.len)?;
        let thumb = take(record.thumb_len)?;
        out.push(DecodedPin {
            record,
            data,
            thumb,
        });
    }
    if at != bytes.len() {
        return Err("trailing data".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(kind: EntryKind) -> PinRecord {
        PinRecord {
            kind,
            created: 1_794_000_000,
            len: 0,
            width: 0,
            height: 0,
            thumb_len: 0,
        }
    }

    #[test]
    fn requests_use_an_op_tag() {
        assert_eq!(
            serde_json::to_value(Request::Paste { id: 7 }).unwrap(),
            json!({ "op": "paste", "id": 7 })
        );
        assert_eq!(
            serde_json::from_value::<Request>(json!({ "op": "clear_pins" })).unwrap(),
            Request::ClearPins
        );
        assert!(serde_json::from_value::<Request>(json!({ "op": "format_disk" })).is_err());
    }

    #[test]
    fn response_helpers() {
        let e = Response::error("nope");
        assert!(!e.ok);
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({ "ok": false, "error": "nope" })
        );
        assert_eq!(
            serde_json::to_value(Response::ok()).unwrap(),
            json!({ "ok": true })
        );
    }

    #[test]
    fn pins_round_trip() {
        let image = PinRecord {
            width: 2,
            height: 1,
            ..rec(EntryKind::Image)
        };
        let none = &b""[..];
        let bytes = encode_pins([
            (rec(EntryKind::Text), "héllo".as_bytes(), none),
            (image.clone(), &[1u8, 2, 3][..], &[9u8, 8][..]),
            (rec(EntryKind::Text), none, none),
        ]);
        let pins = decode_pins(&bytes).unwrap();
        assert_eq!(pins.len(), 3);
        assert_eq!(&bytes[pins[0].data.clone()], "héllo".as_bytes());
        assert_eq!(pins[0].record.len, 6);
        assert!(pins[0].thumb.is_empty());
        let expected = PinRecord {
            len: 3,
            thumb_len: 2,
            ..image
        };
        assert_eq!(pins[1].record, expected);
        assert_eq!(&bytes[pins[1].data.clone()], &[1, 2, 3]);
        assert_eq!(&bytes[pins[1].thumb.clone()], &[9, 8]);
        assert!(pins[2].data.is_empty() && pins[2].thumb.is_empty());
        assert_eq!(decode_pins(&encode_pins([])).unwrap(), vec![]);
    }

    #[test]
    fn version_1_pins_have_no_thumbnails() {
        // The layout written before thumbnails were stored: header, then only the data.
        let header = br#"{"entries":[{"kind":"image","created":5,"len":3,"width":2,"height":1}]}"#;
        let mut v1 = PINS_MAGIC.to_vec();
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&(header.len() as u32).to_le_bytes());
        v1.extend_from_slice(header);
        v1.extend_from_slice(&[1, 2, 3]);
        let pins = decode_pins(&v1).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(&v1[pins[0].data.clone()], &[1, 2, 3]);
        assert!(pins[0].thumb.is_empty());
        assert_eq!((pins[0].record.width, pins[0].record.thumb_len), (2, 0));
    }

    #[test]
    fn bad_pins_payloads_fail() {
        let good = encode_pins([(rec(EntryKind::Text), &b"abc"[..], &b""[..])]);
        assert!(decode_pins(b"").is_err());
        assert!(decode_pins(b"XXXX\x01\0\0\0\0\0\0\0").is_err());
        assert!(
            decode_pins(&good[..good.len() - 1]).is_err(),
            "truncated data"
        );
        let mut extra = good.clone();
        extra.push(0);
        assert!(decode_pins(&extra).is_err(), "trailing data");
        let mut newer = good.clone();
        newer[4] = 3;
        assert!(decode_pins(&newer).is_err(), "version");
        let mut zero = good;
        zero[4] = 0;
        assert!(decode_pins(&zero).is_err(), "version");
    }
}
