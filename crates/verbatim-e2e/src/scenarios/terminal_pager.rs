//! A pager scrolling a line at a time on the alternate screen, as
//! `windows_terminal_pager` and `conhost_pager`, each its own code
//! (`docs/testing.md`). The shared setup is described in the `terminal`
//! module.
//!
//! `pager.ps1` draws as a pager does: it opens the alternate screen, fills
//! it with "page line 1" to "page line 29" and its prompt, ":", on the last
//! row, the caret after the prompt, all spoken as output. Down Arrow moves
//! a line forward, in one write: the prompt erased, the next line written
//! on its row, and a line feed scrolling the screen up, then the prompt.
//! Up Arrow moves a line back: a line inserted at the top, pushing the
//! prompt off the bottom, the earlier line written there, then the prompt
//! on the last row again. Q closes the alternate screen and prints
//! "closed", in one write, so that every read after it finds the same
//! screen; the main screen comes back as a new screen, so its rows are
//! spoken, the command, "closed" and the prompt, as NVDA speaks the shell's
//! screen when a pager closes (`docs/design/terminal-model.md`, "Screens",
//! Dickson, 2026-10-24).
//!
//! `windows_terminal_pager_history` and `conhost_pager_history` run the
//! same pager after `lines.ps1` has printed 100 lines, "history line 1" to
//! "history line 100", heard through the flood policy as lines 1 to 30,
//! "skipped 41 lines", lines 72 to 100 and the prompt. The pager's own
//! reads keep its rows, so the shell's screen stays aside while it runs;
//! closing it finds that screen back at its places, a new screen, so only
//! its visible rows are spoken, the newest 27 history lines, the command,
//! "closed" and the prompt, 30 lines, with no skip
//! (`docs/design/terminal-model.md`, "The aside screen", Dickson,
//! 2026-10-25). The text scrolls through rows that stay put, so the screen
//! diff finds how far from the text (`docs/design/phase6-design.md`, "Terminal line
//! keys as NVDA has them"), and each move speaks only the line it brought
//! onto the screen. The caret stays after the prompt, so the keys
//! themselves say nothing.
//!
//! NVDA, captured live on 2026-10-09 with this script in both terminals,
//! says the same lines, each after the prompt, ":", the caret's line it
//! speaks once its wait for the caret to move times out; Verbatim keeps a
//! key that does not move the caret silent
//! (`docs/parity/terminals.md`, "New terminal output").

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The longest the pager's first page takes to be spoken; it only bounds a
/// hang.
const PAGE_SPEECH: Duration = Duration::from_secs(60);

/// The pager.
const PAGER_SCRIPT: &str = "Add-Type -TypeDefinition @\"\r\n\
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
$page = $rows - 1\r\n\
function Text([int]$n) { \"page line $n\" }\r\n\
$screen = \"$e[?1049h$e[H$e[2J\"\r\n\
for ($n = 1; $n -le $page; $n++) { $screen += (Text $n) + \"`r`n\" }\r\n\
[Console]::Write($screen + ':')\r\n\
$top = 1\r\n\
while ($true) {\r\n\
\x20   $key = [Console]::ReadKey($true)\r\n\
\x20   if ($key.Key -eq 'Q') { break }\r\n\
\x20   if ($key.Key -eq 'DownArrow') {\r\n\
\x20       $top++\r\n\
\x20       [Console]::Write(\"`r$e[K\" + (Text ($top + $page - 1)) + \"`r`n:\")\r\n\
\x20   }\r\n\
\x20   if ($key.Key -eq 'UpArrow' -and $top -gt 1) {\r\n\
\x20       $top--\r\n\
\x20       [Console]::Write(\"$e[H$e[L\" + (Text $top) + \"$e[$rows;1H$e[K:\")\r\n\
\x20   }\r\n\
}\r\n\
[Console]::Write(\"$e[?1049l\" + \"closed`r`n\")\r\n";

/// 100 numbered lines, as output before the pager.
const LINES_SCRIPT: &str = "foreach ($n in 1..100) { \"history line $n\" }\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "pager", &[("pager.ps1", PAGER_SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "pager", &[("pager.ps1", PAGER_SCRIPT)])
}

pub(crate) fn setup_history_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(
        scenario,
        "pager-history",
        &[("pager.ps1", PAGER_SCRIPT), ("lines.ps1", LINES_SCRIPT)],
    )
}

pub(crate) fn setup_history_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(
        scenario,
        "pager-history",
        &[("pager.ps1", PAGER_SCRIPT), ("lines.ps1", LINES_SCRIPT)],
    )
}

/// "history line `first`" to "history line `last`".
fn history(first: u32, last: u32) -> Vec<String> {
    (first..=last)
        .map(|line| format!("history line {line}"))
        .collect()
}

/// Prints the 100 history lines and hears them through the flood policy.
fn history_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\lines.ps1", terminal::Echo::Shown);
    let mut heard = history(1, 30);
    heard.push("sound: skipped-lines skipped 41 lines".to_owned());
    heard.extend(history(72, 100));
    heard.push(PROMPT.to_owned());
    let heard: Vec<&str> = heard.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&heard, PAGE_SPEECH);
}

/// Hears the shell's visible screen as the pager closes over the history:
/// the newest 27 history lines, the command, "closed" and the prompt.
fn closed_over_history(scenario: &mut Scenario) {
    let mut heard = history(74, 100);
    heard.extend([
        r"ready> .\pager.ps1".to_owned(),
        "closed".to_owned(),
        PROMPT.to_owned(),
    ]);
    let heard: Vec<&str> = heard.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&heard, PAGE_SPEECH);
}

/// Opens the pager, hears its page, moves down a line and back up, and
/// presses Q.
fn pager_until_closed(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\pager.ps1", terminal::Echo::Shown);
    let page: Vec<String> = (1..=29)
        .map(|line| format!("page line {line}"))
        .chain(std::iter::once(":".to_owned()))
        .collect();
    let page: Vec<&str> = page.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&page, PAGE_SPEECH);
    scenario.send_keys(&["downarrow"]).expect("presses Down");
    scenario.deviates_from_nvda("docs/parity/terminals.md", "New terminal output");
    scenario.speech().expect(&["page line 30"]);
    scenario.send_keys(&["uparrow"]).expect("presses Up");
    scenario.deviates_from_nvda("docs/parity/terminals.md", "New terminal output");
    scenario.speech().expect(&["page line 1"]);
    scenario.send_keys(&["q"]).expect("presses Q");
    scenario.deviates_from_nvda("docs/parity/terminals.md", "New terminal output");
}

/// `windows_terminal_pager_history`.
pub(crate) fn body_history_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    history_steps(scenario);
    pager_until_closed(scenario);
    closed_over_history(scenario);
}

/// `conhost_pager_history`.
pub(crate) fn body_history_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    history_steps(scenario);
    pager_until_closed(scenario);
    closed_over_history(scenario);
}

/// Opens the pager, hears its page, moves down a line and back up, and
/// closes it.
fn pager_steps(scenario: &mut Scenario) {
    pager_until_closed(scenario);
    scenario
        .speech()
        .expect(&[r"ready> .\pager.ps1", "closed", PROMPT]);
}

/// `windows_terminal_pager`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    pager_steps(scenario);
}

/// `conhost_pager`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    pager_steps(scenario);
}
