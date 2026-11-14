//! The measure set: the measurement workloads that
//! `docs/design/communication.md` section 11 runs at every migration step,
//! its first step's baseline and every later step's comparison. They
//! measure nothing themselves: they give Verbatim focus changes, typing and
//! Backspace to handle, and its latency ledger logs the figures, one line
//! per announcement at info on the `verbatim::latency` target, which a run
//! keeps in its artifacts. They are the registry's `measure` group, left
//! out of every run that does not name them, like the demonstrations, and
//! they are measurement, not tests (`books/dev/src/reference/testing.md`).
//!
//! They follow the harness's rules: every scenario starts from the
//! minimized desktop and asserts Verbatim's startup speech; the window a
//! key goes to is confirmed in front before every key; every wait is for
//! evidence (Verbatim idle after the last input, a window in front, a file
//! a script writes). Windows are switched as a taskbar button does, never
//! with Alt+Tab. They assert nothing beyond completing, except where a
//! workload proves it measures its path, as the focus returns do by their
//! speech and their count of live re-reads: the registry runs the
//! `measure` group with the speech collector in collect-only mode, which
//! never fails on unasserted speech. Just before every input,
//! everything Verbatim has queued is taken, not asserted, and written with
//! the ground truth of each step (what was typed, what a key removed, and
//! the harness numbers of its keys) into `measure-truth.jsonl` in the
//! scenario's artifacts directory.
//!
//! Typing and Backspace are paced by the agent (`SendKeysPaced`,
//! `TypeTextPaced`): the pace is the workload, a typist's or keyboard
//! auto-repeat's rate, not a wait. The ticker script prints a line every
//! 50 ms for the same reason.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};

use super::terminal::{self, PROMPT};
use super::text_box::{self, BOX_NAME};
use crate::artifacts;
use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario, WINDOW_TIMEOUT, harness_marker};
use crate::speech::STEP_TIMEOUT;

pub(crate) use super::no_teardown as teardown;

/// How many times each focus change is repeated, for percentiles.
const REPEATS: usize = 20;

/// How many fresh applications are started, each with a fresh outpost.
const FRESH_APPLICATIONS: usize = 12;

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

/// The lines the caret keys move over in the text box and Notepad, one
/// more than the burst and each round of line keys can reach.
const ARROW_LINES: usize = 30;

/// How many Down Arrows a burst presses, and how far apart: faster than
/// the reads each key's answer waits for, so keys come before the answers
/// to the keys before them.
const BURST_KEYS: usize = 10;
const BURST_INTERVAL_MS: u32 = 30;

/// The harness document `measure_keys_notepad` opens in Notepad.
const NOTEPAD_NAME: &str = "m-keys-notepad";

/// The text the caret keys move over: [`ARROW_LINES`] numbered lines.
fn arrow_text() -> String {
    (1..=ARROW_LINES).fold(String::new(), |mut text, line| {
        let _ = write!(text, "line {line:02}\r\n");
        text
    })
}

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
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    let second_state = terminal::open_windows_terminal_window(scenario, "m-wt-b", state)
        .expect("opens a second Windows Terminal window");
    let second = terminal::title(&second_state).to_owned();
    terminal::expect_second_windows_terminal_prompt_read(scenario, &second_state);
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
            .close_launched(&title)
            .expect("closes the application's window");
        truth.drain(scenario, "after close");
    }
}

/// The Windows Forms text box's one line in the return workloads.
const RETURN_LINE: &str = "alpha line";

/// The return workloads' mockapp UIA window: a list holding two items, the
/// first focused, below the window, so that a return's read meets the
/// listener's ancestry cache at the list and takes the window from it, as
/// mockapp's `a_uia_window_with_an_msaa_menu_re_reads_live_once_per_return`
/// does; its focus event is raised each time the window is activated, as
/// an application restores its focus. `{title}` is the window's title.
const RETURN_FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "focus_on_activate": true,
  "children": [
    {
      "id": "folder",
      "role": "list",
      "name": "Items View",
      "states": ["focusable"],
      "children": [
        {
          "id": "inner",
          "role": "list_item",
          "name": "Inner",
          "states": ["focusable", "selectable", "focused"],
          "position_in_set": 1,
          "set_size": 2
        },
        {
          "id": "alpha",
          "role": "list_item",
          "name": "alpha.txt",
          "states": ["focusable", "selectable"],
          "position_in_set": 2,
          "set_size": 2
        }
      ]
    }
  ]
}
"#;

/// The prefix of the named events the mockapp window `name` sets as it
/// prints its lines (`--line-events`), unique to this run of the test
/// binary.
fn line_events(name: &str) -> String {
    format!(r"Local\verbatim-e2e-{name}-{}-", std::process::id())
}

/// Opens a mockapp UIA window titled with [`harness_marker`] of `name`,
/// from `fixture` with `{title}` replaced by that title, its standard input
/// a pipe and its lines setting the events of [`line_events`], and waits
/// for it to take the foreground. Returns its process id.
pub(super) fn open_mockapp(scenario: &mut Scenario, name: &str, fixture: &str) -> u32 {
    open_mockapp_with(scenario, name, fixture, "uia", &[])
}

