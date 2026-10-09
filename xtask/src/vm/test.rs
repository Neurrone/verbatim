//! `cargo xtask vm test`: builds the current source, starts the guest if it
//! is not running (or, with `--restore`, restores the golden checkpoint
//! first), stages and copies the build onto it, discovers the guest's IP,
//! then runs `crates/verbatim-e2e`'s scenarios on the host against it, one
//! at a time.
//!
//! Milestone M3 Track B replaced "the whole suite runs as one blob with one
//! recording" with per-scenario selection and boundaries:
//!
//! - `--scenario <name>` (repeatable) and `--group <name>` (repeatable)
//!   choose which of `verbatim_e2e::registry::SCENARIOS` to run; with
//!   neither given, every registered scenario runs except the
//!   demonstrations (the `demo` group), which are recorded with
//!   `cargo xtask demo` on a development machine and refused here.
//!   `--list` prints the registry (name and group, one per line) and exits
//!   without touching the VM at all — no build, no restore, no deploy.
//! - `session_info` (`agent_reports_an_interactive_window_station` in
//!   `crates/verbatim-e2e/tests/e2e.rs`) is not a
//!   scenario — a precondition every scenario depends on — so it always
//!   runs first, once, regardless of `--scenario`/`--group`, and a failure
//!   there aborts the whole run before any scenario is attempted: nothing
//!   downstream can work from a non-interactive agent session.
//! - Each selected scenario runs as its own `cargo test -p verbatim-e2e
//!   --test e2e <name> -- --exact` subprocess ([`run_scenario_subprocess`]), not one
//!   shared invocation covering every scenario. This is the design choice
//!   for controlling scenario boundaries from here (`docs/roadmap.md`'s M3
//!   Track B item asks for one or the other): reusing the existing
//!   `#[test]`-per-scenario libtest binary this way needs no new runner
//!   mode inside `verbatim-e2e` itself, and it gives each scenario a
//!   boundary from process start to process exit, with no new IPC. The
//!   scenario's recording lives inside that boundary too: its
//!   `Scenario::launch` starts the capture and `verbatim_e2e::registry::run`
//!   finishes it, so one scenario's video never spills into the next.
//! - After every selected scenario's subprocess exits, this reads back the
//!   [`verbatim_e2e::artifacts::ScenarioSummary`] that scenario's own run
//!   wrote to its artifacts directory (`verbatim_e2e::artifacts`) — rather
//!   than parsing the subprocess's stdout — and prints one line per
//!   scenario at the end: pass or fail, and how many of its latency
//!   timelines reached audio. A selected scenario that exited but wrote *no*
//!   summary is reported as a failure of the run, never a pass: a
//!   `--scenario` typo or a scenario missing from `tests/e2e.rs` makes
//!   `cargo test --test e2e <name> -- --exact` match zero tests and still exit 0, and
//!   silently green-lighting that would defeat the point of running it. The
//!   artifacts directory holds the interleaved timeline, Verbatim's captured
//!   stderr log, and a reducer flight-recorder dump — all for every run, pass
//!   or fail (so a passing run leaves its timings and reducer inputs
//!   behind) — all collected by `verbatim_e2e::registry::run` itself, inside
//!   the subprocess, since that is where the live control and agent
//!   connections needed to fetch them still exist.
//! - There is still no retry of any kind, at any level: a scenario that
//!   fails is reported failed, once, with its artifacts left for a human to
//!   root-cause — never re-run automatically by this module or by
//!   `verbatim-e2e` itself.
//!
//! The build ([`deploy::build`]) deliberately runs *before* the guest is
//! touched at all: [`deploy::run`]'s original ordering built only after
//! restoring, so a compile failure wasted the restore and the next attempt,
//! once the code was fixed, paid for another one. Building first means a
//! compile failure costs zero VM state changes, with or without
//! `--restore`.
//!
//! The suite's two host-filesystem steps — checking `verbatim.exe` exists
//! and writing the fixed `settings.toml` next to it — are correct
//! only in runner-direct mode, where the suite and Verbatim share a
//! filesystem. Here `VERBATIM_E2E_VERBATIM_EXE` names a path inside the
//! guest, so this verb sets `VERBATIM_E2E_REMOTE` as well, which tells
//! `verbatim_e2e::Scenario::launch` to skip both: [`super::deploy::stage_and_copy`]
//! has already staged that same configuration inside the guest.
//!
//! Restoring is opt-in (`--restore`, `restore` here), never automatic: an
//! ordinary run deploys onto whatever the guest is currently running,
//! starting it first if it is off. Nothing is installed into the guest —
//! [`deploy::stage_and_copy`] copies files and skips unchanged ones — so a
//! restore buys nothing on an ordinary run, while it and its agent wait are
//! most of a run's wall-clock cost. An acceptance run, which must start
//! from a known-clean guest, asks for one with `--restore`.
//!
//! `test` is always audible: [`deploy::stage_and_copy`] stages a
//! `settings.toml` selecting eSpeak NG, and `VERBATIM_E2E_AUDIBLE=1` is set
//! on the suite process, so `verbatim_e2e::scenario::Scenario::launch` omits
//! `VERBATIM_TEST_AUDIO=null` for the Verbatim it launches in the guest, and
//! Verbatim speaks through real WASAPI. Over a connected
//! `cargo xtask vm connect` session that speech plays to the session's own
//! audio, so a human hears the run live.
//!
//! Every scenario is also recorded, by the suite itself rather than by this
//! module (see `verbatim_e2e::recording`): ffmpeg captures the guest's
//! desktop through the agent, Verbatim writes everything it plays to a WAV
//! file, and at the end of the scenario the two are muxed in the guest and
//! copied to `target/e2e-artifacts/<scenario>/<scenario>.mp4` on the host.
//! This module only names the guest's copy of ffmpeg in
//! `VERBATIM_E2E_FFMPEG` ([`FFMPEG_GUEST_PATH`]). Because the audio comes
//! from Verbatim's own mixer and not from a capture device, recording does
//! not depend on the guest's audio endpoints, so watching live and recording
//! work together. Setting `VERBATIM_E2E_RECORD=0` turns recording off.
//!
//! Every speech assertion waits for the matched utterance to be heard in
//! full before the next input is injected (decision D17), so a human
//! watching over `cargo xtask vm connect`, or a recording, hears each
//! utterance whole.

