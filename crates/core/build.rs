//! Build script of `kuality.exe`.
//!
//! - The application manifest (per-monitor DPI awareness v2, Windows 10/11 `supportedOS`, common
//!   controls v6, `asInvoker`, segment heap) comes from GPUI, which the exe links for the settings
//!   window: its build script embeds it as resource `RT_MANIFEST 1`, and an exe can only have
//!   one, so we do not embed our own (see `docs/DECISIONS.md`, "DPI manifest").
//! - DLLs only the settings window (GPUI) uses are delay-loaded, so the always-running core does
//!   not map them (see `docs/specs/distribution.md`).
//! - Icons and version info are compiled from a generated `.rc` with `embed-resource` (already
//!   in the tree through GPUI): resource 1 is the app icon (`assets/icon/kuality.ico`), 2 and 3
//!   the monochrome tray icons (`tray-white.ico` for dark taskbars, `tray-black.ico` for light
//!   ones). Replacing the files and rebuilding is enough to change them.

use std::env;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Imported only by GPUI and its dependencies; loaded on first use (the settings window).
const DELAY_LOAD: &[&str] = &[
    "d3d11.dll",
    "dxgi.dll",
    "dcomp.dll",
    "dwmapi.dll",
    "winmm.dll",
    "gdiplus.dll",
    "icuuc.dll",
    "uiautomationcore.dll",
    "userenv.dll",
    "comctl32.dll",
    "imm32.dll",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    for dll in DELAY_LOAD {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bins=delayimp.lib");

    embed_icon_and_version();
}

fn embed_icon_and_version() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icons = ICONS.map(|(id, file)| {
        let path = manifest_dir.join("../../assets/icon").join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let path = path
            .canonicalize()
            .unwrap_or_else(|e| panic!("cannot find {}: {e}", path.display()));
        (id, path.to_string_lossy().into_owned())
    });

    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let part = |name: &str| -> u16 {
        env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let numbers = [
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
        0,
    ];

    let rc = rc_source(&icons, &version, numbers);
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("kuality.rc");
    std::fs::write(&out, rc).expect("write kuality.rc");
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_optional()
        .expect("compile kuality.rc");
}

/// Icon resources: id and file in `assets/icon/`. Id 1 is also what Explorer shows for the exe
/// and what GPUI loads for its window; the tray picks 2 or 3 (see `platform/windows/tray.rs`).
const ICONS: [(u16, &str); 3] = [
    (1, "kuality.ico"),
    (2, "tray-white.ico"),
    (3, "tray-black.ico"),
];

/// Resource script: the icons and `VS_VERSION_INFO`. Numeric constants instead of `winver.h`,
/// so no SDK include path is needed.
fn rc_source(icons: &[(u16, String)], version: &str, numbers: [u16; 4]) -> String {
    let [a, b, c, d] = numbers;
    let strings = [
        ("CompanyName", "Kuality"),
        ("FileDescription", "Kuality"),
        ("FileVersion", version),
        ("InternalName", "kuality"),
        ("LegalCopyright", "MIT License"),
        ("OriginalFilename", "kuality.exe"),
        ("ProductName", "Kuality"),
        ("ProductVersion", version),
    ];
    let mut rc = String::new();
    for (id, path) in icons {
        // `canonicalize` gives a `\\?\` path; `.rc` strings use C escapes: forward slashes.
        let path = path.trim_start_matches(r"\\?\").replace('\\', "/");
        let _ = writeln!(rc, "{id} ICON \"{path}\"");
    }
    let _ = writeln!(rc, "1 VERSIONINFO");
    let _ = writeln!(rc, "FILEVERSION {a},{b},{c},{d}");
    let _ = writeln!(rc, "PRODUCTVERSION {a},{b},{c},{d}");
    let _ = writeln!(rc, "FILEFLAGSMASK 0x3F");
    let _ = writeln!(rc, "FILEFLAGS 0x0");
    // VOS_NT_WINDOWS32 and VFT_APP.
    let _ = writeln!(rc, "FILEOS 0x40004");
    let _ = writeln!(rc, "FILETYPE 0x1");
    let _ = writeln!(rc, "FILESUBTYPE 0x0");
    let _ = writeln!(rc, "BEGIN");
    let _ = writeln!(rc, "  BLOCK \"StringFileInfo\"");
    let _ = writeln!(rc, "  BEGIN");
    // U.S. English, Unicode.
    let _ = writeln!(rc, "    BLOCK \"040904B0\"");
    let _ = writeln!(rc, "    BEGIN");
    for (key, value) in strings {
        let _ = writeln!(rc, "      VALUE \"{key}\", \"{value}\"");
    }
    let _ = writeln!(rc, "    END");
    let _ = writeln!(rc, "  END");
    let _ = writeln!(rc, "  BLOCK \"VarFileInfo\"");
    let _ = writeln!(rc, "  BEGIN");
    let _ = writeln!(rc, "    VALUE \"Translation\", 0x409, 1200");
    let _ = writeln!(rc, "  END");
    let _ = writeln!(rc, "END");
    rc
}
