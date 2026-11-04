//! Where cargo puts this build's executables, for a build script that
//! places files beside them. Included with `#[path]` by the build scripts
//! of `verbatim-app`, `verbatim-gui` and `verbatim-synth-espeak`, which
//! share no build dependency.
//!
//! Cargo tells a build script only its `OUT_DIR`, which lies under the
//! build directory, `<build-dir>/[<triple>/]<profile>/build/` followed by
//! one folder per package in the old layout or two in the layout Cargo
//! 1.100 makes the default (cargo #17354). The build directory is the
//! target directory unless `build.build-dir` moves it, and the executables
//! are always in the target directory's matching profile folder. So the
//! profile folder is the parent of `OUT_DIR`'s nearest ancestor named
//! `build`, and its place under the build directory, which `cargo
//! metadata` reports with the target directory, gives the executables'
//! folder.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The folder cargo puts this build's executables in, such as
/// `target/debug`.
pub fn artifact_dir() -> PathBuf {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
    let profile = out_dir
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "build"))
        .and_then(Path::parent)
        .unwrap_or_else(|| {
            panic!(
                "OUT_DIR {} is not under a profile's build folder",
                out_dir.display()
            )
        });
    // Without `cargo metadata`'s answer, or when `OUT_DIR` is not under the
    // build directory it names (a `--target-dir` given on the command line,
    // which also moves the build directory unless one is configured), the
    // build directory is the target directory.
    match directories() {
        Some((target, build)) => match profile.strip_prefix(&build) {
            Ok(relative) => target.join(relative),
            Err(_) => profile.to_path_buf(),
        },
        None => profile.to_path_buf(),
    }
}

/// The target and build directories `cargo metadata` reports for this
/// workspace, the build directory being the target directory when it
/// reports none.
fn directories() -> Option<(PathBuf, PathBuf)> {
    let cargo = std::env::var_os("CARGO")?;
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let json = String::from_utf8(output.stdout).ok()?;
    let target = PathBuf::from(json_string(&json, "target_directory")?);
    let build = json_string(&json, "build_directory").map_or_else(|| target.clone(), PathBuf::from);
    Some((target, build))
}

/// The string value of the last `"key":` in `json`, unescaped. The
/// workspace's own keys come after its packages, so the last one is the
/// workspace's.
fn json_string(json: &str, key: &str) -> Option<String> {
    let start = json.rfind(&format!("\"{key}\":\""))? + key.len() + 4;
    let mut value = String::new();
    let mut characters = json[start..].chars();
    loop {
        match characters.next()? {
            '"' => return Some(value),
            '\\' => match characters.next()? {
                'u' => {
                    let code: String = characters.by_ref().take(4).collect();
                    value.push(char::from_u32(u32::from_str_radix(&code, 16).ok()?)?);
                }
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'r' => value.push('\r'),
                escaped => value.push(escaped),
            },
            character => value.push(character),
        }
    }
}
