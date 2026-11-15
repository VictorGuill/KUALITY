//! The App lock password (`docs/specs/app-lock.md`): `app-lock.bin` (PBKDF2-HMAC-SHA256 hash,
//! DPAPI-encrypted for the current user) and the limit on wrong tries.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kuality_protocol::app_lock::{
    HASH_LEN, MAX_PASSWORD_CHARS, SALT_LEN, Secret, password_problem,
};
use kuality_protocol::store;

use crate::clipboard::sealed::Plain;
use crate::platform;

/// PBKDF2 iterations of a new password (OWASP's figure for PBKDF2-HMAC-SHA256).
pub const ITERATIONS: u32 = 600_000;
/// DPAPI entropy of `app-lock.bin` (not a secret; it ties the blob to this use).
const ENTROPY: &[u8] = b"Kuality.AppLock.v1";
/// Wrong tries allowed before tries are refused for a while.
const FREE_TRIES: u32 = 5;
const FIRST_WAIT: Duration = Duration::from_secs(30);
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);

/// How long tries are refused after `failures` wrong passwords in a row: nothing for the first
/// 4, then 30 s, 60 s, 120 s… up to 15 min.
pub fn wait_after(failures: u32) -> Option<Duration> {
    let extra = failures.checked_sub(FREE_TRIES)?;
    let factor = 1u32.checked_shl(extra.min(16)).unwrap_or(u32::MAX);
    Some(FIRST_WAIT.saturating_mul(factor).min(MAX_WAIT))
}

/// Wrong tries in a row and until when tries are refused. Memory only: a restart starts over.
#[derive(Debug, Default)]
pub struct Throttle {
    failures: u32,
    until: Option<Instant>,
}

impl Throttle {
    /// How long tries are still refused at `now`.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        self.until
            .and_then(|u| u.checked_duration_since(now))
            .filter(|d| !d.is_zero())
    }

    pub fn failed(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        self.until = wait_after(self.failures).map(|d| now + d);
    }

    pub fn succeeded(&mut self) {
        *self = Self::default();
    }
}

/// Why a password was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// No password is set.
    NoPassword,
    Wrong,
    /// Too many wrong tries: try again after this long.
    Wait(Duration),
    /// The new password breaks a rule.
    Invalid(&'static str),
    /// Hashing or writing the file failed.
    Failed(String),
}

impl Refused {
    /// The text shown to the user.
    pub fn message(&self) -> String {
        match self {
            Self::NoPassword => "No App lock password is set yet.".into(),
            Self::Wrong => "Wrong password.".into(),
            Self::Wait(d) => format!(
                "Too many wrong passwords. Try again in {}.",
                human_secs(d.as_secs().max(1))
            ),
            Self::Invalid(why) => (*why).into(),
            Self::Failed(e) => format!("Something went wrong: {e}"),
        }
    }
}

/// `"45 s"`, `"2 min"`, `"2 min 30 s"`.
fn human_secs(s: u64) -> String {
    match (s / 60, s % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
}

/// The password record and the throttle, owned by the pipe handler.
#[derive(Debug)]
pub struct Vault {
    path: PathBuf,
    secret: Option<Secret>,
    throttle: Throttle,
    /// Iterations of a new password ([`ITERATIONS`]; fewer in tests).
    iterations: u32,
}

impl Vault {
    /// Reads `app-lock.bin`. A missing file means no password; an unreadable one is logged and
    /// treated the same (it is replaced when a password is set).
    pub fn load(path: PathBuf) -> Self {
        let secret = match read_secret(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!(
                    "cannot read {}: {e}; App lock has no password until a new one is set",
                    path.display()
                );
                None
            }
        };
        Self {
            path,
            secret,
            throttle: Throttle::default(),
            iterations: ITERATIONS,
        }
    }

    pub fn has_password(&self) -> bool {
        self.secret.is_some()
    }

    /// How long tries are still refused.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        self.throttle.wait(now)
    }

    /// Checks `password`; every wrong try counts toward the throttle.
    pub fn check(&mut self, password: &str, now: Instant) -> Result<(), Refused> {
        let Some(secret) = &self.secret else {
            return Err(Refused::NoPassword);
        };
        if let Some(d) = self.throttle.wait(now) {
            return Err(Refused::Wait(d));
        }
        // Longer than any password that can be set: wrong without hashing it.
        let right = password.chars().count() <= MAX_PASSWORD_CHARS
            && hash(password, &secret.salt, secret.iterations)
                .map_err(|e| Refused::Failed(e.to_string()))?
                .ct_eq(&secret.hash);
        if right {
            self.throttle.succeeded();
            Ok(())
        } else {
            self.throttle.failed(now);
            Err(Refused::Wrong)
        }
    }

    /// Sets the first password (`current` is ignored then) or changes it (`current` must be
    /// right). Returns whether this was the first password.
    pub fn set(&mut self, current: &str, new: &str, now: Instant) -> Result<bool, Refused> {
        let first = self.secret.is_none();
        if !first {
            self.check(current, now)?;
        }
        if let Some(why) = password_problem(new, new) {
            return Err(Refused::Invalid(why));
        }
        let mut salt = [0u8; SALT_LEN];
        platform::crypto::random_bytes(&mut salt).map_err(|e| Refused::Failed(e.to_string()))?;
        let hashed =
            hash(new, &salt, self.iterations).map_err(|e| Refused::Failed(e.to_string()))?;
        let secret = Secret {
            iterations: self.iterations,
            salt,
            hash: hashed.0,
        };
        let plain = Plain::new(secret.encode());
        platform::crypto::dpapi_protect(&plain, ENTROPY)
            .and_then(|blob| store::write_atomic(&self.path, &blob))
            .map_err(|e| Refused::Failed(e.to_string()))?;
        self.secret = Some(secret);
        log::info!(
            "App lock password {}",
            if first { "set" } else { "changed" }
        );
        Ok(first)
    }
}

