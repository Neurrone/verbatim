//! Demonstration: editing a short paragraph in Windows 11 Notepad
//! (milestone M4 items 3 and 4), recorded by `cargo xtask demo` for
//! `videos/demos`. The `notepad_editing` scenario tests the same features;
//! this one shows them at a viewer's pace.
//!
//! The walk, each step heard in full before the next key:
//!
//! 1. The caret moves to the top, which speaks the first line, then by
//!    character ("e", "r"), by word ("reads", "this"), and by line, down
//!    and back up.
//! 2. From the start of the line, Shift+Control+Right Arrow selects the
//!    first word ("Verbatim selected") and then the second ("reads
//!    selected"), Shift+Control+Left Arrow unselects the second again
//!    ("reads unselected"), and Shift+End extends the selection to the end
//!    of the line ("reads this short note selected").
//! 3. At the end of the text, a sentence is typed with typed-character
//!    echo, each character spoken as it is typed. A typo in it is fixed
//!    with Backspace, which speaks the character it deleted.
//! 4. Verbatim+3 turns on typed-word echo ("speak typed words only in edit
//!    controls"), and a second sentence is typed: each character is still
//!    spoken, and each finished word is spoken before the space or full
//!    stop that ends it.
//!
//! Every key is a real key press, except Verbatim+3, which is sent as a
//! gesture. The document is saved at the end, and by the teardown when the
//! body failed part-way, so Notepad never restores an edited copy of it.

use std::io;
use std::time::Duration;

use super::type_and_hear;
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// The harness document's name.
const NAME: &str = "demo-editing";

/// The paragraph's first line.
const FIRST: &str = "Verbatim reads this short note";

/// The paragraph's second line.
const SECOND: &str = "one line at a time as the caret moves.";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let document = format!("{FIRST}\r\n{SECOND}\r\n");
    let pid = scenario.open_document_with("notepad.exe", NAME, &document)?;
    Ok(ScenarioState::TargetPid(pid))
}

/// Presses `keys` and waits for exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &str) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect_exactly(&[heard], STEP_TIMEOUT);
}

/// Presses `keys` and waits for speech containing `heard`.
fn press_hearing(scenario: &mut Scenario, keys: &str, heard: &str) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect_in_order(&[heard], STEP_TIMEOUT);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let line = super::expect_notepad_text(scenario, STEP_TIMEOUT);
    assert!(
        [FIRST, SECOND, "blank"].contains(&line.as_str()),
        "focusing the text area spoke {line:?}, not the line at the caret"
    );

    // Moving by character, word, and line.
    press(scenario, "control+home", FIRST);
    press(scenario, "rightarrow", "e");
    press(scenario, "rightarrow", "r");
    press(scenario, "control+rightarrow", "reads");
    press(scenario, "control+rightarrow", "this");
    press(scenario, "downarrow", SECOND);
    press(scenario, "uparrow", FIRST);

    // Selecting two words, unselecting the second, then selecting the rest
    // of the line. Unselecting comes before Shift+End because Notepad's
    // Shift+End takes the line break in, which Shift+Control+Left Arrow
    // would then unselect first.
    press(scenario, "home", "V");
    press_hearing(scenario, "shift+control+rightarrow", "Verbatim selected");
    press_hearing(scenario, "shift+control+rightarrow", "reads selected");
    press_hearing(scenario, "shift+control+leftarrow", "reads unselected");
    press_hearing(scenario, "shift+end", "reads this short note selected");

    // A sentence typed with typed-character echo, with a typo fixed by
    // Backspace, which speaks the character it deleted.
    press(scenario, "control+end", "blank");
    super::type_slowly(scenario, "Typing is echoef", STEP_TIMEOUT);
    press(scenario, "backspace", "f");
    super::type_slowly(scenario, "d.", STEP_TIMEOUT);

    // Typed-word echo on: each finished word is spoken before the
    // character that ends it.
    scenario
        .send_gesture("kb:verbatim+3")
        .expect("sends Verbatim+3");
    scenario
        .speech()
        .expect_in_order(&["speak typed words only in edit controls"], STEP_TIMEOUT);
    type_and_hear(scenario, ' ', &["space"], STEP_TIMEOUT);
    for (word, ending) in [("So", ' '), ("are", ' '), ("words", '.')] {
        for character in word.chars() {
            let name = character.to_string();
            type_and_hear(scenario, character, &[&name], STEP_TIMEOUT);
        }
        let name = super::character_name(ending);
        type_and_hear(scenario, ending, &[word, &name], STEP_TIMEOUT);
    }

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
