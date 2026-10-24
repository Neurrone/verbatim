//! The Windows+X menu, opened from the desktop and closed with Escape:
//! the popup window, cut off by the menu's first item, then the item, and
//! on Escape nothing, as NVDA says it (captured with `cargo xtask nvda
//! compare quick_link_menu` against the pinned build on 2026-10-24).
//!
//! NVDA's File Explorer app module leaves unsaid a focus on the menu's
//! host, `LauncherTipWnd`, one of the shell's staging windows
//! (`docs/parity/windows-and-processes.md`, "The shell's staging
//! windows"). On the Windows 11 build this was written on, the host took
//! neither the focus nor the foreground in any run, so this scenario
//! checks the menu's speech, not that rule, which Core's unit test pins.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::{cut_off, heard};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    scenario
        .send_keys(&["leftwindows+x"])
        .expect("sends Windows+X");
    // The menu's first item, entering a menu, cuts the popup window off,
    // as NVDA cuts it.
    scenario.speech().expect_sequence(&[
        cut_off("Popup window"),
        heard("Installed apps Alt, p 1 of 17"),
    ]);
    // The desktop's list item takes the focus back, which NVDA does not
    // say again: the scenario's end asserts that nothing was said.
    scenario.send_keys(&["escape"]).expect("sends Escape");
}
