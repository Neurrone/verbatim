//! The scenario registry (milestone M3 Track B): every live scenario as one
//! named, grouped, `#[non_exhaustive]`-free definition — [`ScenarioDef`] —
//! instead of logic living only inside a `#[test]` function.
//!
//! One identifier, [`ScenarioDef::name`], is used everywhere a scenario
//! needs naming: the plain-libtest `#[test]` function wrapping it (so
//! `cargo test -p verbatim-e2e <name> -- --exact` selects exactly one
//! scenario), `cargo xtask vm test --scenario <name>`'s selector, the
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
//! Groups (`docs/roadmap.md`'s M3 track) are a coarse selector for
//! `cargo xtask vm test --group`, not a strict taxonomy:
//!
//! - [`Group::Speech`]: Verbatim's own menu and Speech settings dialog, and
//!   the latency reporting built into walking them —
//!   [`menu_and_settings_dialog`](crate::scenarios::menu_and_settings_dialog),
//!   and
//!   [`rapid_tabbing_in_settings`](crate::scenarios::rapid_tabbing_in_settings),
//!   a burst of focus changes in the settings dialog that must leave focus
//!   where it really is, and
//!   [`synth_host_crash_recovery`](crate::scenarios::synth_host_crash_recovery),
//!   speech going on after the synthesizer host is killed, and
//!   [`switch_to_onecore`](crate::scenarios::switch_to_onecore), switching
//!   to Windows `OneCore` voices and back, and
//!   [`lock_key_announcements`](crate::scenarios::lock_key_announcements),
//!   a lock key's new state spoken, and
//!   [`theme_panel`](crate::scenarios::theme_panel), the settings dialog's
//!   Theme page, where a role changed to a sound is then heard as that
//!   sound, and
//!   [`terminal_settings_page`](crate::scenarios::terminal_settings_page),
//!   its Terminal page, whose applied change Verbatim+5 then finds in
//!   Core.
//! - [`Group::Shell`]: the Windows shell — switching foreground between
//!   applications (the "task switching" item `docs/roadmap.md`'s M3 E2E
//!   list names,
//!   [`second_application_and_verbatim_menu`](crate::scenarios::second_application_and_verbatim_menu)),
//!   an outpost replaced after it dies
//!   ([`outpost_crash_recovery`](crate::scenarios::outpost_crash_recovery)),
//!   a File Explorer window
//!   ([`explorer_folder_window`](crate::scenarios::explorer_folder_window)),
//!   and the Settings app's System page
//!   ([`settings_system_page`](crate::scenarios::settings_system_page)).
//! - [`Group::Navigation`]: the M3 object-navigation and review-cursor
//!   commands (`docs/roadmap.md`'s M3 section) —
//!   [`object_navigation_in_settings`](crate::scenarios::object_navigation_in_settings)
//!   against Verbatim's own settings dialog,
//!   [`object_navigation_over_uia`](crate::scenarios::object_navigation_over_uia)
//!   against `mockapp`'s UIA provider, and
//!   [`system_information_tree`](crate::scenarios::system_information_tree)
//!   against msinfo32's real Win32 tree view over MSAA, which is also the
//!   suite's MSAA-only legacy application (the M3 exit item).
//! - [`Group::Text`]: milestone M4's text — editing, word selection, typed
//!   word echo, and the review cursor, each in the Windows Forms text box
//!   (`text_box_`) and in Windows 11 Notepad (`notepad_`, local-only,
//!   below); say-all in each
//!   ([`text_box_say_all`](crate::scenarios::text_box_say_all) and
//!   `notepad_say_all`); spelling errors in `mockapp`'s scripted text and
//!   in Windows 11 Notepad
//!   ([`notepad_spelling_errors`](crate::scenarios::notepad_spelling_errors));
//!   and the terminal scenarios, each in Windows Terminal
//!   (`windows_terminal_`) and in the console host (`conhost_`):
//!   commands, spoken password, flood
//!   ([`terminal_commands`](crate::scenarios::terminal_commands),
//!   [`terminal_flood`](crate::scenarios::terminal_flood)), editing
//!   ([`terminal_editing`](crate::scenarios::terminal_editing)), review grid
//!   ([`terminal_review_grid`](crate::scenarios::terminal_review_grid)),
//!   progress ([`terminal_progress`](crate::scenarios::terminal_progress)),
//!   typing the terminal does not show plainly ([`terminal_typing`](crate::scenarios::terminal_typing)),
//!   short output
//!   ([`terminal_short_output`](crate::scenarios::terminal_short_output)),
//!   and two windows of each terminal and two tabs of Windows Terminal
//!   ([`terminal_windows`](crate::scenarios::terminal_windows)).
//! - [`Group::Demo`]: demonstrations, recorded as videos for
//!   `videos/demos` by `cargo xtask demo` and never part of the suite: a
//!   selection with no `--scenario` or `--group` leaves them out
//!   ([`select`]), `cargo xtask vm test` refuses them, and their `#[test]`
//!   wrappers are `#[ignore]`d, so a plain `cargo test -p verbatim-e2e`, as
//!   CI's `e2e` job runs it, skips them. Each still asserts what it shows,
//!   so a broken feature fails rather than recording a misleading video.
//!
//! A scenario marked [`ScenarioDef::local_only`] tests Windows 11 Notepad,
//! which GitHub's Windows Server runner, with classic Notepad, lacks; its
//! name starts with [`LOCAL_ONLY_PREFIX`]. Every local and VM run includes
//! it; GitHub's `e2e` job sets [`SKIP_LOCAL_ONLY_ENV`] and deselects it by
//! name (`--skip notepad_`), and [`run_named`] fails a local-only scenario
//! that runs anyway with the variable set, so a skip is never reported as
//! a pass. The skip comes from that setting alone, never from detecting
//! the machine.