/// [`open_mockapp`], for mockapp's MSAA backend.
pub(super) fn open_msaa_mockapp(scenario: &mut Scenario, name: &str, fixture: &str) -> u32 {
    open_mockapp_with(scenario, name, fixture, "msaa", &[])
}

/// [`open_mockapp`], its root counting UIA's event registrations, for
/// mockapp's `await-advised`.
pub(super) fn open_mockapp_counting(scenario: &mut Scenario, name: &str, fixture: &str) -> u32 {
    open_mockapp_with(scenario, name, fixture, "uia", &["--count-registrations"])
}

/// [`open_mockapp`] on `backend`, with `extra` on mockapp's command line.
fn open_mockapp_with(
    scenario: &mut Scenario,
    name: &str,
    fixture: &str,
    backend: &str,
    extra: &[&str],
) -> u32 {
    let directory = scenario.run_directory().to_owned();
    let title = harness_marker(name);
    let path = scenario.harness_file(name, "json");
    let output = scenario.harness_file(name, "log");
    scenario
        .write_agent_file(&path, fixture.replace("{title}", &title).as_bytes())
        .expect("writes mockapp's fixture");
    let events = line_events(name);
    let args: Vec<String> = [
        "--fixture",
        &path,
        "--backend",
        backend,
        "--title",
        &title,
        "--show",
        "--line-events",
        &events,
    ]
    .iter()
    .chain(extra)
    .map(|arg| (*arg).to_owned())
    .collect();
    scenario
        .launch_titled_with_stdin(&format!(r"{directory}\mockapp.exe"), &args, &title, &output)
        .expect("opens the mockapp window")
        .pid
}

/// Writes `command` to the mockapp window `name`, process `pid`, and waits
/// for mockapp's acknowledgement that it took effect.
pub(super) fn apply_to_mockapp(scenario: &mut Scenario, window: (&str, u32), command: &str) {
    send_to_mockapp(scenario, window, command);
    await_mockapp(scenario, window);
}

/// Writes `command` to the mockapp window `name`, process `pid`, leaving
/// its acknowledgement to [`await_mockapp`], so a scenario acts while the
/// command runs.
pub(super) fn send_to_mockapp(scenario: &mut Scenario, (name, pid): (&str, u32), command: &str) {
    let applied = format!("{}applied", line_events(name));
    scenario
        .create_event(&applied)
        .expect("creates the event for mockapp's acknowledgement");
    scenario
        .write_stdin(pid, command)
        .expect("writes the command to mockapp");
}

/// Waits for mockapp's acknowledgement of the command [`send_to_mockapp`]
/// wrote.
pub(super) fn await_mockapp(scenario: &mut Scenario, (name, pid): (&str, u32)) {
    scenario
        .wait_for_event(
            &format!("{}applied", line_events(name)),
            pid,
            WINDOW_TIMEOUT,
        )
        .expect("mockapp applies the command");
}

/// Asserts the entry into [`RETURN_FIXTURE`]'s window titled `title`: the
/// window, the list and the item, as mockapp's
/// `a_uia_window_with_an_msaa_menu_re_reads_live_once_per_return` speaks
/// them, in the words `explorer_folder_window` hears for a list and its
/// item.
fn expect_list_entered(scenario: &mut Scenario, title: &str) {
    scenario.speech().expect(&[
        &format!("{title} window"),
        "Items View list",
        "Inner not selected 1 of 2",
    ]);
}

/// The return workloads: from the Windows Forms text box `box_name`, open
/// since the setup, to [`RETURN_FIXTURE`]'s mockapp window `app_name`, read
/// by the listener, and back, [`REPEATS`] times, each window brought
/// forward as a taskbar button does. With `retitle`, the mockapp window is
/// given a new title each time while the text box is in front. Each
/// window's speech is asserted, the mockapp window's by its title at that
/// return.
fn focus_returns(
    scenario: &mut Scenario,
    truth: &Truth,
    (box_name, app_name): (&str, &str),
    retitle: bool,
) {
    text_box::expect_announced(scenario, box_name, BOX_NAME, RETURN_LINE);
    let pid = open_mockapp(scenario, app_name, RETURN_FIXTURE);
    let app = harness_marker(app_name);
    expect_list_entered(scenario, &app);
    truth.write(&serde_json::json!({ "step": "launched", "kind": "mockapp_uia", "title": app }));
    for round in 0..REPEATS {
        scenario
            .bring_window_forward(&harness_marker(box_name))
            .expect("brings the text box forward");
        truth.write(&serde_json::json!({ "step": "bring_forward", "to": "forms_text_box" }));
        text_box::expect_announced(scenario, box_name, BOX_NAME, RETURN_LINE);
        let title = if retitle {
            let title = format!("{app} round {round}");
            apply_to_mockapp(scenario, (app_name, pid), &format!("set-title {title}"));
            truth.write(&serde_json::json!({ "step": "set_title", "title": title }));
            title
        } else {
            app.clone()
        };
        // The new title still holds the marker the window is found by.
        scenario
            .bring_window_forward(&app)
            .expect("brings the mockapp window forward");
        truth.write(&serde_json::json!({
            "step": "bring_forward", "to": "mockapp_uia", "title": title,
        }));
        expect_list_entered(scenario, &title);
    }
}

