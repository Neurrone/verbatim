//! Builds eSpeak NG from the vendored source in `third_party/espeak-ng` as
//! a static library, together with its compiled language data, and links
//! the library into this crate.
//!
//! `eSpeak NG`'s own `CMake` build does the work: its `data` target builds the
//! library, the `espeak-ng` program, and then runs that program to compile
//! the phoneme and dictionary data. The data is copied next to the
//! executables in the cargo target directory (`espeak-ng-data`), where the
//! driver looks for it first, and where the end-to-end staging and the VM
//! deploy copy it from.
//!
//! Only the C parts are built: mbrola, sonic, pcaudio, the C++ speech
//! player, and asynchronous output are off, so nothing beyond the C runtime
//! the Rust build already uses is needed.
//!
//! When `VERBATIM_ESPEAK_DIR` is set and the submodule is checked out at a
//! commit with no changes to its tracked files, the build goes to a fixed
//! directory under it, `<commit>-<architecture>`, shared by every target
//! directory and worktree as `VERBATIM_WX_DIR` is for wxWidgets: once
//! built, it holds only `lib`, `espeak-ng-data` and a stamp file naming the
//! recipe, and a lock file serializes builds that start at the same time.
//! CI caches it, keyed on the submodule's commit. Otherwise the build goes
//! to this crate's own output directory, rebuilt as its sources change.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../build-support/artifact_dir.rs"]
mod artifact_dir;

/// Bumped whenever the recipe changes, so a shared build made by an older
/// recipe is redone rather than reused.
const RECIPE: u32 = 1;

/// The libraries this crate links, each with the folder of the `CMake`
/// build tree it is made in.
const LIBRARIES: [(&str, &str); 2] = [("espeak-ng", "src/libespeak-ng"), ("ucd", "src/ucd-tools")];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let source = manifest.join("../../third_party/espeak-ng");
    assert!(
        source.join("CMakeLists.txt").is_file(),
        "eSpeak NG's source is missing at {}; run `git submodule update --init third_party/espeak-ng`",
        source.display()
    );
    for watched in [
        "CMakeLists.txt",
        "cmake",
        "src/CMakeLists.txt",
        "src/include",
        "src/espeak-ng.c",
        "src/libespeak-ng",
        "espeak-ng-data",
        "src/ucd-tools",
        "phsource",
        "dictsource",
    ] {
        println!("cargo:rerun-if-changed={}", source.join(watched).display());
    }
    println!("cargo:rerun-if-env-changed=VERBATIM_ESPEAK_DIR");

    let built = match shared_dir(&source) {
        Some(dir) => shared(&source, &dir),
        None => {
            let out = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo"));
            let build = build(&source, &out);
            Built {
                libraries: LIBRARIES
                    .iter()
                    .flat_map(|(_, folder)| {
                        [build.join(folder), build.join(folder).join("Release")]
                    })
                    .collect(),
                data: build.join("espeak-ng-data"),
            }
        }
    };

    for library in &built.libraries {
        println!("cargo:rustc-link-search=native={}", library.display());
    }
    for (name, _) in LIBRARIES {
        println!("cargo:rustc-link-lib=static={name}");
    }
    // The data path lookup reads the registry.
    println!("cargo:rustc-link-lib=dylib=advapi32");

    let destination = artifact_dir::artifact_dir().join("espeak-ng-data");
    copy_dir(&built.data, &destination).unwrap_or_else(|error| {
        panic!(
            "copy eSpeak NG's data from {} to {}: {error}",
            built.data.display(),
            destination.display()
        )
    });
    println!(
        "cargo:rustc-env=VERBATIM_ESPEAK_BUILD_DATA={}",
        destination.display()
    );
}

/// A finished build: the folders to search for the libraries, and the
/// compiled data.
struct Built {
    libraries: Vec<PathBuf>,
    data: PathBuf,
}

/// The shared directory for this source and architecture, when
/// `VERBATIM_ESPEAK_DIR` is set and the source is a commit with no changes
/// to its tracked files: a changed source is built in the crate's own
/// output directory, so it never replaces or reuses the commit's build.
fn shared_dir(source: &Path) -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("VERBATIM_ESPEAK_DIR")?);
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(source)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let commit = git(&["rev-parse", "HEAD"])?;
    let changes = git(&[
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--untracked-files=no",
    ])?;
    if !changes.is_empty() {
        println!(
            "cargo:warning=eSpeak NG's source has changes, so it is built in this crate's output directory, not under VERBATIM_ESPEAK_DIR"
        );
        return None;
    }
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("set by cargo");
    Some(root.join(format!("{commit}-{arch}")))
}

