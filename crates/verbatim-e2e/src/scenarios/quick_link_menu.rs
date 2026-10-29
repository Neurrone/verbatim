//! The Windows+X menu, opened from the desktop and closed with Escape:
//! the popup window, cut off by the menu's first item, then the item, and
//! on Escape the taskbar's Start button, which File Explorer gives the
//! focus, as NVDA says it (captured with `cargo xtask nvda compare
//! quick_link_menu` against the pinned build on 2026-10-30, five runs from
//! the desktop).
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

pub(crate) use super::no_setup as setup;

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
    // Escape gives the focus to the taskbar's Start button, which NVDA
    // says as it says any focus.
    scenario.send_keys(&["escape"]).expect("sends Escape");
    scenario
        .speech()
        .expect_sequence(&[heard("Start toggle button not pressed")]);
}

/// Brings the desktop back to the foreground: Escape closes the menu but
/// leaves the taskbar in front, its Start button focused and its tooltip
/// shown, which cleanup would otherwise find left open.
///
/// # Panics
///
/// Panics if the desktop does not take the foreground.
pub(crate) fn teardown(scenario: &mut Scenario, _state: ScenarioState) {
    scenario
        .return_to_desktop()
        .expect("brings the desktop back to the foreground");
}
