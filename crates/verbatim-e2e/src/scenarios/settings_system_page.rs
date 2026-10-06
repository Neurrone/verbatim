//! Reading the Settings app (roadmap M3's Settings-app scenario, deferred
//! until runs moved off the freshly restored VM guest): its System page's
//! list of settings, each item with its position.
//!
//! Read-only, so it changes no setting, and the same everywhere it runs:
//! `ms-settings:system` opens the System page both on this machine and on
//! GitHub's hosted runner (Windows Server), where other pages asked for by
//! URI did not open. The scenario waits for the page to load (its window
//! announced, and everything queued ended, since the app moves focus on
//! its own while loading), then Tabs to the list one control at a time,
//! each Tab waiting until what it caused has been spoken. How many controls
//! lie between differs between the two (an account button, the
//! navigation, links), so the scenario Tabs until it hears the list. Item names and counts differ between
//! Windows editions; the list's first item, Display, does not.
//!
//! The Settings app's switches, which this scenario first toggled on the
//! Clipboard page, are verified by mockapp's cross-process test of the
//! toggle mapping and the reducer's tests of their wording.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

const STEP_TIMEOUT: Duration = Duration::from_secs(15);
/// How many Tabs to try before giving up on reaching the list.
const MAX_TABS: usize = 15;

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_settings_page("ms-settings:system")?;
    Ok(ScenarioState::None)
}

/// Presses Tab and waits for everything it caused to be spoken: the first
/// new announcement, then until every queued utterance has ended, since a
/// Tab into a new container announces the container and then the control.
/// Returns the last announcement, the focused control's.
fn tab_and_hear(scenario: &mut Scenario, previous: &str) -> String {
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_change_capturing(previous, STEP_TIMEOUT);
    scenario.speech().wait_until_quiet(STEP_TIMEOUT);
    scenario.speech().last_heard().unwrap_or_default()
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // The page has loaded once its window is announced; the app may still
    // move focus on its own, so everything queued is let finish first.
    scenario
        .speech()
        .expect_in_order(&["Settings window"], STEP_TIMEOUT);
    scenario.speech().wait_until_quiet(STEP_TIMEOUT);
    let mut heard = scenario.speech().last_heard().unwrap_or_default();
    let mut tabs = 0;
    while !(heard.starts_with("Display") && heard.contains("1 of ")) {
        assert!(
            tabs < MAX_TABS,
            "never reached the System page's list after {MAX_TABS} Tabs; heard:
{}",
            scenario.speech().transcript()
        );
        heard = tab_and_hear(scenario, &heard);
        tabs += 1;
    }

    // Down moves to the second item, announced with its position; Up
    // returns to Display.
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect_in_order(&["2 of "], STEP_TIMEOUT);
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario
        .speech()
        .expect_in_order(&["Display", "1 of "], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // The Settings app is ended by image name when the scenario ends.
}
