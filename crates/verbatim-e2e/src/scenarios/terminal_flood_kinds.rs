//! Floods of other kinds, and the flood limits changed (`phase6-design.md`,
//! "Test design decisions", the floods; the coherence review of
//! 2026-10-09), each scenario in both terminals as its own code
//! (`docs/testing.md`). The shared setup is described in the `terminal`
//! module.
//!
//! - `*_same_flood`: `same.ps1` prints "the same line" 2000 times. The
//!   screen diff must count lines that read alike: heard as any flood, the
//!   line 30 times, "skipped 1941 lines", the line 29 times and the prompt.
//!   NVDA, captured live on 2026-10-09, speaks every line.
//! - `*_raised_flood`: "Lines spoken in full" and "Last lines to speak"
//!   raised to 200, above the 101 lines of `hundred.ps1`'s output and its
//!   prompt, so every line is heard, as NVDA speaks every one.
//! - `*_redraw_limit`: both limits lowered to 10. `screen.ps1` opens the
//!   alternate screen and draws its 30 rows in one write, "screen row 1" to
//!   "screen row 30", and Q closes it and prints "closed". The redraw is a
//!   burst larger than the limit: rows 1 to 10, "skipped 10 lines", rows 21
//!   to 30. NVDA, captured live, speaks all 30 rows.

use std::io;

use verbatim_config::Settings;

use super::terminal::{self, PROMPT};
use super::terminal_flood::{FLOOD_STEP, burst_speech};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// Two thousand lines that read alike.
const SAME_SCRIPT: &str = "for ($line = 1; $line -le 2000; $line++) { \"the same line\" }\r\n";

/// A hundred numbered lines.
const HUNDRED_SCRIPT: &str =
    "for ($line = 1; $line -le 100; $line++) { \"output line $line\" }\r\n";

/// The full-screen program: its screen drawn in one write, closed by Q.
const SCREEN_SCRIPT: &str = "Add-Type -TypeDefinition @\"\r\n\
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
$screen = \"$e[?1049h$e[H$e[2J\"\r\n\
for ($n = 1; $n -lt $rows; $n++) { $screen += \"screen row $n`r`n\" }\r\n\
[Console]::Write($screen + \"screen row $rows\")\r\n\
while ([Console]::ReadKey($true).Key -ne 'Q') { }\r\n\
[Console]::Write(\"$e[?1049l\")\r\n\
'closed'\r\n";

/// The lowered limit of the redraw scenarios.
const LOWERED: u16 = 10;

/// The raised limit of the raised-flood scenarios.
const RAISED: u16 = 200;

/// The raised-flood scenarios' settings: both limits 200.
pub(crate) fn raised_limits(settings: &mut Settings) {
    settings.reader.terminal_full_lines = RAISED;
    settings.reader.terminal_last_lines = RAISED;
}

/// The redraw scenarios' settings: both limits 10.
pub(crate) fn lowered_limits(settings: &mut Settings) {
    settings.reader.terminal_full_lines = LOWERED;
    settings.reader.terminal_last_lines = LOWERED;
}

/// The announcement as a terminal of this scenario takes the focus with
/// nothing written yet, asserted with the first prompt.
fn opened(scenario: &mut Scenario, state: &ScenarioState, windows_terminal: bool) {
    let title = terminal::title(state).to_owned();
    let window = format!("{title} window");
    let named = format!("{title} terminal");
    if windows_terminal {
        terminal::expect_prompt_read(scenario, state, &[&window, &named]);
    } else {
        // The console host's text area has no name.
        terminal::expect_prompt_read(scenario, state, &[&window, "terminal", "blank"]);
    }
}

/// Runs `command` and asserts exactly `speech` within the flood bound.
fn run_hearing(scenario: &mut Scenario, command: &str, speech: &[String]) {
    terminal::type_with_echo(scenario, command, terminal::Echo::Shown);
    let speech: Vec<&str> = speech.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&speech, FLOOD_STEP);
}

pub(crate) fn setup_same_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "same-flood", &[("same.ps1", SAME_SCRIPT)])
}

pub(crate) fn setup_same_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "same-flood", &[("same.ps1", SAME_SCRIPT)])
}

/// "the same line", whichever line.
fn same(_: u32) -> String {
    "the same line".to_owned()
}

/// `windows_terminal_same_flood`.
pub(crate) fn body_same_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, true);
    run_hearing(scenario, r".\same.ps1", &burst_speech(2000, same));
}

/// `conhost_same_flood`.
pub(crate) fn body_same_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, false);
    run_hearing(scenario, r".\same.ps1", &burst_speech(2000, same));
}

pub(crate) fn setup_raised_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "raised-flood", &[("hundred.ps1", HUNDRED_SCRIPT)])
}

pub(crate) fn setup_raised_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "raised-flood", &[("hundred.ps1", HUNDRED_SCRIPT)])
}

/// Every line of `hundred.ps1`, and the prompt.
fn hundred() -> Vec<String> {
    (1..=100)
        .map(|n| format!("output line {n}"))
        .chain(std::iter::once(PROMPT.to_owned()))
        .collect()
}

/// `windows_terminal_raised_flood`.
pub(crate) fn body_raised_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, true);
    run_hearing(scenario, r".\hundred.ps1", &hundred());
}

/// `conhost_raised_flood`.
pub(crate) fn body_raised_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, false);
    run_hearing(scenario, r".\hundred.ps1", &hundred());
}

pub(crate) fn setup_redraw_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "redraw-limit", &[("screen.ps1", SCREEN_SCRIPT)])
}

pub(crate) fn setup_redraw_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "redraw-limit", &[("screen.ps1", SCREEN_SCRIPT)])
}

/// What the 30-row redraw says with both limits at 10.
fn redraw() -> Vec<String> {
    let row = |n: u16| format!("screen row {n}");
    let mut speech: Vec<String> = (1..=LOWERED).map(row).collect();
    speech.push(format!(
        "sound: skipped-lines skipped {} lines",
        30 - 2 * LOWERED
    ));
    speech.extend((30 - LOWERED + 1..=30).map(row));
    speech
}

/// The redraw heard within the limit, then the program closed.
fn redraw_steps(scenario: &mut Scenario) {
    run_hearing(scenario, r".\screen.ps1", &redraw());
    scenario.send_keys(&["q"]).expect("presses Q");
    scenario.speech().expect(&["closed", PROMPT]);
}

/// `windows_terminal_redraw_limit`.
pub(crate) fn body_redraw_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, true);
    redraw_steps(scenario);
}

/// `conhost_redraw_limit`.
pub(crate) fn body_redraw_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, false);
    redraw_steps(scenario);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redraw_keeps_ten_rows_each_side_of_the_skip() {
        let speech = redraw();
        assert_eq!(speech.len(), 21);
        assert_eq!(speech[9], "screen row 10");
        assert_eq!(speech[10], "sound: skipped-lines skipped 10 lines");
        assert_eq!(speech[11], "screen row 21");
        assert_eq!(speech[20], "screen row 30");
    }

    #[test]
    fn the_raised_limit_is_above_the_output() {
        assert!(usize::from(RAISED) > hundred().len());
    }

    #[test]
    fn identical_lines_are_counted_as_any_flood() {
        let speech = burst_speech(2000, same);
        assert_eq!(speech.len(), 61);
        assert_eq!(speech[30], "sound: skipped-lines skipped 1941 lines");
        assert_eq!(speech[60], PROMPT);
    }
}
