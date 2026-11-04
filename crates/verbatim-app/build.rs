//! Embeds the application manifest into `verbatim.exe`, puts the shared
//! sounds beside it, and records the commit it is built from for the
//! flight-recorder dump's header.
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
//! `sounds/readme.md`) is copied next to the executables in the
//! cargo target directory, where Verbatim looks for it beside itself and
//! where the end-to-end staging and the VM deploy copy it from, as eSpeak
//! NG's data is.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

#[path = "../../build-support/artifact_dir.rs"]
mod artifact_dir;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    copy_sounds(&manifest_dir.join("../../sounds"));
    source_commit(&manifest_dir);

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
    let destination = artifact_dir::artifact_dir().join("sounds");
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

/// Sets `VERBATIM_COMMIT` to the commit this build is made from (`git
/// rev-parse HEAD`) and `VERBATIM_UNCOMMITTED_CHANGES` to whether its
/// working tree has uncommitted changes (`git status --porcelain` prints
/// anything), or to `unknown` and `false` when git cannot answer, as in a
/// source archive. Reruns when HEAD, the ref it points to, the packed refs
/// or the index change, so a commit does not leave a stale hash.
fn source_commit(manifest_dir: &Path) {
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(manifest_dir)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8(output.stdout).ok()?.trim_end().to_owned())
    };
    let commit = git(&["rev-parse", "HEAD"]);
    let status = git(&["--no-optional-locks", "status", "--porcelain"]);
    let (commit, uncommitted_changes) = match (commit, status) {
        (Some(commit), Some(status)) => (commit, !status.is_empty()),
        _ => ("unknown".to_owned(), false),
    };
    println!("cargo:rustc-env=VERBATIM_COMMIT={commit}");
    println!("cargo:rustc-env=VERBATIM_UNCOMMITTED_CHANGES={uncommitted_changes}");

    let mut watched = vec![
        "HEAD".to_owned(),
        "index".to_owned(),
        "packed-refs".to_owned(),
    ];
    watched.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in watched {
        // A path that does not exist would rerun this script on every build.
        if let Some(path) = git(&["rev-parse", "--git-path", &name])
            .map(|path| manifest_dir.join(path))
            .filter(|path| path.exists())
        {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
