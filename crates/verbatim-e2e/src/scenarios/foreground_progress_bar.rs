//! A foreground window's UIA progress bar that is not the focus indicates
//! its changes, and another control's range value change says nothing
//! (`docs/parity/changes-and-selection.md`, "Progress bars that are not the
//! focus"). mockapp raises the changes; the wait before the first is for the
//! window's second property registration, the focus's being the first, so
//! the subscription is known to be in place.

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};
use crate::speech::heard;

use super::measure::{apply_to_mockapp, open_mockapp_counting};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const NAME: &str = "foreground-progress-bar";

const FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "focus_on_activate": true,
  "children": [
    {
      "id": "go",
      "role": "button",
      "name": "Go",
      "states": ["focusable", "focused"]
    },
    { "id": "bar", "role": "progress_bar", "name": "Copying", "value": "10" },
    { "id": "level", "role": "slider", "name": "Level", "value": "1" }
  ]
}
"#;

/// `UIA_AutomationPropertyChangedEventId`, for mockapp's `await-advised`.
const PROPERTY_CHANGED: i32 = 20004;

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let pid = open_mockapp_counting(scenario, NAME, FIXTURE);
    let title = harness_marker(NAME);
    scenario
        .speech()
        .expect(&[&format!("{title} window"), "Go button"]);
    let window = (NAME, pid);
    apply_to_mockapp(
        scenario,
        window,
        &format!("await-advised {PROPERTY_CHANGED} 2"),
    );

    apply_to_mockapp(scenario, window, "set-range bar 40");
    scenario
        .speech()
        .expect_sequence(&[heard("sound: progress")]);
    apply_to_mockapp(scenario, window, "set-range level 5");
    apply_to_mockapp(scenario, window, "set-range bar 60");
    scenario
        .speech()
        .expect_sequence(&[heard("sound: progress")]);
}