use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus};

use verbatim_e2e::artifacts::{self, ScenarioSummary};
use verbatim_e2e::registry;

use super::host::{Host, renew_guest_dhcp, wait_for_agent};
use super::{
    AGENT_PORT, CHECKPOINT_NAME, FFMPEG_GUEST_PATH, VERBATIM_DIR, VM_NAME, VmResult, deploy, dotenv,
};

/// `session_info`'s own test function name
/// (in `crates/verbatim-e2e/tests/e2e.rs`) — not a registered
/// scenario, but the precondition [`test`] always runs first, once, before
/// any scenario. Named literally here rather than looked up, since it
/// deliberately has no [`registry::ScenarioDef`] to look up.
const SESSION_INFO_TEST_NAME: &str = "agent_reports_an_interactive_window_station";

/// Readies the guest before deploying. By default it only makes sure the
/// guest is running (a no-op when it already is) and its agent answers.
/// With `restore` (`--restore`), it restores the golden checkpoint first,
/// starts the guest, and renews its DHCP lease (the restored guest may hold
/// a lease from a Default Switch subnet that no longer exists — see
/// `renew_guest_dhcp`) before the agent wait.
fn prepare_guest(
    host: &dyn Host,
    restore: bool,
    credentials: &dotenv::GuestCredentials,
) -> VmResult<()> {
    if !restore {
        println!(
            "xtask vm test: deploying onto the guest as it is (pass --restore to restore \
             '{CHECKPOINT_NAME}' first, as an acceptance run needs)"
        );
        host.start_vm(VM_NAME)?;
        return wait_for_agent(host, VM_NAME);
    }
    println!("xtask vm test: restoring checkpoint '{CHECKPOINT_NAME}'");
    host.restore_checkpoint(VM_NAME, CHECKPOINT_NAME)?;
    host.start_vm(VM_NAME)?;
    if let Err(error) = renew_guest_dhcp(host, VM_NAME, credentials) {
        eprintln!("xtask vm test: guest DHCP renewal failed (continuing): {error}");
    }
    wait_for_agent(host, VM_NAME)
}

