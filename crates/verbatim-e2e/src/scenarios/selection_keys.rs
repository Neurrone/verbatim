//! Keys pressed over a selection (milestone M4 item 3), as
//! `text_box_selection_keys` in the Windows Forms text box and as
//! `notepad_selection_keys` in Windows 11 Notepad (each its own code, as
//! `docs/testing.md` requires): Backspace, Control+Backspace, Delete,
//! Control+Delete, Control+X, Control+V and Enter, each over a word
//! selected with Shift+Control+Right Arrow, say the key's own words and
//! then the selection's change, as NVDA says its script's words and then
//! its caret event's (`docs/parity/text.md`, "TextInfo-equivalent layer").
//!
//! Every expectation comes from a capture of NVDA pressing the same key
//! over the same selection in the same line, "alpha beta gamma", in
//! `target/e2e-evidence/m42-selection-keys-2026-10-22/`, named by the
//! control and the key (`textbox-word-backspace.txt`,
//! `notepad-second-word-backspace.txt`, and so on); each key has a line of
//! its own, so each starts from the text its capture started from. Two
//! differences are deliberate and declared where they occur: Verbatim has
//! no delayed character description, the "Bravo" NVDA says after the "b"
//! Delete reaches; and in Notepad, whose text is read through UI
//! Automation, a selection whose old range no longer holds its text is
//! said removed, where NVDA names what the range now holds or says
//! nothing (Dickson, 2026-10-22). The clipboard is filled by the scenario's
//! own Control+X, so Control+V pastes a word the scenario cut.
//!
//! Every key is a real key press the keyboard hook sees and passes to the
//! editor; each step waits for its speech to be heard in full before the
//! next, and there is no other wait. Notepad's document is saved at the
//! end, and by cleanup when the body failed part-way.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario, WINDOW_TIMEOUT};

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "selection-keys";

/// Each line of the document, one for each key.
const LINE: &str = "alpha beta gamma";

/// The scenario's document: eight copies of [`LINE`], which Notepad opens
/// before Verbatim starts.
pub(crate) fn document() -> Document {
    Document {
        name: NAME,
        contents: format!("{LINE}\r\n").repeat(8),
    }
}

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(heard);
}

/// Presses `keys`, declared a deliberate difference from NVDA, and asserts
/// exactly `heard`.
fn press_deviating(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
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

/// The scenario in the Windows Forms text box, a classic edit control,
/// whose caret is the selection's start and whose value change has a
/// selection's change said in general terms.
pub(crate) fn text_box_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(scenario, NAME, super::text_box::BOX_NAME, LINE);

    // Backspace over the first word: nothing was before the caret, then
    // the selection removed (`textbox-word-backspace.txt`).
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press(scenario, "backspace", &["blank", "selection removed"]);

    // Control+Backspace over the first word: no word before the caret
    // either (`textbox-word-control-backspace.txt`).
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press(
        scenario,
        "control+backspace",
        &["blank", "selection removed"],
    );

    // Backspace and Control+Backspace over the second word: the space and
    // the word before the caret (`textbox-second-word-backspace.txt`,
    // `textbox-second-word-control-backspace.txt`).
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "control+rightarrow", &["beta"]);
    press(scenario, "shift+control+rightarrow", &["beta  selected"]);
    press(scenario, "backspace", &["space", "selection removed"]);
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "home", &["a"]);
    press(scenario, "control+rightarrow", &["beta"]);
    press(scenario, "shift+control+rightarrow", &["beta  selected"]);
    press(
        scenario,
        "control+backspace",
        &["alpha", "selection removed"],
    );

    // Delete and Control+Delete over the first word: the character and the
    // word now at the caret (`textbox-word-delete.txt`,
    // `textbox-word-control-delete.txt`), with no delayed description.
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "home", &["a"]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press_deviating(scenario, "delete", &["b", "selection removed"]);
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press(scenario, "control+delete", &["beta", "selection removed"]);

    // Control+X and Control+V have no words of their own
    // (`m42-desktop-2-2026-10-22/part3b-textbox-2-cut.txt`,
    // `textbox-word-control-v.txt`); the word cut is the one pasted.
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press(scenario, "control+x", &["selection removed"]);
    press(scenario, "shift+control+rightarrow", &["beta  selected"]);
    press(scenario, "control+v", &["selection removed"]);

    // Enter, typed, comes last, since the line break it types splits the
    // line (`textbox-word-enter.txt`).
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "home", &["a"]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press(scenario, "enter", &["selection removed"]);
}

/// The scenario in Windows 11 Notepad, read through UI Automation, whose
/// caret is the selection's active end and whose old selection's range
/// follows the edit.
pub(crate) fn notepad_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, NAME, LINE);

    // Backspace over the first word and the second: the space before the
    // caret (`notepad-word-backspace.txt`,
    // `notepad-second-word-backspace.txt`), then the selection removed,
    // where NVDA says nothing more.
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press_deviating(scenario, "backspace", &["space", "selection removed"]);
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "control+rightarrow", &["beta"]);
    press(scenario, "shift+control+rightarrow", &["beta  selected"]);
    press_deviating(scenario, "backspace", &["space", "selection removed"]);

    // Delete and Control+Delete over the first word: the character and the
    // word now at the caret (`notepad-word-delete.txt`,
    // `notepad-word-control-delete.txt`), with no delayed description,
    // then the selection removed.
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "home", &["a"]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press_deviating(scenario, "delete", &["b", "selection removed"]);
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press_deviating(scenario, "control+delete", &["beta", "selection removed"]);

    // Control+X and Control+V: the selection removed, where NVDA says
    // nothing (`m42-desktop-4-2026-10-22/notepad-cut-after-typing-over.txt`,
    // `notepad-word-control-v.txt`); the word cut is the one pasted.
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press_deviating(scenario, "control+x", &["selection removed"]);
    press(scenario, "shift+control+rightarrow", &["beta  selected"]);
    press_deviating(scenario, "control+v", &["selection removed"]);

    // Enter, typed: the selection removed, where NVDA names the line break
    // now in the old range, "carriage return unselected"
    // (`notepad-word-enter.txt`).
    press(scenario, "downarrow", &[LINE]);
    press(scenario, "home", &["a"]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press_deviating(scenario, "enter", &["selection removed"]);
    scenario
        .save_document(NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}
