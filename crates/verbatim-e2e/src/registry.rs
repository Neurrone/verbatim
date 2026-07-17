//! The scenario registry (milestone M3 Track B): every live scenario as one
//! named, grouped, `#[non_exhaustive]`-free definition — [`ScenarioDef`] —
//! instead of logic living only inside a `#[test]` function.
//!
//! One identifier, [`ScenarioDef::name`], is used everywhere a scenario
//! needs naming: the plain-libtest `#[test]` function wrapping it (so
//! `cargo test -p verbatim-e2e <name> -- --exact` selects exactly one
//! scenario), `cargo xtask vm test --scenario <name>`'s selector, the
//! per-scenario artifacts directory ([`crate::artifacts::scenario_dir`]),
//! and the per-scenario recording filename `xtask` writes under
//! `artifacts/vm-recordings`. Keeping every one of those in lockstep off a
//! single `&'static str` is deliberate: a scenario renamed in one place is a
//! compile error (or an unmistakable "unknown scenario" message) everywhere
//! else, rather than a silently stale mapping maintained by hand.
//!
//! A scenario is setup, body, and teardown, each a plain function pointer
//! operating on the already-launched [`Scenario`]:
//!
//! - `setup` declares and creates whatever state the scenario's body needs
//!   beyond `Scenario::launch` itself already provides — today, just
//!   launching a target application such as Notepad and remembering its
//!   pid; a future scenario needing a scratch folder would create it here
//!   too.
//! - `body` is the scripted walk itself: gestures, keys, and speech
//!   assertions, exactly as today's `#[test]` functions already read, just
//!   moved into a free function instead of the test function directly.
//! - `teardown` restores whatever `setup` created — killing a launched
//!   target application, removing a scratch folder — and always runs, even
//!   when `body` panicked, because [`run`] wraps `body` (and `teardown`
//!   itself) in [`std::panic::catch_unwind`] rather than letting a body
//!   panic skip cleanup outright.
//!
//! None of this weakens the existing guard-struct discipline
//! [`crate::scenario::Scenario`]'s own doc comment describes: `Scenario`'s
//! `Drop` impl still unconditionally kills Verbatim and everything launched
//! through it, panic or not. `setup`/`body`/`teardown` are a layer of
//! structure *on top* of that guarantee, for scenario-specific state
//! `Scenario` itself does not track (and, looking ahead, does not need to:
//! a scratch folder needs no process-kill-style guard, just an ordinary
//! `Drop` or an explicit teardown step) — not a replacement for it.
//!
//! Groups (`docs/roadmap.md`'s M3 track) are a coarse selector for
//! `cargo xtask vm test --group`, not a strict taxonomy:
//!
//! - [`Group::Speech`]: Verbatim's own menu and Speech settings dialog, and
//!   the latency reporting built into walking them —
//!   [`m1_exit_regression`](crate::scenarios::m1_exit_regression) today.
//! - [`Group::Shell`]: the Windows shell — switching foreground between
//!   applications (the "task switching" item `docs/roadmap.md`'s M3 E2E
//!   list names,
//!   [`multi_outpost_switch`](crate::scenarios::multi_outpost_switch)) and
//!   opening the Start/Search surface
//!   ([`start_menu`](crate::scenarios::start_menu)).
//! - [`Group::Legacy`]: a real external target application, standing in for
//!   the "at least one MSAA-only legacy app" M3 exit item until a
//!   genuinely MSAA-only one is chosen —
//!   [`notepad_focus`](crate::scenarios::notepad_focus) today.
//! - [`Group::Navigation`]: the M3 object-navigation and review-cursor
//!   commands (`docs/roadmap.md`'s M3 section) —
//!   [`object_navigation`](crate::scenarios::object_navigation) against
//!   Verbatim's own settings dialog, and
//!   [`tree_navigation`](crate::scenarios::tree_navigation) against
//!   msinfo32's real Win32 tree view over MSAA.
//! - [`Group::Diagnostic`]: measurement tools, not gates —
//!   [`start_menu_repeat`](crate::scenarios::start_menu_repeat), which opens
//!   the Start menu twice to measure a reported first-press gap. Unlike the
//!   other groups, [`select`] excludes this one from the no-filter default
//!   run, so it is only ever run when named explicitly (see [`select`]).

