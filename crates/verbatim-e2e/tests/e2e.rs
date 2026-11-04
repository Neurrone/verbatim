//! The end-to-end suite's one test binary: a test per scenario registered
//! in `crates/verbatim-e2e/src/registry.rs`, each calling
//! `registry::run_named` with its own name, and the session check every
//! scenario depends on. A scenario's test has the scenario's name, so
//! `cargo test -p verbatim-e2e --test e2e -- --ignored --exact <name>
//! --test-threads=1` runs exactly that scenario.
//!
//! The scenarios share one binary rather than having one each because every
//! test binary links the whole harness with its own debug information:
//! a binary per scenario made the suite most of a build's size on disk
//! (`docs/tooling/setup.md`, "Build disk use").
//!
//! Every test here is ignored, so a workspace test run, and `cargo xtask
//! ci`, lists them as ignored rather than running them; the end-to-end job
//! runs them with `--ignored`, and `cargo xtask demo` runs a demonstration
//! with `--include-ignored`.
//!
//! The binary has its own `main` (`harness = false`, with `libtest-mimic`
//! taking libtest's command line), so that once every test has run it
//! prints the end-of-run summary (`verbatim_e2e::run_summary`), which
//! libtest's harness has no place for. It keeps libtest's behaviour where
//! `libtest-mimic` differs: any number of filters
//! (`verbatim_e2e::command_line`), the tests run in the order of their
//! names, each on a thread of its own, and each failure's whole report in
//! the `failures:` section at the end.

use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Instant;

use libtest_mimic::{Arguments, Failed, Trial};
use verbatim_e2e::AgentClient;
use verbatim_e2e::command_line;
use verbatim_e2e::run_summary::RunSummary;

/// The names of the scenarios listed, each of whose tests is ignored for
/// the reason given first.
macro_rules! scenario_tests {
    ($reason:literal: $($name:ident)*) => {
        &[$(stringify!($name)),*]
    };
}

// The disabled scenarios (`registry::DISABLED`, each with its reason) have
// no test here.
const LIVE: &[&str] = scenario_tests! {
    "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling/e2e.md)":
    menu_and_settings_dialog
    second_application_and_verbatim_menu
    rapid_tabbing_in_settings
    object_navigation_in_settings
    switch_to_onecore
    crash_recovery
    verbatim_lifecycle
    speak_time_and_date
    alt_tab_switcher
    return_after_retirement
    return_from_a_held_application
    lock_key_announcements
    quick_link_menu
    explorer_folder_window
    settings_dialog_keys
    settings_search_suggestions
    settings_search_suggestions_reentered
    settings_system_page
    settings_combo_box_expanded
    notepad_editing
    text_box_editing
    notepad_selection_keys
    text_box_selection_keys
    notepad_caret_keys
    text_box_caret_keys
    notepad_document_keys
    text_box_document_keys
    notepad_page_keys
    text_box_page_keys
    notepad_paragraph_keys
    text_box_paragraph_keys
    notepad_select_all_count
    text_box_select_all_count
    text_box_password
    wpf_password
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
    windows_terminal_full_history_flood
    conhost_full_history_flood
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
    windows_terminal_redraw_limit
    conhost_redraw_limit
    conhost_raised_limit
    windows_terminal_leave_flood
    conhost_leave_flood
    windows_terminal_close_tab
    windows_terminal_history_flood_during_group
    conhost_history_flood_during_group
    windows_terminal_clear_flood
    windows_terminal_cleared_flood
    conhost_fresh_flood
    windows_terminal_scrollback_overflow
    conhost_scrollback_overflow
    windows_terminal_scrollback_overflow_during_group
    conhost_scrollback_overflow_during_group
    windows_terminal_editing
    conhost_editing
    windows_terminal_review_grid
    conhost_review_grid
    windows_terminal_review_padding
    conhost_review_padding
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
    windows_terminal_pager_history
    conhost_pager_history
    windows_terminal_review_output
    conhost_review_output
    conhost_footer_overflow
    windows_terminal_status_bar
    conhost_status_bar
    windows_terminal_command_marks
    conhost_command_marks
    windows_terminal_short_output
    conhost_short_output
    windows_terminal_two_windows
    conhost_two_windows
    windows_terminal_tabs
    object_navigation_over_uia
    system_information_tree
};

