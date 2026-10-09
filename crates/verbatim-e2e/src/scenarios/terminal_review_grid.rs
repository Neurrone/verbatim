//! The review cursor down a column of a text table in a terminal
//! (milestone M4 item 9; `phase6-design.md`, "Terminal end-to-end
//! scenarios"), as `windows_terminal_review_grid` in Windows Terminal and
//! `conhost_review_grid` in the console host. The shared setup is
//! described in the `terminal` module.
//!
//! A written script prints a table whose second column starts at column 10
//! on every row long enough to have one; two rows, "Fig" and "Kiwi", are
//! shorter than 10 characters. The review cursor in a terminal moves by
//! cell, and a cell past the end of a row's text is blank. Moving to
//! another line keeps the column, Verbatim's deliberate difference from
//! NVDA (`docs/parity.md`, "Review cursor columns").
//!
//! What it asserts, each step heard in full before the next:
//!
//! 1. The prompt line is read with numpad 8 ("ready>"), then `.\grid.ps1`
//!    runs: each row is spoken, exactly and in order, then "ready>".
//! 2. Numpad 7, pressed six times from the prompt line, reads the rows from
//!    the last up to the first, each line's text being the row (trailing
//!    whitespace aside).
//! 3. On the first row, Shift+numpad 1 says "F" (the start of the line),
//!    numpad 6 says "Count" (the next word, at column 10), and numpad 2
//!    says "C", the character at column 10.
//! 4. Down the table, numpad 9 reads each next row and numpad 2 then says
//!    exactly the character at column 10: "3", "blank" on "Fig", "1",
//!    "blank" on "Kiwi", and "7". Every step lands on column 10, through
//!    the shorter rows.
//!
//! The review commands are the desktop layout's numpad keys, sent as
//! gestures.

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// The table, each row with the character at its column 10, or "blank"
/// for a row shorter than that.
const TABLE: [(&str, &str); 6] = [
    ("Fruit    Count", "C"),
    ("Apple    3", "3"),
    ("Fig", "blank"),
    ("Banana   12", "1"),
    ("Kiwi", "blank"),
    ("Cherry   7", "7"),
];

/// The script printing [`TABLE`].
const SCRIPT: &str =
    "'Fruit    Count'\r\n'Apple    3'\r\n'Fig'\r\n'Banana   12'\r\n'Kiwi'\r\n'Cherry   7'\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "review-grid", &[("grid.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "review-grid", &[("grid.ps1", SCRIPT)])
}

/// Sends the review gesture `gesture` and asserts that exactly `heard` is
/// read.
fn review(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

/// `windows_terminal_review_grid`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), &format!("{title} terminal")],
    );
    terminal::type_with_echo(scenario, r".\grid.ps1", terminal::Echo::Shown);
    let mut printed: Vec<&str> = TABLE.iter().map(|(row, _)| *row).collect();
    printed.push(PROMPT);
    scenario.speech().expect(&printed);

    // Up from the prompt line to the first row, each row on the way.
    for (row, _) in TABLE.iter().rev() {
        review(scenario, "kb:numpad7", row);
    }

    // Onto column 10 of the first row: the start of the line, then the
    // next word, which starts the second column.
    review(scenario, "kb:shift+numpad1", "F");
    review(scenario, "kb:numpad6", "Count");
    review(scenario, "kb:numpad2", TABLE[0].1);

    // Down the table, column 10 kept on every row.
    for (row, cell) in &TABLE[1..] {
        review(scenario, "kb:numpad9", row);
        review(scenario, "kb:numpad2", cell);
    }
}

/// `conhost_review_grid`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    terminal::type_with_echo(scenario, r".\grid.ps1", terminal::Echo::Shown);
    let mut printed: Vec<&str> = TABLE.iter().map(|(row, _)| *row).collect();
    printed.push(PROMPT);
    scenario.speech().expect(&printed);

    // Up from the prompt line to the first row, each row on the way.
    for (row, _) in TABLE.iter().rev() {
        review(scenario, "kb:numpad7", row);
    }

    // Onto column 10 of the first row: the start of the line, then the
    // next word, which starts the second column.
    review(scenario, "kb:shift+numpad1", "F");
    review(scenario, "kb:numpad6", "Count");
    review(scenario, "kb:numpad2", TABLE[0].1);

    // Down the table, column 10 kept on every row.
    for (row, cell) in &TABLE[1..] {
        review(scenario, "kb:numpad9", row);
        review(scenario, "kb:numpad2", cell);
    }
}

pub(crate) use super::no_teardown as teardown;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_s_cells_are_its_rows_column_10() {
        for (row, cell) in TABLE {
            let at_10 = row.chars().nth(9).map_or("blank".to_owned(), String::from);
            assert_eq!(at_10, cell, "row {row:?}");
        }
    }

    #[test]
    fn the_script_prints_the_table() {
        let printed: Vec<String> = SCRIPT
            .lines()
            .map(|line| line.trim_matches('\'').to_owned())
            .collect();
        let rows: Vec<&str> = TABLE.iter().map(|(row, _)| *row).collect();
        assert_eq!(printed, rows);
    }
}
