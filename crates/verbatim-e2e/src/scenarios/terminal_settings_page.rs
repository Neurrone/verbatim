//! The settings dialog's Terminal page (`phase6-design.md`, "M4: text,
//! editing, and terminals", Questions; `crates/verbatim-gui/src/terminal_panel.rs`):
//! a change applied there reaches Core, and the page shows a change Core
//! made itself.
//!
//! The walk: open the settings dialog and move to the Terminal category;
//! Tab to "Report new output", heard checked, as the harness's settings
//! have it; press Space and hear it not checked, then Control+S to apply,
//! and Escape, waiting for the dialog to close. Verbatim+5 then toggles
//! the setting in Core: it says "report new output on", which it says only
//! if the applied change had turned it off. That also restores the
//! setting. Reopen the Terminal page and hear "Report new output" checked
//! again, so the page opens on the value Verbatim+5 set, and close it.
//!
//! Every step waits for the speech it causes, or for the dialog to close,
//! with a deadline that only bounds failure; nothing waits a fixed time.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// What Verbatim+5 says as it turns output reporting on.
const OUTPUT_ON: &str = "report new output on";

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::None)
}

/// Opens the settings dialog on the Terminal category and Tabs to "Report
/// new output", returning its announcement.
fn open_at_report_output(scenario: &mut Scenario) -> String {
    super::open_speech_settings(scenario, STEP_TIMEOUT);
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect_in_order(&["Theme"], STEP_TIMEOUT);
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["Terminal"], STEP_TIMEOUT);
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_in_order_capturing(&["Report new output", "check box", "checked"], STEP_TIMEOUT)
}

/// Asserts that an announcement of "Report new output" says it is checked.
fn assert_checked(heard: &str) {
    assert!(
        !heard.contains("not checked"),
        "\"Report new output\" should be checked: {heard:?}"
    );
}

/// Closes the settings dialog with Escape and waits until it has gone.
fn close_dialog(scenario: &mut Scenario) {
    scenario.send_keys(&["escape"]).expect("sends escape");
    scenario
        .wait_for_window_to_close("Verbatim Settings", STEP_TIMEOUT)
        .expect("the settings dialog closes on Escape");
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // On by default, as the harness writes the settings.
    let heard = open_at_report_output(scenario);
    assert_checked(&heard);

    // Off, and applied with Control+S; Escape then closes the dialog
    // without undoing what was applied.
    scenario.send_keys(&["space"]).expect("sends space");
    scenario
        .speech()
        .expect_in_order(&["not checked"], STEP_TIMEOUT);
    scenario
        .send_keys(&["control+s"])
        .expect("sends control+s on the check box");
    close_dialog(scenario);

    // Verbatim+5 toggles Core's setting: "on" shows it was off, so the
    // applied change reached Core. It also turns the setting back on.
    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    let toggled = scenario
        .speech()
        .expect_in_order_capturing(&["report new output"], STEP_TIMEOUT);
    assert_eq!(
        toggled, OUTPUT_ON,
        "Verbatim+5 should have turned on what the Terminal page turned off"
    );

    // The page opens on the value Verbatim+5 set.
    let heard = open_at_report_output(scenario);
    assert_checked(&heard);
    close_dialog(scenario);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // The walk turns "Report new output" back on itself, and the harness
    // writes fixed settings before every launch.
}
