//! Typing a terminal does not show plainly, as `windows_terminal_typing`
//! in Windows Terminal and `conhost_typing` in the console host
//! (`phase6-design.md`, "Terminal risks found by studying NVDA" and "What
//! a key did to the text"). The shared setup is described in the
//! `terminal` module.
//!
//! What it asserts, each step heard in full before the next:
//!
//! 1. A password typed while its prompt's line changes: the written
//!    script `secret.ps1` shows "Password: tick 0" and reads keys without
//!    showing them, rewriting the line's count after each key ("Password:
//!    tick 1" and so on), as a clock on a password prompt's line would.
//!    Each key's rewrite speaks the count that changed ("1", "2", "3") and
//!    never the character typed. Enter ends it: "got 3 characters" and
//!    the prompt.
//! 2. A character typed in the middle of a command is echoed: `echo helo`
//!    is typed and echoed, Left Arrow says "o", "l" is typed and echoed as
//!    "l" (not the rewritten word), and Enter gives "hello" and the
//!    prompt.
//! 3. Escape clears the command line being typed and says what it
//!    removed: `echo hi` is typed and echoed, and Escape says "echo hi".
//!    `echo ok`, typed on the cleared line, is echoed as typed (the line
//!    starts as the one cleared did), and Enter gives "ok" and the prompt.
//! 4. `cls` clears the screen down to the prompt, which is spoken as new
//!    (NVDA, captured live on 2026-10-09, says it too).
//! 5. Escape on a typed line that wrapped onto a second row says all it
//!    removed, as on a line of one row: `echo ` and 108 "a"s fill the
//!    120-column row after the 7-cell prompt, "xyz" goes onto the next
//!    row, and Escape says the whole command. (NVDA, captured live on
//!    2026-10-09, says nothing.)
//!
//! Commands are typed with the agent's `TypeText` a character at a time,
//! and keys pressed with `SendKeys`.

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The script reading a password it never shows, rewriting its prompt's
/// line after each key.
const SECRET_SCRIPT: &str = "$count = 0\r\n\
[Console]::Write('Password: tick 0')\r\n\
while ($true) {\r\n\
\x20   $key = [Console]::ReadKey($true)\r\n\
\x20   if ($key.Key -eq 'Enter') { break }\r\n\
\x20   $count++\r\n\
\x20   [Console]::Write(\"`rPassword: tick $count\")\r\n\
}\r\n\
[Console]::WriteLine()\r\n\
\"got $count characters\"\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "typing", &[("secret.ps1", SECRET_SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "typing", &[("secret.ps1", SECRET_SCRIPT)])
}

/// Types `text` a character at a time, hearing exactly `heard` after each.
fn type_each(scenario: &mut Scenario, text: &str, heard: &[&str]) {
    for (character, heard) in text.chars().zip(heard) {
        scenario
            .type_text(&character.to_string())
            .expect("types a character");
        scenario.speech().expect(&[heard]);
    }
}

/// Escape on a typed line that wrapped onto a second row.
fn escape_on_a_wrapped_line(scenario: &mut Scenario) {
    let command = format!("echo {}xyz", "a".repeat(108));
    terminal::type_hearing(scenario, &command, terminal::Echo::Shown);
    scenario.send_keys(&["escape"]).expect("presses escape");
    scenario.speech().expect(&[command.as_str()]);
}

/// `windows_terminal_typing`.
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
    // A password typed while its prompt's line changes.
    terminal::type_with_echo(scenario, r".\secret.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["Password: tick 0"]);
    type_each(scenario, "abc", &["1", "2", "3"]);
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&["got 3 characters", PROMPT]);

    // A character typed in the middle of a command.
    terminal::type_hearing(scenario, "echo helo", terminal::Echo::Shown);
    scenario.send_keys(&["leftarrow"]).expect("presses left");
    scenario.speech().expect(&["o"]);
    terminal::type_hearing(scenario, "l", terminal::Echo::Shown);
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&["hello", PROMPT]);

    // Escape clears the line and says what it removed.
    terminal::type_hearing(scenario, "echo hi", terminal::Echo::Shown);
    scenario.send_keys(&["escape"]).expect("presses escape");
    scenario.speech().expect(&["echo hi"]);

    // The line it cleared, typed again, is echoed as typed.
    terminal::type_with_echo(scenario, "echo ok", terminal::Echo::Shown);
    scenario.speech().expect(&["ok", PROMPT]);

    // `cls` clears the screen down to the prompt, which is new.
    terminal::type_with_echo(scenario, "cls", terminal::Echo::Shown);
    scenario.speech().expect(&[PROMPT]);

    escape_on_a_wrapped_line(scenario);
}

/// `conhost_typing`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    // A password typed while its prompt's line changes.
    terminal::type_with_echo(scenario, r".\secret.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["Password: tick 0"]);
    type_each(scenario, "abc", &["1", "2", "3"]);
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&["got 3 characters", PROMPT]);

    // A character typed in the middle of a command.
    terminal::type_hearing(scenario, "echo helo", terminal::Echo::Shown);
    scenario.send_keys(&["leftarrow"]).expect("presses left");
    scenario.speech().expect(&["o"]);
    terminal::type_hearing(scenario, "l", terminal::Echo::Shown);
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&["hello", PROMPT]);

    // Escape clears the line and says what it removed.
    terminal::type_hearing(scenario, "echo hi", terminal::Echo::Shown);
    scenario.send_keys(&["escape"]).expect("presses escape");
    scenario.speech().expect(&["echo hi"]);

    // The line it cleared, typed again, is echoed as typed.
    terminal::type_with_echo(scenario, "echo ok", terminal::Echo::Shown);
    scenario.speech().expect(&["ok", PROMPT]);

    // `cls` clears the screen down to the prompt, which is new.
    terminal::type_with_echo(scenario, "cls", terminal::Echo::Shown);
    scenario.speech().expect(&[PROMPT]);

    escape_on_a_wrapped_line(scenario);
}
