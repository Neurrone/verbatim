//! The Alt+Tab switcher (`docs/parity/windows-and-processes.md`, "The
//! Alt+Tab switcher's items"): two of the harness's Windows Forms text
//! boxes are opened, the first and then the second, so the first is the
//! window used before the one in front. With Alt held, Tab opens the
//! switcher on the first window's item, which Verbatim says alone, by its
//! window's title, its row and column and its position in the list. Still
//! with Alt held, Tab is pressed once for each item of the list, moving to
//! the next item each time, from the last back to the first, so the
//! switcher goes once round its list, each item said as the first was, and
//! comes back to the first window's item; with Alt released there, the
//! first window comes to the front and is announced.
//!
//! The items, their rows, columns and positions depend on how many windows
//! the desktop has, so the whole list is read through the agent, from the
//! switcher's own list, once the switcher is in front and before any of
//! its speech is asserted. Alt+Tab is what the scenario is about, so it is pressed
//! (`docs/testing.md`, "The harness does not change what it measures");
//! which window it reaches is known because the harness opened both, in
//! order, after every other window was minimized.

use std::io;

use verbatim_agent::protocol::SwitcherItem;

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
    let items = scenario
        .switcher_items()
        .expect("reads the switcher's items");
    let count = items.len();
    let places: Vec<(usize, u32, u32)> = items
        .iter()
        .enumerate()
        .map(|(index, item)| (index, item.position, item.set_size))
        .collect();
    let expected: Vec<(usize, u32, u32)> = (0..count)
        .map(|index| {
            let position = u32::try_from(index + 1).expect("a position");
            let size = u32::try_from(count).expect("a size");
            (index, position, size)
        })
        .collect();
    assert_eq!(
        places, expected,
        "each of the switcher's items is at its own position in a list of its size: {items:?}"
    );
    assert_eq!(
        items.get(1).map(|item| item.name.as_str()),
        Some(first.as_str()),
        "the switcher opens on its second item, the window used before the one in front: {items:?}"
    );
    scenario.speech().expect(&[&said(&items[1])]);

    // Once round the list: the next item at each Tab, after the last the
    // first, until the first window's item again.
    for step in 1..=count {
        scenario.send_keys(&["tab"]).expect("sends Tab");
        scenario.speech().expect(&[&said(&items[(1 + step) % count])]);
    }

    scenario.release_key("alt").expect("releases Alt");
    scenario
        .wait_for_window_in_front(&first, WINDOW_TIMEOUT)
        .expect("the first window comes to the front");
    super::text_box::expect_announced(scenario, FIRST, BOX_NAME, FIRST_LINE);
}

/// What Verbatim says of `item` reached with Alt held: its window's title,
/// row, column and position in the list.
fn said(item: &SwitcherItem) -> String {
    format!(
        "{} row {} column {} {} of {}",
        item.name, item.row, item.column, item.position, item.set_size
    )
}
