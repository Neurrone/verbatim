//! Commands, their output, typed echo, and a password prompt in a terminal
//! (milestone M4 item 9; `phase6-design.md`, "Terminal end-to-end
//! scenarios"): `windows_terminal_commands` in Windows Terminal,
//! `conhost_commands` in the console host, and `terminal_spoken_password`
//! with the "speak passwords" setting on. The shared setup is described in
//! the `terminal` module.
//!
//! Every scenario first reads the prompt line with the review cursor
//! (numpad 8) and hears "ready>", so typing starts only once Verbatim reads
//! this terminal. Then:
//!
//! 1. It types `echo hello` a character at a time, hearing each echo
//!    before the next (the space as "space", with the letter after it, as
//!    the terminal shows a space only once something follows it). Enter then gives exactly "hello" and
//!    then the prompt, "ready>". This shows echo works in this window, so
//!    the silence that follows means something.
//! 2. It runs `.\password.ps1`, a written script that calls
//!    `Read-Host -AsSecureString "Password"` and then prints "done": its
//!    echo, then the prompt "Password:", is spoken.
//! 3. It types `secret` one character at a time. With "speak passwords" off
//!    (the default, `windows_terminal_commands` and `conhost_commands`),
//!    each character is not spoken: Windows PowerShell's console shows an
//!    asterisk for it, and that new output is what is spoken. With "speak
//!    passwords" on (`terminal_spoken_password`), each character is spoken.
//! 4. Enter: "done", and then "ready>". Every asterisk was spoken as it
//!    was shown, so the line is not read again.
//!
//! Every step asserts exactly what it says before the next key.

use std::io;

use verbatim_config::Settings;

use super::terminal::{self, Echo, PROMPT, Terminal};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// The script the scenario runs: a password prompt, then "done".
pub(crate) const SCRIPTS: &[(&str, &str)] = &[(
    "password.ps1",
    "$secure = Read-Host -AsSecureString 'Password'\r\n'done'\r\n",
)];

/// What is typed at the password prompt.
const SECRET: &str = "secret";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open(scenario, "commands", Terminal::WindowsTerminal, SCRIPTS)
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open(scenario, "console-commands", Terminal::ConsoleHost, SCRIPTS)
}

pub(crate) fn setup_spoken_password(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open(
        scenario,
        "spoken-password",
        Terminal::WindowsTerminal,
        SCRIPTS,
    )
}

/// `terminal_spoken_password`'s settings: "speak passwords" on.
pub(crate) fn speak_passwords(settings: &mut Settings) {
    settings.reader.speak_terminal_passwords = true;
}

/// Steps 1 and 2: `echo hello` with its echo and output, then the password
/// prompt.
fn echo_then_password_prompt(scenario: &mut Scenario, echo: Echo) {
    terminal::type_with_echo(scenario, "echo hello", echo);
    scenario.speech().expect(&["hello", PROMPT]);
    terminal::type_with_echo(scenario, r".\password.ps1", echo);
    scenario.speech().expect(&["Password:"]);
}

/// What the console shows for each character typed at the password
/// prompt: an asterisk. The prompt's space was on the line already, though
/// the outpost could not tell it from padding until the first asterisk
/// followed it, so it is not spoken again.
const MASKS: [&str; 6] = ["*", "*", "*", "*", "*", "*"];

/// Enter at the password prompt: the script's output, and the prompt.
const AFTER_PASSWORD: [&str; 2] = ["done", PROMPT];

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_prompt_read(scenario, state);
    steps(scenario);
}

/// The scenario's steps, with "speak passwords" off, once the prompt has
/// been read, in a terminal whose folder holds [`SCRIPTS`].
pub(crate) fn steps(scenario: &mut Scenario) {
    echo_then_password_prompt(scenario, Echo::Shown);
    for (character, mask) in SECRET.chars().zip(MASKS) {
        scenario
            .type_text(&character.to_string())
            .expect("types a character of the password");
        scenario.speech().expect(&[mask]);
    }
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&AFTER_PASSWORD);
}

pub(crate) fn body_spoken_password(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_prompt_read(scenario, state);
    echo_then_password_prompt(scenario, Echo::Typed);
    for (character, mask) in SECRET.chars().zip(MASKS) {
        let spoken = character.to_string();
        scenario
            .type_text(&spoken)
            .expect("types a character of the password");
        scenario.speech().expect(&[&spoken, mask]);
    }
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario.speech().expect(&AFTER_PASSWORD);
}

pub(crate) use super::no_teardown as teardown;