/// The flags `cargo xtask vm test` accepts, all defaulting to off or empty:
/// `--restore` restores the golden checkpoint first, `--list` prints the
/// scenario registry and exits, and `--scenario`/`--group` (each repeatable)
/// select which scenarios run. See this module's own doc comment for the
/// details, and `parse_test_flags` in `super` for the parsing.
#[derive(Clone, Default)]
pub(crate) struct TestFlags {
    pub restore: bool,
    pub list: bool,
    pub scenarios: Vec<String>,
    pub groups: Vec<String>,
}

/// # Errors
///
/// Returns an error if `--scenario`/`--group` name something unregistered,
/// readying the guest (including the checkpoint restore when `--restore` is
/// given), the deploy, IP discovery, the `session_info` precondition, or any
/// selected scenario itself fails. A recording that cannot start or finish
/// is only a warning, printed by the scenario's own subprocess.
pub(crate) fn test(host: &dyn Host, repo_root: &Path, flags: TestFlags) -> VmResult<()> {
    let TestFlags {
        restore,
        list,
        scenarios,
        groups,
    } = flags;

    if list {
        print_scenario_list();
        return Ok(());
    }

    let selected = registry::select(registry::SCENARIOS, &scenarios, &groups)?;
    if let Some(demo) = selected
        .iter()
        .find(|def| def.group == registry::Group::Demo)
    {
        return Err(format!(
            "{} is a demonstration, which the suite never runs; record it on a development \
             machine with `cargo xtask demo {}`",
            demo.name, demo.name
        ));
    }
    if selected.is_empty() {
        return Err(
            "no scenarios matched the given --scenario/--group filters (a --group with no \
             scenarios registered under it yet selects nothing)"
                .to_owned(),
        );
    }

    let credentials = dotenv::load_guest_credentials(repo_root)?;

    println!(
        "xtask vm test: building first, before touching the VM, so a compile failure leaves \
         guest state untouched"
    );
    let built = deploy::build(repo_root)?;

    prepare_guest(host, restore, &credentials)?;

    println!(
        "xtask vm test: speaking through eSpeak NG; connect with `cargo xtask vm connect` to hear \
         a run live; each scenario's video, with Verbatim's audio, is saved to \
         target/e2e-artifacts/<scenario>/<scenario>.mp4 (set VERBATIM_E2E_RECORD=0 to skip it)"
    );

    println!("xtask vm test: staging and copying the build onto the guest");
    deploy::stage_and_copy(host, repo_root, &credentials, built)?;
    wait_for_agent(host, VM_NAME)?;

    let ip = host.guest_ip(VM_NAME)?;
    let endpoint = format!("{ip}:{AGENT_PORT}");
    let guest_exe = format!(r"{VERBATIM_DIR}\verbatim.exe");

    println!(
        "xtask vm test: checking the session_info precondition (agent reports an interactive \
         session)"
    );
    let session_status =
        run_scenario_subprocess(repo_root, &endpoint, &guest_exe, SESSION_INFO_TEST_NAME).map_err(
            |error| {
                format!("could not launch cargo test for the session_info precondition: {error}")
            },
        )?;
    if !session_status.success() {
        return Err(format!(
            "session_info precondition failed ({session_status}); the agent is not reporting an \
             interactive session, so no scenario downstream can work — see docs/tooling.md's \
             Troubleshooting section before looking at anything else"
        ));
    }

    // Every failure from here on is accumulated rather than returned
    // immediately: one scenario's failure must not skip the rest — with
    // several scenarios and occasional environment flakes, a run should
    // always report the complete picture, with no retry of anything.
    let mut errors = Vec::new();
    let mut summaries: Vec<(String, Option<ScenarioSummary>)> = Vec::new();

    for def in &selected {
        let summary = run_one_scenario(repo_root, &endpoint, &guest_exe, def.name, &mut errors);
        summaries.push((def.name.to_owned(), summary));
    }

    print_run_summary(&summaries);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Runs one scenario's subprocess ([`run_scenario_subprocess`]) and reads
/// back whatever [`ScenarioSummary`] its own run wrote. Every failure along
/// the way is pushed onto `errors` rather than returned, so one scenario's
/// trouble never skips the ones after it — see [`test`]'s own doc comment.
fn run_one_scenario(
    repo_root: &Path,
    endpoint: &str,
    guest_exe: &str,
    scenario_name: &str,
    errors: &mut Vec<String>,
) -> Option<ScenarioSummary> {
    println!("xtask vm test: running scenario '{scenario_name}'");

    // Clear this scenario's artifacts before its subprocess starts, so the
    // summary read back below can only be one this run wrote. The subprocess
    // clears the directory too, but only once it reaches the scenario runner;
    // a subprocess that dies or matches no test before that would otherwise
    // leave the previous run's summary to be read as this run's result.
    let dir = artifacts::scenario_dir(&artifacts::artifacts_root(), scenario_name);
    if let Err(error) = clear_scenario_artifacts(&dir) {
        errors.push(format!(
            "scenario '{scenario_name}': could not clear its previous artifacts at {}, so its \
             result cannot be trusted and it was not run: {error}",
            dir.display()
        ));
        return None;
    }

    let scenario_process = run_scenario_subprocess(repo_root, endpoint, guest_exe, scenario_name);
    let process_ok = match scenario_process {
        Ok(status) if status.success() => true,
        Ok(status) => {
            errors.push(format!("scenario '{scenario_name}' failed: {status}"));
            false
        }
        Err(error) => {
            errors.push(format!(
                "scenario '{scenario_name}': could not launch cargo test: {error}"
            ));
            false
        }
    };

    // Read back what the scenario's own subprocess wrote, rather than parsing
    // its stdout — see this module's own doc comment. The directory was
    // cleared before the subprocess started, so any summary here is this
    // run's own.
    let summary = ScenarioSummary::read(&dir).ok();

    // A selected scenario whose subprocess exited cleanly but wrote no summary
    // never actually ran its body, and that is a run FAILURE, never a pass: a
    // `--scenario` typo or a scenario missing from `tests/e2e.rs` makes
    // `cargo test --test e2e <name> -- --exact` match zero tests and still exit 0 (libtest
    // treats "no tests ran" as success), and a crash before
    // `verbatim_e2e::registry::run` writes the summary lands here too. This
    // holds only because the directory was cleared above; otherwise a
    // previous run's summary would be read in place of the missing one. A
    // subprocess that already failed to launch or exited non-zero pushed its
    // own error above, so this does not double-count it.
    if process_ok && summary.is_none() {
        errors.push(format!(
            "scenario '{scenario_name}' exited cleanly but wrote no run summary — it did not run \
             (a `--scenario` name matching no test, or a scenario missing from \
             tests/e2e.rs), or it crashed before writing the summary; treated as a failure, not a pass"
        ));
    }
    summary
}

/// Removes `dir` and everything in it; a directory that does not exist yet
/// is already clear and is not an error.
fn clear_scenario_artifacts(dir: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Prints [`registry::SCENARIOS`], one line per scenario naming it and its
/// group — `cargo xtask vm test --list`. Touches nothing else: no build, no
/// restore, no deploy.
fn print_scenario_list() {
    println!("available E2E scenarios (name, group):");
    for def in registry::SCENARIOS {
        println!("  {:<24} {}", def.name, def.group.name());
    }
}

/// The one-line-per-scenario report `cargo xtask vm test` prints at the end
/// of a multi-scenario run: pass or fail, and how many of that scenario's
/// latency timelines reached audio, when known.
fn print_run_summary(results: &[(String, Option<ScenarioSummary>)]) {
    println!("xtask vm test: run summary");
    for (name, summary) in results {
        let latency_text = summary.as_ref().map_or_else(
            || {
                "no run summary written — the scenario did not run (see the run error above)"
                    .to_owned()
            },
            |summary| {
                format!(
                    "latency: {} of {} reached audio; pipeline max {} ms, audio max {} ms",
                    format_optional_count(summary.latency_reached_audio),
                    format_optional_count(summary.latency_records),
                    format_optional_u64(summary.max_event_to_queue_ms),
                    format_optional_u64(summary.max_event_to_audio_ms),
                )
            },
        );
        println!(
            "  {:<4} {name:<28} {latency_text}",
            result_word(summary.as_ref())
        );
    }
}

/// The pass/fail word for a scenario's run summary. A missing summary is
/// always `"fail"`, never `"pass"`: a selected scenario that wrote no summary
/// did not run (a `--scenario` typo, a scenario missing from `tests/e2e.rs`) or crashed
/// before writing it — the same trap [`run_one_scenario`] records as a run
/// error. Pure, so the pass/fail policy is unit-tested directly.
fn result_word(summary: Option<&ScenarioSummary>) -> &'static str {
    match summary {
        Some(summary) if summary.passed => "pass",
        _ => "fail",
    }
}

fn format_optional_count(value: Option<usize>) -> String {
    value.map_or_else(|| "?".to_owned(), |count| count.to_string())
}

fn format_optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "?".to_owned(), |ms| ms.to_string())
}

