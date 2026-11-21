//! Verbatim+F12 says the time, and pressed twice quickly, the date
//! (`ScriptAction::SpeakTime`), each as Windows formats it in the user's
//! locale: the time without seconds and the date in the long format.
//!
//! Both are read through the agent first, from the same clock and with the
//! same formats, independently of Verbatim. A read within two seconds of
//! the next minute waits for the minute to turn, on the clock's own timer,
//! so Verbatim's reads, made at once after, fall in the same minute. The
//! two presses are real keys sent in one burst, well within the
//! multi-press timeout, so the second counts as a second press: the time,
//! said at the first, is cut off by the date, heard in full.

use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::{cut_off, heard};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// How close to the next minute the clock's read waits for it to turn.
const TURN_WITHIN: Duration = Duration::from_secs(2);

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let (time, date) = scenario
        .local_clock(TURN_WITHIN)
        .expect("reads the local clock");
    scenario
        .send_keys(&["insert+f12", "insert+f12"])
        .expect("sends Verbatim+F12 twice");
    scenario
        .speech()
        .expect_sequence(&[cut_off(&time), heard(&date)]);
}