/// The text box of `measure_focus_return`.
const RETURN_BOX: &str = "m-return-box";

pub(crate) fn setup_focus_return(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    text_box::open(scenario, RETURN_BOX, BOX_NAME, RETURN_LINE)?;
    Ok(ScenarioState::None)
}

/// `measure_focus_return`: a return between top-level windows that a live
/// re-read answers (`books/dev/src/reference/crates/verbatim-core.md`, "The
/// ancestry cache"). The user goes from mockapp's UIA window to the
/// Windows Forms text box and back ([`focus_returns`]); each return's
/// focus event, read by the listener's reading thread for mockapp, which
/// still holds its last focus, meets its ancestry cache at the list and
/// takes the window from it, and Core, whose focus ancestry is the text
/// box's, would enter the window from the cache, so it asks for a live
/// re-read and speaks its answer: at least one live re-read per return,
/// and one more for each further focus event the window raises for its
/// item while a re-read waits, which Core judges as a new report.
pub(crate) fn body_focus_return(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_focus_return",
    };
    focus_returns(scenario, &truth, (RETURN_BOX, "m-return-app"), false);
    let re_reads = scenario.live_re_reads();
    assert!(
        re_reads >= REPEATS as u64,
        "every return makes a live re-read: {re_reads} for {REPEATS} returns"
    );
}

/// The text box of `measure_focus_return_retitled`.
const RETITLED_BOX: &str = "m-retitled-box";

pub(crate) fn setup_focus_return_retitled(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    text_box::open(scenario, RETITLED_BOX, BOX_NAME, RETURN_LINE)?;
    Ok(ScenarioState::None)
}

/// `measure_focus_return_retitled`: the returns of `measure_focus_return`,
/// with mockapp's window given a new title (`set-title`) each time while
/// the text box is in front, so the listener's title check finds the
/// window text saved with its ancestry cache changed and walks live to the
/// top: the new title is spoken, with no live re-read.
pub(crate) fn body_focus_return_retitled(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_focus_return_retitled",
    };
    focus_returns(scenario, &truth, (RETITLED_BOX, "m-retitled-app"), true);
    scenario.expect_live_re_reads(0);
}

/// How many buttons the flood's mockapp window holds.
const FLOOD_CHOICES: usize = 10;

/// The flood's mockapp window: [`FLOOD_CHOICES`] buttons, `c1` to `c10`,
/// named "Choice 1" to "Choice 10", the first focused. `{title}` is the
/// window's title.
fn flood_fixture() -> String {
    let children: Vec<serde_json::Value> = (1..=FLOOD_CHOICES)
        .map(|choice| {
            let states: &[&str] = if choice == 1 {
                &["focusable", "focused"]
            } else {
                &["focusable"]
            };
            serde_json::json!({
                "id": format!("c{choice}"),
                "role": "button",
                "name": format!("Choice {choice}"),
                "states": states,
            })
        })
        .collect();
    serde_json::json!({
        "id": "root",
        "role": "window",
        "name": "{title}",
        "children": children,
    })
    .to_string()
}

/// `measure_focus_flood`: a burst of focus events in one application.
/// [`REPEATS`] times, mockapp's UIA window is sent one `focus` command for
/// each of its buttons after the focused one, in one write, so they come
/// faster than the listener reads each focus: up from the first to the
/// last in even rounds, down from the last to the first in odd ones, so
/// each burst ends on a button other than the one it started on. The
/// evidence a burst is over is Core receiving the focus on its last
/// button, as in `rapid_tabbing_in_settings`, and then Verbatim being idle.
/// Which buttons a burst says depends on how mockapp's focus changes and
/// the listener's reads interleave, so its speech is collected, not
/// asserted. Every move is within one top-level window, so none asks for a
/// live re-read.
pub(crate) fn body_focus_flood(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_focus_flood",
    };
    let name = "m-flood-app";
    let pid = open_mockapp(scenario, name, &flood_fixture());
    let title = harness_marker(name);
    scenario
        .speech()
        .expect(&[&format!("{title} window"), "Choice 1 button"]);
    let mut events = scenario
        .subscribe_events()
        .expect("subscribes to Verbatim's events");
    for round in 0..REPEATS {
        let choices: Vec<usize> = if round % 2 == 0 {
            (2..=FLOOD_CHOICES).collect()
        } else {
            (1..FLOOD_CHOICES).rev().collect()
        };
        let burst: Vec<String> = choices
            .iter()
            .map(|choice| format!("focus c{choice}"))
            .collect();
        ready(scenario, &truth, &title);
        scenario
            .write_stdin(pid, &burst.join("\n"))
            .expect("writes the burst of focus commands");
        let last = format!("Choice {}", choices[choices.len() - 1]);
        truth.write(&serde_json::json!({
            "step": "focus_burst", "round": round, "commands": burst, "last": last,
        }));
        super::rapid_tabbing_in_settings::wait_for_focus_on(&mut events, &last);
        truth.drain(scenario, "after the burst");
    }
    scenario.expect_live_re_reads(0);
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

