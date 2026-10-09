//! Commands, their output, typed echo, and a password prompt in a terminal
//! (milestone M4 item 9; `phase6-design.md`, "Terminal end-to-end
//! scenarios"): `windows_terminal_commands` in Windows Terminal and
//! `conhost_commands` in the console host, and
//! `windows_terminal_spoken_password` and `conhost_spoken_password` with the
//! "speak passwords" setting on, each its own code (`docs/testing.md`).
//! The shared setup is described in the `terminal` module.
//!
//! Every scenario first reads the prompt line with the review cursor
//! (numpad 8) and hears "ready>", so typing starts only once Verbatim reads
//! this terminal. Then:
//!
//! 1. It types `echo hello` a character at a time, hearing each echo
//!    before the next (the space as "space", with the letter after it, as
//!    the terminal shows a space only once something follows it). Enter
//!    then gives exactly "hello" and then the prompt, "ready>". This shows
//!    echo works in this window, so the silence that follows means
//!    something.
//! 2. It runs `.\password.ps1`, a written script that calls
//!    `Read-Host -AsSecureString "Password"` and then prints "done": its
//!    echo, then the prompt "Password:", is spoken.
//! 3. It types `secret` one character at a time. With "speak passwords" off
//!    (the default), each character is not spoken: Windows PowerShell's
//!    console shows an asterisk for it, and that new output is what is
//!    spoken. With "speak passwords" on, each character is spoken, then its
//!    asterisk.
//! 4. Enter: "done", and then "ready>". Every asterisk was spoken as it
//!    was shown, so the line is not read again.
//!
//! Every step asserts exactly what it says before the next key.

use std::io;

use verbatim_config::Settings;

use super::terminal::{self, Echo, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The script the scenario runs: a password prompt, then "done".
pub(crate) const SCRIPTS: &[(&str, &str)] = &[(
    "password.ps1",
    "$secure = Read-Host -AsSecureString 'Password'\r\n'done'\r\n",
)];

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "commands", SCRIPTS)
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "commands", SCRIPTS)
}

pub(crate) fn setup_spoken_password_windows_terminal(
    scenario: &mut Scenario,
) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "spoken-password", SCRIPTS)
}

pub(crate) fn setup_spoken_password_console_host(
    scenario: &mut Scenario,
) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "spoken-password", SCRIPTS)
}

/// The spoken-password scenarios' settings: "speak passwords" on.
pub(crate) fn speak_passwords(settings: &mut Settings) {
    settings.reader.speak_terminal_passwords = true;
}

/// Types `character` at the password prompt and asserts exactly `heard`.
fn type_password_character(scenario: &mut Scenario, character: &str, heard: &[&str]) {
    scenario
        .type_text(character)
        .expect("types a character of the password");
    scenario.speech().expect(heard);
}

/// `windows_terminal_commands`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), &format!("{title} terminal")],
    );
    windows_terminal_steps(scenario);
}

/// `windows_terminal_commands`' steps once the prompt has been read, in a
/// Windows Terminal whose folder holds [`SCRIPTS`].
pub(crate) fn windows_terminal_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, "echo hello", Echo::Shown);
    scenario.speech().expect(&["hello", PROMPT]);
    terminal::type_with_echo(scenario, r".\password.ps1", Echo::Shown);
    scenario.speech().expect(&["Password:"]);
    // The prompt's space was on the line already, though the outpost could
    // not tell it from padding until the first asterisk followed it, so it
    // is not spoken again.
    for character in ["s", "e", "c", "r", "e", "t"] {
        type_password_character(scenario, character, &["*"]);
    }
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&["done", PROMPT]);
}

/// `conhost_commands`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    terminal::type_with_echo(scenario, "echo hello", Echo::Shown);
    scenario.speech().expect(&["hello", PROMPT]);
    terminal::type_with_echo(scenario, r".\password.ps1", Echo::Shown);
    scenario.speech().expect(&["Password:"]);
    for character in ["s", "e", "c", "r", "e", "t"] {
        type_password_character(scenario, character, &["*"]);
    }
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&["done", PROMPT]);
}

/// `windows_terminal_spoken_password`.
pub(crate) fn body_spoken_password_windows_terminal(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), &format!("{title} terminal")],
    );
    terminal::type_with_echo(scenario, "echo hello", Echo::Typed);
    scenario.speech().expect(&["hello", PROMPT]);
    terminal::type_with_echo(scenario, r".\password.ps1", Echo::Typed);
    scenario.speech().expect(&["Password:"]);
    for character in ["s", "e", "c", "r", "e", "t"] {
        type_password_character(scenario, character, &[character, "*"]);
    }
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&["done", PROMPT]);
}

/// `conhost_spoken_password`.
pub(crate) fn body_spoken_password_console_host(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    terminal::type_with_echo(scenario, "echo hello", Echo::Typed);
    scenario.speech().expect(&["hello", PROMPT]);
    terminal::type_with_echo(scenario, r".\password.ps1", Echo::Typed);
    scenario.speech().expect(&["Password:"]);
    for character in ["s", "e", "c", "r", "e", "t"] {
        type_password_character(scenario, character, &[character, "*"]);
    }
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&["done", PROMPT]);
}
