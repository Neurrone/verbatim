//! A search box holding text, entered again while its suggestions open:
//! the box's line is part of its focus announcement and comes in the
//! focus's own message, so it is spoken with the box, before the
//! suggestion Settings selects as the focus arrives, which cuts both off
//! (`docs/parity/focus-announcement.md`, "Text on focus"). The sequence is that of the NVDA capture of
//! 2026-10-21 (`target/e2e-evidence/fixes-round-six-2026-10-21/rapid-tab-nvda-1.txt`):
//! "sound" typed into Settings' search box, Escape, which closes the
//! suggestions, Tab to the account button, and Shift+Tab back into the box,
//! where NVDA says "Search box, Find a setting edit sound", cut off by
//! "Sound settings 1 of 22".
//!
//! Read-only, so it changes no setting. The suggestions' size and first
//! name come from the agent's read of the list once it is open, as in
//! [`settings_search_suggestions`](super::settings_search_suggestions). The
//! account button's name is the machine's account name, so the agent reads
//! it by its UI Automation identifier, `UserProfileControlButton`, before
//! the Tab reaches it.
//!
//! Local-only, as `settings_search_suggestions` is: GitHub's runner's
//! Settings raises no selection event for a first suggestion as the list
//! opens.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::{cut_off, heard, optional};

pub(crate) use super::no_teardown as teardown;

/// The UI Automation identifier of Settings' account button, the Tab stop
/// after the search box.
const ACCOUNT_BUTTON: &str = "UserProfileControlButton";

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
        .controlled_items(1)
        .expect("the agent reads the suggestions");
    let [first] = names.as_slice() else {
        panic!("the agent read {names:?}, not one suggestion");
    };
    let suggestion = format!("{first} 1 of {count}");
    scenario
        .speech()
        .expect_sequence(&[heard(&suggestion), heard("sound: suggestions-opened")]);

    scenario.send_keys(&["escape"]).expect("sends Escape");
    // Settings sometimes raises the box's ControllerFor change twice, a
    // few milliseconds apart, as the suggestions close, and the sound is
    // played for each, as NVDA plays it for each it receives (NVDA's UIA
    // log received the second in one Escape of three,
    // `target/e2e-evidence/m42-desktop-2-2026-10-22/part3a-nvda-2.log`):
    // an optional utterance at the end of the step (`docs/testing.md`,
    // "Optional utterances"), matched against what has arrived once the
    // first has been heard in full.
    scenario.speech().expect_sequence_with_trailing_optional(&[
        heard("sound: suggestions-closed"),
        optional("sound: suggestions-closed"),
    ]);

    let account = scenario
        .name_by_automation_id(ACCOUNT_BUTTON)
        .expect("the agent reads the account button's name");
    scenario.send_keys(&["tab"]).expect("sends Tab");
    scenario.speech().expect(&[&format!("{account} button")]);

    // The box and its line, cut off by the suggestion Settings selects as
    // the focus returns.
    scenario.send_keys(&["shift+tab"]).expect("sends Shift+Tab");
    scenario.speech().expect_sequence(&[
        cut_off("Search box, Find a setting edit"),
        cut_off("sound"),
        heard(&suggestion),
    ]);
}
