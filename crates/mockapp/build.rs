//! Embeds mockapp's application manifest, which declares Common Controls
//! v6 for its real tree view (`src/tree_view.rs`), through the MSVC linker,
//! as `verbatim-app`'s build script embeds Verbatim's.

use std::env;
use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("mockapp.exe.manifest");
    println!("cargo:rerun-if-changed=mockapp.exe.manifest");
    if env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() != "msvc" {
        return;
    }
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
