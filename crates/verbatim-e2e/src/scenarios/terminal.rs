//! The setup the terminal scenarios share (`phase6-design.md`, "Terminal
//! end-to-end scenarios"), so their results depend neither on the user's
//! own terminals and settings nor on timing.
//!
//! Each scenario opens a window of its own, titled with a marker unique to
//! the run ([`harness_marker`]), and finds, brings forward, and closes it
//! by that title, never by class or program, so the user's own terminals
//! are never touched; nothing here ends `WindowsTerminal.exe` or
//! `conhost.exe` by name. Windows Terminal opens with `wt.exe -w new
//! --size 120,30 new-tab --title <title> --suppressApplicationTitle`; its
//! window may belong to a Windows Terminal process the user's own windows
//! share, so a window that will not close is reported and left open, never
//! terminated. The console host opens with `conhost.exe`, and the shell
//! sets its size with `mode con cols=120 lines=30` and its title. Which
//! terminal a scenario gets is decided by what is installed: a scenario
//! that prefers Windows Terminal asks the agent to start `wt.exe`, and when
//! that fails, because Windows Terminal is not installed (as on a Windows
//! Server runner), it says so and uses the console host. Nothing depends on
//! the machine's name.
//!
//! The shell is Windows PowerShell, present on both, started with
//! `-NoProfile -NoLogo -NoExit -ExecutionPolicy Bypass -File start.ps1`.
//! The start script removes `PSReadLine`, so a line is neither re-rendered
//! nor given predictions, moves to the run's folder, and sets a one-word
//! prompt, `ready> `, spoken as "ready>". The prompt also writes a file
//! the first time it runs, the evidence that the shell is waiting for
//! input. Everything a scenario runs is a small script written into that
//! folder before the window opens, so the typed command is short (such as
//! `.\flood.ps1 1`) and the output is exactly known. Commands are typed
//! with the agent's `TypeText`, and Enter is pressed with `SendKeys`.

use std::io;
use std::time::{Duration, Instant};

use verbatim_control::protocol::{Frame, ReplyPayload, Request};

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};
use crate::speech::Heard;

/// The prompt line as Verbatim speaks it, without its trailing space.
pub(crate) const PROMPT: &str = "ready>";

/// How long each step's speech is given to arrive.
pub(crate) const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a terminal's window is given to appear.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the shell is given to show its first prompt.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long one read of speech lasts before the caller checks its own
/// deadline and, when asked, Verbatim's control plane.
const LISTEN_SLICE: Duration = Duration::from_millis(500);

/// The file the prompt writes the first time it runs.
const READY_FILE: &str = "prompt-ready";

/// The terminal a scenario's shell runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Terminal {
    /// Windows Terminal, when it is installed; the console host otherwise.
    WindowsTerminal,
    /// The console host, `conhost.exe`.
    ConsoleHost,
}

/// Writes `scripts` (file names and contents) and the start script into a
/// folder of the run's own, opens a terminal running the shell in it, with
/// `name` in its title, brings its window forward, and waits for the
/// shell's first prompt. The window is closed by [`close`].
///
/// # Errors
///
/// Returns an error if a file cannot be written, no terminal can be
/// started, its window does not take the foreground, or the shell shows no
/// prompt in time.
pub(crate) fn open(
    scenario: &mut Scenario,
    name: &str,
    preferred: Terminal,
    scripts: &[(&str, &str)],
) -> io::Result<ScenarioState> {
    let title = harness_marker(name);
    let directory = format!(r"{}\{title}", scenario.run_directory()?);
    for (file, contents) in scripts {
        scenario.write_agent_file(&format!(r"{directory}\{file}"), contents.as_bytes())?;
    }
    let ready = format!(r"{directory}\{READY_FILE}");
    let start = format!(r"{directory}\start.ps1");
    scenario.write_agent_file(&start, start_script(&title, &directory, &ready).as_bytes())?;

    let pid = launch(scenario, preferred, &title, &start)?;
    let image = scenario.bring_titled_window_forward(&title, WINDOW_TIMEOUT)?;
    println!("the terminal window {title:?} belongs to {image}");
    scenario.wait_for_agent_file(&ready, READY_TIMEOUT)?;
    Ok(ScenarioState::Window {
        pid,
        title,
        directory,
    })
}

