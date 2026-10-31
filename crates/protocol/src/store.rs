//! Writing `config.json`. OS-independent.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Writes `bytes` to `path` atomically: a temp file next to it (`<name>.tmp`), flushed to disk,
/// then renamed over `path`. Creates the parent folder if needed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(path);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // On Windows `fs::rename` uses MoveFileExW with MOVEFILE_REPLACE_EXISTING.
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// The temp file [`write_atomic`] uses for `path`.
pub fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn writes_replaces_and_leaves_no_temp_file() {
        let dir = TempDir::new("store");
        let path = dir.0.join("sub").join("config.json");
        write_atomic(&path, b"one").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!tmp_path(&path).exists());
        assert_eq!(tmp_path(&path).file_name().unwrap(), "config.json.tmp");
    }
}