use std::io;
use std::panic::{self, AssertUnwindSafe};

use verbatim_control::protocol::LatencyRecord;

use crate::artifacts::{self, ScenarioSummary};
use crate::scenario::Scenario;
use crate::scenarios::{
    m1_exit_regression, msinfo32, multi_outpost_switch, notepad_focus, object_navigation,
    start_menu, start_menu_repeat, tree_navigation,
};

/// A coarse selector for `cargo xtask vm test --group` — see this module's
/// own doc comment for what each group is meant to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Verbatim's own menu and Speech settings dialog, and latency
    /// reporting.
    Speech,
    /// Switching foreground between applications ("task switching").
    Shell,
    /// A real external, non-Verbatim target application.
    Legacy,
    /// Object navigation and review-cursor commands.
    Navigation,
    /// A measurement tool, not an acceptance gate. Scenarios in this group are
    /// deliberately excluded from [`select`]'s no-filter default run, so an
    /// ordinary `cargo xtask vm test` never runs them; they are selected
    /// explicitly with `--scenario <name>` or `--group diagnostic`. Reserved
    /// for scenarios whose value is the artifacts they leave behind (timings,
    /// stderr) rather than a pass/fail verdict a suite should gate on.
    Diagnostic,
}

impl Group {
    /// The lowercase name used on the command line and in [`ScenarioDef`]
    /// listings — `cargo xtask vm test --list`'s own output, and
    /// [`select`]'s `--group` matching.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Speech => "speech",
            Self::Shell => "shell",
            Self::Legacy => "legacy",
            Self::Navigation => "navigation",
            Self::Diagnostic => "diagnostic",
        }
    }

    /// Parses a group name (case-sensitive, matching [`Group::name`]
    /// exactly), for `--group` argument validation.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "speech" => Some(Self::Speech),
            "shell" => Some(Self::Shell),
            "legacy" => Some(Self::Legacy),
            "navigation" => Some(Self::Navigation),
            "diagnostic" => Some(Self::Diagnostic),
            _ => None,
        }
    }
}

/// Scenario-specific state a [`ScenarioDef::setup`] creates and its matching
/// [`ScenarioDef::teardown`] restores. `None` when a scenario needs nothing
/// beyond `Scenario::launch` itself; `TargetPid` names a process launched via
/// [`Scenario::launch_target`] that teardown must kill.
#[derive(Debug)]
pub enum ScenarioState {
    /// No extra state: `setup` did nothing beyond validating preconditions.
    None,
    /// The pid of a target application `setup` launched via
    /// [`Scenario::launch_target`], for `teardown` to kill.
    TargetPid(u32),
}

/// One named, grouped scenario. See this module's own doc comment for the
/// setup/body/teardown contract and why [`name`](Self::name) is the single
/// identifier used everywhere.
#[derive(Debug)]
pub struct ScenarioDef {
    /// Selects this scenario: its `#[test]` function name
    /// (`cargo test -p verbatim-e2e <name> -- --exact`), its
    /// `cargo xtask vm test --scenario <name>` selector, its artifacts
    /// directory name, and its recording file name prefix.
    pub name: &'static str,
    /// The coarse group this scenario belongs to, for `--group` selection.
    pub group: Group,
    /// Image (executable file) names this scenario's `setup` or `teardown`
    /// may launch or kill — [`swept_target_image_names`] unions these across
    /// the whole registry so [`Scenario::launch`]'s pre-launch sweep grows
    /// automatically as scenarios are added, with nothing to remember to
    /// update by hand.
    pub target_images: &'static [&'static str],
    /// Declares and creates whatever state `body` needs beyond
    /// `Scenario::launch` itself.
    ///
    /// # Errors
    ///
    /// Returns an error if creating that state fails (for example, the
    /// agent refuses to launch a target application).
    pub setup: fn(&mut Scenario) -> io::Result<ScenarioState>,
    /// The scripted walk: gestures, keys, and speech assertions. Panics
    /// (via `assert!`/`expect`/`panic!`, exactly like today's `#[test]`
    /// bodies) are how a scenario failure is reported; [`run`] catches them
    /// to collect failure artifacts before letting the panic continue.
    pub body: fn(&mut Scenario, &mut ScenarioState),
    /// Restores whatever `setup` created. Always runs, even when `body`
    /// panicked — see [`run`].
    pub teardown: fn(&mut Scenario, ScenarioState),
}

