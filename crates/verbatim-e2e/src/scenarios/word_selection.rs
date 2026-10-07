//! Moving up a line and selecting by word (milestone M4 item 3), as
//! `text_box_word_selection` and `notepad_word_selection`
//! (each its own code, as `docs/testing.md` requires): the test of what the `demo_notepad_editing` demonstration shows
//! beyond `notepad_editing`. Up Arrow speaks the line it reaches;
//! Shift+Control+Right Arrow selects a word and then the next, each
//! spoken followed by "selected"; Shift+Control+Left Arrow takes the
//! second out again, spoken followed by "unselected"; and Shift+End
//! extends the selection to the line's end, spoken followed by
//! "selected", NVDA's word order.
//!
//! A selected word takes the space after it in, and Windows 11 Notepad's
//! Shift+End the line break, each spoken as white space before "selected";
//! an edit control's Shift+End stops before the line break.
//!
//! Every key is a real key press, and each step waits for its speech to be
//! heard in full before the next; there is no other wait.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario};

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "word-selection";

/// The first line.
const FIRST: &str = "Verbatim reads this short note";

/// The second line.
const SECOND: &str = "one line at a time as the caret moves.";

/// The scenario's document, which Notepad opens before Verbatim starts.
pub(crate) fn document() -> Document {
    Document {
        name: NAME,
        contents: format!("{FIRST}\r\n{SECOND}\r\n"),
    }
}

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &str) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(&[heard]);
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
    super::text_box::expect_announced(scenario, NAME, super::text_box::BOX_NAME, FIRST);

    // Down to the second line and back up: Up Arrow speaks the line too.
    press(scenario, "control+home", FIRST);
    press(scenario, "downarrow", SECOND);
    press(scenario, "uparrow", FIRST);

    // Two words selected, the second unselected, then the rest of the line
    // selected. A word selected takes the space after it in, which is
    // spoken as the space before the state. Unselecting comes before
    // Shift+End because Windows 11 Notepad's Shift+End takes the line
    // break in, which Shift+Control+Left Arrow would then unselect first.
    press(scenario, "home", "V");
    press(scenario, "shift+control+rightarrow", "Verbatim  selected");
    press(scenario, "shift+control+rightarrow", "reads  selected");
    press(scenario, "shift+control+leftarrow", "reads  unselected");
    press(scenario, "shift+end", "reads this short note selected");
}

/// The scenario in Windows 11 Notepad.
pub(crate) fn notepad_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, NAME, FIRST);

    // Down to the second line and back up: Up Arrow speaks the line too.
    press(scenario, "control+home", FIRST);
    press(scenario, "downarrow", SECOND);
    press(scenario, "uparrow", FIRST);

    // Two words selected, the second unselected, then the rest of the line
    // selected. A word selected takes the space after it in, which is
    // spoken as the space before the state. Unselecting comes before
    // Shift+End because Windows 11 Notepad's Shift+End takes the line
    // break in, which Shift+Control+Left Arrow would then unselect first.
    press(scenario, "home", "V");
    press(scenario, "shift+control+rightarrow", "Verbatim  selected");
    press(scenario, "shift+control+rightarrow", "reads  selected");
    press(scenario, "shift+control+leftarrow", "reads  unselected");
    press(scenario, "shift+end", "reads this short note  selected");
}
