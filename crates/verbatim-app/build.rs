//! Embeds the application manifest into `verbatim.exe`, and puts the shared
//! sounds beside it.
//!
//! wxWidgets checks at startup that the process declares Common Controls v6
//! and puts up a modal warning dialog when it does not — before any window
//! exists, which hangs GUI startup. The manifest also selects per-monitor
//! DPI awareness and the unelevated execution level.
//!
//! Done through the MSVC linker rather than a resource-compiler crate so the
//! workspace gains no build dependency and the ARM64 build works the
//! same way.
//!
//! The top-level `sounds` folder (the built-in default theme's sounds,
//! `phase6-design.md`, "Packaging") is copied next to the executables in the
//! cargo target directory, where Verbatim looks for it beside itself and
//! where the end-to-end staging and the VM deploy copy it from, as eSpeak
//! NG's data is.

use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    copy_sounds(&manifest_dir.join("../../sounds"));

    let manifest = manifest_dir.join("verbatim.exe.manifest");
    println!("cargo:rerun-if-changed=verbatim.exe.manifest");

    let target = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target != "msvc" {
        return;
    }
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}

/// Copies every file of `sounds` into a `sounds` folder next to the
/// executables, replacing only files whose contents differ, so a Verbatim
/// started from the target directory meanwhile never finds a sound
/// missing.
fn copy_sounds(sounds: &Path) {
    println!("cargo:rerun-if-changed={}", sounds.display());
    let profile_dir = env::var("OUT_DIR")
        .ok()
        .map(PathBuf::from)
        .and_then(|out_dir| out_dir.ancestors().nth(3).map(Path::to_path_buf))
        .expect("OUT_DIR is target/<profile>/build/<crate>/out");
    let destination = profile_dir.join("sounds");
    let copy = || -> std::io::Result<()> {
        fs::create_dir_all(&destination)?;
        for entry in fs::read_dir(sounds)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let source = entry.path();
            println!("cargo:rerun-if-changed={}", source.display());
            let target = destination.join(entry.file_name());
            let contents = fs::read(&source)?;
            if fs::read(&target).ok().as_deref() != Some(contents.as_slice()) {
                fs::write(&target, contents)?;
            }
        }
        Ok(())
    };
    copy().unwrap_or_else(|error| {
        panic!(
            "copy the sounds from {} to {}: {error}",
            sounds.display(),
            destination.display()
        )
    });
}
