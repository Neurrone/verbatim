//! The setup the terminal scenarios share (`phase6-design.md`, "Terminal
//! end-to-end scenarios"), so their results depend neither on the user's
//! own terminals and settings nor on timing.
//!
//! A scenario in Windows Terminal and its twin in the console host are
//! separate code, each with its own expectations (`docs/testing.md`): the
//! helpers here open a terminal and drive the shell, and every expectation
//! is the caller's.
//!
//! Each scenario opens a window of its own, titled with a marker unique to
//! the run ([`harness_marker`]), waits for it to take the foreground, and
//! closes it by that title at cleanup, never by class or program, so the
//! user's own terminals are never touched. Each scenario names its
//! terminal, and gets that one or fails. Windows Terminal is the harness's
//! own portable copy ([`crate::windows_terminal`]), never the installed
//! one or `wt.exe`: its settings folder is deleted, so it starts from the
//! release's defaults, and its `WindowsTerminal.exe` is started directly
//! with `-w new --size 120,30 new-tab --title <title>
//! --suppressApplicationTitle`. The process the harness launched must own
//! the window, no other Windows Terminal process may have opened a window
//! meanwhile, and the process must exit once the window closes. The
//! console host opens with `conhost.exe`, its
//! window titled from its first frame (the launch's console title, without
//! which it shows its own path until the shell sets one), owns its window,
//! and must exit, and the shell sets its size with `mode con cols=120
//! lines=30` and its title. The program that owns the window is
//! asserted; Windows reports a console window as the shell's, so for the
//! console host the window's class is, and that the shell is the launched
//! console host's child. Nothing depends on the machine's name.
//!
//! The shell is Windows PowerShell, present on both, started with
//! `-NoProfile -NoLogo -NoExit -ExecutionPolicy Bypass -File start.ps1`.
//! The start script removes `PSReadLine`, so a line is neither re-rendered
//! nor given predictions, moves to the run's folder, and sets a one-word
//! prompt, `ready> `, spoken as "ready>". It writes the shell's process id
//! to a file, so cleanup can wait for the shell to exit before deleting the
//! folder it ran in. The script then waits, on the folder's change
//! notifications, for a file the scenario writes once Verbatim's
//! announcement of the focused terminal has been asserted, so the first prompt appears only after the outpost
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

use verbatim_agent::protocol::WindowInfo;
use verbatim_control::client::Client as ControlClient;
use verbatim_control::protocol::Frame;
use verbatim_model::NormalizedEvent;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};
use crate::windows_terminal;

/// The prompt line as Verbatim speaks it, without its trailing space.
pub(crate) const PROMPT: &str = "ready>";

/// How long Core is given to receive the caret on the prompt.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the shell is given to show its first prompt.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// The file the prompt writes the first time it runs.
const READY_FILE: &str = "prompt-ready";

/// The file the start script waits for before the shell shows its first
/// prompt, written once Verbatim's announcement of the terminal's focus
/// has been asserted.
const GO_FILE: &str = "prompt-go";

/// The file the start script writes the shell's process id into.
const SHELL_PID_FILE: &str = "shell-pid";

/// The window class only the console host registers.
const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// The terminal a scenario's shell runs in, for launching it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Terminal {
    /// Windows Terminal.
    WindowsTerminal,
    /// The console host, `conhost.exe`.
    ConsoleHost,
}

impl Terminal {
    /// The program that owns the terminal's window.
    fn owner(self) -> &'static str {
        match self {
            Self::WindowsTerminal => "WindowsTerminal.exe",
            Self::ConsoleHost => "conhost.exe",
        }
    }
}

/// Opens Windows Terminal running the shell, as [`open`] describes.
///
/// # Errors
///
/// As [`open`].
pub(crate) fn open_windows_terminal(
    scenario: &mut Scenario,
    name: &str,
    scripts: &[(&str, &str)],
) -> io::Result<ScenarioState> {
    open(scenario, name, Terminal::WindowsTerminal, scripts)
}

/// Opens the console host running the shell, as [`open`] describes.
///
/// # Errors
///
/// As [`open`].
pub(crate) fn open_console_host(
    scenario: &mut Scenario,
    name: &str,
    scripts: &[(&str, &str)],
) -> io::Result<ScenarioState> {
    open(scenario, name, Terminal::ConsoleHost, scripts)
}

