//! The review cursor in a terminal, in the Windows Terminal
//! (`windows_terminal_review_stays_on_line`) and the console host
//! (`conhost_review_stays_on_line`): with the review cursor kept off the
//! caret, a line reviewed stays on its text as new output is written below
//! it without filling the history (`docs/parity/review.md`).

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Prints two lines, waits for a key, then prints two more, so the output
/// below the reviewed line arrives while the review cursor rests on it.
const SCRIPT: &str =
    "'Alpha'\r\n'Beta'\r\n$null = [Console]::ReadKey($true)\r\n'Gamma'\r\n'Delta'\r\n";

const SCRIPTS: &[(&str, &str)] = &[("stays.ps1", SCRIPT)];

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "review-stays", SCRIPTS)
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "review-stays", SCRIPTS)
}

pub(crate) use super::no_teardown as teardown;

/// Sends `gesture` and asserts exactly `heard`.
fn review(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

fn steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\stays.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["Alpha", "Beta"]);
    review(scenario, "kb:numpad7", "Beta");
    // The review cursor is kept off the caret, so the output's moving the
    // caret leaves it on "Beta".
    review(scenario, "kb:verbatim+6", "caret moves review cursor off");
    scenario.send_keys(&["n"]).expect("presses n");
    scenario.speech().expect(&["Gamma", "Delta", PROMPT]);
    review(scenario, "kb:numpad8", "Beta");
    review(scenario, "kb:verbatim+6", "caret moves review cursor on");
}

pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    steps(scenario);
}

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