use std::io;
use std::panic::{self, AssertUnwindSafe};

use verbatim_config::Settings;

use crate::artifacts::{self, ScenarioSummary};
use crate::scenario::{Document, Scenario};
use crate::scenarios::{
    demo_notepad_editing, demo_review_cursor, demo_say_all, demo_settings_dialog_keys,
    demo_terminal_session, editing, explorer_folder_window, lock_key_announcements,
    menu_and_settings_dialog, notepad_say_all, notepad_spelling_errors,
    object_navigation_in_settings, object_navigation_over_uia, outpost_crash_recovery,
    rapid_tabbing_in_settings, review_cursor, review_words, second_application_and_verbatim_menu,
    settings_dialog_keys, settings_system_page, spelling_errors, switch_to_onecore,
    synth_host_crash_recovery, system_information_tree, terminal_commands, terminal_editing,
    terminal_flood, terminal_flood_kinds, terminal_footer, terminal_key_timing, terminal_lists,
    terminal_overflow, terminal_pager, terminal_progress, terminal_review_grid,
    terminal_review_output, terminal_screens, terminal_settings_page, terminal_short_output,
    terminal_typing, terminal_windows, text_box_say_all, theme_panel, typed_words, word_selection,
};
use crate::speech::SpeechFailure;

/// Environment variable that, set to `1`, says the run has no Windows 11
/// Notepad, so the local-only scenarios ([`ScenarioDef::local_only`]) must
/// not run: GitHub's `e2e` job sets it and deselects them by name
/// (`--skip notepad_`), and one that runs anyway fails rather than
/// reporting a pass. Unset, empty, or `0` runs them. Nothing detects the
/// machine.
pub const SKIP_LOCAL_ONLY_ENV: &str = "VERBATIM_E2E_SKIP_LOCAL_ONLY";

/// What the name of every local-only scenario starts with, and no other
/// scenario's: the `--skip` filter a run without Windows 11 Notepad
/// deselects them by. Every local-only scenario tests Windows 11 Notepad.
pub const LOCAL_ONLY_PREFIX: &str = "notepad_";

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
    Ok(skip_local_only && def.local_only)
}

/// A coarse selector for `cargo xtask vm test --group` — see this module's
/// own doc comment for what each group is meant to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Verbatim's own menu and Speech settings dialog, and latency
    /// reporting.
    Speech,
    /// Switching foreground between applications ("task switching").
    Shell,
    /// Object navigation and review-cursor commands.
    Navigation,
    /// Text: editing, the review cursor over text, say-all, and terminals
    /// (milestone M4).
    Text,
    /// Demonstrations, recorded by `cargo xtask demo` and left out of the
    /// suite.
    Demo,
}

impl Group {
    /// The lowercase name used on the command line and in [`ScenarioDef`]
    /// listings — `cargo xtask vm test --list`'s own output, and
    /// [`select`]'s `--group` matching.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Speech => "speech",
            Self::Shell => "shell",
            Self::Navigation => "navigation",
            Self::Text => "text",
            Self::Demo => "demo",
        }
    }

    /// Parses a group name (case-sensitive, matching [`Group::name`]
    /// exactly), for `--group` argument validation.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "speech" => Some(Self::Speech),
            "shell" => Some(Self::Shell),
            "navigation" => Some(Self::Navigation),
            "text" => Some(Self::Text),
            "demo" => Some(Self::Demo),
            _ => None,
        }
    }
}

/// Scenario-specific state a [`ScenarioDef::setup`] creates and its matching
/// [`ScenarioDef::teardown`] restores. `None` when a scenario needs nothing
/// beyond `Scenario::launch` itself; `TargetPid` names a process launched via
/// [`Scenario::launch_titled`] that teardown must kill.
#[derive(Debug)]
pub enum ScenarioState {
    /// No extra state: `setup` did nothing beyond validating preconditions.
    None,
    /// The pid of a target application `setup` launched via
    /// [`Scenario::launch_titled`], for `teardown` to kill.
    TargetPid(u32),
    /// Names `setup` recorded for `teardown`, such as the folders that were
    /// there before the scenario.
    Names(Vec<String>),
    /// The title of a window `setup` opened, such as a harness folder's
    /// ([`Scenario::open_folder`]), for the body to listen for.
    Title(String),
    /// A window `setup` opened with a title of the run's own
    /// ([`Scenario::launch_titled`]), such as a terminal's.
    Window {
        /// The launch that opened it, for `teardown` to close it by its
        /// title ([`Scenario::kill_target`]).
        pid: u32,
        /// Its title.
        title: String,
        /// The folder, on the agent's machine, holding the files it uses.
        directory: String,
    },
}

/// One named, grouped scenario. See this module's own doc comment for the
/// setup/body/teardown contract and why [`name`](Self::name) is the single
/// identifier used everywhere.
#[derive(Debug)]
pub struct ScenarioDef {
    /// Selects this scenario: its `#[test]` function name
    /// (`cargo test -p verbatim-e2e <name> -- --exact`), its
    /// `cargo xtask vm test --scenario <name>` selector, its artifacts
    /// directory name, and its recording file name prefix.
    pub name: &'static str,
    /// The coarse group this scenario belongs to, for `--group` selection.
    pub group: Group,
    /// Changes the fixed settings this scenario's Verbatim is launched
    /// with ([`Scenario::launch_with`]), for a scenario that needs
    /// a reader setting other than its default; `None` for most.
    pub settings: Option<fn(&mut Settings)>,
    /// Whether this scenario needs something only a Windows 11 desktop
    /// has, such as Windows 11 Notepad's spell checker, so that it cannot
    /// hold on GitHub's Windows Server runner. Every local run and every
    /// VM run includes it; a run with [`SKIP_LOCAL_ONLY_ENV`] set to `1`,
    /// as GitHub's `e2e` job sets it, skips it ([`skips`]).
    pub local_only: bool,
    /// The document the scenario edits in Windows 11 Notepad, opened
    /// before Verbatim starts ([`crate::scenario::Document`]); `None` for
    /// most.
    pub document: Option<fn() -> Document>,
    /// Declares and creates whatever state `body` needs beyond
    /// `Scenario::launch` itself.
    ///
    /// # Errors
    ///
    /// Returns an error if creating that state fails (for example, the
    /// agent refuses to launch a target application).
    pub setup: fn(&mut Scenario) -> io::Result<ScenarioState>,
    /// The scripted walk: gestures, keys, and speech assertions. Panics
    /// (via `assert!`/`expect`/`panic!`, exactly like today's `#[test]`
    /// bodies) are how a scenario failure is reported; [`run`] catches them
    /// to collect failure artifacts before letting the panic continue.
    pub body: fn(&mut Scenario, &mut ScenarioState),
    /// Restores whatever `setup` created. Always runs, even when `body`
    /// panicked — see [`run`].
    pub teardown: fn(&mut Scenario, ScenarioState),
}

