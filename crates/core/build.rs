//! Embeds `kuality.manifest` (per-monitor DPI awareness v2, supported OS, common controls v6)
//! into the `kuality` binary using MSVC linker flags, so no resource-compiler crate is needed.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=kuality.manifest");
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("kuality.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
