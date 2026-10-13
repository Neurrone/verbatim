//! The measure set: the measurement workloads that
//! `docs/design/communication.md` section 11 runs at every migration step,
//! its first step's baseline and every later step's comparison. They
//! measure nothing themselves: they give Verbatim focus changes, typing and
//! Backspace to handle, and its latency ledger logs the figures, one line
//! per announcement at info on the `verbatim::latency` target, which a run
//! keeps in its artifacts. They are the registry's `measure` group, left
//! out of every run that does not name them, like the demonstrations, and
//! they are measurement, not tests (`docs/testing.md`).
//!
//! They follow the harness's rules: every scenario starts from the
//! minimized desktop and asserts Verbatim's startup speech; the window a
//! key goes to is confirmed in front before every key; every wait is for
//! evidence (Verbatim idle after the last input, a window in front, a file
//! a script writes). Windows are switched as a taskbar button does, never
//! with Alt+Tab. They assert nothing beyond completing: the registry runs
//! the `measure` group with the speech collector in collect-only mode,
//! which never fails on unasserted speech. Just before every input,
//! everything Verbatim has queued is taken, not asserted, and written with
//! the ground truth of each step (what was typed, what a key removed, and
//! the harness numbers of its keys) into `measure-truth.jsonl` in the
//! scenario's artifacts directory.
//!
//! Typing and Backspace are paced by the agent (`SendKeysPaced`,
//! `TypeTextPaced`): the pace is the workload, a typist's or keyboard
//! auto-repeat's rate, not a wait. The ticker script prints a line every
//! 50 ms for the same reason.

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};

use super::terminal::{self, PROMPT};
use super::text_box::{self, BOX_NAME};
use crate::artifacts;
use crate::registry::ScenarioState;
use crate::scenario::{Scenario, WINDOW_TIMEOUT, harness_marker};

pub(crate) use super::no_teardown as teardown;

/// How many times each focus change is repeated, for percentiles.
const REPEATS: usize = 20;

/// How many fresh applications are started, each with a fresh outpost.
const FRESH_APPLICATIONS: usize = 12;

/// How many new File Explorer windows are opened, each its first focus.
const NEW_EXPLORER_WINDOWS: usize = 10;

/// Typed at 150 ms and 60 ms a character.
const TYPED: &str = "the quick brown fox jumps over the lazy dog";

/// Typed in a terminal, on the prompt's line.
const TYPED_COMMAND: &str = "echo the quick brown fox";

/// Typed while the ticker prints.
const TYPED_DURING_OUTPUT: &str = "echo typed during output";

/// How many rounds of typing and Backspace each key workload makes.
const ROUNDS: usize = 3;

/// How many times a wrapped line is typed and cleared with Escape.
const WRAPPED_ROUNDS: usize = 5;

/// Starts `ticker-run.ps1` sharing the terminal's console, so the shell
/// returns to its prompt and reads typing while the ticker prints.
const TICKER: &str = "Start-Process powershell.exe -NoNewWindow -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',(Join-Path $PSScriptRoot 'ticker-run.ps1')\r\n";

/// Prints a line every 50 ms, 200 lines, then writes `ticked`. The pace is
/// the workload's rate of output.
const TICKER_RUN: &str = "for ($tick = 1; $tick -le 200; $tick++) { \"tick $tick\"; [System.Threading.Tasks.Task]::Delay(50).Wait() }\r\n\
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'ticked'), '')\r\n";

/// The ground truth file of the running scenario.
struct Truth {
    name: &'static str,
}

impl Truth {
    /// Appends one step's ground truth.
    fn write(&self, step: &serde_json::Value) {
        let dir = artifacts::scenario_dir(&artifacts::artifacts_root(), self.name);
        fs::create_dir_all(&dir).expect("creates the artifacts directory");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("measure-truth.jsonl"))
            .expect("opens the ground truth file");
        writeln!(file, "{step}").expect("writes the ground truth");
    }

    /// Takes everything Verbatim has queued once it is idle after the last
    /// input, and every frame that has reached the speech connection since
    /// (`Scenario::take_all_queued`), and writes it with `step`, unasserted.
    /// Called just before every input, so the harness's check that nothing
    /// unasserted waits before an input finds nothing.
    fn drain(&self, scenario: &mut Scenario, step: &str) {
        let heard: Vec<String> = scenario
            .take_all_queued()
            .into_iter()
            .map(|heard| heard.text)
            .collect();
        self.write(&serde_json::json!({ "step": step, "heard": heard }));
    }
}

