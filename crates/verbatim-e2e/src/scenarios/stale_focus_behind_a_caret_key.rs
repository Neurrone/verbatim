//! A stale MSAA focus behind a caret key is confirmed and does not silence
//! the key: mockapp's classic edit field, empty, beside a tree view, the
//! stand-in for the Open dialog's "File name" field and folder tree. After
//! each Left Arrow the field receives, the first read of its text from
//! another process, a reader's look for the key's effect, raises
//! `EVENT_OBJECT_FOCUS` on the tree's selected item, while the field keeps
//! the focus, as the Open dialog's tree raises its stale focus (survey
//! `np_open`: `SysTreeView32`, `OBJID_CLIENT`, an item's child id, the
//! tree without the focus; NVDA refusing it in every run). Triggered by
//! that read, the focus always arrives while the key's watch is out. Core
//! confirms the focus with a read of its facts, finds it not focused, and
//! the arrow says "blank".
//!
//! The speech is NVDA's, captured on this scenario's steps against the same
//! mockapp (10 of 10 runs, the pinned build, 2026-11-18): "<title> window";
//! the field's focus, "edit multi line blank", as mockapp's edit is
//! `ES_MULTILINE`, which Verbatim speaks as two utterances, the field then
//! its empty line, as every scenario hears a focused field and its line;
//! and the arrow's "blank" about 105 ms after the key, the
//! stale focus arriving 23 to 44 ms after the arrow, inside NVDA's caret
//! wait, and refused, never silencing the key. That capture placed the
//! focus 30 ms after the key; the speech does not depend on the trigger.

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
    send_to_mockapp(scenario, app, "stale-focus-on-left");
    await_mockapp(scenario, app);
    scenario.speech().expect(&["edit multi line", "blank"]);
    // The arrow in the empty field: the tree's focus, raised at the watch's
    // first read, is found not focused, and the arrow says its character,
    // blank.
    scenario.send_keys(&["leftarrow"]).expect("sends the key");
    scenario.speech().expect(&["blank"]);
}
