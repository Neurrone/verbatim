//! Caret, editing and selection keys within a line or two, as
//! `text_box_caret_keys` in the Windows Forms text box and as
//! `notepad_caret_keys` in Windows 11 Notepad (each its own code, as
//! `docs/testing.md` requires): the numpad's Delete, Shift+Delete and
//! Shift+Backspace, selecting a line break, a word and lines with Shift,
//! Enter after Control+Right, Control+Backspace at a line's end, typing
//! with Tab, undo, copy, cut and paste, and Control+V and Enter over a
//! line's rest selected with Shift+End.
//!
//! Every expectation of a captured key comes from NVDA pressing the same
//! keys on the same text, "alpha beta gamma" and "delta epsilon", from a
//! fresh document, in `target/e2e-evidence/m42-text-captures-2026-10-23/`
//! (its `readme.md` lists each item; each step names its file). The
//! document is that text over and over, so each item has lines of its
//! own; the keys between items, Down Arrow and Home, read the line and the
//! character, and End over a selection reads the line break and then what
//! it unselected, as the captures of `notepad_editing`'s keys show (item
//! 24). The clipboard is filled by the scenario's own Control+C of the
//! document's "kiwi" and "mango" lines, the texts NVDA's captures pasted.
//!
//! Differences from NVDA are deliberate and declared where they occur:
//! a key outside the caret table speaks by its evidence (Control+Z says the
//! "x" it removed, and a paste that moves the caret to another line reads
//! that line, where NVDA says nothing; Dickson, 2026-10-08); and in
//! Notepad, read through UI Automation, a selection whose old range no
//! longer holds its text is said removed, where NVDA names what the range
//! now holds or says nothing (Dickson, 2026-10-22). Alt+Down Arrow, which
//! NVDA answers in the text box by moving to the next sentence itself, is
//! not here: Verbatim does not move by sentence when the application does
//! not (`docs/parity/text.md`, "TextInfo-equivalent layer").
//!
//! Every key is a real key press the keyboard hook sees and passes to the
//! editor; each step waits for its speech to be heard in full before the
//! next, and there is no other wait. A key that says nothing is followed by
//! a key whose words, asserted exactly, show that nothing came between.
//! Notepad's document is saved at the end, and by cleanup when the body
//! failed part-way.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario, WINDOW_TIMEOUT};

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "caret-keys";

/// The first line of each block of the document.
const FIRST: &str = "alpha beta gamma";

/// The second line of each block of the document.
const SECOND: &str = "delta epsilon";