/// Runs one scenario (or, for [`SESSION_INFO_TEST_NAME`], the `session_info`
/// precondition) as its own `cargo test -p verbatim-e2e --test e2e <test_name>
/// -- --exact --ignored --test-threads=1` subprocess against the guest, with the
/// environment `verbatim_e2e::Scenario::launch` needs for a remote, audible,
/// recorded run. See this module's own doc comment for why one subprocess per
/// scenario is the scenario-boundary design this harness uses.
///
/// # Errors
///
/// Returns an error if the subprocess cannot be launched at all (the
/// subprocess's own test failure is reported through its `ExitStatus`
/// instead, not this `Err`).
fn run_scenario_subprocess(
    repo_root: &Path,
    endpoint: &str,
    guest_exe: &str,
    test_name: &str,
) -> io::Result<ExitStatus> {
    let mut command = Command::new(env!("CARGO"));
    command
        .args([
            "test",
            "-p",
            "verbatim-e2e",
            "--test",
            "e2e",
            test_name,
            "--",
            "--exact",
            // Every live test is ignored, so a plain `cargo test` lists it as
            // ignored rather than running it; the suite runs it explicitly.
            "--ignored",
            "--test-threads=1",
        ])
        .env("VERBATIM_E2E_ENDPOINT", endpoint)
        .env("VERBATIM_E2E_VERBATIM_EXE", guest_exe)
        .env("VERBATIM_E2E_REMOTE", "1")
        .env("VERBATIM_E2E_AUDIBLE", "1")
        .env("VERBATIM_E2E_FFMPEG", FFMPEG_GUEST_PATH)
        .current_dir(repo_root);
    command.status()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_summary_is_a_failure_never_a_pass() {
        // A selected scenario that wrote no summary did not run (a typo, a
        // missing wrapper) or crashed before writing it — never a pass.
        assert_eq!(result_word(None), "fail");
    }

    #[test]
    fn clearing_removes_a_previous_runs_summary() {
        let dir = std::env::temp_dir()
            .join("xtask-vm-test-tests")
            .join("stale-summary");
        ScenarioSummary::new("scenario", true, None)
            .write(&dir)
            .expect("plants a previous run's passing summary");

        clear_scenario_artifacts(&dir).expect("clears the directory");

        assert!(
            ScenarioSummary::read(&dir).is_err(),
            "a previous run's summary must not survive to be read as this run's result"
        );
        clear_scenario_artifacts(&dir).expect("an already-clear directory is not an error");
    }

    #[test]
    fn a_present_summary_reports_its_own_pass_or_fail() {
        let passed = ScenarioSummary::new("scenario", true, None);
        let failed = ScenarioSummary::new("scenario", false, None);
        assert_eq!(result_word(Some(&passed)), "pass");
        assert_eq!(result_word(Some(&failed)), "fail");
    }
}
