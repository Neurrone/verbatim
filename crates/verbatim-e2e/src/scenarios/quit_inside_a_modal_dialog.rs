//! Verbatim quits while one of its own modal dialogs is open: the Speech
//! page's Select Synthesizer dialog is opened and left open, and the run's
//! quit, through the control plane as the tray menu's Exit would, must
//! end Verbatim with code 0 within the quit timeout. Shutdown then arrives
//! inside the dialog's modal loop, where ending only the main loop did
//! nothing, so Verbatim stayed up; it now ends the dialog and schedules the
//! main loop's exit. The run's own quit is the assertion
//! ([`Scenario::quit_verbatim`](crate::scenario::Scenario::quit_verbatim)).

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario);
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect(&["Synthesizer grouping", "Change... button Alt+h"]);
    scenario.send_keys(&["space"]).expect("sends space");
    scenario.speech().expect(&[
        "Select Synthesizer dialog",
        "Synthesizer: combo box eSpeak NG collapsed Alt+s",
    ]);
}
