//! Editing (milestone M4 items 3 and 4), as `text_box_editing` in the
//! Windows Forms text box and as `notepad_editing` in Windows 11 Notepad
//! (each its own code, as `docs/testing.md` requires): focus on the
//! text saying the caret's line rather than the whole text; the caret by
//! character, word, and line; selecting and unselecting with Shift; typing
//! with character echo; deleting with Backspace and Delete; and End and
//! Backspace naming the line break they meet, as NVDA does
//! (`docs/nvda/editable-text-and-terminals.md`, "A line break as a
//! character"): Windows 11 Notepad breaks lines with a carriage return, an
//! edit control with a carriage return and a line feed. The Notepad
//! scenario is local-only: GitHub's Windows Server runner has classic
//! Notepad.
//!
//! Every key is a real key press the keyboard hook sees and passes to the
//! editor, which moves its caret; Verbatim waits for evidence that it did
//! and reads the caret back, so each assertion is on what the caret really
//! reached. The caret starts at the top, where Control+Home moves nothing
//! and so says nothing. Notepad's document is saved at
//! the end, and by cleanup when the body failed part-way.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario, WINDOW_TIMEOUT};

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "editing";

/// The document the scenario edits.
const DOCUMENT: &str = "alpha beta gamma\r\ndelta epsilon\r\n";

/// The scenario's document, which Notepad opens before Verbatim starts.
pub(crate) fn document() -> Document {
    Document {
        name: NAME,
        contents: DOCUMENT.to_owned(),
    }
}

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(heard);
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
    super::text_box::expect_announced(
        scenario,
        NAME,
        super::text_box::BOX_NAME,
        "alpha beta gamma",
    );
    // Control+Home at the top moves nothing, so it says nothing, a
    // deliberate difference from NVDA (`docs/parity.md`, "Text, documents,
    // terminals"): Right Arrow, pressed next, is the first key heard.
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    press(scenario, "rightarrow", &["l"]);
    press(scenario, "control+rightarrow", &["beta"]);
    press(scenario, "downarrow", &["delta epsilon"]);

    // Focus leaves for Verbatim's menu and comes back: the text area says
    // the line the caret is on, the second.
    super::open_verbatim_menu(scenario);
    scenario.send_keys(&["escape"]).expect("sends escape");
    super::text_box::expect_announced(scenario, NAME, super::text_box::BOX_NAME, "delta epsilon");

    // End puts the caret on the line break, which is named. Shift+Home
    // selects back to the start of the line; Shift+Right Arrow then
    // unselects its first character, the text first, in NVDA's word order.
    // End leaves the rest of the selection: the character there, then what
    // it unselected.
    press(scenario, "end", &["carriage return"]);
    press(scenario, "shift+home", &["delta epsilon selected"]);
    press(scenario, "shift+rightarrow", &["d unselected"]);
    press(
        scenario,
        "end",
        &["carriage return", "elta epsilon unselected"],
    );

    // Typed characters are echoed; Backspace speaks what it deleted, and
    // Delete the character that took the deleted one's place.
    press(scenario, "x", &["x"]);
    press(scenario, "y", &["y"]);
    press(scenario, "backspace", &["y"]);
    press(scenario, "backspace", &["x"]);
    press(scenario, "home", &["d"]);
    press(scenario, "delete", &["e"]);

    // Backspace at the line's start deletes the line break before it.
    press(scenario, "backspace", &["line feed"]);
}

/// The scenario in Windows 11 Notepad.
pub(crate) fn notepad_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, NAME, "alpha beta gamma");
    // Control+Home at the top moves nothing, so it says nothing, a
    // deliberate difference from NVDA (`docs/parity.md`, "Text, documents,
    // terminals"): Right Arrow, pressed next, is the first key heard.
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    press(scenario, "rightarrow", &["l"]);
    press(scenario, "control+rightarrow", &["beta"]);
    press(scenario, "downarrow", &["delta epsilon"]);

    // Focus leaves for Verbatim's menu and comes back: the text area says
    // the line the caret is on, the second.
    super::open_verbatim_menu(scenario);
    scenario.send_keys(&["escape"]).expect("sends escape");
    super::expect_notepad_in_front(scenario, NAME, "delta epsilon");

    // End puts the caret on the line break, which is named. Shift+Home
    // selects back to the start of the line; Shift+Right Arrow then
    // unselects its first character, the text first, in NVDA's word order.
    // End leaves the rest of the selection: the character there, then what
    // it unselected.
    press(scenario, "end", &["carriage return"]);
    press(scenario, "shift+home", &["delta epsilon selected"]);
    press(scenario, "shift+rightarrow", &["d unselected"]);
    press(
        scenario,
        "end",
        &["carriage return", "elta epsilon unselected"],
    );

    // Typed characters are echoed; Backspace speaks what it deleted, and
    // Delete the character that took the deleted one's place.
    press(scenario, "x", &["x"]);
    press(scenario, "y", &["y"]);
    press(scenario, "backspace", &["y"]);
    press(scenario, "backspace", &["x"]);
    press(scenario, "home", &["d"]);
    press(scenario, "delete", &["e"]);

    // Backspace at the line's start deletes the line break before it.
    press(scenario, "backspace", &["carriage return"]);
    scenario
        .save_document(NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}
