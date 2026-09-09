//! Workspace automation, invoked as `cargo xtask <command>`.
//!
//! `ci` is the standard check for both local runs and GitHub Actions, so
//! the two cannot drift: the platform-neutral dependency check, rustfmt,
//! clippy (warnings denied) and unit tests for x64, then a release-profile
//! ARM64 cross-build. ARM64 artifacts are build-verified only; they are
//! never executed on x64 machines.
//!
//! `vm` drives the milestone M2 Hyper-V E2E harness (`docs/architecture.md`
//! section 14): building and importing the golden VM, deploying builds into
//! it, and running `crates/verbatim-e2e`'s suite against it. See
//! `xtask/src/vm/mod.rs` for the verb list.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::str;

mod vm;

const TARGET_X64: &str = "x86_64-pc-windows-msvc";
const TARGET_ARM64: &str = "aarch64-pc-windows-msvc";

/// The ARM64 build uses the release profile until upstream wxDragon fixes
/// debug-profile ARM64 MSVC builds:
/// <https://github.com/AllenDang/wxDragon/issues/162>
const ARM64_PROFILE_FLAG: &str = "--release";

/// The crates `CLAUDE.md`'s "NVDA provenance" section lists as
/// platform-neutral. None of them may depend, directly or through another
/// workspace crate, on the Windows bindings; `ci` checks their dependency
/// trees for [`WINDOWS_BINDINGS`] so the boundary is a fact of the graph
/// rather than a convention. Adding a crate to the neutral tier means
/// adding it here.
const PLATFORM_NEUTRAL_CRATES: &[&str] = &[
    "verbatim-model",
    "verbatim-core",
    "verbatim-speech",
    "verbatim-config",
    "verbatim-i18n",
    "verbatim-input",
    "verbatim-audio",
];

/// The crates through which Verbatim reaches the Windows API. Third-party
/// libraries with target-gated `windows-sys` dependencies are deliberately
/// not on this list: they are cross-platform code, not a dependency on
/// Windows.
const WINDOWS_BINDINGS: &[&str] = &["windows", "windows-core"];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("vm") => vm::run(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask <command>");
            eprintln!("commands:");
            eprintln!(
                "  ci    platform-neutral dependency check, rustfmt + clippy + unit tests (x64), release build (ARM64)"
            );
            eprintln!("  vm    Hyper-V E2E harness; run `cargo xtask vm` alone for its verbs");
            ExitCode::from(2)
        }
    }
}

fn ci() -> ExitCode {
    let libclang = find_libclang();
    match &libclang {
        Some(dir) => println!("xtask ci: using libclang from {}", dir.display()),
        None => println!(
            "xtask ci: libclang not found in known locations; relying on LIBCLANG_PATH or PATH"
        ),
    }

    println!("xtask ci: platform-neutral dependency check");
    if let Err(error) = check_platform_neutral_deps() {
        eprintln!("xtask ci: step failed: platform-neutral dependency check\n{error}");
        return ExitCode::FAILURE;
    }

    let steps: &[(&str, &[&str])] = &[
        ("rustfmt", &["fmt", "--all", "--check"]),
        (
            "clippy (x64)",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--target",
                TARGET_X64,
                "--",
                "-D",
                "warnings",
            ],
        ),
        (
            "unit tests (x64)",
            &["test", "--workspace", "--target", TARGET_X64],
        ),
        (
            "build (ARM64, release)",
            &[
                "build",
                "--workspace",
                ARM64_PROFILE_FLAG,
                "--target",
                TARGET_ARM64,
            ],
        ),
    ];

    for (name, cargo_args) in steps {
        println!("xtask ci: {name}");
        let mut command = Command::new(env!("CARGO"));
        command.args(*cargo_args);
        if let Some(dir) = &libclang {
            command.env("LIBCLANG_PATH", dir);
        }
        match command.status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("xtask ci: step failed: {name} ({status})");
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("xtask ci: could not run cargo for step {name}: {error}");
                return ExitCode::FAILURE;
            }
        }
    }

    println!("xtask ci: all steps passed");
    ExitCode::SUCCESS
}

/// Fails if any of [`PLATFORM_NEUTRAL_CRATES`] has one of
/// [`WINDOWS_BINDINGS`] anywhere in its normal (non-dev, non-build)
/// dependency tree for the x64 target. The error names every offending
/// crate and binding so one run reports the whole problem.
fn check_platform_neutral_deps() -> Result<(), String> {
    let mut offences = Vec::new();
    for crate_name in PLATFORM_NEUTRAL_CRATES {
        let output = Command::new(env!("CARGO"))
            .args([
                "tree", "-p", crate_name, "-e", "normal", "--prefix", "none", "--target",
                TARGET_X64,
            ])
            .output()
            .map_err(|error| format!("could not run cargo tree for {crate_name}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cargo tree for {crate_name} failed ({}):\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let tree = str::from_utf8(&output.stdout)
            .map_err(|error| format!("cargo tree output for {crate_name} is not UTF-8: {error}"))?;
        // Each line is "name vX.Y.Z (source)"; the first word is the crate.
        for binding in tree
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| WINDOWS_BINDINGS.contains(name))
        {
            offences.push(format!("{crate_name} depends on {binding}"));
        }
    }
    if offences.is_empty() {
        Ok(())
    } else {
        offences.sort();
        offences.dedup();
        Err(format!(
            "platform-neutral crates must not depend on the Windows bindings \
             (see the NVDA provenance section of CLAUDE.md):\n  {}",
            offences.join("\n  ")
        ))
    }
}

/// Locates a libclang directory for wxDragon's bindgen when the environment
/// does not already provide one, checking Visual Studio's bundled LLVM and a
/// standalone LLVM install (the layout on GitHub-hosted Windows runners).
///
/// `pub(crate)` rather than private: `xtask::vm::deploy::build_binaries`
/// reuses this exact probe before its own plain `cargo build`, which
/// otherwise fails to build `verbatim-gui`'s wxDragon dependency whenever
/// `LIBCLANG_PATH` is not already set in the caller's environment — this
/// `ci` path is the only other place in this binary that needs it, so one
/// shared probe stays in lockstep rather than two copies drifting apart.
pub(crate) fn find_libclang() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        r"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\Llvm\x64\bin",
        r"C:\Program Files\Microsoft Visual Studio\2022\Professional\VC\Tools\Llvm\x64\bin",
        r"C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\Llvm\x64\bin",
        r"C:\Program Files\LLVM\bin",
    ];

    if let Some(dir) = env::var_os("LIBCLANG_PATH") {
        return Some(PathBuf::from(dir));
    }
    CANDIDATES
        .iter()
        .map(Path::new)
        .find(|dir| dir.join("libclang.dll").exists())
        .map(Path::to_path_buf)
}
