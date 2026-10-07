//! Typed character and word echo (milestone M4 item 4), as
//! `text_box_typed_words` and `notepad_typed_words` (each its own code, as `docs/testing.md` requires): the test
//! of what the `demo_notepad_editing` demonstration shows beyond
//! `notepad_editing`. Typed punctuation and spaces are echoed by name
//! ("comma", "dot", "space"); Verbatim+3 turns typed-word echo on, saying
//! "speak typed words only in edit controls"; and then each finished word
//! is spoken after its last character's echo and before the echo of the
//! space or full stop that ends it. It holds for Windows 11 Notepad (UIA)
//! and an edit control alike, since the echo comes from the keyboard hook's
//! own translation of each key.
//!
//! Text is typed through the agent's `TypeText` a character at a time, as
//! a listening user types, and what each character says is heard in full
//! before the next is typed: typed while an echo plays, a character would
//! cut it off, as typing does. There is no other wait. Verbatim+3 is sent
//! as a gesture. Notepad's document is saved at the end, so Windows 11
//! Notepad never restores an edited copy of it.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario, WINDOW_TIMEOUT};

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "typed-words";

/// The document typed into: one line, and the empty line after it.
const DOCUMENT: &str = "Typing at the end:\r\n";

/// Typed with only typed-character echo on, each character with what it
/// says: itself, punctuation and the space by name.
const CHARACTERS: [(char, &[&str]); 8] = [
    ('H', &["H"]),
    ('i', &["i"]),
    (',', &["comma"]),
    (' ', &["space"]),
    ('y', &["y"]),
    ('o', &["o"]),
    ('u', &["u"]),
    ('.', &["dot"]),
];

/// Typed with typed-word echo on too, each character with what it says:
/// itself, and the character that ends a word the finished word first.
const WORDS: [(char, &[&str]); 14] = [
    (' ', &["space"]),
    ('S', &["S"]),
    ('o', &["o"]),
    (' ', &["So", "space"]),
    ('a', &["a"]),
    ('r', &["r"]),
    ('e', &["e"]),
    (' ', &["are", "space"]),
    ('w', &["w"]),
    ('o', &["o"]),
    ('r', &["r"]),
    ('d', &["d"]),
    ('s', &["s"]),
    ('.', &["words", "dot"]),
];

/// Types each of `typed`'s characters, hearing what it says in full
/// before the next.
fn type_each(scenario: &mut Scenario, typed: &[(char, &[&str])]) {
    for (character, said) in typed {
        scenario
            .type_text(&character.to_string())
            .expect("types a character");
        scenario.speech().expect(said);
    }
}

/// The scenario's document, which Notepad opens before Verbatim starts.
pub(crate) fn document() -> Document {
    Document {
        name: NAME,
        contents: DOCUMENT.to_owned(),
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

/// The scenario in the Windows Forms text box.
pub(crate) fn text_box_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(
        scenario,
        NAME,
        super::text_box::BOX_NAME,
        "Typing at the end:",
    );
    scenario
        .send_keys(&["control+end"])
        .expect("sends control+end");
    scenario.speech().expect(&["blank"]);

    type_each(scenario, &CHARACTERS);

    scenario
        .send_gesture("kb:verbatim+3")
        .expect("sends Verbatim+3");
    scenario
        .speech()
        .expect(&["speak typed words only in edit controls"]);
    type_each(scenario, &WORDS);
}

/// The scenario in Windows 11 Notepad.
pub(crate) fn notepad_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, NAME, "Typing at the end:");
    scenario
        .send_keys(&["control+end"])
        .expect("sends control+end");
    scenario.speech().expect(&["blank"]);

    type_each(scenario, &CHARACTERS);

    scenario
        .send_gesture("kb:verbatim+3")
        .expect("sends Verbatim+3");
    scenario
        .speech()
        .expect(&["speak typed words only in edit controls"]);
    type_each(scenario, &WORDS);

    scenario
        .save_document(NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
}
