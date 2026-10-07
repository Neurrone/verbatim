//! The settings dialog's keys (audit item 7, decided with Dickson on
//! 2026-10-06; `crates/verbatim-gui/src/keys.rs`): Enter on a focused
//! button activates that button, so Enter on Cancel cancels and Enter on
//! Apply applies and keeps the dialog open; Control+S applies and
//! Control+Tab and Control+Shift+Tab change category from any control,
//! the Speech page's own controls included, since one key hook on the
//! dialog sees every key. NVDA's own settings dialogs send every Enter to
//! OK; Verbatim's difference is recorded in `docs/parity.md`.
//!
//! The walk: open the Speech settings, change the rate, Tab to Cancel and
//! press Enter, reopen, and hear the rate unchanged; change it again, Tab
//! to Apply and press Enter, hear that focus stays on Apply, close with
//! Escape, reopen, and hear the applied rate. Then, on the rate slider,
//! change the rate and press Control+S, close with Escape, reopen, and hear
//! that rate kept; press Control+Tab on the slider and hear the category
//! list take focus on the next category, Theme, then Control+Tab there move
//! on to Terminal and wrap round to Speech; Tab into the page and press
//! Control+Shift+Tab on the Change button, and hear the category list
//! again, wrapped round to the last category, Terminal. None of these
//! category changes announces the dialog again, though its title names the
//! category: it is the same window, so the same node, as it is to NVDA,
//! which says nothing about the title either.
//!
//! The walk is fixed: the Speech page's controls in their Tab order, so
//! every Tab is asserted by the control it reaches. Every time the dialog
//! closes, the scenario waits for the desktop, where the focus returns, to
//! take the foreground and asserts its announcement.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// The Speech page's controls from the Change button to the rate slider,
/// in Tab order, as each announces itself.
const TO_RATE: [&str; 3] = [
    "Change... button Alt+h",
    "Voice combo box English (Great Britain) collapsed Alt+v",
    "Variant combo box Max collapsed Alt+a",
];

/// The controls after the rate slider, in Tab order.
const AFTER_RATE: [&str; 6] = [
    "Pitch slider 50 Alt+p",
    "Inflection slider 80 Alt+i",
    "Volume slider 100 Alt+o",
    "OK button",
    "Cancel button",
    "Apply button Alt+a",
];

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(heard);
}

/// Opens the Speech settings and Tabs to the rate slider, which must say
/// `rate`.
pub(crate) fn open_at_rate(scenario: &mut Scenario, rate: u32) {
    super::open_speech_settings(scenario);
    for control in TO_RATE {
        press(scenario, "tab", &[control]);
    }
    press(scenario, "tab", &[&format!("Rate slider {rate} Alt+r")]);
}

/// Tabs from the rate slider through `count` controls after it.
fn tab_past_rate(scenario: &mut Scenario, count: usize) {
    for control in &AFTER_RATE[..count] {
        press(scenario, "tab", &[control]);
    }
}

/// Presses `key`, which closes the dialog, and asserts the desktop, where
/// the focus returns.
pub(crate) fn close_with(scenario: &mut Scenario, key: &str) {
    scenario.send_keys(&[key]).expect("sends the key");
    super::expect_desktop(scenario);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // Enter on Cancel reverts the changed rate and closes the dialog. On
    // this slider the up arrow lowers the value.
    open_at_rate(scenario, 80);
    press(scenario, "uparrow", &["79"]);
    tab_past_rate(scenario, 5);
    close_with(scenario, "enter");

    // Enter on Apply applies the change and leaves the dialog open: the
    // next Shift+Tab moves back from Apply inside the dialog. Escape then
    // closes it without undoing what was applied.
    open_at_rate(scenario, 80);
    press(scenario, "uparrow", &["79"]);
    tab_past_rate(scenario, 6);
    scenario
        .send_keys(&["enter"])
        .expect("sends enter on Apply");
    press(scenario, "shift+tab", &["Cancel button"]);
    close_with(scenario, "escape");

    // Control+S on the rate slider, inside the Speech page, applies: the
    // Cancel that Escape then sends reverts to the applied rate, not past
    // it.
    open_at_rate(scenario, 79);
    press(scenario, "uparrow", &["78"]);
    scenario
        .send_keys(&["control+s"])
        .expect("sends control+s on the slider");
    close_with(scenario, "escape");

    // Control+Tab on the slider moves to the next category, Theme, and puts
    // focus on the category list; Control+Tab there cycles on through
    // Terminal and wraps round to Speech. Control+Shift+Tab from the Change
    // button moves to the previous category, wrapping to the last,
    // Terminal.
    open_at_rate(scenario, 78);
    press(
        scenario,
        "control+tab",
        &["Categories: list Alt+c", "Theme 2 of 3"],
    );
    press(scenario, "control+tab", &["Terminal 3 of 3"]);
    press(scenario, "control+tab", &["Speech 1 of 3"]);
    press(scenario, "tab", &["Change... button Alt+h"]);
    press(
        scenario,
        "control+shift+tab",
        &["Categories: list Alt+c", "Terminal 3 of 3"],
    );
    close_with(scenario, "escape");
}
