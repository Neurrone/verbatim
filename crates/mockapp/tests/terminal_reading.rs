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
//!
//! While the terminal is held, the evidence that the outpost has taken a
//! text change is the report of a caret event raised after it: UI
//! Automation delivers one client's events from a provider in the order
//! they were raised, and the outpost reads the caret on a caret event
//! whatever the reading state.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::{NodeId, NormalizedEvent, Role, TerminalOutput, TextOp, TextReply};
use verbatim_outpost::listener::uia_focus_fact;
use verbatim_outpost::protocol::{
    ListenerFact, OutpostToSupervisor, Query, QueryOutcome, QueryResult,
};
use verbatim_uia::{ElementExt, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_HasKeyboardFocusPropertyId,
};

use common::outpost::OutpostUnderTest;

/// The terminal's text before each test writes to it.
const START: &str = "one\ntwo\nready>";

/// The element mockapp reports as having the keyboard focus, built with the
/// listener's cache request, as a focus event's element arrives.
fn focused(hwnd: HWND) -> IUIAutomationElement {
    let uia = Uia::new().expect("a UIA client");
    let cache = uia.base_cache_request().expect("a cache request");
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("mockapp's root element");
    let condition = uia
        .property_condition(UIA_HasKeyboardFocusPropertyId, &VARIANT::from(true))
        .expect("a condition");
    root.find_first_build_cache(TreeScope_Descendants, &condition, &cache)
        .expect("the search")
        .expect("mockapp reports a focused element")
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
    common::init_com();
    let title = common::unique_title(name);
    let mut app = common::spawn("terminal_reading.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    app.send("set-focus term");
    let element = focused(hwnd);
    outpost.read_focus_as(&element);
    let ListenerFact { pid: _, fact } =
        uia_focus_fact(&element).expect("mockapp's element has its process");
    outpost.deliver(fact);
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

/// Runs each test on a desktop of its own (`common/harness.rs`).
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
    ]);
}
