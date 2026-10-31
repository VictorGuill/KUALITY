//! Office key: Explorer launches Office (Ctrl+Shift+Alt+Win alone and with some letters) through
//! the `ms-officeapp` protocol. While Hyperkey is enabled we override that protocol in HKCU
//! with a no-op command, and restore the previous state afterwards.
//!
//! The backup lives in `HKCU\Software\Kuality\OfficeKey`. All registry access goes through
//! [`RegistryOps`] so the apply/restore logic is unit-tested with an in-memory fake.
//! Runs on the main thread only (never on the keyboard thread).

use std::io;

use ::windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use ::windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_EXPAND_SZ,
    REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_REG_DWORD,
    RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyW,
    RegDeleteTreeW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegSetValueExW,
};
use ::windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use ::windows::core::PCWSTR;

use super::wide;

/// Parent of the backup key; removed on restore if left empty.
const KUALITY_KEY: &str = r"Software\Kuality";
/// Backup of the state before the override.
const BACKUP_KEY: &str = r"Software\Kuality\OfficeKey";
const CLASS_KEY: &str = r"Software\Classes\ms-officeapp";
const COMMAND_KEY: &str = r"Software\Classes\ms-officeapp\Shell\Open\Command";
/// Keys created (outermost first) when the override is written.
const KEY_CHAIN: [&str; 4] = [
    CLASS_KEY,
    r"Software\Classes\ms-officeapp\Shell",
    r"Software\Classes\ms-officeapp\Shell\Open",
    COMMAND_KEY,
];
/// The no-op command.
const OVERRIDE_COMMAND: &str = "rundll32";

const V_APPLIED: &str = "Applied";
const V_PREV_EXISTED: &str = "PrevExisted";
const V_PREV_COMMAND: &str = "PrevCommand";
const V_CREATED_KEY: &str = "CreatedKey";

/// A registry value of the kinds this module uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    Dword(u32),
    Sz(String),
    ExpandSz(String),
}

/// Minimal registry access under HKCU. `name = None` is the key's default value.
pub trait RegistryOps {
    fn key_exists(&self, path: &str) -> io::Result<bool>;
    /// `Ok(None)` if the key or value is missing; an error for other value types.
    fn get(&self, path: &str, name: Option<&str>) -> io::Result<Option<RegValue>>;
    /// Creates the key (and parents) if needed.
    fn set(&mut self, path: &str, name: Option<&str>, value: &RegValue) -> io::Result<()>;
    /// Missing key or value is fine.
    fn delete_value(&mut self, path: &str, name: Option<&str>) -> io::Result<()>;
    /// Deletes the key and everything below it. Missing key is fine.
    fn delete_tree(&mut self, path: &str) -> io::Result<()>;
    /// Deletes the key only if it has no values and no subkeys.
    fn delete_key_if_empty(&mut self, path: &str) -> io::Result<()>;
}

/// Disables the Office key (idempotent). Returns `true` if the registry was changed.
pub fn apply() -> io::Result<bool> {
    let changed = apply_with(&mut WinRegistry)?;
    if changed {
        notify_assoc_changed();
    }
    Ok(changed)
}

/// Restores the Office key if Kuality disabled it. Returns `true` if the registry was changed.
pub fn restore() -> io::Result<bool> {
    let changed = restore_with(&mut WinRegistry)?;
    if changed {
        notify_assoc_changed();
    }
    Ok(changed)
}

