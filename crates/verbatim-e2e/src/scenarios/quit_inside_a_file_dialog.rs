//! Verbatim quits while the system's file dialog is open: the settings
//! dialog's Theme page is shown, Alt+X presses its Export button, and the
//! Save dialog that asks where to export the Default theme is left open.
//! The run's quit, through the control plane as the tray menu's Exit
//! would, must end Verbatim with code 0 within the quit timeout. Shutdown
//! then arrives inside the file dialog's modal loop, which Verbatim ends
//! with `WM_CLOSE`, as the dialog's close button does. The run's own quit
//! is the assertion
//! ([`Scenario::quit_verbatim`](crate::scenario::Scenario::quit_verbatim)).
//! Nothing is exported, so nothing is left behind.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario);
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect(&["Theme 2 of 3"]);
    scenario.send_keys(&["alt+x"]).expect("sends alt+x");
    scenario.speech().expect(&[
        "Export Default dialog",
        "File name: edit Alt+n",
        "default.zip",
    ]);
}
