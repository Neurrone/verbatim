//! Caret, selection and deleting keys whose words depend on the whole
//! document, each in the Windows Forms text box (`text_box_`) and in
//! Windows 11 Notepad (`notepad_`), each its own code, as `docs/testing.md`
//! requires: Control+A, Shift with Page Down and Page Up, Shift with
//! Control+End and Control+Home, and Backspace over the first two lines,
//! on a document of two lines (`*_document_keys`), with, in Notepad, the
//! word keys at the end of the text; Page Down and Page Up on sixty lines
//! (`*_page_keys`); Control with Down and Up Arrow on three paragraphs
//! (`*_paragraph_keys`); and Control+A over more than 512 characters,
//! said as a count (`*_select_all_count`).
//!
//! Every expectation of a captured key comes from NVDA pressing the same
//! keys on the same text from a fresh document, in
//! `target/e2e-evidence/m42-text-captures-2026-10-23/` (its `readme.md`
//! lists each item; each step names its file). The keys between items read
//! what the captures show such keys reading: a caret moved off a selection
//! says its unit and then what it unselected. Differences from NVDA are
//! deliberate and declared where they occur: a key that moves nothing is
//! silent, where NVDA reads the unit at the caret (Dickson, 2026-10-08); a
//! selection's characters are counted as grapheme clusters, so a carriage
//! return and line feed count as one where NVDA counts two (Dickson,
//! 2026-10-22); and in Notepad a selection whose old range no longer holds
//! its text is said removed (Dickson, 2026-10-22). The page sizes are the
//! windows' own: the harness opens both as the captures did.
//!
//! Every key is a real key press the keyboard hook sees and passes to the
//! editor; each step waits for its speech to be heard in full before the
//! next, and there is no other wait. A key that says nothing is followed by
//! a key whose words, asserted exactly, show that nothing came between.
//! Notepad's documents are saved at the end, and by cleanup when the body
//! failed part-way.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario, WINDOW_TIMEOUT};

pub(crate) use super::no_teardown as teardown;

/// The two-line document's name.
const DOCUMENT_NAME: &str = "document-keys";

/// The sixty-line document's name.
const PAGES_NAME: &str = "page-keys";

/// The three-paragraph document's name.
const PARAGRAPHS_NAME: &str = "paragraph-keys";

/// The forty-line document's name.
const COUNT_NAME: &str = "select-all-count";

/// The first line of the two-line and forty-line documents.
const FIRST: &str = "alpha beta gamma";

/// The second line of the two-line document.
const SECOND: &str = "delta epsilon";

/// The two-line document, Doc A of the captures, which Notepad opens
/// before Verbatim starts.
pub(crate) fn document() -> Document {
    Document {
        name: DOCUMENT_NAME,
        contents: format!("{FIRST}\r\n{SECOND}\r\n"),
    }
}

/// The sixty-line document, "Line 1 of sixty" to "Line 60 of sixty".
pub(crate) fn pages_document() -> Document {
    Document {
        name: PAGES_NAME,
        contents: (1..=60)
            .map(|line| format!("Line {line} of sixty\r\n"))
            .collect::<Vec<_>>()
            .concat(),
    }
}

/// The three-paragraph document, the first two of two lines each.
pub(crate) fn paragraphs_document() -> Document {
    Document {
        name: PARAGRAPHS_NAME,
        contents: [
            "First paragraph opens here.",
            "It has a second line.",
            "",
            "Second paragraph starts.",
            "And continues.",
            "",
            "Third paragraph alone.",
        ]
        .map(|line| format!("{line}\r\n"))
        .concat(),
    }
}

/// The forty-line document: forty copies of [`FIRST`], 680 grapheme
/// clusters with its line breaks in either control, and 720 code points in
/// the text box, whose line break is two.
pub(crate) fn count_document() -> Document {
    Document {
        name: COUNT_NAME,
        contents: format!("{FIRST}\r\n").repeat(40),
    }
}

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(heard);
}

/// Presses `keys`, which says nothing; the next step's words show it.
fn press_silent(scenario: &mut Scenario, keys: &str) {
    scenario.send_keys(&[keys]).expect("sends the key");
}

/// Presses `keys`, declared a deliberate difference from NVDA, and asserts
/// exactly `heard`.
fn press_deviating(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
    scenario.speech().expect(heard);
}

/// Types `text` and asserts exactly `heard`.
fn type_text(scenario: &mut Scenario, text: &str, heard: &[&str]) {
    scenario.type_text(text).expect("types the text");
    scenario.speech().expect(heard);
}

/// Opens `document` in the Windows Forms text box.
fn open_text_box(scenario: &mut Scenario, document: &Document) -> io::Result<ScenarioState> {
    super::text_box::open(
        scenario,
        document.name,
        super::text_box::BOX_NAME,
        &document.contents,
    )?;
    Ok(ScenarioState::None)
}

