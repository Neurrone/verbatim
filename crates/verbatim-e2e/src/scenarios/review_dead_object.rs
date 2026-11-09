//! The navigator on an object that no longer exists, in `mockapp`'s UIA
//! provider: the object is reached by navigation and then removed, as
//! `mockapp`'s disconnected nodes fail as a destroyed element's calls do
//! (`docs/design/m42-parity.md`, "Exact speech"; NVDA's captures of
//! 2026-11-09). Each command says the direction's edge and keeps the
//! navigator; review commands see empty flat text; the parent is the
//! window's; the to-focus command moves the navigator back.

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

use super::measure::{apply_to_mockapp, open_mockapp};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const NAME: &str = "review-dead-object";

const FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "focus_on_activate": true,
  "children": [
    {
      "id": "holder",
      "role": "group",
      "name": "Holder",
      "children": [
        { "id": "dead", "role": "button", "name": "Dead", "states": ["focusable"] },
        { "id": "holder2", "role": "button", "name": "Holder 2",
          "states": ["focusable", "focused"] }
      ]
    }
  ]
}
"#;

/// Sends `gesture` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

/// Sends `gesture`, which says nothing: the next press's words are the
/// next heard.
fn press_silent(scenario: &mut Scenario, gesture: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let pid = open_mockapp(scenario, NAME, FIXTURE);
    let window = (NAME, pid);
    scenario.speech().expect(&[
        &format!("{} window", harness_marker(NAME)),
        "Holder grouping",
        "Holder 2 button",
    ]);

    press(scenario, "kb:verbatim+numpad4", "Dead button");
    apply_to_mockapp(scenario, window, "disconnect dead");

    press(scenario, "kb:verbatim+numpad6", "No next");
    press(scenario, "kb:verbatim+numpad4", "No previous");
    press(scenario, "kb:verbatim+numpad2", "No objects inside");

    // Report Current Object says nothing, once and twice.
    press_silent(scenario, "kb:verbatim+numpad5");
    press_silent(scenario, "kb:verbatim+numpad5");

    press(scenario, "kb:numpad8", "blank");
    press(scenario, "kb:numpad5", "blank");
    press(scenario, "kb:numpad2", "blank");
    press(scenario, "kb:numpad9", "Bottom blank");

    press(scenario, "kb:verbatim+numpad8", "Desktop window");
    press(
        scenario,
        "kb:verbatim+numpadminus",
        "Move to focus Holder 2 button",
    );
}
