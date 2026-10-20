//! Selection in a list the focus controls: the Settings app's search
//! suggestions, read as the selection moves through them while the
//! focus stays in the search box, as NVDA reads them
//! (`docs/parity/changes-and-selection.md`, "Selection in a list the
//! focus controls").
//!
//! Read-only, so it changes no setting. `ms-settings:system` opens the
//! System page with the keyboard focus in its search box, as its opening
//! speech asserts. Typing "sound" opens the suggestions, of which the
//! Settings app selects the first itself and announces that suggestions
//! are available; Down Arrow twice and Up Arrow then move the selection,
//! the focus staying in the box.

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
    for character in ["s", "o", "u", "n"] {
        scenario.type_text(character).expect("types a character");
        scenario.speech().expect(&[character]);
    }
    // The last character opens the suggestions: the application's
    // notification that they are available, cut off by its selection of
    // the first one, and the search box's suggestions relation, which
    // plays the suggestions-opened indication.
    scenario.type_text("d").expect("types a character");
    scenario.speech().expect_sequence(&[
        heard("d"),
        cut_off("Suggestions available"),
        heard("Sound settings 1 of 22"),
        heard("sound: suggestions-opened"),
    ]);

    // Down Arrow, which moves no caret in the one-line box, moves the
    // selection; NVDA also says the box's line for the key, where
    // Verbatim says nothing for a caret key that moves nothing.
    for (key, said) in [
        ("downarrow", "Sound mixer options 2 of 22"),
        ("downarrow", "Themes and related settings 3 of 22"),
        ("uparrow", "Sound mixer options 2 of 22"),
    ] {
        scenario.send_keys(&[key]).expect("sends the key");
        scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
        scenario.speech().expect(&[said]);
    }
}