/// The caret key workload in the window titled `title`, `target` naming
/// it, over [`arrow_text`] with the caret at its top: Right and Left
/// Arrow, then Down and Up Arrow, ten each at 150 ms, then a burst of
/// [`BURST_KEYS`] Down Arrows [`BURST_INTERVAL_MS`] apart from the top,
/// whose answers are kept in the flight recorder dumped after it
/// (`burst-<round>` in the artifacts), each burst's truth naming the line
/// each key leaves the caret on, [`ROUNDS`] times.
fn arrow_keys(scenario: &mut Scenario, truth: &Truth, (title, target): (&str, &str)) {
    for round in 0..ROUNDS {
        top(scenario, truth, title);
        caret_keys(scenario, truth, title, "rightarrow", 10);
        caret_keys(scenario, truth, title, "leftarrow", 10);
        caret_keys(scenario, truth, title, "downarrow", 10);
        caret_keys(scenario, truth, title, "uparrow", 10);
        top(scenario, truth, title);
        ready(scenario, truth, title);
        let keys = vec!["downarrow"; BURST_KEYS];
        let (first, last) = scenario
            .send_keys_paced(&keys, BURST_INTERVAL_MS)
            .expect("presses the burst of down arrows");
        let lines: Vec<String> = (2..=BURST_KEYS + 1)
            .map(|line| format!("line {line:02}"))
            .collect();
        truth.write(&serde_json::json!({
            "step": "burst", "target": target, "round": round, "key": "downarrow",
            "first": first, "last": last, "interval_ms": BURST_INTERVAL_MS, "lines": lines,
        }));
        truth.drain(scenario, "after the burst");
        let dir = artifacts::scenario_dir(&artifacts::artifacts_root(), truth.name)
            .join(format!("burst-{round}"));
        scenario
            .collect_flight_recorder(&dir)
            .expect("dumps the flight recorder after the burst");
    }
}

/// How many times [`select_then_right`] selects and presses Right.
const SELECT_THEN_RIGHT: usize = 10;

/// The first case of holding keys (`books/dev/src/explanation/design/key-holding.md`, "How
/// often a key's effect was misjudged"): select then copy's first press selects the first word of
/// the top line, from a start marker at the line's start through the
/// review cursor on the word's last character, and Right Arrow follows at
/// once, before the application can have reported the selection's caret,
/// [`SELECT_THEN_RIGHT`] times. The flight recorder, dumped after
/// (`select-then-right` in the artifacts), holds each key's answer.
fn select_then_right(scenario: &mut Scenario, truth: &Truth, title: &str) {
    for _ in 0..SELECT_THEN_RIGHT {
        top(scenario, truth, title);
        for gesture in ["kb:verbatim+f9", "kb:numpad3", "kb:numpad3", "kb:numpad3"] {
            ready(scenario, truth, title);
            scenario.send_gesture(gesture).expect("sends the gesture");
            truth.drain(scenario, "after the review gesture");
        }
        ready(scenario, truth, title);
        scenario
            .send_gesture("kb:verbatim+f10")
            .expect("sends the copy's first press");
        scenario
            .send_keys(&["rightarrow"])
            .expect("presses right arrow");
        truth.write(&serde_json::json!({
            "step": "select_then_right", "selected": "line", "at": " ",
        }));
        truth.drain(scenario, "after select then right");
    }
    // One more key after the last Right, so that Right's answer, which can
    // come after Verbatim is idle, is in the recorder before it is dumped.
    top(scenario, truth, title);
    let dir =
        artifacts::scenario_dir(&artifacts::artifacts_root(), truth.name).join("select-then-right");
    scenario
        .collect_flight_recorder(&dir)
        .expect("dumps the flight recorder after the selections");
}

/// How many times each of the stale caret cases runs in one run of its
/// workload.
const STALE_ROUNDS: usize = 10;

/// The harness document `measure_stale_caret_notepad` opens in Notepad.
const STALE_NOTEPAD_NAME: &str = "m-stale-notepad";

/// Dumps the flight recorder into `<name>-<round>` in the artifacts.
fn dump_recorder(scenario: &mut Scenario, truth: &Truth, name: &str, round: usize) {
    let dir = artifacts::scenario_dir(&artifacts::artifacts_root(), truth.name)
        .join(format!("{name}-{round}"));
    scenario
        .collect_flight_recorder(&dir)
        .expect("dumps the flight recorder");
}

