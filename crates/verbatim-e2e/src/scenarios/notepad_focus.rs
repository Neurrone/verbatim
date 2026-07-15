//! Regression: launching Notepad brings its focus announcement through
//! Verbatim, and Verbatim keeps running after Notepad exits.
//!
//! Windows 11's Notepad is tabbed, so the window-name check is generous (a
//! substring match on "Notepad") rather than an exact title. Deliberately
//! minimal: typed-character echo is a later milestone, so this proves only
//! that focus tracking reaches a second, real application and that losing it
//! does not take Verbatim down too.
//!
//! Ported into the scenario registry (milestone M3 Track B) from what used
//! to be `crates/verbatim-e2e/tests/notepad_focus.rs`'s whole test body;
//! that file is now the thin `#[test]` wrapper calling
//! [`crate::registry::run_named`].

use std::io;
use std::time::Duration;

use verbatim_control::client::ok_or_error;
use verbatim_control::protocol::Request;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let pid = scenario.launch_target("notepad.exe", &[])?;
    Ok(ScenarioState::TargetPid(pid))
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    scenario
        .speech()
        .expect_in_order(&["Notepad"], Duration::from_secs(10));
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature: teardown owns and consumes what setup produced, matching notepad_focus::setup's return"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    if let ScenarioState::TargetPid(pid) = state {
        scenario
            .kill_target(pid)
            .expect("kills notepad through the agent");
    }

    // The actual regression under test: Verbatim must still answer requests
    // after the application it was watching has gone away.
    let status = ok_or_error(
        scenario
            .control()
            .request(Request::Status)
            .expect("sends Status"),
    )
    .expect("Verbatim still answers Status after Notepad exits");
    println!("status after notepad exit: {status:?}");
}
