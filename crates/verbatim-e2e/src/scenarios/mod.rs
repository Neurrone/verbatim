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
pub(crate) mod notepad_spelling_errors;
pub(crate) mod object_navigation_in_settings;
pub(crate) mod rapid_tabbing_in_settings;
pub(crate) mod settings_dialog_keys;
pub(crate) mod settings_system_page;
pub(crate) mod spelling_errors;
pub(crate) mod start_menu_search;
pub(crate) mod switch_to_onecore;
pub(crate) mod synth_host_crash_recovery;
pub(crate) mod system_information_tree;
pub(crate) mod terminal;
pub(crate) mod terminal_commands;
pub(crate) mod terminal_flood;
pub(crate) mod terminal_review_grid;
pub(crate) mod terminal_settings_page;
pub(crate) mod theme_panel;

/// Waits for Notepad's window, then its text area, then the text the text
/// area's announcement ends with, and returns that text once heard in full.
///
/// The text area is Windows 11 Notepad's UIA document, "Text editor
/// document", or classic Notepad's Win32 edit control, "Text Editor edit",
/// as GitHub's Windows Server runners have it; both hold. Either way the
/// announcement leaves the text's value out and is followed by the caret's
/// line, or the selection, as its own utterance (`docs/nvda/speech.md`,
/// "What an object with text says"): the next utterance after the text
/// area, which is what is returned. Waiting for it to be heard means the
/// caller's next key cannot cut it off.
pub(crate) fn expect_notepad_text(scenario: &mut Scenario, timeout: Duration) -> String {
    let text_area = scenario
        .speech()
        .expect_in_order_capturing(&["Notepad", "Text "], timeout);
    assert!(
        ["Text editor document", "Text Editor edit"].contains(&text_area.as_str()),
        "Notepad's text area was announced as {text_area:?}, not by its name and role alone"
    );
    scenario
        .speech()
        .expect_change_capturing(&text_area, timeout)
}

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
