//! Embeds mockapp's manifest, which declares Common Controls v6 for its
//! real tree view (`src/tree_view.rs`), through the MSVC linker, as
//! `verbatim-app`'s build script embeds Verbatim's.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"))
        .join("mockapp.exe.manifest");
    println!("cargo:rerun-if-changed=mockapp.exe.manifest");
    if env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() != "msvc" {
        return;
    }
    // Resource 2, not 1: the process keeps the classic common controls,
    // which its edit control and the tests of it were written against, and
    // the tree view alone is created in an activation context made from
    // this manifest.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED,ID=2");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