/// A stale caret case of `books/dev/src/explanation/design/key-holding.md`: Shift+Right Arrow
/// followed at once by Right Arrow, in one batch, from the top of
/// [`arrow_text`], so that Right is pressed before the application can
/// have reported the selection Shift+Right made, [`STALE_ROUNDS`] times.
/// Right should leave the caret on "i" with nothing selected. The flight
/// recorder, dumped after each round (`shift-right-then-right-<round>`),
/// holds the keys' answers.
fn shift_right_then_right(scenario: &mut Scenario, truth: &Truth, title: &str) {
    for round in 0..STALE_ROUNDS {
        top(scenario, truth, title);
        ready(scenario, truth, title);
        scenario
            .send_keys(&["shift+rightarrow", "rightarrow"])
            .expect("presses shift+rightarrow and rightarrow");
        truth.write(&serde_json::json!({
            "step": "shift_right_then_right", "round": round, "at": "i",
        }));
        truth.drain(scenario, "after shift+right then right");
        // One more key, so that Right's answer, which can come after
        // Verbatim is idle, is in the recorder before it is dumped.
        top(scenario, truth, title);
        dump_recorder(scenario, truth, "shift-right-then-right", round);
    }
}

pub(crate) fn setup_stale_caret_text_box(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    text_box::open(scenario, "m-stale-box", BOX_NAME, &arrow_text())?;
    Ok(ScenarioState::None)
}

/// `measure_stale_caret_text_box`: Shift+Right then Right at once
/// ([`shift_right_then_right`]) in the Windows Forms text box.
pub(crate) fn body_stale_caret_text_box(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_stale_caret_text_box",
    };
    let title = harness_marker("m-stale-box");
    truth.drain(scenario, "opened");
    shift_right_then_right(scenario, &truth, &title);
}

/// The document `measure_stale_caret_notepad` opens in Notepad before
/// Verbatim starts: [`arrow_text`].
pub(crate) fn stale_notepad_document() -> Document {
    Document {
        name: STALE_NOTEPAD_NAME,
        contents: arrow_text(),
    }
}

pub(crate) fn setup_stale_caret_notepad(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.bring_document_forward(STALE_NOTEPAD_NAME)?;
    Ok(ScenarioState::None)
}

/// `measure_stale_caret_notepad`: Shift+Right then Right at once
/// ([`shift_right_then_right`]) in Windows 11 Notepad.
pub(crate) fn body_stale_caret_notepad(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_stale_caret_notepad",
    };
    let title = harness_marker(STALE_NOTEPAD_NAME);
    truth.drain(scenario, "opened");
    shift_right_then_right(scenario, &truth, &title);
    ready(scenario, &truth, &title);
    scenario
        .save_document(STALE_NOTEPAD_NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
    truth.drain(scenario, "saved");
}

pub(crate) fn setup_stale_caret_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "m-stale", &[])
}

/// How long say-all's first utterance in the console host may take to be
/// heard: the command's first row and the read-ahead's twenty rows after
/// it, which repeat the second row (`books/dev/src/explanation/design/key-holding.md`, "How
/// often a key's effect was misjudged").
const SAY_ALL_FIRST_UTTERANCE: std::time::Duration = std::time::Duration::from_secs(180);

/// A command too long for the console host's 120 columns: after the
/// prompt, `ready> `, its first 113 characters fill the first row, ending
/// with a space, and the rest wraps onto the next.
const LONG_COMMAND: &str = "echo alpha bravo charlie delta foxtrot golfer hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango uniform victor whiskey";

/// `measure_stale_caret_conhost`: the other stale caret case of
/// `books/dev/src/explanation/design/key-holding.md`, a say-all caret move the application
/// refuses. A command that wraps onto a second row is typed
/// ([`LONG_COMMAND`]); then, [`STALE_ROUNDS`] times, Home
/// puts the cursor at its start, say-all from the caret reads on from
/// there, moving the caret to the second row as playback reaches it,
/// which the console host refuses, and once say-all's first utterance
/// has been heard in full, so every move it carries was made, Right
/// Arrow is pressed, which should leave the cursor on the command's "c".
/// The flight recorder is dumped after each round
/// (`say-all-then-right-<round>`).
pub(crate) fn body_stale_caret_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_stale_caret_conhost",
    };
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    ready(scenario, &truth, &title);
    scenario.type_text(LONG_COMMAND).expect("types the command");
    truth.drain(scenario, "after typing");
    for round in 0..STALE_ROUNDS {
        ready(scenario, &truth, &title);
        scenario.send_keys(&["home"]).expect("presses home");
        truth.drain(scenario, "after home");
        ready(scenario, &truth, &title);
        scenario
            .send_gesture("kb:verbatim+downarrow")
            .expect("sends say all");
        let first = scenario.speech().take_next(STEP_TIMEOUT);
        let ending = scenario.speech().ending_of(&first, SAY_ALL_FIRST_UTTERANCE);
        truth.write(&serde_json::json!({
            "step": "say all", "first": first.text, "ending": format!("{ending:?}"),
        }));
        in_front(scenario, &title);
        scenario
            .send_keys(&["rightarrow"])
            .expect("presses right arrow");
        truth.write(&serde_json::json!({
            "step": "say_all_then_right", "round": round, "at": "c",
        }));
        truth.drain(scenario, "after right arrow");
        ready(scenario, &truth, &title);
        scenario.send_keys(&["home"]).expect("presses home");
        truth.drain(scenario, "after home");
        dump_recorder(scenario, &truth, "say-all-then-right", round);
    }
}

