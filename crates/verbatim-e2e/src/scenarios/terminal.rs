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
//! that fails, because Windows Terminal is not installed, it says so and
//! uses the console host. GitHub's Windows Server runner image ships
//! Windows Terminal, and CI's `e2e` job checks it is there, so there it is
//! used too. Nothing depends on the
//! machine's name.
//!
//! The shell is Windows PowerShell, present on both, started with
//! `-NoProfile -NoLogo -NoExit -ExecutionPolicy Bypass -File start.ps1`.
//! The start script removes `PSReadLine`, so a line is neither re-rendered
//! nor given predictions, moves to the run's folder, and sets a one-word
//! prompt, `ready> `, spoken as "ready>". The script then waits for a file
//! the scenario writes once Verbatim's announcement of the focused terminal
//! has been heard out, so the first prompt appears only after the outpost
//! has read where the terminal's text ends, and is spoken as new output,
//! which the scenario hears in full before it goes on: the evidence that
//! Verbatim follows this terminal's text. It then waits for Core to receive
//! the caret on the prompt line (an event, through a subscription of its
//! own), since the review cursor follows the caret and the console host
//! reports its caret some time after its text. The prompt also writes a file the
//! first time it runs, the evidence that the shell is waiting for input.
//! Everything a scenario runs is a small script written into that
//! folder before the window opens, so the typed command is short (such as
//! `.\flood.ps1 1`) and the output is exactly known. Commands are typed
//! with the agent's `TypeText`, and Enter is pressed with `SendKeys`.

use std::io;
use std::time::{Duration, Instant};

use verbatim_control::client::Client as ControlClient;
use verbatim_control::protocol::{Frame, ReplyPayload, Request};
use verbatim_model::NormalizedEvent;

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

/// The file the start script waits for before the shell shows its first
/// prompt, written once Verbatim has announced the terminal's focus.
const GO_FILE: &str = "prompt-go";

/// What Verbatim's announcement of a focused terminal contains: its role.
const FOCUSED_TERMINAL: &str = "terminal";

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
/// `name` in its title, and brings its window forward. The shell shows its
/// first prompt once [`expect_prompt_read`] lets it. The window is closed
/// by [`close`].
///
/// # Errors
///
/// Returns an error if a file cannot be written, no terminal can be
/// started, or its window does not take the foreground.
pub(crate) fn open(
    scenario: &mut Scenario,
    name: &str,
    preferred: Terminal,
    scripts: &[(&str, &str)],
) -> io::Result<ScenarioState> {
    open_with(scenario, name, preferred, true, scripts)
}

/// [`open`] in Windows Terminal, with no console host in its place: for a
/// demonstration of Windows Terminal, which fails when it cannot be
/// started.
///
/// # Errors
///
/// As [`open`], and when Windows Terminal cannot be started.
pub(crate) fn open_windows_terminal_only(
    scenario: &mut Scenario,
    name: &str,
    scripts: &[(&str, &str)],
) -> io::Result<ScenarioState> {
    open_with(scenario, name, Terminal::WindowsTerminal, false, scripts)
}

/// [`open`], using the console host when Windows Terminal is preferred but
/// cannot be started only when `console_host_fallback` is set.
fn open_with(
    scenario: &mut Scenario,
    name: &str,
    preferred: Terminal,
    console_host_fallback: bool,
    scripts: &[(&str, &str)],
) -> io::Result<ScenarioState> {
    let title = harness_marker(name);
    let directory = scenario.harness_folder(name)?;
    for (file, contents) in scripts {
        scenario.write_agent_file(&format!(r"{directory}\{file}"), contents.as_bytes())?;
    }
    let ready = format!(r"{directory}\{READY_FILE}");
    let go = format!(r"{directory}\{GO_FILE}");
    let start = format!(r"{directory}\start.ps1");
    scenario.write_agent_file(
        &start,
        start_script(&title, &directory, &go, &ready).as_bytes(),
    )?;

    // What Verbatim said before the window opens, such as another
    // terminal's focus, is not taken for this one's.
    scenario.speech().wait_until_quiet(STEP_TIMEOUT);
    let pid = launch(scenario, preferred, console_host_fallback, &title, &start)?;
    let image = scenario.bring_titled_window_forward(&title, WINDOW_TIMEOUT)?;
    println!("the terminal window {title:?} belongs to {image}");
    Ok(ScenarioState::Window {
        pid,
        title,
        directory,
    })
}

/// Waits until `events`, a subscription to the events Core receives,
/// carries a caret report whose line, trailing white space aside, is
/// `line`.
///
/// # Errors
///
/// Returns an error if no such report arrives within `timeout`, or the
/// connection fails.
fn wait_for_caret_on(events: &mut ControlClient, line: &str, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match events.next_frame() {
            Ok(Frame::Event {
                event: NormalizedEvent::CaretMoved { caret, .. },
                ..
            }) if caret.line.text.trim_end() == line => return Ok(()),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "Core received no caret on the line {line:?} within {timeout:?}"
            )));
        }
    }
}

