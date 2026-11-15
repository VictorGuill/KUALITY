//! App lock (`docs/specs/app-lock.md`): the requests the prompt and the settings window send to
//! the core over the pipe ([`crate::ipc`]), the decrypted layout of `app-lock.bin`, and the
//! rules shared by the core and the settings window (path matching, apps that cannot be
//! locked, password length). OS-independent.

use serde::{Deserialize, Serialize};

/// Shortest password accepted (characters).
pub const MIN_PASSWORD_CHARS: usize = 4;
/// Longest password accepted (characters); keeps requests far below the pipe's request limit.
pub const MAX_PASSWORD_CHARS: usize = 128;

/// One request (JSON header of a frame with an empty payload). Op names start with
/// `app_lock_` so they never collide with the clipboard requests on the same pipe.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Whether a password is set and how long until the next try is accepted.
    AppLockStatus,
    /// Unlocks the program `app` (exe path, as in `app_lock.apps`).
    AppLockUnlock { app: String, password: String },
    /// Checks the password (the settings tab).
    AppLockVerify { password: String },
    /// Checks the password, then the core runs `action` (tray Quit / Pause).
    AppLockConfirm {
        action: ConfirmAction,
        password: String,
    },
    /// Sets the first password (`current` empty) or changes it.
    AppLockSetPassword { current: String, new: String },
}

impl Request {
    /// The passwords the request carries (to wipe them once handled).
    pub fn passwords_mut(&mut self) -> Vec<&mut String> {
        match self {
            Self::AppLockStatus => Vec::new(),
            Self::AppLockUnlock { password, .. }
            | Self::AppLockVerify { password }
            | Self::AppLockConfirm { password, .. } => vec![password],
            Self::AppLockSetPassword { current, new } => vec![current, new],
        }
    }
}

/// Never shows a password (requests are logged at Debug).
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AppLockStatus => f.write_str("AppLockStatus"),
            Self::AppLockUnlock { app, .. } => write!(f, "AppLockUnlock {{ app: {app:?} }}"),
            Self::AppLockVerify { .. } => f.write_str("AppLockVerify"),
            Self::AppLockConfirm { action, .. } => write!(f, "AppLockConfirm({action:?})"),
            Self::AppLockSetPassword { .. } => f.write_str("AppLockSetPassword"),
        }
    }
}

/// A tray command that needs the password while App lock is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmAction {
    Quit,
    Pause,
}

impl ConfirmAction {
    /// The value of `--confirm`.
    pub fn as_arg(self) -> &'static str {
        match self {
            Self::Quit => "quit",
            Self::Pause => "pause",
        }
    }

    pub fn from_arg(arg: &str) -> Option<Self> {
        match arg {
            "quit" => Some(Self::Quit),
            "pause" => Some(Self::Pause),
            _ => None,
        }
    }
}

/// Reply header.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A password is set.
    #[serde(default)]
    pub has_password: bool,
    /// Tries are refused for this many more seconds (too many wrong passwords).
    #[serde(default)]
    pub retry_after_secs: u64,
}

impl Response {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            ..Self::default()
        }
    }
}

/// Why a new password is refused, if it is.
pub fn password_problem(new: &str, confirm: &str) -> Option<&'static str> {
    let chars = new.chars().count();
    if chars < MIN_PASSWORD_CHARS {
        Some("Use at least 4 characters.")
    } else if chars > MAX_PASSWORD_CHARS {
        Some("Use at most 128 characters.")
    } else if new != confirm {
        Some("The two passwords are different.")
    } else {
        None
    }
}

/// The form of an exe path used to compare paths: lowercase, `/` as `\`, trimmed.
pub fn normalize_path(path: &str) -> String {
    path.trim().replace('/', "\\").to_lowercase()
}

/// Exe names (lowercase) that cannot be locked: hiding them would hide parts of Windows itself.
const REFUSED_EXES: &[&str] = &[
    "explorer.exe",
    "applicationframehost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "searchapp.exe",
    "shellexperiencehost.exe",
    "shellhost.exe",
    "textinputhost.exe",
    "lockapp.exe",
    "logonui.exe",
    "consent.exe",
    "dwm.exe",
    "csrss.exe",
    "winlogon.exe",
    "sihost.exe",
    "taskhostw.exe",
    "systemsettings.exe",
];

/// Why `path` cannot be added to App lock, if it cannot. `own_exe`: Kuality's own exe.
pub fn refused_app(path: &str, own_exe: &str) -> Option<&'static str> {
    let norm = normalize_path(path);
    if norm.is_empty() {
        return Some("Choose a program.");
    }
    if !norm.ends_with(".exe") {
        return Some("Choose a program (.exe file).");
    }
    if norm == normalize_path(own_exe) || norm.ends_with("\\kuality.exe") {
        return Some("Kuality itself cannot be locked.");
    }
    let file = norm.rsplit('\\').next().unwrap_or_default();
    REFUSED_EXES
        .contains(&file)
        .then_some("This is part of Windows and cannot be locked.")
}

