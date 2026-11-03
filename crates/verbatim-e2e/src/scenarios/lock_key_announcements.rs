//! A lock key's new state is announced: Scroll Lock pressed through real
//! keyboard input reaches Windows, the keyboard hook reports the lock-key
//! gesture, and the reducer thread reads the key's state 30 ms later and
//! speaks "scroll lock on" or "scroll lock off", as NVDA does. Num Lock is
//! left alone, since it changes what the numpad navigation gestures send.
//!
//! Caps Lock is the Verbatim modifier under the suite's settings, where a
//! single press is swallowed; a double tap passes it through to Windows,
//! which toggles it, and its new state is announced the same way.
//!
//! Each key's state is read through the agent first, independently of
//! Verbatim, so each announcement is fixed before the key is pressed, and
//! read again after each press, showing the key really changed. Each key
//! is toggled twice, so the machine is left as it was found.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// What Verbatim says when the lock `name` (such as "scroll lock") turns
/// `on` or off.
fn announcement(name: &str, on: bool) -> String {
    format!("{name} {}", if on { "on" } else { "off" })
}

/// Toggles the lock key `key`, named `name`, twice, each time with
/// `keys`, asserting its announcement and its new state.
fn toggle_twice(scenario: &mut Scenario, key: &str, name: &str, keys: &[&str]) {
    let started_on = scenario.key_toggled(key).expect("reads the key's state");
    for expected_on in [!started_on, started_on] {
        scenario.send_keys(keys).expect("sends the keys");
        scenario
            .speech()
            .expect(&[&announcement(name, expected_on)]);
        assert_eq!(
            scenario.key_toggled(key).expect("reads the key's state"),
            expected_on,
            "{name}'s state after {keys:?}"
        );
    }
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    toggle_twice(scenario, "scrolllock", "scroll lock", &["scrolllock"]);
    // A double tap of the Verbatim key, within the multi-press timeout.
    toggle_twice(scenario, "capslock", "caps lock", &["capslock", "capslock"]);
}