/// Every registered scenario, in a fixed, stable order (also the order
/// [`select`] preserves and `--list` prints in).
pub const SCENARIOS: &[ScenarioDef] = &[
    ScenarioDef {
        name: "menu_and_settings_dialog",
        group: Group::Speech,
        settings: None,
        local_only: false,
        document: None,
        setup: menu_and_settings_dialog::setup,
        body: menu_and_settings_dialog::body,
        teardown: menu_and_settings_dialog::teardown,
    },
    ScenarioDef {
        name: "second_application_and_verbatim_menu",
        group: Group::Shell,
        settings: None,
        local_only: false,
        document: None,
        setup: second_application_and_verbatim_menu::setup,
        body: second_application_and_verbatim_menu::body,
        teardown: second_application_and_verbatim_menu::teardown,
    },
    ScenarioDef {
        name: "rapid_tabbing_in_settings",
        group: Group::Speech,
        settings: None,
        local_only: false,
        document: None,
        setup: rapid_tabbing_in_settings::setup,
        body: rapid_tabbing_in_settings::body,
        teardown: rapid_tabbing_in_settings::teardown,
    },
    ScenarioDef {
        name: "object_navigation_in_settings",
        group: Group::Navigation,
        settings: None,
        local_only: false,
        document: None,
        setup: object_navigation_in_settings::setup,
        body: object_navigation_in_settings::body,
        teardown: object_navigation_in_settings::teardown,
    },
    ScenarioDef {
        name: "switch_to_onecore",
        group: Group::Speech,
        settings: None,
        local_only: false,
        document: None,
        setup: switch_to_onecore::setup,
        body: switch_to_onecore::body,
        teardown: switch_to_onecore::teardown,
    },
    ScenarioDef {
        name: "synth_host_crash_recovery",
        group: Group::Speech,
        settings: None,
        local_only: false,
        document: None,
        setup: synth_host_crash_recovery::setup,
        body: synth_host_crash_recovery::body,
        teardown: synth_host_crash_recovery::teardown,
    },
    ScenarioDef {
        name: "outpost_crash_recovery",
        group: Group::Shell,
        settings: None,
        local_only: false,
        document: None,
        setup: outpost_crash_recovery::setup,
        body: outpost_crash_recovery::body,
        teardown: outpost_crash_recovery::teardown,
    },
    ScenarioDef {
        name: "lock_key_announcements",
        group: Group::Speech,
        settings: None,
        local_only: false,
        document: None,
        setup: lock_key_announcements::setup,
        body: lock_key_announcements::body,
        teardown: lock_key_announcements::teardown,
    },
    ScenarioDef {
        name: "explorer_folder_window",
        group: Group::Shell,
        settings: None,
        local_only: false,
        document: None,
        setup: explorer_folder_window::setup,
        body: explorer_folder_window::body,
        teardown: explorer_folder_window::teardown,
    },
    ScenarioDef {
        name: "settings_dialog_keys",
        group: Group::Speech,
        settings: None,
        local_only: false,
        document: None,
        setup: settings_dialog_keys::setup,
        body: settings_dialog_keys::body,
        teardown: settings_dialog_keys::teardown,
    },
    ScenarioDef {
        name: "settings_system_page",
        group: Group::Shell,
        settings: None,
        local_only: false,
        document: None,
        setup: settings_system_page::setup,
        body: settings_system_page::body,
        teardown: settings_system_page::teardown,
    },
    ScenarioDef {
        name: "notepad_editing",
        group: Group::Text,
        settings: None,
        local_only: true,
        document: Some(editing::document),
        setup: editing::notepad_setup,
        body: editing::notepad_body,
        teardown: editing::teardown,
    },
    ScenarioDef {
        name: "text_box_editing",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: editing::text_box_setup,
        body: editing::text_box_body,
        teardown: editing::teardown,
    },
    ScenarioDef {
        name: "notepad_review_cursor",
        group: Group::Text,
        settings: None,
        local_only: true,
        document: Some(review_cursor::document),
        setup: review_cursor::notepad_setup,
        body: review_cursor::notepad_body,
        teardown: review_cursor::teardown,
    },
    ScenarioDef {
        name: "text_box_review_cursor",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: review_cursor::text_box_setup,
        body: review_cursor::text_box_body,
        teardown: review_cursor::teardown,
    },
    ScenarioDef {
        name: "notepad_say_all",
        group: Group::Text,
        settings: None,
        local_only: true,
        document: Some(notepad_say_all::document),
        setup: notepad_say_all::setup,
        body: notepad_say_all::body,
        teardown: notepad_say_all::teardown,
    },
    ScenarioDef {
        name: "notepad_word_selection",
        group: Group::Text,
        settings: None,
        local_only: true,
        document: Some(word_selection::document),
        setup: word_selection::notepad_setup,
        body: word_selection::notepad_body,
        teardown: word_selection::teardown,
    },
    ScenarioDef {
        name: "text_box_word_selection",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: word_selection::text_box_setup,
        body: word_selection::text_box_body,
        teardown: word_selection::teardown,
    },
    ScenarioDef {
        name: "notepad_typed_words",
        group: Group::Text,
        settings: None,
        local_only: true,
        document: Some(typed_words::document),
        setup: typed_words::notepad_setup,
        body: typed_words::notepad_body,
        teardown: typed_words::teardown,
    },
    ScenarioDef {
        name: "text_box_typed_words",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: typed_words::text_box_setup,
        body: typed_words::text_box_body,
        teardown: typed_words::teardown,
    },
    ScenarioDef {
        name: "notepad_review_words",
        group: Group::Text,
        settings: None,
        local_only: true,
        document: Some(review_words::document),
        setup: review_words::notepad_setup,
        body: review_words::notepad_body,
        teardown: review_words::teardown,
    },
    ScenarioDef {
        name: "text_box_review_words",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: review_words::text_box_setup,
        body: review_words::text_box_body,
        teardown: review_words::teardown,
    },
    ScenarioDef {
        name: "text_box_say_all",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: text_box_say_all::setup,
        body: text_box_say_all::body,
        teardown: text_box_say_all::teardown,
    },
    ScenarioDef {
        name: "spelling_errors",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: spelling_errors::setup,
        body: spelling_errors::body,
        teardown: spelling_errors::teardown,
    },
    // Windows 11 Notepad's own spell checker, which GitHub's Windows Server
    // runner does not have: its Notepad is classic Notepad, a Win32 edit
    // control. `spelling_errors` hears the same speech from mockapp's
    // scripted text everywhere.
    ScenarioDef {
        name: "notepad_spelling_errors",
        group: Group::Text,
        settings: None,
        local_only: true,
        document: Some(notepad_spelling_errors::document),
        setup: notepad_spelling_errors::setup,
        body: notepad_spelling_errors::body,
        teardown: notepad_spelling_errors::teardown,
    },
    ScenarioDef {
        name: "theme_panel",
        group: Group::Speech,
        settings: None,
        local_only: false,
        document: None,
        setup: theme_panel::setup,
        body: theme_panel::body,
        teardown: theme_panel::teardown,
    },
    ScenarioDef {
        name: "terminal_settings_page",
        group: Group::Speech,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_settings_page::setup,
        body: terminal_settings_page::body,
        teardown: terminal_settings_page::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_commands",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_commands::setup_windows_terminal,
        body: terminal_commands::body_windows_terminal,
        teardown: terminal_commands::teardown,
    },
    ScenarioDef {
        name: "conhost_commands",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_commands::setup_console_host,
        body: terminal_commands::body_console_host,
        teardown: terminal_commands::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_spoken_password",
        group: Group::Text,
        settings: Some(terminal_commands::speak_passwords),
        local_only: false,
        document: None,
        setup: terminal_commands::setup_spoken_password_windows_terminal,
        body: terminal_commands::body_spoken_password_windows_terminal,
        teardown: terminal_commands::teardown,
    },
    ScenarioDef {
        name: "conhost_spoken_password",
        group: Group::Text,
        settings: Some(terminal_commands::speak_passwords),
        local_only: false,
        document: None,
        setup: terminal_commands::setup_spoken_password_console_host,
        body: terminal_commands::body_spoken_password_console_host,
        teardown: terminal_commands::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_flood::setup_windows_terminal,
        body: terminal_flood::body_windows_terminal,
        teardown: terminal_flood::teardown,
    },
    ScenarioDef {
        name: "conhost_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_flood::setup_console_host,
        body: terminal_flood::body_console_host,
        teardown: terminal_flood::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_control_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_key_timing::setup_control_windows_terminal,
        body: terminal_key_timing::body_control_windows_terminal,
        teardown: terminal_key_timing::teardown,
    },
    ScenarioDef {
        name: "conhost_control_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_key_timing::setup_control_console_host,
        body: terminal_key_timing::body_control_console_host,
        teardown: terminal_key_timing::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_shift_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_key_timing::setup_shift_windows_terminal,
        body: terminal_key_timing::body_shift_windows_terminal,
        teardown: terminal_key_timing::teardown,
    },
    ScenarioDef {
        name: "conhost_shift_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_key_timing::setup_shift_console_host,
        body: terminal_key_timing::body_shift_console_host,
        teardown: terminal_key_timing::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_line_key_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_key_timing::setup_line_key_windows_terminal,
        body: terminal_key_timing::body_line_key_windows_terminal,
        teardown: terminal_key_timing::teardown,
    },
    ScenarioDef {
        name: "conhost_line_key_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_key_timing::setup_line_key_console_host,
        body: terminal_key_timing::body_line_key_console_host,
        teardown: terminal_key_timing::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_up_typing",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_key_timing::setup_up_typing_windows_terminal,
        body: terminal_key_timing::body_up_typing_windows_terminal,
        teardown: terminal_key_timing::teardown,
    },
    ScenarioDef {
        name: "conhost_up_typing",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_key_timing::setup_up_typing_console_host,
        body: terminal_key_timing::body_up_typing_console_host,
        teardown: terminal_key_timing::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_same_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_flood_kinds::setup_same_windows_terminal,
        body: terminal_flood_kinds::body_same_windows_terminal,
        teardown: terminal_flood_kinds::teardown,
    },
    ScenarioDef {
        name: "conhost_same_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_flood_kinds::setup_same_console_host,
        body: terminal_flood_kinds::body_same_console_host,
        teardown: terminal_flood_kinds::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_raised_flood",
        group: Group::Text,
        settings: Some(terminal_flood_kinds::raised_limits),
        local_only: false,
        document: None,
        setup: terminal_flood_kinds::setup_raised_windows_terminal,
        body: terminal_flood_kinds::body_raised_windows_terminal,
        teardown: terminal_flood_kinds::teardown,
    },
    ScenarioDef {
        name: "conhost_raised_flood",
        group: Group::Text,
        settings: Some(terminal_flood_kinds::raised_limits),
        local_only: false,
        document: None,
        setup: terminal_flood_kinds::setup_raised_console_host,
        body: terminal_flood_kinds::body_raised_console_host,
        teardown: terminal_flood_kinds::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_redraw_limit",
        group: Group::Text,
        settings: Some(terminal_flood_kinds::lowered_limits),
        local_only: false,
        document: None,
        setup: terminal_flood_kinds::setup_redraw_windows_terminal,
        body: terminal_flood_kinds::body_redraw_windows_terminal,
        teardown: terminal_flood_kinds::teardown,
    },
    ScenarioDef {
        name: "conhost_redraw_limit",
        group: Group::Text,
        settings: Some(terminal_flood_kinds::lowered_limits),
        local_only: false,
        document: None,
        setup: terminal_flood_kinds::setup_redraw_console_host,
        body: terminal_flood_kinds::body_redraw_console_host,
        teardown: terminal_flood_kinds::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_leave_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_windows::setup_leave_windows_terminal,
        body: terminal_windows::body_leave_windows_terminal,
        teardown: terminal_windows::teardown,
    },
    ScenarioDef {
        name: "conhost_leave_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_windows::setup_leave_console_host,
        body: terminal_windows::body_leave_console_host,
        teardown: terminal_windows::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_close_tab",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_windows::setup_close_tab,
        body: terminal_windows::body_close_tab,
        teardown: terminal_windows::teardown,
    },
    ScenarioDef {
        name: "conhost_wrapped_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_flood::setup_wrapped_console_host,
        body: terminal_flood::body_wrapped_console_host,
        teardown: terminal_flood::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_history_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_overflow::setup_windows_terminal,
        body: terminal_overflow::body_history_windows_terminal,
        teardown: terminal_overflow::teardown,
    },
    ScenarioDef {
        name: "conhost_history_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_overflow::setup_console_host,
        body: terminal_overflow::body_history_console_host,
        teardown: terminal_overflow::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_history_flood_during_group",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_overflow::setup_windows_terminal,
        body: terminal_overflow::body_history_during_group_windows_terminal,
        teardown: terminal_overflow::teardown,
    },
    ScenarioDef {
        name: "conhost_history_flood_during_group",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_overflow::setup_console_host,
        body: terminal_overflow::body_history_during_group_console_host,
        teardown: terminal_overflow::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_scrollback_overflow",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_overflow::setup_windows_terminal,
        body: terminal_overflow::body_overflow_windows_terminal,
        teardown: terminal_overflow::teardown,
    },
    ScenarioDef {
        name: "conhost_scrollback_overflow",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_overflow::setup_console_host,
        body: terminal_overflow::body_overflow_console_host,
        teardown: terminal_overflow::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_scrollback_overflow_during_group",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_overflow::setup_windows_terminal,
        body: terminal_overflow::body_overflow_during_group_windows_terminal,
        teardown: terminal_overflow::teardown,
    },
    ScenarioDef {
        name: "conhost_scrollback_overflow_during_group",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_overflow::setup_console_host,
        body: terminal_overflow::body_overflow_during_group_console_host,
        teardown: terminal_overflow::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_editing",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_editing::setup_windows_terminal,
        body: terminal_editing::body_windows_terminal,
        teardown: terminal_editing::teardown,
    },
    ScenarioDef {
        name: "conhost_editing",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_editing::setup_console_host,
        body: terminal_editing::body_console_host,
        teardown: terminal_editing::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_review_grid",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_review_grid::setup_windows_terminal,
        body: terminal_review_grid::body_windows_terminal,
        teardown: terminal_review_grid::teardown,
    },
    ScenarioDef {
        name: "conhost_review_grid",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_review_grid::setup_console_host,
        body: terminal_review_grid::body_console_host,
        teardown: terminal_review_grid::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_progress",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_progress::setup_windows_terminal,
        body: terminal_progress::body_windows_terminal,
        teardown: terminal_progress::teardown,
    },
    ScenarioDef {
        name: "conhost_progress",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_progress::setup_console_host,
        body: terminal_progress::body_console_host,
        teardown: terminal_progress::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_typing",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_typing::setup_windows_terminal,
        body: terminal_typing::body_windows_terminal,
        teardown: terminal_typing::teardown,
    },
    ScenarioDef {
        name: "conhost_typing",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_typing::setup_console_host,
        body: terminal_typing::body_console_host,
        teardown: terminal_typing::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_long_lines",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_screens::setup_windows_terminal,
        body: terminal_screens::body_long_lines_windows_terminal,
        teardown: terminal_screens::teardown,
    },
    ScenarioDef {
        name: "conhost_long_lines",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_screens::setup_console_host,
        body: terminal_screens::body_long_lines_console_host,
        teardown: terminal_screens::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_full_screen",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_screens::setup_windows_terminal,
        body: terminal_screens::body_full_screen_windows_terminal,
        teardown: terminal_screens::teardown,
    },
    ScenarioDef {
        name: "conhost_full_screen",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_screens::setup_console_host,
        body: terminal_screens::body_full_screen_console_host,
        teardown: terminal_screens::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_marker_list",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_lists::setup_windows_terminal,
        body: terminal_lists::body_marker_windows_terminal,
        teardown: terminal_lists::teardown,
    },
    ScenarioDef {
        name: "conhost_marker_list",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_lists::setup_console_host,
        body: terminal_lists::body_marker_console_host,
        teardown: terminal_lists::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_redrawn_list",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_lists::setup_windows_terminal,
        body: terminal_lists::body_redrawn_windows_terminal,
        teardown: terminal_lists::teardown,
    },
    ScenarioDef {
        name: "conhost_redrawn_list",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_lists::setup_console_host,
        body: terminal_lists::body_redrawn_console_host,
        teardown: terminal_lists::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_pager",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_pager::setup_windows_terminal,
        body: terminal_pager::body_windows_terminal,
        teardown: terminal_pager::teardown,
    },
    ScenarioDef {
        name: "conhost_pager",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_pager::setup_console_host,
        body: terminal_pager::body_console_host,
        teardown: terminal_pager::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_review_output",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_review_output::setup_windows_terminal,
        body: terminal_review_output::body_windows_terminal,
        teardown: terminal_review_output::teardown,
    },
    ScenarioDef {
        name: "conhost_review_output",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_review_output::setup_console_host,
        body: terminal_review_output::body_console_host,
        teardown: terminal_review_output::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_footer_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_footer::setup_windows_terminal,
        body: terminal_footer::body_windows_terminal,
        teardown: terminal_footer::teardown,
    },
    ScenarioDef {
        name: "conhost_footer_flood",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_footer::setup_console_host,
        body: terminal_footer::body_console_host,
        teardown: terminal_footer::teardown,
    },
    ScenarioDef {
        name: "conhost_footer_overflow",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_footer::setup_overflow_console_host,
        body: terminal_footer::body_overflow_console_host,
        teardown: terminal_footer::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_short_output",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_short_output::setup_windows_terminal,
        body: terminal_short_output::body_windows_terminal,
        teardown: terminal_short_output::teardown,
    },
    ScenarioDef {
        name: "conhost_short_output",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_short_output::setup_console_host,
        body: terminal_short_output::body_console_host,
        teardown: terminal_short_output::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_two_windows",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_windows::setup_windows_terminal,
        body: terminal_windows::body_two_windows_terminal,
        teardown: terminal_windows::teardown,
    },
    ScenarioDef {
        name: "conhost_two_windows",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_windows::setup_console_host,
        body: terminal_windows::body_two_console_host,
        teardown: terminal_windows::teardown,
    },
    ScenarioDef {
        name: "windows_terminal_tabs",
        group: Group::Text,
        settings: None,
        local_only: false,
        document: None,
        setup: terminal_windows::setup_tabs,
        body: terminal_windows::body_tabs,
        teardown: terminal_windows::teardown,
    },
    ScenarioDef {
        name: "object_navigation_over_uia",
        group: Group::Navigation,
        settings: None,
        local_only: false,
        document: None,
        setup: object_navigation_over_uia::setup,
        body: object_navigation_over_uia::body,
        teardown: object_navigation_over_uia::teardown,
    },
    ScenarioDef {
        name: "system_information_tree",
        group: Group::Navigation,
        settings: None,
        local_only: false,
        document: None,
        setup: system_information_tree::setup,
        body: system_information_tree::body,
        teardown: system_information_tree::teardown,
    },
    ScenarioDef {
        name: "demo_notepad_editing",
        group: Group::Demo,
        settings: None,
        local_only: false,
        document: Some(demo_notepad_editing::document),
        setup: demo_notepad_editing::setup,
        body: demo_notepad_editing::body,
        teardown: demo_notepad_editing::teardown,
    },
    ScenarioDef {
        name: "demo_review_cursor",
        group: Group::Demo,
        settings: None,
        local_only: false,
        document: Some(demo_review_cursor::document),
        setup: demo_review_cursor::setup,
        body: demo_review_cursor::body,
        teardown: demo_review_cursor::teardown,
    },
    ScenarioDef {
        name: "demo_say_all",
        group: Group::Demo,
        settings: None,
        local_only: false,
        document: Some(notepad_say_all::document),
        setup: demo_say_all::setup,
        body: demo_say_all::body,
        teardown: demo_say_all::teardown,
    },
    ScenarioDef {
        name: "demo_terminal_session",
        group: Group::Demo,
        settings: None,
        local_only: false,
        document: None,
        setup: demo_terminal_session::setup,
        body: demo_terminal_session::body,
        teardown: demo_terminal_session::teardown,
    },
    ScenarioDef {
        name: "demo_settings_dialog_keys",
        group: Group::Demo,
        settings: None,
        local_only: false,
        document: None,
        setup: demo_settings_dialog_keys::setup,
        body: demo_settings_dialog_keys::body,
        teardown: demo_settings_dialog_keys::teardown,
    },
];