fn notify_assoc_changed() {
    // SAFETY: SHCNE_ASSOCCHANGED takes no item pointers.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

fn is_applied(reg: &impl RegistryOps) -> io::Result<bool> {
    Ok(reg.get(BACKUP_KEY, Some(V_APPLIED))? == Some(RegValue::Dword(1)))
}

pub fn apply_with(reg: &mut impl RegistryOps) -> io::Result<bool> {
    if is_applied(reg)? {
        return Ok(false);
    }
    let prev_existed = reg.key_exists(CLASS_KEY)?;
    let mut created = None;
    for key in KEY_CHAIN {
        if !reg.key_exists(key)? {
            created = Some(key);
            break;
        }
    }
    let prev_command = if created.is_none() {
        match reg.get(COMMAND_KEY, None)? {
            Some(RegValue::Dword(_)) => {
                return Err(io::Error::other(
                    "ms-officeapp command has an unexpected type; leaving it alone",
                ));
            }
            other => other,
        }
    } else {
        None
    };

    // Backup first, then mark applied, then override: an interrupted apply is still restorable.
    reg.delete_tree(BACKUP_KEY)?;
    reg.set(
        BACKUP_KEY,
        Some(V_PREV_EXISTED),
        &RegValue::Dword(u32::from(prev_existed)),
    )?;
    if let Some(key) = created {
        reg.set(BACKUP_KEY, Some(V_CREATED_KEY), &RegValue::Sz(key.into()))?;
    }
    if let Some(cmd) = &prev_command {
        reg.set(BACKUP_KEY, Some(V_PREV_COMMAND), cmd)?;
    }
    reg.set(BACKUP_KEY, Some(V_APPLIED), &RegValue::Dword(1))?;
    reg.set(COMMAND_KEY, None, &RegValue::Sz(OVERRIDE_COMMAND.into()))?;
    Ok(true)
}

pub fn restore_with(reg: &mut impl RegistryOps) -> io::Result<bool> {
    if !is_applied(reg)? {
        return Ok(false);
    }
    let created = match reg.get(BACKUP_KEY, Some(V_CREATED_KEY))? {
        // Only ever delete one of our own known keys.
        Some(RegValue::Sz(k)) => KEY_CHAIN.into_iter().find(|c| c.eq_ignore_ascii_case(&k)),
        _ => None,
    };
    let prev_existed = reg.get(BACKUP_KEY, Some(V_PREV_EXISTED))? != Some(RegValue::Dword(0));

    if let Some(key) = created {
        reg.delete_tree(key)?;
    } else if !prev_existed {
        reg.delete_tree(CLASS_KEY)?;
    } else {
        match reg.get(BACKUP_KEY, Some(V_PREV_COMMAND))? {
            Some(prev @ (RegValue::Sz(_) | RegValue::ExpandSz(_))) => {
                reg.set(COMMAND_KEY, None, &prev)?
            }
            _ => reg.delete_value(COMMAND_KEY, None)?,
        }
    }
    reg.delete_tree(BACKUP_KEY)?;
    reg.delete_key_if_empty(KUALITY_KEY)?;
    Ok(true)
}

/// The real registry (HKCU).
struct WinRegistry;

fn check(e: WIN32_ERROR) -> io::Result<()> {
    if e == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(e.0 as i32))
    }
}

/// An open registry key, closed on drop.
struct OpenKey(HKEY);

impl Drop for OpenKey {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by us and is closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open(
    path: &str,
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> io::Result<Option<OpenKey>> {
    let p = wide(path);
    let mut key = HKEY::default();
    // SAFETY: `p` is NUL-terminated and alive; `key` receives the handle.
    let r = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(p.as_ptr()),
            None,
            access,
            &mut key,
        )
    };
    if r == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(r)?;
    Ok(Some(OpenKey(key)))
}

fn name_ptr(name: &Option<Vec<u16>>) -> PCWSTR {
    name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()))
}

impl RegistryOps for WinRegistry {
    fn key_exists(&self, path: &str) -> io::Result<bool> {
        Ok(open(path, KEY_READ)?.is_some())
    }