/// How many times `measure_held_repeat_text_box` holds Right Arrow.
const HELD_REPEAT_ROUNDS: usize = 3;

/// Windows' default keyboard auto-repeat: the first repeat 500 ms after the
/// press, then about 30 a second.
const REPEAT_DELAY_MS: u32 = 500;
const REPEAT_INTERVAL_MS: u32 = 33;

/// The repeats in a hold of about one second: the release comes
/// 500 + 15 × 33 = 995 ms after the press.
const AUTO_REPEATS: u32 = 15;

/// How long Verbatim may take to become idle after the hold's release: a
/// bound on a hang, above the 10 seconds a held key waited for each key
/// ahead of it that moved nothing before that wait was bounded.
const HELD_REPEAT_IDLE_BOUND: std::time::Duration = std::time::Duration::from_secs(300);

pub(crate) fn setup_held_repeat_text_box(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    text_box::open(scenario, "m-repeat-box", BOX_NAME, &arrow_text())?;
    Ok(ScenarioState::None)
}

/// `measure_held_repeat_text_box`: the case where a held key's wait for
/// the key ahead of it repeats (`books/dev/src/explanation/design/key-holding.md`, "Keys
/// behind a held key"). [`HELD_REPEAT_ROUNDS`] times, Control+End puts the
/// caret at the end of the Windows Forms text box's text, where Right
/// Arrow moves nothing, and Right Arrow is held for about a second with
/// auto-repeat. The ground truth records when the release was sent, in
/// microseconds since the Unix epoch, to compare with the times of
/// Verbatim's log, and what was said until Verbatim was idle.
pub(crate) fn body_held_repeat_text_box(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_held_repeat_text_box",
    };
    let title = harness_marker("m-repeat-box");
    truth.drain(scenario, "opened");
    for round in 0..HELD_REPEAT_ROUNDS {
        ready(scenario, &truth, &title);
        scenario
            .send_keys(&["control+end"])
            .expect("presses control+end");
        truth.drain(scenario, "after control+end");
        ready(scenario, &truth, &title);
        let (first, last) = scenario
            .repeat_key(
                "rightarrow",
                REPEAT_DELAY_MS,
                REPEAT_INTERVAL_MS,
                AUTO_REPEATS,
            )
            .expect("holds right arrow");
        let released_unix_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_micros();
        let heard: Vec<String> = scenario
            .take_all_queued_within(HELD_REPEAT_IDLE_BOUND)
            .into_iter()
            .map(|heard| heard.text)
            .collect();
        truth.write(&serde_json::json!({
            "step": "held_right_at_the_end", "round": round, "first": first, "last": last,
            "released_unix_us": released_unix_us.to_string(), "heard": heard,
        }));
    }
}

/// Moves the caret to the top of the text with Control+Home.
fn top(scenario: &mut Scenario, truth: &Truth, title: &str) {
    ready(scenario, truth, title);
    scenario
        .send_keys(&["control+home"])
        .expect("presses control+home");
    truth.write(&serde_json::json!({ "step": "top" }));
    truth.drain(scenario, "after control+home");
}

pub(crate) fn setup_keys_text_box(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    text_box::open(scenario, "m-keys-box", BOX_NAME, &arrow_text())?;
    Ok(ScenarioState::None)
}

/// `measure_keys_text_box`: typing at 150 ms and 60 ms a character and
/// Backspace at 33 ms in the Windows Forms text box, a classic edit
/// control, after 20 Pause keys injected by Verbatim itself through its
/// control plane, the workload for what re-injecting a held key costs: the
/// time from its `SendInput` to its own hook. Pause is not announced, so
/// the latency ledger logs no line for those keys. Then the caret key
/// workload ([`arrow_keys`]) over the box's numbered lines, and select
/// then Right ([`select_then_right`]).
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
    arrow_keys(scenario, &truth, target);
    select_then_right(scenario, &truth, &title);
}

/// The document `measure_keys_notepad` opens in Notepad before Verbatim
/// starts: [`arrow_text`].
pub(crate) fn notepad_document() -> Document {
    Document {
        name: NOTEPAD_NAME,
        contents: arrow_text(),
    }
}

pub(crate) fn setup_keys_notepad(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.bring_document_forward(NOTEPAD_NAME)?;
    Ok(ScenarioState::None)
}

