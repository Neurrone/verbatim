//! A grid's cells, in `mockapp`'s UIA grids: moving within one table says
//! only the row or column that changed, and entering a table says both,
//! even at the row and column last spoken
//! (`docs/parity/focus-announcement.md`, "Table cell coordinates").

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

use super::measure::{apply_to_mockapp, open_mockapp};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const NAME: &str = "table-cells";

const FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "focus_on_activate": true,
  "children": [
    {
      "id": "grid-a",
      "role": "list",
      "name": "Grid A",
      "children": [
        {
          "id": "first", "role": "list_item", "name": "First",
          "states": ["focusable", "focused"], "position_in_set": 2, "set_size": 6,
          "cell": { "row": 0, "column": 1, "grid": "grid-a" }
        },
        {
          "id": "second", "role": "list_item", "name": "Second",
          "states": ["focusable"], "position_in_set": 3, "set_size": 6,
          "cell": { "row": 0, "column": 2, "grid": "grid-a" }
        },
        {
          "id": "third", "role": "list_item", "name": "Third",
          "states": ["focusable"], "position_in_set": 6, "set_size": 6,
          "cell": { "row": 1, "column": 2, "grid": "grid-a" }
        }
      ]
    },
    {
      "id": "grid-b",
      "role": "list",
      "name": "Grid B",
      "children": [
        {
          "id": "other", "role": "list_item", "name": "Other",
          "states": ["focusable"], "position_in_set": 1, "set_size": 2,
          "cell": { "row": 1, "column": 2, "grid": "grid-b" }
        }
      ]
    }
  ]
}
"#;

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let pid = open_mockapp(scenario, NAME, FIXTURE);
    let title = harness_marker(NAME);
    let window = (NAME, pid);
    scenario.speech().expect(&[
        &format!("{title} window"),
        "Grid A list",
        "First row 1 column 2 2 of 6",
    ]);

    // The next cell in the row says only its column.
    apply_to_mockapp(scenario, window, "set-focus second");
    scenario.speech().expect(&["Second column 3 3 of 6"]);

    // The next cell in the column says only its row.
    apply_to_mockapp(scenario, window, "set-focus third");
    scenario.speech().expect(&["Third row 2 6 of 6"]);

    // Another table says both, at the same row and column.
    apply_to_mockapp(scenario, window, "set-focus other");
    scenario
        .speech()
        .expect(&["Grid B list", "Other row 2 column 3 1 of 2"]);

    // And so does coming back.
    apply_to_mockapp(scenario, window, "set-focus third");
    scenario
        .speech()
        .expect(&["Grid A list", "Third row 2 column 3 6 of 6"]);
}
