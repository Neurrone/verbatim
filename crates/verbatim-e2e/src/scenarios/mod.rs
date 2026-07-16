//! The registered scenarios' actual setup, body, and teardown logic, one
//! module per scenario. [`crate::registry::SCENARIOS`] is what wires each of
//! these into a named, grouped [`crate::registry::ScenarioDef`]; the
//! `#[test]` wrappers under `crates/verbatim-e2e/tests/` call
//! [`crate::registry::run_named`] with the matching name, rather than
//! calling into these modules directly.

use std::time::Duration;

use crate::scenario::Scenario;

pub(crate) mod m1_exit_regression;
pub(crate) mod msinfo32;
pub(crate) mod multi_outpost_switch;
pub(crate) mod notepad_focus;
pub(crate) mod object_navigation;
pub(crate) mod tree_navigation;

/// Opens the Verbatim menu with Verbatim+V and waits for the popup to be
/// announced before returning, so the caller's very next arrow key lands
/// inside a menu that actually exists.
///
/// The wait is the synchronization a listening user performs naturally.
/// Root-caused live on a cold guest: the first menu popup of a session can
/// take over two seconds to appear (first-menu resource loading in the
/// GUI process — the foreground grab itself completed in under twenty
/// milliseconds), and an arrow key sent blind in that window lands
/// nowhere, so no menu item is ever focused or announced. The popup
/// window's own announcement is the open signal; the platform names menu
/// popup windows "Context", so that plus the window role is the stable
/// thing to wait for.
pub(crate) fn open_verbatim_menu(scenario: &mut Scenario, timeout: Duration) {
    scenario
        .send_gesture("kb:verbatim+v")
        .expect("sends the Verbatim+V gesture");
    scenario
        .speech()
        .expect_in_order(&["Context", "window"], timeout);
}
