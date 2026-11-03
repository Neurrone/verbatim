//! Workspace automation, invoked as `cargo xtask <command>`.
//!
//! xtask builds on every host, but only `ci`, `docs` and `replay` exist on
//! all of them; every other command, with its module and dependencies,
//! exists on Windows alone (`#[cfg(windows)]`), and on another host it gets
//! the usage text, which lists only `ci`, `docs` and `replay`.
//!
//! `ci` is the standard check for both local runs and GitHub Actions, so
//! the two cannot drift. On every host it runs the check of the crate
//! tiers (`xtask/src/tiers.rs`) and the scans that test code waits for evidence
//! (`xtask/src/waits.rs`), that UIA caches are read through shapes alone
//! (`xtask/src/shapes.rs`), and that the pure owners name nothing impure
//! (`xtask/src/ownership.rs`), then the documentation check. On Windows it
//! then runs the workspace-hack checks, rustfmt, and clippy (warnings
//! denied) and unit tests for the whole workspace; on any other host,
//! clippy and unit tests for [`LINUX_CRATES`] alone, which GitHub Actions
//! runs as the `ci-linux` job. Everything builds for the host's own
//! architecture.
//!
//! On Windows `ci` ends with the fault step, [`faults`].
//!
//! `docs` checks the repository's Markdown documents: that every relative
//! link and every backticked repository path names something that exists,
//! and that `docs/readme.md` indexes every document under `docs/`. See
//! `xtask/src/docs.rs`.
//!
//! The commands below exist on Windows only.
//!
//! `faults` builds and runs the fault tests (`docs/testing.md`, and
//! `docs/crates/verbatim-faults.md`, "Failure paths"): clippy
//! of every crate with a `faults` feature with that feature on, one package
//! at a time, then each fault test target, into `target/debug`. No other
//! command turns a `faults` feature on, so the ordinary steps build and
//! lint exactly the shipped configuration.
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
//! compares an end-to-end scenario with NVDA (`docs/tooling/nvda.md`).
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
#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use windows::Media::SpeechSynthesis::SpeechSynthesizer;
#[cfg(windows)]
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
#[cfg(windows)]
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

// The portable modules, built on every host.
mod docs;
mod ownership;
mod replay;
mod shapes;
mod tiers;
mod waits;

// The Windows-only commands' modules: each is gated whole, never single
// functions inside it, so a Windows module never keeps a branch for
// another host compiling.
#[cfg(windows)]
mod conformance;
#[cfg(windows)]
mod demo;
#[cfg(windows)]
mod nvda;
#[cfg(windows)]
mod park;
#[cfg(windows)]
mod perf;
#[cfg(windows)]
mod record;
#[cfg(windows)]
mod trace;
#[cfg(windows)]
mod vm;