/// Confirms that the window whose title contains `title` is in front.
fn in_front(scenario: &mut Scenario, title: &str) {
    scenario
        .wait_for_window_in_front(title, WINDOW_TIMEOUT)
        .unwrap_or_else(|error| panic!("{title:?} is not in front: {error}"));
}

/// Readies the next input to the window whose title contains `title`:
/// confirms it is in front, then takes everything said so far.
fn ready(scenario: &mut Scenario, truth: &Truth, title: &str) {
    in_front(scenario, title);
    truth.drain(scenario, "before input");
}

/// Types `text` one character every `interval_ms` into the window titled
/// `title`, recording it as typed.
fn type_paced(
    scenario: &mut Scenario,
    truth: &Truth,
    (title, target): (&str, &str),
    text: &str,
    interval_ms: u32,
) {
    ready(scenario, truth, title);
    let (first, last) = scenario
        .type_text_paced(text, interval_ms)
        .expect("types the text");
    truth.write(&serde_json::json!({
        "step": "type", "target": target, "first": first, "last": last,
        "interval_ms": interval_ms, "text": text,
    }));
    truth.drain(scenario, "after typing");
}

/// Presses Backspace once for each character of `typed`, the text before
/// the caret, one every 33 ms (auto-repeat's rate), recording what each
/// press removes.
fn backspace_paced(
    scenario: &mut Scenario,
    truth: &Truth,
    (title, target): (&str, &str),
    typed: &str,
) {
    ready(scenario, truth, title);
    let count = typed.chars().count();
    let keys = vec!["backspace"; count];
    let (first, last) = scenario
        .send_keys_paced(&keys, 33)
        .expect("presses backspace");
    let removed: Vec<String> = typed.chars().rev().map(String::from).collect();
    truth.write(&serde_json::json!({
        "step": "backspace", "target": target, "first": first, "last": last,
        "interval_ms": 33, "removed": removed,
    }));
    truth.drain(scenario, "after backspace");
}

/// Presses Escape, which clears the line being typed, `typed`.
fn escape(scenario: &mut Scenario, truth: &Truth, (title, target): (&str, &str), typed: &str) {
    ready(scenario, truth, title);
    scenario.send_keys(&["escape"]).expect("presses escape");
    truth.write(&serde_json::json!({
        "step": "escape", "target": target, "removed": typed,
    }));
    truth.drain(scenario, "after escape");
}

/// Brings the windows titled `first` and `second` forward by turns,
/// `REPEATS` times, as clicking a taskbar button does
/// (`Scenario::bring_window_forward`, which names the window it activates
/// and so never reaches a window the scenario did not open), taking what
/// is said after each. Never Alt+Tab, whose order is not Z order.
fn switch_between(scenario: &mut Scenario, truth: &Truth, first: &str, second: &str) {
    for index in 0..REPEATS {
        let to = if index % 2 == 0 { first } else { second };
        truth.drain(scenario, "before switching");
        scenario
            .bring_window_forward(to)
            .expect("brings the window forward");
        truth.write(&serde_json::json!({ "step": "bring_forward", "to": to }));
        truth.drain(scenario, "after switching");
    }
}

pub(crate) fn setup_focus_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "m-wt-a", &[])
}

/// `measure_focus_windows_terminal`: two Windows Terminal windows, brought
/// forward by turns as their taskbar buttons do.
pub(crate) fn body_focus_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_focus_windows_terminal",
    };
    let first = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[
            &format!("{first} window"),
            &format!("{first} terminal"),
            "blank",
        ],
    );
    let second_state = terminal::open_windows_terminal_window(scenario, "m-wt-b", state)
        .expect("opens a second Windows Terminal window");
    let second = terminal::title(&second_state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        &second_state,
        &[
            &format!("{second} window"),
            &format!("{second} terminal"),
            "blank",
        ],
    );
    switch_between(scenario, &truth, &first, &second);
}

pub(crate) fn setup_focus_tabs(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal_for_tabs(scenario, "m-tabs")
}

