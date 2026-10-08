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
//! the repository's `videos` folder: a test scenario's in `videos/tests`,
//! a demonstration's in `videos/demos`. See `xtask/src/demo.rs`.
//!
//! `nvda` builds the NVDA transcript add-on and captures what NVDA speaks
//! (`docs/nvda-transcript.md`). See `xtask/src/nvda.rs`.
//!
//! `windows-terminal` downloads and unpacks the pinned portable Windows
//! Terminal the terminal scenarios drive (`verbatim_e2e::windows_terminal`).
//!
//! `park` moves this Remote Desktop session onto the machine's console, so a
//! local end-to-end run keeps working with no RDP client connected. See
//! `xtask/src/park.rs`.

use std::env;
use std::process::{Command, ExitCode};
use std::str;

mod demo;
mod nvda;
mod park;
mod vm;
mod waits;

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
        Some("windows-terminal") => windows_terminal(),
        _ => {
            eprintln!("usage: cargo xtask <command>");
            eprintln!("commands:");
            eprintln!(
                "  ci    platform-neutral dependency check, rustfmt, clippy, and unit tests, for the host's architecture"
            );
            eprintln!("  vm    Hyper-V E2E harness; run `cargo xtask vm` alone for its verbs");
            eprintln!(
                "  demo  record one scenario on this machine as a video in videos/tests, or a demonstration in videos/demos (cargo xtask demo <scenario> [--name <name>])"
            );
            eprintln!(
                "  nvda  build the NVDA transcript add-on, or capture what NVDA speaks; run `cargo xtask nvda` alone for its verbs"
            );
            eprintln!(
                "  park  move this Remote Desktop session to the console, unlocked, for unattended local runs"
            );
            eprintln!(
                "  windows-terminal  download and unpack the terminal scenarios' pinned portable Windows Terminal into target/e2e-stage"
            );
            ExitCode::from(2)
        }
    }
}

/// Downloads, checks, and unpacks the pinned portable Windows Terminal the
/// terminal scenarios drive into the runner-direct stage, as the suite's
/// first launch would: CI runs it as a step of its own, so a failed
/// download is reported before the suite starts.
fn windows_terminal() -> ExitCode {
    let stage = verbatim_e2e::scenario::stage_directory();
    match verbatim_e2e::windows_terminal::prepare(&stage) {
        Ok(folder) => {
            println!(
                "Windows Terminal {} is ready, in portable mode, in {}",
                verbatim_e2e::windows_terminal::VERSION,
                folder.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "could not prepare Windows Terminal {}: {error}",
                verbatim_e2e::windows_terminal::VERSION
            );
            ExitCode::FAILURE
        }
    }
}

fn ci() -> ExitCode {
    println!("xtask ci: platform-neutral dependency check");
    if let Err(error) = check_platform_neutral_deps() {
        eprintln!("xtask ci: step failed: platform-neutral dependency check\n{error}");
        return ExitCode::FAILURE;
    }

    println!("xtask ci: waits in test code");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root");
    if let Err(error) = waits::check(root) {
        eprintln!("xtask ci: step failed: waits in test code\n{error}");
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
