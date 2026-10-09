//! Two windows of each terminal, and two tabs of Windows Terminal, switched
//! between and typed into (`phase6-design.md`, "Terminal reading by
//! diffing the screen"): `conhost_two_windows`, `windows_terminal_two_windows`
//! and `windows_terminal_tabs`. The console host has no tabs. The shared
//! setup is described in the `terminal` module.
//!
//! NVDA was captured first on each (2026-10-09, an NVDA alpha build,
//! alpha-57645, with the same start script). Switching to another window,
//! as clicking its taskbar button does, NVDA says the window's title and
//! "window", then the terminal with the line the caret is on: "<title>
//! terminal ready>" in Windows Terminal, whose text area is named for its
//! tab, and "terminal ready>" in the console host, whose text area's name
//! NVDA drops. Verbatim says the same, the terminal and its line as two
//! utterances.
//!
//! The second tab is opened in the running window from the command line
//! (`WindowsTerminal.exe -w 0 new-tab`), as the first window was: opened
//! together, the window's title is the first tab's or the second's
//! depending on when Windows Terminal reports it in front. NVDA says the
//! new tab's terminal, "<title> terminal", and then its prompt as output;
//! Verbatim says the same with an extra "blank" for the empty line before
//! the prompt, as when a window opens. NVDA says no "blank" there: a known
//! difference being fixed (Dickson, 2026-10-09, coherence review), which
//! this scenario asserts until the fix lands.
//!
//! Switching tabs with Control+Tab or Control+Shift+Tab, NVDA says "list"
//! and the tab ("<title> 1 of 2"), which holds the keyboard focus until
//! Windows Terminal moves it to the tab's terminal, and then "<title>
//! terminal" with the line. Verbatim says only where the focus lands,
//! "<title> terminal" and the line: it judges a focus event by where the
//! keyboard focus is when its outpost handles it, a few milliseconds
//! later, where NVDA judges it as its event thread receives it, so the tab
//! is already behind it. This difference was recorded as deliberate and is
//! withdrawn (Dickson, 2026-10-09, coherence review): Control+Tab is being
//! changed to announce the tab as NVDA does, and this scenario asserts
//! today's speech until then (`docs/parity.md`).
//! Typing after a switch is echoed and its output spoken as in any
//! terminal.

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "two-a", &[])
}

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "two-a", &[])
}

pub(crate) fn setup_tabs(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal_for_tabs(scenario, "tabs")
}

/// `conhost_two_windows`.
pub(crate) fn body_two_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let first = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{first} window"), "terminal", "blank"],
    );
    let second_state = terminal::open_console_host(scenario, "two-b", &[])
        .expect("opens the second console window");
    let second = terminal::title(&second_state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        &second_state,
        &[&format!("{second} window"), "terminal", "blank"],
    );
    scenario
        .bring_window_forward(&first)
        .expect("brings the first window forward");
    scenario
        .speech()
        .expect(&[&format!("{first} window"), "terminal", PROMPT]);
    terminal::type_with_echo(scenario, "echo hi", terminal::Echo::Shown);
    scenario.speech().expect(&["hi", PROMPT]);
    scenario
        .bring_window_forward(&second)
        .expect("brings the second window forward");
    scenario
        .speech()
        .expect(&[&format!("{second} window"), "terminal", PROMPT]);
    terminal::type_with_echo(scenario, "echo yo", terminal::Echo::Shown);
    scenario.speech().expect(&["yo", PROMPT]);
}

/// `windows_terminal_two_windows`.
pub(crate) fn body_two_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let first = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[
            &format!("{first} window"),
            &format!("{first} terminal"),
            "blank",
        ],
    );
    let second_state = terminal::open_windows_terminal_window(scenario, "two-b", state)
        .expect("opens a second Windows Terminal window");
    let second = terminal::title(&second_state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        &second_state,
        &[
            &format!("{second} window"),
            &format!("{second} terminal"),
            "blank",
        ],
    );
    scenario
        .bring_window_forward(&first)
        .expect("brings the first window forward");
    scenario.speech().expect(&[
        &format!("{first} window"),
        &format!("{first} terminal"),
        PROMPT,
    ]);
    terminal::type_with_echo(scenario, "echo hi", terminal::Echo::Shown);
    scenario.speech().expect(&["hi", PROMPT]);
    scenario
        .bring_window_forward(&second)
        .expect("brings the second window forward");
    scenario.speech().expect(&[
        &format!("{second} window"),
        &format!("{second} terminal"),
        PROMPT,
    ]);
    terminal::type_with_echo(scenario, "echo yo", terminal::Echo::Shown);
    scenario.speech().expect(&["yo", PROMPT]);
}

/// `windows_terminal_tabs`: a second tab is opened in the window.
pub(crate) fn body_tabs(scenario: &mut Scenario, state: &mut ScenarioState) {
    let first = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[
            &format!("{first} window"),
            &format!("{first} terminal"),
            "blank",
        ],
    );
    let second_state = terminal::open_windows_terminal_tab(scenario, "tabs-two", state)
        .expect("opens a second tab");
    let second = terminal::title(&second_state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        &second_state,
        &[&format!("{second} terminal"), "blank"],
    );
    scenario
        .send_keys(&["control+shift+tab"])
        .expect("switches to the first tab");
    scenario
        .speech()
        .expect(&[&format!("{first} terminal"), PROMPT]);
    terminal::type_with_echo(scenario, "echo hi", terminal::Echo::Shown);
    scenario.speech().expect(&["hi", PROMPT]);
    scenario
        .send_keys(&["control+tab"])
        .expect("switches to the second tab");
    scenario
        .speech()
        .expect(&[&format!("{second} terminal"), PROMPT]);
    terminal::type_with_echo(scenario, "echo yo", terminal::Echo::Shown);
    scenario.speech().expect(&["yo", PROMPT]);
}
