//! Editing (milestone M4 items 3 and 4), as `text_box_editing` in the
//! Windows Forms text box and as `notepad_editing` in Windows 11 Notepad
//! (each its own code, as `docs/testing.md` requires): focus on the
//! text saying the caret's line rather than the whole text; the caret by
//! character, word, and line; selecting and unselecting with Shift; typing
//! with character echo; deleting with Backspace and Delete; and End and
//! Backspace naming the line break they meet, as NVDA does
//! (`docs/nvda/editable-text-and-terminals.md`, "A line break as a
//! character"): Windows 11 Notepad breaks lines with a carriage return, an
//! edit control with a carriage return and a line feed; in the text box,
//! no word at the end of the text, "blank"; and a selection
//! changed by typing over it or cutting it, said in general terms in both,
//! as NVDA says it in the text box, and in Notepad by Dickson's deliberate
//! difference from NVDA (`docs/parity/text.md`, "TextInfo-equivalent
//! layer"). The
//! Notepad scenario is local-only: GitHub's Windows Server runner has
//! classic Notepad.
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
    // deliberate difference from NVDA (`docs/parity/text.md`,
    // "TextInfo-equivalent layer"): Right Arrow, pressed next, is the first key heard.
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
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

    // A selection changed by a change of the text is said in general
    // terms, as NVDA's edit control says it
    // (`target/e2e-evidence/m42-desktop-2-2026-10-22/part3b-textbox-1-type-over.txt`
    // and `part3b-textbox-2-cut.txt`): "x" typed over the selected word is
    // echoed, then the selection is removed; so is the line cut with
    // Control+X, and nothing more, once Verbatim reports the key handled
    // and itself idle. The "x" typed after it changes no selection.
    press(scenario, "control+home", &["alpha beta gammaelta epsilon"]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press(scenario, "x", &["x", "selection removed"]);
    press(scenario, "control+home", &["xbeta gammaelta epsilon"]);
    press(scenario, "shift+end", &["xbeta gammaelta epsilon selected"]);
    press(scenario, "control+x", &["selection removed"]);
    scenario.expect_nothing_more();
    press(scenario, "x", &["x"]);

    // At the end of the text there is no word, as NVDA finds it
    // (`target/e2e-evidence/m42-word-at-end-2026-10-22/text-box-editing-compare.txt`,
    // steps 27 to 33): "y" typed after a space, then Delete removes the
    // line break after it, leaving the caret at the end of "x y";
    // Control+Right onto the end and Control+Delete of the last word each
    // leave nothing at the caret, "blank".
    press(scenario, "space", &["space"]);
    press(scenario, "y", &["y"]);
    press(scenario, "delete", &["blank"]);
    press(scenario, "control+leftarrow", &["y"]);
    press(scenario, "control+rightarrow", &["blank"]);
    press(scenario, "control+leftarrow", &["y"]);
    press(scenario, "control+delete", &["blank"]);
}

/// The scenario in Windows 11 Notepad.
pub(crate) fn notepad_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, NAME, "alpha beta gamma");
    // Control+Home at the top moves nothing, so it says nothing, a
    // deliberate difference from NVDA (`docs/parity/text.md`,
    // "TextInfo-equivalent layer"): Right Arrow, pressed next, is the first key heard.
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
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

    // A selection whose old range no longer holds the text it held when it
    // was reported is said in general terms, the selection removed, as in
    // the text box: "x" typed over the selected word is echoed, then the
    // selection is removed; so is the line cut with Control+X. NVDA, which
    // reads the old range as it now is, says "x unselected" and then
    // nothing (`target/e2e-evidence/m42-desktop-2-2026-10-22/part3b-notepad-1.txt`
    // and `part3b-notepad-2.txt`); the difference is Dickson's decision of
    // 2026-10-22 (`docs/parity/text.md`, "TextInfo-equivalent layer").
    //
    // Notepad's spell checker marks a misspelt word a moment after the
    // caret leaves it, raising no event, and never while the caret is in
    // it: "gammaelta", made by the Backspace above, is unmarked when the
    // Control+Home here reads it, and marked once the caret has left it;
    // "xbeta" stays unmarked while the caret follows its "x". Since Notepad
    // raises no event when it marks a word, the agent reads the marks once
    // through UI Automation, independently of Verbatim, immediately before
    // the line is read again, as the precondition's evidence, and the
    // scenario fails, naming the words read, unless "gammaelta" alone is
    // marked; whether a precondition with no event has a sanctioned form of
    // wait is Dickson's question (`docs/roadmap.md`, M4). The line is then
    // read with its error, as NVDA reads it after the same keys
    // (`target/e2e-evidence/m42-desktop-4-2026-10-22/notepad-cut-after-typing-over.txt`).
    press(scenario, "control+home", &["alpha beta gammaelta epsilon"]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    scenario.send_keys(&["x"]).expect("sends x");
    scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
    scenario.speech().expect(&["x", "selection removed"]);
    assert_eq!(
        scenario
            .misspelt_words()
            .expect("the agent reads the marked words"),
        ["gammaelta"],
        "the words Notepad's spell checker marked once the caret left them"
    );
    press(
        scenario,
        "control+home",
        &["xbeta sound: spelling-error spelling error gammaelta epsilon"],
    );
    press(
        scenario,
        "shift+end",
        &["xbeta gammaelta epsilon  selected"],
    );
    scenario.send_keys(&["control+x"]).expect("sends control+x");
    scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
    scenario.speech().expect(&["selection removed"]);
    scenario.expect_nothing_more();
    press(scenario, "x", &["x"]);
    scenario
        .save_document(NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}
