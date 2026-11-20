//! The end-to-end suite's one test binary: a test per scenario registered
//! in `crates/verbatim-e2e/src/registry.rs`, built from the registry's
//! entries, so that the registry is the one list of the scenarios, each calling
//! `registry::run_named` with its own name, and the session check every
//! scenario depends on. A scenario's test has the scenario's name, so
//! `cargo test -p verbatim-e2e --test e2e -- --ignored --exact <name>
//! --test-threads=1` runs exactly that scenario.
//!
//! The scenarios share one binary rather than having one each because every
//! test binary links the whole harness with its own debug information:
//! a binary per scenario made the suite most of a build's size on disk
//! (`books/dev/src/how-to/setup.md`, "Build disk use").
//!
//! Every test here is ignored, so a workspace test run, and `cargo xtask
//! ci`, lists them as ignored rather than running them; the end-to-end job
//! runs them with `--ignored`, and `cargo xtask demo` runs a demonstration
//! with `--include-ignored`.
//!
//! The binary has its own `main` (`harness = false`, with `libtest-mimic`
//! taking libtest's command line), so that once every test has run it
//! prints the end-of-run summary (`verbatim_e2e::run_summary`), which
//! libtest's harness has no place for. It keeps libtest's behaviour where
//! `libtest-mimic` differs: any number of filters
//! (`verbatim_e2e::command_line`), the tests run in the order of their
//! names, each on a thread of its own, and each failure's whole report in
//! the `failures:` section at the end.

use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Instant;

use libtest_mimic::{Arguments, Failed, Trial};
use verbatim_e2e::AgentClient;
use verbatim_e2e::command_line;
use verbatim_e2e::run_summary::RunSummary;

/// The agent reports an interactive session on the default desktop: the
/// precondition every scenario depends on, not itself a scenario. A screen
/// reader driven from a non-interactive session (session 0, `WinRM`,
/// PowerShell Direct) can never work, and a locked machine's input desktop
/// is the secure desktop, where the suite does nothing useful, so this is
/// the first thing worth checking when the suite behaves strangely. It
/// fails when no agent is named.
/// It is ignored like the scenarios: it needs a running agent, and the
/// end-to-end job runs it with `--ignored` (books/dev/src/how-to/e2e.md).
fn agent_reports_an_interactive_window_station() {
    let endpoint = verbatim_e2e::endpoint().unwrap_or_else(|| {
        panic!(
            "{} is not set: a live test needs a running agent",
            verbatim_e2e::ENDPOINT_ENV
        )
    });
    let mut agent =
        AgentClient::connect(&endpoint).expect("connects to the agent and completes Hello");
    let info = agent.session_info().expect("agent answers SessionInfo");
    assert!(
        info.interactive_window_station,
        "agent's window station is not interactive (session {}); the E2E suite cannot drive a screen reader from here",
        info.session_id
    );
    assert_eq!(
        info.input_desktop_name.as_deref(),
        Some("Default"),
        "the input desktop is not the default desktop: the machine is locked, or the secure desktop is up"
    );
}

/// A scenario's test: runs it, records its result and seconds in
/// `summary`, and fails as it does.
fn scenario_trial(name: &'static str, summary: &Arc<Mutex<RunSummary>>) -> Trial {
    let summary = Arc::clone(summary);
    Trial::test(name, move || {
        let started = Instant::now();
        let outcome = on_test_thread(name, move || verbatim_e2e::registry::run_named(name));
        summary
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(name, outcome.is_ok(), started.elapsed());
        outcome
    })
    .with_ignored_flag(true)
}

/// Runs `test` as libtest runs a test: on a thread of its own, named
/// after it, with the stack a spawned thread has by default, 2 MiB unless
/// `RUST_MIN_STACK` says otherwise, as libtest's test threads have, rather
/// than on the main thread, which `libtest-mimic` runs a test on with
/// `--test-threads=1`, and whose stack is the linker's. A panic is
/// its failure. Its message, for a scenario its whole report
/// (`registry::run_named`), is printed once, by the panic hook where it
/// failed, so that a run cut off later keeps it; the `failures:` section
/// says only where it is ([`PRINTED_ABOVE`]).
/// What the `failures:` section prints for a test that panicked.
const PRINTED_ABOVE: &str = "its report is printed above, where it failed";

fn on_test_thread(name: &'static str, test: impl FnOnce() + Send + 'static) -> Result<(), Failed> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || panic::catch_unwind(AssertUnwindSafe(test)).map_err(|_| ()))
        .map_err(|error| format!("the test's thread could not start: {error}"))?
        .join()
        .unwrap_or(Err(()))
        .map_err(|()| Failed::from(PRINTED_ABOVE))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (filters, rest) = command_line::split_filters(args.get(1..).unwrap_or_default());
    let arguments = Arguments::from_iter(args.iter().take(1).cloned().chain(rest));
    let summary = Arc::new(Mutex::new(RunSummary::default()));
    let mut trials: Vec<Trial> = verbatim_e2e::registry::SCENARIOS
        .iter()
        .map(|def| scenario_trial(def.name, &summary))
        .collect();
    trials.push(
        Trial::test("agent_reports_an_interactive_window_station", || {
            on_test_thread(
                "agent_reports_an_interactive_window_station",
                agent_reports_an_interactive_window_station,
            )
        })
        .with_ignored_flag(true),
    );
    // libtest runs its tests in the order of their names, so the session
    // check runs before every scenario.
    trials.sort_by(|a, b| a.name().cmp(b.name()));
    let names: Vec<&str> = trials.iter().map(Trial::name).collect();
    let unmatched = command_line::unmatched(&names, &filters, arguments.exact);
    if !unmatched.is_empty() {
        for filter in unmatched {
            eprintln!("no test is named {filter}");
        }
        return ExitCode::FAILURE;
    }
    trials.retain(|trial| command_line::selected(trial.name(), &filters, arguments.exact));
    // A panic a scenario catches is printed once, in its report.
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if !verbatim_e2e::registry::panic_is_reported_later() {
            default(info);
        }
    }));
    let conclusion = libtest_mimic::run(&arguments, trials);
    for line in summary
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .lines()
    {
        println!("{line}");
    }
    conclusion.exit_code()
}
