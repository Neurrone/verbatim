//! Demonstration: the review cursor over a plain-text table in Notepad
//! (milestone M4 item 5), recorded by `cargo xtask demo` for
//! `videos/demos`. The `notepad_review_cursor` scenario tests the same
//! commands; this one shows them at a viewer's pace.
//!
//! The table has three columns, the third, Price, starting at column 16 on
//! every row long enough to have one; the "Fig" row is shorter. The walk,
//! each step heard in full before the next:
//!
//! 1. The caret moves to the top, and the review cursor, following it,
//!    reads the header row (numpad 8), then across it by word: "Fruit"
//!    (numpad 5), "Color" and "Price" (numpad 6).
//! 2. Down the Price column: numpad 9 reads each next row and numpad 2 the
//!    character in the kept column, "1" on the Apple row, "e" on the
//!    shorter Fig row (its last character), and "0" and "3" on the rows
//!    after it, back in the Price column. Numpad 7 then reads back up past
//!    the Fig row to the Apple row, the column kept all the way.
//! 3. On the Apple row, Shift+numpad 1 moves to its start ("A"), numpad 5
//!    reads the word "Apple", and numpad 5 pressed twice spells it.
//!    Numpad 2 pressed twice describes the character ("Alpha"). Numpad 3
//!    and numpad 1 move to the next character ("p") and back ("A"), and
//!    numpad 2 pressed three times gives the character code, 65; the move
//!    between them keeps the third press from counting with the earlier
//!    two, which a press within half a second of them would.
//! 4. Verbatim+F9 marks the start of the row, Shift+numpad 3 moves to its
//!    end, and Verbatim+F10 pressed twice copies the row. Pasted at the end
//!    of the text, the copy is read back with numpad 8.
//!
//! Single presses are sent as gestures. Presses that count, the spelling,
//! the description, the character code, and the copy, are real key presses
//! sent together, since a gesture sent through the control plane is always
//! a first press.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// The harness document's name.
const NAME: &str = "demo-review";

/// The table's rows, each with the character the review cursor reads in
/// the Price column, column 16, or on the last character of a shorter row.
const ROWS: [(&str, &str); 5] = [
    ("Fruit   Color   Price", "P"),
    ("Apple   red     1.20", "1"),
    ("Fig     purple", "e"),
    ("Banana  yellow  0.50", "0"),
    ("Cherry  red     3.00", "3"),
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

    // Across the header row by word.
    review(scenario, "kb:numpad8", ROWS[0].0);
    review_text(scenario, "kb:numpad5", "Fruit");
    review_text(scenario, "kb:numpad6", "Color");
    review_text(scenario, "kb:numpad6", "Price");

    // Down the Price column, past the shorter row and back into the
    // column, then up again past it.
    for (row, cell) in &ROWS[1..] {
        review(scenario, "kb:numpad9", row);
        review(scenario, "kb:numpad2", cell);
    }
    for (row, cell) in ROWS[1..4].iter().rev() {
        review(scenario, "kb:numpad7", row);
        review(scenario, "kb:numpad2", cell);
    }

    // The word spelled, and the character described and given as a code.
    review(scenario, "kb:shift+numpad1", "A");
    review_text(scenario, "kb:numpad5", "Apple");
    press_hearing(scenario, &["numpad5", "numpad5"], "p p l e");
    press_hearing(scenario, &["numpad2", "numpad2"], "Alpha");
    press_hearing(scenario, &["numpad3"], "p");
    press_hearing(scenario, &["numpad1"], "A");
    press_hearing(scenario, &["numpad2", "numpad2", "numpad2"], "65");

    // The row marked, copied, pasted at the end, and read back.
    review(scenario, "kb:verbatim+f9", "Start marked");
    review(scenario, "kb:shift+numpad3", "0");
    press_hearing(
        scenario,
        &["insert+f10", "insert+f10"],
        "Copied to clipboard: Apple",
    );
    scenario
        .send_keys(&["control+end"])
        .expect("sends control+end");
    scenario.speech().expect_exactly(&["blank"], STEP_TIMEOUT);
    scenario.send_keys(&["control+v"]).expect("sends control+v");
    scenario
        .expect_unsaved(NAME, STEP_TIMEOUT)
        .expect("the paste reaches the document");
    scenario.send_keys(&["home"]).expect("sends home");
    scenario.speech().expect_exactly(&["A"], STEP_TIMEOUT);
    review(scenario, "kb:numpad8", ROWS[1].0);
    scenario
        .save_document(NAME, STEP_TIMEOUT)
        .expect("saves the document");
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    if let Err(error) = scenario.save_document(NAME, STEP_TIMEOUT) {
        println!("the edited document could not be saved: {error}");
    }
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
