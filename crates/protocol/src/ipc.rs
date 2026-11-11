//! Local IPC with the core: a named pipe per core process, one request per connection
//! (`docs/specs/clipboard.md`). OS-independent: framing over any `Read` / `Write`.
//!
//! A frame is `u32` LE header length, the JSON header, `u32` LE payload length, then the
//! payload (binary, may be empty). Requests use an empty payload.

use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Largest JSON header accepted.
pub const MAX_HEADER: usize = 16 * 1024 * 1024;
/// Largest binary payload accepted.
pub const MAX_PAYLOAD: usize = 256 * 1024 * 1024;

/// Pipe path served by the core with process id `core_pid`. The pid in the name means a client
/// that knows its core never talks to another process's pipe by mistake; clients also check
/// the server pid.
pub fn pipe_name(core_pid: u32) -> String {
    format!(r"\\.\pipe\Kuality.Core.{core_pid}")
}

/// Writes one frame: `header` as JSON, then `payload`.
pub fn write_frame<T: Serialize>(w: &mut impl Write, header: &T, payload: &[u8]) -> io::Result<()> {
    let header = serde_json::to_vec(header).map_err(io::Error::other)?;
    if header.len() > MAX_HEADER || payload.len() > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame too large",
        ));
    }
    w.write_all(&(header.len() as u32).to_le_bytes())?;
    w.write_all(&header)?;
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// Reads one frame: the parsed header and the payload.
pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<(T, Vec<u8>)> {
    let header = read_part(r, MAX_HEADER)?;
    let header = serde_json::from_slice(&header)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let payload = read_part(r, MAX_PAYLOAD)?;
    Ok((header, payload))
}

fn read_part(r: &mut impl Read, max: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame part too large",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn frame_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &json!({ "op": "list" }), b"").unwrap();
        write_frame(&mut buf, &json!({ "ok": true }), &[1, 2, 3]).unwrap();
        let mut r = buf.as_slice();
        let (h, p): (Value, _) = read_frame(&mut r).unwrap();
        assert_eq!(h, json!({ "op": "list" }));
        assert!(p.is_empty());
        let (h, p): (Value, _) = read_frame(&mut r).unwrap();
        assert_eq!(h, json!({ "ok": true }));
        assert_eq!(p, vec![1, 2, 3]);
        assert!(r.is_empty());
    }

    #[test]
    fn truncated_or_oversized_frames_fail() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &json!({ "a": 1 }), &[9; 10]).unwrap();
        for cut in [0, 3, 6, buf.len() - 1] {
            assert!(read_frame::<Value>(&mut &buf[..cut]).is_err(), "cut {cut}");
        }
        let mut huge = (MAX_HEADER as u32 + 1).to_le_bytes().to_vec();
        huge.extend_from_slice(b"{}");
        let err = read_frame::<Value>(&mut huge.as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let mut bad = 2u32.to_le_bytes().to_vec();
        bad.extend_from_slice(b"{x");
        bad.extend_from_slice(&0u32.to_le_bytes());
        assert!(read_frame::<Value>(&mut bad.as_slice()).is_err());
    }

    #[test]
    fn pipe_name_has_the_pid() {
        assert_eq!(pipe_name(42), r"\\.\pipe\Kuality.Core.42");
    }
}
