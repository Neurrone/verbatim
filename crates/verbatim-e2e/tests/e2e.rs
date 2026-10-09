//! The end-to-end suite's one test binary: a `#[test]` per scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`, each calling
//! `registry::run_named` with its own name, and the session check every
//! scenario depends on. A scenario's test has the scenario's name, so
//! `cargo test -p verbatim-e2e --test e2e -- --ignored --exact <name>
//! --test-threads=1` runs exactly that scenario.
//!
//! The scenarios share one binary rather than having one each because every
//! test binary links the whole harness with its own debug information:
//! a binary per scenario made the suite most of a build's size on disk
//! (`docs/tooling.md`, "Build disk use").
//!
//! Every test here is ignored, so a workspace test run, and `cargo xtask
//! ci`, lists them as ignored rather than running them; the end-to-end job
//! runs them with `--ignored`, and `cargo xtask demo` runs a demonstration
//! with `--include-ignored`.

use verbatim_e2e::AgentClient;

/// Defines one ignored `#[test]` per name, ignored for `$reason`, running
/// the registered scenario of that name.
macro_rules! scenario_tests {
    ($reason:literal: $($name:ident)*) => {
        $(
            #[test]
            #[ignore = $reason]
            fn $name() {
                verbatim_e2e::registry::run_named(stringify!($name));
            }
        )*
    };
}

scenario_tests! {
    "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)":
    menu_and_settings_dialog
    second_application_and_verbatim_menu
    rapid_tabbing_in_settings
    object_navigation_in_settings
    switch_to_onecore
    synth_host_crash_recovery
    outpost_crash_recovery
    lock_key_announcements
    explorer_folder_window
    settings_dialog_keys
    settings_system_page
    notepad_editing
    text_box_editing
    notepad_review_cursor
    text_box_review_cursor
    notepad_say_all
    notepad_word_selection
    text_box_word_selection
    notepad_typed_words
    text_box_typed_words
    notepad_review_words
    text_box_review_words
    text_box_say_all
    spelling_errors
    notepad_spelling_errors
    theme_panel
    terminal_settings_page
    windows_terminal_commands
    conhost_commands
    windows_terminal_spoken_password
    conhost_spoken_password
    windows_terminal_flood
    conhost_flood
    windows_terminal_control_flood
    conhost_control_flood
    windows_terminal_shift_flood
    conhost_shift_flood
    windows_terminal_line_key_flood
    conhost_line_key_flood
    windows_terminal_up_typing
    conhost_up_typing
    windows_terminal_same_flood
    conhost_same_flood
    windows_terminal_raised_flood
    conhost_raised_flood
    windows_terminal_redraw_limit
    conhost_redraw_limit
    windows_terminal_leave_flood
    conhost_leave_flood
    windows_terminal_close_tab
    conhost_wrapped_flood
    windows_terminal_history_flood
    conhost_history_flood
    windows_terminal_history_flood_during_group
    conhost_history_flood_during_group
    windows_terminal_scrollback_overflow
    conhost_scrollback_overflow
    windows_terminal_scrollback_overflow_during_group
    conhost_scrollback_overflow_during_group
    windows_terminal_editing
    conhost_editing
    windows_terminal_review_grid
    conhost_review_grid
    windows_terminal_progress
    conhost_progress
    windows_terminal_typing
    conhost_typing
    windows_terminal_long_lines
    conhost_long_lines
    windows_terminal_full_screen
    conhost_full_screen
    windows_terminal_marker_list
    conhost_marker_list
    windows_terminal_redrawn_list
    conhost_redrawn_list
    windows_terminal_pager
    conhost_pager
    windows_terminal_review_output
    conhost_review_output
    windows_terminal_footer_flood
    conhost_footer_flood
    conhost_footer_overflow
    windows_terminal_short_output
    conhost_short_output
    windows_terminal_two_windows
    conhost_two_windows
    windows_terminal_tabs
    object_navigation_over_uia
    system_information_tree
}

// The demonstrations are ignored too, so the suite never runs them; `cargo
// xtask demo` runs one with `--include-ignored`, and the suite's commands
// also deselect them with `--skip demo_`.
scenario_tests! {
    "a demonstration, recorded by cargo xtask demo":
    demo_notepad_editing
    demo_review_cursor
    demo_say_all
    demo_terminal_session
    demo_settings_dialog_keys
}

/// The agent reports an interactive session on the default desktop: the
/// precondition every scenario depends on, not itself a scenario. A screen
/// reader driven from a non-interactive session (session 0, `WinRM`,
/// PowerShell Direct) can never work, and a locked machine's input desktop
/// is the secure desktop, where the suite does nothing useful, so this is
/// the first thing worth checking when the suite behaves strangely. It
/// fails when no agent is named.
#[test]
#[ignore = "live: needs a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn agent_reports_an_interactive_window_station() {
    let endpoint = verbatim_e2e::endpoint().unwrap_or_else(|| {
        panic!(
            "{} is not set: a live test needs a running agent",
            verbatim_e2e::ENDPOINT_ENV
        )
    });
    let mut agent =
        AgentClient::connect(&endpoint).expect("connects to the agent and completes Hello");
    let info = agent.session_info().expect("agent answers SessionInfo");
    assert!(
        info.interactive_window_station,
        "agent's window station is not interactive (session {}); the E2E suite cannot drive a screen reader from here",
        info.session_id
    );
    assert_eq!(
        info.input_desktop_name.as_deref(),
        Some("Default"),
        "the input desktop is not the default desktop: the machine is locked, or the secure desktop is up"
    );
}
