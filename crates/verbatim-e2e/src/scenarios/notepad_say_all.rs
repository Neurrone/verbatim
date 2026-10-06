//! Say-all in Notepad (milestone M4 item 6): Verbatim+Down Arrow, the
//! desktop layout's say all from the caret (NVDA+A on laptops), reads the
//! text piece by piece, moving the caret as each piece starts to play; a
//! key interrupts it and leaves the caret where speech stopped.
//!
//! Notepad's text is UIA, which has no sentence unit, so say-all reads by
//! line, one line per piece. The second line is long, so it is still
//! playing when the scenario presses Control, which cuts speech off as any
//! key does. Home then speaks the first character of the caret's line,
//! the second line's, and the third line is never heard. The key is
//! pressed once the second line is heard starting, which is the evidence
//! that its index mark was reached; there is no other wait.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// The harness document's name.
const NAME: &str = "say-all";

/// The first line, read first.
const FIRST: &str = "Reading starts on this line.";

/// The second line, long enough to be playing when the key is pressed.
const SECOND: &str = "Mostly this second line is long enough to be playing when a key interrupts it, since it goes on for quite a while with nothing much to say.";

/// The third line, never reached.
const THIRD: &str = "Nobody hears this third line.";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let document = format!("{FIRST}\r\n{SECOND}\r\n{THIRD}\r\n");
    let pid = scenario.open_document_with("notepad.exe", NAME, &document)?;
    Ok(ScenarioState::TargetPid(pid))
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    scenario
        .speech()
        .expect_in_order(&["Notepad", "Text editor"], STEP_TIMEOUT);
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    scenario.speech().expect_exactly(&[FIRST], STEP_TIMEOUT);

    scenario
        .send_gesture("kb:verbatim+downarrow")
        .expect("sends say all");
    scenario.speech().expect_exactly(&[FIRST], STEP_TIMEOUT);
    // A long line can be spoken in more than one piece; its start is
    // enough.
    scenario
        .speech()
        .expect_playing("Mostly this second line", STEP_TIMEOUT);
    scenario.send_keys(&["control"]).expect("sends control");

    // The caret is on the line speech stopped in.
    scenario.send_keys(&["home"]).expect("sends home");
    scenario.speech().expect_exactly(&["M"], STEP_TIMEOUT);
    scenario.speech().wait_until_quiet(STEP_TIMEOUT);
    assert!(
        !scenario.speech().has_played(THIRD),
        "say-all went on after the key; transcript:\n{}",
        scenario.speech().transcript()
    );
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
