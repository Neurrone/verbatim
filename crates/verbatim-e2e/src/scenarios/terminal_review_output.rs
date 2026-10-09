//! The review cursor while new output is written in a terminal
//! (`phase6-design.md`, "The review cursor in a terminal"), as
//! `windows_terminal_review_output` in Windows Terminal and
//! `conhost_review_output` in the console host. The shared setup is
//! described in the `terminal` module.
//!
//! A written script, `review.ps1`, prints a table of six rows, then reads
//! a key without showing it and prints two more lines, "new one" and "new
//! two". While it waits, the review cursor goes up two lines from the
//! caret's, to "Kiwi", and numpad 8 reads it there. The key lets the
//! output through, which is spoken, and moves the caret to the prompt
//! after it; the review cursor follows the caret, as NVDA's does by
//! default (captured live on 2026-10-09 in both terminals, where NVDA
//! said the same lines for the same keys), so numpad 8 reads the prompt,
//! numpad 7 the last line of the new output, and numpad 9 the prompt
//! again. The key is not echoed, the script not showing it; NVDA echoes
//! it ("n"), the recorded difference for typing a terminal does not show.

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The table's rows.
const TABLE: [&str; 6] = [
    "Fruit    Count",
    "Apple    3",
    "Fig",
    "Banana   12",
    "Kiwi",
    "Cherry   7",
];

/// The script printing [`TABLE`], then, after a key, two more lines.
const SCRIPT: &str = "'Fruit    Count'\r\n'Apple    3'\r\n'Fig'\r\n'Banana   12'\r\n'Kiwi'\r\n\
'Cherry   7'\r\n$null = [Console]::ReadKey($true)\r\n'new one'\r\n'new two'\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "review-output", &[("review.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "review-output", &[("review.ps1", SCRIPT)])
}

/// Sends the review gesture `gesture` and asserts that exactly `heard` is
/// read.
fn review(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

/// The steps once the prompt has been read.
fn steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\review.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&TABLE);
    // Up from the caret's line, below the table, to "Kiwi".
    review(scenario, "kb:numpad7", "Cherry   7");
    review(scenario, "kb:numpad7", "Kiwi");
    review(scenario, "kb:numpad8", "Kiwi");
    // The new output, and the review cursor with the caret after it.
    scenario.send_keys(&["n"]).expect("presses n");
    scenario.speech().expect(&["new one", "new two", PROMPT]);
    review(scenario, "kb:numpad8", PROMPT);
    review(scenario, "kb:numpad7", "new two");
    review(scenario, "kb:numpad9", PROMPT);
}

/// `windows_terminal_review_output`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), &format!("{title} terminal")],
    );
    steps(scenario);
}

/// `conhost_review_output`.
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
