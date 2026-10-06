//! The settings dialog's keys (audit item 7, decided with Dickson on
//! 2026-10-06; `crates/verbatim-gui/src/keys.rs`): Enter on a focused
//! button activates that button, so Enter on Cancel cancels and Enter on
//! Apply applies and keeps the dialog open. NVDA's own settings dialogs
//! send every Enter to OK; Verbatim's difference is recorded in
//! `docs/parity.md`.
//!
//! The walk: open the Speech settings, change the rate, Tab to Cancel and
//! press Enter, reopen, and hear the rate unchanged; change it again, Tab
//! to Apply and press Enter, hear that focus stays on Apply, close with
//! Escape, reopen, and hear the applied rate.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

const STEP_TIMEOUT: Duration = Duration::from_secs(15);
/// Controls between the rate slider and the dialog's buttons are tabbed
/// through one at a time, up to this many.
const MAX_TABS_TO_BUTTON: u32 = 12;

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::None)
}

/// The last number in an announcement, such as a slider's value.
fn trailing_number(text: &str) -> Option<i64> {
    text.split(|c: char| !c.is_ascii_digit())
        .rfind(|run| !run.is_empty())
        .and_then(|run| run.parse().ok())
}

/// Opens the Speech settings and Tabs to the rate slider, returning its
/// value.
fn open_at_rate(scenario: &mut Scenario) -> i64 {
    super::open_speech_settings(scenario, STEP_TIMEOUT);
    let mut heard = String::new();
    for _ in 0..MAX_TABS_TO_BUTTON {
        scenario.send_keys(&["tab"]).expect("sends tab");
        heard = scenario
            .speech()
            .expect_change_capturing(&heard, STEP_TIMEOUT);
        if heard.contains("Rate") && heard.contains("slider") {
            return trailing_number(&heard)
                .unwrap_or_else(|| panic!("the rate slider announced no value: {heard:?}"));
        }
    }
    panic!("never reached the rate slider; last heard {heard:?}");
}

/// Tabs from the current control to the button named `button`.
fn tab_to_button(scenario: &mut Scenario, button: &str, mut heard: String) {
    for _ in 0..MAX_TABS_TO_BUTTON {
        scenario.send_keys(&["tab"]).expect("sends tab");
        heard = scenario
            .speech()
            .expect_change_capturing(&heard, STEP_TIMEOUT);
        if heard.contains(button) && heard.contains("button") {
            return;
        }
    }
    panic!("never reached the {button} button; last heard {heard:?}");
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // Enter on Cancel reverts the changed rate and closes the dialog. On
    // this slider the up arrow lowers the value (see menu_and_settings_dialog).
    let rate = open_at_rate(scenario);
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    let lowered = (rate - 1).to_string();
    scenario.speech().expect_in_order(&[&lowered], STEP_TIMEOUT);
    tab_to_button(scenario, "Cancel", lowered.clone());
    scenario
        .send_keys(&["enter"])
        .expect("sends enter on Cancel");
    scenario
        .speech()
        .wait_until_quiet(Duration::from_millis(500), STEP_TIMEOUT);
    let reopened = open_at_rate(scenario);
    assert_eq!(
        reopened, rate,
        "Enter on Cancel should have reverted the rate to {rate}"
    );

    // Enter on Apply applies the change and leaves the dialog open: the
    // next Tab moves on from Apply inside the dialog. Escape then closes it
    // without undoing what was applied.
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario.speech().expect_in_order(&[&lowered], STEP_TIMEOUT);
    tab_to_button(scenario, "Apply", lowered.clone());
    scenario
        .send_keys(&["enter"])
        .expect("sends enter on Apply");
    scenario.send_keys(&["shift+tab"]).expect("sends shift+tab");
    scenario
        .speech()
        .expect_in_order(&["Cancel", "button"], STEP_TIMEOUT);
    scenario.send_keys(&["escape"]).expect("sends escape");
    scenario
        .speech()
        .wait_until_quiet(Duration::from_millis(500), STEP_TIMEOUT);
    let applied = open_at_rate(scenario);
    assert_eq!(
        applied,
        rate - 1,
        "Enter on Apply should have kept the rate at {}",
        rate - 1
    );
    scenario.send_keys(&["escape"]).expect("sends escape");
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // The harness writes fixed settings before every launch, so the
    // applied rate does not outlive the scenario.
}