/// Starts the terminal: Windows Terminal when it is preferred and can be
/// started, the console host otherwise. Returns the launch's pid.
fn launch(
    scenario: &mut Scenario,
    preferred: Terminal,
    title: &str,
    start: &str,
) -> io::Result<u32> {
    if preferred == Terminal::WindowsTerminal {
        let mut args: Vec<String> = [
            "-w",
            "new",
            "--size",
            "120,30",
            "new-tab",
            "--title",
            title,
            "--suppressApplicationTitle",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        args.extend(shell_command(start, Terminal::WindowsTerminal));
        match scenario.launch_titled("wt.exe", &args, title, false) {
            Ok(pid) => {
                println!("terminal: Windows Terminal");
                return Ok(pid);
            }
            Err(error) => println!(
                "terminal: the console host, since Windows Terminal could not be started: {error}"
            ),
        }
    } else {
        println!("terminal: the console host");
    }
    let args = shell_command(start, Terminal::ConsoleHost);
    scenario.launch_titled("conhost.exe", &args, title, true)
}

/// The shell's command line, running the start script.
fn shell_command(start: &str, terminal: Terminal) -> Vec<String> {
    let mut command: Vec<String> = [
        "powershell.exe",
        "-NoProfile",
        "-NoLogo",
        "-NoExit",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        start,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if terminal == Terminal::ConsoleHost {
        command.push("-ConsoleHost".to_owned());
    }
    command
}

/// The start script: `PSReadLine` removed, the console host's size and
/// every terminal's title set, the run's folder made current, and the
/// prompt, which writes `ready` the first time it runs. The prompt removes
/// `PSReadLine` again, in case the shell loaded it after the script ran.
fn start_script(title: &str, directory: &str, ready: &str) -> String {
    let title = quoted(title);
    let directory = quoted(directory);
    let ready = quoted(ready);
    format!(
        "param([switch]$ConsoleHost)\r\n\
         Remove-Module PSReadLine -ErrorAction SilentlyContinue\r\n\
         if ($ConsoleHost) {{ mode con cols=120 lines=30 | Out-Null }}\r\n\
         $Host.UI.RawUI.WindowTitle = {title}\r\n\
         Set-Location -LiteralPath {directory}\r\n\
         function global:prompt {{\r\n\
         \x20   Remove-Module PSReadLine -ErrorAction SilentlyContinue\r\n\
         \x20   if (-not (Test-Path -LiteralPath {ready})) {{ [IO.File]::WriteAllText({ready}, '') }}\r\n\
         \x20   'ready> '\r\n\
         }}\r\n"
    )
}

/// `text` as a PowerShell single-quoted string.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// What Verbatim echoes for each character of `text` typed: the character
/// itself, and "space" for a space.
pub(crate) fn echo_of(text: &str) -> Vec<String> {
    text.chars()
        .map(|character| {
            if character == ' ' {
                "space".to_owned()
            } else {
                character.to_string()
            }
        })
        .collect()
}

/// Reads the review cursor's line, which follows the caret onto the
/// prompt: the evidence that Verbatim reads this terminal's text before a
/// scenario types into it.
pub(crate) fn expect_prompt_read(scenario: &mut Scenario) {
    scenario
        .send_gesture("kb:numpad8")
        .expect("sends the read-line gesture");
    scenario.speech().expect_in_order(&[PROMPT], STEP_TIMEOUT);
}

/// Types `command` and presses Enter.
pub(crate) fn run_command(scenario: &mut Scenario, command: &str) {
    scenario.type_text(command).expect("types the command");
    scenario.send_keys(&["enter"]).expect("presses enter");
}

/// Types `text`, waits until every character's echo has been queued, in
/// order, and the last heard in full, then presses Enter.
pub(crate) fn type_with_echo(scenario: &mut Scenario, text: &str) {
    scenario.type_text(text).expect("types the text");
    let echo = echo_of(text);
    let echo: Vec<&str> = echo.iter().map(String::as_str).collect();
    scenario.speech().expect_exactly(&echo, STEP_TIMEOUT);
    scenario.send_keys(&["enter"]).expect("presses enter");
}

/// Fails unless Verbatim's control plane answers a status request, within
/// the connection's read timeout.
pub(crate) fn expect_status_answers(scenario: &mut Scenario) {
    match scenario.control().request(Request::Status) {
        Ok(Frame::Reply {
            payload: ReplyPayload::Status(_),
            ..
        }) => {}
        other => panic!("Verbatim's control plane did not answer a status request: {other:?}"),
    }
}

/// Every utterance queued from now until one exactly `last`, which is the
/// final one returned and is waited for until heard in full. With
/// `poll_status`, Verbatim's control plane is asked for its status between
/// reads, so a Verbatim that stops answering fails the scenario rather than
/// stalling it.
///
/// # Panics
///
/// Panics, naming the last things heard, if `last` is not queued within
/// `timeout`, if it is not heard in full, or if the control plane stops
/// answering.
pub(crate) fn listen_until(
    scenario: &mut Scenario,
    last: &str,
    timeout: Duration,
    poll_status: bool,
) -> Vec<Heard> {
    let deadline = Instant::now() + timeout;
    let mut heard: Vec<Heard> = Vec::new();
    loop {
        heard.extend(scenario.speech().take_heard(last, LISTEN_SLICE));
        if poll_status {
            expect_status_answers(scenario);
        }
        if let Some(found) = heard.last().filter(|heard| heard.text == last) {
            let found = found.clone();
            scenario.speech().expect_completed(&found);
            return heard;
        }
        if Instant::now() >= deadline {
            let recent: Vec<&str> = heard
                .iter()
                .rev()
                .take(40)
                .rev()
                .map(|heard| heard.text.as_str())
                .collect();
            panic!(
                "{last:?} was not spoken within {timeout:?}; the last things queued were: {recent:?}"
            );
        }
    }
}

/// Closes the scenario's terminal window by its title.
pub(crate) fn close(scenario: &mut Scenario, state: &ScenarioState) {
    if let ScenarioState::Window { pid, .. } = state {
        scenario
            .kill_target(*pid)
            .expect("closes the terminal window by its title");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_space_is_echoed_by_name() {
        assert_eq!(echo_of("echo hi"), ["e", "c", "h", "o", "space", "h", "i"]);
    }

    #[test]
    fn the_start_script_quotes_what_it_names() {
        let script = start_script("it's", r"C:\run's", r"C:\run's\ready");
        assert!(script.contains("WindowTitle = 'it''s'"));
        assert!(script.contains(r"Set-Location -LiteralPath 'C:\run''s'"));
        assert!(script.contains(r"WriteAllText('C:\run''s\ready', '')"));
        assert!(script.contains("    'ready> '\r\n"));
    }

    #[test]
    fn only_the_console_host_is_sized_by_the_shell() {
        assert!(
            shell_command("start.ps1", Terminal::ConsoleHost).contains(&"-ConsoleHost".to_owned())
        );
        assert!(
            !shell_command("start.ps1", Terminal::WindowsTerminal)
                .contains(&"-ConsoleHost".to_owned())
        );
    }
}
