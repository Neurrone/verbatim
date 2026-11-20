//! The settings dialog's Theme page (milestone M4 item 8,
//! `books/dev/src/reference/crates/verbatim-gui.md`, the Theme page): the page is read, the
//! indications tree is walked through an
//! entry of every category, and the button role is changed to a sound,
//! which Verbatim then plays in place of the word "button".
//!
//! The walk: open the settings dialog and move to the Theme category; Tab
//! through the page's controls, each asserted, to the indications tree,
//! whose first category is selected; in each category, expand it, hear its
//! first indication with its setting, and go back to the next category;
//! expanding the first says how many indications it holds. Then type
//! "button" in the find field, Tab into the filtered tree, and select
//! "button: speech". Set "Report as" to sound: the default theme cannot be
//! changed, so Verbatim asks for a new theme's name, and Enter accepts the
//! one it offers. Choose a sound, and Tab to the Preview button, which is
//! now announced with the sound in place of its role ("sound: role-button"
//! in the speech stream); press it and hear the sample. Reset the
//! indication, which is spoken again, Tab to the Remove button and remove
//! the new theme, hearing the confirmation's question, and close the
//! dialog.
//!
//! The new theme is made in the themes folder beside the staged Verbatim.
//! The walk removes it itself; the teardown, which runs whether the walk
//! passed or failed, deletes any theme folder the scenario left there and
//! fails if it cannot, so a later run never starts with an extra theme.

use super::press;
use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Each category of the tree, the first indication in it as the default
/// theme reports it, the category's own announcement expanded, and how
/// many indications it holds as its expansion says it.
const CATEGORIES: [(&str, &str, &str, &str); 6] = [
    (
        "Roles collapsed 1 of 6 level 0",
        "level 1 window: speech 1 of 71",
        "level 0 Roles expanded 1 of 6",
        "71 items",
    ),
    (
        "States collapsed 2 of 6 level 0",
        "level 1 selected: speech 1 of 19",
        "level 0 States expanded 2 of 6",
        "19 items",
    ),
    // Five, with the cell coordinates indication NVDA's "Cell coordinates"
    // option matches (`docs/parity/focus-announcement.md`, "Table cell
    // coordinates").
    (
        "Properties collapsed 3 of 6 level 0",
        "level 1 description: speech 1 of 5",
        "level 0 Properties expanded 3 of 6",
        "5 items",
    ),
    (
        "Text formatting collapsed 4 of 6 level 0",
        "level 1 spelling error: speech and sound (textError.wav) 1 of 12",
        "level 0 Text formatting expanded 4 of 6",
        "12 items",
    ),
    (
        "Structure collapsed 5 of 6 level 0",
        "level 1 blank: speech 1 of 2",
        "level 0 Structure expanded 5 of 6",
        "2 items",
    ),
    (
        "Events collapsed 6 of 6 level 0",
        "level 1 application not responding: sound (tone, 220 hertz, 150 milliseconds) 1 of 9",
        "",
        "9 items",
    ),
];

/// The themes folder beside the staged Verbatim.
fn themes_folder(scenario: &Scenario) -> String {
    format!(r"{}\themes", scenario.run_directory())
}

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let folder = themes_folder(scenario);
    Ok(ScenarioState::Names(scenario.list_agent_folders(&folder)?))
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    to_the_remove_confirmation(scenario);
    press(
        scenario,
        "y",
        &[
            "Verbatim Settings: Theme dialog",
            "Theme: combo box Default collapsed Alt+t",
        ],
    );
    super::close_settings_to_desktop(scenario);
}

