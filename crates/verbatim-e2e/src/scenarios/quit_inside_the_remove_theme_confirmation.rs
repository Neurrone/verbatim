//! Verbatim quits while the Theme page's Remove Theme confirmation is open:
//! the Theme page walk makes a theme and presses Remove on it, and the
//! confirmation, a native task dialog with Yes and No and no Cancel, is
//! left open. The run's quit, through the control plane as the tray menu's
//! Exit would, must end Verbatim with code 0 within the quit timeout.
//! Shutdown then arrives inside the task dialog's modal loop, which
//! ignores `WM_CLOSE`, so Verbatim ends it by clicking No. The run's own
//! quit is the assertion
//! ([`Scenario::quit_verbatim`](crate::scenario::Scenario::quit_verbatim)).
//!
//! The theme the walk made is left in the themes folder beside the staged
//! Verbatim; the Theme page scenario's teardown deletes it.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::theme_panel::{setup, teardown};

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::theme_panel::to_the_remove_confirmation(scenario);
}
