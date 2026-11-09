//! A classic application's MSAA flood does not hold up focus in another
//! window, and Verbatim goes idle afterwards: mockapp's MSAA window raises
//! value changes on new objects, value changes on its own objects, and
//! show events on new objects, from one thread, while the focus moves
//! between two other windows. The flood's events say nothing; each focus
//! is asserted in full, and the scenario's end asserts that Verbatim is
//! idle with nothing further said.

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

use super::measure::{await_mockapp, open_mockapp, open_msaa_mockapp, send_to_mockapp};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const FLOODING: &str = "msaa-flood-source";
const FIRST: &str = "msaa-flood-first";
const SECOND: &str = "msaa-flood-second";

/// How many events each flood raises, a quarter of a millisecond apart,
/// so the focus's change comes while it runs.
const FLOOD: &str = "4000 0.25";

const FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    {
      "id": "go",
      "role": "button",
      "name": "Go",
      "states": ["focusable", "focused"]
    }
  ]
}
"#;

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let flooding = (FLOODING, open_msaa_mockapp(scenario, FLOODING, FIXTURE));
    expect_window(scenario, FLOODING);
    open_mockapp(scenario, FIRST, FIXTURE);
    expect_window(scenario, FIRST);
    open_mockapp(scenario, SECOND, FIXTURE);
    expect_window(scenario, SECOND);

    flood_and_move_to(scenario, flooding, "new-values", FIRST);
    flood_and_move_to(scenario, flooding, "repeated-values", SECOND);
    flood_and_move_to(scenario, flooding, "new-shows", FIRST);
}

/// Starts a flood of `kind` from `flooding`, brings the window `to`
/// forward while it runs, and asserts its focus; then waits for the flood
/// to end.
fn flood_and_move_to(scenario: &mut Scenario, flooding: (&str, u32), kind: &str, to: &str) {
    send_to_mockapp(scenario, flooding, &format!("flood {FLOOD} {kind}"));
    scenario
        .bring_window_forward(&harness_marker(to))
        .expect("brings the window forward");
    expect_window(scenario, to);
    await_mockapp(scenario, flooding);
}

/// What the window `name` says as it comes to the foreground.
fn expect_window(scenario: &mut Scenario, name: &str) {
    let title = harness_marker(name);
    scenario
        .speech()
        .expect(&[&format!("{title} window"), "Go button"]);
}