/// Looks up a scenario or a demonstration by [`ScenarioDef::name`].
#[must_use]
pub fn find(name: &str) -> Option<&'static ScenarioDef> {
    SCENARIOS.iter().find(|def| def.name == name)
}

/// Resolves `--scenario` and `--group` selections against `scenarios`
/// (always [`SCENARIOS`] outside tests) into an ordered, deduplicated list
/// of matching definitions, preserving registry order. Empty `names` and
/// `groups` selects every scenario but the demonstrations
/// ([`Group::Demo`]) — the default, no-flags behavior of
/// `cargo xtask vm test`.
///
/// # Errors
///
/// Returns the first unrecognized scenario name or group name as `Err`, so a
/// typo is reported before anything is built or restored, not discovered
/// partway through a run.
pub fn select<'a>(
    scenarios: &'a [ScenarioDef],
    names: &[String],
    groups: &[String],
) -> Result<Vec<&'a ScenarioDef>, String> {
    if names.is_empty() && groups.is_empty() {
        return Ok(scenarios
            .iter()
            .filter(|def| def.group != Group::Demo)
            .collect());
    }

    let mut parsed_groups = Vec::with_capacity(groups.len());
    for group in groups {
        parsed_groups
            .push(Group::parse(group).ok_or_else(|| format!("unknown scenario group {group:?}"))?);
    }
    for name in names {
        if !scenarios.iter().any(|def| def.name == name) {
            return Err(format!("unknown scenario {name:?}"));
        }
    }

    Ok(scenarios
        .iter()
        .filter(|def| {
            names.iter().any(|name| name == def.name) || parsed_groups.contains(&def.group)
        })
        .collect())
}

