//! Verbatim quits while its own menu is popped: Verbatim+V opens the menu,
//! which is left open, and the run's quit, through the control plane as
//! the tray menu's Exit would, must end Verbatim with code 0 within the
//! quit timeout. Shutdown then arrives inside the menu's native modal loop,
//! which nothing ended, so Verbatim stayed up; it now ends the menu and
//! schedules the main loop's exit. The run's own quit is the assertion
//! ([`Scenario::quit_verbatim`](crate::scenario::Scenario::quit_verbatim)).

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_verbatim_menu(scenario);
}
