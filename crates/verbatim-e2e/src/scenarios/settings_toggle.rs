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
//! on the switch. On GitHub's hosted runner (Windows Server) Settings
//! opened its System page instead, with focus in its search box, even
//! when asked twice. So when the switch is not heard, the scenario goes
//! there as a user would: Tab to the System page's list, arrow to
//! "Clipboard", and press Enter.
//!
//! The switch is a real setting, so the scenario waits for the page to
//! settle before pressing Space, and presses it only while the switch has
//! focus.

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

/// Goes to the Clipboard page from the System page, as a user would: Tab
/// to the page's list of settings, arrow down to "Clipboard", and press
/// Enter. Returns the switch's announcement, or `None` when it was never
/// heard.
fn navigate_to_clipboard(scenario: &mut Scenario) -> Option<String> {
    let mut in_list = false;
    for _ in 0..MAX_TABS {
        scenario.send_keys(&["tab"]).expect("sends tab");
        if scenario
            .speech()
            .heard_within("Display", TAB_TIMEOUT)
            .is_some_and(|heard| heard.contains(" of "))
        {
            in_list = true;
            break;
        }
    }
    if !in_list {
        return None;
    }
    for _ in 0..MAX_TABS * 2 {
        scenario.send_keys(&["downarrow"]).expect("sends downarrow");
        if let Some(item) = scenario.speech().heard_within(" of ", TAB_TIMEOUT)
            && item.starts_with("Clipboard")
        {
            scenario.send_keys(&["enter"]).expect("sends enter");
            return scenario
                .speech()
                .heard_within("Clipboard history", STEP_TIMEOUT);
        }
    }
    None
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
    if heard.is_none() {
        heard = navigate_to_clipboard(scenario);
    }
    let arrival = heard.unwrap_or_else(|| {
        panic!(
            "never reached the Clipboard history switch; heard:
{}",
            scenario.speech().transcript()
        )
    });
    // Let the page finish loading, so nothing moves focus off the switch
    // after Space.
    scenario
        .speech()
        .wait_until_quiet(Duration::from_millis(700), STEP_TIMEOUT);
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