/// The speech every scenario starts with: Verbatim's start sound, heard in
/// full. Verbatim speaks no start message, as NVDA speaks none
/// (`docs/parity.md`, "Starting"); the announcement of the desktop, which
/// has the focus once every window is minimized (`Scenario::launch`),
/// follows, asserted after it.
#[must_use]
pub fn startup_speech() -> [crate::speech::Expected; 1] {
    [crate::speech::heard("sound: start")]
}

/// Looks up `name` and runs it: the body of every `#[test]` wrapper under
/// `crates/verbatim-e2e/tests/`. The wrappers are `#[ignore]`d, so a
/// workspace test run lists them as ignored rather than running them, and
/// the end-to-end job runs them with `--ignored`.
///
/// # Panics
///
/// Panics if `name` is not registered; if [`crate::ENDPOINT_ENV`] is unset,
/// since a live scenario that cannot reach an agent has not passed; if the
/// scenario is local-only and [`SKIP_LOCAL_ONLY_ENV`] is `1`, since such a
/// run deselects it by name instead (a local-only scenario's name starts
/// with [`LOCAL_ONLY_PREFIX`]); and if the scenario itself fails.
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
            "{name:?} runs only on a Windows 11 desktop, and {SKIP_LOCAL_ONLY_ENV} is 1: a run without Windows 11 Notepad deselects the local-only scenarios with --skip {LOCAL_ONLY_PREFIX} rather than running them"
        ),
        Ok(false) => {}
        Err(error) => panic!("{error}"),
    }
    assert!(
        crate::endpoint().is_some(),
        "{} is not set: a live scenario needs a running agent (docs/tooling.md)",
        crate::ENDPOINT_ENV
    );
    run(def);
}