/// Brings the document `name` forward in Windows 11 Notepad, which opened
/// it before Verbatim started.
fn bring_forward(scenario: &mut Scenario, name: &str) -> io::Result<ScenarioState> {
    scenario.bring_document_forward(name)?;
    Ok(ScenarioState::None)
}

/// Opens the two-line document in the Windows Forms text box.
pub(crate) fn text_box_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    open_text_box(scenario, &document())
}

/// Brings the two-line document forward in Windows 11 Notepad.
pub(crate) fn notepad_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    bring_forward(scenario, DOCUMENT_NAME)
}

/// Opens the sixty-line document in the Windows Forms text box.
pub(crate) fn text_box_pages_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    open_text_box(scenario, &pages_document())
}

/// Brings the sixty-line document forward in Windows 11 Notepad.
pub(crate) fn notepad_pages_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    bring_forward(scenario, PAGES_NAME)
}

/// Opens the three-paragraph document in the Windows Forms text box.
pub(crate) fn text_box_paragraphs_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    open_text_box(scenario, &paragraphs_document())
}

/// Brings the three-paragraph document forward in Windows 11 Notepad.
pub(crate) fn notepad_paragraphs_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    bring_forward(scenario, PARAGRAPHS_NAME)
}

/// Opens the forty-line document in the Windows Forms text box.
pub(crate) fn text_box_count_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    open_text_box(scenario, &count_document())
}

/// Brings the forty-line document forward in Windows 11 Notepad.
pub(crate) fn notepad_count_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    bring_forward(scenario, COUNT_NAME)
}

/// The two-line document in the Windows Forms text box, whose line break
/// is a carriage return and a line feed.
pub(crate) fn text_box_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(scenario, DOCUMENT_NAME, super::text_box::BOX_NAME, FIRST);

    // Control+A: the whole text selected
    // (`09e-textbox-docA-control-a-under-512-run1.txt`).
    press(
        scenario,
        "control+a",
        &["alpha beta gamma  delta epsilon   selected"],
    );
    press(
        scenario,
        "control+home",
        &[FIRST, "alpha beta gamma  delta epsilon   unselected"],
    );

    // Shift+Page Down and Shift+Page Up at the top of a text that fits in
    // the window select nothing in the text box, and say nothing
    // (`09c-textbox-docA-shift-page-down-up-run1.txt`).
    press_silent(scenario, "shift+pagedown");
    press_silent(scenario, "shift+pageup");

    // Shift+Control+End from the second line, then Shift+Control+Home,
    // which selects the first line and unselects the second, each said on
    // its own (`09d-textbox-docA-shift-control-end-home-run1.txt`).
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "shift+control+end", &["delta epsilon   selected"]);
    press(
        scenario,
        "shift+control+home",
        &["alpha beta gamma   selected", "delta epsilon   unselected"],
    );
    press(scenario, "home", &["a", "alpha beta gamma   unselected"]);

    // Both lines selected with Shift+Down Arrow from the text's start, then
    // Backspace: nothing before the selection, then the selection removed
    // (`18-textbox-docA-home-shift-down-twice-backspace-run1.txt`).
    press(
        scenario,
        "shift+downarrow",
        &["alpha beta gamma   selected"],
    );
    press(scenario, "shift+downarrow", &["delta epsilon   selected"]);
    press(scenario, "backspace", &["blank", "selection removed"]);
}

/// The two-line document in Windows 11 Notepad, read through UI
/// Automation, whose line break is a carriage return.
pub(crate) fn notepad_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, DOCUMENT_NAME, FIRST);

    // Control+A: the whole text selected
    // (`09e-notepad-docA-control-a-under-512-run1.txt`).
    press(
        scenario,
        "control+a",
        &["alpha beta gamma delta epsilon  selected"],
    );
    press(
        scenario,
        "control+home",
        &[FIRST, "alpha beta gamma delta epsilon  unselected"],
    );

    // Shift+Page Down and Shift+Page Up at the top of a text that fits in
    // the window select the whole text and unselect it
    // (`09c-notepad-docA-shift-page-down-up-run1.txt`).
    press(
        scenario,
        "shift+pagedown",
        &["alpha beta gamma delta epsilon  selected"],
    );
    press(
        scenario,
        "shift+pageup",
        &["alpha beta gamma delta epsilon  unselected"],
    );

    // Shift+Control+End from the second line, then Shift+Control+Home,
    // which selects the first line and unselects the second, each said on
    // its own (`09d-notepad-docA-shift-control-end-home-run1.txt`).
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "shift+control+end", &["delta epsilon  selected"]);
    press(
        scenario,
        "shift+control+home",
        &["alpha beta gamma  selected", "delta epsilon  unselected"],
    );
    press(scenario, "home", &["a", "alpha beta gamma  unselected"]);

    // Both lines selected with Shift+Down Arrow from the text's start, then
    // Backspace: nothing before the selection, then the selection removed,
    // where NVDA says nothing more
    // (`18-notepad-docA-home-shift-down-twice-backspace-run1.txt`).
    press(scenario, "shift+downarrow", &["alpha beta gamma  selected"]);
    press(scenario, "shift+downarrow", &["delta epsilon  selected"]);
    press_deviating(scenario, "backspace", &["blank", "selection removed"]);

    // The word keys at the end of the text, from "x" and a line break with
    // the caret after the "x" (`17-notepad-x1-end-of-text-word-keys.txt`):
    // Delete removes the line break, leaving nothing at the caret;
    // Control+Right Arrow onto the end says the last word, "y", where the
    // text box finds no word (`text_box_editing`); Control+Delete of the
    // last word says the word now before the caret, "x".
    type_text(scenario, "x", &["x"]);
    press_silent(scenario, "enter");
    press(scenario, "uparrow", &["x"]);
    press(scenario, "end", &["carriage return"]);
    press(scenario, "space", &["space"]);
    type_text(scenario, "y", &["y"]);
    press(scenario, "delete", &["blank"]);
    press(scenario, "control+leftarrow", &["y"]);
    press(scenario, "control+rightarrow", &["y"]);
    press(scenario, "control+leftarrow", &["y"]);
    press(scenario, "control+delete", &["x"]);
    scenario
        .save_document(DOCUMENT_NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}

