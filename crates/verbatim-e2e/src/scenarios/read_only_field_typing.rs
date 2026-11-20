//! Typing into a read-only edit field echoes nothing, with character echo
//! on only in edit controls (the default): mockapp's classic single-line
//! edit with `ES_READONLY` and no tab stop, labelled "Location:", the
//! stand-in for the read-only Location field of a file's Properties, which
//! the system's MSAA proxy answers as editable text with the read-only
//! state (`crates/mockapp/tests/focus_reports.rs` checks it against the
//! field's recording). The field is focused directly, as the field, no tab
//! stop, is reached by a click or UI Automation's `SetFocus`.
//!
//! NVDA's speech, captured on the Location field itself (5 of 5 runs, the
//! pinned build, 2026-11-19): on the focus "Location:, edit, read only,
//! C:\...\annfolder"; typed "a", nothing but a cancel; typed "b",
//! nothing. Verbatim says the field, then its line, as every scenario hears
//! a focused field and its line, and nothing for "a" and "b".

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

use super::mockapp::{apply_to_mockapp, open_msaa_mockapp};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const NAME: &str = "read-only-field-typing";

/// The field's text, a folder's path as the Location field holds.
const PATH: &str = r"C:\Users\Public";

const FIXTURE: &str = include_str!("../../../mockapp/tests/fixtures/read_only_field.json");

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let app = (NAME, open_msaa_mockapp(scenario, NAME, FIXTURE));
    // MSAA mockapp raises no focus for its controls as it opens.
    let title = harness_marker(NAME);
    scenario.speech().expect(&[&format!("{title} window")]);
    apply_to_mockapp(scenario, app, "edit-focus");
    scenario
        .speech()
        .expect(&["Location: edit read only", PATH]);
    scenario.type_text("a").expect("types a");
    scenario.type_text("b").expect("types b");
    scenario.expect_nothing_more();
}