/// Every registered scenario, in a fixed, stable order (also the order
/// [`select`] preserves and `--list` prints in).
pub const SCENARIOS: &[ScenarioDef] = &[
    ScenarioDef {
        name: "m1_exit_regression",
        group: Group::Speech,
        target_images: &[],
        setup: m1_exit_regression::setup,
        body: m1_exit_regression::body,
        teardown: m1_exit_regression::teardown,
    },
    ScenarioDef {
        name: "notepad_focus",
        group: Group::Legacy,
        target_images: &["notepad.exe"],
        setup: notepad_focus::setup,
        body: notepad_focus::body,
        teardown: notepad_focus::teardown,
    },
    ScenarioDef {
        name: "multi_outpost_switch",
        group: Group::Shell,
        target_images: &["notepad.exe"],
        setup: multi_outpost_switch::setup,
        body: multi_outpost_switch::body,
        teardown: multi_outpost_switch::teardown,
    },
    ScenarioDef {
        name: "object_navigation",
        group: Group::Navigation,
        target_images: &[],
        setup: object_navigation::setup,
        body: object_navigation::body,
        teardown: object_navigation::teardown,
    },
    ScenarioDef {
        name: "msinfo32",
        group: Group::Legacy,
        target_images: &["msinfo32.exe"],
        setup: msinfo32::setup,
        body: msinfo32::body,
        teardown: msinfo32::teardown,
    },
    ScenarioDef {
        name: "start_menu",
        group: Group::Shell,
        target_images: &[],
        setup: start_menu::setup,
        body: start_menu::body,
        teardown: start_menu::teardown,
    },
    ScenarioDef {
        name: "tree_navigation",
        group: Group::Navigation,
        target_images: &["msinfo32.exe"],
        setup: tree_navigation::setup,
        body: tree_navigation::body,
        teardown: tree_navigation::teardown,
    },
    ScenarioDef {
        name: "start_menu_repeat",
        // A diagnostic, not a gate: excluded from the no-filter default run
        // (see `select`), selected explicitly to measure the first-press gap.
        group: Group::Diagnostic,
        target_images: &[],
        setup: start_menu_repeat::setup,
        body: start_menu_repeat::body,
        teardown: start_menu_repeat::teardown,
    },
];

/// Looks up a scenario by [`ScenarioDef::name`].
#[must_use]
pub fn find(name: &str) -> Option<&'static ScenarioDef> {
    SCENARIOS.iter().find(|def| def.name == name)
}

/// The deduplicated union of every registered scenario's
/// [`ScenarioDef::target_images`] — what [`Scenario::launch`] sweeps clean
/// before every run. Replaces a hand-maintained constant: a scenario that
/// launches a new target application widens this sweep just by declaring it
/// in its own [`ScenarioDef`], with nothing else to remember to update.
#[must_use]
pub fn swept_target_image_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = SCENARIOS
        .iter()
        .flat_map(|def| def.target_images.iter().copied())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Resolves `--scenario` and `--group` selections against `scenarios`
/// (always [`SCENARIOS`] outside tests) into an ordered, deduplicated list
/// of matching definitions, preserving registry order. Empty `names` and
/// `groups` selects every scenario *except* the [`Group::Diagnostic`] ones —
/// the default, no-flags behavior of `cargo xtask vm test`, which runs the
/// acceptance gate but not the measurement-only diagnostics. A diagnostic is
/// still reachable by naming it (`--scenario`) or its group (`--group
/// diagnostic`), both of which honor an explicit request.
///
/// # Errors
///
/// Returns the first unrecognized scenario name or group name as `Err`, so a
/// typo is reported before anything is built or restored, not discovered
/// partway through a run.
pub fn select<'a>(
    scenarios: &'a [ScenarioDef],
    names: &[String],
    groups: &[String],
) -> Result<Vec<&'a ScenarioDef>, String> {
    if names.is_empty() && groups.is_empty() {
        return Ok(scenarios
            .iter()
            .filter(|def| def.group != Group::Diagnostic)
            .collect());
    }

    let mut parsed_groups = Vec::with_capacity(groups.len());
    for group in groups {
        parsed_groups
            .push(Group::parse(group).ok_or_else(|| format!("unknown scenario group {group:?}"))?);
    }
    for name in names {
        if !scenarios.iter().any(|def| def.name == name) {
            return Err(format!("unknown scenario {name:?}"));
        }
    }

    Ok(scenarios
        .iter()
        .filter(|def| {
            names.iter().any(|name| name == def.name) || parsed_groups.contains(&def.group)
        })
        .collect())
}

