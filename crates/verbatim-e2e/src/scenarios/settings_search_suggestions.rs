//! Selection in a list the focus controls: the Settings app's search
//! suggestions, read as the selection moves through them while the
//! focus stays in the search box, as NVDA reads them
//! (`docs/parity/changes-and-selection.md`, "Selection in a list the
//! focus controls").
//!
//! Read-only, so it changes no setting. `ms-settings:system` opens the
//! System page with the keyboard focus in its search box, as its opening
//! speech asserts. "sound" is typed in one injection, all five characters
//! before Settings' search delay, so the suggestions open once, after the
//! last; Settings selects the first itself and announces that suggestions
//! are available. Down Arrow twice and Up Arrow then move the selection,
//! the focus staying in the box. How many suggestions there are, and
//! what they are called, comes from Settings' search index, which differs
//! between editions and builds, so the agent reads the list's size and its
//! first three names through UI Automation, independently of Verbatim,
//! once the list is open and before the scenario fixes what it expects.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::{cut_off, heard};

pub(crate) use super::no_teardown as teardown;

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_settings_page("ms-settings:system")?;
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    scenario.speech().expect(&[
        "Settings",
        "Settings window",
        "Search box, Find a setting edit",
        "blank",
    ]);
    // Each character's echo is cut off by the next key, the last heard in
    // full; then the application's notification that suggestions are
    // available, cut off by its selection of the first one.
    scenario.type_text("sound").expect("types the query");
    // NVDA says the first suggestion as "list item", the name its event's
    // cache holds, which Settings has not set yet; Verbatim reads its name.
    scenario.deviates_from_nvda(
        "docs/parity/changes-and-selection.md",
        "Selection in a list the focus controls",
    );
    scenario.speech().expect_sequence(&[
        cut_off("s"),
        cut_off("o"),
        cut_off("u"),
        cut_off("n"),
        heard("d"),
        cut_off("Suggestions available"),
    ]);
    let (count, names) = scenario
        .controlled_items(3)
        .expect("the agent reads the suggestions");
    let [first, second, third] = names.as_slice() else {
        panic!("the agent read {names:?}, not three suggestions");
    };
    let said = |name: &str, position: u32| format!("{name} {position} of {count}");
    // The search box's suggestions relation plays the suggestions-opened
    // indication after the first suggestion is said.
    scenario
        .speech()
        .expect_sequence(&[heard(&said(first, 1)), heard("sound: suggestions-opened")]);

    // Down Arrow, which moves no caret in the one-line box, moves the
    // selection; NVDA also says the box's line for the key, where
    // Verbatim says nothing for a caret key that moves nothing.
    for (key, said) in [
        ("downarrow", said(second, 2)),
        ("downarrow", said(third, 3)),
        ("uparrow", said(second, 2)),
    ] {
        scenario.send_keys(&[key]).expect("sends the key");
        scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
        scenario.speech().expect(&[&said]);
    }
}
