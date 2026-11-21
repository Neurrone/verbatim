//! The scenario registry (milestone M3 Track B): every live scenario as one
//! named, grouped, `#[non_exhaustive]`-free definition — [`ScenarioDef`] —
//! instead of logic living only inside a `#[test]` function.
//!
//! One identifier, [`ScenarioDef::name`], is used everywhere a scenario
//! needs naming: the test in `tests/e2e.rs` wrapping it (so
//! `cargo test -p verbatim-e2e --test e2e <name> -- --exact` selects exactly one
//! scenario), `cargo xtask e2e <name>`, the
//! per-scenario artifacts directory ([`crate::artifacts::scenario_dir`]),
//! and the name of the scenario's video in that directory
//! ([`crate::recording`]). Keeping every one of those in lockstep off a
//! single `&'static str` is deliberate: a scenario renamed in one place is a
//! compile error (or an unmistakable "unknown scenario" message) everywhere
//! else, rather than a silently stale mapping maintained by hand.
//!
//! A scenario is setup, body, and teardown, each a plain function pointer
//! operating on the already-launched [`Scenario`]:
//!
//! - `setup` declares and creates whatever state the scenario's body needs
//!   beyond `Scenario::launch` itself already provides: launching a target
//!   application such as Notepad and remembering its pid or window, or
//!   writing the harness folder a terminal's scripts run in.
//! - `body` is the scripted walk itself: gestures, keys, and speech
//!   assertions, exactly as today's `#[test]` functions already read, just
//!   moved into a free function instead of the test function directly.
//! - `teardown` restores what `setup` changed beyond the windows and files
//!   the harness closes and deletes itself, such as a theme the scenario
//!   created, and always runs, even
//!   when `body` panicked, because [`run`] wraps `body` (and `teardown`
//!   itself) in [`std::panic::catch_unwind`] rather than letting a body
//!   panic skip cleanup outright.
//!
//! None of this weakens the guard-struct discipline
//! [`crate::scenario::Scenario`]'s own doc comment describes: [`run`] has
//! `Scenario::clean_up` close everything the scenario opened, by process
//! id or window, failing the run when something will not close, and
//! `Scenario`'s `Drop` impl does the same after a panic. `setup`/`body`/
//! `teardown` are a layer of structure on top of that guarantee, for
//! scenario-specific state `Scenario` itself does not track.
//!
//! Groups say which runs include a scenario:
//!
//! - [`Group::Suite`]: the end-to-end suite, which CI's `e2e` job and
//!   `cargo test -p verbatim-e2e` run.
//! - [`Group::Demo`]: demonstrations, recorded as videos for
//!   `videos/demos` by `cargo xtask demo` and never part of the suite:
//!   their tests are ignored, so a plain `cargo test -p verbatim-e2e`, as
//!   CI's `e2e` job runs it, skips them. Each still asserts what it shows,
//!   so a broken feature fails rather than recording a misleading video.
//! - [`Group::Measure`]: the measure set, workloads that make Verbatim's
//!   latency ledger log the figures `docs/design/communication.md`
//!   section 11 compares at every migration step. They are measurement,
//!   not tests, and are left out like the demonstrations: their tests are
//!   ignored and named `measure_`, so the
//!   suite's runs deselect them with `--skip measure_`. Their speech is
//!   collected without being asserted ([`SpeechCollector::collect_only`](crate::speech::SpeechCollector::collect_only));
//!   `books/dev/src/how-to/e2e.md` says how to run them.
//!
//! An entry is one constructor, `suite!`, `demo!` or `measure!`, with its
//! defaults, and a method for each thing it changes:
//! [`ScenarioDef::local_only`], [`ScenarioDef::settings`] and
//! [`ScenarioDef::document`]. `suite!(module, speech)` names a scenario
//! after its module, whose `setup`, `body` and `teardown` it runs;
//! `suite!("name", module::{setup, body, teardown}, speech)` names the
//! three where a module holds several scenarios. The test binary
//! (`tests/e2e.rs`) builds a test from each entry, and `cargo xtask
//! e2e-plan` reads the constructors from this file's source
//! (`xtask/src/plan.rs`), so this is the one list of the scenarios.
//!
//! A scenario that is local-only cannot hold on GitHub's Windows Server
//! runner, for the reason its entry gives, with what would let it run
//! there again, such as Windows 11 Notepad, or a fault of the runner's that
//! Verbatim accepts; an entry cannot be local-only without its reason, and
//! it is the one record of a scenario taken off GitHub's runs. Every other
//! run includes it; GitHub's `e2e` job sets [`SKIP_LOCAL_ONLY_ENV`] and
//! leaves it out of its groups (`cargo xtask e2e-plan` reads the method),
//! and [`run_named`] fails a local-only scenario that runs anyway with the
//! variable set, so a skip is never reported as a pass. The skip comes from
//! that setting alone, never from detecting the machine.
//!
//! In compare mode ([`crate::compare`]), an exploration tool,
//! [`run_named`] runs a scenario against NVDA instead, with [`run`]'s
//! order, and writes its report. Every entry says whether compare mode
//! can run it, its [`Compare`]: a scenario that needs Verbatim itself,
//! such as one that opens Verbatim's own menu, is refused with why.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, PoisonError};

use verbatim_config::Settings;

use crate::artifacts::{self, OutpostCrash, ScenarioSummary};
use crate::compare::{self, ReportHeader};
use crate::scenario::{
    Document, Scenario, launch_evidence_of, write_launch_evidence, write_start_failure,
};
use crate::scenarios::{
    alt_tab_switcher, background_progress_bar, caret_keys, controlled_list_selections,
    crash_recovery, demo_notepad_editing, demo_review_cursor, demo_say_all,
    demo_settings_dialog_keys, demo_terminal_session, document_keys, editing, event_recorder,
    explorer_folder_window, foreground_progress_bar, lock_key_announcements, measure,
    menu_and_settings_dialog, menu_end_over_a_hung_window, msaa_flood_then_focus, notepad_say_all,
    notepad_spelling_errors, object_navigation_over_uia, owned_dialog_across_processes, password,
    quick_link_menu, quit_inside_a_file_dialog, quit_inside_a_modal_dialog,
    quit_inside_the_remove_theme_confirmation, quit_inside_the_verbatim_menu,
    rapid_tabbing_in_settings, read_only_field_typing, return_from_a_held_application,
    review_cursor, review_dead_object, review_words, second_application_and_verbatim_menu,
    selection_keys, settings_combo_box_expanded, settings_dialog_keys,
    settings_search_suggestions_reentered, speak_time_and_date, spelling_errors,
    stale_focus_behind_a_caret_key, switch_to_onecore, system_information_tree, table_cells,
    terminal_commands, terminal_editing, terminal_flood, terminal_key_timing, terminal_lists,
    terminal_overflow, terminal_pager, terminal_progress, terminal_review_grid, terminal_screens,
    terminal_settings_page, terminal_status_bar, terminal_typing, terminal_windows,
    text_box_say_all, theme_panel, toast, twin_item_at_a_deleted_items_ids, typed_words,
    verbatim_lifecycle,
};
use crate::speech::{FailurePoint, SpeechFailure};

/// Environment variable that, set to `1`, says the run is not on a Windows
/// 11 desktop, so the local-only scenarios ([`ScenarioDef::local_only`])
/// must not run: GitHub's `e2e` job sets it and leaves them out of the
/// scenarios it names, and one that runs anyway fails rather than
/// reporting a pass. Unset, empty, or `0` runs them. Nothing detects the
/// machine.
pub const SKIP_LOCAL_ONLY_ENV: &str = "VERBATIM_E2E_SKIP_LOCAL_ONLY";

