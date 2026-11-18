//! The identity check in the duplicate test with a real list view
//! (`docs/parity/focus-events.md`, "Duplicate focus suppression"): mockapp's
//! comctl32 list view holds two items both named "Copy". The first takes
//! the focus and is spoken; with no key between, mockapp deletes it and
//! focuses the second, now at the first's child id. Its MSAA focus is read
//! unless it is the first by its ids and its reading, and it is not, its set
//! one smaller: "Copy 1 of 1".
//!
//! Code-level: no recording of a real application gives a deletion that
//! focuses the next item with no key. The desktop's list, recorded deleting
//! its focused item by the Delete key (2026-11-18), raised its focus at the
//! deleted item's child id after the key, and focused nothing. The speech
//! is not yet captured from NVDA; NVDA's duplicate test never holds for a
//! child id above 0, so it queues the second item's focus as Verbatim reads
//! it.

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

use super::measure::{await_mockapp, open_msaa_mockapp, send_to_mockapp};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const NAME: &str = "twin-item-at-a-deleted-items-ids";

const FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    {
      "id": "clipboard",
      "role": "list",
      "native": "list_view",
      "columns": [["Name", 200]],
      "children": [
        { "id": "first", "role": "list_item", "name": "Copy" },
        { "id": "second", "role": "list_item", "name": "Copy" }
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
    // The list takes the focus on its first item.
    send_to_mockapp(scenario, app, "list-view-focus 0");
    await_mockapp(scenario, app);
    scenario.speech().expect(&["list", "Copy 1 of 2"]);
    // The first deleted and its twin focused at its child id, with no key
    // between: the same name, its set one smaller.
    send_to_mockapp(scenario, app, "list-view-delete 0");
    await_mockapp(scenario, app);
    scenario.speech().expect(&["Copy 1 of 1"]);
}
