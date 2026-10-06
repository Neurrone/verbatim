//! Commands, their output, typed echo, and a password prompt in a terminal
//! (milestone M4 item 9; `phase6-design.md`, "Terminal end-to-end
//! scenarios"): `windows_terminal_commands` in Windows Terminal (the
//! console host, saying so, where Windows Terminal is not installed),
//! `conhost_commands` in the console host, and `terminal_spoken_password`
//! with the "speak passwords" setting on. The shared setup is described in
//! the `terminal` module.
//!
//! Every scenario first reads the prompt line with the review cursor
//! (numpad 8) and hears "ready>", so typing starts only once Verbatim reads
//! this terminal. Then:
//!
//! 1. It types `echo hello`: each character is spoken, exactly and in
//!    order (the space as "space"). Enter then gives exactly "hello" and
//!    then the prompt, "ready>". This shows echo works in this window, so
//!    the silence that follows means something.
//! 2. It runs `.\password.ps1`, a written script that calls
//!    `Read-Host -AsSecureString "Password"` and then prints "done": an
//!    utterance containing "Password:" is spoken.
//! 3. It types `secret` and presses Enter. With "speak passwords" off (the
//!    default, `windows_terminal_commands` and `conhost_commands`), no
//!    utterance from the prompt up to "done" is exactly one of the typed
//!    characters (s, e, c, r, t) or contains "secret". Windows PowerShell's
//!    console shows an asterisk per character, which may be spoken; that is
//!    not asserted either way. With "speak passwords" on
//!    (`terminal_spoken_password`), the six characters are each spoken,
//!    exactly and in order, before Enter is pressed.
//! 4. "done" is spoken, exactly, and then "ready>", which shows Verbatim
//!    did not go silent for another reason.
//!
//! Each step waits for its speech to be heard in full before the next key;
//! there is no other wait.

use std::io;

use verbatim_config::Settings;

use super::terminal::{self, PROMPT, STEP_TIMEOUT, Terminal};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// The script the scenario runs: a password prompt, then "done".
const SCRIPTS: &[(&str, &str)] = &[(
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
fn echo_then_password_prompt(scenario: &mut Scenario, state: &ScenarioState) {
    terminal::expect_prompt_read(scenario, state);
    terminal::type_with_echo(scenario, "echo hello");
    scenario
        .speech()
        .expect_exactly(&["hello", PROMPT], STEP_TIMEOUT);
    terminal::run_command(scenario, r".\password.ps1");
    scenario
        .speech()
        .expect_in_order(&["Password:"], STEP_TIMEOUT);
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    echo_then_password_prompt(scenario, state);
    terminal::run_command(scenario, SECRET);
    let heard = terminal::listen_until(scenario, "done", STEP_TIMEOUT, false);
    let typed = ["s", "e", "c", "r", "t"];
    let spoken: Vec<&str> = heard
        .iter()
        .map(|heard| heard.text.as_str())
        .filter(|text| typed.contains(text) || text.contains(SECRET))
        .collect();
    assert!(
        spoken.is_empty(),
        "the password's characters were spoken: {spoken:?}; everything from the prompt to \"done\": {:?}",
        heard.iter().map(|heard| &heard.text).collect::<Vec<_>>()
    );
    scenario.speech().expect_exactly(&[PROMPT], STEP_TIMEOUT);
}

pub(crate) fn body_spoken_password(scenario: &mut Scenario, state: &mut ScenarioState) {
    echo_then_password_prompt(scenario, state);
    scenario.type_text(SECRET).expect("types the password");
    scenario
        .speech()
        .expect_exactly(&["s", "e", "c", "r", "e", "t"], STEP_TIMEOUT);
    scenario.send_keys(&["enter"]).expect("presses enter");
    scenario
        .speech()
        .expect_exactly(&["done", PROMPT], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    terminal::close(scenario, &state);
}
