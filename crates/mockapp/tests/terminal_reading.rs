//! A focused terminal's on-demand reading (`phase6-design.md`, "Terminal
//! decisions") through a real outpost's worker, against mockapp's UIA text
//! provider presenting a Windows Terminal control (UIA class
//! `TermControl`): each transition of `verbatim_outpost::terminal::reading`
//! as the worker drives it, by text change events and Core's requests.
//!
//! - Live, a text change is read at once and reported as terminal output,
//!   after the caret read with it.
//! - Held (`TerminalHold`), a change is only noted: nothing is said.
//! - `TerminalRead` answers with what is new since the last read, and
//!   reading is live again; asked with nothing changed, it answers empty
//!   without reading, and asked to hold, the terminal stays held.
//! - `TerminalCancel` answers with what is new, and reading is live again.
//! - A caret event waiting ahead of a text change in one batch is planned
//!   after it, so the caret is read once, with the text.
//! - Held with its text changed, a caret event is only noted, and the read
//!   Core asks for reads the caret with the text.
//! - The console host's text area, in a window of the console's class,
//!   takes its caret event from its console (`EVENT_CONSOLE_CARET`), and
//!   UI Automation's caret event is not subscribed for it.
//!
//! While the terminal is held, the evidence that the outpost has taken a
//! text change is the report of a caret event raised after it: UI
//! Automation delivers one client's events from a provider in the order
//! they were raised, and the outpost reads the caret of a caret event noted
//! while held once it has nothing else to do. The two tests of one batch
//! hold the outpost's worker ([`OutpostUnderTest::hold_worker`]) while
//! mockapp raises the batch's events.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::{
    NodeId, NormalizedEvent, PropertyChange, Role, TerminalOutput, TextOp, TextReply,
};
use verbatim_outpost::Heard;
use verbatim_outpost::protocol::{OutpostToSupervisor, Query, QueryOutcome, QueryResult};
use verbatim_uia::{Cached, ListenerEvent, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::UIA_HelpTextPropertyId;

use common::outpost::OutpostUnderTest;

/// The terminal's text before each test writes to it.
const START: &str = "one\ntwo\nready>";

/// The element mockapp reports as having the keyboard focus, built with the
/// listener's cache request, as a focus event's element arrives.
fn focused(hwnd: HWND) -> Cached<ListenerEvent> {
    let uia = Uia::new().expect("a UIA client");
    common::listener_focus(&uia, hwnd.0 as isize)
}

/// The caret the outpost reports, which must be its next message.
fn caret_read(outpost: &OutpostUnderTest, node: NodeId) {
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, .. },
            ..
        } if node_id == node => {}
        other => panic!("the outpost said {other:?}, not the terminal's caret"),
    }
}

/// mockapp's terminal fixture in its UIA backend, with an outpost watching
/// it and the terminal focused: its focus and caret said, and nothing else.
fn start(name: &str) -> (common::MockApp, OutpostUnderTest, NodeId) {
    start_with(name, "terminal_reading.json", "term")
}

/// [`start`] for the terminal `id` of fixture `fixture`.
fn start_with(name: &str, fixture: &str, id: &str) -> (common::MockApp, OutpostUnderTest, NodeId) {
    common::init_com();
    let title = common::unique_title(name);
    let mut app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    app.send(&format!("set-focus {id}"));
    let element = focused(hwnd);
    outpost.deliver(outpost.listener_fact(&element));
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => node,
        other => panic!("the outpost said {other:?}, not the terminal's focus"),
    };
    assert_eq!(node.role, Role::Terminal);
    caret_read(&outpost, node.id);
    outpost.settled();
    (app, outpost, node.id)
}

/// Writes `lines` after [`START`] as the terminal's whole text and raises
/// its text changed event.
fn write(app: &mut common::MockApp, lines: &[&str]) {
    let mut text = START.to_owned();
    for line in lines {
        text.push('\n');
        text.push_str(line);
    }
    app.send(&format!("set-text term {}", text.replace('\n', r"\n")));
    app.send("text-changed term");
}