/// Writes `scripts` (file names and contents) and the start script into a
/// folder of the run's own, opens `terminal` running the shell in it, with
/// `name` in its title (after "console-" in the console host, so the two
/// terminals' runs of a scenario never share a title or a folder), waits
/// for its window to take the foreground, and
/// asserts the program that owns it. The shell shows its first prompt once
/// [`expect_prompt_read`] lets it. The window is closed at cleanup.
///
/// # Errors
///
/// Returns an error if a file cannot be written, the terminal cannot be
/// started, its window does not take the foreground, or another program
/// owns it.
fn open(
    scenario: &mut Scenario,
    name: &str,
    terminal: Terminal,
    scripts: &[(&str, &str)],
) -> io::Result<ScenarioState> {
    open_with(scenario, name, terminal, scripts, None)
}

/// [`open`], with Windows Terminal's `settings.json` written as
/// `windows_terminal_settings` when given, instead of starting from the
/// release's defaults.
fn open_with(
    scenario: &mut Scenario,
    name: &str,
    terminal: Terminal,
    scripts: &[(&str, &str)],
    windows_terminal_settings: Option<&[u8]>,
) -> io::Result<ScenarioState> {
    let name = match terminal {
        Terminal::WindowsTerminal => name.to_owned(),
        Terminal::ConsoleHost => format!("console-{name}"),
    };
    let title = harness_marker(&name);
    let (directory, start) = prepare_shell(scenario, &name, &title, scripts)?;
    let window = match terminal {
        Terminal::WindowsTerminal => {
            let (folder, executable) = windows_terminal_paths(scenario);
            // Every run starts from the release's default settings.
            let settings = format!(r"{folder}\{}", windows_terminal::SETTINGS_FOLDER);
            scenario.delete_agent_folder(&settings)?;
            if let Some(contents) = windows_terminal_settings {
                scenario.write_agent_file(&format!(r"{settings}\settings.json"), contents)?;
            }
            let others_before = other_terminal_windows(scenario, None)?;
            let mut args = new_window();
            args.extend(new_tab(&title, &start));
            let window = scenario.launch_owning_window(&executable, &args, &title)?;
            require_no_other_terminal_window(scenario, &others_before, window.pid)?;
            window
        }
        Terminal::ConsoleHost => {
            let args = shell_command(&start, terminal);
            scenario.launch_console("conhost.exe", &args, &title)?
        }
    };
    match terminal {
        Terminal::WindowsTerminal => {
            if !window.image.eq_ignore_ascii_case(terminal.owner()) {
                return Err(io::Error::other(format!(
                    "the terminal window {title:?} belongs to {}, not {}",
                    window.image,
                    terminal.owner()
                )));
            }
        }
        Terminal::ConsoleHost => {
            // Windows reports a console window as the console's first
            // client's, here the shell; the window is the console host's
            // when it has the class only the console host registers and
            // that client is the shell the launched console host started.
            let (host, children) = scenario.launched_children()?;
            if window.class != CONSOLE_WINDOW_CLASS
                || !children.iter().any(|child| child.pid == window.pid)
            {
                return Err(io::Error::other(format!(
                    "the terminal window {title:?} (class {}, reported as {} pid {}) is not the                      window of the {} the scenario launched, pid {host}, whose children are {children:?}",
                    window.class,
                    window.image,
                    window.pid,
                    terminal.owner()
                )));
            }
        }
    }
    Ok(ScenarioState::Window {
        pid: window.pid,
        title,
        directory,
    })
}

/// Writes `scripts` and the start script for a shell whose terminal is
/// titled `title` into the run's folder named `name`, and returns the
/// folder and the start script's path.
fn prepare_shell(
    scenario: &mut Scenario,
    name: &str,
    title: &str,
    scripts: &[(&str, &str)],
) -> io::Result<(String, String)> {
    let directory = scenario.harness_folder(name);
    for (file, contents) in scripts {
        scenario.write_agent_file(&format!(r"{directory}\{file}"), contents.as_bytes())?;
    }
    let ready = format!(r"{directory}\{READY_FILE}");
    let go = format!(r"{directory}\{GO_FILE}");
    let shell_pid = format!(r"{directory}\{SHELL_PID_FILE}");
    let start = format!(r"{directory}\start.ps1");
    scenario.write_agent_file(
        &start,
        start_script(title, &directory, &go, &ready, &shell_pid).as_bytes(),
    )?;
    Ok((directory, start))
}

