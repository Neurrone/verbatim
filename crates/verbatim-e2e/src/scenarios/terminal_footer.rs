//! A flood above a footer that redraws during and after it
//! (`phase6-design.md`, "Floods and the screen diff"), as
//! `windows_terminal_footer_flood` in Windows Terminal and
//! `conhost_footer_flood` in the console host. The shared setup is
//! described in the `terminal` module.
//!
//! The written script `footer.ps1` clears the screen, keeps its last row
//! for a footer, "status: busy", with a scroll region over the rows above,
//! and waits for a key: only the footer is heard, the cleared screen's
//! blank rows above it being no output. A key read without showing it
//! starts the flood, "flood line 1" to "flood line 100" written into the
//! region, with the footer redrawn as it was every twenty lines and as
//! "status: done" at the end. The flood policy applies as to any flood:
//! lines 1 to 30 whole, then "skipped 41 lines" (lines 31 to 71), then the
//! newest thirty, lines 72 to 100 and the footer's change, "done": the
//! footer stays on the last row while the rows above it scroll, and is
//! said only as it changed, never again as new. A redraw that leaves the
//! footer as it was says nothing. Another key removes the region, clears
//! the screen and prints "footer gone".
//!
//! NVDA, captured live on 2026-10-09 with the same script, said "status:
//! busy" too, then, having no flood policy, lines 35 to 38, 98 to 100 and
//! "status: done" in the console host, and lines 2 to 100 and "done" in
//! Windows Terminal (`docs/parity.md`, "New terminal output").

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The longest the flood's speech may take between one utterance and the
/// next; it only bounds a hang.
const FLOOD_STEP: Duration = Duration::from_secs(120);

/// The flood above a redrawing footer.
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
for ($n = 1; $n -le 100; $n++) {\r\n\
\x20   [Console]::Write(\"`nflood line $n\")\r\n\
\x20   if ($n % 20 -eq 0) { [Console]::Write(\"$e[s$e[$rows;1Hstatus: busy$e[u\") }\r\n\
}\r\n\
[Console]::Write(\"$e[s$e[$rows;1Hstatus: done$e[u\")\r\n\
$null = [Console]::ReadKey($true)\r\n\
[Console]::Write(\"$e[r$e[2J$e[H\")\r\n\
'footer gone'\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "footer", &[("footer.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "footer", &[("footer.ps1", SCRIPT)])
}

/// `first` to `last` of the flood's lines.
fn flood_lines(first: u32, last: u32) -> Vec<String> {
    (first..=last).map(|n| format!("flood line {n}")).collect()
}

/// The steps once the prompt has been read.
fn steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\footer.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["status: busy"]);
    scenario.send_keys(&["n"]).expect("presses n");
    let mut heard = flood_lines(1, 30);
    heard.push("sound: skipped-lines skipped 41 lines".to_owned());
    heard.extend(flood_lines(72, 100));
    heard.push("done".to_owned());
    let heard: Vec<&str> = heard.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&heard, FLOOD_STEP);
    scenario.send_keys(&["n"]).expect("presses n");
    scenario.speech().expect(&["footer gone", PROMPT]);
}

/// `windows_terminal_footer_flood`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
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
    steps(scenario);
}

/// `conhost_footer_flood`.
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
