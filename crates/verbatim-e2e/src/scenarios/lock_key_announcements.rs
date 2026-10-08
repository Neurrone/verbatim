//! A lock key's new state is announced: Scroll Lock pressed through real
//! keyboard input reaches Windows, the keyboard hook reports the lock-key
//! gesture, and the gesture router speaks "scroll lock on" or "scroll lock
//! off" once the key's state has changed, as NVDA does. Scroll Lock,
//! because Caps Lock is the Verbatim modifier under the suite's settings,
//! where a single press is swallowed, and Num Lock changes what the numpad
//! navigation gestures send.
//!
//! The key's state is read through the agent first, independently of
//! Verbatim, so each announcement is fixed before the key is pressed, and
//! read again after each press, showing the key really changed. The key is
//! pressed twice, so the machine is left as it was found.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// What Verbatim says when Scroll Lock turns `on` or off.
fn announcement(on: bool) -> &'static str {
    if on {
        "scroll lock on"
    } else {
        "scroll lock off"
    }
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let started_on = scenario
        .key_toggled("scrolllock")
        .expect("reads Scroll Lock's state");
    for expected_on in [!started_on, started_on] {
        scenario
            .send_keys(&["scrolllock"])
            .expect("sends Scroll Lock");
        scenario.speech().expect(&[announcement(expected_on)]);
        assert_eq!(
            scenario
                .key_toggled("scrolllock")
                .expect("reads Scroll Lock's state"),
            expected_on,
            "Scroll Lock's state after the press"
        );
    }
}