/// Whether `def` is skipped on a run whose [`SKIP_LOCAL_ONLY_ENV`] holds
/// `setting` (`None` when it is unset): only a local-only scenario is ever
/// skipped, and only when the setting is `1`.
///
/// # Errors
///
/// Returns an error naming the variable when `setting` is anything but
/// unset, empty, `0`, or `1`, so a mistyped setting fails the run rather
/// than silently running or skipping.
pub fn skips(def: &ScenarioDef, setting: Option<&str>) -> Result<bool, String> {
    let skip_local_only = match setting.unwrap_or("") {
        "" | "0" => false,
        "1" => true,
        other => {
            return Err(format!(
                "{SKIP_LOCAL_ONLY_ENV} is {other:?}; set it to 1 to skip the local-only scenarios, or 0 or nothing to run them"
            ));
        }
    };
    Ok(skip_local_only && def.local_only.is_some())
}

/// Which runs include a scenario; see this module's own doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// The end-to-end suite.
    Suite,
    /// Demonstrations, recorded by `cargo xtask demo` and left out of the
    /// suite.
    Demo,
    /// The measure set: measurement workloads, which assert no speech,
    /// left out of the suite.
    Measure,
}

/// Whether compare mode ([`crate::compare`]) can run a scenario against
/// NVDA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    /// Compare mode runs it.
    Runs,
    /// Compare mode refuses it, for this reason: it needs Verbatim itself,
    /// such as Verbatim's own menu, or NVDA's speech for it could not be
    /// compared, such as a progress bar's tones, which the transcript
    /// add-on does not record.
    Refused(&'static str),
}

/// Scenario-specific state a [`ScenarioDef::setup`] creates and its matching
/// [`ScenarioDef::teardown`] restores. `None` when a scenario needs nothing
/// beyond `Scenario::launch` itself; `TargetPid` names the process owning a
/// window launched via [`Scenario::launch`], which cleanup closes.
#[derive(Debug)]
pub enum ScenarioState {
    /// No extra state: `setup` did nothing beyond validating preconditions.
    None,
    /// The pid of the process owning the window of a target application
    /// `setup` launched via [`Scenario::launch`], for the body;
    /// cleanup closes it ([`Scenario::clean_up`]).
    TargetPid(u32),
    /// Names `setup` recorded for `teardown`, such as the folders that were
    /// there before the scenario.
    Names(Vec<String>),
    /// The title of a window `setup` opened, such as a harness folder's
    /// ([`Scenario::open_folder`]), for the body to listen for.
    Title(String),
    /// A window `setup` opened with a title of the run's own
    /// ([`Scenario::launch`]), such as a terminal's.
    Window {
        /// The process that owns it, which cleanup closes it by
        /// ([`Scenario::clean_up`]).
        pid: u32,
        /// Its title.
        title: String,
        /// The folder, on the agent's machine, holding the files it uses.
        directory: String,
    },
}

/// A scenario's setup: declares and creates whatever state its body needs
/// beyond `Scenario::launch` itself, failing if creating it fails (for
/// example, the agent refuses to launch a target application).
pub type Setup = fn(&mut Scenario) -> io::Result<ScenarioState>;
/// A scenario's body: the scripted walk of gestures, keys, and speech
/// assertions. Panics are how a scenario failure is reported; [`run`]
/// catches them to collect failure artifacts before letting the panic
/// continue.
pub type Body = fn(&mut Scenario, &mut ScenarioState);
/// A scenario's teardown: restores whatever its setup created. Always
/// runs, even when the body panicked — see [`run`].
pub type Teardown = fn(&mut Scenario, ScenarioState);

/// One named, grouped scenario. See this module's own doc comment for the
/// setup/body/teardown contract and why [`name`](Self::name) is the single
/// identifier used everywhere.
#[derive(Debug, Clone, Copy)]
pub struct ScenarioDef {
    /// Selects this scenario: its test's name
    /// (`cargo test -p verbatim-e2e --test e2e <name> -- --exact`), its
    /// name on `cargo xtask e2e`, its artifacts directory name, and its
    /// recording file name prefix.
    pub name: &'static str,
    /// Which runs include this scenario.
    pub group: Group,
    /// Whether compare mode can run it.
    pub compare: Compare,
    /// Changes the fixed settings this scenario's Verbatim is launched
    /// with ([`Scenario::launch_with`]), for a scenario that needs
    /// a reader setting other than its default; `None` for most.
    pub settings: Option<fn(&mut Settings)>,
    /// Why this scenario cannot hold on GitHub's Windows Server runner,
    /// such as needing Windows 11 Notepad's spell checker, with what would
    /// let it run there again; `None` for most. Every other run includes
    /// it; a run with [`SKIP_LOCAL_ONLY_ENV`] set to `1`, as GitHub's `e2e`
    /// job sets it, skips it ([`skips`]).
    pub local_only: Option<&'static str>,
    /// Why an outpost is expected to fall back from a remote operation
    /// during this scenario, which otherwise fails it
    /// ([`crate::artifacts::fallback_lines`]); `None` for every scenario
    /// today.
    pub expected_fallback: Option<&'static str>,
    /// The document the scenario edits in Windows 11 Notepad, opened
    /// before Verbatim starts ([`crate::scenario::Document`]); `None` for
    /// most.
    pub document: Option<fn() -> Document>,
    /// Its setup.
    pub setup: Setup,
    /// Its body.
    pub body: Body,
    /// Its teardown.
    pub teardown: Teardown,
}

impl ScenarioDef {
    /// An entry with its defaults: the default settings, run everywhere,
    /// no Notepad document. The `suite!`, `demo!` and `measure!` macros
    /// call it.
    #[must_use]
    pub const fn new(
        name: &'static str,
        group: Group,
        compare: Compare,
        (setup, body, teardown): (Setup, Body, Teardown),
    ) -> Self {
        Self {
            name,
            group,
            compare,
            settings: None,
            local_only: None,
            expected_fallback: None,
            document: None,
            setup,
            body,
            teardown,
        }
    }

    /// The entry, run only off GitHub's runner, for `reason`.
    #[must_use]
    pub const fn local_only(mut self, reason: &'static str) -> Self {
        self.local_only = Some(reason);
        self
    }

    /// The entry, with an outpost's remote-operation fallback during it
    /// expected, for `reason`.
    #[must_use]
    pub const fn expects_fallback(mut self, reason: &'static str) -> Self {
        self.expected_fallback = Some(reason);
        self
    }

    /// The entry, its Verbatim launched with `settings` applied.
    #[must_use]
    pub const fn settings(mut self, settings: fn(&mut Settings)) -> Self {
        self.settings = Some(settings);
        self
    }

    /// The entry, with `document` opened in Windows 11 Notepad before
    /// Verbatim starts.
    #[must_use]
    pub const fn document(mut self, document: fn() -> Document) -> Self {
        self.document = Some(document);
        self
    }
}

/// A scenario of the suite: `suite!(module, compare)` runs `module`'s
/// `setup`, `body` and `teardown` and is named after it;
/// `suite!("name", module::{setup, body, teardown}, compare)` names them.
macro_rules! suite {
    ($module:ident, $compare:expr) => {
        ScenarioDef::new(
            stringify!($module),
            Group::Suite,
            $compare,
            ($module::setup, $module::body, $module::teardown),
        )
    };
    ($name:literal, $module:ident::{$setup:ident, $body:ident, $teardown:ident}, $compare:expr) => {
        ScenarioDef::new(
            $name,
            Group::Suite,
            $compare,
            ($module::$setup, $module::$body, $module::$teardown),
        )
    };
}

