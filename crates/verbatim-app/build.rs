//! Embeds the application manifest into `verbatim.exe`.
//!
//! wxWidgets checks at startup that the process declares Common Controls v6
//! and puts up a modal warning dialog when it does not — before any window
//! exists, which hangs GUI startup. The manifest also selects per-monitor
//! DPI awareness and the unelevated execution level.
//!
//! Done through the MSVC linker rather than a resource-compiler crate so the
//! workspace gains no build dependency and the ARM64 cross-build works the
//! same way.

use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("verbatim.exe.manifest");
    println!("cargo:rerun-if-changed=verbatim.exe.manifest");

    let target = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target != "msvc" {
        return;
    }
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
