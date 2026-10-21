//! The review cursor over an editor's text (milestone M4 item 5), as
//! `text_box_review_cursor` in the Windows Forms text box and as
//! `notepad_review_cursor` in Windows 11 Notepad (each its own code, as `docs/testing.md` requires): reading
//! the current line, word, and character without moving the caret, moving
//! by line, word, and character, the line's ends (the end being its line
//! break, named as NVDA names it) and the text's top and
//! bottom, and Verbatim+F9 and F10 copying a range, checked by pasting it:
//! a first press of F10 again before the copy asks for the second press,
//! and after the copy a further press finds the start marker cleared.
//! Last, a start marker set and the navigator moved to the parent object,
//! where F10 finds no start marker, and back at the focus, where F10
//! selects from the kept marker.
//!
//! The document is a small text table whose second column starts at column
//! 8 on every row long enough to have one. Moving down the table keeps the
//! review cursor's column, Verbatim's one deliberate difference from NVDA
//! here (`docs/parity/review.md`, "Review cursor columns"): a shorter row puts it
//! on its last character, and the next longer row returns it to column 8.
//!
//! The review commands are the desktop layout's numpad keys, sent as
//! gestures; the copy is pressed as real keys, since a gesture sent
//! through the control plane is always a first press, the second press of
//! Verbatim+F10 as soon as the first's announcement starts to play, so it
//! counts as a second press. Every other step waits for its speech to be
//! heard in full before the next; there is no other wait.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario, WINDOW_TIMEOUT};
use crate::speech::Ending;

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "review";

/// The document: a table whose quantity column starts at column 8.
const DOCUMENT: &str = "Name    Qty\r\nApple   3\r\nFig\r\nBanana  12\r\n";

/// What Verbatim+F10 says pressed once after a first press whose range is
/// not yet copied.
const PRESS_TWICE: &str = "Press twice to copy or reset the start marker";

/// The scenario's document, which Notepad opens before Verbatim starts.
pub(crate) fn document() -> Document {
    Document {
        name: NAME,
        contents: DOCUMENT.to_owned(),
    }
}

/// Sends the review gesture `gesture` and asserts exactly `heard`.
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

