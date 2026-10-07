//! The review cursor over Notepad's text (milestone M4 item 5): reading
//! the current line, word, and character without moving the caret, moving
//! by line, word, and character, the line's ends (the end being its line
//! break, named as NVDA names it) and the text's top and
//! bottom, and Verbatim+F9 and F10 copying a range, checked by pasting it.
//!
//! The document is a small text table whose second column starts at column
//! 8 on every row long enough to have one. Moving down the table keeps the
//! review cursor's column, Verbatim's one deliberate difference from NVDA
//! here (`docs/parity.md`, "Review cursor columns"): a shorter row puts it
//! on its last character, and the next longer row returns it to column 8.
//!
//! The review commands are the desktop layout's numpad keys, sent as
//! gestures; the copy is pressed as real keys, Verbatim+F10 twice in a
//! row, since a gesture sent through the control plane is always a first
//! press. Each step waits for its speech to be heard in full before the
//! next; there is no other wait.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, WINDOW_TIMEOUT};

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "review";

/// The document: a table whose quantity column starts at column 8.
const DOCUMENT: &str = "Name    Qty\r\nApple   3\r\nFig\r\nBanana  12\r\n";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_document_with(NAME, DOCUMENT)?;
    Ok(ScenarioState::None)
}

/// Sends the review gesture `gesture` and asserts exactly `heard`.
fn review(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // Notepad's text area, and the line at its caret, at the top of the
    // new document.
    super::expect_notepad_opened(scenario, NAME, "Name    Qty");
    // The caret to the top; the review cursor follows it there.
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    scenario.speech().expect(&["Name    Qty"]);

    // The current line, then the next word, the quantity column.
    review(scenario, "kb:numpad8", "Name    Qty");
    review(scenario, "kb:numpad6", "Qty");

    // Down the table: each line, then the character in the kept column.
    review(scenario, "kb:numpad9", "Apple   3");
    review(scenario, "kb:numpad2", "3");
    review(scenario, "kb:numpad9", "Fig");
    review(scenario, "kb:numpad2", "g");
    review(scenario, "kb:numpad9", "Banana  12");
    review(scenario, "kb:numpad2", "1");

    // The empty line after the text's final line break is a line of its
    // own, as in NVDA, and the bottom; back up to the last line of text.
    review(scenario, "kb:numpad9", "blank");
    review(scenario, "kb:numpad9", "Bottom blank");
    review(scenario, "kb:numpad7", "Banana  12");

    // Back up a line, by character, and to the line's ends. The end of a
    // line is its last character, its line break included, named as NVDA
    // names it (`docs/nvda/editable-text-and-terminals.md`, "A line break
    // as a character"): Windows 11 Notepad's carriage return, or the line
    // feed after classic Notepad's carriage return. Next character there
    // says the edge and the break again; previous character crosses back
    // to the line's last letter.
    review(scenario, "kb:numpad7", "Fig");
    review(scenario, "kb:numpad1", "i");
    review(scenario, "kb:shift+numpad1", "F");
    review(scenario, "kb:shift+numpad3", "carriage return");
    review(scenario, "kb:numpad3", "Right carriage return");
    review(scenario, "kb:numpad1", "g");

    // The top, and a range copied from the start marker to the review
    // cursor, the word "Name".
    review(scenario, "kb:shift+numpad7", "Name    Qty");
    review(scenario, "kb:shift+numpad1", "N");
    review(scenario, "kb:verbatim+f9", "Start marked");
    review(scenario, "kb:numpad3", "a");
    review(scenario, "kb:numpad3", "m");
    review(scenario, "kb:numpad3", "e");
    scenario
        .send_keys(&["insert+f10", "insert+f10"])
        .expect("sends Verbatim+F10 twice");
    scenario.speech().expect(&["Copied to clipboard: Name"]);

    // The first press of Verbatim+F10 selected the range in Notepad, as
    // NVDA's does, and the second copied it; moving the caret to the end
    // unselects it. Pasted there, the copy is a line of its own. A paste
    // says nothing, so the document's title marking unsaved changes is the
    // evidence it is done. Home then speaks the line's first character, and
    // the review cursor, following the caret, reads the line.
    scenario
        .send_keys(&["control+end"])
        .expect("sends control+end");
    scenario.speech().expect(&["blank", "Name unselected"]);
    scenario.send_keys(&["control+v"]).expect("sends control+v");
    scenario
        .expect_unsaved(NAME, WINDOW_TIMEOUT)
        .expect("the paste reaches the document");
    scenario.send_keys(&["home"]).expect("sends home");
    scenario.speech().expect(&["N"]);
    review(scenario, "kb:numpad8", "Name");
    scenario
        .save_document(NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}
