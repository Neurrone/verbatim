//! Object navigation over UI Automation (milestone M3 reducer item 4):
//! the walk `object_navigation_in_settings` makes over MSAA, made in
//! `mockapp`'s UIA provider, the scripted application staged beside
//! Verbatim, which is the same on every machine. Its window holds a group,
//! "Options", of two buttons and a check box, the first button focused.
//!
//! The walk reports the current object, moves to its next and previous
//! siblings, past the last sibling to the edge ("No next"), up to the group
//! and the window, down to the window's title bar and across to the group,
//! down to the group's first child and past it to the edge ("No
//! previous"), and then moves
//! the navigator back to the focus. Every step asserts exactly what is
//! said. Gestures are the desktop layout's bindings, sent through the
//! control plane by their identifiers, so `NumLock` cannot affect the run.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

pub(crate) use super::no_teardown as teardown;

/// The harness name of the window.
const NAME: &str = "object-navigation";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let directory = scenario.run_directory().to_owned();
    let title = harness_marker(NAME);
    let fixture = scenario.harness_file(NAME, "json");
    let contents = format!(
        r#"{{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    {{
      "id": "options",
      "role": "group",
      "name": "Options",
      "children": [
        {{ "id": "ok", "role": "button", "name": "OK", "states": ["focusable", "focused"] }},
        {{ "id": "cancel", "role": "button", "name": "Cancel", "states": ["focusable"] }},
        {{ "id": "enable", "role": "check_box", "name": "Enable", "states": ["focusable", "checked"] }}
      ]
    }}
  ]
}}
"#
    );
    scenario.write_agent_file(&fixture, contents.as_bytes())?;
    let args = [
        "--fixture",
        &fixture,
        "--backend",
        "uia",
        "--title",
        &title,
        "--show",
    ]
    .map(str::to_owned);
    let window =
        scenario.launch_titled(&format!(r"{directory}\mockapp.exe"), &args, &title, true)?;
    Ok(ScenarioState::TargetPid(window.pid))
}

/// Sends `gesture` and asserts exactly `heard`.
fn navigate(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let window = format!("{} window", harness_marker(NAME));
    scenario
        .speech()
        .expect(&[&window, "Options grouping", "OK button"]);

    // The current object, reported with its states; across its siblings
    // to the edge and back.
    navigate(scenario, "kb:verbatim+numpad5", "OK button focused");
    navigate(scenario, "kb:verbatim+numpad6", "Cancel button");
    navigate(scenario, "kb:verbatim+numpad6", "Enable check box checked");
    navigate(scenario, "kb:verbatim+numpad6", "No next");
    navigate(scenario, "kb:verbatim+numpad4", "Cancel button");

    // Up to the group and the window; down to the window's first child,
    // its title bar, which UIA gives every window, across to the group,
    // and down to its first child.
    navigate(scenario, "kb:verbatim+numpad8", "Options grouping");
    navigate(scenario, "kb:verbatim+numpad8", &window);
    navigate(
        scenario,
        "kb:verbatim+numpad2",
        &format!("title bar {}", harness_marker(NAME)),
    );
    navigate(scenario, "kb:verbatim+numpad6", "Options grouping");
    navigate(scenario, "kb:verbatim+numpad2", "OK button");
    navigate(scenario, "kb:verbatim+numpad4", "No previous");

    // Back to the focus.
    navigate(scenario, "kb:verbatim+numpad6", "Cancel button");
    navigate(
        scenario,
        "kb:verbatim+numpadminus",
        "Move to focus OK button",
    );
}
