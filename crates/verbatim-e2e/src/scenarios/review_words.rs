//! The review cursor's current word, its column kept going up, and its
//! repeated presses (milestone M4 item 5), as `text_box_review_words` and
//! `notepad_review_words` (each its own code, as `docs/testing.md` requires): the test of what the
//! `demo_review_cursor` demonstration shows beyond `notepad_review_cursor`,
//! on the same table. Numpad 5 reads the current word; going back up the
//! table with numpad 7 keeps the column, as going down does (Verbatim's
//! deliberate difference from NVDA, `docs/parity.md`, "Review cursor
//! columns"); numpad 5 pressed twice spells the word; numpad 2 pressed
//! twice describes the character ("Alfa") and three times gives its code
//! ("65"). It holds for Windows 11 Notepad (UIA) and an edit control
//! alike.
//!
//! Single presses are sent as gestures. Presses that count are real key
//! presses sent together, since a gesture sent through the control plane
//! is always a first press, and a move comes between two counted presses
//! of the same key so they are never taken for one longer run: each press
//! speaks, the first what one press says, the next what the repeat adds,
//! and each press cuts off what the press before it said, as any key press
//! cuts speech off; only the last press's speech is heard in full.
//! Every step asserts exactly what it says before the next.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario};
use crate::speech::{Expected, cut_off, heard};

pub(crate) use super::no_teardown as teardown;

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

/// The scenario's document, which Notepad opens before Verbatim starts.
pub(crate) fn document() -> Document {
    Document {
        name: NAME,
        contents: ROWS.iter().flat_map(|(row, _)| [*row, "\r\n"]).collect(),
    }
}

/// Sends the review gesture `gesture` and asserts exactly `heard`.
fn review(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

/// Presses the real keys `keys` in one burst and asserts exactly `said`,
/// one utterance a press: each cut off by the press after it, and the last
/// heard in full.
fn press_hearing(scenario: &mut Scenario, keys: &[&str], said: &[&str]) {
    scenario.send_keys(keys).expect("sends the keys");
    let expected: Vec<Expected> = said
        .iter()
        .enumerate()
        .map(|(index, text)| {
            if index + 1 == said.len() {
                heard(text)
            } else {
                cut_off(text)
            }
        })
        .collect();
    scenario.speech().expect_sequence(&expected);
}

/// Opens the scenario's document in the Windows Forms text box.
pub(crate) fn text_box_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    super::text_box::open(
        scenario,
        NAME,
        super::text_box::BOX_NAME,
        &document().contents,
    )?;
    Ok(ScenarioState::None)
}

/// Brings the scenario's document forward in Windows 11 Notepad, which
/// opened it before Verbatim started.
pub(crate) fn notepad_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.bring_document_forward(NAME)?;
    Ok(ScenarioState::None)
}

/// The scenario in the Windows Forms text box.
pub(crate) fn text_box_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(scenario, NAME, super::text_box::BOX_NAME, ROWS[0].0);
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    scenario.speech().expect(&[ROWS[0].0]);

    // The current word, then the next ones across the header row.
    review(scenario, "kb:numpad5", "Fruit");
    review(scenario, "kb:numpad6", "Color");
    review(scenario, "kb:numpad6", "Price");

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
    review(scenario, "kb:numpad5", "Apple");
    press_hearing(scenario, &["numpad5", "numpad5"], &["Apple", "A p p l e"]);
    press_hearing(scenario, &["numpad2", "numpad2"], &["A", "Alfa"]);
    press_hearing(scenario, &["numpad3"], &["p"]);
    press_hearing(scenario, &["numpad1"], &["A"]);
    press_hearing(
        scenario,
        &["numpad2", "numpad2", "numpad2"],
        &["A", "Alfa", "65, 0 x 4 1"],
    );
}

/// The scenario in Windows 11 Notepad.
pub(crate) fn notepad_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, NAME, ROWS[0].0);
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    scenario.speech().expect(&[ROWS[0].0]);

    // The current word, then the next ones across the header row.
    review(scenario, "kb:numpad5", "Fruit");
    review(scenario, "kb:numpad6", "Color");
    review(scenario, "kb:numpad6", "Price");

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
    review(scenario, "kb:numpad5", "Apple");
    press_hearing(scenario, &["numpad5", "numpad5"], &["Apple", "A p p l e"]);
    press_hearing(scenario, &["numpad2", "numpad2"], &["A", "Alfa"]);
    press_hearing(scenario, &["numpad3"], &["p"]);
    press_hearing(scenario, &["numpad1"], &["A"]);
    press_hearing(
        scenario,
        &["numpad2", "numpad2", "numpad2"],
        &["A", "Alfa", "65, 0 x 4 1"],
    );
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