/// The crates `ci` lints and tests on a host other than Windows, each
/// passed to cargo as `-p`: the platform-neutral tier, the logic crates,
/// and xtask itself, so
/// the unit tests of the documentation check and the scans run there too
/// (`docs/crates/readme.md`). Every crate here
/// must build on Linux, which GitHub's `ci-linux` job proves.
const LINUX_CRATES: &[&str] = &[
    "verbatim-model",
    "verbatim-core",
    "verbatim-speech",
    "verbatim-config",
    "verbatim-i18n",
    "verbatim-input",
    "verbatim-audio",
    "verbatim-text",
    "verbatim-reader-rules",
    "verbatim-outpost-logic",
    "verbatim-listener-logic",
    "verbatim-replay",
    "xtask",
];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("docs") => docs::run(),
        Some("replay") => replay::run(&args[1..]),
        #[cfg(windows)]
        Some("vm") => vm::run(&args[1..]),
        #[cfg(windows)]
        Some("faults") => faults(),
        #[cfg(windows)]
        Some("park") => park::run(),
        #[cfg(windows)]
        Some("trace") => trace::run(&args[1..]),
        #[cfg(windows)]
        Some("measure") => perf::measure(&args[1..]),
        #[cfg(windows)]
        Some("demo") => demo::run(&args[1..]),
        #[cfg(windows)]
        Some("nvda") => nvda::run(&args[1..]),
        #[cfg(windows)]
        Some("windows-terminal") => windows_terminal(),
        #[cfg(windows)]
        Some("warm-speech") => warm_speech(),
        #[cfg(windows)]
        Some("regenerate-terminal-conformance-expectations") => conformance::regenerate(&args[1..]),
        #[cfg(windows)]
        Some("conformance-player") => conformance::player(&args[1..]),
        #[cfg(windows)]
        Some("record-terminal-streams") => record::run(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask <command>");
            eprintln!("commands:");
            #[cfg(windows)]
            eprintln!(
                "  ci    crate tier check, source scans, documentation check, workspace-hack checks, rustfmt, and clippy and unit tests for the workspace, for the host's architecture"
            );
            #[cfg(not(windows))]
            eprintln!(
                "  ci    crate tier check, source scans, documentation check, and clippy and unit tests for the crates in LINUX_CRATES (xtask/src/main.rs), for the host's architecture"
            );
            eprintln!(
                "  docs  check that the documents' links and backticked repository paths exist, and that docs/readme.md indexes every document under docs"
            );
            eprintln!(
                "  replay  replay a flight-recorder dump through Core's reducer and print each input with its effects (cargo xtask replay <flight-recorder.jsonl> [--from <line>] [--to <line>])"
            );
            #[cfg(windows)]
            eprintln!(
                "  faults  clippy with the faults feature, then the fault tests, into target/debug (docs/testing.md)"
            );
            #[cfg(windows)]
            eprintln!("  vm    Hyper-V E2E harness; run `cargo xtask vm` alone for its verbs");
            #[cfg(windows)]
            eprintln!(
                "  demo  record one scenario on this machine as a video in videos/tests, or a demonstration in videos/demos (cargo xtask demo <scenario> [--name <name>])"
            );
            #[cfg(windows)]
            eprintln!(
                "  nvda  build the NVDA transcript add-on, capture what NVDA speaks, or compare a scenario with NVDA; run `cargo xtask nvda` alone for its verbs"
            );
            #[cfg(windows)]
            eprintln!(
                "  park  move this Remote Desktop session to the console, unlocked, for unattended local runs"
            );
            #[cfg(windows)]
            eprintln!(
                "  trace  WPR traces of the CPU's scheduling: start, stop, waits <trace.etl>, or scenario <name> to trace a window inside one scenario and read it (docs/tooling/measuring.md)"
            );
            #[cfg(windows)]
            eprintln!(
                "  measure  run the measure set on this machine and report each latency stage's distribution (cargo xtask measure [--release | --verbatim-exe <path>]; docs/tooling/measuring.md)"
            );
            #[cfg(windows)]
            eprintln!(
                "  windows-terminal  download and unpack the terminal scenarios' pinned portable Windows Terminal into target/e2e-stage"
            );
            #[cfg(windows)]
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
#[cfg(windows)]
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
#[cfg(windows)]
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
#[cfg(windows)]
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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root");
    println!("xtask ci: crate tiers");
    let covered = match tiers::check(root) {
        Ok(covered) => covered,
        Err(error) => {
            eprintln!("xtask ci: step failed: crate tiers\n{error}");
            return ExitCode::FAILURE;
        }
    };

    println!("xtask ci: waits in test code");
    if let Err(error) = waits::check(root) {
        eprintln!("xtask ci: step failed: waits in test code\n{error}");
        return ExitCode::FAILURE;
    }

    println!("xtask ci: UIA cached reads through shapes");
    if let Err(error) = shapes::check(root) {
        eprintln!("xtask ci: step failed: UIA cached reads through shapes\n{error}");
        return ExitCode::FAILURE;
    }

    println!("xtask ci: pure owners' names");
    if let Err(error) = ownership::check(root) {
        eprintln!("xtask ci: step failed: pure owners' names\n{error}");
        return ExitCode::FAILURE;
    }

    println!("xtask ci: determinism of the crates on a reducer's path");
    if let Err(error) = ownership::determinism(root, &covered) {
        eprintln!("xtask ci: step failed: determinism of the crates on a reducer's path\n{error}");
        return ExitCode::FAILURE;
    }

    println!("xtask ci: documents' links and paths");
    if docs::run() != ExitCode::SUCCESS {
        eprintln!("xtask ci: step failed: documents' links and paths");
        return ExitCode::FAILURE;
    }

    // No step names a target: everything builds for the host's own
    // architecture, x64 on an x64 machine and ARM64 on an ARM64 one, into
    // `target/debug`, the same output the end-to-end suite and plain
    // `cargo test` use.
    let steps = if cfg!(windows) {
        windows_steps()
    } else {
        other_host_steps()
    };

    if run_steps("ci", &steps) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }
    #[cfg(windows)]
    if run_steps("ci", &fault_steps()) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    println!("xtask ci: all steps passed");
    ExitCode::SUCCESS
}

