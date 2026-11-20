//! mockapp, the scripted application staged beside Verbatim, launched as
//! the scenarios launch it: one window titled with [`harness_marker`] of a
//! name, from a fixture whose `{title}` is replaced by that title, brought
//! to the foreground. [`open_mockapp`] and its variants give it a standard
//! input pipe and line events, for scenarios that drive it with commands
//! ([`apply_to_mockapp`]); [`open_without_input`] gives it neither.

use std::io;

use crate::scenario::{Launch, Start};
use crate::scenario::{Scenario, WINDOW_TIMEOUT, harness_marker};

/// `fixture`, mockapp's JSON, with its root named `title`, the window's
/// title, and every `{title}` in it replaced by that title, so a scenario
/// can load one of mockapp's own fixture files
/// (`crates/mockapp/tests/fixtures`) with `include_str!`.
///
/// # Panics
///
/// Panics if `fixture` is not a JSON object, a defect of the scenario.
pub(crate) fn titled(fixture: &str, title: &str) -> String {
    let mut root: serde_json::Value = serde_json::from_str(&fixture.replace("{title}", title))
        .expect("a mockapp fixture is JSON");
    root["name"] = serde_json::Value::from(title);
    serde_json::to_string_pretty(&root).expect("JSON serializes")
}

/// Opens a mockapp window on `backend` (`uia` or `msaa`) titled with
/// [`harness_marker`] of `name`, from `fixture` with `{title}` replaced by
/// that title, with no standard input and no line events, and waits for it
/// to take the foreground. Returns its process id.
///
/// # Errors
///
/// Returns an error if the fixture cannot be written, mockapp cannot be
/// started, or its window does not take the foreground.
pub(crate) fn open_without_input(
    scenario: &mut Scenario,
    name: &str,
    fixture: &str,
    backend: &str,
) -> io::Result<u32> {
    let directory = scenario.run_directory().to_owned();
    let title = harness_marker(name);
    let path = scenario.harness_file(name, "json");
    scenario.write_agent_file(&path, titled(fixture, &title).as_bytes())?;
    let args = [
        "--fixture",
        &path,
        "--backend",
        backend,
        "--title",
        &title,
        "--show",
    ]
    .map(str::to_owned);
    let window = scenario.launch(
        &format!(r"{directory}\mockapp.exe"),
        &args,
        Launch::titled(&title),
    )?;
    Ok(window.pid)
}

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
pub(crate) fn open_mockapp(scenario: &mut Scenario, name: &str, fixture: &str) -> u32 {
    open_mockapp_with(scenario, name, fixture, "uia", &[])
}

/// [`open_mockapp`], for mockapp's MSAA backend.
pub(crate) fn open_msaa_mockapp(scenario: &mut Scenario, name: &str, fixture: &str) -> u32 {
    open_mockapp_with(scenario, name, fixture, "msaa", &[])
}

/// [`open_mockapp`], its root counting UIA's event registrations, for
/// mockapp's `await-advised`.
pub(crate) fn open_mockapp_counting(scenario: &mut Scenario, name: &str, fixture: &str) -> u32 {
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
        .write_agent_file(&path, titled(fixture, &title).as_bytes())
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
        .launch(
            &format!(r"{directory}\mockapp.exe"),
            &args,
            Launch {
                start: Start::WithStdin { output_to: &output },
                ..Launch::titled(&title)
            },
        )
        .expect("opens the mockapp window")
        .pid
}

/// Writes `command` to the mockapp window `name`, process `pid`, and waits
/// for mockapp's acknowledgement that it took effect.
pub(crate) fn apply_to_mockapp(scenario: &mut Scenario, window: (&str, u32), command: &str) {
    send_to_mockapp(scenario, window, command);
    await_mockapp(scenario, window);
}

/// Writes `command` to the mockapp window `name`, process `pid`, leaving
/// its acknowledgement to [`await_mockapp`], so a scenario acts while the
/// command runs.
pub(crate) fn send_to_mockapp(scenario: &mut Scenario, (name, pid): (&str, u32), command: &str) {
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
pub(crate) fn await_mockapp(scenario: &mut Scenario, (name, pid): (&str, u32)) {
    scenario
        .wait_for_event(
            &format!("{}applied", line_events(name)),
            pid,
            WINDOW_TIMEOUT,
        )
        .expect("mockapp applies the command");
}
