//! Switching synthesizer still works (decisions D17 and D18): from eSpeak NG,
//! the default, to Windows `OneCore` voices through the Speech page's
//! Select Synthesizer dialog, and back. Each synthesizer runs in its own
//! host process, so the switch ends one host and starts another; every
//! announcement after it must still be heard in full, now from `OneCore`.
//! The settings dialog is then cancelled, so the configuration is left as
//! it was.
//!
//! `OneCore` voices are installed on Windows 11 and on GitHub's Windows
//! runners; the voice asserted is any of Microsoft's, since which one is
//! the default depends on the machine.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Generous per-step budget: starting a synthesizer host for the first time
/// on a busy machine takes a moment.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    // Nothing external: the scenario drives Verbatim's own settings dialog.
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario, STEP_TIMEOUT);

    choose_synthesizer(scenario, "downarrow", "Windows OneCore voices");
    // The rebuilt page lists OneCore's voices, and the announcement is
    // heard from OneCore.
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_in_order(&["Voice", "combo box", "Microsoft"], STEP_TIMEOUT);

    // And back to eSpeak NG.
    scenario.send_keys(&["shift+tab"]).expect("sends shift+tab");
    scenario
        .speech()
        .expect_in_order(&["Change", "button"], STEP_TIMEOUT);
    choose_synthesizer_from_change(scenario, "uparrow", "eSpeak NG");
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario.speech().expect_in_order(
        &["Voice", "combo box", "English (Great Britain)"],
        STEP_TIMEOUT,
    );
}

/// From the selected category, tabs to the Change button and chooses the
/// synthesizer `key` moves to, announced as `name`.
fn choose_synthesizer(scenario: &mut Scenario, key: &str, name: &str) {
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_in_order(&["Change", "button"], STEP_TIMEOUT);
    choose_synthesizer_from_change(scenario, key, name);
}

/// With the Change button focused, opens Select Synthesizer, moves the
/// selection with `key` to `name`, and confirms with Enter (OK is the
/// default button), returning to the rebuilt Speech page.
fn choose_synthesizer_from_change(scenario: &mut Scenario, key: &str, name: &str) {
    scenario.send_keys(&["space"]).expect("presses Change");
    scenario
        .speech()
        .expect_in_order(&["Synthesizer", "combo box"], STEP_TIMEOUT);
    scenario.send_keys(&[key]).expect("moves the selection");
    scenario.speech().expect_in_order(&[name], STEP_TIMEOUT);
    scenario.send_keys(&["enter"]).expect("confirms with OK");
    scenario
        .speech()
        .expect_in_order(&["Change", "button"], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, _state: ScenarioState) {
    // Cancel the settings dialog: nothing chosen here is saved.
    let _ = scenario.send_keys(&["escape"]);
}