/// A demonstration, named after its module.
macro_rules! demo {
    ($module:ident) => {
        ScenarioDef::new(
            stringify!($module),
            Group::Demo,
            Compare::Refused("it is a demonstration, recorded by cargo xtask demo"),
            ($module::setup, $module::body, $module::teardown),
        )
    };
}

/// A workload of the measure set.
macro_rules! measure {
    ($name:literal, $module:ident::{$setup:ident, $body:ident, $teardown:ident}) => {
        ScenarioDef::new(
            $name,
            Group::Measure,
            Compare::Refused("it is a measurement workload"),
            ($module::$setup, $module::$body, $module::$teardown),
        )
    };
}

/// A Windows 11 Notepad scenario's reason to be local-only.
const NOTEPAD: &str = "Windows 11 Notepad, which GitHub's Windows Server runner does not have: its Notepad is classic Notepad";

/// Why compare mode refuses a scenario on Verbatim's own menu or settings dialog.
const OWN_GUI: &str =
    "it drives Verbatim's own menu or settings dialog, which do not exist without Verbatim";

/// Why compare mode refuses a progress bar scenario.
const PROGRESS_TONES: &str = "nvda/source/NVDAObjects/behaviors.py 41-88: a progress bar's value change is a tone, which the transcript add-on does not record";

/// Why compare mode refuses a terminal flood's scenario.
const TERMINAL_FLOOD: &str = "nvda/source/NVDAObjects/behaviors.py 420-510: LiveText reads each change's new lines, and which of a flood's middle lines NVDA says depends on timing";

/// Why the Settings search box's scenarios are local-only.
const SUGGESTIONS_REPLACED: &str = "Settings replaced its suggestions list as it opened, after raising the first selection, in 2 of 13 local runs (2026-11-09), so nothing was said of it, as in NVDA; on GitHub's runner \"Suggestions available\" played to its end (run 39558690998), likely the same. Settings' timing is not Verbatim's to control. It runs there again once dispatched runs show the runner's Settings keeping its first selection (`docs/flaky-tests.md`).";

use Compare::{Refused, Runs};

