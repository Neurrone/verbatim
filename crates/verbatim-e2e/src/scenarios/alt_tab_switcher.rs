//! The Alt+Tab switcher (`docs/parity/windows-and-processes.md`, "The
//! Alt+Tab switcher's items"): two of the harness's Windows Forms text
//! boxes are opened, the first and then the second, so the first is the
//! window used before the one in front. With Alt held, Tab opens the
//! switcher on the first window's item, which Verbatim says alone, by its
//! window's title, its row and column and its position in the list; with
//! Alt released, the first window comes to the front and is announced.
//!
//! The item's row, column and position depend on how many windows the
//! desktop has, so they are read through the agent, from the switcher's
//! own list, once the switcher is in front and before its speech is
//! asserted. Alt+Tab is what the scenario is about, so it is pressed
//! (`docs/testing.md`, "The harness does not change what it measures");
//! which window it reaches is known because the harness opened both, in
//! order, after every other window was minimized.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, WINDOW_TIMEOUT, harness_marker};

pub(crate) use super::no_teardown as teardown;

/// The first window's name in the run, the one switched back to.
const FIRST: &str = "switch-first";

/// The second window's name in the run, in front when Alt+Tab is pressed.
const SECOND: &str = "switch-second";

/// The text boxes' name.
const BOX_NAME: &str = "Note";

/// The first text box's one line.
const FIRST_LINE: &str = "The window switched back to.";

/// The second text box's one line.
const SECOND_LINE: &str = "The window switched away from.";

/// The title of the switcher's window, in front while it is open.
const SWITCHER_TITLE: &str = "Task Switching";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    super::text_box::open(scenario, FIRST, BOX_NAME, FIRST_LINE)?;
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(scenario, FIRST, BOX_NAME, FIRST_LINE);
    super::text_box::open(scenario, SECOND, BOX_NAME, SECOND_LINE)
        .expect("opens the second text box");
    super::text_box::expect_announced(scenario, SECOND, BOX_NAME, SECOND_LINE);

    let first = harness_marker(FIRST);
    scenario.hold_key("alt").expect("holds Alt");
    scenario.send_keys(&["tab"]).expect("sends Tab");
    scenario
        .wait_for_window_in_front(SWITCHER_TITLE, WINDOW_TIMEOUT)
        .expect("the switcher comes to the front");
    let (row, column, position, set_size) = scenario
        .switcher_item(&first)
        .expect("reads the switcher's item for the first window");
    scenario.speech().expect(&[&format!(
        "{first} row {row} column {column} {position} of {set_size}"
    )]);

    scenario.release_key("alt").expect("releases Alt");
    scenario
        .wait_for_window_in_front(&first, WINDOW_TIMEOUT)
        .expect("the first window comes to the front");
    super::text_box::expect_announced(scenario, FIRST, BOX_NAME, FIRST_LINE);
}