/// The walk up to the Remove Theme confirmation, which is left open, its
/// question and focused button asserted: from opening the settings dialog
/// to pressing Remove on the theme the walk made.
#[expect(
    clippy::too_many_lines,
    reason = "one walk of the page, in order, so a failure's line says which step"
)]
pub(crate) fn to_the_remove_confirmation(scenario: &mut Scenario) {
    super::open_speech_settings(scenario);

    // The Theme category, after Speech, and its controls.
    press(scenario, "downarrow", &["Theme 2 of 3"]);
    press(
        scenario,
        "tab",
        &["Theme: combo box Default collapsed Alt+t"],
    );
    press(
        scenario,
        "tab",
        &[
            "Description: edit read only multi line Alt+d",
            "Matches NVDA's speech and sounds.",
        ],
    );
    press(scenario, "tab", &["Sound volume: slider 100 Alt+u"]);
    press(
        scenario,
        "tab",
        &["Play sounds during say all check box checked Alt+l"],
    );
    press(
        scenario,
        "tab",
        &["Also speak indications that play a sound check box not checked Alt+s"],
    );
    press(scenario, "tab", &["Find: edit Alt+f", "blank"]);
    press(
        scenario,
        "tab",
        &["Indications: tree view", "level 0 Roles collapsed 1 of 6"],
    );

    // Expanding a category says how many indications it holds, after
    // "expanded", as NVDA says it for a Win32 tree view item. Each
    // category is expanded to hear its first indication, then left by
    // going back to it, collapsing it, and moving down to the next.
    //
    // Each key is its own press: sent in one batch, a key lands within a
    // millisecond of the state change the one before it raised, which no
    // one typing does, and Verbatim's cut for it then reaches the change's
    // speech (NVDA, pumping non-focus events up to 10 ms late, cuts first).
    // The speech is NVDA's for one key at a time in a Win32 tree view
    // (captured 2026-11-19).
    for (index, (_, first, expanded, count)) in CATEGORIES.iter().enumerate() {
        press(scenario, "rightarrow", &["expanded", count]);
        press(scenario, "downarrow", &[first]);
        if let Some((next, _, _, _)) = CATEGORIES.get(index + 1) {
            press(scenario, "leftarrow", &[expanded]);
            press(scenario, "leftarrow", &["collapsed"]);
            press(scenario, "downarrow", &[next]);
        }
    }

    // The find field narrows the tree to the buttons; the first category
    // is selected again, expanded, and its first match is the button role.
    press(scenario, "shift+tab", &["Find: edit Alt+f", "blank"]);
    for letter in ["b", "u", "t", "t", "o", "n"] {
        press(scenario, letter, &[letter]);
    }
    press(
        scenario,
        "tab",
        &["Indications: tree view", "level 0 Roles expanded 1 of 1"],
    );
    press(scenario, "downarrow", &["level 1 button: speech 1 of 9"]);

    // Report as sound: the built-in theme asks for a new theme to make the
    // change in, offering a name, which Enter accepts.
    press(
        scenario,
        "tab",
        &["Report as: combo box speech collapsed Alt+r"],
    );
    press(
        scenario,
        "downarrow",
        &[
            "New Theme dialog",
            "Default is built in and cannot be changed. Name of the new theme, based on it, to make the change in: edit",
            "Default copy",
        ],
    );
    press(
        scenario,
        "enter",
        &[
            "Verbatim Settings: Theme dialog",
            "Report as: combo box sound collapsed Alt+r",
        ],
    );

    // A sound for it, played as it is chosen. The buttons are now
    // announced with the sound in place of their role, and Preview's
    // sample plays it too.
    press(scenario, "tab", &["Sound: combo box none collapsed Alt+o"]);
    press(scenario, "downarrow", &["browseMode.wav"]);
    press(scenario, "tab", &["Preview sound: role-button Alt+p"]);
    press(scenario, "enter", &["Sample sound: role-button"]);
    press(scenario, "tab", &["Reset sound: role-button Alt+e"]);

    // Reset speaks the role again; the focus moves to the tree, whose item
    // says so.
    press(
        scenario,
        "enter",
        &["Indications: tree view", "button: speech 1 of 9 level 1"],
    );

    // On to the Remove button, past the indication's other controls and
    // the theme's buttons. The new theme is removed, which selects the
    // default theme again. The confirmation is announced by its title and
    // its question, as NVDA reads a dialog's own text on entering it, and
    // then its focused button. The settings dialog, disabled as the
    // confirmation opens while the focus is still on Remove, says nothing,
    // as NVDA says nothing (`docs/parity/focus-announcement.md`, "A top-level window's state
    // change").
    for control in [
        "Report as: combo box speech collapsed Alt+r",
        "Words: edit Alt+w",
    ] {
        press(scenario, "tab", &[control]);
    }
    scenario.speech().expect(&["blank"]);
    for control in [
        "Voice: combo box default collapsed Alt+v",
        "Preview button Alt+p",
        "New theme based on this... button Alt+n",
        "Rename... button Alt+m",
        "Import... button",
        "Export... button Alt+x",
        "Remove button",
    ] {
        press(scenario, "tab", &[control]);
    }
    press(
        scenario,
        "enter",
        &[
            "Remove Theme dialog Remove the theme Default copy? This cannot be undone.",
            "No button Alt+N",
        ],
    );
}

/// Deletes every theme folder that was not there when the scenario began,
/// whether the walk passed or failed.
///
/// # Panics
///
/// Panics if the themes folder cannot be listed or a folder cannot be
/// deleted.
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    let ScenarioState::Names(before) = state else {
        panic!("setup lists the theme folders");
    };
    let folder = themes_folder(scenario);
    for name in scenario
        .list_agent_folders(&folder)
        .expect("lists the theme folders")
    {
        if !before.contains(&name) {
            scenario
                .delete_agent_folder(&format!(r"{folder}\{name}"))
                .expect("deletes the theme the scenario made");
        }
    }
}