// The demonstrations are ignored too, so the suite never runs them; `cargo
// xtask demo` runs one with `--include-ignored`, and the suite's commands
// also deselect them with `--skip demo_`.
const DEMONSTRATIONS: &[&str] = scenario_tests! {
    "a demonstration, recorded by cargo xtask demo":
    demo_notepad_editing
    demo_review_cursor
    demo_say_all
    demo_terminal_session
    demo_settings_dialog_keys
};

// The measure set, the measurement workloads of
// `docs/design/communication.md` section 11: ignored, run by name or with
// `cargo xtask vm test --group measure`, never by the suite, whose runs
// deselect them with `--skip measure_` (docs/tooling/e2e.md).
const MEASURE_SET: &[&str] = scenario_tests! {
    "a measurement workload, run by name (docs/tooling/e2e.md)":
    measure_focus_windows_terminal
    measure_focus_windows_terminal_tabs
    measure_focus_text_box
    measure_focus_new_application
    measure_keys_text_box
    measure_keys_notepad
    measure_keys_conhost
    measure_keys_windows_terminal
    measure_stale_caret_text_box
    measure_stale_caret_notepad
    measure_stale_caret_conhost
    measure_held_repeat_text_box
    measure_speech_real
};

/// The agent reports an interactive session on the default desktop: the
/// precondition every scenario depends on, not itself a scenario. A screen
/// reader driven from a non-interactive session (session 0, `WinRM`,
/// PowerShell Direct) can never work, and a locked machine's input desktop
/// is the secure desktop, where the suite does nothing useful, so this is
/// the first thing worth checking when the suite behaves strangely. It
/// fails when no agent is named.
/// It is ignored like the scenarios: it needs a running agent, and the
/// end-to-end job runs it with `--ignored` (docs/tooling/e2e.md).
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

/// A scenario's test: runs it, records its result and seconds in
/// `summary`, and fails as it does.
fn scenario_trial(name: &'static str, summary: &Arc<Mutex<RunSummary>>) -> Trial {
    let summary = Arc::clone(summary);
    Trial::test(name, move || {
        let started = Instant::now();
        let outcome = on_test_thread(name, move || verbatim_e2e::registry::run_named(name));
        summary
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(name, outcome.is_ok(), started.elapsed());
        outcome
    })
    .with_ignored_flag(true)
}

/// Runs `test` as libtest runs a test: on a thread of its own, named
/// after it, with the stack a spawned thread has by default, 2 MiB unless
/// `RUST_MIN_STACK` says otherwise, as libtest's test threads have, rather
/// than on the main thread, which `libtest-mimic` runs a test on with
/// `--test-threads=1`, and whose stack is the linker's. A panic is
/// its failure. Its message, for a scenario its whole report
/// (`registry::run_named`), is printed once, by the panic hook where it
/// failed, so that a run cut off later keeps it; the `failures:` section
/// says only where it is ([`PRINTED_ABOVE`]).
/// What the `failures:` section prints for a test that panicked.
const PRINTED_ABOVE: &str = "its report is printed above, where it failed";

fn on_test_thread(name: &'static str, test: impl FnOnce() + Send + 'static) -> Result<(), Failed> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || panic::catch_unwind(AssertUnwindSafe(test)).map_err(|_| ()))
        .map_err(|error| format!("the test's thread could not start: {error}"))?
        .join()
        .unwrap_or(Err(()))
        .map_err(|()| Failed::from(PRINTED_ABOVE))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (filters, rest) = command_line::split_filters(args.get(1..).unwrap_or_default());
    let arguments = Arguments::from_iter(args.iter().take(1).cloned().chain(rest));
    let summary = Arc::new(Mutex::new(RunSummary::default()));
    let mut trials: Vec<Trial> = [LIVE, DEMONSTRATIONS, MEASURE_SET]
        .into_iter()
        .flatten()
        .map(|name| scenario_trial(name, &summary))
        .collect();
    trials.push(
        Trial::test("agent_reports_an_interactive_window_station", || {
            on_test_thread(
                "agent_reports_an_interactive_window_station",
                agent_reports_an_interactive_window_station,
            )
        })
        .with_ignored_flag(true),
    );
    // libtest runs its tests in the order of their names, so the session
    // check runs before every scenario.
    trials.sort_by(|a, b| a.name().cmp(b.name()));
    trials.retain(|trial| command_line::selected(trial.name(), &filters, arguments.exact));
    // A panic a scenario catches is printed once, in its report.
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if !verbatim_e2e::registry::panic_is_reported_later() {
            default(info);
        }
    }));
    let conclusion = libtest_mimic::run(&arguments, trials);
    for line in summary
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .lines()
    {
        println!("{line}");
    }
    conclusion.exit_code()
}
