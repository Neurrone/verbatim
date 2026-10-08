//! Embeds mockapp's manifest, which declares Common Controls v6 for its
//! real tree view (`src/tree_view.rs`), through the MSVC linker, as
//! `verbatim-app`'s build script embeds Verbatim's.

use std::env;
use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("mockapp.exe.manifest");
    println!("cargo:rerun-if-changed=mockapp.exe.manifest");
    if env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() != "msvc" {
        return;
    }
    // Resource 2, not 1: the process keeps the classic common controls,
    // which its edit control and the tests of it were written against, and
    // the tree view alone is created in an activation context made from
    // this manifest.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED,ID=2");
    // An 8 MB main thread stack, where Windows gives 1 MB: a fixture is
    // parsed, converted and built into the tree recursively, a frame per
    // level, and `deep.json` nests sixty levels, which every field a node
    // gains makes deeper in a debug build (a node's UIA class name and
    // automation id overflowed it on 2026-10-08).
    println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