/// The sixty lines in the Windows Forms text box, whose page is eleven
/// lines (`07-textbox-l60-page-down-up.txt`).
pub(crate) fn text_box_pages_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(
        scenario,
        PAGES_NAME,
        super::text_box::BOX_NAME,
        "Line 1 of sixty",
    );
    press(scenario, "pagedown", &["Line 12 of sixty"]);
    press(scenario, "pagedown", &["Line 23 of sixty"]);
    press(scenario, "pageup", &["Line 12 of sixty"]);
    press(scenario, "pageup", &["Line 1 of sixty"]);
}

/// The sixty lines in Windows 11 Notepad
/// (`07-notepad-l60-page-down-up.txt`).
pub(crate) fn notepad_pages_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, PAGES_NAME, "Line 1 of sixty");
    press(scenario, "pagedown", &["Line 23 of sixty"]);
    press(scenario, "pagedown", &["Line 38 of sixty"]);
    press(scenario, "pageup", &["Line 15 of sixty"]);
    press(scenario, "pageup", &["Line 1 of sixty"]);
    scenario
        .save_document(PAGES_NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}

/// The three paragraphs in the Windows Forms text box, whose Control with
/// Down or Up Arrow does not move the caret: each says nothing, where NVDA
/// reads the line at the caret, "First paragraph opens here.", every time
/// (`08-textbox-paragraphs-control-down-up.txt`). Down Arrow then reads the
/// second line, which shows the caret never moved.
pub(crate) fn text_box_paragraphs_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(
        scenario,
        PARAGRAPHS_NAME,
        super::text_box::BOX_NAME,
        "First paragraph opens here.",
    );
    for keys in [
        "control+downarrow",
        "control+downarrow",
        "control+downarrow",
        "control+uparrow",
        "control+uparrow",
        "control+uparrow",
    ] {
        press_silent(scenario, keys);
    }
    scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
    press(scenario, "downarrow", &["It has a second line."]);
}

/// The three paragraphs in Windows 11 Notepad, whose Control with Down or
/// Up Arrow moves by line (`08-notepad-paragraphs-control-down-up.txt`).
pub(crate) fn notepad_paragraphs_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, PARAGRAPHS_NAME, "First paragraph opens here.");
    press(scenario, "control+downarrow", &["It has a second line."]);
    press(scenario, "control+downarrow", &["blank"]);
    press(scenario, "control+downarrow", &["Second paragraph starts."]);
    press(scenario, "control+uparrow", &["blank"]);
    press(scenario, "control+uparrow", &["It has a second line."]);
    press(
        scenario,
        "control+uparrow",
        &["First paragraph opens here."],
    );
    scenario
        .save_document(PARAGRAPHS_NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}

/// Control+A over forty lines in the Windows Forms text box: said as a
/// count of 680 grapheme clusters, where NVDA counts the 720 code points
/// (`09f-textbox-big720-control-a-over-512-run1.txt`).
pub(crate) fn text_box_count_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(scenario, COUNT_NAME, super::text_box::BOX_NAME, FIRST);
    press_deviating(scenario, "control+a", &["680 characters selected"]);
}

/// Control+A over forty lines in Windows 11 Notepad, whose line break is
/// one character: 680 (`09f-notepad-big720-control-a-over-512-run1.txt`).
pub(crate) fn notepad_count_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, COUNT_NAME, FIRST);
    press(scenario, "control+a", &["680 characters selected"]);
    scenario
        .save_document(COUNT_NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}
