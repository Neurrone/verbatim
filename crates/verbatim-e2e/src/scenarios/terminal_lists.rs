//! Selection lists drawn in a terminal, as `windows_terminal_marker_list`,
//! `conhost_marker_list`, `windows_terminal_redrawn_list` and
//! `conhost_redrawn_list`, each its own code (`docs/testing.md`). The
//! shared setup is described in the `terminal` module.
//!
//! Each script prints "Pick a fruit:" and a five-item list with a ">"
//! marker on its first item, then moves the marker with Down and Up Arrow
//! until Enter chooses. `marker.ps1` moves it by rewriting only the two
//! cells before the item it leaves and the item it reaches, leaving the
//! caret after the new marker; `redraw.ps1` writes every row of the list
//! again on each move, as Ink-based programs redraw their region, leaving
//! the caret on the row below the list. Each move speaks the line that
//! gained the marker, once: on the caret's line for the first script, on
//! another line for the second (`phase6-design.md`, "Selection lists in a
//! terminal"). NVDA, captured live on 2026-10-09, says the caret's line
//! part-way through the redraw instead, or "blank" for the second script;
//! the difference is recorded in `docs/parity.md`.

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The list whose marker moves by two cells.
const MARKER_SCRIPT: &str = "$items = 'apple', 'banana', 'cherry', 'date', 'elderberry'\r\n\
[Console]::WriteLine('Pick a fruit:')\r\n\
$top = [Console]::CursorTop\r\n\
for ($i = 0; $i -lt $items.Count; $i++) {\r\n\
\x20   $mark = if ($i -eq 0) { '> ' } else { '  ' }\r\n\
\x20   [Console]::WriteLine($mark + $items[$i])\r\n\
}\r\n\
$end = [Console]::CursorTop\r\n\
$selected = 0\r\n\
while ($true) {\r\n\
\x20   $key = [Console]::ReadKey($true)\r\n\
\x20   if ($key.Key -eq 'Enter') { break }\r\n\
\x20   $next = $selected\r\n\
\x20   if ($key.Key -eq 'DownArrow' -and $selected -lt $items.Count - 1) { $next++ }\r\n\
\x20   if ($key.Key -eq 'UpArrow' -and $selected -gt 0) { $next-- }\r\n\
\x20   if ($next -ne $selected) {\r\n\
\x20       [Console]::SetCursorPosition(0, $top + $selected)\r\n\
\x20       [Console]::Write('  ')\r\n\
\x20       [Console]::SetCursorPosition(0, $top + $next)\r\n\
\x20       [Console]::Write('> ')\r\n\
\x20       $selected = $next\r\n\
\x20   }\r\n\
}\r\n\
[Console]::SetCursorPosition(0, $end)\r\n\
\"chose $($items[$selected])\"\r\n";

/// The list redrawn whole on every move.
const REDRAW_SCRIPT: &str = "$items = 'apple', 'banana', 'cherry', 'date', 'elderberry'\r\n\
[Console]::WriteLine('Pick a fruit:')\r\n\
$top = [Console]::CursorTop\r\n\
function Draw($selected) {\r\n\
\x20   [Console]::SetCursorPosition(0, $top)\r\n\
\x20   for ($i = 0; $i -lt $items.Count; $i++) {\r\n\
\x20       $mark = if ($i -eq $selected) { '> ' } else { '  ' }\r\n\
\x20       [Console]::WriteLine($mark + $items[$i])\r\n\
\x20   }\r\n\
}\r\n\
Draw 0\r\n\
$end = [Console]::CursorTop\r\n\
$selected = 0\r\n\
while ($true) {\r\n\
\x20   $key = [Console]::ReadKey($true)\r\n\
\x20   if ($key.Key -eq 'Enter') { break }\r\n\
\x20   $next = $selected\r\n\
\x20   if ($key.Key -eq 'DownArrow' -and $selected -lt $items.Count - 1) { $next++ }\r\n\
\x20   if ($key.Key -eq 'UpArrow' -and $selected -gt 0) { $next-- }\r\n\
\x20   if ($next -ne $selected) {\r\n\
\x20       $selected = $next\r\n\
\x20       Draw $selected\r\n\
\x20   }\r\n\
}\r\n\
[Console]::SetCursorPosition(0, $end)\r\n\
\"chose $($items[$selected])\"\r\n";

/// The scripts each scenario writes.
fn scripts() -> [(&'static str, &'static str); 2] {
    [("marker.ps1", MARKER_SCRIPT), ("redraw.ps1", REDRAW_SCRIPT)]
}

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "lists", &scripts())
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "lists", &scripts())
}

/// Presses `key` and asserts exactly `heard`.
fn key_hearing(scenario: &mut Scenario, key: &str, heard: &[&str]) {
    scenario.send_keys(&[key]).expect("presses the key");
    scenario.speech().expect(heard);
}

/// Runs `script`, moves its marker down twice and up once, and chooses.
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
