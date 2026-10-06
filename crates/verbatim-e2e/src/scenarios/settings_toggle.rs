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
//!
//! What opens depends on the machine too. Here the page opens with focus
//! on the switch. On GitHub's hosted runner, the Settings app's first,
//! cold start opened its System page instead, ignoring the page asked
//! for, with focus in its search box. So when the switch is not heard,
//! the scenario asks for the page again, which the running app honors,
//! and then presses Tab until it hears the switch, up to a limit.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// The Settings page the scenario opens.
const PAGE: &str = "ms-settings:clipboard";
const STEP_TIMEOUT: Duration = Duration::from_secs(20);
/// How long to listen for the switch after each Tab.
const TAB_TIMEOUT: Duration = Duration::from_secs(3);
/// How many Tabs to try before giving up: the switch is the page's first
/// control after the navigation pane and the search box.
const MAX_TABS: usize = 15;

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_settings_page(PAGE)?;
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
    let mut heard = scenario
        .speech()
        .heard_within("Clipboard history", STEP_TIMEOUT);
    if heard.is_none() {
        scenario
            .open_settings_page(PAGE)
            .expect("asks the running Settings app for the page again");
        heard = scenario
            .speech()
            .heard_within("Clipboard history", STEP_TIMEOUT);
    }
    let mut tabs = 0;
    while heard.is_none() && tabs < MAX_TABS {
        scenario.send_keys(&["tab"]).expect("sends tab");
        heard = scenario
            .speech()
            .heard_within("Clipboard history", TAB_TIMEOUT);
        tabs += 1;
    }
    let arrival = heard.unwrap_or_else(|| {
        panic!(
            "never heard the Clipboard history switch after {MAX_TABS} Tabs; heard:
{}",
            scenario.speech().transcript()
        )
    });
    assert!(
        arrival.contains("toggle button"),
        "the switch should be announced as a toggle button, heard {arrival:?}"
    );
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