/// Writes `lines` as [`write`] does while the terminal is held, then moves
/// the caret to `caret` and raises its caret event, and takes its report:
/// the evidence that the outpost has taken the text change.
fn write_held(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    node: NodeId,
    lines: &[&str],
    caret: usize,
) {
    write(app, lines);
    app.send(&format!("caret term {caret}"));
    app.send("caret-event term");
    caret_read(outpost, node);
    outpost.settled();
}

/// The output the outpost reports live, after the caret read with it.
fn reported(outpost: &OutpostUnderTest, node: NodeId) -> TerminalOutput {
    caret_read(outpost, node);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::TerminalOutput { node_id, output },
            ..
        } if node_id == node => output,
        other => panic!("the outpost said {other:?}, not the terminal's output"),
    }
}

/// The answer to request `request`, which must be the outpost's next
/// message.
fn answer_to(outpost: &OutpostUnderTest, request: u64) -> TextReply {
    match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(reply)),
            ..
        } if request_id == request => reply,
        other => panic!("the outpost said {other:?}, not the answer to {request}"),
    }
}

/// The answer to request `request`, after the caret read with it.
fn read_answer(outpost: &OutpostUnderTest, node: NodeId, request: u64) -> TextReply {
    caret_read(outpost, node);
    answer_to(outpost, request)
}

/// Output of new lines only.
fn lines(lines: &[&str]) -> TerminalOutput {
    TerminalOutput {
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        ..TerminalOutput::default()
    }
}

/// Asks the outpost `op` about `node`.
fn ask(outpost: &mut OutpostUnderTest, node: NodeId, op: TextOp) -> u64 {
    outpost.ask(Query::Text { node_id: node, op })
}

/// Holds the terminal, which is answered at once.
fn hold(outpost: &mut OutpostUnderTest, node: NodeId) {
    let hold = ask(outpost, node, TextOp::TerminalHold);
    assert_eq!(answer_to(outpost, hold), TextReply::Done);
    outpost.settled();
}

/// Live, then held, then read: a change while held says nothing, and the
/// read answers with it; reading is then live again.
fn a_held_terminal_notes_changes_and_a_read_answers_them() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-held");
    write(&mut app, &["three"]);
    assert_eq!(reported(&outpost, term), lines(&["three"]));
    outpost.settled();

    hold(&mut outpost, term);
    write_held(&mut app, &outpost, term, &["three", "four"], 1);
    let read = ask(&mut outpost, term, TextOp::TerminalRead { hold: false });
    assert_eq!(
        read_answer(&outpost, term, read),
        TextReply::Terminal(Box::new(lines(&["four"])))
    );
    outpost.settled();

    // Live again.
    write(&mut app, &["three", "four", "five"]);
    assert_eq!(reported(&outpost, term), lines(&["five"]));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A read asked with nothing changed since the last answers empty, and