/// The build in the shared directory `dir`, made first when it does not
/// hold this recipe's build.
fn shared(source: &Path, dir: &Path) -> Built {
    let root = dir.parent().expect("the shared directory has a root");
    fs::create_dir_all(root).unwrap_or_else(|error| panic!("create {}: {error}", root.display()));
    let lock_path = dir.with_extension("lock");
    let lock = File::create(&lock_path)
        .unwrap_or_else(|error| panic!("create {}: {error}", lock_path.display()));
    lock.lock()
        .unwrap_or_else(|error| panic!("lock {}: {error}", lock_path.display()));

    let stamp = dir.join("stamp");
    let expected = format!("eSpeak NG recipe {RECIPE}\n");
    if fs::read_to_string(&stamp).ok().as_deref() != Some(expected.as_str()) {
        if dir.exists() {
            fs::remove_dir_all(dir)
                .unwrap_or_else(|error| panic!("remove {}: {error}", dir.display()));
        }
        let build = build(source, dir);
        let lib = dir.join("lib");
        fs::create_dir_all(&lib)
            .unwrap_or_else(|error| panic!("create {}: {error}", lib.display()));
        for (name, folder) in LIBRARIES {
            let file = format!("{name}.lib");
            let made = [
                build.join(folder).join(&file),
                build.join(folder).join("Release").join(&file),
            ]
            .into_iter()
            .find(|path| path.is_file())
            .unwrap_or_else(|| panic!("no {file} under {}", build.join(folder).display()));
            fs::copy(&made, lib.join(&file))
                .unwrap_or_else(|error| panic!("copy {}: {error}", made.display()));
        }
        let data = build.join("espeak-ng-data");
        copy_dir(&data, &dir.join("espeak-ng-data"))
            .unwrap_or_else(|error| panic!("copy {}: {error}", data.display()));
        fs::remove_dir_all(&build)
            .unwrap_or_else(|error| panic!("remove {}: {error}", build.display()));
        fs::write(&stamp, expected)
            .unwrap_or_else(|error| panic!("write {}: {error}", stamp.display()));
    }
    println!("cargo:rerun-if-changed={}", stamp.display());
    drop(lock);
    Built {
        libraries: vec![dir.join("lib")],
        data: dir.join("espeak-ng-data"),
    }
}

/// Builds eSpeak NG's `data` target with `CMake` into `out`, returning the
/// build tree.
fn build(source: &Path, out: &Path) -> PathBuf {
    let mut config = cmake::Config::new(source);
    // Release whatever cargo's profile, so a debug build links the same C
    // runtime flavor (`/MD`) the Rust standard library uses.
    config
        .out_dir(out)
        .profile("Release")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("ENABLE_TESTS", "OFF")
        .define("USE_MBROLA", "OFF")
        .define("USE_LIBSONIC", "OFF")
        .define("USE_LIBPCAUDIO", "OFF")
        .define("USE_KLATT", "ON")
        .define("USE_SPEECHPLAYER", "OFF")
        .define("USE_ASYNC", "OFF")
        .define("ESPEAK_COMPAT", "OFF")
        // Keeps the build directory out of the library's compiled-in
        // fallback data path; the driver always passes its own.
        .define("CMAKE_INSTALL_PREFIX", "/espeak-ng")
        .build_target("data");
    // The cmake crate asks Visual Studio for its x64-hosted toolset even on
    // an ARM64 host, which ARM64 machines may not have installed. Only the
    // Visual Studio generators take a toolset.
    let visual_studio = std::env::var("CMAKE_GENERATOR")
        .map_or(true, |generator| generator.starts_with("Visual Studio"));
    if visual_studio && std::env::var("HOST").is_ok_and(|host| host.starts_with("aarch64")) {
        config.generator_toolset("host=ARM64");
    }
    config.build().join("build")
}

/// Copies a tree, replacing only files whose contents differ, so a host
/// started from the target directory meanwhile never finds the data
/// missing.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if fs::read(&target).ok() != Some(fs::read(entry.path())?) {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
