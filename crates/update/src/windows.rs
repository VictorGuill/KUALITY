//! Windows side: HTTPS GET with WinHTTP (system proxy settings, redirects followed), SHA-256
//! with CNG, and the install folder from the installer's uninstall key.

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::null_mut;

use ::windows::Win32::Foundation::ERROR_SUCCESS;
use ::windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_ETAG, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle,
    WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use ::windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use ::windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use ::windows::core::{HSTRING, PCWSTR, w};

use crate::Response;

/// Uninstall key written by the installer: `AppId` of `installer/kuality.iss` + `_is1`.
const UNINSTALL_KEY: PCWSTR = w!(
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{6C1C4F0E-2B7A-4C55-9E7B-4B2D7C1A9F31}_is1"
);

/// Timeouts in ms: name resolution, connect, send, each receive.
const TIMEOUTS: (i32, i32, i32, i32) = (15_000, 15_000, 30_000, 60_000);

/// A WinHTTP handle, closed on drop.
struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void, what: &str) -> Result<Self, String> {
        if raw.is_null() {
            Err(format!("{what}: {}", describe(last_error())))
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a valid handle from WinHTTP, closed once.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn last_error() -> u32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32
}

fn net_error(e: ::windows::core::Error) -> String {
    describe((e.code().0 as u32) & 0xFFFF)
}

/// Readable text for the WinHTTP errors users actually hit (the system message table does not
/// have WinHTTP's).
fn describe(code: u32) -> String {
    match code {
        12002 => "the connection timed out".into(),
        12007 => "cannot resolve the server name (offline?)".into(),
        12029 | 12030 => "cannot connect to the server (offline?)".into(),
        12175 => "secure connection failed".into(),
        _ => format!("network error {code}"),
    }
}

/// HTTPS GET of `url` with extra request headers. The body is read only for `200 OK` and must
/// not be larger than `max` bytes.
pub(crate) fn get(url: &str, headers: &[(&str, &str)], max: usize) -> Result<Response, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("not an https URL: {url}"))?;
    let (host, path) = rest.find('/').map_or((rest, "/"), |i| rest.split_at(i));
    let agent = HSTRING::from(format!("Kuality/{}", env!("CARGO_PKG_VERSION")));
    let header_text: Vec<u16> = headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect::<String>()
        .encode_utf16()
        .collect();

    // SAFETY: every handle comes from WinHTTP and outlives the calls using it (dropped in
    // reverse order: request, connection, session); all buffers are locals alive for the calls
    // and their lengths are passed correctly.
    unsafe {
        let session = Handle::new(
            WinHttpOpen(
                &agent,
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
            "cannot start WinHTTP",
        )?;
        let (resolve, connect_t, send, receive) = TIMEOUTS;
        WinHttpSetTimeouts(session.0, resolve, connect_t, send, receive).map_err(net_error)?;
        let connection = Handle::new(
            WinHttpConnect(
                session.0,
                &HSTRING::from(host),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            ),
            "cannot connect",
        )?;
        let request = Handle::new(
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                &HSTRING::from(path),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "cannot open the request",
        )?;
        let header_arg = (!header_text.is_empty()).then_some(header_text.as_slice());
        WinHttpSendRequest(request.0, header_arg, None, 0, 0, 0).map_err(net_error)?;
        WinHttpReceiveResponse(request.0, null_mut()).map_err(net_error)?;

        let mut status: u32 = 0;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut len,
            null_mut(),
        )
        .map_err(net_error)?;

        let mut buf = [0u16; 256];
        let mut len = size_of_val(&buf) as u32;
        let etag = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_ETAG,
            PCWSTR::null(),
            Some(buf.as_mut_ptr().cast()),
            &mut len,
            null_mut(),
        )
        .ok()
        .map(|()| String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]))
        .filter(|s| !s.is_empty());

        let mut body = Vec::new();
        if status == 200 {
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                let mut read = 0u32;
                WinHttpReadData(
                    request.0,
                    chunk.as_mut_ptr().cast(),
                    chunk.len() as u32,
                    &mut read,
                )
                .map_err(net_error)?;
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read as usize]);
                if body.len() > max {
                    return Err(format!("the download is larger than {max} bytes"));
                }
            }
        }
        Ok(Response { status, body, etag })
    }
}

/// SHA-256 with the CNG one-shot API.
pub(crate) fn sha256(data: &[u8]) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    // SAFETY: the SHA-256 pseudo-handle needs no setup; input and output are valid slices.
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    if status.0 >= 0 {
        Ok(out)
    } else {
        Err(format!("SHA-256 failed (NTSTATUS {:#010x})", status.0))
    }
}

/// `InstallLocation` of the installed Kuality (per-user install), if any.
pub(crate) fn install_dir() -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    let mut len = size_of_val(&buf) as u32;
    // SAFETY: the buffer and its size in bytes are valid for the call.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            UNINSTALL_KEY,
            w!("InstallLocation"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let chars = &buf[..(len as usize / 2).min(buf.len())];
    let s = String::from_utf16_lossy(chars);
    let s = s.trim_end_matches('\0');
    (!s.is_empty()).then(|| PathBuf::from(s))
}
