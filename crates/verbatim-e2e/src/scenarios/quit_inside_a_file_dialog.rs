//! Verbatim quits while the system's file dialog is open: on the settings
//! dialog's Theme page, with the Default theme selected (so Rename and
//! Export are disabled and Tab skips them), Tab reaches the Import button
//! and Enter opens the "Import a Theme" file dialog, which is left open.
//! The run's quit, through the control plane as the tray menu's Exit
//! would, must end Verbatim with code 0 within the quit timeout. Shutdown
//! then arrives inside the file dialog's modal loop, which Verbatim ends
//! with `WM_CLOSE`, as the dialog's close button does. The run's own quit
//! is the assertion
//! ([`Scenario::quit_verbatim`](crate::scenario::Scenario::quit_verbatim)).
//! Nothing is imported, so nothing is left behind. The speech is NVDA's,
//! captured at the pin on 2026-11-19.

use super::press;
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario);
    press(scenario, "downarrow", &["Theme 2 of 3"]);
    let tabs: [&[&str]; 9] = [
        &["Theme: combo box Default collapsed Alt+t"],
        &[
            "Description: edit read only multi line Alt+d",
            "Matches NVDA's speech and sounds.",
        ],
        &["Sound volume: slider 100 Alt+u"],
        &["Play sounds during say all check box checked Alt+l"],
        &["Also speak indications that play a sound check box not checked Alt+s"],
        &["Find: edit Alt+f", "blank"],
        &["Indications: tree view", "level 0 Roles collapsed 1 of 6"],
        &["New theme based on this... button Alt+n"],
        &["Import... button"],
    ];
    for heard in tabs {
        press(scenario, "tab", heard);
    }
    press(
        scenario,
        "enter",
        &[
            "Import a Theme dialog File name:",
            "File name: combo box collapsed",
            "edit Alt+n",
            "blank",
        ],
    );
}
