//! Encrypted-in-memory buffers for clipboard data (`docs/specs/clipboard.md`).
//!
//! [`Sealed`] keeps bytes encrypted by a [`Cipher`] (on Windows `CryptProtectMemory` with a key
//! that only exists inside this process); [`Plain`] is a decrypted copy that is wiped when it
//! is dropped. Portable: the cipher is supplied by the platform layer.

use std::ops::Deref;
use std::sync::atomic::{Ordering, compiler_fence};

/// Block size the cipher works on; buffers are padded to a multiple of it.
pub const BLOCK: usize = 16;

/// In-place encryption of whole blocks (`buf.len()` is a multiple of [`BLOCK`]).
pub trait Cipher {
    fn encrypt(&self, buf: &mut [u8]) -> bool;
    fn decrypt(&self, buf: &mut [u8]) -> bool;
}

/// Overwrites `buf` with zeros in a way the compiler does not remove.
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

/// Decrypted bytes, wiped on drop.
#[derive(Default)]
pub struct Plain(Vec<u8>);

impl Plain {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// UTF-8 text from a wiped-on-drop buffer.
    pub fn from_string(s: String) -> Self {
        Self(s.into_bytes())
    }

    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    pub fn as_mut_vec(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}

impl Deref for Plain {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Plain {
    fn drop(&mut self) {
        // The spare capacity may hold old bytes too.
        let cap = self.0.capacity();
        self.0.resize(cap, 0);
        wipe(&mut self.0);
    }
}

impl std::fmt::Debug for Plain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plain({} bytes)", self.0.len())
    }
}

/// Bytes kept encrypted.
pub struct Sealed {
    buf: Box<[u8]>,
    len: usize,
}

impl Sealed {
    /// Encrypts a copy of `data`. `None` if the cipher failed (nothing is kept then).
    pub fn seal(cipher: &impl Cipher, data: &[u8]) -> Option<Self> {
        let padded = data.len().div_ceil(BLOCK).max(1) * BLOCK;
        let mut buf = vec![0u8; padded].into_boxed_slice();
        buf[..data.len()].copy_from_slice(data);
        if !cipher.encrypt(&mut buf) {
            wipe(&mut buf);
            return None;
        }
        Some(Self {
            buf,
            len: data.len(),
        })
    }

    /// A decrypted copy. `None` if the cipher failed.
    pub fn open(&self, cipher: &impl Cipher) -> Option<Plain> {
        let mut plain = Plain::new(self.buf.to_vec());
        if !cipher.decrypt(plain.as_mut_vec()) {
            return None;
        }
        plain.as_mut_vec().truncate(self.len);
        Some(plain)
    }

    /// Length of the plain data.
    pub fn len(&self) -> usize {
        self.len
    }
}

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sealed({} bytes)", self.len)
    }
}

/// Test cipher: XOR with a fixed byte (enough to prove data is transformed and restored).
#[cfg(test)]
#[derive(Debug, Default)]
pub struct XorCipher;

#[cfg(test)]
impl Cipher for XorCipher {
    fn encrypt(&self, buf: &mut [u8]) -> bool {
        assert_eq!(buf.len() % BLOCK, 0);
        buf.iter_mut().for_each(|b| *b ^= 0x5A);
        true
    }
    fn decrypt(&self, buf: &mut [u8]) -> bool {
        self.encrypt(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open() {
        for data in [&b""[..], b"a", b"0123456789abcdef", b"0123456789abcdefg"] {
            let s = Sealed::seal(&XorCipher, data).unwrap();
            assert_eq!(s.len(), data.len());
            assert_eq!(s.buf.len() % BLOCK, 0);
            assert!(!s.buf.is_empty());
            if !data.is_empty() {
                assert_ne!(&s.buf[..data.len()], data, "stored encrypted");
            }
            assert_eq!(&*s.open(&XorCipher).unwrap(), data);
        }
    }

    #[test]
    fn failing_cipher_keeps_nothing() {
        struct Broken;
        impl Cipher for Broken {
            fn encrypt(&self, _: &mut [u8]) -> bool {
                false
            }
            fn decrypt(&self, _: &mut [u8]) -> bool {
                false
            }
        }
        assert!(Sealed::seal(&Broken, b"x").is_none());
        let s = Sealed::seal(&XorCipher, b"x").unwrap();
        assert!(s.open(&Broken).is_none());
    }

    #[test]
    fn wipe_zeroes() {
        let mut b = vec![1u8, 2, 3];
        wipe(&mut b);
        assert_eq!(b, [0, 0, 0]);
    }
}
