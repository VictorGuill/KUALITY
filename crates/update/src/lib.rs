//! Kuality updates (`docs/specs/updates.md`): asks GitHub for the newest release, compares it
//! with this build, downloads the installer, checks it against `SHA256SUMS.txt` and starts it in
//! update mode.
//!
//! Every call blocks (network): run them on a worker thread. Shared by the core (periodic check,
//! tray) and the settings window (General → Updates).

mod release;
mod version;
#[cfg(windows)]
mod windows;

use std::fs;
use std::path::{Path, PathBuf};

pub use release::{Release, SUMS_ASSET, expected_hash, pick_update};
pub use version::Version;

#[cfg(windows)]
use self::windows as platform;

/// GitHub repository that publishes the releases.
pub const REPO: &str = "VictorGuill/KUALITY";
/// Arguments for the downloaded Inno Setup installer: progress window only, no questions, and
/// `/update=1` so it keeps the start-at-logon entry as it is and starts Kuality again
/// (`installer/kuality.iss`).
pub const INSTALLER_ARGS: [&str; 4] = ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/update=1"];

/// Size limits: the releases list and the checksum file are small; the installer is ~6 MB.
const MAX_API_BYTES: usize = 4 << 20;
const MAX_SUMS_BYTES: usize = 64 << 10;
const MAX_SETUP_BYTES: usize = 256 << 20;

/// This build's version (the workspace version). Debug builds only: the environment variable
/// `KUALITY_UPDATE_AS_VERSION` pretends another version, to test the update flow.
pub fn current_version() -> Version {
    #[cfg(debug_assertions)]
    if let Some(v) = std::env::var("KUALITY_UPDATE_AS_VERSION")
        .ok()
        .and_then(|v| Version::parse(&v))
    {
        return v;
    }
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the workspace version is semver")
}

/// HTTP answer.
#[derive(Debug)]
pub(crate) struct Response {
    pub status: u32,
    pub body: Vec<u8>,
    pub etag: Option<String>,
}

/// Checks GitHub for a newer release. Keeps the last answer and its `ETag`, so repeated checks
/// are conditional requests (a `304 Not Modified` does not count against GitHub's rate limit).
#[derive(Debug, Default)]
pub struct Checker {
    etag: Option<String>,
    /// Result of the last successful check.
    last: Option<Option<Release>>,
}

impl Checker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The newest release newer than this build, or `None` when up to date.
    pub fn check(&mut self) -> Result<Option<Release>, String> {
        self.check_from(&current_version())
    }

    /// Like [`check`](Self::check), as if this build were `current` (tests, dev tools). Keep
    /// one `current` per checker: a `304` answer repeats the last result.
    pub fn check_from(&mut self, current: &Version) -> Result<Option<Release>, String> {
        let mut headers = vec![
            ("Accept", "application/vnd.github+json"),
            ("X-GitHub-Api-Version", "2022-11-28"),
        ];
        if let (Some(etag), Some(_)) = (&self.etag, &self.last) {
            headers.push(("If-None-Match", etag));
        }
        let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=20");
        let resp = platform::get(&url, &headers, MAX_API_BYTES)?;
        match resp.status {
            304 => Ok(self.last.clone().flatten()),
            200 => {
                let found = pick_update(&resp.body, current)?;
                self.etag = resp.etag;
                self.last = Some(found.clone());
                Ok(found)
            }
            403 | 429 => Err("GitHub's rate limit was reached, try again later".into()),
            s => Err(format!("GitHub answered HTTP {s}")),
        }
    }
}

/// Whether this exe is the installed copy, which the installer can replace. Portable and
/// development copies only get a link to the release page.
pub fn can_self_update() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    if dir.join("portable").exists() {
        return false;
    }
    platform::install_dir().is_some_and(|installed| same_dir(&installed, dir))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// Downloads the installer of `release`, checks its SHA-256 and starts it in update mode.
/// Returns once it is started: the installer closes Kuality, installs and starts it again.
pub fn download_and_install(release: &Release) -> Result<(), String> {
    let path = download_verified(release)?;
    std::process::Command::new(&path)
        .args(INSTALLER_ARGS)
        .spawn()
        .map_err(|e| format!("cannot start the installer: {e}"))?;
    log::info!("started {} {}", path.display(), INSTALLER_ARGS.join(" "));
    Ok(())
}

/// Downloads the installer of `release` into [`download_dir`] and checks it against the
/// release's `SHA256SUMS.txt`. Returns its path.
pub fn download_verified(release: &Release) -> Result<PathBuf, String> {
    let sums = download(&release.sums_url, MAX_SUMS_BYTES)?;
    let expected = expected_hash(&String::from_utf8_lossy(&sums), &release.setup_name)
        .ok_or_else(|| format!("{} is missing from {SUMS_ASSET}", release.setup_name))?;
    let setup = download(&release.setup_url, MAX_SETUP_BYTES)?;
    let actual = hex(&platform::sha256(&setup)?);
    if actual != expected {
        return Err("the downloaded installer does not match its checksum".into());
    }
    log::info!(
        "downloaded {} ({} bytes, sha256 ok)",
        release.setup_name,
        setup.len()
    );

    let dir = download_dir();
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(&release.setup_name);
    fs::write(&path, &setup).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

fn download(url: &str, max: usize) -> Result<Vec<u8>, String> {
    let resp = platform::get(url, &[("Accept", "application/octet-stream")], max)?;
    match resp.status {
        200 => Ok(resp.body),
        s => Err(format!("download failed (HTTP {s})")),
    }
}

/// Where the installer is downloaded (`%TEMP%\Kuality-update`).
pub fn download_dir() -> PathBuf {
    std::env::temp_dir().join("Kuality-update")
}

/// Deletes an installer left by an earlier update (best effort).
pub fn clean_downloads() {
    let dir = download_dir();
    if dir.exists() {
        let _ = fs::remove_dir_all(dir);
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(not(windows))]
mod platform {
    //! Updates only exist on Windows for now.
    use super::Response;
    use std::path::PathBuf;

    pub(crate) fn get(_: &str, _: &[(&str, &str)], _: usize) -> Result<Response, String> {
        Err("updates are not supported on this platform".into())
    }
    pub(crate) fn sha256(_: &[u8]) -> Result<[u8; 32], String> {
        Err("updates are not supported on this platform".into())
    }
    pub(crate) fn install_dir() -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_parses() {
        assert!(Version::parse(env!("CARGO_PKG_VERSION")).is_some());
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(hex(&[0x00, 0xab, 0x7f]), "00ab7f");
    }

    #[cfg(windows)]
    #[test]
    fn sha256_of_abc() {
        assert_eq!(
            hex(&platform::sha256(b"abc").unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn same_dir_ignores_case_and_trailing_slash() {
        assert!(same_dir(
            Path::new(r"C:\Some\Missing\Dir\"),
            Path::new(r"c:\some\missing\dir")
        ));
        assert!(!same_dir(Path::new(r"C:\a"), Path::new(r"C:\b")));
    }
}
