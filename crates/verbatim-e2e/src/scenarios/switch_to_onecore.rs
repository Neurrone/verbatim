//! Switching synthesizer still works (decisions D17 and D18): from eSpeak
//! NG, the default, to Windows `OneCore` voices through the Speech page's
//! Select Synthesizer dialog, and back. Each synthesizer runs in its own
//! host process, so the switch ends one host and starts another; every
//! announcement after it must still be heard in full.
//!
//! After the switch the Speech page's voice is Microsoft David, the voice
//! `OneCore` starts with, and Verbatim's status names `OneCore` as the
//! active synthesizer, which the text alone could not show, since either
//! synthesizer speaks the same words. Switching back leaves eSpeak NG
//! active with its English (Great Britain) voice; the dialog is then
//! closed, and the fixed settings are written afresh before the next run.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(heard);
}

/// Asserts the synthesizer Verbatim's status names as active.
fn expect_active(scenario: &mut Scenario, synthesizer: &str) {
    let status = scenario.status().expect("Verbatim answers Status");
    assert_eq!(
        status.active_synth.as_deref(),
        Some(synthesizer),
        "the active synthesizer"
    );
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario);
    expect_active(scenario, "espeak");
    press(scenario, "tab", &["Change... button Alt+h"]);
    press(
        scenario,
        "space",
        &[
            "Select Synthesizer dialog",
            "Synthesizer: combo box eSpeak NG collapsed Alt+s",
        ],
    );
    press(scenario, "downarrow", &["Windows OneCore voices"]);
    press(
        scenario,
        "enter",
        &["Verbatim Settings: Speech dialog", "Change... button Alt+h"],
    );
    press(
        scenario,
        "tab",
        &["Voice combo box Microsoft David collapsed Alt+v"],
    );
    expect_active(scenario, "onecore");

    press(scenario, "shift+tab", &["Change... button Alt+h"]);
    press(
        scenario,
        "space",
        &[
            "Select Synthesizer dialog",
            "Synthesizer: combo box Windows OneCore voices collapsed Alt+s",
        ],
    );
    press(scenario, "uparrow", &["eSpeak NG"]);
    press(
        scenario,
        "enter",
        &["Verbatim Settings: Speech dialog", "Change... button Alt+h"],
    );
    press(
        scenario,
        "tab",
        &["Voice combo box English (Great Britain) collapsed Alt+v"],
    );
    expect_active(scenario, "espeak");
    super::close_settings_to_desktop(scenario);
}
