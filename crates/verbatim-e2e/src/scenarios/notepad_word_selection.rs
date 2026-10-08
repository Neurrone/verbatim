//! Moving up a line and selecting by word in Notepad (milestone M4 item
//! 3): the test of what the `demo_notepad_editing` demonstration shows
//! beyond `notepad_editing`. Up Arrow speaks the line it reaches;
//! Shift+Control+Right Arrow selects a word and then the next, each
//! spoken followed by "selected"; Shift+Control+Left Arrow takes the
//! second out again, spoken followed by "unselected"; and Shift+End
//! extends the selection to the line's end, spoken followed by
//! "selected", NVDA's word order. It holds for Windows 11 Notepad (UIA)
//! and classic Notepad's edit control alike.
//!
//! A selected word takes the space after it in, and Windows 11 Notepad's
//! Shift+End the line break, each spoken as white space before "selected";
//! classic Notepad's Shift+End stops before the line break. So each
//! selection is compared with its white space runs collapsed to one space.
//!
//! Every key is a real key press, and each step waits for its speech to be
//! heard in full before the next; there is no other wait.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// The harness document's name.
const NAME: &str = "word-selection";

/// The first line.
const FIRST: &str = "Verbatim reads this short note";

/// The second line.
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

/// Presses `keys` and waits for an utterance ending in `ending`
/// ("selected" or "unselected"), which must read `heard` once its runs of
/// white space are collapsed.
fn press_selecting(scenario: &mut Scenario, keys: &str, heard: &str, ending: &str) {
    scenario.send_keys(&[keys]).expect("sends the key");
    let spoken = scenario
        .speech()
        .expect_in_order_capturing(&[&format!(" {ending}")], STEP_TIMEOUT);
    let collapsed = spoken.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(
        collapsed,
        format!("{heard} {ending}"),
        "{keys} spoke {spoken:?}"
    );
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let _ = super::expect_notepad_text(scenario, STEP_TIMEOUT);

    // Down to the second line and back up: Up Arrow speaks the line too.
    press(scenario, "control+home", FIRST);
    press(scenario, "downarrow", SECOND);
    press(scenario, "uparrow", FIRST);

    // Two words selected, the second unselected, then the rest of the line
    // selected, from the line's start, where Up Arrow left the caret (Home
    // there would move nothing, and say nothing). Unselecting comes before
    // Shift+End because Windows 11 Notepad's Shift+End takes the line
    // break in, which Shift+Control+Left Arrow would then unselect first.
    press_selecting(scenario, "shift+control+rightarrow", "Verbatim", "selected");
    press_selecting(scenario, "shift+control+rightarrow", "reads", "selected");
    press_selecting(scenario, "shift+control+leftarrow", "reads", "unselected");
    press_selecting(scenario, "shift+end", "reads this short note", "selected");
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    if let ScenarioState::TargetPid(pid) = state {
        scenario
            .kill_target(pid)
            .expect("kills notepad through the agent");
    }
}