/// First bytes of a decrypted `app-lock.bin`.
pub const SECRET_MAGIC: &[u8; 4] = b"KALK";
/// Version written.
pub const SECRET_VERSION: u32 = 1;
pub const SALT_LEN: usize = 16;
pub const HASH_LEN: usize = 32;

/// The password record of `app-lock.bin`, decrypted.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    pub iterations: u32,
    pub salt: [u8; SALT_LEN],
    pub hash: [u8; HASH_LEN],
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret {{ iterations: {} }}", self.iterations)
    }
}

impl Secret {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + SALT_LEN + HASH_LEN);
        out.extend_from_slice(SECRET_MAGIC);
        out.extend_from_slice(&SECRET_VERSION.to_le_bytes());
        out.extend_from_slice(&self.iterations.to_le_bytes());
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.hash);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 12 + SALT_LEN + HASH_LEN || &bytes[..4] != SECRET_MAGIC {
            return Err("not an App lock password file".into());
        }
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let version = word(4);
        if version != SECRET_VERSION {
            return Err(format!("unknown version {version}"));
        }
        let iterations = word(8);
        if iterations == 0 {
            return Err("no iterations".into());
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&bytes[12..12 + SALT_LEN]);
        let mut hash = [0u8; HASH_LEN];
        hash.copy_from_slice(&bytes[12 + SALT_LEN..]);
        Ok(Self {
            iterations,
            salt,
            hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_use_their_own_op_names() {
        let r = Request::AppLockUnlock {
            app: "C:\\a.exe".into(),
            password: "pw".into(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["op"], json!("app_lock_unlock"));
        let back: Request = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
        let v = serde_json::to_value(Request::AppLockConfirm {
            action: ConfirmAction::Pause,
            password: "x".into(),
        })
        .unwrap();
        assert_eq!(v["action"], json!("pause"));
    }

    #[test]
    fn debug_never_shows_passwords() {
        let all = [
            Request::AppLockUnlock {
                app: "a".into(),
                password: "secret1".into(),
            },
            Request::AppLockVerify {
                password: "secret1".into(),
            },
            Request::AppLockConfirm {
                action: ConfirmAction::Quit,
                password: "secret1".into(),
            },
            Request::AppLockSetPassword {
                current: "secret1".into(),
                new: "secret1".into(),
            },
        ];
        for mut r in all {
            assert!(!format!("{r:?}").contains("secret1"));
            assert!(!r.passwords_mut().is_empty());
        }
    }

    #[test]
    fn new_password_rules() {
        assert!(password_problem("abc", "abc").is_some());
        assert!(password_problem("abcd", "abce").is_some());
        assert_eq!(password_problem("abcd", "abcd"), None);
        assert_eq!(
            password_problem("ñañá", "ñañá"),
            None,
            "characters, not bytes"
        );
        let long = "x".repeat(129);
        assert!(password_problem(&long, &long).is_some());
    }

    #[test]
    fn paths_and_refused_apps() {
        assert_eq!(
            normalize_path(" C:/Program Files/X.EXE "),
            "c:\\program files\\x.exe"
        );
        let own = "C:\\Tools\\Kuality\\kuality.exe";
        assert!(refused_app("", own).is_some());
        assert!(refused_app("C:\\a\\notes.txt", own).is_some());
        assert!(refused_app("c:/tools/kuality/KUALITY.exe", own).is_some());
        assert!(refused_app("D:\\other\\kuality.exe", own).is_some());
        assert!(refused_app("C:\\Windows\\explorer.exe", own).is_some());
        assert_eq!(refused_app("C:\\Windows\\System32\\notepad.exe", own), None);
        assert_eq!(refused_app("C:\\B\\brave.exe", own), None);
    }

    #[test]
    fn confirm_arguments() {
        for a in [ConfirmAction::Quit, ConfirmAction::Pause] {
            assert_eq!(ConfirmAction::from_arg(a.as_arg()), Some(a));
        }
        assert_eq!(ConfirmAction::from_arg("x"), None);
    }

    #[test]
    fn secret_round_trip() {
        let s = Secret {
            iterations: 600_000,
            salt: [7; SALT_LEN],
            hash: [9; HASH_LEN],
        };
        let bytes = s.encode();
        assert_eq!(Secret::decode(&bytes).unwrap(), s);
        assert!(Secret::decode(&bytes[1..]).is_err());
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(Secret::decode(&bad).is_err());
        let mut v2 = bytes.clone();
        v2[4] = 2;
        assert!(Secret::decode(&v2).is_err());
        assert!(!format!("{s:?}").contains('9'));
    }
}
