//! The review cursor down a column of a text table in a terminal
//! (milestone M4; `docs/parity/review.md`, "Review cursor columns"), while
//! new output is written (`docs/parity/terminals.md`, "The review cursor
//! while output is written"), and along a short row, as
//! `windows_terminal_review_grid` in Windows Terminal and
//! `conhost_review_grid` in the console host. The shared setup is
//! described in the `terminal` module.
//!
//! A written script prints a table whose second column starts at column 10
//! on every row long enough to have one; two rows, "Fig" and "Kiwi", are
//! shorter than 10 characters. The review cursor in a terminal moves by
//! cell, and a cell past the end of a row's text is blank. Moving to
//! another line keeps the column, Verbatim's deliberate difference from
//! NVDA (`docs/parity/review.md`, "Review cursor columns").
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
//! 5. A second script, `review.ps1`, prints the table again, then reads a
//!    key without showing it and prints two more lines, "new one" and "new
//!    two". While it waits, the review cursor goes up two lines from the
//!    caret's, to "Kiwi", and numpad 8 reads it there. The key lets the
//!    output through, which is spoken, and moves the caret to the prompt
//!    after it; the review cursor follows the caret, as NVDA's does by
//!    default (captured live on 2026-10-09 in both terminals, where NVDA
//!    said the same lines for the same keys), so numpad 8 reads the
//!    prompt, numpad 7 the last line of the new output, and numpad 9 the
//!    prompt again. The key is not echoed, the script not showing it; NVDA
//!    echoes it ("n"), the recorded difference for typing a terminal does
//!    not show.
//! 6. A third script, `short.ps1`, prints one short line, "ab", far
//!    shorter than the terminal's width. Both terminals pad every row to
//!    the width, and padding is never reviewed: a row's line ends at its
//!    last character (`docs/design/terminal-model.md`, "Features and
//!    settings"). Numpad 7 from the prompt's line reads "ab", the start of
//!    the line says "a", numpad 3 says "b", and numpad 3 again is at the
//!    line's edge, "Right b", where the review cursor once moved on into
//!    the padding, saying "blank" for each of its cells to the row's
//!    width. These expectations are the design's rule, not an NVDA
//!    capture: none has been made of NVDA's review cursor at the end of a
//!    terminal row.
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

/// The script printing [`TABLE`], then, after a key, two more lines.
const OUTPUT_SCRIPT: &str = "'Fruit    Count'\r\n'Apple    3'\r\n'Fig'\r\n'Banana   12'\r\n'Kiwi'\r\n\
'Cherry   7'\r\n$null = [Console]::ReadKey($true)\r\n'new one'\r\n'new two'\r\n";

/// The script printing the short line.
const SHORT_SCRIPT: &str = "'ab'\r\n";

/// The scripts the scenario runs.
const SCRIPTS: &[(&str, &str)] = &[
    ("grid.ps1", SCRIPT),
    ("review.ps1", OUTPUT_SCRIPT),
    ("short.ps1", SHORT_SCRIPT),
];

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "review-grid", SCRIPTS)
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "review-grid", SCRIPTS)
}

/// Sends the review gesture `gesture` and asserts that exactly `heard` is
/// read.
fn review(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

/// [`review`] for a step that reads the column the review cursor kept from
/// the line before, Verbatim's deliberate difference from NVDA, whose
/// review cursor moves to the line's start (`docs/parity/review.md`,
/// "Review cursor columns").
fn review_kept_column(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.deviates_from_nvda("docs/parity/review.md", "Review cursor columns");
    scenario.speech().expect(&[heard]);
}

/// Step 5: the review cursor while new output is written, and following
/// the caret after it.
fn output_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\review.ps1", terminal::Echo::Shown);
    let rows: Vec<&str> = TABLE.iter().map(|(row, _)| *row).collect();
    scenario.speech().expect(&rows);
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

/// Step 6: the review cursor along a short row, stopping at its last
/// character.
fn padding_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\short.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["ab", PROMPT]);
    review(scenario, "kb:numpad7", "ab");
    review(scenario, "kb:shift+numpad1", "a");
    review(scenario, "kb:numpad3", "b");
    // The line ends at its last character: no padding after it.
    review(scenario, "kb:numpad3", "Right b");
    review(scenario, "kb:numpad3", "Right b");
}

/// `windows_terminal_review_grid`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
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
        review_kept_column(scenario, "kb:numpad2", cell);
    }
    output_steps(scenario);
    padding_steps(scenario);
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
        review_kept_column(scenario, "kb:numpad2", cell);
    }
    output_steps(scenario);
    padding_steps(scenario);
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