/// [`review`] for a step that moves the navigator to an object NVDA's
/// default simple review skips: Verbatim's object navigation walks the
/// full tree, as NVDA's does with simple review off
/// (`docs/parity/review.md`, "Navigation tree shape").
fn review_full_tree(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.deviates_from_nvda("docs/parity/review.md", "Navigation tree shape");
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
    // The text box, and the line at its caret, at the top of the
    // new document.
    super::text_box::expect_announced(scenario, NAME, super::text_box::BOX_NAME, "Name    Qty");
    // The review cursor starts at the caret, at the top: the current
    // line, then the next word, the quantity column.
    review(scenario, "kb:numpad8", "Name    Qty");
    review(scenario, "kb:numpad6", "Qty");

    // Down the table: each line, then the character in the kept column.
    review(scenario, "kb:numpad9", "Apple   3");
    review_kept_column(scenario, "kb:numpad2", "3");
    review(scenario, "kb:numpad9", "Fig");
    review_kept_column(scenario, "kb:numpad2", "g");
    review(scenario, "kb:numpad9", "Banana  12");
    review_kept_column(scenario, "kb:numpad2", "1");

    // The empty line after the text's final line break is a line of its
    // own, as in NVDA, and the bottom; back up to the last line of text.
    review(scenario, "kb:numpad9", "blank");
    review(scenario, "kb:numpad9", "Bottom blank");
    review(scenario, "kb:numpad7", "Banana  12");

    // Back up a line, by character, and to the line's ends. The end of a
    // line is its last character, its line break included, named as NVDA
    // names it (`docs/nvda/editable-text-and-terminals.md`, "A line break
    // as a character"): the line feed after the edit control's carriage
    // return. Next character there
    // says the edge and the break again; previous character crosses back
    // to the line's last letter.
    review(scenario, "kb:numpad7", "Fig");
    // The review cursor is on a different column from NVDA's here, from the kept column.
    scenario
        .send_gesture("kb:numpad1")
        .expect("sends the gesture");
    scenario.deviates_from_nvda("docs/parity/review.md", "Review cursor columns");
    scenario.speech().expect(&["i"]);
    review(scenario, "kb:shift+numpad1", "F");
    review(scenario, "kb:shift+numpad3", "line feed");
    review(scenario, "kb:numpad3", "Right line feed");
    review(scenario, "kb:numpad1", "carriage return");

    // The top, and a range copied from the start marker to the review
    // cursor, the word "Name".
    review(scenario, "kb:shift+numpad7", "Name    Qty");
    review(scenario, "kb:shift+numpad1", "N");
    review(scenario, "kb:verbatim+f9", "Start marked");
    review(scenario, "kb:numpad3", "a");
    review(scenario, "kb:numpad3", "m");
    review(scenario, "kb:numpad3", "e");
    // The first press of Verbatim+F10 selects the range and announces the
    // selection as a selecting key's is. A first press again, with that
    // range not yet copied, only asks for the second press.
    review(scenario, "kb:verbatim+f10", "Name selected");
    review(scenario, "kb:verbatim+f10", PRESS_TWICE);
    // Pressed twice, the first press asks again, and the second, pressed
    // while that plays, so within the multi-press timeout, cuts it off and
    // copies the range the first selection made.
    scenario
        .send_keys(&["insert+f10"])
        .expect("sends Verbatim+F10");
    let asked = scenario.speech().expect_started(PRESS_TWICE);
    scenario
        .send_keys(&["insert+f10"])
        .expect("sends Verbatim+F10 again");
    scenario.speech().expect_ended(&asked, Ending::Cancelled);
    scenario.speech().expect(&["Copied to clipboard: Name"]);
    // The copy cleared the start marker, so Verbatim+F10 pressed again,
    // once the copy's confirmation is heard in full, finds none.
    review(scenario, "kb:verbatim+f10", "No start marker set");

    // The range Verbatim+F10 selected in the text box is still selected;
    // moving the caret to the end
    // unselects it. Pasted there, the copy is a line of its own. A paste
    // says nothing, so the scenario waits for evidence of it. Home then
    // speaks the line's first character, and
    // the review cursor, following the caret, reads the line.
    scenario
        .send_keys(&["control+end"])
        .expect("sends control+end");
    scenario.speech().expect(&["blank", "Name unselected"]);
    super::text_box::paste(scenario);
    scenario.send_keys(&["home"]).expect("sends home");
    scenario.speech().expect(&["N"]);
    review(scenario, "kb:numpad8", "Name");

    // A start marked there, and the navigator moved to the text box's
    // parent: Verbatim+F10 there finds no start marker, which is kept for
    // the text box, and back at the focus selects from it through the
    // review cursor, both on the line's first character.
    review(scenario, "kb:verbatim+f9", "Start marked");
    // The parent is the text box's own window, which NVDA's simple review
    // skips for the form's window.
    review_full_tree(scenario, "kb:verbatim+numpad8", "Text window");
    review(scenario, "kb:verbatim+f10", "No start marker set");
    scenario
        .send_gesture("kb:verbatim+numpadminus")
        .expect("sends the gesture");
    scenario
        .speech()
        .expect(&["Move to focus Text edit multi line", "Name"]);
    review(scenario, "kb:verbatim+f10", "N selected");
}

