//! Long lines and a full-screen program in a terminal, as
//! `windows_terminal_long_lines`, `conhost_long_lines`,
//! `windows_terminal_full_screen` and `conhost_full_screen`, each its own
//! code (`docs/testing.md`). The shared setup is described in the
//! `terminal` module. The expected speech is what NVDA said for the same
//! keys, captured live on 2026-10-09, where the two agree (`docs/parity.md`,
//! "New terminal output").
//!
//! Long lines: `long.ps1` prints one line of forty numbered words, far
//! wider than the terminal's 120 columns, which is spoken whole. `grow.ps1`
//! reads keys without showing them and, for each, appends ten more numbered
//! words to the same line, which keeps growing past any earlier size: each
//! key's ten words are spoken, and nothing of what was there. Enter ends it
//! with "grown".
//!
//! Full screen: `altscreen.ps1` opens the alternate screen and fills it,
//! rows 1 to 29 and a status line on the 30th, spoken in full; then, a key
//! at a time, changes row 3 and row 27 ("changed", from the word that
//! changed), scrolls the rows above the status line down a line ("row 0",
//! the new first row) and back up ("row 30", the new last), and closes the
//! alternate screen: only "closed" and the prompt are new, the main screen
//! being back as it was. The keys are read without being shown, so they
//! echo nothing.

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The longest forty words take to be spoken; it only bounds a hang.
const LONG_SPEECH: Duration = Duration::from_secs(60);

/// One line of forty numbered words.
const LONG_SCRIPT: &str = "$words = foreach ($i in 1..40) { \"word$i\" }\r\n\
$words -join ' '\r\n";

/// A line that grows by ten numbered words for each key read, until Enter.
const GROW_SCRIPT: &str = "$next = 1\r\n\
while ($true) {\r\n\
\x20   $key = [Console]::ReadKey($true)\r\n\
\x20   if ($key.Key -eq 'Enter') { break }\r\n\
\x20   $words = foreach ($i in $next..($next + 9)) { \"part$i\" }\r\n\
\x20   [Console]::Write(($words -join ' ') + ' ')\r\n\
\x20   $next += 10\r\n\
}\r\n\
[Console]::WriteLine()\r\n\
'grown'\r\n";

/// A full-screen program on the alternate screen.
const ALT_SCRIPT: &str = "Add-Type -TypeDefinition @\"\r\n\
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
function Row([int]$n, [string]$text) { [Console]::Write(\"$e[$n;1H$e[2K$text\") }\r\n\
function Park { [Console]::Write(\"$e[$rows;14H\") }\r\n\
[Console]::Write(\"$e[?1049h$e[2J\")\r\n\
for ($n = 1; $n -lt $rows; $n++) { Row $n \"row $n\" }\r\n\
Row $rows 'status: ready'\r\n\
Park\r\n\
$null = [Console]::ReadKey($true)\r\n\
Row 3 'row 3 changed'\r\n\
Park\r\n\
$null = [Console]::ReadKey($true)\r\n\
Row ($rows - 3) \"row $($rows - 3) changed\"\r\n\
Park\r\n\
$null = [Console]::ReadKey($true)\r\n\
[Console]::Write(\"$e[1;$($rows - 1)r$e[1T$e[r\")\r\n\
Row 1 'row 0'\r\n\
Park\r\n\
$null = [Console]::ReadKey($true)\r\n\
[Console]::Write(\"$e[1;$($rows - 1)r$e[1S$e[r\")\r\n\
Row ($rows - 1) \"row $rows\"\r\n\
Park\r\n\
$null = [Console]::ReadKey($true)\r\n\
[Console]::Write(\"$e[?1049l\")\r\n\
'closed'\r\n";

/// The scripts each scenario writes.
fn scripts() -> [(&'static str, &'static str); 3] {
    [
        ("long.ps1", LONG_SCRIPT),
        ("grow.ps1", GROW_SCRIPT),
        ("altscreen.ps1", ALT_SCRIPT),
    ]
}

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "screens", &scripts())
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "screens", &scripts())
}

/// `first` to `last` of `prefix` and a number, joined by spaces.
fn numbered(prefix: &str, first: u32, last: u32) -> String {
    (first..=last)
        .map(|n| format!("{prefix}{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Presses `key`, which the running script reads without showing, and
/// asserts exactly `heard`.
fn key_hearing(scenario: &mut Scenario, key: &str, heard: &[&str]) {
    scenario.send_keys(&[key]).expect("presses the key");
    scenario.speech().expect_within(heard, LONG_SPEECH);
}

/// The long-line steps once the prompt has been read.
fn long_line_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\long.ps1", terminal::Echo::Shown);
    let long = numbered("word", 1, 40);
    scenario
        .speech()
        .expect_within(&[long.as_str(), PROMPT], LONG_SPEECH);
    terminal::type_with_echo(scenario, r".\grow.ps1", terminal::Echo::Shown);
    for first in [1, 11, 21, 31] {
        let part = numbered("part", first, first + 9);
        key_hearing(scenario, "n", &[part.as_str()]);
    }
    key_hearing(scenario, "enter", &["grown", PROMPT]);
}

/// The full-screen steps once the prompt has been read.
fn full_screen_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\altscreen.ps1", terminal::Echo::Shown);
    let mut opened: Vec<String> = (1..30).map(|n| format!("row {n}")).collect();
    opened.push("status: ready".to_owned());
    let opened: Vec<&str> = opened.iter().map(String::as_str).collect();
    scenario.speech().expect(&opened);
    key_hearing(scenario, "n", &["changed"]);
    key_hearing(scenario, "n", &["changed"]);
    key_hearing(scenario, "n", &["row 0"]);
    key_hearing(scenario, "n", &["row 30"]);
    key_hearing(scenario, "n", &["closed", PROMPT]);
}

/// `windows_terminal_long_lines`.
pub(crate) fn body_long_lines_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
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
    long_line_steps(scenario);
}

/// `conhost_long_lines`.
pub(crate) fn body_long_lines_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    long_line_steps(scenario);
}

/// `windows_terminal_full_screen`.
pub(crate) fn body_full_screen_windows_terminal(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
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
    full_screen_steps(scenario);
}

/// `conhost_full_screen`.
pub(crate) fn body_full_screen_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    full_screen_steps(scenario);
}