/// The scenario's document: blocks of [`FIRST`] and [`SECOND`], with a
/// "kiwi" line and a "mango" line after the eighth block and after the
/// eleventh, the text the scenario copies to paste. Notepad opens it
/// before Verbatim starts.
pub(crate) fn document() -> Document {
    let block = format!("{FIRST}\r\n{SECOND}\r\n");
    let clipped = "kiwi\r\nmango\r\n";
    Document {
        name: NAME,
        contents: format!(
            "{}{clipped}{}{clipped}{}",
            block.repeat(8),
            block.repeat(3),
            block.repeat(2)
        ),
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

/// Goes down from a block's first line to the first line of the block
/// `blocks` on, reading each line.
fn down_blocks(scenario: &mut Scenario, blocks: usize) {
    for _ in 0..blocks {
        press(scenario, "downarrow", &[SECOND]);
        press(scenario, "downarrow", &[FIRST]);
    }
}

/// Copies "kiwi", selected by character from the start of its line, which
/// the caret has reached at the line's end, then goes down to the next
/// block's first line, at its start.
fn copy_kiwi(scenario: &mut Scenario) {
    press(scenario, "home", &["k"]);
    for letter in ["k", "i", "w", "i"] {
        press(
            scenario,
            "shift+rightarrow",
            &[&format!("{letter} selected")],
        );
    }
    press_silent(scenario, "control+c");
    press(scenario, "downarrow", &["mango", "kiwi unselected"]);
    press(scenario, "downarrow", &[FIRST]);
    press(scenario, "home", &["a"]);
}

/// Selects "kiwi", its line break, and "mango" from the start of the
/// "kiwi" line by line and by character: the words for the line break are
/// the text box's and Notepad's own.
fn select_kiwi_mango(scenario: &mut Scenario, kiwi_line: &str) {
    press(scenario, "shift+downarrow", &[kiwi_line]);
    for letter in ["m", "a", "n", "g", "o"] {
        press(
            scenario,
            "shift+rightarrow",
            &[&format!("{letter} selected")],
        );
    }
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
/// whose line break is a carriage return and a line feed.
pub(crate) fn text_box_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(scenario, NAME, super::text_box::BOX_NAME, FIRST);

    // The numpad's Delete: the character now at the caret
    // (`04a-textbox-docA-home-numpaddelete-run1.txt`). NVDA's Home at the
    // text's start says "a" though the caret does not move; End first
    // gives Home a move.
    press(scenario, "end", &["carriage return"]);
    press(scenario, "home", &["a"]);
    press(scenario, "numpaddelete", &["l"]);
    down_blocks(scenario, 1);

    // Shift+Delete cuts the selected word: the character now at the caret,
    // then the selection removed
    // (`04b-textbox-docA-select-word-shift-delete-run1.txt`). Shift+Backspace
    // at a line's start: the line break it deletes
    // (`04c-textbox-docA-down-shift-backspace-run1.txt`).
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press(scenario, "shift+delete", &["b", "selection removed"]);
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "shift+backspace", &["line feed"]);
    press(scenario, "downarrow", &[FIRST]);

    // The line break selected and unselected at the line's end, which
    // NVDA says as "selected" and "unselected" with no text, the text being
    // the break (`03a-textbox-docA-end-shift-right-shift-left-run1.txt`);
    // then the words before the caret selected with Shift+Control+Left
    // Arrow (`09a-textbox-docA-shift-control-left-run1.txt`).
    press(scenario, "end", &["carriage return"]);
    press(scenario, "shift+rightarrow", &["   selected"]);
    press(scenario, "shift+leftarrow", &["   unselected"]);
    press(scenario, "shift+control+leftarrow", &["gamma selected"]);
    press(scenario, "shift+control+leftarrow", &["beta  selected"]);
    press(
        scenario,
        "end",
        &["carriage return", "beta gamma unselected"],
    );
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "downarrow", &[FIRST]);
    press(scenario, "home", &["a"]);

    // Lines selected and unselected with Shift+Down and Shift+Up Arrow
    // (`09b-textbox-docA-shift-down-up-run1.txt`).
    press(
        scenario,
        "shift+downarrow",
        &["alpha beta gamma   selected"],
    );
    press(scenario, "shift+downarrow", &["delta epsilon   selected"]);
    press(scenario, "shift+uparrow", &["delta epsilon   unselected"]);
    press(
        scenario,
        "shift+uparrow",
        &["alpha beta gamma   unselected"],
    );
    down_blocks(scenario, 1);

    // Enter after Control+Right Arrow says nothing
    // (`13-textbox-docA-control-home-control-right-enter.txt`).
    press(scenario, "control+rightarrow", &["beta"]);
    press_silent(scenario, "enter");
    down_blocks(scenario, 1);

    // A typed "x" undone with Control+Z, which NVDA does not speak
    // (`15a-textbox-docA-type-x-control-z.txt`): Control+Z, outside the
    // caret table, says the "x" it removed. Then, on the next block, since
    // the edit control's one undo would take back both, a word cut and the
    // cut undone, which selects it again
    // (`19-textbox-docA-select-word-cut-undo-run1.txt`).
    type_text(scenario, "x", &["x"]);
    press_deviating(scenario, "control+z", &["x"]);
    down_blocks(scenario, 1);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press(scenario, "control+x", &["selection removed"]);
    press(scenario, "control+z", &["alpha  selected"]);
    press(scenario, "end", &["carriage return", "alpha  unselected"]);
    down_blocks(scenario, 1);
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "downarrow", &["kiwi"]);
    copy_kiwi(scenario);

    // "kiwi" pasted at a line's start says nothing
    // (`15b-textbox-docA-paste-one-line.txt`).
    press_silent(scenario, "control+v");
    down_blocks(scenario, 1);
    press(scenario, "home", &["a"]);

    // A line's rest selected with Shift+End, then Control+V and, on the
    // next line, Enter: the selection removed
    // (`25a-textbox-docA-shift-end-control-v-run1.txt`,
    // `25b-textbox-docA-shift-end-enter-run1.txt`).
    press(scenario, "shift+end", &["alpha beta gamma selected"]);
    press(scenario, "control+v", &["selection removed"]);
    down_blocks(scenario, 1);
    press(scenario, "home", &["a"]);
    press(scenario, "shift+end", &["alpha beta gamma selected"]);
    press(scenario, "enter", &["selection removed"]);
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "downarrow", &["kiwi"]);

    // "kiwi", a line break and "mango" pasted at a line's start, which
    // NVDA does not speak (`15c-textbox-docA-paste-two-lines.txt`): the
    // paste, outside the caret table, leaves the caret on another line,
    // which is read.
    select_kiwi_mango(scenario, "kiwi   selected");
    press_silent(scenario, "control+c");
    press(scenario, "downarrow", &[FIRST, "kiwi  mango unselected"]);
    press(scenario, "home", &["a"]);
    press_deviating(scenario, "control+v", &["mangoalpha beta gamma"]);
}

