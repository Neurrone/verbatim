//! Diagnostic (not a gate): the Start menu opened twice, to measure the
//! first-press gap a maintainer reported.
//!
//! On the *first* Windows-key press after Verbatim starts there is a
//! noticeable gap between the "Search window" announcement and the search
//! box's "edit" announcement; the *second* press has no such gap. This
//! scenario reproduces both presses so the run's `timeline.txt` (written for
//! every run, pass or fail — see
//! [`crate::scenario::Scenario::collect_run_artifacts`]) carries the
//! millisecond offsets of both announcements, and Verbatim's `stderr.log`
//! carries the listener- and outpost-ready timestamps, so the gap can be
//! attributed between the candidate costs (outpost spawn wait, the first
//! arbitration probe, the two cold runtime-id `FindFirst` searches in the UIA
//! fact path, and OS-side focus timing).
//!
//! It is registered in [`crate::registry::Group::Diagnostic`], which
//! [`crate::registry::select`] deliberately excludes from the no-filter
//! default run, so an ordinary `cargo xtask vm test` never runs it — it is a
//! measurement tool, run explicitly with `--scenario start_menu_repeat` (or
//! `--group diagnostic`), not an acceptance gate. Each press asserts only the
//! search box's "edit" announcement — the window announcement can lose the
//! emission race and be dropped by the reducer's last-observation-wins rule,
//! and a measurement must complete either way; the timeline records whatever
//! was actually spoken.

use std::io;
use std::thread;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Generous budget: the first Start open on a guest can be slow — that
/// slowness is exactly what this scenario measures.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to let the shell settle after closing Start before pressing the
/// Windows key a second time, so the second open is a genuine warm reopen and
/// not a race against the first one's teardown.
const SETTLE_DELAY: Duration = Duration::from_secs(2);

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    // Nothing external: the Windows key opens Start.
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // First press: the interesting one. The Windows key opens the
    // Start/Search surface; the window announces as "Search window", then focus
    // settles into the search box, announced with the "edit" role. The gap
    // being investigated falls between these two announcements — the
    // timeline's millisecond offsets capture it.
    scenario
        .send_keys(&["leftwindows"])
        .expect("sends the Windows key (first press)");
    // Only the "edit" announcement is asserted: the window announcement can
    // legitimately lose the emission race and be dropped by the reducer's
    // last-observation-wins rule (observed live from this very scenario's
    // first run), and a measurement scenario must keep going either way —
    // the timeline records whatever was actually spoken.
    scenario.speech().expect_in_order(&["edit"], STEP_TIMEOUT);

    // Close Start and let the shell settle before the comparison press.
    scenario
        .send_keys(&["escape"])
        .expect("closes Start after the first press");
    thread::sleep(SETTLE_DELAY);

    // Second press: the comparison. The same two announcements, expected
    // without the first-press gap — the timeline shows the smaller offset.
    scenario
        .send_keys(&["leftwindows"])
        .expect("sends the Windows key (second press)");
    scenario.speech().expect_in_order(&["edit"], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, _state: ScenarioState) {
    // Close Start so it does not linger foreground; the golden restore cleans
    // up regardless.
    let _ = scenario.send_keys(&["escape"]);
}
