//! Builds the static wxWidgets libraries the GUI's C++ layer links: base
//! and core only, for the target architecture, once per version and
//! architecture.
//!
//! The recipe is adapted from wxdragon-sys's build script, which built the
//! same wxWidgets release for wxDragon: the pinned source archive is
//! downloaded and its SHA-256 checked, then `CMake` builds static libraries
//! against the release C runtime (`/MD`) in `RelWithDebInfo`, which the
//! Rust standard library also uses in debug builds. x64 builds with Ninja;
//! ARM64 builds with the Visual Studio generator, the ARM64 platform, and,
//! on an ARM64 machine, the ARM64-hosted toolset, as `verbatim-synth-espeak`
//! does. Differences from wxdragon-sys: only the `wxcore` target is built
//! (it pulls in `wxbase` and the bundled image and regex libraries), the
//! TIFF, JPEG, and WebP readers are off, debug information is embedded in
//! the libraries (`/Z7`) so no separate PDB has to travel with them, and
//! the download and extraction use the `curl.exe` and `tar.exe` that ship
//! with Windows instead of an HTTP client crate.
//!
//! The result goes to a fixed directory, `<root>/<version>-<architecture>`,
//! not the crate's own output directory, so it is built once rather than
//! for every profile and every change to the crate. `<root>` is the
//! `VERBATIM_WX_DIR` environment variable when set (CI points it at a
//! directory it caches), and otherwise `wxwidgets` in the cargo target
//! directory. Once built, the directory holds only `include`, `lib`, and a
//! stamp file naming the recipe: the source and the `CMake` build tree are
//! deleted, which keeps the CI cache small. A lock file serializes builds
//! that start at the same time.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The wxWidgets release, as wxDragon pinned it.
const VERSION: &str = "3.3.3";
/// Where the release's source archive is downloaded from.
const URL: &str =
    "https://github.com/wxWidgets/wxWidgets/releases/download/v3.3.3/wxWidgets-3.3.3.zip";
/// The archive's SHA-256.
const SHA256: &str = "458a1ef598c90174ee43622e8e63bfa1eccb451ffc2258bb4f8edcb050c5feb1";
/// Bumped whenever the recipe changes, so a build made by an older recipe
/// is redone rather than reused.
const RECIPE: u32 = 1;

/// A built wxWidgets: where its headers and libraries are.
#[expect(dead_code, reason = "read once the C++ layer is compiled and linked")]
pub struct Wx {
    /// The directory of wxWidgets' public headers.
    pub include: PathBuf,
    /// The directory holding `wx/setup.h` for this build.
    pub setup: PathBuf,
    /// The directory of the static libraries.
    pub lib: PathBuf,
    /// The static libraries' names, without the `.lib` extension.
    pub libraries: Vec<String>,
}

/// Returns the built wxWidgets for the target architecture, building it
/// first when the fixed directory does not hold this recipe's build.
pub fn prepare() -> Wx {
    println!("cargo:rerun-if-env-changed=VERBATIM_WX_DIR");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("set by cargo");
    let root = root();
    fs::create_dir_all(&root).unwrap_or_else(|error| panic!("create {}: {error}", root.display()));
    let dir = root.join(format!("{VERSION}-{arch}"));

    let lock_path = root.join(format!("{VERSION}-{arch}.lock"));
    let lock = File::create(&lock_path)
        .unwrap_or_else(|error| panic!("create {}: {error}", lock_path.display()));
    lock.lock()
        .unwrap_or_else(|error| panic!("lock {}: {error}", lock_path.display()));

    let stamp = dir.join("stamp");
    let expected = stamp_contents(&arch);
    let wx = if fs::read_to_string(&stamp).ok().as_deref() == Some(expected.as_str()) {
        installed(&dir)
    } else {
        build(&dir, &arch);
        let wx = installed(&dir);
        fs::write(&stamp, expected)
            .unwrap_or_else(|error| panic!("write {}: {error}", stamp.display()));
        wx
    };
    println!("cargo:rerun-if-changed={}", stamp.display());
    drop(lock);
    wx
}

/// The directory the per-version, per-architecture builds go under.
fn root() -> PathBuf {
    if let Some(root) = std::env::var_os("VERBATIM_WX_DIR") {
        return PathBuf::from(root);
    }
    // OUT_DIR is <target>/<profile>/build/<package>-<hash>/out.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo"));
    out_dir
        .ancestors()
        .nth(4)
        .expect("OUT_DIR is <target>/<profile>/build/<package>/out")
        .join("wxwidgets")
}

/// What the stamp file holds for this recipe.
fn stamp_contents(arch: &str) -> String {
    format!("wxWidgets {VERSION} {arch} recipe {RECIPE}\n")
}

/// Reads an installed build's layout.
fn installed(dir: &Path) -> Wx {
    let lib = dir.join("lib");
    let mut libraries: Vec<String> = fs::read_dir(&lib)
        .unwrap_or_else(|error| panic!("read {}: {error}", lib.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "lib"))
        .filter_map(|path| Some(path.file_stem()?.to_str()?.to_owned()))
        .collect();
    libraries.sort();
    assert!(
        libraries.iter().any(|name| name.ends_with("_core")),
        "no wxWidgets core library in {}",
        lib.display()
    );
    Wx {
        include: dir.join("include"),
        setup: dir.join("lib").join("mswu"),
        lib,
        libraries,
    }
}

