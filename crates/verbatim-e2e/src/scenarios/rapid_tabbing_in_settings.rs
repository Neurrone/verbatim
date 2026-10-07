//! Rapid focus churn (outpost redesign, step 7): many focus changes in
//! quick succession inside one application must leave Verbatim's focus and
//! navigator on the control that really has focus, with the outpost still
//! answering. Against Verbatim's own settings dialog, which every run has.
//!
//! The churn is a single burst of real keystrokes: Tab six times, then
//! Shift+Tab six times, sent in one request so they arrive faster than the
//! outpost reads each focus. Tab order is a fixed cycle, so the burst ends
//! where it started, on the selected category item.
//!
//! This is the one scenario whose speech is deliberately not all asserted
//! by its text. Which controls the burst passes through get read, and so
//! announced, depends on how the dialog's focus changes and the outpost's
//! reads interleave, as it would in NVDA, and the scenario's intent is the
//! end state, not the path. So every utterance the burst causes before its
//! final announcement must end cut off, none heard in full, whatever its
//! text; the final announcement, the category list entered and the
//! category item, is asserted exactly, queued once and heard in full; and
//! once Verbatim has handled the burst's last key and is idle, nothing more
//! was said. The report of the current navigator object must then be
//! exactly the category item, which proves the final focus won, and its
//! answer proves the outpost still answers queries.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Ending;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// The final announcement: the category list entered, then its selected
/// item, where the burst ends.
const FINAL: [&str; 2] = ["Categories: list Alt+c", "Speech 1 of 3"];

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario);
    let mut burst = vec!["tab"; 6];
    burst.extend(["shift+tab"; 6]);
    scenario.send_keys(&burst).expect("sends the burst");
    let said = scenario
        .speech()
        .take_until(FINAL[1], crate::scenario::WINDOW_TIMEOUT);
    let Some((last, before)) = said.split_last() else {
        unreachable!("take_until returns at least the utterance it waited for");
    };
    let Some((entered, passed)) = before.split_last() else {
        panic!(
            "the burst's final announcement lacks {:?}: {said:?}",
            FINAL[0]
        );
    };
    assert_eq!(
        [entered.text.as_str(), last.text.as_str()],
        FINAL,
        "the burst's final announcement"
    );
    for heard in passed {
        scenario.speech().expect_ended(heard, Ending::Cancelled);
    }
    scenario.speech().expect_ended(entered, Ending::Completed);
    scenario.speech().expect_ended(last, Ending::Completed);
    scenario.expect_nothing_more();

    scenario
        .send_gesture("kb:verbatim+numpad5")
        .expect("sends report-current-object");
    scenario
        .speech()
        .expect(&["Speech list item focused selected 1 of 3"]);
    super::close_settings_to_desktop(scenario);
}
