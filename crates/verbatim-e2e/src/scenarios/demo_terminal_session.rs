//! Demonstration: a session in Windows Terminal (milestone M4 item 9),
//! recorded by `cargo xtask demo` for `videos/demos`. It runs, in one
//! window, the steps the Windows Terminal scenarios test, so it shows
//! nothing no test covers. The window, the shell, and the prompt are set up
//! as for every terminal scenario (the `terminal` module). The walk:
//!
//! 1. The prompt is spoken as it appears and read with the review cursor.
//! 2. `terminal_editing`'s steps: a typo corrected with Backspace,
//!    punctuation echoed by name, a table printed, and the review cursor
//!    down its Moons column.
//! 3. `windows_terminal_commands`' steps: `echo hello`, and a password
//!    typed at a prompt, its characters not spoken.
//! 4. `terminal_flood`'s first flood: its first lines, how many lines were
//!    skipped, and its last lines.

use std::io;

use super::terminal::{self, Terminal};
use super::{terminal_commands, terminal_editing, terminal_flood};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let table = terminal_editing::table_script();
    let mut scripts: Vec<(&str, &str)> = terminal_commands::SCRIPTS.to_vec();
    scripts.push((terminal_editing::SCRIPT_NAME, table.as_str()));
    scripts.push(("flood.ps1", terminal_flood::SCRIPT));
    terminal::open(
        scenario,
        "demo-session",
        Terminal::WindowsTerminal,
        &scripts,
    )
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_prompt_read(scenario, state);
    terminal_editing::steps(scenario);
    terminal_commands::steps(scenario);
    terminal_flood::heard_flood(scenario, 1);
}
