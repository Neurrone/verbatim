//! Verbatim's menu closed over an application that has stopped responding
//! (`docs/parity/focus-events.md`, "A window that stops responding"): a
//! `mockapp` MSAA window stalls (mockapp's `stall`) while Verbatim's menu
//! is in front, until Windows judges it hung. Windows makes a ghost for a
//! hung window only when something calls into it, and Verbatim
//! deliberately makes no call into a window it knows is hung, so with
//! Verbatim alone no ghost appears (none in 30 s, 3 of 3 runs on
//! 2026-11-19). The captures' ghost behind the menu came from NVDA's own
//! reads of the window, which NVDA makes whatever its state, and from the
//! event recorders. So the harness makes one call into the hung window
//! itself, the first question UI Automation asks a window
//! (`Scenario::call_into_window`), which changes nothing in mockapp, and
//! waits for Windows to show the ghost, titled with " (Not Responding)",
//! behind the menu. Escape closes the menu with the ghost in front of the
//! hung window, and the check 50 ms later, NVDA's fake focus, finds the
//! ghost in front: Verbatim
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
//! started` and `stall ended` lines (`--line-events`); its `applied` line
//! for a command it takes six seconds into the stall, while still taking
//! no message, past the five seconds after which Windows judges a window
//! that takes none hung (`IsHungAppWindow`); and the window events that
//! show the ghost.

use std::io;
use std::time::Duration;

use verbatim_agent::protocol::UiaRootAnswer;

use crate::registry::{ScenarioState, startup_speech};
use crate::scenario::{Scenario, WINDOW_TIMEOUT, harness_marker};

pub(crate) use super::no_setup as setup;
pub(crate) use super::no_teardown as teardown;

/// The stalling mockapp window's name in the run.
const HUNG: &str = "hung-window";

/// How long the first stall lasts: the five seconds Windows takes to call
/// the window hung, the Escape and four commands, with room to spare.
const FIRST_STALL_MS: u64 = 40_000;

/// How long the second stall lasts: the six seconds before the window is
/// judged hung, its ghost, and the replacement of Verbatim.
const SECOND_STALL_MS: u64 = 40_000;

/// The prefix of the named events the mockapp window sets as it prints
/// its lines, unique to this run of the test binary.
fn line_events() -> String {
    format!(r"Local\verbatim-e2e-hung-window-{}-", std::process::id())
}

/// Opens the mockapp window, titled with [`harness_marker`] of [`HUNG`],
/// an MSAA window holding one button "OK", whose focus it raises on its
/// own client, its standard input a
/// pipe and its lines setting the events of `events`, and waits for it to
/// take the foreground. Returns its process id and window.
fn open(scenario: &mut Scenario, events: &str) -> io::Result<(u32, u64)> {
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
    Ok((window.pid, window.window))
}

/// How long the harness's call into the hung window waits for an answer
/// that does not come.
const CALL_LIMIT: Duration = Duration::from_secs(2);

/// How far into a stall mockapp takes the command whose acknowledgement
/// is the evidence that Windows judges the window hung: past the five
/// seconds with no message taken after which `IsHungAppWindow` answers
/// true.
const HUNG_AFTER_MS: u64 = 6_000;

/// The event mockapp sets as it prints a `stall` line, its start or its
/// end.
fn stall_line(events: &str) -> String {
    format!("{events}stall")
}

/// Stalls mockapp `pid` for `ms` and waits for its `stall started` line,
/// then for the acknowledgement of a command that changes nothing it sees
/// (`refuse-text off`, the default), taken [`HUNG_AFTER_MS`] into the stall
/// while still taking no message, so Windows judges the window hung; then
/// makes ready the wait for its `stall ended` line.
fn stall(scenario: &mut Scenario, pid: u32, events: &str, ms: u64) {
    let line = stall_line(events);
    let applied = format!("{events}applied");
    scenario
        .create_event(&line)
        .expect("creates the event for the stall's start");
    scenario
        .create_event(&applied)
        .expect("creates the event for the command inside the stall");
    scenario
        .write_stdin(pid, &format!("stall {ms} {HUNG_AFTER_MS} refuse-text off"))
        .expect("writes stall");
    scenario
        .wait_for_event(&line, pid, WINDOW_TIMEOUT)
        .expect("mockapp's stall starts");
    scenario
        .create_event(&line)
        .expect("creates the event for the stall's end");
    scenario
        .wait_for_event(&applied, pid, WINDOW_TIMEOUT)
        .expect("mockapp has taken no message for six seconds, so Windows judges it hung");
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
    let (pid, hung_window) = open(scenario, &events).expect("opens the mockapp window");
    let title = harness_marker(HUNG);
    let window = format!("{title} window");
    let ghost = format!("{title} (Not Responding)");
    let plain = format!("{ghost} window");
    // mockapp's MSAA window raises its focus on its own client as it
    // opens, never on its button, as in `msaa_flood_then_focus`.
    scenario.speech().expect(&[&window]);

    // The menu over the window, which stalls until Windows judges it hung;
    // one call into it, as NVDA's reads make, and Windows shows its ghost
    // behind the menu.
    super::open_verbatim_menu(scenario);
    stall(scenario, pid, &events, FIRST_STALL_MS);
    assert_eq!(
        scenario
            .call_into_window(hung_window, CALL_LIMIT)
            .expect("calls into the hung window"),
        UiaRootAnswer::TimedOut {
            limit_ms: u64::try_from(CALL_LIMIT.as_millis()).expect("fits")
        },
        "the hung window answers nothing"
    );
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
