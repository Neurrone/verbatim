//! A File Explorer folder window: opening it, arrowing through its items,
//! opening a subfolder and going back (roadmap M3's Explorer scenario,
//! deferred until runs moved off the freshly restored VM guest).
//!
//! The folder is the scenario's own (`Scenario::open_folder`): a subfolder
//! and three files, so the list and its positions are known. Expected
//! readings were taken from NVDA with the transcript tool
//! (`docs/nvda-transcript.md`) on 2026-10-06: on opening, the window's
//! title, "Items View list", and the first item with its position ("Inner
//! not selected 1 of 4"); each arrow press the item and position ("alpha
//! dot txt 2 of 4"); Enter on the subfolder its list and first item;
//! Backspace back to the subfolder's entry ("Inner 1 of 4").

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

const STEP_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let title = scenario.open_folder(
        "folder",
        &["alpha.txt", "beta.txt", "gamma.txt", "Inner\\delta.txt"],
    )?;
    Ok(ScenarioState::Title(title))
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Title(title) = state else {
        panic!("setup records the folder window's title");
    };
    let title = title.clone();
    scenario
        .speech()
        .expect_in_order(&[&title, "Items View", "Inner", "1 of 4"], STEP_TIMEOUT);

    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["alpha", "2 of 4"], STEP_TIMEOUT);
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["beta", "3 of 4"], STEP_TIMEOUT);
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario
        .speech()
        .expect_in_order(&["alpha", "2 of 4"], STEP_TIMEOUT);
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario
        .speech()
        .expect_in_order(&["Inner", "1 of 4"], STEP_TIMEOUT);

    // Into the subfolder, whose only item is delta.txt, and back out, which
    // returns focus to the subfolder's entry.
    scenario.send_keys(&["enter"]).expect("sends enter");
    scenario
        .speech()
        .expect_in_order(&["delta", "1 of 1"], STEP_TIMEOUT);
    scenario.send_keys(&["backspace"]).expect("sends backspace");
    scenario
        .speech()
        .expect_in_order(&["Inner", "1 of 4"], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // The window is closed by its title when the scenario ends.
}