/// Starts the terminal: Windows Terminal when it is preferred and can be
/// started, the console host otherwise, unless `console_host_fallback` is
/// off. Returns the launch's pid.
fn launch(
    scenario: &mut Scenario,
    preferred: Terminal,
    console_host_fallback: bool,
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
            Err(error) if !console_host_fallback => {
                return Err(io::Error::other(format!(
                    "this scenario needs Windows Terminal, which could not be started: {error}"
                )));
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
/// The script ends, and the shell shows its first prompt, once `go` exists.
fn start_script(title: &str, directory: &str, go: &str, ready: &str) -> String {
    let title = quoted(title);
    let directory = quoted(directory);
    let go = quoted(go);
    let ready = quoted(ready);
    format!(
        "param([switch]$ConsoleHost)\r\n\
         Remove-Module PSReadLine -ErrorAction SilentlyContinue\r\n\
         if ($ConsoleHost) {{\r\n\
         \x20   mode con cols=120 lines=30 | Out-Null\r\n\
         \x20   $Host.UI.RawUI.BufferSize = New-Object Management.Automation.Host.Size(120, 9001)\r\n\
         }}\r\n\
         $Host.UI.RawUI.WindowTitle = {title}\r\n\
         Set-Location -LiteralPath {directory}\r\n\
         function global:prompt {{\r\n\
         \x20   Remove-Module PSReadLine -ErrorAction SilentlyContinue\r\n\
         \x20   if (-not (Test-Path -LiteralPath {ready})) {{ [IO.File]::WriteAllText({ready}, '') }}\r\n\
         \x20   'ready> '\r\n\
         }}\r\n\
         while (-not (Test-Path -LiteralPath {go})) {{ Start-Sleep -Milliseconds 20 }}\r\n"
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

/// Lets the shell in the window [`open`] opened show its first prompt, and
/// reads the review cursor's line, which follows the caret onto it: the
/// evidence that Verbatim reads this terminal's text before a scenario
/// types into it.
///
/// First the terminal's focus is heard out: by then the outpost has read
/// where its text ends, so the prompt, shown only once the go file exists,
/// is new output, heard in full. The console host reports its caret some
/// time after its text, and the review cursor follows the caret, so the
/// line is read only once Core has received the caret on the prompt.
pub(crate) fn expect_prompt_read(scenario: &mut Scenario, state: &ScenarioState) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("a terminal scenario's setup opens a terminal window");
    };
    let focused = scenario
        .speech()
        .expect_in_order_capturing(&[FOCUSED_TERMINAL], STEP_TIMEOUT);
    // The console host names its text area "Text Area", in English only;
    // NVDA drops the name, and so does Verbatim.
    assert!(
        !focused.contains("Text Area"),
        "the terminal was announced as {focused:?}"
    );
    scenario.speech().wait_until_quiet(STEP_TIMEOUT);
    let mut events = scenario
        .subscribe_events()
        .expect("subscribes to Verbatim's events");
    scenario
        .write_agent_file(&format!(r"{directory}\{GO_FILE}"), b"")
        .expect("writes the go file");
    scenario
        .wait_for_agent_file(&format!(r"{directory}\{READY_FILE}"), READY_TIMEOUT)
        .expect("the shell shows its first prompt");
    scenario.speech().expect_exactly(&[PROMPT], STEP_TIMEOUT);
    wait_for_caret_on(&mut events, PROMPT, STEP_TIMEOUT)
        .expect("Core receives the caret on the prompt");
    scenario
        .send_gesture("kb:numpad8")
        .expect("sends the read-line gesture");
    scenario.speech().expect_exactly(&[PROMPT], STEP_TIMEOUT);
}

/// Types `command` and presses Enter.
pub(crate) fn run_command(scenario: &mut Scenario, command: &str) {
    scenario.type_text(command).expect("types the command");
    scenario.send_keys(&["enter"]).expect("presses enter");
}

/// Types `command`, waits until the echo of its end, from its last space,
/// has been heard in full, and presses Enter: the terminal has then shown
/// the command line as typed, so running it adds only the command's own
/// output. A command typed and entered faster than the terminal shows it
/// is read back as output instead, since Enter drops typing not yet shown.
pub(crate) fn run_command_after_echo(scenario: &mut Scenario, command: &str) {
    scenario.type_text(command).expect("types the command");
    let end = command
        .rfind(' ')
        .map_or(command, |space| &command[space..]);
    let echo = echo_of(end);
    let echo: Vec<&str> = echo.iter().map(String::as_str).collect();
    scenario.speech().expect_exactly(&echo, STEP_TIMEOUT);
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
        let script = start_script("it's", r"C:\run's", r"C:\run's\go", r"C:\run's\ready");
        assert!(script.contains("WindowTitle = 'it''s'"));
        assert!(script.contains(r"Set-Location -LiteralPath 'C:\run''s'"));
        assert!(script.contains(r"WriteAllText('C:\run''s\ready', '')"));
        assert!(script.contains("    'ready> '\r\n"));
        assert!(script.ends_with(
            "while (-not (Test-Path -LiteralPath 'C:\\run''s\\go')) { Start-Sleep -Milliseconds 20 }\r\n"
        ));
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
