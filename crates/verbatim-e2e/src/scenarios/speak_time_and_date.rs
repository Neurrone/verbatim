//! Verbatim+F12 says the time, and pressed twice quickly, the date
//! (`ScriptAction::SpeakTime`), each as Windows formats it in the user's
//! locale: the time without seconds and the date in the long format.
//!
//! Both are read through the agent first, from the same clock and with the
//! same formats, independently of Verbatim. A read within two seconds of
//! the next minute waits for the minute to turn, on the clock's own timer,
//! so Verbatim's reads, made at once after, fall in the same minute. The
//! two presses are real keys, the second pressed as soon as the time
//! starts to play, so it counts as a second press and cuts the time off.

use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Ending;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// How close to the next minute the clock's read waits for it to turn.
const TURN_WITHIN: Duration = Duration::from_secs(2);

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let (time, date) = scenario
        .local_clock(TURN_WITHIN)
        .expect("reads the local clock");
    scenario
        .send_keys(&["insert+f12"])
        .expect("sends Verbatim+F12");
    let said = scenario.speech().expect_started(&time);
    scenario
        .send_keys(&["insert+f12"])
        .expect("sends Verbatim+F12 again");
    scenario.speech().expect_ended(&said, Ending::Cancelled);
    scenario.speech().expect(&[&date]);
}
