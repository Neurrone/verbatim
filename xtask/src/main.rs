//! Workspace automation, invoked as `cargo xtask <command>`.
//!
//! `ci` is the standard check for both local runs and GitHub Actions, so
//! the two cannot drift: the platform-neutral dependency check, the scans
//! that test code waits for evidence (`xtask/src/waits.rs`) and that UIA
//! caches are read through shapes alone (`xtask/src/shapes.rs`), rustfmt,
//! clippy (warnings denied), and unit tests, all for the host's own
//! architecture.
//!
//! `docs` checks the repository's Markdown documents: that every relative
//! link and every backticked repository path names something that exists,
//! and that `docs/readme.md` indexes every document under `docs/`. See
//! `xtask/src/docs.rs`.
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
//! `nvda` builds the NVDA transcript add-on, captures what NVDA speaks, and
//! compares an end-to-end scenario with NVDA (`docs/nvda-transcript.md`).
//! See `xtask/src/nvda.rs`.
//!
//! `windows-terminal` downloads and unpacks the pinned portable Windows
//! Terminal the terminal scenarios drive (`verbatim_e2e::windows_terminal`).
//!
//! `park` moves this Remote Desktop session onto the machine's console, so a
//! local end-to-end run keeps working with no RDP client connected. See
//! `xtask/src/park.rs`.
//!
//! `warm-speech` activates the Windows speech platform once and reports how
//! long that took, so a CI job pays the platform's cold first activation
//! before the tests that start synthesizer hosts in parallel.
//!
//! `regenerate-terminal-conformance-expectations` records what the real
//! console host and Windows Terminal answer to the simulated terminal's
//! corpus, running `conformance-player` in each. See
//! `xtask/src/conformance.rs`.
//!
//! `record-terminal-streams` records what real programs write to a
//! terminal, from the recipes beside that corpus, as streams of it. See
//! `xtask/src/record.rs`.
//!
//! `trace start` and `trace stop` record a WPR trace of the CPU's
//! scheduling through two elevated tasks. See `xtask/src/trace.rs`.

use std::env;
use std::process::{Command, ExitCode};
use std::str;
use std::time::Instant;

use windows::Media::SpeechSynthesis::SpeechSynthesizer;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

mod conformance;
mod demo;
mod docs;
mod nvda;
mod park;
mod perf;
mod record;
mod shapes;
mod trace;
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

/// The crates through which Verbatim reaches the Windows API, which no
/// platform-neutral crate may reach by any path.
const WINDOWS_BINDINGS: &[&str] = &["windows", "windows-core"];

/// The raw Windows bindings, which no workspace crate in a platform-neutral
/// crate's tree may depend on directly. Third-party libraries with
/// target-gated `windows-sys` dependencies are cross-platform code, not a
/// dependency on Windows, so `windows-sys` beneath them is allowed, as it is
/// beneath the generated `workspace-hack` crate, which only unifies those
/// libraries' features.
const RAW_WINDOWS_BINDINGS: &[&str] = &["windows-sys"];

/// The generated crate that unifies third-party features across the
/// workspace (`docs/tooling/setup.md`), exempt from
/// [`RAW_WINDOWS_BINDINGS`].
const WORKSPACE_HACK: &str = "workspace-hack";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("docs") => docs::run(),
        Some("vm") => vm::run(&args[1..]),
        Some("park") => park::run(),
        Some("trace") => trace::run(&args[1..]),
        Some("measure") => perf::measure(&args[1..]),
        Some("demo") => demo::run(&args[1..]),
        Some("nvda") => nvda::run(&args[1..]),
        Some("windows-terminal") => windows_terminal(),
        Some("warm-speech") => warm_speech(),
        Some("regenerate-terminal-conformance-expectations") => conformance::regenerate(&args[1..]),
        Some("conformance-player") => conformance::player(&args[1..]),
        Some("record-terminal-streams") => record::run(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask <command>");
            eprintln!("commands:");
            eprintln!(
                "  ci    platform-neutral dependency check, rustfmt, clippy, and unit tests, for the host's architecture"
            );
            eprintln!(
                "  docs  check that the documents' links and backticked repository paths exist, and that docs/readme.md indexes every document under docs"
            );
            eprintln!("  vm    Hyper-V E2E harness; run `cargo xtask vm` alone for its verbs");
            eprintln!(
                "  demo  record one scenario on this machine as a video in videos/tests, or a demonstration in videos/demos (cargo xtask demo <scenario> [--name <name>])"
            );
            eprintln!(
                "  nvda  build the NVDA transcript add-on, capture what NVDA speaks, or compare a scenario with NVDA; run `cargo xtask nvda` alone for its verbs"
            );
            eprintln!(
                "  park  move this Remote Desktop session to the console, unlocked, for unattended local runs"
            );
            eprintln!(
                "  trace  WPR traces of the CPU's scheduling: start, stop, waits <trace.etl>, or scenario <name> to trace a window inside one scenario and read it (docs/tooling/measuring.md)"
            );
            eprintln!(
                "  measure  run the measure set on this machine and report each latency stage's distribution (cargo xtask measure [--release | --verbatim-exe <path>]; docs/tooling/measuring.md)"
            );
            eprintln!(
                "  windows-terminal  download and unpack the terminal scenarios' pinned portable Windows Terminal into target/e2e-stage"
            );
            eprintln!(
                "  warm-speech  activate the Windows speech platform once and print how long it took and the default voice"
            );
            ExitCode::from(2)
        }
    }
}

