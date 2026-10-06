//! Demonstration: the settings dialog's keys, recorded by
//! `cargo xtask demo` for `videos/demos`. The `settings_dialog_keys`
//! scenario tests the same keys, and more; this is the short version, and
//! uses its helpers.
//!
//! The walk, each step heard in full before the next:
//!
//! 1. Enter on Cancel cancels: the Speech settings open, Tab reaches the
//!    rate slider, Up Arrow changes the rate, Tab reaches Cancel, and Enter
//!    closes the dialog. Reopened, the rate slider has its old value.
//! 2. Enter on Apply applies: the rate is changed again, Tab reaches Apply,
//!    and Enter applies it, leaving the dialog open, which Shift+Tab shows
//!    by moving to Cancel. Escape closes the dialog, and reopened, the
//!    slider has the applied value.
//! 3. Control+S saves from the slider: the rate is changed once more and
//!    Control+S pressed on the slider itself. Escape closes the dialog, and
//!    reopened, the slider has the saved value.
//! 4. Control+Tab on the slider, inside the Speech page, changes category:
//!    the category list takes the focus on the next category, Theme.
//!    Escape closes the dialog.
//!
//! The harness writes its fixed settings before every launch, so the
//! changed rate does not outlive the run.

use std::io;

use super::settings_dialog_keys::{STEP_TIMEOUT, close_dialog, open_at_rate, tab_to_button};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::None)
}

/// Lowers the rate slider by one with Up Arrow, which lowers it on this
/// slider, and returns the new value as spoken.
fn lower_rate(scenario: &mut Scenario, to: i64) -> String {
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    let lowered = to.to_string();
    scenario.speech().expect_in_order(&[&lowered], STEP_TIMEOUT);
    lowered
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // Enter on Cancel.
    let rate = open_at_rate(scenario);
    let lowered = lower_rate(scenario, rate - 1);
    tab_to_button(scenario, "Cancel", lowered);
    scenario
        .send_keys(&["enter"])
        .expect("sends enter on Cancel");
    scenario
        .wait_for_window_to_close("Verbatim Settings", STEP_TIMEOUT)
        .expect("Enter on Cancel closes the settings dialog");
    let reopened = open_at_rate(scenario);
    assert_eq!(
        reopened, rate,
        "Enter on Cancel should have kept the rate at {rate}"
    );

    // Enter on Apply.
    let lowered = lower_rate(scenario, rate - 1);
    tab_to_button(scenario, "Apply", lowered);
    scenario
        .send_keys(&["enter"])
        .expect("sends enter on Apply");
    scenario.send_keys(&["shift+tab"]).expect("sends shift+tab");
    scenario
        .speech()
        .expect_in_order(&["Cancel", "button"], STEP_TIMEOUT);
    close_dialog(scenario);
    let applied = open_at_rate(scenario);
    assert_eq!(
        applied,
        rate - 1,
        "Enter on Apply should have changed the rate to {}",
        rate - 1
    );

    // Control+S on the slider.
    let _ = lower_rate(scenario, rate - 2);
    scenario
        .send_keys(&["control+s"])
        .expect("sends control+s on the slider");
    close_dialog(scenario);
    let saved = open_at_rate(scenario);
    assert_eq!(
        saved,
        rate - 2,
        "Control+S on the rate slider should have saved the rate {}",
        rate - 2
    );

    // Control+Tab on the slider.
    scenario
        .send_keys(&["control+tab"])
        .expect("sends control+tab on the slider");
    scenario
        .speech()
        .expect_in_order(&["Categories: list", "Theme"], STEP_TIMEOUT);
    close_dialog(scenario);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // The harness writes fixed settings before every launch.
}
