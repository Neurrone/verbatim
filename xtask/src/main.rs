//! Workspace automation, invoked as `cargo xtask <command>`.
//!
//! `ci` is the standard check for both local runs and GitHub Actions, so
//! the two cannot drift: rustfmt, clippy (warnings denied) and unit tests
//! for x64, then a release-profile ARM64 cross-build. ARM64 artifacts are
//! build-verified only; they are never executed on x64 machines.
//!
//! `vm` drives the milestone M2 Hyper-V E2E harness (`docs/architecture.md`
//! section 14): building and importing the golden VM, deploying builds into
//! it, and running `crates/verbatim-e2e`'s suite against it. See
//! `xtask/src/vm/mod.rs` for the verb list.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod vm;

const TARGET_X64: &str = "x86_64-pc-windows-msvc";
const TARGET_ARM64: &str = "aarch64-pc-windows-msvc";

/// The ARM64 build uses the release profile until upstream wxDragon fixes
/// debug-profile ARM64 MSVC builds:
/// <https://github.com/AllenDang/wxDragon/issues/162>
const ARM64_PROFILE_FLAG: &str = "--release";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("vm") => vm::run(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask <command>");
            eprintln!("commands:");
            eprintln!("  ci    rustfmt + clippy + unit tests (x64), release build (ARM64)");
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

/// Locates a libclang directory for wxDragon's bindgen when the environment
/// does not already provide one, checking Visual Studio's bundled LLVM and a
/// standalone LLVM install (the layout on GitHub-hosted Windows runners).
fn find_libclang() -> Option<PathBuf> {
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