/// `measure_focus_windows_terminal_tabs`: two tabs of one Windows Terminal
/// window, switched between with Control+Tab and Control+Shift+Tab.
pub(crate) fn body_focus_tabs(scenario: &mut Scenario, state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_focus_windows_terminal_tabs",
    };
    let first = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[
            &format!("{first} window"),
            &format!("{first} terminal"),
            "blank",
        ],
    );
    let second_state = terminal::open_windows_terminal_tab(scenario, "m-tabs-two", state)
        .expect("opens a second tab");
    let second = terminal::title(&second_state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        &second_state,
        &[&format!("{second} terminal"), "blank"],
    );
    for index in 0..REPEATS {
        // The second tab's title contains the first's, so the window is
        // confirmed in front by the first's.
        ready(scenario, &truth, &first);
        let key = if index % 2 == 0 {
            "control+shift+tab"
        } else {
            "control+tab"
        };
        scenario.send_keys(&[key]).expect("switches tabs");
        if index % 2 == 1 {
            in_front(scenario, &second);
        }
        truth.write(&serde_json::json!({ "step": "tab", "key": key }));
        truth.drain(scenario, "after tab");
    }
}

pub(crate) fn setup_focus_explorer(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let title = scenario.open_folder("m-folder", &["alpha", "beta", "gamma", "Inner\\delta"])?;
    Ok(ScenarioState::Title(title))
}

/// `measure_focus_explorer`: moving in a File Explorer folder window, and
/// into its subfolder and back.
pub(crate) fn body_focus_explorer(scenario: &mut Scenario, state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_focus_explorer",
    };
    let ScenarioState::Title(title) = state else {
        panic!("setup opens the folder");
    };
    let title = title.clone();
    truth.drain(scenario, "opened");
    // The first focus in a new File Explorer window, each closed again.
    for index in 0..NEW_EXPLORER_WINDOWS {
        let opened = scenario
            .open_folder(&format!("m-new-folder-{index}"), &["one", "two"])
            .expect("opens another folder window");
        truth.write(&serde_json::json!({ "step": "explorer_window", "title": opened }));
        truth.drain(scenario, "after opening");
        scenario
            .close_windows_titled(&opened)
            .expect("closes the folder window");
        truth.drain(scenario, "after closing");
    }
    for _ in 0..REPEATS {
        for (key, front) in [
            ("downarrow", title.as_str()),
            ("downarrow", title.as_str()),
            ("uparrow", title.as_str()),
            ("uparrow", title.as_str()),
            ("enter", title.as_str()),
            ("backspace", "Inner - File Explorer"),
        ] {
            ready(scenario, &truth, front);
            scenario.send_keys(&[key]).expect("sends the key");
            truth.write(&serde_json::json!({ "step": "explorer", "key": key }));
            truth.drain(scenario, "after key");
        }
    }
}

pub(crate) fn setup_focus_text_box(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    text_box::open(scenario, "m-box-a", BOX_NAME, "alpha line")?;
    Ok(ScenarioState::None)
}

/// `measure_focus_text_box`: two Windows Forms text boxes, brought forward
/// by turns as their taskbar buttons do.
pub(crate) fn body_focus_text_box(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_focus_text_box",
    };
    truth.drain(scenario, "first opened");
    text_box::open(scenario, "m-box-b", BOX_NAME, "beta line").expect("opens the second box");
    truth.drain(scenario, "second opened");
    let first = harness_marker("m-box-a");
    let second = harness_marker("m-box-b");
    switch_between(scenario, &truth, &first, &second);
}

/// `measure_focus_new_application`: the first focus in a newly started
/// application, whose outpost starts with it: the Windows Forms text box
/// (MSAA) and `mockapp`'s text box (UIA) by turns, each closed before the
/// next starts.
pub(crate) fn body_focus_new_application(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_focus_new_application",
    };
    for index in 0..FRESH_APPLICATIONS {
        let name = format!("m-new-{index}");
        let kind = if index % 2 == 0 {
            text_box::open(scenario, &name, BOX_NAME, "fresh line").expect("opens a text box");
            "forms_text_box"
        } else {
            super::open_text_box_app(scenario, &name, "fresh line").expect("opens mockapp");
            "mockapp_uia"
        };
        let title = harness_marker(&name);
        truth.write(&serde_json::json!({ "step": "launched", "kind": kind, "title": title }));
        truth.drain(scenario, "after launch");
        scenario
            .close_windows_titled(&title)
            .expect("closes the application's window");
        truth.drain(scenario, "after close");
    }
}

