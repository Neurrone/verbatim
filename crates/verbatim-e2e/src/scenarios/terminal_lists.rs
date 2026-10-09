//! Selection lists drawn in a terminal, as `windows_terminal_marker_list`,
//! `conhost_marker_list`, `windows_terminal_redrawn_list` and
//! `conhost_redrawn_list`, each its own code (`docs/testing.md`). The
//! shared setup is described in the `terminal` module.
//!
//! Each script prints "Pick a fruit:" and a five-item list with a ">"
//! marker on its first item, then moves the marker with Down and Up Arrow
//! until Enter chooses, each move in one write, as list prompts draw
//! (`phase6-design.md`, "Terminal line keys as NVDA has them").
//! `marker.ps1` erases the old marker and draws the new one, leaving the
//! caret after it, so the caret moves to the item's line: the key says that
//! line, as in a text field, and the marker drawn on it is the key's own,
//! not spoken again as output. `redraw.ps1` writes every row of the list
//! again on each move, as Ink-based programs redraw their region, leaving
//! the caret on the row below the list, where it was: the key says
//! nothing, the caret not having moved, and the line that gained the marker
//! is spoken as output, from the word that changed.
//!
//! NVDA, captured live on 2026-10-09 with these scripts, says the same for
//! the first script in both terminals ("greater banana", "greater cherry",
//! "greater banana"). For the second it says "blank" for each key: it
//! speaks the caret's line once its wait for the caret to move times out,
//! and drops the marker's change as output, a single character inserted on
//! one line; Verbatim keeps a key that does not move the caret silent and
//! speaks a rewritten line from the word that changed (`docs/parity.md`).

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// Turns on the console's processing of escape sequences, which the
/// scripts write their moves with.
const ENABLE_ESCAPES: &str = "Add-Type -TypeDefinition @\"\r\n\
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
$e = [char]27\r\n";

/// The list whose marker moves, the caret following it to the item.
const MARKER_SCRIPT: &str = "$items = 'apple', 'banana', 'cherry', 'date', 'elderberry'\r\n\
[Console]::WriteLine('Pick a fruit:')\r\n\
for ($i = 0; $i -lt $items.Count; $i++) {\r\n\
\x20   $mark = if ($i -eq 0) { '> ' } else { '  ' }\r\n\
\x20   [Console]::WriteLine($mark + $items[$i])\r\n\
}\r\n\
[Console]::Write(\"$e[$($items.Count)A$e[3G\")\r\n\
$selected = 0\r\n\
while ($true) {\r\n\
\x20   $key = [Console]::ReadKey($true)\r\n\
\x20   if ($key.Key -eq 'Enter') { break }\r\n\
\x20   $next = $selected\r\n\
\x20   if ($key.Key -eq 'DownArrow' -and $selected -lt $items.Count - 1) { $next++ }\r\n\
\x20   if ($key.Key -eq 'UpArrow' -and $selected -gt 0) { $next-- }\r\n\
\x20   if ($next -ne $selected) {\r\n\
\x20       $move = if ($next -gt $selected) { \"$e[$($next - $selected)B\" } else { \"$e[$($selected - $next)A\" }\r\n\
\x20       [Console]::Write(\"`r  $move`r> \")\r\n\
\x20       $selected = $next\r\n\
\x20   }\r\n\
}\r\n\
[Console]::Write(\"$e[$($items.Count - $selected)B`r\")\r\n\
\"chose $($items[$selected])\"\r\n";

/// The list redrawn whole on every move, the caret staying below it.
const REDRAW_SCRIPT: &str = "$items = 'apple', 'banana', 'cherry', 'date', 'elderberry'\r\n\
function Lines($selected) {\r\n\
\x20   $text = ''\r\n\
\x20   for ($i = 0; $i -lt $items.Count; $i++) {\r\n\
\x20       $mark = if ($i -eq $selected) { '> ' } else { '  ' }\r\n\
\x20       $text += $mark + $items[$i] + \"`r`n\"\r\n\
\x20   }\r\n\
\x20   $text\r\n\
}\r\n\
[Console]::WriteLine('Pick a fruit:')\r\n\
[Console]::Write((Lines 0))\r\n\
$selected = 0\r\n\
while ($true) {\r\n\
\x20   $key = [Console]::ReadKey($true)\r\n\
\x20   if ($key.Key -eq 'Enter') { break }\r\n\
\x20   $next = $selected\r\n\
\x20   if ($key.Key -eq 'DownArrow' -and $selected -lt $items.Count - 1) { $next++ }\r\n\
\x20   if ($key.Key -eq 'UpArrow' -and $selected -gt 0) { $next-- }\r\n\
\x20   if ($next -ne $selected) {\r\n\
\x20       $selected = $next\r\n\
\x20       [Console]::Write(\"$e[$($items.Count)A`r\" + (Lines $selected))\r\n\
\x20   }\r\n\
}\r\n\
\"chose $($items[$selected])\"\r\n";

/// The scripts each scenario writes.
fn scripts() -> [(&'static str, String); 2] {
    [
        ("marker.ps1", format!("{ENABLE_ESCAPES}{MARKER_SCRIPT}")),
        ("redraw.ps1", format!("{ENABLE_ESCAPES}{REDRAW_SCRIPT}")),
    ]
}

/// [`scripts`], as the setup writes them.
fn written<'a>(scripts: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    scripts
        .iter()
        .map(|(name, text)| (*name, text.as_str()))
        .collect()
}

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "lists", &written(&scripts()))
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "lists", &written(&scripts()))
}

/// Presses `key` and asserts exactly `heard`.
fn key_hearing(scenario: &mut Scenario, key: &str, heard: &[&str]) {
    scenario.send_keys(&[key]).expect("presses the key");
    scenario.speech().expect(heard);
}

/// Runs `script`, moves its marker down twice and up once, each move
/// saying the line that gained the marker, and chooses.
fn list_steps(scenario: &mut Scenario, script: &str) {
    terminal::type_with_echo(scenario, script, terminal::Echo::Shown);
    scenario.speech().expect(&[
        "Pick a fruit:",
        "> apple",
        "  banana",
        "  cherry",
        "  date",
        "  elderberry",
    ]);
    key_hearing(scenario, "downarrow", &["> banana"]);
    key_hearing(scenario, "downarrow", &["> cherry"]);
    key_hearing(scenario, "uparrow", &["> banana"]);
    key_hearing(scenario, "enter", &["chose banana", PROMPT]);
}

/// The window, the terminal and its blank line, as the Windows Terminal
/// scenarios hear them.
fn windows_terminal_opening(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[
            &format!("{title} window"),
            &format!("{title} terminal"),
            "blank",
        ],
    );
}

/// The same for the console host, whose text area has no name.
fn console_host_opening(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
}

/// `windows_terminal_marker_list`.
pub(crate) fn body_marker_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    windows_terminal_opening(scenario, state);
    list_steps(scenario, r".\marker.ps1");
}

/// `conhost_marker_list`.
pub(crate) fn body_marker_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    console_host_opening(scenario, state);
    list_steps(scenario, r".\marker.ps1");
}

/// `windows_terminal_redrawn_list`.
pub(crate) fn body_redrawn_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    windows_terminal_opening(scenario, state);
    list_steps(scenario, r".\redraw.ps1");
}

/// `conhost_redrawn_list`.
pub(crate) fn body_redrawn_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    console_host_opening(scenario, state);
    list_steps(scenario, r".\redraw.ps1");
}
