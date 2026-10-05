//! A lock key's new state is announced: Scroll Lock pressed through real
//! keyboard input reaches Windows, the keyboard hook reports the lock-key
//! gesture, and the gesture router speaks "scroll lock on" or "scroll lock
//! off" once the key's state has changed, as NVDA does. Scroll Lock, because
//! Caps Lock is the Verbatim modifier under the suite's settings, where a
//! single press is swallowed, and Num Lock changes what the numpad
//! navigation gestures send.
//!
//! The key is sent with `SendKeys`, not `SendGesture`: the announcement
//! follows the real key, whose state must change first, so injecting the
//! gesture alone would only report the state as it already was. The key is
//! pressed twice, so the machine is left as it was found, and the two
//! announcements must name opposite states whichever one it started in.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Generous: the announcement follows the key by about 30 ms, but the suite
/// runs on a real desktop.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    scenario
        .send_keys(&["scrolllock"])
        .expect("sends Scroll Lock");
    let first = scenario
        .speech()
        .expect_in_order_capturing(&["scroll lock"], STEP_TIMEOUT);
    let second = if first.contains("on") {
        "scroll lock off"
    } else {
        "scroll lock on"
    };
    scenario
        .send_keys(&["scrolllock"])
        .expect("sends Scroll Lock again");
    scenario.speech().expect_in_order(&[second], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {}