/// Runs one scenario end to end and fails it, after collecting everything,
/// if anything went wrong.
///
/// The order: launch Verbatim from the minimized desktop; assert its
/// startup speech ([`startup_speech`]); run setup and the body, which ends
/// by asserting that nothing more was said; save Core's focus and the
/// flight recorder while Verbatim is up; check that none of Verbatim's own
/// processes exited unexpectedly; quit Verbatim and check it exited with
/// code 0; run the scenario's teardown; close everything the scenario
/// opened; collect the logs, latency report, audio, crash dumps, and video;
/// write the summary and archive the run. Each step after the body runs
/// whatever happened before it, and every problem it meets is a failure of
/// the run, reported together with the body's own.
#[expect(
    clippy::too_many_lines,
    reason = "one run in order, each step's problems collected, so a failure's place in the order is plain"
)]
fn run(def: &ScenarioDef) {
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

    let mut scenario = Scenario::launch_with(def.settings, def.document.map(|document| document()))
        .unwrap_or_else(|error| {
            panic!(
                "scenario {:?}: launching Verbatim through the agent failed: {error}",
                def.name
            )
        });
    let mut foreground = vec![format!("before setup: {}", scenario.foreground_report())];
    let mut problems: Vec<String> = Vec::new();

    let mut state = None;
    let body_outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        scenario.speech().expect_sequence(&startup_speech());
        crate::scenarios::expect_desktop(&mut scenario);
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
    }));

    let latency = match scenario.latency_snapshot(LATENCY_RECORDS) {
        Ok(latency) => Some(latency),
        Err(error) => {
            problems.push(format!("could not fetch the latency timelines: {error}"));
            None
        }
    };
    if let Err(error) = scenario.collect_focus(&dir) {
        problems.push(format!("could not save Core's focus: {error}"));
    }
    if let Err(error) = scenario.collect_flight_recorder(&dir) {
        problems.push(format!("could not save the flight recorder: {error}"));
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
        Ok(()) => match scenario.outposts_shut_down_cleanly() {
            Ok(found) => problems.extend(found),
            Err(error) => problems.push(format!(
                "could not check that Verbatim's outposts shut down cleanly: {error}"
            )),
        },
        Err(error) => problems.push(format!("quitting Verbatim failed: {error}")),
    }
    let teardown_outcome = state.map(|state| {
        panic::catch_unwind(AssertUnwindSafe(|| (def.teardown)(&mut scenario, state)))
    });
    if let Some(Err(payload)) = &teardown_outcome {
        problems.push(format!(
            "teardown failed: {}",
            panic_message(payload.as_ref())
        ));
    }
    problems.extend(scenario.clean_up());
    problems.extend(scenario.collect_run_artifacts(&dir));
    // A failed speech assertion is reported once the logs holding its
    // step's trace are collected.
    let speech_report = body_outcome
        .as_ref()
        .err()
        .and_then(|payload| payload.downcast_ref::<SpeechFailure>())
        .map(|failure| {
            let lines = failure.trace.map_or_else(
                || Ok(Vec::new()),
                |trace| artifacts::trace_lines(&dir, trace).map_err(|error| error.to_string()),
            );
            failure.report(
                scenario.timeline(),
                lines.as_deref().map_err(String::as_str),
            )
        });
    if let Err(error) = scenario.finish_recording(&dir.join(format!("{}.mp4", def.name))) {
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
    foreground.push(format!("after cleanup: {}", scenario.foreground_report()));
    drop(scenario);
    if let Err(error) = write_foreground(&dir, &foreground) {
        problems.push(format!("could not write the foreground record: {error}"));
    }

    let passed = body_outcome.is_ok() && problems.is_empty();
    if let Err(error) = ScenarioSummary::new(def.name, passed, latency.as_deref()).write(&dir) {
        problems.push(format!("could not write the run summary: {error}"));
    }
    if let Err(error) = artifacts::archive_run(&artifacts::artifacts_root(), def.name, &dir, passed)
    {
        problems.push(format!("could not archive the run's artifacts: {error}"));
    }
    println!(
        "scenario {:?}: {}",
        def.name,
        if body_outcome.is_ok() && problems.is_empty() {
            "pass"
        } else {
            "fail"
        }
    );
    let problems = problems.join("\n");
    if let Err(payload) = body_outcome {
        if !problems.is_empty() {
            eprintln!(
                "scenario {:?}: also failed after its body:\n{problems}",
                def.name
            );
        }
        if let Some(report) = speech_report {
            panic!("{report}");
        }
        panic::resume_unwind(payload);
    }
    assert!(
        problems.is_empty(),
        "scenario {:?} failed after its body:\n{problems}",
        def.name
    );
}

