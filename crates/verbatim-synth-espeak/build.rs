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

use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
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

    let mut config = cmake::Config::new(&source);
    // Release whatever cargo's profile, so a debug build links the same C
    // runtime flavor (`/MD`) the Rust standard library uses.
    config
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
    let visual_studio = env::var("CMAKE_GENERATOR")
        .map_or(true, |generator| generator.starts_with("Visual Studio"));
    if visual_studio && env::var("HOST").is_ok_and(|host| host.starts_with("aarch64")) {
        config.generator_toolset("host=ARM64");
    }
    let out = config.build();
    let build = out.join("build");

    for library in ["src/libespeak-ng", "src/ucd-tools"] {
        println!(
            "cargo:rustc-link-search=native={}",
            build.join(library).display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            build.join(library).join("Release").display()
        );
    }
    println!("cargo:rustc-link-lib=static=espeak-ng");
    println!("cargo:rustc-link-lib=static=ucd");
    // The data path lookup reads the registry.
    println!("cargo:rustc-link-lib=dylib=advapi32");

    let data = build.join("espeak-ng-data");
    let profile_dir = env::var("OUT_DIR")
        .ok()
        .map(PathBuf::from)
        .and_then(|out_dir| out_dir.ancestors().nth(3).map(Path::to_path_buf))
        .expect("OUT_DIR is target/<profile>/build/<crate>/out");
    let destination = profile_dir.join("espeak-ng-data");
    copy_dir(&data, &destination).unwrap_or_else(|error| {
        panic!(
            "copy eSpeak NG's data from {} to {}: {error}",
            data.display(),
            destination.display()
        )
    });
    println!(
        "cargo:rustc-env=VERBATIM_ESPEAK_BUILD_DATA={}",
        destination.display()
    );
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
