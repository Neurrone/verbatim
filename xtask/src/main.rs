//! Workspace automation, invoked as `cargo xtask <command>`.
//!
//! `ci` is the standard check for both local runs and GitHub Actions, so
//! the two cannot drift: the platform-neutral dependency check, rustfmt,
//! clippy (warnings denied), and unit tests, all for the host's own
//! architecture.
//!
//! `vm` drives the milestone M2 Hyper-V E2E harness (`docs/architecture.md`
//! section 14): building and importing the golden VM, deploying builds into
//! it, and running `crates/verbatim-e2e`'s suite against it. See
//! `xtask/src/vm/mod.rs` for the verb list.
//!
//! `demo` records one end-to-end scenario on this machine as a video for
//! the repository's `videos` folder. See `xtask/src/demo.rs`.
//!
//! `nvda` builds the NVDA transcript add-on and captures what NVDA speaks
//! (`docs/nvda-transcript.md`). See `xtask/src/nvda.rs`.
//!
//! `park` moves this Remote Desktop session onto the machine's console, so a
//! local end-to-end run keeps working with no RDP client connected. See
//! `xtask/src/park.rs`.

use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::str;

mod demo;
mod nvda;
mod park;
mod vm;

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
    "verbatim-text",
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
        Some("park") => park::run(),
        Some("demo") => demo::run(&args[1..]),
        Some("nvda") => nvda::run(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask <command>");
            eprintln!("commands:");
            eprintln!(
                "  ci    platform-neutral dependency check, rustfmt, clippy, and unit tests, for the host's architecture"
            );
            eprintln!("  vm    Hyper-V E2E harness; run `cargo xtask vm` alone for its verbs");
            eprintln!(
                "  demo  record one scenario on this machine as a video in videos/ (cargo xtask demo <scenario> [--name <name>])"
            );
            eprintln!(
                "  nvda  build the NVDA transcript add-on, or capture what NVDA speaks; run `cargo xtask nvda` alone for its verbs"
            );
            eprintln!(
                "  park  move this Remote Desktop session to the console, unlocked, for unattended local runs"
            );
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

    // No step names a target: everything builds for the host's own
    // architecture, x64 on an x64 machine and ARM64 on an ARM64 one, into
    // `target/debug`, the same output the end-to-end suite and plain
    // `cargo test` use.
    let steps: &[(&str, &[&str])] = &[
        // The workspace-hack crate must match what cargo-hakari would
        // generate, and every package must depend on it, or commands that
        // select different packages build dependencies apart again
        // (`docs/tooling.md`, "The workspace-hack crate").
        ("workspace-hack", &["hakari", "generate", "--diff"]),
        (
            "workspace-hack dependencies",
            &["hakari", "manage-deps", "--dry-run"],
        ),
        ("rustfmt", &["fmt", "--all", "--check"]),
        (
            "clippy",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        ("unit tests", &["test", "--workspace"]),
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
/// dependency tree for the host's target. The error names every offending
/// crate and binding so one run reports the whole problem.
fn check_platform_neutral_deps() -> Result<(), String> {
    let mut offences = Vec::new();
    for crate_name in PLATFORM_NEUTRAL_CRATES {
        let output = Command::new(env!("CARGO"))
            .args(["tree", "-p", crate_name, "-e", "normal", "--prefix", "none"])
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
    /// The Visual Studio installations to look in, newest first.
    const VISUAL_STUDIO: &[&str] = &[
        r"18\Community",
        r"2022\Community",
        r"2022\Professional",
        r"2022\Enterprise",
    ];
    /// Visual Studio's LLVM folder for the host's architecture: bindgen
    /// loads `libclang.dll` into the build script, which runs on the host.
    const LLVM_ARCH: &str = if cfg!(target_arch = "aarch64") {
        "ARM64"
    } else {
        "x64"
    };

    if let Some(dir) = env::var_os("LIBCLANG_PATH") {
        return Some(PathBuf::from(dir));
    }
    VISUAL_STUDIO
        .iter()
        .map(|edition| {
            PathBuf::from(format!(
                r"C:\Program Files\Microsoft Visual Studio\{edition}\VC\Tools\Llvm\{LLVM_ARCH}\bin"
            ))
        })
        .chain(std::iter::once(PathBuf::from(r"C:\Program Files\LLVM\bin")))
        .find(|dir| dir.join("libclang.dll").exists())
}