/// `measure_keys_notepad`: the text box's key workload in Windows 11
/// Notepad, whose text Verbatim reads through UIA: typing at 150 ms and
/// 60 ms a character and Backspace at 33 ms, then the caret key workload
/// ([`arrow_keys`]) and select then Right ([`select_then_right`]). The document is saved at the end, as typed and
/// removed, so Notepad closes without asking.
pub(crate) fn body_keys_notepad(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_keys_notepad",
    };
    let title = harness_marker(NOTEPAD_NAME);
    let target = (title.as_str(), "notepad");
    truth.drain(scenario, "opened");
    for _ in 0..ROUNDS {
        type_paced(scenario, &truth, target, TYPED, 150);
        backspace_paced(scenario, &truth, target, TYPED);
        type_paced(scenario, &truth, target, TYPED, 60);
        backspace_paced(scenario, &truth, target, TYPED);
    }
    arrow_keys(scenario, &truth, target);
    select_then_right(scenario, &truth, &title);
    ready(scenario, &truth, &title);
    scenario
        .save_document(NOTEPAD_NAME, WINDOW_TIMEOUT)
        .expect("saves the document");
    truth.drain(scenario, "saved");
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
        // Caret keys on the typed line, back over its last ten characters
        // and forward again.
        caret_keys(scenario, truth, title, "leftarrow", 10);
        caret_keys(scenario, truth, title, "rightarrow", 10);
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

/// Measurement prototype (branch `prototype/background-events`): the
/// background workloads measure what ADR 0003's limit "Every background
/// event reaches Core" costs while the Windows Forms text box in front
/// takes paced caret keys, so the latency ledger's "hook to Core" stage
/// measures a held key's way into Core while the background runs. How many
/// rounds of caret keys each presses: each round is ten Down Arrows and ten
/// Up Arrows 150 ms apart, a burst of [`BURST_KEYS`] Down Arrows
/// [`BURST_INTERVAL_MS`] apart and ten Up Arrows 150 ms apart, about 4.8
/// seconds, so six rounds are about 30 seconds of keys.
const BACKGROUND_ROUNDS: usize = 6;

/// How long a background load runs, in seconds: longer than the keys, so a
/// trace window of 8 seconds fits inside both, away from their start and
/// end.
const BACKGROUND_LOAD_SECONDS: u32 = 40;

/// The longest the end of a background load may take after the keys: a
/// bound on a hang.
const BACKGROUND_LOAD_END: std::time::Duration = std::time::Duration::from_secs(180);

/// The text box of the background workloads, over [`arrow_text`].
const BACKGROUND_BOX: &str = "m-bg-box";

/// Opens the background workloads' text box, which takes the foreground,
/// and returns its title.
fn open_background_box(scenario: &mut Scenario) -> String {
    text_box::open(scenario, BACKGROUND_BOX, BOX_NAME, &arrow_text()).expect("opens the text box");
    harness_marker(BACKGROUND_BOX)
}

/// Presses `key` `count` times `interval_ms` apart in the window titled
/// `title`, confirmed in front first, without waiting for Verbatim to be
/// idle before or after, since a background load can keep it busy
/// throughout; the keys are recorded in the ground truth.
fn background_keys(
    scenario: &mut Scenario,
    truth: &Truth,
    title: &str,
    (key, count, interval_ms): (&str, usize, u32),
) {
    in_front(scenario, title);
    let keys = vec![key; count];
    let (first, last) = scenario
        .send_keys_paced(&keys, interval_ms)
        .expect("presses the caret keys");
    truth.write(&serde_json::json!({
        "step": "caret_keys", "key": key, "first": first, "last": last,
        "interval_ms": interval_ms,
    }));
}

/// The caret keys of a background workload in the text box titled
/// `title`, from its top: [`BACKGROUND_ROUNDS`] rounds of Down and Up
/// Arrow at 150 ms and a burst of Down Arrows 30 ms apart.
fn background_caret_keys(scenario: &mut Scenario, truth: &Truth, title: &str) {
    in_front(scenario, title);
    scenario
        .send_keys(&["control+home"])
        .expect("presses control+home");
    truth.write(&serde_json::json!({ "step": "top" }));
    for round in 0..BACKGROUND_ROUNDS {
        truth.write(&serde_json::json!({ "step": "round", "round": round }));
        background_keys(scenario, truth, title, ("downarrow", 10, 150));
        background_keys(scenario, truth, title, ("uparrow", 10, 150));
        background_keys(
            scenario,
            truth,
            title,
            ("downarrow", BURST_KEYS, BURST_INTERVAL_MS),
        );
        background_keys(scenario, truth, title, ("uparrow", BURST_KEYS, 150));
    }
}

/// `measure_background_idle_explorer`: a File Explorer folder window open
/// behind the text box, no other load, during the caret keys.
pub(crate) fn body_background_idle_explorer(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_background_idle_explorer",
    };
    let folder = scenario
        .open_folder("m-bg-folder", &["alpha", "beta", "gamma"])
        .expect("opens the folder window");
    truth.write(&serde_json::json!({ "step": "folder_opened", "title": folder }));
    let title = open_background_box(scenario);
    truth.write(&serde_json::json!({ "step": "box_opened", "title": title }));
    background_caret_keys(scenario, &truth, &title);
    truth.drain(scenario, "after the keys");
}