/// The harness's Windows Terminal's arguments for a new window of the
/// test size, before its tabs.
fn new_window() -> Vec<String> {
    ["-w", "new", "--size", "120,30"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// A Windows Terminal tab titled `title` running the shell with the start
/// script `start`.
fn new_tab(title: &str, start: &str) -> Vec<String> {
    let mut args: Vec<String> = ["new-tab", "--title", title, "--suppressApplicationTitle"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    args.extend(shell_command(start, Terminal::WindowsTerminal));
    args
}

/// The harness's Windows Terminal's folder and executable.
fn windows_terminal_paths(scenario: &Scenario) -> (String, String) {
    let folder = format!(r"{}\{}", scenario.run_directory(), windows_terminal::FOLDER);
    let executable = format!(r"{folder}\{}", windows_terminal::EXECUTABLE);
    (folder, executable)
}

/// Fails if a Windows Terminal process other than `own` has a window that
/// was not among `before`: the harness's start was handed to another one.
fn require_no_other_terminal_window(
    scenario: &mut Scenario,
    before: &[WindowInfo],
    own: u32,
) -> io::Result<()> {
    let opened: Vec<WindowInfo> = other_terminal_windows(scenario, Some(own))?
        .into_iter()
        .filter(|after| !before.iter().any(|before| before.window == after.window))
        .collect();
    if opened.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "starting the harness's Windows Terminal (pid {own}) opened windows in another Windows Terminal process: {opened:?}"
        )))
    }
}

/// Opens a second window of the harness's Windows Terminal, whose first
/// window `first` opened ([`open_windows_terminal`]), running the shell in
/// a folder of the run's own named `name`, and brings it forward. The
/// `WindowsTerminal.exe` launched hands its command line to the running
/// one, which must own the new window, and exits; no other Windows
/// Terminal process may open a window. The window is closed by its title
/// at cleanup, and the running Windows Terminal exits once its first
/// window closes too.
///
/// # Errors
///
/// Returns an error if a file cannot be written, the window does not take
/// the foreground, or another process owns it.
pub(crate) fn open_windows_terminal_window(
    scenario: &mut Scenario,
    name: &str,
    first: &ScenarioState,
) -> io::Result<ScenarioState> {
    let ScenarioState::Window { pid: running, .. } = first else {
        panic!("the first window's setup opens a terminal window");
    };
    let running = *running;
    let title = harness_marker(name);
    let (directory, start) = prepare_shell(scenario, name, &title, &[])?;
    let (_, executable) = windows_terminal_paths(scenario);
    let others_before = other_terminal_windows(scenario, Some(running))?;
    let mut args = new_window();
    args.extend(new_tab(&title, &start));
    let window = scenario.launch_titled(&executable, &args, &title, false)?;
    if window.pid != running {
        return Err(io::Error::other(format!(
            "the second window {title:?} belongs to {} pid {}, not to the harness's running Windows Terminal, pid {running}",
            window.image, window.pid
        )));
    }
    require_no_other_terminal_window(scenario, &others_before, running)?;
    Ok(ScenarioState::Window {
        pid: window.pid,
        title,
        directory,
    })
}