/// The scenario in Windows 11 Notepad.
pub(crate) fn notepad_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // Notepad's text area, and the line at its caret, at the top of the
    // new document.
    super::expect_notepad_in_front(scenario, NAME, "Name    Qty");
    // The review cursor starts at the caret, at the top: the current
    // line, then the next word, the quantity column.
    review(scenario, "kb:numpad8", "Name    Qty");
    review(scenario, "kb:numpad6", "Qty");

    // Down the table: each line, then the character in the kept column.
    review(scenario, "kb:numpad9", "Apple   3");
    review_kept_column(scenario, "kb:numpad2", "3");
    review(scenario, "kb:numpad9", "Fig");
    review_kept_column(scenario, "kb:numpad2", "g");
    review(scenario, "kb:numpad9", "Banana  12");
    review_kept_column(scenario, "kb:numpad2", "1");

    // The empty line after the text's final line break is a line of its
    // own, as in NVDA, and the bottom; back up to the last line of text.
    review(scenario, "kb:numpad9", "blank");
    review(scenario, "kb:numpad9", "Bottom blank");
    review(scenario, "kb:numpad7", "Banana  12");

    // Back up a line, by character, and to the line's ends. The end of a
    // line is its last character, its line break included, named as NVDA
    // names it (`docs/nvda/editable-text-and-terminals.md`, "A line break
    // as a character"): Windows 11 Notepad's carriage return. Next character there
    // says the edge and the break again; previous character crosses back
    // to the line's last letter.
    review(scenario, "kb:numpad7", "Fig");
    // The review cursor is on a different column from NVDA's here, from the kept column.
    scenario
        .send_gesture("kb:numpad1")
        .expect("sends the gesture");
    scenario.deviates_from_nvda("docs/parity/review.md", "Review cursor columns");
    scenario.speech().expect(&["i"]);
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
    // The first press of Verbatim+F10 selects the range and announces the
    // selection as a selecting key's is. A first press again, with that
    // range not yet copied, only asks for the second press.
    review(scenario, "kb:verbatim+f10", "Name selected");
    review(scenario, "kb:verbatim+f10", PRESS_TWICE);
    // Pressed twice, the first press asks again, and the second, pressed
    // while that plays, so within the multi-press timeout, cuts it off and
    // copies the range the first selection made.
    scenario
        .send_keys(&["insert+f10"])
        .expect("sends Verbatim+F10");
    let asked = scenario.speech().expect_started(PRESS_TWICE);
    scenario
        .send_keys(&["insert+f10"])
        .expect("sends Verbatim+F10 again");
    scenario.speech().expect_ended(&asked, Ending::Cancelled);
    scenario.speech().expect(&["Copied to clipboard: Name"]);
    // The copy cleared the start marker, so Verbatim+F10 pressed again,
    // once the copy's confirmation is heard in full, finds none.
    review(scenario, "kb:verbatim+f10", "No start marker set");

    // The range Verbatim+F10 selected in Notepad is still selected; moving
    // the caret to the end
    // unselects it. Pasted there, the copy is a line of its own. A paste
    // says nothing, so the scenario waits for evidence of it. Home then
    // speaks the line's first character, and
    // the review cursor, following the caret, reads the line.
    scenario
        .send_keys(&["control+end"])
        .expect("sends control+end");
    scenario.speech().expect(&["blank", "Name unselected"]);
    super::paste_in_notepad(scenario, NAME);
    scenario.send_keys(&["home"]).expect("sends home");
    scenario.speech().expect(&["N"]);
    review(scenario, "kb:numpad8", "Name");

    // A start marked there, and the navigator moved to the text area's
    // parent: Verbatim+F10 there finds no start marker, which is kept for
    // the text area, and back at the focus selects from it through the
    // review cursor, both on the line's first character.
    review(scenario, "kb:verbatim+f9", "Start marked");
    // The parent is a pane, which NVDA's simple review skips for
    // Notepad's window.
    review_full_tree(scenario, "kb:verbatim+numpad8", "pane");
    review(scenario, "kb:verbatim+f10", "No start marker set");
    scenario
        .send_gesture("kb:verbatim+numpadminus")
        .expect("sends the gesture");
    scenario
        .speech()
        .expect(&["Move to focus Text editor document", "Name"]);
    review(scenario, "kb:verbatim+f10", "N selected");
    scenario
        .save_document(NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}
