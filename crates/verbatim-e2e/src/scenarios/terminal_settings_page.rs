//! The settings dialog's Terminal page (`phase6-design.md`, "M4: text,
//! editing, and terminals", Questions; `crates/verbatim-gui/src/terminal_panel.rs`):
//! a change applied there reaches Core, and the page shows a change Core
//! made itself.
//!
//! The walk: open the settings dialog and move to the Terminal category;
//! Tab to "Report new output", heard checked, as the harness's settings
//! have it; press Space and hear it not checked, then Control+S to apply,
//! and Escape, waiting for the dialog to close. Verbatim+5 then toggles
//! the setting in Core: it says "report new output on", which it says only
//! if the applied change had turned it off. That also restores the
//! setting. Reopen the Terminal page and hear "Report new output" checked
//! again, so the page opens on the value Verbatim+5 set. Tab to the two
//! line-limit sliders and hear each read the setting, 30, not 29: a
//! standard trackbar's MSAA value is its position as a percentage of its
//! range, 1 to 10,000 here, so the page gives each slider its position as its
//! value. Right Arrow then moves the second to 31, heard as 31, and Escape
//! closes the dialog without applying it.
//!
//! Every step asserts exactly what it says; each time the dialog closes,
//! the scenario waits for the desktop, where the focus returns, to take
//! the foreground and asserts its announcement.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// "Report new output", checked.
const REPORT_CHECKED: &str = "Report new output check box checked Alt+n";

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(heard);
}

/// Opens the settings dialog on the Terminal category and Tabs to "Report
/// new output", which says it is checked.
fn open_at_report_output(scenario: &mut Scenario) {
    super::open_speech_settings(scenario);
    press(scenario, "downarrow", &["Theme 2 of 3"]);
    press(scenario, "downarrow", &["Terminal 3 of 3"]);
    press(scenario, "tab", &[REPORT_CHECKED]);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // On by default, as the harness writes the settings. Off, and applied
    // with Control+S; Escape then closes the dialog without undoing what
    // was applied.
    open_at_report_output(scenario);
    press(scenario, "space", &["not checked"]);
    scenario
        .send_keys(&["control+s"])
        .expect("sends control+s on the check box");
    super::close_settings_to_desktop(scenario);

    // Verbatim+5 toggles Core's setting: "on" shows it was off, so the
    // applied change reached Core. It also turns the setting back on.
    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    scenario.speech().expect(&["report new output on"]);

    // The page opens on the value Verbatim+5 set. Each line limit reads
    // its setting, as does a change to it.
    open_at_report_output(scenario);
    press(scenario, "tab", &["Lines spoken in full: slider 30 Alt+l"]);
    press(scenario, "tab", &["Last lines to speak: slider 30 Alt+p"]);
    press(scenario, "rightarrow", &["31"]);
    super::close_settings_to_desktop(scenario);
}