/// one asked to hold again leaves the terminal held.
fn a_read_with_nothing_new_answers_empty_and_may_hold_again() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-empty");
    hold(&mut outpost, term);
    let read = ask(&mut outpost, term, TextOp::TerminalRead { hold: true });
    assert_eq!(
        answer_to(&outpost, read),
        TextReply::Terminal(Box::default())
    );
    outpost.settled();
    // Still held: a change is only noted.
    write_held(&mut app, &outpost, term, &["six"], 1);
    let read = ask(&mut outpost, term, TextOp::TerminalRead { hold: false });
    assert_eq!(
        read_answer(&outpost, term, read),
        TextReply::Terminal(Box::new(lines(&["six"])))
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A cancel answers with what changed while held, and reading is live
/// again.
fn a_cancel_answers_what_is_new_and_reading_is_live_again() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-cancel");
    hold(&mut outpost, term);
    write_held(&mut app, &outpost, term, &["seven"], 1);
    let cancel = ask(&mut outpost, term, TextOp::TerminalCancel);
    assert_eq!(
        read_answer(&outpost, term, cancel),
        TextReply::Terminal(Box::new(lines(&["seven"])))
    );
    outpost.settled();
    write(&mut app, &["seven", "eight"]);
    assert_eq!(reported(&outpost, term), lines(&["eight"]));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Waits for the outpost to take in the caret event and the text change
/// mockapp raised, each by both routes: UIA's own event, and the
/// `WinEvent` UIA raises for MSAA clients alongside it.
fn heard_caret_and_text(outpost: &OutpostUnderTest) {
    outpost.heard(&[
        Heard::UiaTextSelection,
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::UiaTextChanged,
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
}

/// A caret event waiting ahead of a text change in one batch is planned
/// after it: the text change's read reads the caret with the text, and the
/// caret event, which that read saw, reads nothing more. One caret report,
/// then the output.
fn a_caret_event_ahead_of_a_text_change_is_read_with_it() {
    let (mut app, outpost, term) = start("mockapp-terminal-caret-first");
    outpost.hold_worker();
    app.send("caret term 1");
    app.send("caret-event term");
    write(&mut app, &["three"]);
    heard_caret_and_text(&outpost);
    outpost.release_worker();
    assert_eq!(reported(&outpost, term), lines(&["three"]));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Held, a caret event after a text change is only noted: the read Core
/// asks for in the same batch reads the caret with the text, and the noted
/// caret, which that read saw, reads nothing more. One caret report, then
/// the answer.
fn a_held_terminals_caret_event_is_read_with_the_text() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-held-caret");
    hold(&mut outpost, term);
    outpost.hold_worker();
    write(&mut app, &["three"]);
    app.send("caret term 1");
    app.send("caret-event term");
    heard_caret_and_text(&outpost);
    let read = ask(&mut outpost, term, TextOp::TerminalRead { hold: false });
    outpost.release_worker();
    assert_eq!(
        read_answer(&outpost, term, read),
        TextReply::Terminal(Box::new(lines(&["three"])))
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
/// The console host's text area, in a window of the console's class,
/// takes its caret event from its console: UI Automation's caret event
/// reaches the outpost only as the `WinEvent` UIA raises for MSAA clients,
/// which the outpost drops, as the subscription leaves UIA's out for it,
/// and a description change UIA delivers after it is reported alone; the
/// console's own caret event (`EVENT_CONSOLE_CARET`) has the caret read
/// and reported.
fn the_console_hosts_caret_comes_from_its_console() {
    let (mut app, outpost, text) =
        start_with("mockapp-console-caret", "console_terminal.json", "text");
    app.send("caret text 1");
    app.send("caret-event text");
    app.send("set-description text moved");
    outpost.heard(&[
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::UiaProperty(UIA_HelpTextPropertyId.0),
    ]);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::PropertyChanged {
                    node_id,
                    change: PropertyChange::Description(description),
                    child_count: None,
                },
            ..
        } => assert_eq!((node_id, description.as_deref()), (text, Some("moved"))),
        other => panic!("the outpost said {other:?}, not the text area's description"),
    }
    outpost.settled();
    app.send("console-caret 1 0");
    outpost.heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    caret_read(&outpost, text);
    outpost.settled();
    drop(outpost);
    app.quit();
}

fn main() {
    harness::run_isolated(&[
        (
            "a_held_terminal_notes_changes_and_a_read_answers_them",
            a_held_terminal_notes_changes_and_a_read_answers_them,
        ),
        (
            "a_read_with_nothing_new_answers_empty_and_may_hold_again",
            a_read_with_nothing_new_answers_empty_and_may_hold_again,
        ),
        (
            "a_cancel_answers_what_is_new_and_reading_is_live_again",
            a_cancel_answers_what_is_new_and_reading_is_live_again,
        ),
        (
            "a_caret_event_ahead_of_a_text_change_is_read_with_it",
            a_caret_event_ahead_of_a_text_change_is_read_with_it,
        ),
        (
            "a_held_terminals_caret_event_is_read_with_the_text",
            a_held_terminals_caret_event_is_read_with_the_text,
        ),
        (
            "the_console_hosts_caret_comes_from_its_console",
            the_console_hosts_caret_comes_from_its_console,
        ),
    ]);
}
