//! The registered scenarios' actual setup, body, and teardown logic, one
//! module per scenario. [`crate::registry::SCENARIOS`] is what wires each of
//! these into a named, grouped [`crate::registry::ScenarioDef`]; the
//! `#[test]` wrappers under `crates/verbatim-e2e/tests/` call
//! [`crate::registry::run_named`] with the matching name, rather than
//! calling into these modules directly.

use std::time::Duration;

use crate::scenario::Scenario;

pub(crate) mod explorer_folder_window;
pub(crate) mod lock_key_announcements;
pub(crate) mod menu_and_settings_dialog;
pub(crate) mod notepad_and_verbatim_menu;
pub(crate) mod notepad_editing;
pub(crate) mod notepad_review_cursor;
pub(crate) mod notepad_say_all;
pub(crate) mod object_navigation_in_settings;
pub(crate) mod rapid_tabbing_in_settings;
pub(crate) mod settings_dialog_keys;
pub(crate) mod settings_system_page;
pub(crate) mod start_menu_search;
pub(crate) mod switch_to_onecore;
pub(crate) mod synth_host_crash_recovery;
pub(crate) mod system_information_tree;

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

/// Opens Verbatim's settings dialog from its menu and waits until focus has
/// settled on the selected category item, "Speech".
///
/// Focus settles after an intermediate step: the dialog announces the
/// category list with its selected item, and focus then lands on that item.
/// Waiting for the item as the settled focus lets the focus sequence finish
/// before the caller's next keys run.
pub(crate) fn open_speech_settings(scenario: &mut Scenario, timeout: Duration) {
    open_verbatim_menu(scenario, timeout);
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect_in_order(&["Settings..."], timeout);
    scenario.send_keys(&["enter"]).expect("sends enter");
    scenario
        .speech()
        .expect_in_order(&["Categories: list", "Speech"], timeout);
}