/// Every registered scenario, in a fixed, stable order.
pub const SCENARIOS: &[ScenarioDef] = &[
    suite!(menu_and_settings_dialog, Refused(OWN_GUI)),
    suite!(second_application_and_verbatim_menu, Refused("it reads Verbatim's status and opens Verbatim's own menu")),
    suite!(rapid_tabbing_in_settings, Refused("it takes speech until Verbatim is idle (take_until_idle) and drives Verbatim's own settings dialog")),
    suite!(switch_to_onecore, Refused("it reads Verbatim's status and drives Verbatim's own settings dialog")),
    suite!(crash_recovery, Refused("it ends Verbatim's synthesizer host, an outpost and its focus listener")),
    suite!(verbatim_lifecycle, Refused("it starts a second Verbatim to replace the first")),
    suite!(quit_inside_a_modal_dialog, Refused("it quits Verbatim inside Verbatim's own modal dialog")),
    suite!(quit_inside_the_verbatim_menu, Refused("it quits Verbatim inside Verbatim's own menu")),
    suite!(quit_inside_the_remove_theme_confirmation, Refused("it quits Verbatim inside Verbatim's own confirmation")),
    suite!(quit_inside_a_file_dialog, Refused("it quits Verbatim inside a file dialog Verbatim opened")),
    suite!(speak_time_and_date, Runs),
    suite!(toast, Runs)
        .local_only("No reason was recorded when it was added, and it has never run on GitHub's runner; a dispatched run of it there gives one, or lets it run there (`docs/flaky-tests.md`)."),
    suite!(msaa_flood_then_focus, Refused("nvda/source/IAccessibleHandler/orderedWinEventLimiter.py 10-120: NVDA keeps at most 10 of a thread's events, and a capture records no flood")),
    suite!(stale_focus_behind_a_caret_key, Runs),
    suite!(read_only_field_typing, Runs),
    suite!(twin_item_at_a_deleted_items_ids, Runs),
    suite!(foreground_progress_bar, Refused(PROGRESS_TONES)),
    suite!("background_progress_bar", background_progress_bar::{setup, on_body, teardown}, Refused(PROGRESS_TONES))
        .settings(background_progress_bar::report_background_bars),
    suite!("background_progress_bar_off", background_progress_bar::{setup, off_body, teardown}, Refused(PROGRESS_TONES)),
    suite!(controlled_list_selections, Runs),
    suite!(review_dead_object, Runs),
    suite!(table_cells, Runs),
    suite!(alt_tab_switcher, Runs),
    suite!(return_from_a_held_application, Runs),
    suite!(menu_end_over_a_hung_window, Refused("nvda/source/IAccessibleHandler/__init__.py 984-1008: NVDA fakes the focus on the desktop after the menu ends")),
    suite!(owned_dialog_across_processes, Runs),
    suite!(lock_key_announcements, Runs),
    suite!(quick_link_menu, Runs),
    suite!(explorer_folder_window, Runs)
        .local_only("On GitHub's runner its new explorer.exe window sometimes stops responding, with dwm.exe's window thread blocked too (run 44344991044); the cause is not named, and the harness does not wait for the Desktop Window Manager (Dickson, 2026-10-26). It runs there again once a wait chain names what Explorer's thread waits on and Verbatim's part, if any, is fixed, or Dickson rules to keep it there (`docs/flaky-tests.md`)."),
    suite!(settings_dialog_keys, Refused(OWN_GUI)),
    suite!(settings_combo_box_expanded, Runs)
        .local_only("The runner's Date and time page opens with its focus on its location settings link, not the Region button. It runs there again once the runner's Settings opens it on the Region button."),
    suite!(settings_search_suggestions_reentered, Runs)
        .local_only(SUGGESTIONS_REPLACED),
    suite!("notepad_editing", editing::{notepad_setup, notepad_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(editing::document),
    suite!("text_box_editing", editing::{text_box_setup, text_box_body, teardown}, Runs),
    suite!("notepad_selection_keys", selection_keys::{notepad_setup, notepad_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(selection_keys::document),
    suite!("text_box_selection_keys", selection_keys::{text_box_setup, text_box_body, teardown}, Runs),
    suite!("notepad_caret_keys", caret_keys::{notepad_setup, notepad_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(caret_keys::document),
    suite!("text_box_caret_keys", caret_keys::{text_box_setup, text_box_body, teardown}, Runs)
        .local_only("On GitHub's runner the outpost releases a caret key's starting anchor before Core commits it, so the reads after a paste answer not there and the line is never said (runs 44514942878 and 44754313469; docs/flaky-tests.md). It runs again on GitHub when build 8, which redesigns how anchors cross, fixes the release."),
    suite!("notepad_document_keys", document_keys::{notepad_setup, notepad_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(document_keys::document),
    suite!("text_box_document_keys", document_keys::{text_box_setup, text_box_body, teardown}, Runs),
    suite!("notepad_page_keys", document_keys::{notepad_pages_setup, notepad_pages_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(document_keys::pages_document),
    suite!("text_box_page_keys", document_keys::{text_box_pages_setup, text_box_pages_body, teardown}, Runs),
    suite!("notepad_paragraph_keys", document_keys::{notepad_paragraphs_setup, notepad_paragraphs_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(document_keys::paragraphs_document),
    suite!("text_box_paragraph_keys", document_keys::{text_box_paragraphs_setup, text_box_paragraphs_body, teardown}, Runs),
    suite!("notepad_select_all_count", document_keys::{notepad_count_setup, notepad_count_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(document_keys::count_document),
    suite!("text_box_select_all_count", document_keys::{text_box_count_setup, text_box_count_body, teardown}, Runs),
    suite!("text_box_password", password::{text_box_setup, text_box_body, teardown}, Runs),
    suite!("wpf_password", password::{wpf_setup, wpf_body, teardown}, Runs),
    suite!("notepad_review_cursor", review_cursor::{notepad_setup, notepad_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(review_cursor::document),
    suite!("text_box_review_cursor", review_cursor::{text_box_setup, text_box_body, teardown}, Runs),
    suite!(notepad_say_all, Runs)
        .local_only(NOTEPAD)
        .document(notepad_say_all::document),
    suite!("notepad_typed_words", typed_words::{notepad_setup, notepad_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(typed_words::document),
    suite!("text_box_typed_words", typed_words::{text_box_setup, text_box_body, teardown}, Runs),
    suite!("notepad_review_words", review_words::{notepad_setup, notepad_body, teardown}, Runs)
        .local_only(NOTEPAD)
        .document(review_words::document),
    suite!("text_box_review_words", review_words::{text_box_setup, text_box_body, teardown}, Runs),
    suite!(text_box_say_all, Runs),
    suite!(spelling_errors, Runs),
    suite!(notepad_spelling_errors, Runs)
        .local_only("Windows 11 Notepad's own spell checker, which GitHub's Windows Server runner does not have: its Notepad is classic Notepad, a Win32 edit control. `spelling_errors` hears the same speech from mockapp's scripted text everywhere.")
        .document(notepad_spelling_errors::document),
    suite!(theme_panel, Refused(OWN_GUI)),
    suite!(terminal_settings_page, Refused(OWN_GUI)),
    suite!("windows_terminal_commands", terminal_commands::{setup_windows_terminal, body_windows_terminal, teardown}, Runs),
    suite!("conhost_commands", terminal_commands::{setup_console_host, body_console_host, teardown}, Runs),
    suite!("windows_terminal_flood", terminal_flood::{setup_windows_terminal, body_windows_terminal, teardown}, Runs),
    suite!("conhost_flood", terminal_flood::{setup_console_host, body_console_host, teardown}, Runs),
    suite!("windows_terminal_control_flood", terminal_key_timing::{setup_control_windows_terminal, body_control_windows_terminal, teardown}, Refused(TERMINAL_FLOOD))
        .settings(terminal_flood::lowered_limits),
    suite!("conhost_control_flood", terminal_key_timing::{setup_control_console_host, body_control_console_host, teardown}, Refused(TERMINAL_FLOOD))
        .settings(terminal_flood::lowered_limits),
    suite!("windows_terminal_line_key_flood", terminal_key_timing::{setup_line_key_windows_terminal, body_line_key_windows_terminal, teardown}, Runs),
    suite!("conhost_line_key_flood", terminal_key_timing::{setup_line_key_console_host, body_line_key_console_host, teardown}, Runs),
    suite!("windows_terminal_leave_flood", terminal_windows::{setup_leave_windows_terminal, body_leave_windows_terminal, teardown}, Runs),
    suite!("conhost_leave_flood", terminal_windows::{setup_leave_console_host, body_leave_console_host, teardown}, Runs),
    suite!("windows_terminal_scrollback_overflow_during_group", terminal_overflow::{setup_windows_terminal, body_overflow_during_group_windows_terminal, teardown}, Refused(TERMINAL_FLOOD))
        .settings(terminal_flood::half_limits),
    suite!("conhost_scrollback_overflow_during_group", terminal_overflow::{setup_console_host, body_overflow_during_group_console_host, teardown}, Refused(TERMINAL_FLOOD))
        .settings(terminal_flood::half_limits),
    suite!("windows_terminal_editing", terminal_editing::{setup_windows_terminal, body_windows_terminal, teardown}, Runs),
    suite!("conhost_editing", terminal_editing::{setup_console_host, body_console_host, teardown}, Runs),
    suite!("windows_terminal_review_grid", terminal_review_grid::{setup_windows_terminal, body_windows_terminal, teardown}, Runs),
    suite!("conhost_review_grid", terminal_review_grid::{setup_console_host, body_console_host, teardown}, Runs),
    suite!("windows_terminal_progress", terminal_progress::{setup_windows_terminal, body_windows_terminal, teardown}, Runs),
    suite!("conhost_progress", terminal_progress::{setup_console_host, body_console_host, teardown}, Runs),
    suite!("windows_terminal_typing", terminal_typing::{setup_windows_terminal, body_windows_terminal, teardown}, Runs),
    suite!("conhost_typing", terminal_typing::{setup_console_host, body_console_host, teardown}, Runs),
    suite!("windows_terminal_long_lines", terminal_screens::{setup_windows_terminal, body_long_lines_windows_terminal, teardown}, Runs)
        .local_only("GitHub's runner once delivered no UI Automation focus for the new terminal to the listener (run 44414430624), a loss Verbatim accepts as NVDA does (ADR 0001). It runs there again once dispatched runs show the runner delivering the terminal's focus (`docs/flaky-tests.md`)."),
    suite!("conhost_long_lines", terminal_screens::{setup_console_host, body_long_lines_console_host, teardown}, Runs),
    suite!("windows_terminal_full_screen", terminal_screens::{setup_windows_terminal, body_full_screen_windows_terminal, teardown}, Runs),
    suite!("conhost_full_screen", terminal_screens::{setup_console_host, body_full_screen_console_host, teardown}, Runs),
    suite!("windows_terminal_marker_list", terminal_lists::{setup_windows_terminal, body_marker_windows_terminal, teardown}, Runs),
    suite!("conhost_marker_list", terminal_lists::{setup_console_host, body_marker_console_host, teardown}, Runs),
    suite!("windows_terminal_pager_history", terminal_pager::{setup_history_windows_terminal, body_history_windows_terminal, teardown}, Runs)
        .settings(terminal_flood::half_limits),
    suite!("conhost_pager_history", terminal_pager::{setup_history_console_host, body_history_console_host, teardown}, Runs)
        .settings(terminal_flood::half_limits),
    suite!("windows_terminal_status_bar", terminal_status_bar::{setup_windows_terminal, body_windows_terminal, teardown}, Runs),
    suite!("conhost_status_bar", terminal_status_bar::{setup_console_host, body_console_host, teardown}, Runs),
    suite!("windows_terminal_two_windows", terminal_windows::{setup_windows_terminal, body_two_windows_terminal, teardown}, Runs)
        .local_only("On GitHub's elevated runner the console host's \"Administrator: \" title reaches Windows Terminal's window after the harness restores it, so the first announcement carries the prefix; what sends the prefixed title is not established. It runs there again once a dispatched run with the window's title logged at each name change names the sender, and a harness wait on that evidence is built (`docs/flaky-tests.md`)."),
    suite!("windows_terminal_tabs", terminal_windows::{setup_tabs, body_tabs, teardown}, Runs),
    suite!("event_recorder_records_mockapp", event_recorder::{setup, body, teardown}, Refused("the recorder's file is what is under test, which NVDA does not write")),
    suite!(object_navigation_over_uia, Runs),
    suite!(system_information_tree, Runs),
    demo!(demo_notepad_editing)
        .document(demo_notepad_editing::document),
    demo!(demo_review_cursor)
        .document(demo_review_cursor::document),
    demo!(demo_say_all)
        .document(notepad_say_all::document),
    demo!(demo_terminal_session),
    demo!(demo_settings_dialog_keys),
    measure!("measure_focus_windows_terminal", measure::{setup_focus_windows_terminal, body_focus_windows_terminal, teardown}),
    measure!("measure_focus_windows_terminal_tabs", measure::{setup_focus_tabs, body_focus_tabs, teardown}),
    measure!("measure_focus_text_box", measure::{setup_focus_text_box, body_focus_text_box, teardown}),
    measure!("measure_focus_new_application", measure::{no_setup, body_focus_new_application, teardown}),
    measure!("measure_focus_return", measure::{setup_focus_return, body_focus_return, teardown}),
    measure!("measure_focus_return_retitled", measure::{setup_focus_return_retitled, body_focus_return_retitled, teardown}),
    measure!("measure_focus_flood", measure::{no_setup, body_focus_flood, teardown}),
    measure!("measure_keys_text_box", measure::{setup_keys_text_box, body_keys_text_box, teardown}),
    // It needs Windows 11 Notepad as the `notepad_` scenarios do, but a measurement is never part of a suite run, CI's included, and its name cannot start with a local-only filter.
    measure!("measure_keys_notepad", measure::{setup_keys_notepad, body_keys_notepad, teardown})
        .document(measure::notepad_document),
    measure!("measure_keys_conhost", measure::{setup_keys_console_host, body_keys_console_host, teardown}),
    measure!("measure_keys_windows_terminal", measure::{setup_keys_windows_terminal, body_keys_windows_terminal, teardown}),
    measure!("measure_stale_caret_text_box", measure::{setup_stale_caret_text_box, body_stale_caret_text_box, teardown}),
    // As `measure_keys_notepad`.
    measure!("measure_stale_caret_notepad", measure::{setup_stale_caret_notepad, body_stale_caret_notepad, teardown})
        .document(measure::stale_notepad_document),
    measure!("measure_stale_caret_conhost", measure::{setup_stale_caret_console_host, body_stale_caret_console_host, teardown}),
    measure!("measure_held_repeat_text_box", measure::{setup_held_repeat_text_box, body_held_repeat_text_box, teardown}),
    measure!("measure_speech_real", measure::{setup_speech_real, body_speech_real, teardown}),
];

/// Looks up a scenario or a demonstration by [`ScenarioDef::name`].
#[must_use]
pub fn find(name: &str) -> Option<&'static ScenarioDef> {
    SCENARIOS.iter().find(|def| def.name == name)
}

/// The speech every scenario starts with: Verbatim's start sound, heard in
/// full. Verbatim speaks no start message, as NVDA speaks none
/// (`docs/parity/system.md`, "Starting"); the announcement of the desktop, which
/// has the focus once every window is minimized (`Scenario::launch`),
/// follows, asserted after it.
#[must_use]
pub fn startup_speech() -> [crate::speech::Expected; 1] {
    [crate::speech::heard("sound: start")]
}

/// Looks up `name` and runs it: the body of every scenario's `#[test]` in
/// `crates/verbatim-e2e/tests/e2e.rs`. Those tests are `#[ignore]`d, so a
/// workspace test run lists them as ignored rather than running them, and
/// the end-to-end job runs them with `--ignored`.
///
/// # Panics
///
/// Panics if `name` is not registered; if [`crate::ENDPOINT_ENV`] is unset,
/// since a live scenario that cannot reach an agent has not passed; if the
/// scenario is local-only and [`SKIP_LOCAL_ONLY_ENV`] is `1`, since such a
/// run leaves it out of the scenarios it names instead; if [`compare::ENV`] names another scenario,
/// as a harness error naming both ([`compare::selects`]); and if the
/// scenario itself fails.
pub fn run_named(name: &str) {
    let Some(def) = find(name) else {
        panic!(
            "no scenario named {name:?} is registered; known scenarios: {}",
            SCENARIOS
                .iter()
                .map(|def| def.name)
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    let setting = std::env::var(SKIP_LOCAL_ONLY_ENV).ok();
    match skips(def, setting.as_deref()) {
        Ok(true) => panic!(
            "{name:?} runs only on a Windows 11 desktop, and {SKIP_LOCAL_ONLY_ENV} is 1: such a run leaves the local-only scenarios out of the scenarios it names rather than running them"
        ),
        Ok(false) => {}
        Err(error) => panic!("{error}"),
    }
    assert!(
        crate::endpoint().is_some(),
        "{} is not set: a live scenario needs a running agent (books/dev/src/how-to/e2e.md)",
        crate::ENDPOINT_ENV
    );
    let comparing = compare::selects(name, std::env::var(compare::ENV).ok().as_deref())
        .unwrap_or_else(|error| panic!("{error}"));
    run(
        def,
        if comparing {
            Mode::Compare
        } else {
            Mode::Verbatim
        },
    );
}

/// Takes the workspace's run lock, `target/e2e-run.lock`, for one run of
/// the scenario `name`, from before it clears its artifacts until it has
/// archived them; the lock is released when the returned file is dropped,
/// or when this process ends however it ends. Panics, before the run
/// touches anything, when another test process holds it: two runs at once
/// share the desktop, the stage and the artifacts, and a second run
/// started while a first was saving its recording deleted the first's
/// artifacts folder under it. The in-process lock (`Scenario`'s) does not
/// cover this, since each `cargo test` is its own process.
fn hold_run_lock(name: &str) -> std::fs::File {
    let folder = crate::scenario::workspace_root().join("target");
    let path = folder.join("e2e-run.lock");
    // `target` is missing when cargo builds elsewhere (`CARGO_TARGET_DIR`).
    let file = std::fs::create_dir_all(&folder)
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
        })
        .unwrap_or_else(|error| {
            panic!(
                "scenario {name:?}: could not open the run lock {}: {error}",
                path.display()
            )
        });
    match file.try_lock() {
        Ok(()) => file,
        Err(std::fs::TryLockError::WouldBlock) => panic!(
            "scenario {name:?}: another scenario run in this workspace holds {}; scenarios run one at a time, so wait for that run's test process to exit",
            path.display()
        ),
        Err(std::fs::TryLockError::Error(error)) => panic!(
            "scenario {name:?}: could not take the run lock {}: {error}",
            path.display()
        ),
    }
}

/// What a run of a scenario runs against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// A Verbatim the run launches, every assertion checked.
    Verbatim,
    /// NVDA, in compare mode ([`crate::compare`]): asserting nothing, and
    /// writing what the scenario expects Verbatim to say beside what NVDA
    /// said as `compare.txt` in the run's artifacts, and printing it.
    Compare,
}

/// Runs one scenario in `mode` and fails it, after collecting everything,
/// if anything went wrong.
///
/// The order: refuse, in compare mode, a scenario it cannot run, giving
/// why ([`Compare::Refused`]); start from the
/// minimized desktop, launching Verbatim, or with NVDA's transcript
/// connected ([`Scenario::compare_with`]); assert Verbatim's startup speech
/// ([`startup_speech`]), or hear NVDA's desktop as an ordinary step; run
/// setup and the body, which ends by asserting that nothing more was said;
/// release any key still held. Against Verbatim: save Core's focus and the
/// flight recorder while Verbatim is up; check that none of Verbatim's own
/// processes exited unexpectedly; quit Verbatim and check it exited with
/// code 0 and its outposts shut down cleanly; take the speech Verbatim
/// sent, until the speech connection ends after a clean quit, or what has
/// arrived after a failed one. In compare mode, take the steps instead.
/// Then run the scenario's teardown; close everything the scenario opened;
/// collect the logs, latency report, audio, crash dumps, and video, or
/// write the comparison's report; write the summary and archive the run.
/// Each step after the body runs whatever happened before it, and every
/// problem it meets is a failure of the run, reported together with the
/// body's own. In compare mode none of the checks on Verbatim runs, so it
/// passes unless the harness itself fails, never on what was said.
#[expect(
    clippy::too_many_lines,
    reason = "one run in order, each step's problems collected, so a failure's place in the order is plain"
)]
fn run(def: &ScenarioDef, mode: Mode) {
    if mode == Mode::Compare
        && let Compare::Refused(why) = def.compare
    {
        panic!(
            "scenario {:?} cannot be compared with NVDA: {why}",
            def.name
        );
    }
    let _run_lock = hold_run_lock(def.name);
    let dir = artifacts::scenario_dir(&artifacts::artifacts_root(), def.name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap_or_else(|error| {
            panic!(
                "scenario {:?}: could not clear the previous run's artifacts in {}: {error}",
                def.name,
                dir.display()
            )
        });
    }

    let document = def.document.map(|document| document());
    let started = match mode {
        Mode::Verbatim => Scenario::launch_with(def.settings, document),
        Mode::Compare => Scenario::compare_with(def.settings, document),
    };
    let mut scenario = started.unwrap_or_else(|error| {
        record_launch_evidence(&dir, &error);
        panic!("scenario {:?}: starting the run failed: {error}", def.name)
    });
    let mut foreground = vec![format!("before setup: {}", scenario.foreground_report())];
    let mut problems: Vec<String> = Vec::new();

    let mut state = None;
    let body_outcome = caught(|| {
        if mode == Mode::Verbatim {
            scenario.speech().expect_sequence(&startup_speech());
        }
        crate::scenarios::expect_desktop(&mut scenario);
        // The measure set asserts no speech after the desktop's.
        if def.group == Group::Measure {
            scenario.speech().collect_only();
        }
        let mut created = (def.setup)(&mut scenario)
            .unwrap_or_else(|error| panic!("scenario {:?}: setup failed: {error}", def.name));
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            (def.body)(&mut scenario, &mut created);
            scenario.expect_nothing_more();
        }));
        state = Some(created);
        if let Err(payload) = result {
            panic::resume_unwind(payload);
        }
    });
    // When the body ended, on the wall clock `gdigrab` stamps the video's
    // first frame with, so the recording's check can tell that the video
    // reaches it.
    let body_end_ms = crate::timeline::unix_ms();
    // Where the body failed, with what it failed with: a failed speech
    // assertion records its own point, and any other failure's is the
    // step under way now, before the teardown adds to the timeline.
    let body_failure = body_outcome.as_ref().err().map(|payload| {
        payload.downcast_ref::<SpeechFailure>().map_or_else(
            || {
                (
                    panic_message(payload.as_ref()),
                    FailurePoint::now(scenario.timeline()),
                )
            },
            |failure| (failure.message.clone(), failure.point.clone()),
        )
    });
    // An application that stopped responding is asked what it waits on
    // at once, before anything else, while it may still hang.
    let body_failure =
        body_failure.map(
            |(message, point)| match scenario.record_not_responding_wait_chains() {
                Some(note) => (format!("{message}\n{note}"), point),
                None => (message, point),
            },
        );
    // A key the body held, as Control through a tab switcher, is released
    // first, whether the body passed or failed, so nothing after it runs
    // with a modifier down.
    problems.extend(scenario.release_held_keys());

    let steps = match mode {
        Mode::Compare => Some(scenario.speech().take_steps()),
        Mode::Verbatim => {
            problems.extend(check_and_quit(&mut scenario, &dir));
            None
        }
    };
    let teardown_outcome = state.map(|state| caught(|| (def.teardown)(&mut scenario, state)));
    if let Some(Err(payload)) = &teardown_outcome {
        problems.push(format!(
            "teardown failed: {}",
            panic_message(payload.as_ref())
        ));
    }
    problems.extend(scenario.clean_up());
    problems.extend(scenario.collect_run_artifacts(&dir, def.expected_fallback));
    let outpost_crashes = if let Some(steps) = &steps {
        problems.extend(write_comparison(def, &scenario, &dir, steps));
        None
    } else {
        verbatim_artifacts(def, &mut scenario, &dir, body_end_ms, &mut problems)
    };
    foreground.push(format!("after cleanup: {}", scenario.cleanup_report()));
    let optional_utterances: Vec<String> = scenario
        .speech()
        .optional_outcomes()
        .iter()
        .map(ToString::to_string)
        .collect();
    // Every utterance of the run with its event's time, the rows
    // `latency.csv` holds.
    let latency = (mode == Mode::Verbatim).then(|| scenario.speech().latency_rows());
    // The failure's report, made once the logs holding its step's trace
    // and the flight recorder are collected, reads the timeline after the
    // scenario is gone.
    let timeline = scenario.timeline().clone();
    drop(scenario);
    if let Err(error) = write_foreground(&dir, &foreground) {
        problems.push(format!("could not write the foreground record: {error}"));
    }

    let passed = body_outcome.is_ok() && problems.is_empty();
    report_outpost_crashes(def.name, outpost_crashes.as_deref());
    report_optional_utterances(def.name, &optional_utterances);
    let mut summary = ScenarioSummary::new(def.name, passed, latency.as_deref());
    summary.outpost_crashes = outpost_crashes;
    summary.optional_utterances = optional_utterances;
    if let Err(error) = summary.write(&dir) {
        problems.push(format!("could not write the run summary: {error}"));
    }
    if let Err(error) = artifacts::archive_run(&artifacts::artifacts_root(), def.name, &dir, passed)
    {
        problems.push(format!("could not archive the run's artifacts: {error}"));
    }
    println!(
        "scenario {:?}: {}",
        def.name,
        match (mode, body_outcome.is_ok() && problems.is_empty()) {
            (Mode::Verbatim, true) => "pass",
            (Mode::Verbatim, false) => "fail",
            (Mode::Compare, true) => "compared with NVDA",
            (Mode::Compare, false) => "harness error in its comparison with NVDA",
        }
    );
    let problems = problems.join("\n");
    let failure = match body_failure {
        Some((message, point)) if problems.is_empty() => Some((message, point)),
        Some((message, point)) => Some((
            format!(
                "{message}\n\nscenario {:?} also failed after its body:\n{problems}",
                def.name
            ),
            point,
        )),
        None if problems.is_empty() => None,
        None => Some((
            format!("scenario {:?} failed after its body:\n{problems}", def.name),
            FailurePoint::now(&timeline),
        )),
    };
    // Every failure is reported in full, with the inputs around it: the
    // panic carries the whole report, which the panic hook prints here,
    // once; the `failures:` section only points to it.
    if let Some((message, point)) = failure {
        let lines = artifacts::failure_lines(&dir, point.trace, point.window_us(&timeline))
            .map_err(|error| error.to_string());
        panic!(
            "{}",
            point.report(
                &message,
                &timeline,
                lines.as_deref().map_err(String::as_str)
            )
        );
    }
}

/// Against Verbatim, after the body: saves Core's focus and the flight
/// recorder while Verbatim is up, unless it exited by itself at a step of
/// the body; checks that none of its own processes exited unexpectedly;
/// quits it, checking its outposts shut down cleanly; and takes its speech
/// until the speech connection ends, the evidence that every frame it sent
/// has been taken, so `latency.csv` and the summary cover the run's last
/// utterance, or, after a failed quit, which may leave the connection open,
/// what has arrived. Returns the problems it met.
fn check_and_quit(scenario: &mut Scenario, dir: &std::path::Path) -> Vec<String> {
    let mut problems = Vec::new();
    if !scenario.exited() {
        if let Err(error) = scenario.collect_focus(dir) {
            problems.push(format!("could not save Core's focus: {error}"));
        }
        if let Err(error) = scenario.collect_flight_recorder(dir) {
            problems.push(format!("could not save the flight recorder: {error}"));
        }
    }
    match scenario.unexpected_exits() {
        Ok(exits) => problems.extend(exits.into_iter().map(|exit| {
            format!(
                "Verbatim's process {} (pid {}) exited unexpectedly: exit code {:?}{}",
                exit.image,
                exit.pid,
                exit.exit_code,
                if exit.abnormal {
                    ", abnormally (a crash)"
                } else {
                    ""
                }
            )
        })),
        Err(error) => problems.push(format!("could not read Verbatim's process exits: {error}")),
    }
    match scenario.quit_verbatim() {
        Ok(()) => {
            match scenario.outposts_shut_down_cleanly() {
                Ok(found) => problems.extend(found),
                Err(error) => problems.push(format!(
                    "could not check that Verbatim's outposts shut down cleanly: {error}"
                )),
            }
            if let Err(problem) = scenario.speech().take_until_the_end() {
                problems.push(problem);
            }
        }
        Err(error) => {
            problems.push(format!("quitting Verbatim failed: {error}"));
            scenario.speech().take_remaining();
        }
    }
    problems
}

/// Against Verbatim, once its artifacts are collected: reads which
/// outposts crashed, saves the recording, and checks that no Windows
/// Terminal window the run did not open appeared, adding each problem to
/// `problems`. Returns the outposts that crashed, when they could be read.
fn verbatim_artifacts(
    def: &ScenarioDef,
    scenario: &mut Scenario,
    dir: &std::path::Path,
    body_end_ms: u64,
    problems: &mut Vec<String>,
) -> Option<Vec<OutpostCrash>> {
    let outpost_crashes = match scenario.outpost_crashes(dir) {
        Ok(crashes) => Some(crashes),
        Err(error) => {
            problems.push(format!("could not read which outposts crashed: {error}"));
            None
        }
    };
    if let Err(error) =
        scenario.finish_recording(&dir.join(format!("{}.mp4", def.name)), body_end_ms)
    {
        problems.push(format!("the recording could not be saved: {error}"));
    }
    match scenario.foreign_terminal_windows() {
        Ok(windows) => problems.extend(windows.into_iter().map(|window| {
            format!(
                "a Windows Terminal window the run did not open appeared: {:?} (pid {}, class {})",
                window.title, window.pid, window.class
            )
        })),
        Err(error) => problems.push(format!(
            "could not check for Windows Terminal windows the run did not open: {error}"
        )),
    }
    outpost_crashes
}

/// In compare mode, writes the report of `steps` as `compare.txt` in
/// `dir` and prints it. Returns the problems it met.
fn write_comparison(
    def: &ScenarioDef,
    scenario: &Scenario,
    dir: &std::path::Path,
    steps: &[compare::CompareStep],
) -> Vec<String> {
    let (nvda_version, quiet) = scenario
        .nvda_run()
        .map(|(version, quiet)| (version.to_owned(), quiet))
        .expect("compare mode runs against NVDA");
    let (pin, pin_matches) =
        compare::submodule_pin(&crate::scenario::workspace_root(), &nvda_version);
    let header = ReportHeader {
        scenario: def.name.to_owned(),
        date: artifacts::utc_date(std::time::SystemTime::now()),
        nvda_version,
        pin,
        pin_matches,
        quiet,
    };
    // The run's token in the harness's window titles written as its
    // label, so a report names it in the same way on both sides and in
    // every run.
    let report = compare::substituted(
        &compare::render_report(&header, steps),
        &[("run".to_owned(), crate::scenario::run_token().to_owned())],
    );
    // Written to the handle itself, past the test harness's capture, so
    // the report shows for a run that passes.
    let _ = io::stdout().write_all(format!("{report}\n").as_bytes());
    match std::fs::write(dir.join(compare::REPORT_FILE_NAME), &report) {
        Ok(()) => Vec::new(),
        Err(error) => vec![format!("could not write the comparison: {error}")],
    }
}

thread_local! {
    /// How many [`caught`] calls this thread is inside.
    static CAUGHT_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Runs a scenario's setup, body or teardown, catching a panic, whose
/// message the scenario's report then carries; while it runs,
/// [`panic_is_reported_later`] is true on this thread, so the suite's panic
/// hook does not print that message a first time.
fn caught<R>(part: impl FnOnce() -> R) -> std::thread::Result<R> {
    CAUGHT_DEPTH.with(|depth| depth.set(depth.get() + 1));
    let result = panic::catch_unwind(AssertUnwindSafe(part));
    CAUGHT_DEPTH.with(|depth| depth.set(depth.get() - 1));
    result
}

/// Whether a panic on this thread now is caught by a scenario and its
/// message printed in the scenario's report, for the suite's panic hook.
#[must_use]
pub fn panic_is_reported_later() -> bool {
    CAUGHT_DEPTH.with(std::cell::Cell::get) > 0
}

/// The scenarios this test process has run, and the outpost crashes they
/// saw, for [`report_outpost_crashes`]'s running total. The suite runs
/// every scenario in one test process, so the total on its last scenario's
/// line is the suite's.
static OUTPOST_CRASH_TOTALS: Mutex<(usize, usize)> = Mutex::new((0, 0));

/// Writes one line to stderr after each scenario: the outpost crashes it
/// saw, each with its pid, exit code and dump, and the running total of
/// crashes and scenarios in this test process, so a run's crash frequency
/// is known. It is written to the stderr handle itself rather than with
/// `eprintln!`, which the test harness captures and shows only for a
/// failed test, so the line appears for a passing scenario too.
fn report_outpost_crashes(name: &str, crashes: Option<&[OutpostCrash]>) {
    let (total, scenarios) = {
        let mut totals = OUTPOST_CRASH_TOTALS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        totals.0 += crashes.map_or(0, <[OutpostCrash]>::len);
        totals.1 += 1;
        *totals
    };
    let seen = crashes.map_or_else(
        || "could not be read".to_owned(),
        |crashes| {
            std::iter::once(crashes.len().to_string())
                .chain(crashes.iter().map(ToString::to_string))
                .collect::<Vec<_>>()
                .join("; ")
        },
    );
    let line = format!(
        "scenario {name:?}: outpost crashes: {seen}; in this test run so far: {total} in {scenarios} scenario(s)\n"
    );
    let _ = io::stderr().write_all(line.as_bytes());
}

/// Writes one line to stderr for each optional utterance the scenario
/// asserted (`crate::speech::optional`), saying what became of it: absent,
/// heard in full, or cut off, and how many times when repeated, so a
/// change in how often the application causes it stays visible. Like
/// [`report_outpost_crashes`], it writes to the stderr handle itself, so
/// the lines appear for a passing scenario too.
fn report_optional_utterances(name: &str, outcomes: &[String]) {
    let mut lines = String::new();
    for outcome in outcomes {
        let _ = writeln!(lines, "scenario {name:?}: optional utterance {outcome}");
    }
    let _ = io::stderr().write_all(lines.as_bytes());
}

/// The message a panic carried, for reporting it beside other problems:
/// its text, or a failed speech assertion's message.
#[must_use]
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .or_else(|| {
            payload
                .downcast_ref::<SpeechFailure>()
                .map(|failure| failure.message.clone())
        })
        .unwrap_or_else(|| "(a panic with no message)".to_owned())
}

/// Writes what a failed launch's error carries, recorded when its
/// document's window did not open or take the foreground in time (the wait
/// chains, and the launched process's facts and minidump), or when
/// Verbatim did not become ready (its stderr log), to the scenario's
/// artifacts in `dir`; a launch fails before there is a scenario to
/// collect them.
fn record_launch_evidence(dir: &std::path::Path, error: &io::Error) {
    if let Some(evidence) = launch_evidence_of(error)
        && let Err(write) = write_launch_evidence(dir, std::slice::from_ref(evidence))
    {
        eprintln!("could not write what was recorded of the launch: {write}");
    }
    if let Err(write) = write_start_failure(dir, error) {
        eprintln!("could not write Verbatim's stderr log from the failed start: {write}");
    }
}

/// Records what held the foreground before setup and after cleanup.
fn write_foreground(dir: &std::path::Path, lines: &[String]) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("foreground.txt"), lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<ScenarioDef> {
        #[allow(
            clippy::unnecessary_wraps,
            reason = "must match ScenarioDef::setup's fn-pointer signature"
        )]
        fn no_setup(_: &mut Scenario) -> io::Result<ScenarioState> {
            Ok(ScenarioState::None)
        }
        fn no_body(_: &mut Scenario, _: &mut ScenarioState) {}
        #[allow(
            clippy::needless_pass_by_value,
            reason = "must match ScenarioDef::teardown's fn-pointer signature"
        )]
        fn no_teardown(_: &mut Scenario, _: ScenarioState) {}

        ["alpha", "beta", "gamma"]
            .map(|name| {
                ScenarioDef::new(
                    name,
                    Group::Suite,
                    Compare::Runs,
                    (no_setup, no_body, no_teardown),
                )
            })
            .to_vec()
    }

    #[test]
    fn find_locates_a_real_registered_scenario() {
        assert!(find("menu_and_settings_dialog").is_some());
        assert!(find("no_such_scenario").is_none());
    }

    #[test]
    fn the_skip_setting_skips_exactly_the_local_only_scenarios() {
        let mut scenarios = fixture();
        scenarios[1] = scenarios[1].local_only("a reason");
        let skipped: Vec<&str> = scenarios
            .iter()
            .filter(|def| skips(def, Some("1")).expect("1 is a valid setting"))
            .map(|def| def.name)
            .collect();
        assert_eq!(skipped, vec!["beta"]);
    }

    #[test]
    fn every_scenario_runs_when_the_skip_setting_is_unset_empty_or_0() {
        let mut scenarios = fixture();
        scenarios[1] = scenarios[1].local_only("a reason");
        for setting in [None, Some(""), Some("0")] {
            for def in &scenarios {
                assert_eq!(skips(def, setting), Ok(false), "{setting:?} {}", def.name);
            }
        }
    }

    #[test]
    fn an_unrecognized_skip_setting_is_an_error() {
        let scenarios = fixture();
        let error = skips(&scenarios[0], Some("yes")).expect_err("yes is not a valid setting");
        assert!(error.contains(SKIP_LOCAL_ONLY_ENV), "{error}");
    }

    #[test]
    fn a_local_only_scenario_run_where_it_cannot_hold_fails_instead_of_passing() {
        let local = SCENARIOS
            .iter()
            .find(|def| def.local_only.is_some())
            .expect("there are local-only scenarios");
        assert_eq!(skips(local, Some("1")), Ok(true));
    }

    /// The `(file, entry)` each `deviates_from_nvda` call in `source`
    /// names, in order, or the call's text when it does not name them as
    /// two string literals: every occurrence of the method's name followed
    /// by an opening parenthesis outside a comment line, in either form,
    /// `scenario.deviates_from_nvda("file", "entry")` or
    /// `Scenario::deviates_from_nvda(scenario, "file", "entry")`, however
    /// rustfmt breaks its lines, and with any character in the literals,
    /// parentheses and escaped quotes included. It does not catch a call
    /// made through another name, such as a wrapper function or a
    /// function pointer, and refuses, rather than reads, a call whose file
    /// or entry is a constant or a variable.
    fn deviation_calls(source: &str) -> Vec<Result<(String, String), String>> {
        const CALL: &str = "deviates_from_nvda(";
        let mut calls = Vec::new();
        for (at, _) in source.match_indices(CALL) {
            let line_start = source[..at].rfind('\n').map_or(0, |index| index + 1);
            if source[line_start..at].trim_start().starts_with("//") {
                continue;
            }
            let mut literals = Vec::new();
            let mut chars = source[at + CALL.len()..].chars();
            let mut refused = None;
            while literals.len() < 2 {
                match chars.next() {
                    Some('"') => {
                        let mut literal = String::new();
                        loop {
                            match chars.next() {
                                Some('\\') => literal.extend(chars.next()),
                                Some('"') | None => break,
                                Some(c) => literal.push(c),
                            }
                        }
                        literals.push(literal);
                    }
                    Some(')') | None => {
                        refused = Some(source[at..].lines().next().unwrap_or("").to_owned());
                        break;
                    }
                    // The receiver of the function-call form, commas and
                    // white space.
                    Some(_) => {}
                }
            }
            calls.push(match (refused, literals.as_slice()) {
                (None, [file, entry]) => Ok((file.clone(), entry.clone())),
                (refused, _) => Err(refused.unwrap_or_default()),
            });
        }
        calls
    }

    /// Every `(source, file, entry)` a scenario's `deviates_from_nvda` call
    /// names ([`deviation_calls`]), read from every Rust source under
    /// `src/scenarios`, in its subfolders too.
    fn declared_deviations() -> Vec<(String, String, String)> {
        let mut folders = vec![
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("scenarios"),
        ];
        let mut declared = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("the scenarios folder is readable") {
                let path = entry.expect("lists the scenarios").path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("reads a scenario's source");
                for call in deviation_calls(&source) {
                    let (file, entry) = call.unwrap_or_else(|text| {
                        panic!(
                            "{}: deviates_from_nvda names a file and an entry as two string literals: {text}",
                            path.display()
                        )
                    });
                    declared.push((path.display().to_string(), file, entry));
                }
            }
        }
        declared
    }

    #[test]
    fn a_deviation_is_read_from_either_call_form_with_any_text_in_its_literals() {
        let source = r#"
            scenario.deviates_from_nvda("docs/parity/review.md", "Review cursor columns");
            Scenario::deviates_from_nvda(
                scenario,
                "docs/parity/terminals.md",
                "New terminal output (\"blank\")",
            );
            // scenario.deviates_from_nvda(FILE, ENTRY) in a comment is not a call.
            scenario.deviates_from_nvda(FILE, "Review cursor columns");
        "#;
        assert_eq!(
            deviation_calls(source),
            vec![
                Ok((
                    "docs/parity/review.md".to_owned(),
                    "Review cursor columns".to_owned()
                )),
                Ok((
                    "docs/parity/terminals.md".to_owned(),
                    "New terminal output (\"blank\")".to_owned()
                )),
                Err(r#"deviates_from_nvda(FILE, "Review cursor columns");"#.to_owned()),
            ]
        );
    }

    /// Every deviation a scenario declares names a `###` heading of a file
    /// of the parity ledger: each call under `src/scenarios`, in either
    /// form, as [`deviation_calls`] reads them. It does not catch a call
    /// made through another name, such as a wrapper function or a function
    /// pointer, and fails on one whose file or entry is not a string
    /// literal written in the call.
    #[test]
    fn every_declared_deviation_names_an_entry_of_the_parity_ledger() {
        let declared = declared_deviations();
        assert!(
            !declared.is_empty(),
            "no scenario declares a deviation from NVDA"
        );
        let root = crate::scenario::workspace_root();
        for (source, file, heading) in declared {
            assert!(
                file.starts_with("docs/parity/")
                    && std::path::Path::new(&file)
                        .extension()
                        .is_some_and(|extension| extension == "md"),
                "{source} names {file:?}, which is not a file of the parity ledger"
            );
            let ledger = std::fs::read_to_string(root.join(&file)).unwrap_or_else(|error| {
                panic!("{source} names {file}, which cannot be read: {error}")
            });
            assert!(
                ledger
                    .lines()
                    .any(|line| line.strip_prefix("### ") == Some(heading.as_str())),
                "{source} names the entry {heading:?}, which is no ### heading in {file}"
            );
        }
    }
}
