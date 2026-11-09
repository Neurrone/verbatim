//! A UIA and a classic progress bar in background windows
//! (`docs/design/progress-bars.md`, "Rules"): with
//! `report_background_progress_bars` on each change is indicated, and off
//! each says nothing, shown by the tone of a foreground bar that changes
//! after it. The listener reports both bars, with no outpost asked. A
//! change raised before a client has registered for it reaches nobody, so
//! each UIA window is waited on for a registration of property changes, as
//! in `foreground_progress_bar`; a classic bar's event needs none.

use verbatim_config::Settings;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};
use crate::speech::heard;

use super::measure::{apply_to_mockapp, open_mockapp_counting, open_msaa_mockapp};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const UIA_BAR: &str = "background-progress-bar-uia";
const CLASSIC_BAR: &str = "background-progress-bar-classic";
const FRONT: &str = "background-progress-bar-front";

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
    { "id": "bar", "role": "progress_bar", "name": "Copying", "value": "10",
      "location": [10, 10, 200, 20] }
  ]
}
"#;

/// A UIA window holding only its bar. Nothing in it takes the focus, so it
/// says nothing as it opens, and its bar's change is the first speech it
/// causes.
const UIA_FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    { "id": "bar", "role": "progress_bar", "name": "Copying", "value": "10",
      "location": [10, 10, 200, 20] }
  ]
}
"#;

/// `UIA_AutomationPropertyChangedEventId`, for mockapp's `await-advised`.
const PROPERTY_CHANGED: i32 = 20004;

/// The settings of [`on_body`]: background progress bars indicated.
pub(crate) fn report_background_bars(settings: &mut Settings) {
    settings.reader.report_background_progress_bars = true;
}

/// Opens the UIA window `name` and waits for a client to be listening for
/// its property changes.
fn open_uia_window<'a>(scenario: &mut Scenario, name: &'a str) -> (&'a str, u32) {
    let window = (name, open_mockapp_counting(scenario, name, UIA_FIXTURE));
    apply_to_mockapp(
        scenario,
        window,
        &format!("await-advised-at-least {PROPERTY_CHANGED} 1"),
    );
    window
}

/// Opens the classic window, asserting what it says as it comes to the
/// foreground: MSAA mockapp raises no focus for its button as it opens.
fn open_classic_window<'a>(scenario: &mut Scenario, name: &'a str) -> (&'a str, u32) {
    let window = (name, open_msaa_mockapp(scenario, name, FIXTURE));
    let title = harness_marker(name);
    scenario.speech().expect(&[&format!("{title} window")]);
    window
}

/// With the setting on, each background bar's change is indicated.
pub(crate) fn on_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let uia = open_uia_window(scenario, UIA_BAR);
    let classic = open_classic_window(scenario, CLASSIC_BAR);
    open_classic_window(scenario, FRONT);

    apply_to_mockapp(scenario, uia, "set-range bar 40");
    scenario
        .speech()
        .expect_sequence(&[heard("sound: progress")]);
    apply_to_mockapp(scenario, classic, "set-value bar 60");
    scenario
        .speech()
        .expect_sequence(&[heard("sound: progress")]);
}

/// With the setting off, each background bar's change says nothing: the one
/// tone after it is the foreground bar's, whose change came later.
pub(crate) fn off_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let uia = open_uia_window(scenario, UIA_BAR);
    let classic = open_classic_window(scenario, CLASSIC_BAR);
    let front = open_classic_window(scenario, FRONT);

    apply_to_mockapp(scenario, uia, "set-range bar 40");
    apply_to_mockapp(scenario, front, "set-value bar 20");
    scenario
        .speech()
        .expect_sequence(&[heard("sound: progress")]);
    apply_to_mockapp(scenario, classic, "set-value bar 60");
    apply_to_mockapp(scenario, front, "set-value bar 30");
    scenario
        .speech()
        .expect_sequence(&[heard("sound: progress")]);
}
