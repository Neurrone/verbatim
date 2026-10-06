//! Editing in Notepad (milestone M4 items 3 and 4): focus on the text
//! saying the caret's line rather than the whole text; the caret by
//! character, word, and line; selecting and unselecting with Shift; typing
//! with character echo; and deleting with Backspace and Delete. It holds for
//! Windows 11 Notepad (UIA) and classic Notepad's edit control alike.
//!
//! Every key is a real key press the keyboard hook sees and passes to
//! Notepad, which moves its caret; Verbatim waits for evidence that it did
//! and reads the caret back through UIA's text pattern, so each assertion
//! is on what Notepad's caret really reached. Typed characters are echoed
//! from the hook's translation of each key. Each step waits for its speech
//! to be heard in full before the next key, as a listening user would;
//! there is no other wait.
//!
//! The document is saved at the end, and by the teardown when the body
//! failed part-way, so Windows 11 Notepad never restores an edited copy of
//! it into the next run.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// The harness document's name.
const NAME: &str = "editing";

/// The document the scenario edits.
const DOCUMENT: &str = "alpha beta gamma\r\ndelta epsilon\r\n";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let pid = scenario.open_document_with("notepad.exe", NAME, DOCUMENT)?;
    Ok(ScenarioState::TargetPid(pid))
}

/// Presses `keys` and waits for exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &str) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect_exactly(&[heard], STEP_TIMEOUT);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // Focus on the text area says its name and role, then the line at the
    // caret, never the whole text. Notepad may have put the caret on any
    // line, so any of them will do here.
    let line = super::expect_notepad_text(scenario, STEP_TIMEOUT);
    assert!(
        ["alpha beta gamma", "delta epsilon", "blank"].contains(&line.as_str()),
        "focusing the text area spoke {line:?}, not the line at the caret"
    );

    // Notepad may restore the caret where an earlier session left it, so
    // Control+Home first takes it to the top, speaking the line there.
    // Right Arrow then speaks the character it reached, Control+Right Arrow
    // the word, Down Arrow the line, and End the end of the line, which is
    // blank.
    press(scenario, "control+home", "alpha beta gamma");
    press(scenario, "rightarrow", "l");
    press(scenario, "control+rightarrow", "beta");
    press(scenario, "downarrow", "delta epsilon");

    // Focus leaves for Verbatim's menu and comes back: the text area says
    // the line the caret is on, the second.
    super::open_verbatim_menu(scenario, STEP_TIMEOUT);
    scenario.send_keys(&["escape"]).expect("sends escape");
    let line = super::expect_notepad_text(scenario, STEP_TIMEOUT);
    assert_eq!(
        line, "delta epsilon",
        "focus returning spoke the wrong text"
    );

    press(scenario, "end", "blank");

    // Shift+Home selects back to the start of the line; Shift+Right Arrow
    // then unselects its first character, the text first, in NVDA's word
    // order. End leaves the rest of the selection: the character there,
    // then what it unselected, as NVDA reports it.
    press(scenario, "shift+home", "delta epsilon selected");
    press(scenario, "shift+rightarrow", "d unselected");
    scenario.send_keys(&["end"]).expect("sends end");
    scenario
        .speech()
        .expect_exactly(&["blank", "elta epsilon unselected"], STEP_TIMEOUT);

    // Typed characters are echoed; Backspace speaks what it deleted.
    press(scenario, "x", "x");
    press(scenario, "y", "y");
    press(scenario, "backspace", "y");
    press(scenario, "backspace", "x");

    // Delete speaks the character that took the deleted one's place.
    press(scenario, "home", "d");
    press(scenario, "delete", "e");
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