/// The long line `measure_speech_real` reads with a review key, so that a
/// key pressed after it lands while it is still being spoken.
const LONG_LINE: &str = "This line is long on purpose: it keeps the synthesizer speaking for \
several seconds, so that the caret key pressed after the review key that reads it arrives \
while it is still being spoken, and the echo of that key must cut it off and start at once.";

pub(crate) fn setup_speech_real(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    assert!(
        crate::scenario::is_audible(),
        "measure_speech_real measures the real synthesizer host and audio device: run it with \
         {}=1",
        crate::scenario::AUDIBLE_ENV
    );
    text_box::open(scenario, "m-real-long", BOX_NAME, LONG_LINE)?;
    Ok(ScenarioState::None)
}

/// `measure_speech_real` (`docs/design/communication.md`, section 10.3):
/// the speech path through the real synthesizer host and audio device, so
/// the ledger's host and device stages, the device clock among them, have
/// figures. In two Windows Forms text boxes: focus changes into each by
/// turns, typing at 150 ms a character and Backspace at 33 ms, caret keys,
/// the review key that reads the current line, and a caret key pressed
/// 300 ms after a review key starts the long line, while it is still
/// spoken. Only in an audible run ([`crate::scenario::AUDIBLE_ENV`]).
pub(crate) fn body_speech_real(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_speech_real",
    };
    truth.drain(scenario, "first opened");
    text_box::open(scenario, "m-real-short", BOX_NAME, "").expect("opens the second box");
    truth.drain(scenario, "second opened");
    let long = harness_marker("m-real-long");
    let short = harness_marker("m-real-short");
    switch_between(scenario, &truth, &long, &short);
    scenario
        .bring_window_forward(&short)
        .expect("brings the short box forward");
    let target = (short.as_str(), "forms_text_box");
    for _ in 0..ROUNDS {
        type_paced(scenario, &truth, target, TYPED, 150);
        caret_keys(scenario, &truth, &short, "leftarrow", 10);
        caret_keys(scenario, &truth, &short, "rightarrow", 10);
        backspace_paced(scenario, &truth, target, TYPED);
    }
    scenario
        .bring_window_forward(&long)
        .expect("brings the long box forward");
    for _ in 0..REPEATS {
        ready(scenario, &truth, &long);
        scenario
            .send_keys(&["numpad8"])
            .expect("presses the review key");
        truth.write(&serde_json::json!({ "step": "review_line" }));
        truth.drain(scenario, "after review");
        ready(scenario, &truth, &long);
        let (first, last) = scenario
            .send_keys_paced(&["numpad8", "rightarrow"], 300)
            .expect("presses the review key, then a caret key during it");
        truth.write(&serde_json::json!({
            "step": "key_during_announcement", "first": first, "last": last, "interval_ms": 300,
        }));
        truth.drain(scenario, "after the key during the announcement");
        ready(scenario, &truth, &long);
        scenario.send_keys(&["home"]).expect("presses home");
        truth.drain(scenario, "after home");
    }
}

/// Presses `key` `count` times, one every 150 ms, in the window titled
/// `title`.
fn caret_keys(scenario: &mut Scenario, truth: &Truth, title: &str, key: &str, count: usize) {
    ready(scenario, truth, title);
    let keys = vec![key; count];
    let (first, last) = scenario
        .send_keys_paced(&keys, 150)
        .expect("presses the caret keys");
    truth.write(&serde_json::json!({
        "step": "caret_keys", "key": key, "first": first, "last": last, "interval_ms": 150,
    }));
    truth.drain(scenario, "after caret keys");
}

/// How long `measure_idle` holds Verbatim idle.
const IDLE: std::time::Duration = std::time::Duration::from_secs(20);

/// `measure_idle`: Verbatim at the desktop with nothing happening, for
/// [`IDLE`], so a trace taken meanwhile (`cargo xtask trace start`) shows
/// what its threads do when there is nothing to do: each wake-up there is a
/// cost with no purpose. The idle time is the workload, not a wait for
/// anything.
pub(crate) fn body_idle(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_idle",
    };
    truth.drain(scenario, "started");
    std::thread::sleep(IDLE);
    truth.drain(scenario, "idle");
}

