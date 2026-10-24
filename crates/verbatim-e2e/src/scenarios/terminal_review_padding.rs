//! The review cursor along a short row of a terminal, as
//! `windows_terminal_review_padding` in Windows Terminal and
//! `conhost_review_padding` in the console host. The shared setup is
//! described in the `terminal` module.
//!
//! A written script prints one short line, "ab", far shorter than the
//! terminal's width. Both terminals pad every row to the width, and
//! padding is never reviewed: a row's line ends at its last character
//! (`docs/design/terminal-model.md`, section 5.12). Numpad 7 from the
//! prompt's line reads "ab", the start of the line says "a", numpad 3 says
//! "b", and numpad 3 again is at the line's edge, "Right b", where the
//! review cursor once moved on into the padding, saying "blank" for each
//! of its cells to the row's width.
//!
//! The expectations are the design's rule, not an NVDA capture: none has
//! been made of NVDA's review cursor at the end of a terminal row.

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The script printing the short line.
const SCRIPT: &str = "'ab'\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "review-padding", &[("short.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "review-padding", &[("short.ps1", SCRIPT)])
}

/// Sends the review gesture `gesture` and asserts that exactly `heard` is
/// read.
fn review(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

/// The steps once the prompt has been read.
fn steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\short.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["ab", PROMPT]);
    review(scenario, "kb:numpad7", "ab");
    review(scenario, "kb:shift+numpad1", "a");
    review(scenario, "kb:numpad3", "b");
    // The line ends at its last character: no padding after it.
    review(scenario, "kb:numpad3", "Right b");
    review(scenario, "kb:numpad3", "Right b");
}

/// `windows_terminal_review_padding`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    steps(scenario);
}

/// `conhost_review_padding`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    steps(scenario);
}
