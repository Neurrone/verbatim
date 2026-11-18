//! A stale MSAA focus behind a caret key is confirmed and does not silence
//! the key: mockapp's classic edit field, empty, beside a tree view, the
//! stand-in for the Open dialog's "File name" field and folder tree. Each
//! Left Arrow the field receives raises `EVENT_OBJECT_FOCUS` on the tree's
//! selected item 30 ms later, inside the key's 100 ms watch, while the
//! field keeps the focus, as the Open dialog's tree raises its stale focus
//! (survey `np_open`: `SysTreeView32`, `OBJID_CLIENT`, an item's child id,
//! the tree without the focus; NVDA refusing it in every run). Core
//! confirms the focus with a read of its facts, finds it not focused, and
//! the arrow says "blank", as NVDA's field arrow did in all 18 captured
//! runs where its script ran (Dickson, 2026-11-18). The placement inside
//! the watch is the stand-in's: a real Open dialog cannot place it there
//! on demand.

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

use super::measure::{await_mockapp, open_msaa_mockapp, send_to_mockapp};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const NAME: &str = "stale-focus-behind-a-caret-key";

const FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    {
      "id": "file",
      "role": "editable_text",
      "states": ["focusable"],
      "text": ""
    },
    {
      "id": "folders",
      "role": "tree",
      "native": "tree_view",
      "children": [
        { "id": "pc", "role": "tree_item", "name": "This PC", "states": ["selected"] },
        { "id": "network", "role": "tree_item", "name": "Network" }
      ]
    }
  ]
}
"#;

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let app = (NAME, open_msaa_mockapp(scenario, NAME, FIXTURE));
    // MSAA mockapp raises no focus for its controls as it opens.
    let title = harness_marker(NAME);
    scenario.speech().expect(&[&format!("{title} window")]);
    // The field takes the focus, and the tree's stale focus is armed.
    send_to_mockapp(scenario, app, "stale-focus-on-left 30");
    await_mockapp(scenario, app);
    scenario.speech().expect(&["edit", "blank"]);
    // The arrow in the empty field: the tree's focus 30 ms later is found
    // not focused, and the arrow says its character, blank.
    scenario.send_keys(&["leftarrow"]).expect("sends the key");
    scenario.speech().expect(&["blank"]);
}