pub(crate) fn setup_keys_text_box(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    text_box::open(scenario, "m-keys-box", BOX_NAME, "")?;
    Ok(ScenarioState::None)
}

/// `measure_keys_text_box`: typing at 150 ms and 60 ms a character and
/// Backspace at 33 ms in the Windows Forms text box, after 20 Pause keys
/// injected by Verbatim itself through its control plane, the workload for
/// what re-injecting a held key costs: the time from its `SendInput` to its
/// own hook. Pause is not announced, so the latency ledger logs no line for
/// those keys.
pub(crate) fn body_keys_text_box(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_keys_text_box",
    };
    let title = harness_marker("m-keys-box");
    let target = (title.as_str(), "forms_text_box");
    truth.drain(scenario, "opened");
    // Re-injection first, so its timings are kept however the typing goes.
    // Pause does nothing in a text box, and Verbatim binds nothing to it.
    for _ in 0..REPEATS {
        ready(scenario, &truth, &title);
        scenario
            .send_keys_from_verbatim(&["pause"])
            .expect("Verbatim injects the key");
        truth.write(&serde_json::json!({ "step": "reinject", "key": "pause", "vk": 0x13 }));
        truth.drain(scenario, "after reinject");
    }
    for _ in 0..ROUNDS {
        type_paced(scenario, &truth, target, TYPED, 150);
        backspace_paced(scenario, &truth, target, TYPED);
        type_paced(scenario, &truth, target, TYPED, 60);
        backspace_paced(scenario, &truth, target, TYPED);
    }
}

/// The key workload of a terminal titled `title`, `target` naming it, the
/// shell's folder `directory`.
fn terminal_keys(
    scenario: &mut Scenario,
    truth: &Truth,
    (title, target): (&str, &str),
    directory: &str,
) {
    let window = (title, target);
    for _ in 0..ROUNDS {
        type_paced(scenario, truth, window, TYPED_COMMAND, 150);
        escape(scenario, truth, window, TYPED_COMMAND);
        type_paced(scenario, truth, window, TYPED_COMMAND, 60);
        backspace_paced(scenario, truth, window, TYPED_COMMAND);
    }
    // Typing while the ticker prints a line every 50 ms.
    type_paced(scenario, truth, window, r".\ticker.ps1", 60);
    ready(scenario, truth, title);
    scenario.send_keys(&["enter"]).expect("presses enter");
    truth.write(&serde_json::json!({ "step": "ticker_started" }));
    type_paced(scenario, truth, window, TYPED_DURING_OUTPUT, 150);
    scenario
        .wait_for_agent_file(&format!(r"{directory}\ticked"), WINDOW_TIMEOUT)
        .expect("the ticker finishes");
    truth.drain(scenario, "ticker done");
    escape(scenario, truth, window, TYPED_DURING_OUTPUT);
    // Escape on a typed line that wrapped onto a second row.
    let wrapped = format!("echo {}xyz", "a".repeat(108));
    for _ in 0..WRAPPED_ROUNDS {
        type_paced(scenario, truth, window, &wrapped, 60);
        escape(scenario, truth, window, &wrapped);
    }
    in_front(scenario, title);
    truth.drain(scenario, "done");
    truth.write(&serde_json::json!({ "step": "done", "prompt": PROMPT }));
}

/// The folder of a terminal scenario's shell.
fn directory(state: &ScenarioState) -> String {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("a terminal scenario's setup opens a terminal window");
    };
    directory.clone()
}

/// The ticker's scripts.
fn ticker_scripts() -> [(&'static str, &'static str); 2] {
    [("ticker.ps1", TICKER), ("ticker-run.ps1", TICKER_RUN)]
}

pub(crate) fn setup_keys_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "m-keys", &ticker_scripts())
}

/// `measure_keys_conhost`: the key workload in the console host.
pub(crate) fn body_keys_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_keys_conhost",
    };
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    let directory = directory(state);
    terminal_keys(scenario, &truth, (&title, "conhost"), &directory);
}

pub(crate) fn setup_keys_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "m-keys", &ticker_scripts())
}

/// `measure_keys_windows_terminal`: the key workload in Windows Terminal.
pub(crate) fn body_keys_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_keys_windows_terminal",
    };
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
    let directory = directory(state);
    terminal_keys(scenario, &truth, (&title, "windows_terminal"), &directory);
}
