//! The review cursor's current word, its column kept going up, and its
//! repeated presses, in Notepad (milestone M4 item 5): the test of what the
//! `demo_review_cursor` demonstration shows beyond `notepad_review_cursor`,
//! on the same table. Numpad 5 reads the current word; going back up the
//! table with numpad 7 keeps the column, as going down does (Verbatim's
//! deliberate difference from NVDA, `docs/parity.md`, "Review cursor
//! columns"); numpad 5 pressed twice spells the word; numpad 2 pressed
//! twice describes the character ("Alfa") and three times gives its code
//! ("65"). It holds for Windows 11 Notepad (UIA) and classic Notepad's edit
//! control alike.
//!
//! Single presses are sent as gestures. Presses that count are real key
//! presses sent together, since a gesture sent through the control plane
//! is always a first press, and a move comes between two counted presses
//! of the same key so they are never taken for one longer run. Each step
//! waits for its speech to be heard in full before the next; there is no
//! other wait.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// The harness document's name.
const NAME: &str = "review-words";

/// The table's rows, each with the character the review cursor reads in
/// the Price column, column 16, or on the last character of a shorter row.
const ROWS: [(&str, &str); 4] = [
    ("Fruit   Color   Price", "P"),
    ("Apple   red     1.20", "1"),
    ("Fig     purple", "e"),
    ("Banana  yellow  0.50", "0"),
];

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let document: String = ROWS.iter().flat_map(|(row, _)| [*row, "\r\n"]).collect();
    let pid = scenario.open_document_with("notepad.exe", NAME, &document)?;
    Ok(ScenarioState::TargetPid(pid))
}

/// Sends the review gesture `gesture` and waits for exactly `heard`.
fn review(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect_exactly(&[heard], STEP_TIMEOUT);
}

/// Sends the review gesture `gesture` and waits for `text`, white space at
/// its ends aside.
fn review_text(scenario: &mut Scenario, gesture: &str, text: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    let heard = scenario
        .speech()
        .expect_in_order_capturing(&[text], STEP_TIMEOUT);
    assert_eq!(heard.trim(), text, "{gesture} read {heard:?}, not {text:?}");
}

/// Presses the real keys `keys` together and waits for speech containing
/// `heard`.
fn press_hearing(scenario: &mut Scenario, keys: &[&str], heard: &str) {
    scenario.send_keys(keys).expect("sends the keys");
    scenario.speech().expect_in_order(&[heard], STEP_TIMEOUT);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let _ = super::expect_notepad_text(scenario, STEP_TIMEOUT);
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    scenario.speech().expect_exactly(&[ROWS[0].0], STEP_TIMEOUT);

    // The current word, then the next ones across the header row.
    review_text(scenario, "kb:numpad5", "Fruit");
    review_text(scenario, "kb:numpad6", "Color");
    review_text(scenario, "kb:numpad6", "Price");

    // Down the Price column past the shorter row, then back up past it:
    // the column is kept both ways.
    for (row, cell) in &ROWS[1..] {
        review(scenario, "kb:numpad9", row);
        review(scenario, "kb:numpad2", cell);
    }
    for (row, cell) in ROWS[1..3].iter().rev() {
        review(scenario, "kb:numpad7", row);
        review(scenario, "kb:numpad2", cell);
    }

    // On the Apple row: its first word read and spelled, its first
    // character described and given as a code.
    review(scenario, "kb:shift+numpad1", "A");
    review_text(scenario, "kb:numpad5", "Apple");
    press_hearing(scenario, &["numpad5", "numpad5"], "p p l e");
    press_hearing(scenario, &["numpad2", "numpad2"], "Alfa");
    press_hearing(scenario, &["numpad3"], "p");
    press_hearing(scenario, &["numpad1"], "A");
    press_hearing(scenario, &["numpad2", "numpad2", "numpad2"], "65");
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    if let ScenarioState::TargetPid(pid) = state {
        scenario
            .kill_target(pid)
            .expect("kills notepad through the agent");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_cell_is_the_rows_column_16_or_its_last_character() {
        for (row, cell) in ROWS {
            let characters: Vec<char> = row.chars().collect();
            let at = characters
                .get(16)
                .or_else(|| characters.last())
                .map(char::to_string);
            assert_eq!(at.as_deref(), Some(cell), "row {row:?}");
        }
    }
}