/// The console host's background flood: prints numbered lines as fast as
/// the shell can for [`BACKGROUND_LOAD_SECONDS`] seconds, then writes how
/// many to `bgflood-done`, the evidence it has finished, under another
/// name first so it never exists empty.
fn background_flood_script() -> String {
    format!(
        "$watch = [Diagnostics.Stopwatch]::StartNew()\r\n\
$line = 0\r\n\
while ($watch.ElapsedMilliseconds -lt {}) {{ $line++; \"background flood line $line\" }}\r\n\
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'bgflood-done.tmp'), [string]$line)\r\n\
[IO.File]::Move((Join-Path $PSScriptRoot 'bgflood-done.tmp'), (Join-Path $PSScriptRoot 'bgflood-done'))\r\n",
        BACKGROUND_LOAD_SECONDS * 1000
    )
}

pub(crate) fn setup_background_console_flood(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let script = background_flood_script();
    terminal::open_console_host(scenario, "m-bg-flood", &[("bgflood.ps1", &script)])
}

/// `measure_background_console_flood`: a console host window behind the
/// text box printing continuously during the caret keys. The flood is
/// started in the console, then the text box is opened in front of it.
pub(crate) fn body_background_console_flood(scenario: &mut Scenario, state: &mut ScenarioState) {
    let truth = Truth {
        name: "measure_background_console_flood",
    };
    let console = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{console} window"), "terminal", "blank"],
    );
    let directory = directory(state);
    ready(scenario, &truth, &console);
    scenario
        .type_text(r".\bgflood.ps1")
        .expect("types the flood's command");
    truth.drain(scenario, "after typing");
    in_front(scenario, &console);
    scenario.send_keys(&["enter"]).expect("presses enter");
    truth.write(&serde_json::json!({ "step": "flood_started" }));
    let title = open_background_box(scenario);
    truth.write(&serde_json::json!({ "step": "box_opened", "title": title }));
    background_caret_keys(scenario, &truth, &title);
    let lines = scenario
        .wait_for_agent_file(&format!(r"{directory}\bgflood-done"), BACKGROUND_LOAD_END)
        .expect("the flood finishes");
    truth.write(&serde_json::json!({
        "step": "flood_done", "lines": String::from_utf8_lossy(&lines),
    }));
    truth.drain(scenario, "after the flood");
}

/// The MSAA flood's mockapp window: one focused button.
const BACKGROUND_FLOOD_FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    {
      "id": "go",
      "role": "button",
      "name": "Go",
      "states": ["focusable", "focused"]
    }
  ]
}
"#;

/// The MSAA flood workloads: mockapp's MSAA window, opened first, then the
/// text box in front of it; mockapp's `flood` of value changes on a new
/// object each time, the console's kind of flood, `rate` events a second
/// for [`BACKGROUND_LOAD_SECONDS`] seconds, started before the caret keys,
/// whose end is waited for after them.
fn background_msaa_flood(scenario: &mut Scenario, name: &'static str, rate: u32) {
    let truth = Truth { name };
    let window = "m-bg-msaa";
    let pid = open_msaa_mockapp(scenario, window, BACKGROUND_FLOOD_FIXTURE);
    truth.write(&serde_json::json!({ "step": "mockapp_opened", "pid": pid }));
    let title = open_background_box(scenario);
    truth.write(&serde_json::json!({ "step": "box_opened", "title": title }));
    let count = rate * BACKGROUND_LOAD_SECONDS;
    let interval_ms = 1000.0 / f64::from(rate);
    let command = format!("flood {count} {interval_ms} new-values");
    send_to_mockapp(scenario, (window, pid), &command);
    truth.write(&serde_json::json!({ "step": "flood_started", "command": command }));
    background_caret_keys(scenario, &truth, &title);
    scenario
        .wait_for_event(
            &format!("{}applied", line_events(window)),
            pid,
            BACKGROUND_LOAD_END,
        )
        .expect("mockapp's flood ends");
    truth.write(&serde_json::json!({ "step": "flood_done" }));
    truth.drain(scenario, "after the flood");
}

/// `measure_background_msaa_flood_4000`: about 4,000 MSAA events a second
/// behind the text box ([`background_msaa_flood`]).
pub(crate) fn body_background_msaa_flood_4000(scenario: &mut Scenario, _state: &mut ScenarioState) {
    background_msaa_flood(scenario, "measure_background_msaa_flood_4000", 4_000);
}

/// `measure_background_msaa_flood_20000`: about 20,000 MSAA events a
/// second behind the text box ([`background_msaa_flood`]).
pub(crate) fn body_background_msaa_flood_20000(
    scenario: &mut Scenario,
    _state: &mut ScenarioState,
) {
    background_msaa_flood(scenario, "measure_background_msaa_flood_20000", 20_000);
}