/// A PBKDF2 hash, compared in constant time.
struct Hash([u8; HASH_LEN]);

impl Hash {
    fn ct_eq(&self, other: &[u8; HASH_LEN]) -> bool {
        self.0
            .iter()
            .zip(other)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }
}

impl Drop for Hash {
    fn drop(&mut self) {
        crate::clipboard::sealed::wipe(&mut self.0);
    }
}

fn hash(password: &str, salt: &[u8], iterations: u32) -> io::Result<Hash> {
    let mut out = [0u8; HASH_LEN];
    platform::crypto::pbkdf2_sha256(password.as_bytes(), salt, iterations, &mut out)?;
    Ok(Hash(out))
}

fn read_secret(path: &Path) -> Result<Option<Secret>, String> {
    let blob = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let plain = platform::crypto::dpapi_unprotect(&blob, ENTROPY)
        .map_err(|e| format!("cannot decrypt it ({e})"))?;
    Secret::decode(&plain).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuality_protocol::test_util::TempDir;

    #[test]
    fn waits_grow_after_five_wrong_tries() {
        assert_eq!(wait_after(0), None);
        assert_eq!(wait_after(4), None);
        assert_eq!(wait_after(5), Some(Duration::from_secs(30)));
        assert_eq!(wait_after(6), Some(Duration::from_secs(60)));
        assert_eq!(wait_after(7), Some(Duration::from_secs(120)));
        assert_eq!(wait_after(10), Some(MAX_WAIT));
        assert_eq!(wait_after(u32::MAX), Some(MAX_WAIT));
        assert_eq!(human_secs(45), "45 s");
        assert_eq!(human_secs(120), "2 min");
        assert_eq!(human_secs(150), "2 min 30 s");
    }

    fn vault(dir: &TempDir) -> Vault {
        let mut v = Vault::load(dir.0.join("app-lock.bin"));
        v.iterations = 1000;
        v
    }

    #[test]
    fn set_check_change_and_reload() {
        let dir = TempDir::new("app-lock");
        let mut v = vault(&dir);
        let now = Instant::now();
        assert!(!v.has_password());
        assert_eq!(v.check("x", now), Err(Refused::NoPassword));
        assert_eq!(
            v.set("", "abc", now),
            Err(Refused::Invalid("Use at least 4 characters."))
        );
        assert_eq!(v.set("", "hunter22", now), Ok(true));
        assert!(v.check("hunter22", now).is_ok());
        assert_eq!(v.check("hunter23", now), Err(Refused::Wrong));
        // Changing needs the current password.
        assert_eq!(v.set("nope", "other-pw", now), Err(Refused::Wrong));
        assert_eq!(v.set("hunter22", "other-pw", now), Ok(false));
        // The file holds no readable password, and a new vault reads it back.
        let bytes = std::fs::read(dir.0.join("app-lock.bin")).unwrap();
        assert!(!bytes.windows(8).any(|w| w == b"other-pw"));
        let mut again = vault(&dir);
        assert!(again.has_password());
        assert!(again.check("other-pw", now).is_ok());
        assert_eq!(again.check("hunter22", now), Err(Refused::Wrong));
    }

    #[test]
    fn too_many_wrong_tries_are_refused_for_a_while() {
        let dir = TempDir::new("app-lock-throttle");
        let mut v = vault(&dir);
        let now = Instant::now();
        v.set("", "right-pw", now).unwrap();
        for _ in 0..5 {
            assert_eq!(v.check("wrong", now), Err(Refused::Wrong));
        }
        // Even the right password waits.
        assert!(matches!(v.check("right-pw", now), Err(Refused::Wait(_))));
        let later = now + Duration::from_secs(31);
        assert!(v.check("right-pw", later).is_ok());
        assert_eq!(v.wait(later), None);
        assert!(
            Refused::Wait(Duration::from_secs(30))
                .message()
                .contains("30 s")
        );
    }

    #[test]
    fn an_unreadable_file_means_no_password() {
        let dir = TempDir::new("app-lock-bad");
        std::fs::write(dir.0.join("app-lock.bin"), b"garbage").unwrap();
        assert!(!vault(&dir).has_password());
    }
}
