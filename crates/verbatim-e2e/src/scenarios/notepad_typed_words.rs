//! Typed character and word echo in Notepad (milestone M4 item 4): the test
//! of what the `demo_notepad_editing` demonstration shows beyond
//! `notepad_editing`. Typed punctuation and spaces are echoed by name
//! ("comma", "dot", "space"); Verbatim+3 turns typed-word echo on, saying
//! "speak typed words only in edit controls"; and then each finished word
//! is spoken after its last character's echo and before the echo of the
//! space or full stop that ends it. It holds for Windows 11 Notepad (UIA)
//! and classic Notepad's edit control alike, since the echo comes from the
//! keyboard hook's own translation of each key.
//!
//! Text is typed through the agent's `TypeText` a character at a time, as
//! a listening user types, and what each character says is heard in full
//! before the next is typed: typed while an echo plays, a character would
//! cut it off, as typing does. There is no other wait. Verbatim+3 is sent as a gesture. The document is
//! saved at the end, and by the teardown when the body failed part-way, so
//! Windows 11 Notepad never restores an edited copy of it.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, WINDOW_TIMEOUT};

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

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_document_with(NAME, DOCUMENT)?;
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_opened(scenario, NAME, "Typing at the end:");
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
