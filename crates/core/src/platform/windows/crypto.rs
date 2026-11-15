//! Cryptography helpers on Windows' own APIs (no extra dependency): DPAPI blobs for the current
//! user, PBKDF2-HMAC-SHA256 and random bytes from BCrypt (App lock password,
//! `docs/specs/app-lock.md`).

use std::io;

use ::windows::Win32::Foundation::{HLOCAL, LocalFree};
use ::windows::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_ALG_HANDLE_HMAC_FLAG, BCRYPT_SHA256_ALGORITHM,
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptCloseAlgorithmProvider, BCryptDeriveKeyPBKDF2,
    BCryptGenRandom, BCryptOpenAlgorithmProvider, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    CryptProtectData, CryptUnprotectData,
};
use ::windows::core::PCWSTR;

use crate::clipboard::sealed::{Plain, wipe};

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Encrypts `data` for the current Windows user (DPAPI). `entropy` (not a secret) ties the blob
/// to one use.
pub fn dpapi_protect(data: &[u8], entropy: &[u8]) -> io::Result<Vec<u8>> {
    let input = blob(data);
    let entropy = blob(entropy);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the blobs point to live buffers for the call; `out` is freed below with LocalFree.
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(io::Error::other)?;
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        Ok(bytes)
    }
}

/// Decrypts a [`dpapi_protect`] blob made with the same `entropy` (fails for another user or PC).
pub fn dpapi_unprotect(data: &[u8], entropy: &[u8]) -> io::Result<Plain> {
    let input = blob(data);
    let entropy = blob(entropy);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `dpapi_protect`; the plain output is copied, then wiped before it is freed.
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(io::Error::other)?;
        let plain = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let bytes = Plain::new(plain.to_vec());
        wipe(plain);
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        Ok(bytes)
    }
}

/// PBKDF2-HMAC-SHA256 of `password` with `salt`, filling `out`.
pub fn pbkdf2_sha256(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    out: &mut [u8],
) -> io::Result<()> {
    let mut alg = BCRYPT_ALG_HANDLE::default();
    // SAFETY: opens a provider handle closed below; the slices are live for the calls.
    unsafe {
        BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_SHA256_ALGORITHM,
            PCWSTR::null(),
            BCRYPT_ALG_HANDLE_HMAC_FLAG,
        )
        .ok()
        .map_err(io::Error::other)?;
        let derived = BCryptDeriveKeyPBKDF2(
            alg,
            Some(password),
            Some(salt),
            u64::from(iterations),
            out,
            0,
        )
        .ok()
        .map_err(io::Error::other);
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        derived
    }
}

/// Fills `buf` with random bytes from the system's preferred generator.
pub fn random_bytes(buf: &mut [u8]) -> io::Result<()> {
    // SAFETY: `buf` is a valid writable slice for the call.
    unsafe { BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn pbkdf2_matches_the_rfc_7914_vectors() {
        let mut out = [0u8; 32];
        pbkdf2_sha256(b"password", b"salt", 1, &mut out).unwrap();
        assert_eq!(
            hex(&out),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        pbkdf2_sha256(b"password", b"salt", 2, &mut out).unwrap();
        assert_eq!(
            hex(&out),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
    }

    #[test]
    fn random_bytes_are_not_all_zero() {
        let mut a = [0u8; 32];
        random_bytes(&mut a).unwrap();
        assert_ne!(a, [0u8; 32]);
    }

    #[test]
    fn dpapi_round_trip_needs_the_same_entropy() {
        let blob = dpapi_protect(b"hello", b"one").unwrap();
        assert_eq!(&dpapi_unprotect(&blob, b"one").unwrap()[..], b"hello");
        assert!(dpapi_unprotect(&blob, b"two").is_err());
    }
}