/// Opens the harness's Windows Terminal as [`open_windows_terminal`] does,
/// for a scenario that opens a second tab in its window
/// ([`open_windows_terminal_tab`]). Windows Terminal asks before it closes
/// a window of several tabs, so its settings turn that question off
/// (`confirmCloseAllTabs`), the one setting changed from the release's
/// defaults.
///
/// # Errors
///
/// As [`open_windows_terminal`].
pub(crate) fn open_windows_terminal_for_tabs(
    scenario: &mut Scenario,
    name: &str,
) -> io::Result<ScenarioState> {
    open_with(
        scenario,
        name,
        Terminal::WindowsTerminal,
        &[],
        Some(br#"{"confirmCloseAllTabs": false}"#),
    )
}

/// Opens a second tab in the window `first` opened
/// ([`open_windows_terminal_for_tabs`]), running the shell in a folder of
/// the run's own named `name`, titled with the first tab's title and
/// " two", so the window, titled with the tab in front's, is closed by the
/// first's at cleanup whichever is in front. The `WindowsTerminal.exe`
/// launched hands its command line to the running one (`-w 0`, its most
/// recent window) and exits; the new tab is in front once the window's
/// title is its own.
///
/// # Errors
///
/// Returns an error if a file cannot be written, the launch fails or does
/// not exit, or the tab does not come to the front.
pub(crate) fn open_windows_terminal_tab(
    scenario: &mut Scenario,
    name: &str,
    first: &ScenarioState,
) -> io::Result<ScenarioState> {
    let ScenarioState::Window {
        pid, title: first, ..
    } = first
    else {
        panic!("the first tab's setup opens a terminal window");
    };
    let title = format!("{first} two");
    let (directory, start) = prepare_shell(scenario, name, &title, &[])?;
    let (_, executable) = windows_terminal_paths(scenario);
    let mut args: Vec<String> = vec!["-w".to_owned(), "0".to_owned()];
    args.extend(new_tab(&title, &start));
    scenario.run_handing_off(&executable, &args)?;
    let window = scenario.wait_for_window_in_front(&title, STEP_TIMEOUT)?;
    if window.pid != *pid {
        return Err(io::Error::other(format!(
            "the tab {title:?} opened in {} pid {}, not in the harness's running Windows Terminal, pid {pid}",
            window.image, window.pid
        )));
    }
    Ok(ScenarioState::Window {
        pid: *pid,
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
pub(crate) fn wait_for_caret_on(
    events: &mut ControlClient,
    line: &str,
    timeout: Duration,
) -> io::Result<()> {
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

/// The windows of every Windows Terminal process other than `own`, the
/// harness's: compared before and after the harness's starts, they show
/// that no other Windows Terminal process was asked to open a window.
fn other_terminal_windows(
    scenario: &mut Scenario,
    own: Option<u32>,
) -> io::Result<Vec<WindowInfo>> {
    Ok(scenario
        .top_level_windows()?
        .into_iter()
        .filter(|window| {
            window
                .image
                .eq_ignore_ascii_case(Terminal::WindowsTerminal.owner())
                && Some(window.pid) != own
        })
        .collect())
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
/// every terminal's title set, the shell's process id written to
/// `shell_pid`, the run's folder made current, and the prompt, which writes
/// `ready` the first time it runs. The prompt removes `PSReadLine` again,
/// in case the shell loaded it after the script ran. The script ends, and
/// the shell shows its first prompt, once `go` exists: it waits on the
/// folder's change notifications, never polling.
fn start_script(title: &str, directory: &str, go: &str, ready: &str, shell_pid: &str) -> String {
    let title = quoted(title);
    let directory_text = quoted(directory);
    let go_name = quoted(GO_FILE);
    let go = quoted(go);
    let ready = quoted(ready);
    let shell_pid = quoted(shell_pid);
    format!(
        "param([switch]$ConsoleHost)\r\n\
         Remove-Module PSReadLine -ErrorAction SilentlyContinue\r\n\
         if ($ConsoleHost) {{\r\n\
         \x20   mode con cols=120 lines=30 | Out-Null\r\n\
         \x20   $Host.UI.RawUI.BufferSize = New-Object Management.Automation.Host.Size(120, 9001)\r\n\
         }}\r\n\
         $Host.UI.RawUI.WindowTitle = {title}\r\n\
         [IO.File]::WriteAllText({shell_pid}, \"$PID\")\r\n\
         Set-Location -LiteralPath {directory_text}\r\n\
         function global:prompt {{\r\n\
         \x20   Remove-Module PSReadLine -ErrorAction SilentlyContinue\r\n\
         \x20   if (-not (Test-Path -LiteralPath {ready})) {{ [IO.File]::WriteAllText({ready}, '') }}\r\n\
         \x20   'ready> '\r\n\
         }}\r\n\
         $watcher = New-Object IO.FileSystemWatcher({directory_text}, {go_name})\r\n\
         if (-not (Test-Path -LiteralPath {go})) {{ $null = $watcher.WaitForChanged('Created') }}\r\n\
         $watcher.Dispose()\r\n"
    )
}

/// `text` as a PowerShell single-quoted string.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// What Verbatim echoes for each character of `text` typed: the character
/// itself, and "space" for a space.
#[cfg(test)]
pub(crate) fn echo_of(text: &str) -> Vec<String> {
    text.chars().map(super::character_name).collect()
}

/// The title of the terminal window a scenario's setup opened.
pub(crate) fn title(state: &ScenarioState) -> &str {
    let ScenarioState::Window { title, .. } = state else {
        panic!("a terminal scenario's setup opens a terminal window");
    };
    title
}

/// Asserts that Verbatim says exactly `announcement` as the terminal's
/// window takes the foreground, lets the shell show its first prompt, and
/// reads the review
/// cursor's line, which follows the caret onto it: the evidence that
/// Verbatim reads this terminal's text before a scenario types into it.
///
/// The prompt, shown only once the go file exists, is new output, heard in
/// full. The console host reports its caret some time after its text, and
/// the review cursor follows the caret, so the line is read only once Core
/// has received the caret on the prompt.
pub(crate) fn expect_prompt_read(
    scenario: &mut Scenario,
    state: &ScenarioState,
    announcement: &[&str],
) {
    let ScenarioState::Window {
        directory, title, ..
    } = state
    else {
        panic!("a terminal scenario's setup opens a terminal window");
    };
    scenario.speech().expect(announcement);
    let mut events = scenario
        .subscribe_events()
        .expect("subscribes to Verbatim's events");
    scenario
        .write_agent_file(&format!(r"{directory}\{GO_FILE}"), b"")
        .expect("writes the go file");
    scenario
        .wait_for_agent_file(&format!(r"{directory}\{READY_FILE}"), READY_TIMEOUT)
        .expect("the shell shows its first prompt");
    scenario.speech().expect(&[PROMPT]);
    wait_for_caret_on(&mut events, PROMPT, STEP_TIMEOUT)
        .expect("Core receives the caret on the prompt");
    let shell = scenario
        .wait_for_agent_file(&format!(r"{directory}\{SHELL_PID_FILE}"), READY_TIMEOUT)
        .expect("the shell writes its process id");
    let shell: u32 = String::from_utf8_lossy(&shell)
        .trim()
        .parse()
        .expect("the shell's process id is a number");
    scenario.expect_exit_at_cleanup(title, shell);
    scenario
        .send_gesture("kb:numpad8")
        .expect("sends the read-line gesture");
    scenario.speech().expect(&[PROMPT]);
}

/// Where a terminal's echo of typing comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Echo {
    /// What the terminal shows, as by default.
    Shown,
    /// Each key as it is typed, as with "Speak passwords" on.
    Typed,
}

/// Types `text` a character at a time, as a listening user types, hearing
/// each character's echo in full before the next: typed while an echo
/// plays, a character would cut it off. When the echo is what the terminal
/// shows ([`Echo::Shown`]), a space it shows at the end of a line cannot be
/// told from padding until something follows it, so a space is typed with
/// the character after it, and the two echoes are heard together.
///
/// # Panics
///
/// Panics if `text` ends with a space and the echo is what the terminal
/// shows, since that space's echo would never come.
pub(crate) fn type_hearing(scenario: &mut Scenario, text: &str, echo: Echo) {
    assert!(
        echo == Echo::Typed || !text.ends_with(' '),
        "a space typed last is never echoed: {text:?}"
    );
    let mut typed = String::new();
    for character in text.chars() {
        typed.push(character);
        if character == ' ' && echo == Echo::Shown {
            continue;
        }
        scenario.type_text(&typed).expect("types a character");
        let echo: Vec<String> = typed.chars().map(super::character_name).collect();
        let echo: Vec<&str> = echo.iter().map(String::as_str).collect();
        scenario.speech().expect(&echo);
        typed.clear();
    }
}

/// Types `text` as [`type_hearing`] does, then presses Enter.
pub(crate) fn type_with_echo(scenario: &mut Scenario, text: &str, echo: Echo) {
    type_hearing(scenario, text, echo);
    scenario.send_keys(&["enter"]).expect("presses enter");
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
        let script = start_script(
            "it's",
            r"C:\run's",
            r"C:\run's\go",
            r"C:\run's\ready",
            r"C:\run's\pid",
        );
        assert!(script.contains("WindowTitle = 'it''s'"));
        assert!(script.contains(r"Set-Location -LiteralPath 'C:\run''s'"));
        assert!(script.contains(r"WriteAllText('C:\run''s\ready', '')"));
        assert!(script.contains("    'ready> '\r\n"));
        assert!(script.contains(r"WriteAllText('C:\run''s\pid', "));
        assert!(script.contains(r"New-Object IO.FileSystemWatcher('C:\run''s', 'prompt-go')"));
        assert!(!script.contains("Sleep"));
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
