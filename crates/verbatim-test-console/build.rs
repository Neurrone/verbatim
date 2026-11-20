//! Compiles the test console's client, `src/bin/verbatim-console-client.rs`,
//! on its own into this build's output folder, where the library starts it
//! from: a library cannot name a binary of its own package, and the crates
//! whose tests use the console do not build this package's binaries.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let source = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"))
        .join("src")
        .join("bin")
        .join("verbatim-console-client.rs");
    println!("cargo:rerun-if-changed={}", source.display());
    let out = PathBuf::from(env::var("OUT_DIR").expect("set by cargo"))
        .join("verbatim-console-client.exe");
    let status = Command::new(env::var("RUSTC").expect("set by cargo"))
        .args(["--edition", "2024", "--crate-type", "bin", "--target"])
        .arg(env::var("TARGET").expect("set by cargo"))
        .arg("-o")
        .arg(&out)
        .arg(&source)
        .status()
        .expect("rustc starts");
    assert!(status.success(), "rustc compiles {}", source.display());
}
