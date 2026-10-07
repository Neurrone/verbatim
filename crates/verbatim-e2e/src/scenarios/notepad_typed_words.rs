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
//! Text is typed through the agent's `TypeText` all at once, and every
//! echo is waited for, exactly and in order, the last heard in full;
//! there is no other wait. Verbatim+3 is sent as a gesture. The document is
//! saved at the end, and by the teardown when the body failed part-way, so
//! Windows 11 Notepad never restores an edited copy of it.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// The harness document's name.
const NAME: &str = "typed-words";

/// The document typed into: one line, and the empty line after it.
const DOCUMENT: &str = "Typing at the end:\r\n";

/// Typed with only typed-character echo on.
const CHARACTERS: &str = "Hi, you.";

/// Typed with typed-word echo on too.
const WORDS: &str = " So are words.";

/// The echo of each character of `text` typed, in order, and with
/// `words`, each finished word too, after its last character and before
/// the character that ends it.
fn echo(text: &str, words: bool) -> Vec<String> {
    let mut heard = Vec::new();
    let mut word = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            word.push(character);
        } else if words && !word.is_empty() {
            heard.push(std::mem::take(&mut word));
        } else {
            word.clear();
        }
        heard.push(super::character_name(character));
    }
    heard
}

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let pid = scenario.open_document_with("notepad.exe", NAME, DOCUMENT)?;
    Ok(ScenarioState::TargetPid(pid))
}

/// Types `text` and waits for exactly `heard`, in order.
fn type_hearing(scenario: &mut Scenario, text: &str, heard: &[String]) {
    scenario.type_text(text).expect("types the text");
    let heard: Vec<&str> = heard.iter().map(String::as_str).collect();
    scenario.speech().expect_exactly(&heard, STEP_TIMEOUT);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let _ = super::expect_notepad_text(scenario, STEP_TIMEOUT);
    scenario
        .send_keys(&["control+end"])
        .expect("sends control+end");
    scenario.speech().expect_exactly(&["blank"], STEP_TIMEOUT);

    // Characters alone: punctuation and the space by name.
    type_hearing(scenario, CHARACTERS, &echo(CHARACTERS, false));

    // Words too, once Verbatim+3 has turned them on.
    scenario
        .send_gesture("kb:verbatim+3")
        .expect("sends Verbatim+3");
    scenario
        .speech()
        .expect_exactly(&["speak typed words only in edit controls"], STEP_TIMEOUT);
    type_hearing(scenario, WORDS, &echo(WORDS, true));

    scenario
        .save_document(NAME, STEP_TIMEOUT)
        .expect("saves the document");
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    if let Err(error) = scenario.save_document(NAME, STEP_TIMEOUT) {
        println!("the edited document could not be saved: {error}");
    }
    if let ScenarioState::TargetPid(pid) = state {
        scenario
            .kill_target(pid)
            .expect("kills notepad through the agent");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_alone_are_echoed_by_name() {
        assert_eq!(
            echo(CHARACTERS, false),
            ["H", "i", "comma", "space", "y", "o", "u", "dot"]
        );
    }

    #[test]
    fn a_finished_word_comes_before_the_character_ending_it() {
        assert_eq!(
            echo(WORDS, true),
            [
                "space", "S", "o", "So", "space", "a", "r", "e", "are", "space", "w", "o", "r",
                "d", "s", "words", "dot"
            ]
        );
    }
}