    fn get(&self, path: &str, name: Option<&str>) -> io::Result<Option<RegValue>> {
        let p = wide(path);
        let n = name.map(wide);
        let flags = RRF_RT_REG_DWORD | RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let mut ty = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        // SAFETY: size query; all pointers are locals alive for the call.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(p.as_ptr()),
                name_ptr(&n),
                flags,
                Some(&mut ty),
                None,
                Some(&mut size),
            )
        };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(r)?;
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `buf` has room for `size` bytes.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(p.as_ptr()),
                name_ptr(&n),
                flags,
                Some(&mut ty),
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(r)?;
        if ty == REG_DWORD {
            return Ok(Some(RegValue::Dword(
                u32::from(buf[0]) | (u32::from(buf[1]) << 16),
            )));
        }
        let chars = (size as usize / 2).min(buf.len());
        let s = &buf[..chars];
        let s = s.iter().position(|&c| c == 0).map_or(s, |end| &s[..end]);
        let s = String::from_utf16_lossy(s);
        Ok(Some(if ty == REG_EXPAND_SZ {
            RegValue::ExpandSz(s)
        } else {
            RegValue::Sz(s)
        }))
    }

    fn set(&mut self, path: &str, name: Option<&str>, value: &RegValue) -> io::Result<()> {
        let p = wide(path);
        let n = name.map(wide);
        let mut key = HKEY::default();
        // SAFETY: `p` is NUL-terminated; `key` receives the handle.
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(p.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        })?;
        let key = OpenKey(key);
        let (ty, bytes): (REG_VALUE_TYPE, Vec<u8>) = match value {
            RegValue::Dword(v) => (REG_DWORD, v.to_le_bytes().to_vec()),
            RegValue::Sz(s) | RegValue::ExpandSz(s) => (
                if matches!(value, RegValue::Sz(_)) {
                    REG_SZ
                } else {
                    REG_EXPAND_SZ
                },
                wide(s).iter().flat_map(|c| c.to_le_bytes()).collect(),
            ),
        };
        // SAFETY: `key` is open with KEY_SET_VALUE; `bytes` is alive for the call.
        check(unsafe { RegSetValueExW(key.0, name_ptr(&n), None, ty, Some(&bytes)) })
    }

    fn delete_value(&mut self, path: &str, name: Option<&str>) -> io::Result<()> {
        let Some(key) = open(path, KEY_SET_VALUE)? else {
            return Ok(());
        };
        let n = name.map(wide);
        // SAFETY: `key` is open with KEY_SET_VALUE.
        let r = unsafe { RegDeleteValueW(key.0, name_ptr(&n)) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(r)
    }

    fn delete_tree(&mut self, path: &str) -> io::Result<()> {
        let p = wide(path);
        // SAFETY: `p` is NUL-terminated and alive for the call.
        let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr())) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(r)?;
        // RegDeleteTreeW with a subkey name removes the key itself on current Windows; make
        // sure, in case an older build only emptied it.
        // SAFETY: `p` is NUL-terminated and alive for the call.
        let r = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr())) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(r)
    }

    fn delete_key_if_empty(&mut self, path: &str) -> io::Result<()> {
        let Some(key) = open(path, KEY_QUERY_VALUE)? else {
            return Ok(());
        };
        let (mut subkeys, mut values) = (0u32, 0u32);
        // SAFETY: `key` is open with KEY_QUERY_VALUE; only the two counters are requested.
        check(unsafe {
            RegQueryInfoKeyW(
                key.0,
                None,
                None,
                None,
                Some(&mut subkeys),
                None,
                None,
                Some(&mut values),
                None,
                None,
                None,
                None,
            )
        })?;
        drop(key);
        if subkeys != 0 || values != 0 {
            return Ok(());
        }
        let p = wide(path);
        // SAFETY: `p` is NUL-terminated and alive for the call.
        let r = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr())) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(r)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// In-memory registry: key paths (lowercase) → values (lowercase name, "" = default).
    #[derive(Debug, Default, Clone, PartialEq)]
    struct FakeRegistry {
        keys: BTreeMap<String, BTreeMap<String, RegValue>>,
    }

    fn norm(path: &str) -> String {
        path.to_ascii_lowercase()
    }

    fn vname(name: Option<&str>) -> String {
        name.unwrap_or("").to_ascii_lowercase()
    }

    impl FakeRegistry {
        fn create(&mut self, path: &str) {
            let mut acc = String::new();
            for part in norm(path).split('\\') {
                if !acc.is_empty() {
                    acc.push('\\');
                }
                acc.push_str(part);
                self.keys.entry(acc.clone()).or_default();
            }
        }
    }

    impl RegistryOps for FakeRegistry {
        fn key_exists(&self, path: &str) -> io::Result<bool> {
            Ok(self.keys.contains_key(&norm(path)))
        }
        fn get(&self, path: &str, name: Option<&str>) -> io::Result<Option<RegValue>> {
            Ok(self
                .keys
                .get(&norm(path))
                .and_then(|v| v.get(&vname(name)).cloned()))
        }
        fn set(&mut self, path: &str, name: Option<&str>, value: &RegValue) -> io::Result<()> {
            self.create(path);
            self.keys
                .get_mut(&norm(path))
                .unwrap()
                .insert(vname(name), value.clone());
            Ok(())
        }
        fn delete_value(&mut self, path: &str, name: Option<&str>) -> io::Result<()> {
            if let Some(v) = self.keys.get_mut(&norm(path)) {
                v.remove(&vname(name));
            }
            Ok(())
        }
        fn delete_tree(&mut self, path: &str) -> io::Result<()> {
            let p = norm(path);
            let prefix = format!("{p}\\");
            self.keys.retain(|k, _| *k != p && !k.starts_with(&prefix));
            Ok(())
        }
        fn delete_key_if_empty(&mut self, path: &str) -> io::Result<()> {
            let p = norm(path);
            let prefix = format!("{p}\\");
            let has_sub = self.keys.keys().any(|k| k.starts_with(&prefix));
            if !has_sub && self.keys.get(&p).is_some_and(BTreeMap::is_empty) {
                self.keys.remove(&p);
            }
            Ok(())
        }
    }

    fn base() -> FakeRegistry {
        let mut r = FakeRegistry::default();
        r.create(r"Software\Classes\txtfile");
        r.create(r"Software\Microsoft");
        r
    }

    fn command(r: &FakeRegistry) -> Option<RegValue> {
        r.get(COMMAND_KEY, None).unwrap()
    }

    #[test]
    fn apply_and_restore_when_nothing_existed() {
        let mut r = base();
        let before = r.clone();
        assert!(apply_with(&mut r).unwrap());
        assert_eq!(command(&r), Some(RegValue::Sz("rundll32".into())));
        assert!(!apply_with(&mut r).unwrap(), "apply is idempotent");
        assert!(restore_with(&mut r).unwrap());
        assert_eq!(r, before, "registry back to exactly the original state");
        assert!(!restore_with(&mut r).unwrap(), "restore is idempotent");
    }

    #[test]
    fn keeps_other_kuality_settings() {
        let mut r = base();
        r.set(r"Software\Kuality\Other", Some("x"), &RegValue::Dword(5))
            .unwrap();
        let before = r.clone();
        apply_with(&mut r).unwrap();
        restore_with(&mut r).unwrap();
        assert_eq!(r, before);
    }

    #[test]
    fn restores_previous_command() {
        let mut r = base();
        let prev = RegValue::ExpandSz(r"%ProgramFiles%\office.exe %1".into());
        r.set(COMMAND_KEY, None, &prev).unwrap();
        let before = r.clone();
        apply_with(&mut r).unwrap();
        assert_eq!(command(&r), Some(RegValue::Sz("rundll32".into())));
        restore_with(&mut r).unwrap();
        assert_eq!(r, before);
    }

    #[test]
    fn command_key_without_default_value() {
        let mut r = base();
        r.create(COMMAND_KEY);
        let before = r.clone();
        apply_with(&mut r).unwrap();
        restore_with(&mut r).unwrap();
        assert_eq!(r, before);
    }

    #[test]
    fn partial_chain_only_removes_what_we_created() {
        let mut r = base();
        r.set(
            CLASS_KEY,
            Some("URL Protocol"),
            &RegValue::Sz(String::new()),
        )
        .unwrap();
        let before = r.clone();
        apply_with(&mut r).unwrap();
        assert_eq!(
            r.get(BACKUP_KEY, Some(V_CREATED_KEY)).unwrap(),
            Some(RegValue::Sz(r"Software\Classes\ms-officeapp\Shell".into()))
        );
        restore_with(&mut r).unwrap();
        assert_eq!(r, before);
    }

    #[test]
    fn restore_without_backup_does_nothing() {
        let mut r = base();
        r.create(COMMAND_KEY);
        let before = r.clone();
        assert!(!restore_with(&mut r).unwrap());
        assert_eq!(r, before);
    }

    #[test]
    fn unexpected_command_type_is_left_alone() {
        let mut r = base();
        r.set(COMMAND_KEY, None, &RegValue::Dword(1)).unwrap();
        let before = r.clone();
        assert!(apply_with(&mut r).is_err());
        assert_eq!(r, before);
    }

    #[test]
    fn restore_after_interrupted_apply() {
        // Crash right after Applied=1 but before the override was written.
        let mut r = base();
        let before = r.clone();
        r.set(BACKUP_KEY, Some(V_PREV_EXISTED), &RegValue::Dword(0))
            .unwrap();
        r.set(
            BACKUP_KEY,
            Some(V_CREATED_KEY),
            &RegValue::Sz(CLASS_KEY.into()),
        )
        .unwrap();
        r.set(BACKUP_KEY, Some(V_APPLIED), &RegValue::Dword(1))
            .unwrap();
        assert!(restore_with(&mut r).unwrap());
        assert_eq!(r, before);
    }

    #[test]
    fn ignores_unknown_created_key() {
        let mut r = base();
        r.set(BACKUP_KEY, Some(V_PREV_EXISTED), &RegValue::Dword(1))
            .unwrap();
        r.set(
            BACKUP_KEY,
            Some(V_CREATED_KEY),
            &RegValue::Sz(r"Software\Microsoft".into()),
        )
        .unwrap();
        r.set(BACKUP_KEY, Some(V_APPLIED), &RegValue::Dword(1))
            .unwrap();
        restore_with(&mut r).unwrap();
        assert!(r.key_exists(r"Software\Microsoft").unwrap());
    }
}
