//! A synthesizer host that dies is replaced (decision D18): speech goes on
//! after `verbatim-synth-host.exe` is killed. Verbatim's menu is opened and
//! heard, the host is ended from outside, and the next announcement must
//! still be heard in full, from a host Verbatim started in its place.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Per-step budget, matching the other scenarios that drive the menu.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    // Nothing external: the scenario drives Verbatim's own menu.
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_verbatim_menu(scenario, STEP_TIMEOUT);

    let ended = scenario
        .kill_processes_by_name("verbatim-synth-host.exe")
        .expect("ends the synthesizer host");
    assert!(ended >= 1, "a synthesizer host was running");

    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["Settings..."], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, _state: ScenarioState) {
    // Close the menu again.
    let _ = scenario.send_keys(&["escape"]);
}
