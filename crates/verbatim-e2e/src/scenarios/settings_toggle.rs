//! A toggle in the Settings app (roadmap M3's Settings-app scenario,
//! deferred until runs moved off the freshly restored VM guest).
//!
//! Opens the Clipboard page, whose first control is the "Clipboard
//! history" switch, and presses Space twice, so the user's setting ends as
//! it began. Expected readings were taken from NVDA with the transcript
//! tool (`docs/nvda-transcript.md`) on 2026-10-06: "Clipboard history
//! toggle button not pressed" on arrival, then "pressed", then "not
//! pressed". Which state the switch starts in depends on the machine, so
//! the scenario asserts that each press announces the opposite state.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

const STEP_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_settings_page("ms-settings:clipboard")?;
    Ok(ScenarioState::None)
}

/// The state an announcement ends with: "not pressed" or "pressed".
fn toggle_state(text: &str) -> &'static str {
    if text.contains("not pressed") {
        "not pressed"
    } else {
        "pressed"
    }
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let arrival = scenario
        .speech()
        .expect_in_order_capturing(&["Clipboard history", "toggle button"], STEP_TIMEOUT);
    let first = toggle_state(&arrival);

    scenario.send_keys(&["space"]).expect("sends space");
    let toggled = scenario
        .speech()
        .expect_in_order_capturing(&["pressed"], STEP_TIMEOUT);
    assert_ne!(
        toggle_state(&toggled),
        first,
        "the first Space should announce the opposite of {first:?}, heard {toggled:?}"
    );

    scenario.send_keys(&["space"]).expect("sends space");
    let restored = scenario
        .speech()
        .expect_in_order_capturing(&["pressed"], STEP_TIMEOUT);
    assert_eq!(
        toggle_state(&restored),
        first,
        "the second Space should restore {first:?}, heard {restored:?}"
    );
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // The Settings app is ended by image name when the scenario ends.
}