/// How many latency timelines a scenario's summary is built from.
const LATENCY_RECORDS: u32 = 200;

/// The message a panic carried, for reporting it beside other problems.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
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

        vec![
            ScenarioDef {
                name: "alpha",
                group: Group::Speech,
                settings: None,
                local_only: false,
                document: None,
                setup: no_setup,
                body: no_body,
                teardown: no_teardown,
            },
            ScenarioDef {
                name: "beta",
                group: Group::Shell,
                settings: None,
                local_only: false,
                document: None,
                setup: no_setup,
                body: no_body,
                teardown: no_teardown,
            },
            ScenarioDef {
                name: "gamma",
                group: Group::Shell,
                settings: None,
                local_only: false,
                document: None,
                setup: no_setup,
                body: no_body,
                teardown: no_teardown,
            },
        ]
    }

    #[test]
    fn group_parse_round_trips_every_variant_name() {
        for group in [
            Group::Speech,
            Group::Shell,
            Group::Navigation,
            Group::Text,
            Group::Demo,
        ] {
            assert_eq!(Group::parse(group.name()), Some(group));
        }
    }

    #[test]
    fn group_parse_rejects_unknown_names() {
        assert_eq!(Group::parse("nonsense"), None);
        assert_eq!(Group::parse("Speech"), None, "matching is case-sensitive");
    }

    #[test]
    fn find_locates_a_real_registered_scenario() {
        assert!(find("menu_and_settings_dialog").is_some());
        assert!(find("second_application_and_verbatim_menu").is_some());
        assert!(find("no_such_scenario").is_none());
    }

    #[test]
    fn select_with_no_filters_returns_every_scenario_in_order() {
        let scenarios = fixture();
        let selected = select(&scenarios, &[], &[]).expect("no filters never errors");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn select_with_no_filters_leaves_the_demonstrations_out() {
        let mut scenarios = fixture();
        scenarios[2].group = Group::Demo;
        let selected = select(&scenarios, &[], &[]).expect("no filters never errors");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        let demos = select(&scenarios, &[], &["demo".to_owned()]).expect("demo is a real group");
        assert_eq!(demos.len(), 1);
    }

    #[test]
    fn select_by_name_picks_exactly_that_scenario() {
        let scenarios = fixture();
        let selected =
            select(&scenarios, &["beta".to_owned()], &[]).expect("beta is a real scenario");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(names, vec!["beta"]);
    }

    #[test]
    fn select_by_group_picks_every_scenario_in_that_group() {
        let scenarios = fixture();
        let selected =
            select(&scenarios, &[], &["shell".to_owned()]).expect("shell is a real group");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(names, vec!["beta", "gamma"]);
    }

    #[test]
    fn select_unions_names_and_groups_without_duplicating_overlap() {
        let scenarios = fixture();
        let selected = select(&scenarios, &["beta".to_owned()], &["shell".to_owned()])
            .expect("both beta and shell are real");
        let names: Vec<&str> = selected.iter().map(|def| def.name).collect();
        assert_eq!(
            names,
            vec!["beta", "gamma"],
            "beta must not appear twice just because it matches both the name and the group filter"
        );
    }

    #[test]
    fn select_rejects_an_unknown_scenario_name() {
        let scenarios = fixture();
        let error = select(&scenarios, &["no_such_scenario".to_owned()], &[])
            .expect_err("unknown scenario name must be rejected");
        assert!(error.contains("no_such_scenario"));
    }

    #[test]
    fn select_rejects_an_unknown_group_name() {
        let scenarios = fixture();
        let error = select(&scenarios, &[], &["no_such_group".to_owned()])
            .expect_err("unknown group name must be rejected");
        assert!(error.contains("no_such_group"));
    }

    #[test]
    fn the_skip_setting_skips_exactly_the_local_only_scenarios() {
        let mut scenarios = fixture();
        scenarios[1].local_only = true;
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
        scenarios[1].local_only = true;
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
    fn exactly_the_local_only_scenarios_are_named_for_the_skip_filter() {
        for def in SCENARIOS {
            assert_eq!(
                def.local_only,
                def.name.starts_with(LOCAL_ONLY_PREFIX),
                "{}: a local-only scenario, and only one, is named with {LOCAL_ONLY_PREFIX:?}, which a run without Windows 11 Notepad skips",
                def.name
            );
        }
        let selected = select(SCENARIOS, &[], &[]).expect("no filters never errors");
        assert!(
            selected.iter().any(|def| def.local_only),
            "a run with no filters includes the local-only scenarios"
        );
    }

    #[test]
    fn a_local_only_scenario_run_where_it_cannot_hold_fails_instead_of_passing() {
        let local = SCENARIOS
            .iter()
            .find(|def| def.local_only)
            .expect("there are local-only scenarios");
        assert_eq!(skips(local, Some("1")), Ok(true));
    }
}
