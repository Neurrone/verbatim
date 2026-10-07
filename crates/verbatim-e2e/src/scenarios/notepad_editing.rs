//! Editing in Windows 11 Notepad (milestone M4 items 3 and 4): focus on the
//! text saying the caret's line rather than the whole text; the caret by
//! character, word, and line; selecting and unselecting with Shift; typing
//! with character echo; deleting with Backspace and Delete; and End and
//! Backspace naming the line break they meet, as NVDA does
//! (`docs/nvda/editable-text-and-terminals.md`, "A line break as a
//! character"): Windows 11 Notepad breaks lines with a carriage return.
//! Local-only: GitHub's Windows Server runner has classic Notepad.
//!
//! Every key is a real key press the keyboard hook sees and passes to
//! Notepad, which moves its caret; Verbatim waits for evidence that it did
//! and reads the caret back through UIA's text pattern, so each assertion
//! is on what Notepad's caret really reached. The document is new to
//! Notepad, so its caret starts at the top. The document is saved at the
//! end, and by cleanup when the body failed part-way.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "editing";

/// The document the scenario edits.
const DOCUMENT: &str = "alpha beta gamma\r\ndelta epsilon\r\n";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_document_with(NAME, DOCUMENT)?;
    Ok(ScenarioState::None)
}

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(heard);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_opened(scenario, NAME, "alpha beta gamma");
    press(scenario, "control+home", &["alpha beta gamma"]);
    press(scenario, "rightarrow", &["l"]);
    press(scenario, "control+rightarrow", &["beta"]);
    press(scenario, "downarrow", &["delta epsilon"]);

    // Focus leaves for Verbatim's menu and comes back: the text area says
    // the line the caret is on, the second.
    super::open_verbatim_menu(scenario);
    scenario.send_keys(&["escape"]).expect("sends escape");
    super::expect_notepad_returned(scenario, NAME, "delta epsilon");

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
        .save_document(NAME, crate::scenario::WINDOW_TIMEOUT)
        .expect("saves the document");
}
