//! Verbatim's menu closed over an application that has stopped responding
//! (`docs/parity/focus-events.md`, "A window that stops responding"): a
//! `mockapp` MSAA window stalls (mockapp's `stall`) while Verbatim's menu
//! is in front, and Windows shows its ghost window, titled with " (Not
//! Responding)", behind the menu. Escape closes the menu, and the check
//! 50 ms later, NVDA's fake focus, finds the ghost in front: Verbatim
//! speaks it as NVDA's plain window object of it, "<title> (Not
//! Responding) window", with no sound (NVDA alpha-58022, 20 of 20 runs;
//! Dickson, 2026-11-19). Report focus, report the navigator object and
//! review the current line read that plain object; say-all says nothing,
//! as NVDA's says nothing where it can make no caret position. These
//! commands' speech comes from NVDA's source, since injected keys do not
//! reach NVDA while the ghost is in front (Dickson, 2026-11-19). When the
//! application answers again, its window is spoken as NVDA speaks it,
//! "<title> window".
//!
//! Then the start: the application stalls again with its window in front,
//! its ghost takes the foreground, and a second Verbatim replaces the
//! first; its read of the focus at startup finds the ghost and says the
//! plain window, as NVDA's log says at its start (5 of 5 runs), and the
//! application's window when it answers again.
//!
//! Every wait is on evidence: mockapp's named events for its `stall
//! started` and `stall ended` lines (`--line-events`), and the window
//! events that show the ghost.

use std::io;
use std::time::Duration;

use crate::registry::{ScenarioState, startup_speech};
use crate::scenario::{Scenario, WINDOW_TIMEOUT, harness_marker};

pub(crate) use super::no_setup as setup;
pub(crate) use super::no_teardown as teardown;

/// The stalling mockapp window's name in the run.
const HUNG: &str = "hung-window";

/// How long the first stall lasts: the five seconds Windows takes to call
/// the window hung, the Escape and four commands, with room to spare.
const FIRST_STALL_MS: u64 = 40_000;

/// How long the second stall lasts: the five seconds before the ghost,
/// and the replacement of Verbatim.
const SECOND_STALL_MS: u64 = 40_000;

/// The prefix of the named events the mockapp window sets as it prints
/// its lines, unique to this run of the test binary.
fn line_events() -> String {
    format!(r"Local\verbatim-e2e-hung-window-{}-", std::process::id())
}

/// Opens the mockapp window, titled with [`harness_marker`] of [`HUNG`],
/// an MSAA window holding one focused button "OK", its standard input a
/// pipe and its lines setting the events of `events`, and waits for it to
/// take the foreground. Returns its process id.
fn open(scenario: &mut Scenario, events: &str) -> io::Result<u32> {
    let directory = scenario.run_directory().to_owned();
    let title = harness_marker(HUNG);
    let fixture = scenario.harness_file(HUNG, "json");
    let output = scenario.harness_file(HUNG, "log");
    let contents = format!(
        r#"{{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    {{ "id": "ok", "role": "button", "name": "OK", "states": ["focusable", "focused"] }}
  ]
}}
"#
    );
    scenario.write_agent_file(&fixture, contents.as_bytes())?;
    let args = [
        "--fixture",
        &fixture,
        "--backend",
        "msaa",
        "--title",
        &title,
        "--show",
        "--line-events",
        events,
    ]
    .map(str::to_owned);
    let window = scenario.launch_titled_with_stdin(
        &format!(r"{directory}\mockapp.exe"),
        &args,
        &title,
        &output,
    )?;
    Ok(window.pid)
}

/// The event mockapp sets as it prints a `stall` line, its start or its
/// end.
fn stall_line(events: &str) -> String {
    format!("{events}stall")
}

/// Stalls mockapp `pid` for `ms` and waits for its `stall started` line,
/// then makes ready the wait for its `stall ended` line.
fn stall(scenario: &mut Scenario, pid: u32, events: &str, ms: u64) {
    let line = stall_line(events);
    scenario
        .create_event(&line)
        .expect("creates the event for the stall's start");
    scenario
        .write_stdin(pid, &format!("stall {ms}"))
        .expect("writes stall");
    scenario
        .wait_for_event(&line, pid, WINDOW_TIMEOUT)
        .expect("mockapp's stall starts");
    scenario
        .create_event(&line)
        .expect("creates the event for the stall's end");
}

/// Waits for mockapp `pid`'s `stall ended` line, which comes up to `ms`
/// after its start.
fn stall_ended(scenario: &mut Scenario, pid: u32, events: &str, ms: u64) {
    scenario
        .wait_for_event(
            &stall_line(events),
            pid,
            Duration::from_millis(ms) + WINDOW_TIMEOUT,
        )
        .expect("mockapp's stall ends");
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let events = line_events();
    let pid = open(scenario, &events).expect("opens the mockapp window");
    let title = harness_marker(HUNG);
    let window = format!("{title} window");
    let ghost = format!("{title} (Not Responding)");
    let plain = format!("{ghost} window");
    scenario.speech().expect(&[&window, "OK button"]);

    // The menu over the window, which stalls; Windows shows its ghost
    // behind the menu once it has called the window hung.
    super::open_verbatim_menu(scenario);
    stall(scenario, pid, &events, FIRST_STALL_MS);
    scenario
        .wait_for_window_titled(&ghost)
        .expect("Windows shows the hung window's ghost");

    scenario.send_keys(&["escape"]).expect("sends escape");
    scenario.speech().expect(&[&plain]);

    scenario
        .send_gesture("kb:verbatim+tab")
        .expect("sends report focus");
    scenario.speech().expect(&[&plain]);
    scenario
        .send_gesture("kb:verbatim+numpad5")
        .expect("sends report the navigator object");
    scenario.speech().expect(&[&plain]);
    scenario
        .send_gesture("kb:numpad8")
        .expect("sends review the current line");
    scenario.speech().expect(&[&ghost]);
    // Say-all says nothing: the next thing said is the window answering.
    scenario
        .send_gesture("kb:verbatim+downarrow")
        .expect("sends say-all");

    stall_ended(scenario, pid, &events, FIRST_STALL_MS);
    scenario.speech().expect(&[&window]);

    // The start over the ghost: the window stalls in front, its ghost
    // takes the foreground, and a second Verbatim replaces the first.
    stall(scenario, pid, &events, SECOND_STALL_MS);
    scenario
        .wait_for_window_in_front(&ghost, WINDOW_TIMEOUT)
        .expect("the hung window's ghost takes the foreground");
    scenario
        .replace_verbatim()
        .expect("a second Verbatim replaces the first");
    scenario.speech().expect_sequence(&startup_speech());
    scenario.speech().expect(&[&plain]);

    stall_ended(scenario, pid, &events, SECOND_STALL_MS);
    scenario.speech().expect(&[&window]);
}
