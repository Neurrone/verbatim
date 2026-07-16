//! The Start menu (milestone M3's "Start menu + search" shell item),
//! scoped to what is deterministic on a headless guest: pressing the
//! Windows key opens the Start/Search surface and Verbatim announces its
//! search box.
//!
//! Deliberately minimal. Unlike Explorer and the Settings app — broker
//! surfaces that do not reliably take the foreground through the harness on
//! a freshly-restored guest — the Start menu opens on a real key press and
//! takes the foreground the ordinary way, so its opening announcement is
//! stable to assert. What is *not* asserted is navigating the search
//! results: those are virtualized Web-content panes that report selection
//! rather than focus as the highlight moves, an async surface that does not
//! settle predictably under the suite's pace. Reading them is verified by
//! hand (see `docs/tooling.md`); this scenario guards the reliable half —
//! that the shell's search surface reaches Verbatim at all.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Generous budget: the first Start open on a guest can be slow.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    // Nothing external: the Windows key opens Start.
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // The Windows key opens the Start/Search surface; focus lands in its
    // search box, announced by name and role. The window and the edit both
    // carry "Search"; the edit's "edit" role is the stable settled focus.
    scenario
        .send_keys(&["leftwindows"])
        .expect("sends the Windows key");
    scenario
        .speech()
        .expect_in_order(&["Search", "edit"], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, _state: ScenarioState) {
    // Close Start so it does not linger foreground into the next scenario;
    // the golden restore cleans up regardless.
    let _ = scenario.send_keys(&["escape"]);
}