/// Runs each of `steps` in turn as `command`'s, stopping at the first that
/// fails.
fn run_steps(command_name: &str, steps: &[Step]) -> ExitCode {
    for (name, cargo_args) in steps {
        println!("xtask {command_name}: {name}");
        let mut command = Command::new(env!("CARGO"));
        command.args(cargo_args);
        match command.status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("xtask {command_name}: step failed: {name} ({status})");
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("xtask {command_name}: could not run cargo for step {name}: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// The crates with a `faults` feature, each linted with it on.
#[cfg(windows)]
const FAULT_CRATES: &[&str] = &[
    "verbatim-audio",
    "verbatim-process",
    "verbatim-uia",
    "verbatim-outpost",
    "verbatim-supervisor",
    "mockapp",
];

/// The fault test targets, each its package's and its name: test targets
/// with `required-features = ["faults"]`, which the ordinary steps skip,
/// and `verbatim-audio`'s `mixer`, whose tests that wait on the mixer's
/// observation point are compiled only with the feature.
#[cfg(windows)]
const FAULT_TESTS: &[(&str, &str)] = &[
    ("verbatim-audio", "mixer"),
    ("verbatim-supervisor", "fault_paths"),
    ("mockapp", "failure_paths"),
    ("mockapp", "shutdown"),
    ("mockapp", "terminal_reading"),
    ("mockapp", "time_limits"),
];

/// The fault step's cargo invocations: clippy of each of [`FAULT_CRATES`]
/// with its `faults` feature, one package at a time so no feature reaches
/// a package that did not ask for it, then each of [`FAULT_TESTS`], all
/// into `target/debug`.
#[cfg(windows)]
fn fault_steps() -> Vec<Step> {
    let mut steps: Vec<Step> = FAULT_CRATES
        .iter()
        .map(|package| {
            (
                "fault clippy",
                args(&[
                    "clippy",
                    "-p",
                    package,
                    "--features",
                    "faults",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ]),
            )
        })
        .collect();
    steps.extend(FAULT_TESTS.iter().map(|(package, test)| {
        (
            "fault tests",
            args(&[
                "test",
                "-p",
                package,
                "--features",
                "faults",
                "--test",
                test,
            ]),
        )
    }));
    steps
}

/// `cargo xtask faults`: the fault step on its own.
#[cfg(windows)]
fn faults() -> ExitCode {
    if run_steps("faults", &fault_steps()) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }
    println!("xtask faults: all steps passed");
    ExitCode::SUCCESS
}

/// One cargo invocation of `ci`: the step's name and cargo's arguments.
type Step = (&'static str, Vec<String>);

/// Turns a list of string literals into cargo's arguments.
fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_owned()).collect()
}

/// The tests that run one at a time, after the rest, as a cargo-nextest
/// filter; `.config/nextest.toml` says why each is here.
const SERIAL_TESTS: &str = concat!(
    "binary_id(verbatim-synth-host::hosting) | binary_id(verbatim-synth-host::protocol)",
    " | binary_id(verbatim-audio::mixer) | binary_id(mockapp::call_counts)",
    " | binary_id(verbatim-agent) | binary_id(verbatim-ia2)",
    " | binary_id(verbatim-listener) | binary_id(verbatim-win32)",
);

/// The tests that run with nothing beside them but each other, many at
/// once, as a cargo-nextest filter; `.config/nextest.toml` says why.
const ALONE_TESTS: &str = "binary_id(mockapp::focus_reports)";

/// `ci`'s cargo steps on Windows: the workspace-hack checks, rustfmt, and
/// clippy and unit tests for the whole workspace. The workspace-hack crate
/// must match what cargo-hakari would generate, and every package must
/// depend on it, or commands that select different packages build
/// dependencies apart again (`docs/tooling/setup.md`, "The workspace-hack
/// crate"). The unit tests run under cargo-nextest, many at once, but for
/// [`ALONE_TESTS`], which run after them with nothing else beside them,
/// and [`SERIAL_TESTS`], which run last, one at a time; then the doctests,
/// which nextest does not run.
fn windows_steps() -> Vec<Step> {
    vec![
        ("workspace-hack", args(&["hakari", "generate", "--diff"])),
        (
            "workspace-hack dependencies",
            args(&["hakari", "manage-deps", "--dry-run"]),
        ),
        ("rustfmt", args(&["fmt", "--all", "--check"])),
        (
            "clippy",
            args(&[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ]),
        ),
        (
            "unit tests",
            args(&[
                "nextest",
                "run",
                "--workspace",
                "-E",
                &format!("not ({SERIAL_TESTS} | {ALONE_TESTS})"),
            ]),
        ),
        (
            "unit tests that run with nothing else",
            args(&["nextest", "run", "--workspace", "-E", ALONE_TESTS]),
        ),
        (
            "unit tests that run one at a time",
            args(&[
                "nextest",
                "run",
                "--workspace",
                "--test-threads",
                "1",
                "-E",
                SERIAL_TESTS,
            ]),
        ),
        ("doctests", args(&["test", "--workspace", "--doc"])),
    ]
}

/// `ci`'s cargo steps on any other host: clippy (warnings denied, all
/// targets) and unit tests for [`LINUX_CRATES`] alone. rustfmt and the
/// workspace-hack checks read source and metadata the same way on every
/// host, so the Windows jobs' run of them suffices.
fn other_host_steps() -> Vec<Step> {
    let packages: Vec<String> = LINUX_CRATES
        .iter()
        .flat_map(|name| ["-p".to_owned(), (*name).to_owned()])
        .collect();
    let mut clippy = args(&["clippy", "--all-targets"]);
    clippy.extend(packages.iter().cloned());
    clippy.extend(args(&["--", "-D", "warnings"]));
    let mut test = args(&["test"]);
    test.extend(packages);
    vec![("clippy", clippy), ("unit tests", test)]
}
