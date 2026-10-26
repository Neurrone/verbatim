//! "Report status bar" in a terminal (`docs/design/terminal-model.md`,
//! "Features and settings"), as `windows_terminal_status_bar` in Windows
//! Terminal and `conhost_status_bar` in the console host. The shared setup
//! is described in the `terminal` module.
//!
//! The written script `footer.ps1` clears the screen, keeps its last row
//! for a footer, "status: busy", with a scroll region over the rows above,
//! and waits for a key: only the footer is heard, the cleared screen's
//! blank rows above it being no output. A key read without showing it
//! writes "output line 1" to "output line 3" into the region in one write,
//! fewer than a screen, and the script waits again; once the three lines
//! are heard, Verbatim+End reads the footer below the caret's row as
//! "status: busy". A second key redraws the footer as "status: done", said
//! only as it changed, "done", and Verbatim+End then reads "status: done".
//! A third key removes the region, clears the screen and prints "footer
//! gone"; with nothing below the caret and no row a program rewrote,
//! Verbatim+End reads the prompt's line, the last with text. No flood
//! runs above the footer, because a read taken while output moves the
//! document's end can leave the footer out (`docs/design/terminal-model.md`,
//! "Limits"). NVDA has no terminal status bar, so each press is Verbatim's
//! own (`docs/parity/terminals.md`, "Reading a terminal on demand").

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The footer below a scroll region, with three lines written above it.
const SCRIPT: &str = "Add-Type -TypeDefinition @\"\r\n\
using System;\r\n\
using System.Runtime.InteropServices;\r\n\
public static class Vt {\r\n\
\x20   [DllImport(\"kernel32.dll\")] static extern IntPtr GetStdHandle(int n);\r\n\
\x20   [DllImport(\"kernel32.dll\")] static extern bool GetConsoleMode(IntPtr h, out int m);\r\n\
\x20   [DllImport(\"kernel32.dll\")] static extern bool SetConsoleMode(IntPtr h, int m);\r\n\
\x20   public static void Enable() { var h = GetStdHandle(-11); int m; GetConsoleMode(h, out m); SetConsoleMode(h, m | 4); }\r\n\
}\r\n\
\"@\r\n\
[Vt]::Enable()\r\n\
$e = [char]27\r\n\
$rows = [Console]::WindowHeight\r\n\
[Console]::Write(\"$e[2J$e[1;$($rows - 1)r$e[$rows;1Hstatus: busy$e[$($rows - 1);1H\")\r\n\
$null = [Console]::ReadKey($true)\r\n\
[Console]::Write(\"`noutput line 1`noutput line 2`noutput line 3\")\r\n\
$null = [Console]::ReadKey($true)\r\n\
[Console]::Write(\"$e[s$e[$rows;1Hstatus: done$e[u\")\r\n\
$null = [Console]::ReadKey($true)\r\n\
[Console]::Write(\"$e[r$e[2J$e[H\")\r\n\
'footer gone'\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "status-bar", &[("footer.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "status-bar", &[("footer.ps1", SCRIPT)])
}

/// Presses "Report status bar" and asserts that exactly `heard` is read.
fn status_bar(scenario: &mut Scenario, heard: &str) {
    scenario
        .send_gesture("kb:verbatim+end")
        .expect("sends report status bar");
    scenario.deviates_from_nvda("docs/parity/terminals.md", "Reading a terminal on demand");
    scenario.speech().expect(&[heard]);
}

/// The steps once the prompt has been read.
fn steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\footer.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["status: busy"]);
    scenario.send_keys(&["n"]).expect("presses n");
    scenario
        .speech()
        .expect(&["output line 1", "output line 2", "output line 3"]);
    status_bar(scenario, "status: busy");
    scenario.send_keys(&["n"]).expect("presses n");
    scenario.speech().expect(&["done"]);
    status_bar(scenario, "status: done");
    scenario.send_keys(&["n"]).expect("presses n");
    scenario.speech().expect(&["footer gone", PROMPT]);
    status_bar(scenario, PROMPT);
}

/// `windows_terminal_status_bar`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    steps(scenario);
}

/// `conhost_status_bar`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    steps(scenario);
}
