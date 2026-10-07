//! Correcting a command, typed punctuation, and the review cursor's current
//! word down a column in a terminal (milestone M4 item 9): the test of what
//! the `demo_terminal_session` demonstration shows beyond the other
//! terminal scenarios, in Windows Terminal, or the console host where
//! Windows Terminal is not installed. The shared setup is described in the
//! `terminal` module.
//!
//! What it asserts, each step heard in full before the next:
//!
//! 1. The prompt line is read with numpad 8 ("ready>").
//! 2. `echo helo` is typed, each character echoed exactly and in order;
//!    Backspace deletes the last "o" and speaks it; "lo" is typed and
//!    echoed; Enter gives exactly "hello" and then the prompt.
//! 3. `.\moon-table.ps1` is typed, its punctuation echoed by name ("dot",
//!    "backslash", "dash"), and Enter prints a table of planets whose
//!    Moons column starts at column 9: every row is spoken, exactly and in
//!    order, then the prompt.
//! 4. Numpad 7 reads the rows from the prompt line up to the header row;
//!    Shift+numpad 1 says "P"; numpad 6 says "Moons", the next word; and
//!    down the table, numpad 9 reads each row and numpad 5 the word in the
//!    kept column, the number of moons, "0" to "146".
//!
//! Commands are typed with the agent's `TypeText` all at once, and Enter
//! pressed with `SendKeys`; the review commands are the desktop layout's
//! numpad keys, sent as gestures.

use std::io;

use super::terminal::{self, PROMPT, STEP_TIMEOUT, Terminal};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// The table's rows, each with its word in the Moons column, column 9.
const TABLE: [(&str, &str); 6] = [
    ("Planet   Moons  Rings", "Moons"),
    ("Mercury  0      no", "0"),
    ("Earth    1      no", "1"),
    ("Mars     2      no", "2"),
    ("Jupiter  95     yes", "95"),
    ("Saturn   146    yes", "146"),
];

/// The command printing [`TABLE`].
const TABLE_COMMAND: &str = r".\moon-table.ps1";

/// The script printing [`TABLE`].
fn table_script() -> String {
    TABLE
        .iter()
        .flat_map(|(row, _)| ["'", *row, "'\r\n"])
        .collect()
}

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let script = table_script();
    terminal::open(
        scenario,
        "terminal-editing",
        Terminal::WindowsTerminal,
        &[("moon-table.ps1", script.as_str())],
    )
}

/// What Verbatim echoes for each character of `text` typed: the
/// character's name as it is spoken on its own.
fn echo_of(text: &str) -> Vec<String> {
    text.chars().map(super::character_name).collect()
}

/// Types `text` and waits until every character's echo has been queued,
/// exactly and in order, and the last heard in full.
fn type_hearing(scenario: &mut Scenario, text: &str) {
    scenario.type_text(text).expect("types the text");
    let echo = echo_of(text);
    let echo: Vec<&str> = echo.iter().map(String::as_str).collect();
    scenario.speech().expect_exactly(&echo, STEP_TIMEOUT);
}

/// Sends the review gesture `gesture` and waits for `text`, trailing white
/// space aside.
fn review_text(scenario: &mut Scenario, gesture: &str, text: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    let heard = scenario
        .speech()
        .expect_in_order_capturing(&[text], STEP_TIMEOUT);
    assert_eq!(
        heard.trim_end(),
        text,
        "{gesture} read {heard:?}, not {text:?}"
    );
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_prompt_read(scenario, state);

    // A typo corrected with Backspace, which speaks what it deleted.
    type_hearing(scenario, "echo helo");
    scenario.send_keys(&["backspace"]).expect("sends backspace");
    scenario.speech().expect_exactly(&["o"], STEP_TIMEOUT);
    type_hearing(scenario, "lo");
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario
        .speech()
        .expect_exactly(&["hello", PROMPT], STEP_TIMEOUT);

    // Punctuation echoed by name, then the table printed.
    type_hearing(scenario, TABLE_COMMAND);
    scenario.send_keys(&["enter"]).expect("presses enter");
    let mut printed: Vec<&str> = TABLE.iter().map(|(row, _)| *row).collect();
    printed.push(PROMPT);
    scenario.speech().expect_exactly(&printed, STEP_TIMEOUT);

    // Up to the header row, onto the Moons column, and down it by word.
    for (row, _) in TABLE.iter().rev() {
        review_text(scenario, "kb:numpad7", row);
    }
    scenario
        .send_gesture("kb:shift+numpad1")
        .expect("sends the gesture");
    scenario.speech().expect_exactly(&["P"], STEP_TIMEOUT);
    review_text(scenario, "kb:numpad6", TABLE[0].1);
    for (row, moons) in &TABLE[1..] {
        review_text(scenario, "kb:numpad9", row);
        review_text(scenario, "kb:numpad5", moons);
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    terminal::close(scenario, &state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_rows_moons_are_the_word_at_its_column_9() {
        for (row, moons) in TABLE {
            let word = row[9..].split_whitespace().next();
            assert_eq!(word, Some(moons), "row {row:?}");
        }
    }

    #[test]
    fn punctuation_is_echoed_by_name() {
        assert_eq!(
            echo_of(TABLE_COMMAND),
            [
                "dot",
                "backslash",
                "m",
                "o",
                "o",
                "n",
                "dash",
                "t",
                "a",
                "b",
                "l",
                "e",
                "dot",
                "p",
                "s",
                "1"
            ]
        );
    }
}
