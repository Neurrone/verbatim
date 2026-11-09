//! A selection in an element the focus controls, on `mockapp`'s lists: a
//! selection of the controlled list itself, one in a second controlled
//! element, one in a list the focus no longer controls, and an ordinary
//! selection after the list's items are replaced
//! (`docs/parity/changes-and-selection.md`, "Selection in a list the focus
//! controls"; NVDA's captures of 2026-11-09).

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

use super::measure::{apply_to_mockapp, open_mockapp};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const NAME: &str = "controlled-lists";

const FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "focus_on_activate": true,
  "children": [
    {
      "id": "search", "role": "editable_text", "name": "Search box",
      "automation_id": "SearchTextBox",
      "states": ["focusable", "focused"],
      "controller_for": "results"
    },
    {
      "id": "results",
      "role": "list",
      "name": "Suggestions",
      "states": ["selectable"],
      "children": [
        { "id": "sound", "role": "list_item", "name": "Sound settings",
          "states": ["focusable", "selectable"], "position_in_set": 1, "set_size": 2 },
        { "id": "display", "role": "list_item", "name": "Display settings",
          "states": ["focusable", "selectable"], "position_in_set": 2, "set_size": 2 }
      ]
    },
    {
      "id": "recent",
      "role": "list",
      "name": "Recent",
      "children": [
        { "id": "recent1", "role": "list_item", "name": "Elsewhere",
          "states": ["focusable", "selectable"] }
      ]
    }
  ]
}
"#;

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let pid = open_mockapp(scenario, NAME, FIXTURE);
    let window = (NAME, pid);
    let title = harness_marker(NAME);
    scenario
        .speech()
        .expect(&[&format!("{title} window"), "Search box edit"]);

    // A selection raised on the controlled list itself.
    apply_to_mockapp(scenario, window, "select results");
    scenario.speech().expect(&["Suggestions list selected"]);

    // The relation switched to a second list: its item is the event's own
    // element.
    apply_to_mockapp(scenario, window, "set-controller-for search recent");
    scenario.speech().expect(&["sound: suggestions-opened"]);
    apply_to_mockapp(scenario, window, "select recent1");
    scenario.speech().expect(&["Elsewhere"]);

    // An ordinary selection after the list's items are replaced.
    apply_to_mockapp(scenario, window, "set-controller-for search results");
    scenario.speech().expect(&["sound: suggestions-opened"]);
    apply_to_mockapp(scenario, window, "replace-items results");
    apply_to_mockapp(scenario, window, "select sound");
    scenario.speech().expect(&["Sound settings 1 of 2"]);
}