/// Activates the Windows speech platform once through the binding
/// Verbatim's synthesizer in `verbatim-synth-onecore` uses: COM
/// initialized as MTA as that synthesizer does, one `SpeechSynthesizer`
/// constructed, its voice's display name read, and the synthesizer
/// dropped. It prints how long that
/// took, so a CI job's log shows the platform's cold cost, and fails when
/// any step fails.
fn warm_speech() -> ExitCode {
    let started = Instant::now();
    match activate_speech() {
        Ok(voice) => {
            println!(
                "the Windows speech platform activated in {} ms, default voice {voice}",
                started.elapsed().as_millis()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "xtask warm-speech: the Windows speech platform failed to activate after {} ms: {error}",
                started.elapsed().as_millis()
            );
            ExitCode::FAILURE
        }
    }
}

/// Constructs a speech synthesizer and returns its voice's display name.
fn activate_speech() -> Result<String, String> {
    // SAFETY: no reserved parameter; COM stays initialized for the process.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        return Err(format!("CoInitializeEx failed: {:#010x}", hr.0));
    }
    let synth = SpeechSynthesizer::new().map_err(|error| format!("create synthesizer: {error}"))?;
    let voice = synth
        .Voice()
        .and_then(|voice| voice.DisplayName())
        .map_err(|error| format!("read the voice's name: {error}"))?;
    Ok(voice.to_string())
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

    println!("xtask ci: UIA cached reads through shapes");
    if let Err(error) = shapes::check(root) {
        eprintln!("xtask ci: step failed: UIA cached reads through shapes\n{error}");
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
        // (`docs/tooling/setup.md`, "The workspace-hack crate").
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
/// dependency tree for the host's target, or a workspace crate in that tree
/// other than [`WORKSPACE_HACK`] depends directly on one of
/// [`RAW_WINDOWS_BINDINGS`]. It also fails if cargo tree does not list the
/// neutral crate itself as a workspace crate, since the second rule would
/// then pass whatever the tree held. The error names every offending crate and
/// binding so one run reports the whole problem.
fn check_platform_neutral_deps() -> Result<(), String> {
    // The workspace root, the parent of this crate's directory; a crate
    // whose source path lies under it is a workspace crate.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask's directory has no parent")?
        .to_string_lossy()
        .into_owned();
    let mut offences = Vec::new();
    for crate_name in PLATFORM_NEUTRAL_CRATES {
        let output = Command::new(env!("CARGO"))
            .args([
                "tree", "-p", crate_name, "-e", "normal", "--prefix", "depth",
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
        // Each line is the depth, then "name vX.Y.Z (source)"; the crate on
        // the line above at one less depth is the one that depends on it.
        // `ancestors[depth]` is the latest crate seen at each depth, with
        // whether it is a workspace crate.
        let mut ancestors: Vec<(&str, bool)> = Vec::new();
        // The neutral crate itself, at depth 0, must be recognised as a
        // workspace crate, or a change in how cargo tree writes paths would
        // make every workspace crate look external and the rule pass.
        let mut root_recognised = false;
        for line in tree.lines() {
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            let Ok(depth) = line[..digits].parse::<usize>() else {
                continue;
            };
            let rest = &line[digits..];
            let Some(name) = rest.split_whitespace().next() else {
                continue;
            };
            let in_workspace = rest.contains(&format!("({root}{}", std::path::MAIN_SEPARATOR));
            if depth == 0 && name == *crate_name && in_workspace {
                root_recognised = true;
            }
            if WINDOWS_BINDINGS.contains(&name) {
                offences.push(format!("{crate_name} depends on {name}"));
            }
            if RAW_WINDOWS_BINDINGS.contains(&name)
                && let Some(&(parent, true)) = depth.checked_sub(1).and_then(|up| ancestors.get(up))
                && parent != WORKSPACE_HACK
            {
                offences.push(format!(
                    "{crate_name} depends on {name} through {parent}, a workspace crate"
                ));
            }
            ancestors.truncate(depth);
            ancestors.push((name, in_workspace));
        }
        if !root_recognised {
            return Err(format!(
                "cargo tree for {crate_name} did not list it as a workspace crate under {root}, \
                 so the check cannot tell workspace crates apart:\n{tree}"
            ));
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
