//! Build script of `kuality.exe`.
//!
//! - The application manifest (per-monitor DPI awareness v2, Windows 10/11 `supportedOS`, common
//!   controls v6, `asInvoker`, segment heap) comes from GPUI, which the exe links for the settings
//!   window: its build script embeds it as resource `RT_MANIFEST 1`, and an exe can only have
//!   one, so we do not embed our own (see `docs/DECISIONS.md`, "DPI manifest").
//! - DLLs only the settings window (GPUI) uses are delay-loaded, so the always-running core does
//!   not map them (see `docs/specs/distribution.md`).

use std::env;

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
}
