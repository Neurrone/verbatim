//! The registered scenarios' actual setup, body, and teardown logic, one
//! module per scenario. [`crate::registry::SCENARIOS`] is what wires each of
//! these into a named, grouped [`crate::registry::ScenarioDef`]; the
//! `#[test]` wrappers under `crates/verbatim-e2e/tests/` call
//! [`crate::registry::run_named`] with the matching name, rather than
//! calling into these modules directly.
//!
//! Every assertion is exact ([`crate::speech`]): every utterance, in order,
//! with nothing else in between, and each scenario's body is followed by
//! the registry's assertion that nothing more was said.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

pub(crate) mod demo_notepad_editing;
pub(crate) mod demo_review_cursor;
pub(crate) mod demo_say_all;
pub(crate) mod demo_settings_dialog_keys;
pub(crate) mod demo_terminal_session;
pub(crate) mod edit_control_say_all;
pub(crate) mod explorer_folder_window;
pub(crate) mod lock_key_announcements;
pub(crate) mod menu_and_settings_dialog;
pub(crate) mod notepad_editing;
pub(crate) mod notepad_review_cursor;
pub(crate) mod notepad_review_words;
pub(crate) mod notepad_say_all;
pub(crate) mod notepad_spelling_errors;
pub(crate) mod notepad_typed_words;
pub(crate) mod notepad_word_selection;
pub(crate) mod object_navigation_in_settings;
pub(crate) mod outpost_crash_recovery;
pub(crate) mod rapid_tabbing_in_settings;
pub(crate) mod second_application_and_verbatim_menu;
pub(crate) mod settings_dialog_keys;
pub(crate) mod settings_system_page;
pub(crate) mod spelling_errors;
pub(crate) mod switch_to_onecore;
pub(crate) mod synth_host_crash_recovery;
pub(crate) mod system_information_tree;
pub(crate) mod terminal;
pub(crate) mod terminal_commands;
pub(crate) mod terminal_editing;
pub(crate) mod terminal_flood;
pub(crate) mod terminal_review_grid;
pub(crate) mod terminal_settings_page;
pub(crate) mod text_box;
pub(crate) mod theme_panel;

/// What the scenario says about the desktop, which has the focus after the
/// harness minimized every window and once a dialog of Verbatim's closes:
/// the window, the desktop's list, and its focused item, which the harness
/// reads independently of Verbatim ([`Scenario::desktop_speech`]) once the
/// desktop is in the foreground. When a dialog of Verbatim's closes,
/// Verbatim's own window titled "Verbatim" can be in front for a moment
/// first.
pub(crate) fn expect_desktop(scenario: &mut Scenario) {
    scenario
        .wait_for_window_in_front(
            crate::scenario::DESKTOP_TITLE,
            crate::scenario::WINDOW_TIMEOUT,
        )
        .expect("the desktop takes the foreground");
    let desktop = scenario
        .desktop_speech()
        .expect("reads the desktop's focus");
    let desktop: Vec<&str> = desktop.iter().map(String::as_str).collect();
    scenario.speech().expect(&desktop);
}

/// No state: a setup for a scenario that needs nothing beyond Verbatim.
#[expect(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's signature"
)]
pub(crate) fn no_setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::None)
}

/// Nothing to restore: cleanup closes what the scenario opened.
pub(crate) fn no_teardown(_scenario: &mut Scenario, _state: ScenarioState) {}

/// How Verbatim speaks `character` on its own, as typed-character echo
/// says it: a letter or digit as itself, and the punctuation the
/// scenarios type by its name in the character table
/// (`crates/verbatim-i18n/i18n/en/verbatim.ftl`).
pub(crate) fn character_name(character: char) -> String {
    match character {
        ' ' => "space".to_owned(),
        '.' => "dot".to_owned(),
        ',' => "comma".to_owned(),
        '-' => "dash".to_owned(),
        '\\' => "backslash".to_owned(),
        other => other.to_string(),
    }
}

/// The window title Windows 11 Notepad shows for the harness document
/// `name`.
pub(crate) fn notepad_title(name: &str) -> String {
    format!("{}.txt - Notepad", harness_marker(name))
}

