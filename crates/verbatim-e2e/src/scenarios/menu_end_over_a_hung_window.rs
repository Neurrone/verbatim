//! Verbatim's menu closed over an application that has stopped responding
//! (`docs/parity/focus-events.md`, "A window that stops responding"): a
//! `mockapp` MSAA window stalls (mockapp's `stall`) while Verbatim's menu
//! is in front, until Windows judges it hung. Escape closes the menu, and
//! Windows activates nothing: no window is in front. NVDA's fake focus then
//! focuses its `Desktop`, "Desktop  window" (1 of 18 Escapes in the
//! captures), and so does Verbatim (Dickson, 2026-11-19); when the
//! application answers again, its window is spoken, "<title> window".
//!
//! The ghost case, a ghost window in front when the menu closes, which
//! Verbatim speaks as NVDA's plain window "<title> (Not Responding)
//! window", has no step here. Windows made no ghost for the hung window in
//! any end-to-end run, with Verbatim alone or with one `WM_GETOBJECT` sent
//! by the harness (3 of 3 runs each, 2026-11-19); the captures' ghost came
//! while NVDA or the event recorders were reading the window, and no single
//! call of theirs could be shown to cause it. It is covered by Core's
//! `focus::plain` tests and `hung_menu_end`, in the order of Verbatim's own
//! log of the captures.
//!
//! Every wait is on evidence: mockapp's named events for its `stall
//! started` and `stall ended` lines (`--line-events`), and its `applied`
//! line for a command it takes six seconds into the stall, while still
//! taking no message, past the five seconds after which Windows judges a
//! window that takes none hung (`IsHungAppWindow`).

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::{Launch, Start};
use crate::scenario::{Scenario, WINDOW_TIMEOUT, harness_marker};

pub(crate) use super::no_setup as setup;
pub(crate) use super::no_teardown as teardown;

/// The stalling mockapp window's name in the run.
const HUNG: &str = "hung-window";

/// How long the stall lasts: the six seconds before the window is judged
/// hung, and the Escape.
const STALL_MS: u64 = 20_000;

/// The prefix of the named events the mockapp window sets as it prints
/// its lines, unique to this run of the test binary.
fn line_events() -> String {
    format!(r"Local\verbatim-e2e-hung-window-{}-", std::process::id())
}

/// Opens the mockapp window, titled with [`harness_marker`] of [`HUNG`],
/// an MSAA window holding one button "OK", whose focus it raises on its
/// own client, its standard input a
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
    let window = scenario.launch(
        &format!(r"{directory}\mockapp.exe"),
        &args,
        Launch {
            start: Start::WithStdin { output_to: &output },
            ..Launch::titled(&title)
        },
    )?;
    Ok(window.pid)
}

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
    let pid = open(scenario, &events).expect("opens the mockapp window");
    let title = harness_marker(HUNG);
    let window = format!("{title} window");
    // mockapp's MSAA window raises its focus on its own client as it
    // opens, never on its button, as in `msaa_flood_then_focus`.
    scenario.speech().expect(&[&window]);

    // The menu closed over the hung window: no window is in front, and the
    // desktop is focused.
    super::open_verbatim_menu(scenario);
    stall(scenario, pid, &events, STALL_MS);
    scenario.send_keys(&["escape"]).expect("sends escape");
    scenario.speech().expect(&["Desktop window"]);
    stall_ended(scenario, pid, &events, STALL_MS);
    scenario.speech().expect(&[&window]);
}