/// Downloads, builds, and installs wxWidgets into `dir`, leaving only
/// `include` and `lib`.
fn build(dir: &Path, arch: &str) {
    if dir.exists() {
        fs::remove_dir_all(dir).unwrap_or_else(|error| panic!("remove {}: {error}", dir.display()));
    }
    fs::create_dir_all(dir).unwrap_or_else(|error| panic!("create {}: {error}", dir.display()));

    let archive = dir.join(format!("wxWidgets-{VERSION}.zip"));
    download(&archive);
    let source = dir.join("source");
    fs::create_dir_all(&source)
        .unwrap_or_else(|error| panic!("create {}: {error}", source.display()));
    // The archive has no top-level folder: its contents land in `source`.
    run(Command::new(system_tool("tar.exe"))
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&source));
    fs::remove_file(&archive).ok();

    let mut config = cmake::Config::new(&source);
    config
        .out_dir(dir)
        .profile("RelWithDebInfo")
        .define("CMAKE_BUILD_TYPE", "RelWithDebInfo")
        .define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL")
        .define("CMAKE_POLICY_DEFAULT_CMP0091", "NEW")
        .define("CMAKE_MSVC_DEBUG_INFORMATION_FORMAT", "Embedded")
        .define("CMAKE_POLICY_DEFAULT_CMP0141", "NEW")
        .define("wxBUILD_SHARED", "OFF")
        .define("wxBUILD_MONOLITHIC", "OFF")
        .define("wxBUILD_SAMPLES", "OFF")
        .define("wxBUILD_TESTS", "OFF")
        .define("wxBUILD_DEMOS", "OFF")
        .define("wxBUILD_BENCHMARKS", "OFF")
        .define("wxUSE_ACCESSIBILITY", "ON")
        .define("wxUSE_EXCEPTIONS", "ON")
        .define("wxUSE_LIBTIFF", "OFF")
        .define("wxUSE_LIBJPEG", "OFF")
        .define("wxUSE_LIBWEBP", "OFF")
        .cxxflag("/EHsc")
        .build_target("wxcore");
    if arch == "x86_64" && ninja_available() {
        config.generator("Ninja");
    } else {
        // The Visual Studio generator, which the cmake crate picks and
        // points at the target platform. It asks for the x64-hosted
        // toolset even on an ARM64 machine, which may not have it.
        let visual_studio = std::env::var("CMAKE_GENERATOR")
            .map_or(true, |generator| generator.starts_with("Visual Studio"));
        if visual_studio && std::env::var("HOST").is_ok_and(|host| host.starts_with("aarch64")) {
            config.generator_toolset("host=ARM64");
        }
    }
    config.build();

    let build_tree = dir.join("build");
    let platform = if arch == "aarch64" {
        "vc_arm64_lib"
    } else {
        "vc_x64_lib"
    };
    let built = build_tree.join("lib").join(platform);
    let lib = dir.join("lib");
    fs::create_dir_all(lib.join("mswu").join("wx"))
        .unwrap_or_else(|error| panic!("create {}: {error}", lib.display()));
    for entry in fs::read_dir(&built)
        .unwrap_or_else(|error| panic!("read {}: {error}", built.display()))
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "lib") {
            copy(&path, &lib.join(entry.file_name()));
        }
    }
    copy(
        &built.join("mswu").join("wx").join("setup.h"),
        &lib.join("mswu").join("wx").join("setup.h"),
    );
    copy_tree(&source.join("include"), &dir.join("include"));

    for tree in [&build_tree, &source] {
        fs::remove_dir_all(tree)
            .unwrap_or_else(|error| panic!("remove {}: {error}", tree.display()));
    }
}

/// Downloads the source archive and checks its hash.
fn download(archive: &Path) {
    run(Command::new(system_tool("curl.exe"))
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--retry",
            "3",
        ])
        .arg("--output")
        .arg(archive)
        .arg(URL));
    let actual = sha256(archive);
    assert!(
        actual == SHA256,
        "{} has SHA-256 {actual}, expected {SHA256}",
        archive.display()
    );
}

/// The lowercase hexadecimal SHA-256 of a file.
fn sha256(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    let mut file =
        File::open(path).unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// A tool that ships with Windows, by its full path, so a same-named tool
/// earlier on `PATH` (Git's GNU tar cannot read zip archives) is not
/// picked instead.
fn system_tool(name: &str) -> PathBuf {
    let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(windows).join("System32").join(name)
}

/// Whether Ninja is on `PATH`.
fn ninja_available() -> bool {
    Command::new("ninja")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Runs a command, panicking with its description when it fails.
fn run(command: &mut Command) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("run {command:?}: {error}"));
    assert!(status.success(), "{command:?} failed: {status}");
}

/// Copies one file.
fn copy(from: &Path, to: &Path) {
    fs::copy(from, to)
        .unwrap_or_else(|error| panic!("copy {} to {}: {error}", from.display(), to.display()));
}

/// Copies a directory tree.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap_or_else(|error| panic!("create {}: {error}", to.display()));
    for entry in fs::read_dir(from)
        .unwrap_or_else(|error| panic!("read {}: {error}", from.display()))
        .filter_map(Result::ok)
    {
        let target = to.join(entry.file_name());
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            copy_tree(&entry.path(), &target);
        } else {
            copy(&entry.path(), &target);
        }
    }
}