/// The scenario in Windows 11 Notepad, read through UI Automation, whose
/// line break is a carriage return and whose Shift+End selects the line
/// break too.
pub(crate) fn notepad_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, NAME, FIRST);

    // The numpad's Delete: the character now at the caret
    // (`04a-notepad-docA-home-numpaddelete-run1.txt`). NVDA's Home at the
    // text's start says "a" though the caret does not move; End first
    // gives Home a move.
    press(scenario, "end", &["carriage return"]);
    press(scenario, "home", &["a"]);
    press(scenario, "numpaddelete", &["l"]);
    down_blocks(scenario, 1);

    // Shift+Delete cuts the selected word: the character now at the caret,
    // then the selection removed, where NVDA says nothing more
    // (`04b-notepad-docA-select-word-shift-delete-run1.txt`).
    // Shift+Backspace at a line's start: the line break it deletes
    // (`04c-notepad-docA-down-shift-backspace-run1.txt`).
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press_deviating(scenario, "shift+delete", &["b", "selection removed"]);
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "shift+backspace", &["carriage return"]);
    press(scenario, "downarrow", &[FIRST]);

    // The words before the caret selected with Shift+Control+Left Arrow
    // (`09a-notepad-docA-shift-control-left-run1.txt`).
    press(scenario, "end", &["carriage return"]);
    press(scenario, "shift+control+leftarrow", &["gamma selected"]);
    press(scenario, "shift+control+leftarrow", &["beta  selected"]);
    press(
        scenario,
        "end",
        &["carriage return", "beta gamma unselected"],
    );
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "downarrow", &[FIRST]);
    press(scenario, "home", &["a"]);

    // Lines selected and unselected with Shift+Down and Shift+Up Arrow
    // (`09b-notepad-docA-shift-down-up-run1.txt`), then the line selected
    // from its end with Shift+Home (`10-notepad-docA-end-shift-home-run1.txt`).
    press(scenario, "shift+downarrow", &["alpha beta gamma  selected"]);
    press(scenario, "shift+downarrow", &["delta epsilon  selected"]);
    press(scenario, "shift+uparrow", &["delta epsilon  unselected"]);
    press(scenario, "shift+uparrow", &["alpha beta gamma  unselected"]);
    press(scenario, "end", &["carriage return"]);
    press(scenario, "shift+home", &["alpha beta gamma selected"]);
    press(
        scenario,
        "end",
        &["carriage return", "alpha beta gamma unselected"],
    );
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "downarrow", &[FIRST]);
    press(scenario, "home", &["a"]);

    // Enter after Control+Right Arrow says nothing
    // (`13-notepad-docA-control-home-control-right-enter.txt`).
    press(scenario, "control+rightarrow", &["beta"]);
    press_silent(scenario, "enter");
    down_blocks(scenario, 1);

    // Control+Backspace at a line's end: the word it deletes
    // (`14-notepad-docA-end-control-backspace.txt`).
    press(scenario, "end", &["carriage return"]);
    press(scenario, "control+backspace", &["gamma"]);
    down_blocks(scenario, 1);
    press(scenario, "home", &["a"]);

    // A typed "x" undone with Control+Z, which NVDA does not speak
    // (`15a-notepad-docA-type-x-control-z.txt`): Control+Z, outside the
    // caret table, says the "x" it removed. Then a word copied, which says
    // nothing (`16-notepad-docA-select-word-control-c-run1.txt`).
    type_text(scenario, "x", &["x"]);
    press_deviating(scenario, "control+z", &["x"]);
    press(scenario, "shift+control+rightarrow", &["alpha  selected"]);
    press_silent(scenario, "control+c");
    press(scenario, "end", &["carriage return", "alpha  unselected"]);
    down_blocks(scenario, 1);
    press(scenario, "home", &["a"]);

    // "a", Tab and "b" typed: Tab says nothing (`12-notepad-type-a-tab-b.txt`).
    type_text(scenario, "a", &["a"]);
    press_silent(scenario, "tab");
    type_text(scenario, "b", &["b"]);
    press(scenario, "downarrow", &[SECOND]);
    press(scenario, "downarrow", &["kiwi"]);
    copy_kiwi(scenario);

    // "kiwi" pasted at a line's start says nothing
    // (`15b-notepad-docA-paste-one-line.txt`).
    press_silent(scenario, "control+v");
    down_blocks(scenario, 1);
    press(scenario, "home", &["a"]);

    // A line's rest and its line break selected with Shift+End, then
    // Control+V, which NVDA does not speak
    // (`25a-notepad-docA-shift-end-control-v-run1.txt`), and on the next
    // line Enter, where NVDA names the line break now in the old range,
    // "carriage return unselected"
    // (`25b-notepad-docA-shift-end-enter-run1.txt`): the old range no
    // longer holds its text, so the selection is said removed. The paste
    // replaced the line break, so the next line is the next block's first.
    press(scenario, "shift+end", &["alpha beta gamma  selected"]);
    press_deviating(scenario, "control+v", &["selection removed"]);
    press(scenario, "downarrow", &[FIRST]);
    press(scenario, "home", &["a"]);
    press(scenario, "shift+end", &["alpha beta gamma  selected"]);
    press_deviating(scenario, "enter", &["selection removed"]);
    press(scenario, "downarrow", &["kiwi"]);

    // "kiwi", a line break and "mango" pasted at a line's start, which
    // NVDA does not speak (`15c-notepad-docA-paste-two-lines.txt`): the
    // paste, outside the caret table, leaves the caret on another line,
    // which is read.
    select_kiwi_mango(scenario, "kiwi  selected");
    press_silent(scenario, "control+c");
    press(scenario, "downarrow", &[FIRST, "kiwi mango unselected"]);
    press(scenario, "home", &["a"]);
    press_deviating(scenario, "control+v", &["mangoalpha beta gamma"]);
    scenario
        .save_document(NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}
