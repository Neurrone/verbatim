//! The identity check in the duplicate test with a real list view
//! (`docs/parity/focus-events.md`, "Duplicate focus suppression"): mockapp's
//! comctl32 list view holds two items both named "Copy". The first takes
//! the focus and is spoken; with no key between, mockapp deletes it as the
//! desktop's list deletes its focused item, the list's reset raised and
//! then a focus at the deleted item's child id, where the second now is.
//! Its MSAA focus is read unless it is the first by its ids and its
//! reading, and it is not, its set one smaller: "Copy 1 of 1".
//!
//! The stand-in imitates the desktop list's recording of its focused item
//! deleted (2026-11-18, 5 of 5 runs: destroy and create on the list's
//! client, child 0, then the focus at the deleted item's child id, then the
//! selection within), checked by mockapp's
//! `the_twin_deletion_stand_in_matches_the_desktop_lists_recording`. There
//! the Delete key came first, and NVDA's File Explorer rule refused the
//! focus; here no key comes between, and mockapp's list has no such rule.
//! The speech is to be captured from NVDA on this stand-in.

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

use super::mockapp::{await_mockapp, open_msaa_mockapp, send_to_mockapp};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const NAME: &str = "twin-item-at-a-deleted-items-ids";

const FIXTURE: &str = include_str!("../../../mockapp/tests/fixtures/list_view_twins.json");

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
