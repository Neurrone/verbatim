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
//! new tab's terminal, "<title> terminal", and then its prompt as output,
//! and Verbatim the same: a Windows Terminal window or tab with nothing
//! written yet says no line as it takes the focus, as in NVDA, where the
//! console host's says "blank" in both. A scenario's first window shows
//! Windows PowerShell's notice that it leaves `PSReadLine` out above the
//! caret's blank row, so it says "blank"; the second window and the second
//! tab start their shells without it (`-NonInteractive`) and say no
//! line.
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
//!
//! `*_leave_flood` leaves a terminal during a flood and returns. `away.ps1`
//! writes "flood line 1" to "flood line 100", writes the file `half`, and
//! waits for the file `more`; while its first line plays, the second
//! window is brought forward, which cuts that line and the two queued
//! behind it off and says the second window, its terminal and prompt.
//! Then `more` lets the flood write lines 101 to 2000, and the file
//! `written`, and wait for the file `end`: nothing of it is said while the
//! terminal is in the background. Back in the first window, Verbatim says
//! the window, the terminal and the caret's line, the blank row below the
//! flood: what the flood wrote meanwhile is not new output. `end` lets the
//! script finish, and the prompt is heard. NVDA, captured live on
//! 2026-10-09, speaks the flood's lines it had queued, until the focus
//! moves, and says the same on returning.
//!
//! `windows_terminal_close_tab` closes the second tab with
//! Control+Shift+W: the first tab's terminal and its line, the prompt, are
//! said. NVDA, captured live, said the terminal without its line, which
//! it says in other captures of the focus returning to a tab; Verbatim
//! keeps the line (`phase6-design.md`, decisions of 2026-10-09).

use std::io;

use super::terminal::{self, PROMPT};
use super::terminal_flood::{FLOOD_STEP, line};
use super::terminal_key_timing::FILE_SIGNALS;
use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Ending;

/// The flood that waits halfway, and again at its end.
const AWAY_SCRIPT: &str = "for ($line = 1; $line -le 100; $line++) { \"flood line $line\" }\r\n\
Mark half\r\n\
Wait-For more\r\n\
for ($line = 101; $line -le 2000; $line++) { \"flood line $line\" }\r\n\
Mark written\r\n\
Wait-For end\r\n";

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

pub(crate) fn setup_close_tab(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal_for_tabs(scenario, "close-tab")
}

fn away_scripts() -> String {
    format!("{FILE_SIGNALS}{AWAY_SCRIPT}")
}

pub(crate) fn setup_leave_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "leave-a", &[("away.ps1", &away_scripts())])
}

pub(crate) fn setup_leave_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "leave-a", &[("away.ps1", &away_scripts())])
}

/// The flood started in the first window, `first`, left for the second,
/// whose announcement is `second_back`, continued and finished while away,
/// and returned to, announced as `first_back`.
fn leave_steps(
    scenario: &mut Scenario,
    (first, directory): (&str, &str),
    second: &str,
    (first_back, second_back): (&[&str], &[&str]),
) {
    terminal::type_with_echo(scenario, r".\away.ps1", terminal::Echo::Shown);
    scenario
        .wait_for_agent_file(&format!(r"{directory}\half"), FLOOD_STEP)
        .expect("the flood writes its first hundred lines");
    let playing = scenario.speech().expect_started(&line(1));
    let queued = scenario.speech().expect_queued(&[&line(2), &line(3)]);
    scenario
        .bring_window_forward(second)
        .expect("brings the second window forward");
    scenario.speech().expect_ended(&playing, Ending::Cancelled);
    for heard in &queued {
        scenario.speech().expect_ended(heard, Ending::Cancelled);
    }
    scenario.speech().expect(second_back);
    scenario
        .write_agent_file(&format!(r"{directory}\more"), b"")
        .expect("lets the flood go on");
    scenario
        .wait_for_agent_file(&format!(r"{directory}\written"), FLOOD_STEP)
        .expect("the flood writes the rest");
    scenario.expect_nothing_more();
    scenario
        .bring_window_forward(first)
        .expect("brings the first window back");
    scenario.speech().expect(first_back);
    scenario
        .write_agent_file(&format!(r"{directory}\end"), b"")
        .expect("lets the script end");
    scenario.speech().expect(&[PROMPT]);
}

/// The folder of the scenario's shell.
fn directory(state: &ScenarioState) -> String {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("a terminal scenario's setup opens a terminal window");
    };
    directory.clone()
}

/// `conhost_leave_flood`.
pub(crate) fn body_leave_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let first = terminal::title(state).to_owned();
    let directory = directory(state);
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{first} window"), "terminal", "blank"],
    );
    let second_state = terminal::open_console_host(scenario, "leave-b", &[])
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
    leave_steps(
        scenario,
        (&first, &directory),
        &second,
        (
            &[&format!("{first} window"), "terminal", "blank"],
            &[&format!("{second} window"), "terminal", PROMPT],
        ),
    );
}

/// `windows_terminal_leave_flood`.
pub(crate) fn body_leave_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let first = terminal::title(state).to_owned();
    let directory = directory(state);
    terminal::expect_prompt_read(
        scenario,
        state,
        &[
            &format!("{first} window"),
            &format!("{first} terminal"),
            "blank",
        ],
    );
    let second_state = terminal::open_windows_terminal_window(scenario, "leave-b", state)
        .expect("opens a second Windows Terminal window");
    let second = terminal::title(&second_state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        &second_state,
        &[&format!("{second} window"), &format!("{second} terminal")],
    );
    scenario
        .bring_window_forward(&first)
        .expect("brings the first window forward");
    scenario.speech().expect(&[
        &format!("{first} window"),
        &format!("{first} terminal"),
        PROMPT,
    ]);
    leave_steps(
        scenario,
        (&first, &directory),
        &second,
        (
            &[
                &format!("{first} window"),
                &format!("{first} terminal"),
                "blank",
            ],
            &[
                &format!("{second} window"),
                &format!("{second} terminal"),
                PROMPT,
            ],
        ),
    );
}

/// `windows_terminal_close_tab`: the second tab closed with
/// Control+Shift+W.
pub(crate) fn body_close_tab(scenario: &mut Scenario, state: &mut ScenarioState) {
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
    let second_state = terminal::open_windows_terminal_tab(scenario, "close-tab-two", state)
        .expect("opens a second tab");
    let second = terminal::title(&second_state).to_owned();
    terminal::expect_prompt_read(scenario, &second_state, &[&format!("{second} terminal")]);
    scenario
        .send_keys(&["control+shift+w"])
        .expect("closes the second tab");
    scenario
        .speech()
        .expect(&[&format!("{first} terminal"), PROMPT]);
    terminal::type_with_echo(scenario, "echo hi", terminal::Echo::Shown);
    scenario.speech().expect(&["hi", PROMPT]);
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
        &[&format!("{second} window"), &format!("{second} terminal")],
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
    terminal::expect_prompt_read(scenario, &second_state, &[&format!("{second} terminal")]);
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
