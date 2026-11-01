//! Build script of `kuality.exe`.
//!
//! The application manifest (per-monitor DPI awareness v2, Windows 10/11 `supportedOS`, common
//! controls v6, `asInvoker`, segment heap) comes from GPUI, which the exe links for the settings
//! window: its build script embeds it as resource `RT_MANIFEST 1`, and an exe can only have one,
//! so we no longer embed our own (see `docs/DECISIONS.md`, "DPI manifest").

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
