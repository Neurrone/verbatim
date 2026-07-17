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
pub(crate) mod start_menu;
pub(crate) mod start_menu_repeat;
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
/// nowhere, so no menu item is ever focused or announced. The popup's own
/// announcement is the open signal: it announces as its client object —
/// the platform names menu popups "Context", spoken with the menu role —
/// emitted by the `MenuPopupStart` `WinEvent` the moment the menu opens
/// (NVDA's menu-start behavior), with the foreground-announce path
/// producing the identical node as its fallback.
pub(crate) fn open_verbatim_menu(scenario: &mut Scenario, timeout: Duration) {
    scenario
        .send_gesture("kb:verbatim+v")
        .expect("sends the Verbatim+V gesture");
    scenario
        .speech()
        .expect_in_order(&["Context", "menu"], timeout);
}
