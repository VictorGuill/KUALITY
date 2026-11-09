//! Picks the update to offer from the GitHub releases list (`GET /repos/{repo}/releases`).

use serde::Deserialize;

use crate::Version;

/// Name of the checksum file published with every release.
pub const SUMS_ASSET: &str = "SHA256SUMS.txt";

/// A newer release that can be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release page on GitHub (notes, manual download).
    pub page_url: String,
    /// `Kuality-<version>-setup.exe`.
    pub setup_name: String,
    pub setup_url: String,
    pub sums_url: String,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// The newest release in `json` that is newer than `current`, if any.
///
/// Drafts and releases without the installer or the checksum file are skipped. Pre-releases are
/// only offered to a pre-release build (a beta tester gets the next beta or the final release;
/// a release build only gets releases).
pub fn pick_update(json: &[u8], current: &Version) -> Result<Option<Release>, String> {
    let releases: Vec<GhRelease> =
        serde_json::from_slice(json).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let allow_pre = current.is_prerelease();
    let best = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let version = Version::parse(&r.tag_name)?;
            if (version.is_prerelease() || r.prerelease) && !allow_pre {
                return None;
            }
            let setup_name = format!("Kuality-{version}-setup.exe");
            let url = |name: &str| {
                r.assets
                    .iter()
                    .find(|a| a.name == name)
                    .map(|a| a.browser_download_url.clone())
            };
            Some(Release {
                setup_url: url(&setup_name)?,
                sums_url: url(SUMS_ASSET)?,
                setup_name,
                page_url: r.html_url,
                version,
            })
        })
        .max_by(|a, b| a.version.cmp(&b.version));
    Ok(best.filter(|r| r.version > *current))
}

/// The SHA-256 (lowercase hex) of `file` in a `SHA256SUMS.txt` (`<hex>  <name>` lines).
pub fn expected_hash(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name == file && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str, pre: bool, draft: bool) -> serde_json::Value {
        let v = tag.trim_start_matches('v');
        json!({
            "tag_name": tag,
            "draft": draft,
            "prerelease": pre,
            "html_url": format!("https://github.com/o/r/releases/tag/{tag}"),
            "assets": [
                { "name": format!("Kuality-{v}-setup.exe"), "browser_download_url": format!("https://dl/{v}/setup") },
                { "name": format!("Kuality-{v}-portable.zip"), "browser_download_url": "https://dl/zip" },
                { "name": "SHA256SUMS.txt", "browser_download_url": format!("https://dl/{v}/sums") }
            ]
        })
    }

    fn pick(list: serde_json::Value, current: &str) -> Option<String> {
        let bytes = serde_json::to_vec(&list).unwrap();
        pick_update(&bytes, &Version::parse(current).unwrap())
            .unwrap()
            .map(|r| r.version.to_string())
    }

    #[test]
    fn beta_gets_newest_beta_or_release() {
        let list = json!([
            release("v1.0.0-beta.3", true, false),
            release("v1.0.0-beta.2", true, false),
            release("v1.0.0-beta.1", true, false)
        ]);
        assert_eq!(
            pick(list.clone(), "1.0.0-beta.1").as_deref(),
            Some("1.0.0-beta.3")
        );
        assert_eq!(pick(list, "1.0.0-beta.3"), None);
        let list = json!([
            release("v1.0.0", false, false),
            release("v1.0.0-beta.2", true, false)
        ]);
        assert_eq!(pick(list, "1.0.0-beta.1").as_deref(), Some("1.0.0"));
    }

    #[test]
    fn release_build_skips_prereleases() {
        let list = json!([
            release("v1.1.0-beta.1", true, false),
            release("v1.0.1", false, false)
        ]);
        assert_eq!(pick(list.clone(), "1.0.0").as_deref(), Some("1.0.1"));
        assert_eq!(pick(list, "1.0.1"), None);
    }

    #[test]
    fn skips_drafts_bad_tags_and_missing_assets() {
        let mut no_sums = release("v2.0.0", false, false);
        no_sums["assets"].as_array_mut().unwrap().pop();
        let list = json!([
            release("v3.0.0", false, true),
            no_sums,
            { "tag_name": "nightly", "assets": [] },
            release("v1.0.1", false, false)
        ]);
        assert_eq!(pick(list, "1.0.0").as_deref(), Some("1.0.1"));
    }

    #[test]
    fn release_fields() {
        let bytes = serde_json::to_vec(&json!([release("v1.0.0-beta.2", true, false)])).unwrap();
        let r = pick_update(&bytes, &Version::parse("1.0.0-beta.1").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(r.setup_name, "Kuality-1.0.0-beta.2-setup.exe");
        assert_eq!(r.setup_url, "https://dl/1.0.0-beta.2/setup");
        assert_eq!(r.sums_url, "https://dl/1.0.0-beta.2/sums");
        assert_eq!(
            r.page_url,
            "https://github.com/o/r/releases/tag/v1.0.0-beta.2"
        );
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(pick_update(b"{\"message\":\"x\"}", &Version::parse("1.0.0").unwrap()).is_err());
    }

    #[test]
    fn finds_expected_hash() {
        let h = "a".repeat(64);
        let sums = format!(
            "{h}  Kuality-1.0.0-setup.exe\r\n{}  Kuality-1.0.0-portable.zip\n",
            "b".repeat(64)
        );
        assert_eq!(expected_hash(&sums, "Kuality-1.0.0-setup.exe"), Some(h));
        assert_eq!(expected_hash(&sums, "other.exe"), None);
        assert_eq!(
            expected_hash("xyz  Kuality-1.0.0-setup.exe", "Kuality-1.0.0-setup.exe"),
            None
        );
    }
}