/// Looks up `name` and runs it, or panics if no such scenario is registered
/// — the thin body every `#[test]` wrapper under `crates/verbatim-e2e/tests/`
/// calls. Prints the same one-line skip notice every live test in this crate
/// prints, and returns without running anything, when
/// [`crate::ENDPOINT_ENV`] is unset.
///
/// # Panics
///
/// Panics if `name` is not registered, if launching Verbatim fails, or if
/// the scenario itself fails (setup, body, teardown, or the final clean
/// quit) — the ordinary way a `#[test]` reports failure.
pub fn run_named(name: &str) {
    let Some(def) = find(name) else {
        panic!(
            "no scenario named {name:?} is registered; known scenarios: {}",
            SCENARIOS
                .iter()
                .map(|def| def.name)
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    let Some(_endpoint) = crate::endpoint() else {
        println!("VERBATIM_E2E_ENDPOINT is not set; skipping the live E2E suite");
        return;
    };
    run(def);
}

/// Runs one scenario end to end: launch, setup, body, teardown, a final
/// clean quit, failure-artifact collection, and a [`ScenarioSummary`]
/// written to this scenario's artifacts directory regardless of outcome.
///
/// `body` and `teardown` each run inside their own
/// [`std::panic::catch_unwind`], not one shared one, so a panic in `body`
/// still lets `teardown` run with whatever state `setup` produced (borrowed,
/// not moved, into `body`'s closure — a panic there leaves `state` itself
/// intact for `teardown` to consume next) rather than skipping cleanup
/// outright. This is deliberately not a retry of anything: each of `body`
/// and `teardown` runs exactly once, and a panic from either is re-raised
/// (via [`std::panic::resume_unwind`] or a fresh `panic!`) once cleanup and
/// artifact collection have both had their turn, so `cargo test` still
/// reports the scenario as failed with the original panic message.
fn run(def: &ScenarioDef) {
    let dir = artifacts::scenario_dir(&artifacts::artifacts_root(), def.name);
    // Clear any artifacts a previous run of this same scenario left behind,
    // so a stale failure directory is never mistaken for this run's own.
    let _ = std::fs::remove_dir_all(&dir);

    let mut scenario = Scenario::launch().unwrap_or_else(|error| {
        panic!(
            "scenario {:?}: launching Verbatim through the agent failed: {error}",
            def.name
        )
    });

    let mut state = match (def.setup)(&mut scenario) {
        Ok(state) => state,
        Err(error) => {
            scenario.collect_run_artifacts(&dir);
            scenario.collect_flight_recorder(&dir);
            // Setup failed before any input was driven, so a latency
            // snapshot here would be empty; record none rather than racing
            // the imminent quit for nothing.
            let latency = scenario.latency_snapshot(200).ok();
            write_summary(&dir, def.name, false, latency.as_deref());
            drop(scenario);
            panic!("scenario {:?}: setup failed: {error}", def.name);
        }
    };

    let body_outcome =
        panic::catch_unwind(AssertUnwindSafe(|| (def.body)(&mut scenario, &mut state)));
    let teardown_outcome =
        panic::catch_unwind(AssertUnwindSafe(|| (def.teardown)(&mut scenario, state)));

    // Snapshot latency now, while Verbatim is still up and its control
    // connection still answers — before the quit below tears it down. Taking
    // it after the quit is why a passing scenario used to record "unknown"
    // latency (the snapshot raced Verbatim's exit and lost); a failing one
    // reported real numbers only because it skips the quit.
    let latency = scenario.latency_snapshot(200).ok();

    // Dump the reducer flight recorder for every run, pass or fail, while
    // Verbatim is still up (a passing run quits below; a failing one skipped
    // the quit, so Verbatim is up here either way). It captures the reducer
    // inputs a passing run leaves no other trace of — wanted for chasing
    // symptoms the pass/fail verdict alone does not explain.
    scenario.collect_flight_recorder(&dir);

    // The M3 pipeline latency budget: reported prominently on a breach,
    // never asserted (a recorded decision — see PIPELINE_BUDGET_MS's doc
    // comment). Two clean-rerun episodes showed breaches tracking guest
    // scheduling load, not code, so a hard assertion here only manufactured
    // flaky runs; the per-scenario summary always carries the measured
    // maxima either way, and the assertion returns with eSpeak's controlled
    // reference measurement in M8.
    if let Some(worst) = latency
        .as_deref()
        .and_then(crate::latency::max_event_to_queue_ms)
        .filter(|worst| *worst > crate::latency::PIPELINE_BUDGET_MS)
    {
        println!(
            "scenario {:?}: WARNING: pipeline latency budget exceeded: worst event-to-queue {worst} ms > {} ms (reported, not asserted)",
            def.name,
            crate::latency::PIPELINE_BUDGET_MS
        );
    }

    // A clean quit is asserted only when body and teardown both succeeded:
    // an already-failed scenario's Verbatim may be in any state, and
    // Scenario::drop already guarantees it is killed regardless, so there is
    // nothing more to prove by also asserting a graceful quit on top of a
    // failure already reported.
    let quit_outcome = if body_outcome.is_ok() && teardown_outcome.is_ok() {
        scenario
            .quit_verbatim()
            .map_err(|error| format!("quit_verbatim failed: {error}"))
    } else {
        Ok(())
    };

    let passed = body_outcome.is_ok() && teardown_outcome.is_ok() && quit_outcome.is_ok();
    // The timeline and stderr log are written for every run, pass or fail (so a
    // passing diagnostic leaves its timings behind). The flight-recorder dump
    // already happened above, before the quit, for every run.
    scenario.collect_run_artifacts(&dir);
    write_summary(&dir, def.name, passed, latency.as_deref());
    println!(
        "scenario {:?}: {}",
        def.name,
        if passed { "pass" } else { "fail" }
    );

    drop(scenario);

    if let Err(payload) = body_outcome {
        panic::resume_unwind(payload);
    }
    if let Err(payload) = teardown_outcome {
        panic::resume_unwind(payload);
    }
    if let Err(reason) = quit_outcome {
        panic!("scenario {:?}: {reason}", def.name);
    }
}

/// Fetches a best-effort latency snapshot (never asserted — see
/// [`crate::scenario::Scenario::latency_snapshot`]) and writes this
/// scenario's [`ScenarioSummary`], logging rather than failing the run if
/// the write itself fails.
fn write_summary(
    dir: &std::path::Path,
    name: &str,
    passed: bool,
    latency: Option<&[LatencyRecord]>,
) {
    let summary = ScenarioSummary::new(name, passed, latency);
    if let Err(error) = summary.write(dir) {
        eprintln!("scenario {name:?}: could not write its run summary: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<ScenarioDef> {
        #[allow(
            clippy::unnecessary_wraps,
            reason = "must match ScenarioDef::setup's fn-pointer signature"
        )]
        fn no_setup(_: &mut Scenario) -> io::Result<ScenarioState> {
            Ok(ScenarioState::None)
        }
        fn no_body(_: &mut Scenario, _: &mut ScenarioState) {}
        #[allow(
            clippy::needless_pass_by_value,
            reason = "must match ScenarioDef::teardown's fn-pointer signature"
        )]
        fn no_teardown(_: &mut Scenario, _: ScenarioState) {}

        vec![
            ScenarioDef {
                name: "alpha",
                group: Group::Speech,
                target_images: &["alpha.exe"],
                setup: no_setup,
                body: no_body,
                teardown: no_teardown,
            },
            ScenarioDef {
                name: "beta",
                group: Group::Shell,
                target_images: &["beta.exe", "alpha.exe"],
                setup: no_setup,
                body: no_body,
                teardown: no_teardown,
            },
            ScenarioDef {
                name: "gamma",
                group: Group::Shell,
                target_images: &[],
                setup: no_setup,
                body: no_body,
                teardown: no_teardown,
            },
            ScenarioDef {
                name: "delta",
                group: Group::Diagnostic,
                target_images: &[],
                setup: no_setup,
                body: no_body,
                teardown: no_teardown,
            },
        ]
    }

    #[test]
    fn group_parse_round_trips_every_variant_name() {
        for group in [
            Group::Speech,
            Group::Shell,
            Group::Legacy,
            Group::Navigation,
            Group::Diagnostic,
        ] {
            assert_eq!(Group::parse(group.name()), Some(group));
        }
    }

    #[test]
    fn group_parse_rejects_unknown_names() {
        assert_eq!(Group::parse("nonsense"), None);
        assert_eq!(Group::parse("Speech"), None, "matching is case-sensitive");
    }

    #[test]
    fn find_locates_a_real_registered_scenario() {
        assert!(find("m1_exit_regression").is_some());
        assert!(find("notepad_focus").is_some());
        assert!(find("multi_outpost_switch").is_some());
        assert!(find("no_such_scenario").is_none());
    }

    #[test]
    fn swept_target_image_names_is_deduplicated_and_sorted() {
        let names = swept_target_image_names();
        assert_eq!(names, {
            let mut expected = names.clone();
            expected.sort_unstable();
            expected.dedup();
            expected
        });
        assert!(names.contains(&"notepad.exe"));
    }

    #[test]
    fn select_with_no_filters_returns_every_non_diagnostic_scenario_in_order() {
        let scenarios = fixture();
        let selected = select(&scenarios, &[], &[]).expect("no filters never errors");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(
            names,
            vec!["alpha", "beta", "gamma"],
            "the no-filter default excludes the diagnostic scenario (delta)"
        );
    }

    #[test]
    fn select_by_name_reaches_a_diagnostic_scenario() {
        let scenarios = fixture();
        let selected = select(&scenarios, &["delta".to_owned()], &[])
            .expect("delta is a real diagnostic scenario");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(
            names,
            vec!["delta"],
            "naming a diagnostic explicitly still selects it"
        );
    }

    #[test]
    fn select_by_diagnostic_group_reaches_it() {
        let scenarios = fixture();
        let selected = select(&scenarios, &[], &["diagnostic".to_owned()])
            .expect("diagnostic is a real group");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(
            names,
            vec!["delta"],
            "asking for the diagnostic group runs the diagnostics"
        );
    }

    #[test]
    fn select_by_name_picks_exactly_that_scenario() {
        let scenarios = fixture();
        let selected =
            select(&scenarios, &["beta".to_owned()], &[]).expect("beta is a real scenario");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(names, vec!["beta"]);
    }

    #[test]
    fn select_by_group_picks_every_scenario_in_that_group() {
        let scenarios = fixture();
        let selected =
            select(&scenarios, &[], &["shell".to_owned()]).expect("shell is a real group");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(names, vec!["beta", "gamma"]);
    }

    #[test]
    fn select_unions_names_and_groups_without_duplicating_overlap() {
        let scenarios = fixture();
        let selected = select(&scenarios, &["beta".to_owned()], &["shell".to_owned()])
            .expect("both beta and shell are real");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(
            names,
            vec!["beta", "gamma"],
            "beta must not appear twice just because it matches both the name and the group filter"
        );
    }

    #[test]
    fn select_rejects_an_unknown_scenario_name() {
        let scenarios = fixture();
        let error = select(&scenarios, &["no_such_scenario".to_owned()], &[])
            .expect_err("unknown scenario name must be rejected");
        assert!(error.contains("no_such_scenario"));
    }

    #[test]
    fn select_rejects_an_unknown_group_name() {
        let scenarios = fixture();
        let error = select(&scenarios, &[], &["no_such_group".to_owned()])
            .expect_err("unknown group name must be rejected");
        assert!(error.contains("no_such_group"));
    }
}