/// Asserts what Windows 11 Notepad says as the harness document `name`,
/// opened ([`Scenario::open_document_with`]), is brought forward with its
/// caret on `line`: the window, named with the document, the text area,
/// and the line at the caret.
pub(crate) fn expect_notepad_opened(scenario: &mut Scenario, name: &str, line: &str) {
    expect_notepad_returned(scenario, name, line);
}

/// Asserts what Windows 11 Notepad says as its window comes back to the
/// foreground with its caret on `line`.
pub(crate) fn expect_notepad_returned(scenario: &mut Scenario, name: &str, line: &str) {
    let title = notepad_title(name);
    scenario
        .speech()
        .expect(&[&title, "Text editor document", line]);
}

/// Opens the Verbatim menu with Verbatim+V and asserts its announcement:
/// the menu's owner window, Verbatim's frame, named "Verbatim", then the
/// popup, which the platform names "Context", with the menu role.
pub(crate) fn open_verbatim_menu(scenario: &mut Scenario) {
    scenario
        .send_gesture("kb:verbatim+v")
        .expect("sends the Verbatim+V gesture");
    scenario.speech().expect(&["Verbatim", "Context menu"]);
}

/// Opens Verbatim's settings dialog from its menu and asserts that focus
/// settles on the selected category, "Speech".
pub(crate) fn open_speech_settings(scenario: &mut Scenario) {
    open_verbatim_menu(scenario);
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect(&["Settings... s"]);
    scenario.send_keys(&["enter"]).expect("sends enter");
    scenario.speech().expect(&[
        "Verbatim Settings: Speech dialog",
        "Categories: list Alt+c",
        "Speech 1 of 3",
    ]);
}

/// Opens `mockapp`, the scripted application staged beside Verbatim, as a
/// window titled with [`harness_marker`] of `name` holding one focused text
/// box whose one line is `line`, read over UI Automation, and waits for it
/// to take the foreground. It is the same on every machine. Returns its
/// process id.
///
/// # Errors
///
/// Returns an error if the fixture cannot be written, `mockapp` cannot be
/// started, or its window does not take the foreground.
pub(crate) fn open_text_box_app(
    scenario: &mut Scenario,
    name: &str,
    line: &str,
) -> io::Result<u32> {
    let directory = scenario.run_directory().to_owned();
    let title = harness_marker(name);
    let fixture = scenario.harness_file(name, "json");
    let contents = format!(
        r#"{{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    {{
      "id": "text",
      "role": "editable_text",
      "name": "Text",
      "states": ["focusable", "focused"],
      "text": "{line}\n"
    }}
  ]
}}
"#
    );
    scenario.write_agent_file(&fixture, contents.as_bytes())?;
    let args = [
        "--fixture",
        &fixture,
        "--backend",
        "uia",
        "--title",
        &title,
        "--show",
    ]
    .map(str::to_owned);
    let window =
        scenario.launch_titled(&format!(r"{directory}\mockapp.exe"), &args, &title, true)?;
    Ok(window.pid)
}

/// What [`open_text_box_app`]'s window says as it comes to the foreground:
/// the window, the text box, and its line.
pub(crate) fn expect_text_box_app(scenario: &mut Scenario, name: &str, line: &str) {
    let window = format!("{} window", harness_marker(name));
    scenario.speech().expect(&[&window, "Text edit", line]);
}

/// The process of the outpost watching the application `target`, as
/// Verbatim's status reports it.
pub(crate) fn outpost_of(scenario: &mut Scenario, target: u32) -> u32 {
    let status = scenario.status().expect("Verbatim answers Status");
    let outposts: Vec<_> = status
        .outposts
        .iter()
        .filter(|outpost| outpost.target_pid == verbatim_model::Pid(target))
        .collect();
    let [outpost] = outposts.as_slice() else {
        panic!("expected one outpost watching {target}, found {outposts:?}");
    };
    outpost
        .outpost_pid
        .unwrap_or_else(|| panic!("the outpost watching {target} has no process: {outpost:?}"))
        .0
}

/// Closes Verbatim's settings dialog with Escape and asserts that the
/// desktop, where the focus returns, is announced.
pub(crate) fn close_settings_to_desktop(scenario: &mut Scenario) {
    scenario.send_keys(&["escape"]).expect("sends escape");
    expect_desktop(scenario);
}
