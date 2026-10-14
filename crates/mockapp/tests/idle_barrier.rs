//! The outpost's part of the harness's idle barrier
//! (`docs/design/communication.md`, section 6.7) in a real outpost,
//! against mockapp's UIA text provider: the answer to an idle query goes
//! behind every message the outpost sent before it, one whose write is in
//! flight and one waiting in its backlog included, and is held back while
//! a caret key's watch is open, going behind the watch's answer.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::{
    CaretReport, CaretWatch, NodeId, NormalizedEvent, TextOp, TextPoint, TextPosition, TextReply,
    TextUnit,
};
use verbatim_outpost::protocol::{
    DeliveredFact, OutpostToSupervisor, Query, QueryOutcome, QueryResult, SupervisorToOutpost,
};
use verbatim_outpost::{Heard, OutpostOptions};
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;

use common::outpost::OutpostUnderTest;

/// A pipe buffer smaller than one read of the fixture's document once its
/// text is long, so that read's answer stays in flight until the test
/// reads it.
const SMALL_PIPE: u32 = 4096;

/// How long the document's text is made.
const LONG_TEXT: usize = 20_000;

/// mockapp's "caret watch" fixture in its UIA backend.
fn start(name: &str) -> (common::MockApp, HWND) {
    common::init_com();
    let title = common::unique_title(name);
    let app = common::spawn("caret_watch.json", "uia", &title);
    let hwnd = common::find_window(&title);
    (app, hwnd)
}

/// Asserts the outpost's next message is the answer to `request`, a read
/// of the document's whole long text.
fn whole_text(outpost: &OutpostUnderTest, request: u64) {
    match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Range { text, truncated })),
            ..
        } if request_id == request => {
            assert_eq!((text.len(), truncated), (LONG_TEXT, false));
        }
        other => panic!("the outpost said {other:?}, not the answer to read {request}"),
    }
}

/// An idle query handed over while a message's write is in flight and
/// another waits in the backlog behind it is answered behind both: the
/// worker reaches the query only once it has handled everything handed
/// over before it, and the answer waits in the backlog behind what is
/// there, never ahead of it as a pong goes.
fn the_answer_goes_behind_a_message_in_the_pipe_and_one_in_the_backlog() {
    let (mut app, hwnd) = start("mockapp-idle-backlog");
    app.send(&format!("set-text doc {}", "x".repeat(LONG_TEXT)));
    let mut outpost =
        OutpostUnderTest::with_pipe_buffer(&app, OutpostOptions::default(), SMALL_PIPE);
    let (notes, _) = notes_focus(&mut app, &outpost, hwnd);
    outpost.hold_worker();
    let read = TextOp::ReadRange {
        start: TextPoint::Start,
        end: TextPoint::End,
    };
    let first = outpost.ask(Query::Text {
        node_id: notes,
        op: read.clone(),
    });
    let second = outpost.ask(Query::Text {
        node_id: notes,
        op: read,
    });
    outpost
        .outpost
        .handle_command(&SupervisorToOutpost::IdleQuery { round: 7 });
    // A foreground change behind the query holds the worker again once it
    // has handled everything before it.
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    outpost.step_worker(2);
    // The first read's answer is in flight, the second's and the idle
    // answer wait behind it; only now does Core read.
    whole_text(&outpost, first);
    whole_text(&outpost, second);
    assert_eq!(outpost.next(), OutpostToSupervisor::IdleAnswer { round: 7 });
    outpost.release_worker();
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Focuses mockapp's "Notes" document, hands the outpost the fact the
/// listener sends for it, and returns its node and the caret the outpost
/// reported after the focus, which must be all it said.
fn notes_focus(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    hwnd: HWND,
) -> (NodeId, CaretReport) {
    app.send("set-focus doc");
    let uia = Uia::new().expect("a UIA client");
    let element = common::listener_focus(&uia, hwnd.0 as isize);
    outpost.deliver(outpost.listener_fact(&element));
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => node,
        other => panic!("the outpost said {other:?}, not the document's focus"),
    };
    assert_eq!(node.name.as_deref(), Some("Notes"));
    let caret = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, caret },
            ..
        } if node_id == node.id => caret,
        other => panic!("the outpost said {other:?}, not the document's caret"),
    };
    outpost.settled();
    (node.id, caret)
}

/// An idle query handed over while a caret key's watch is open is not
/// answered while the watch waits for its evidence: the answer goes once
/// the application's caret event answers the key, behind that answer.
fn the_answer_waits_for_an_open_caret_watch() {
    let (mut app, hwnd) = start("mockapp-idle-caret-watch");
    let mut outpost = OutpostUnderTest::new(&app);
    let (notes, before) = notes_focus(&mut app, &outpost, hwnd);
    // Right Arrow, which the application has not handled yet: the watch
    // stays open.
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: Some(TextPosition {
                anchor: before.line.start,
                offset: before.line.offset,
            }),
            pressed_at_ms: 0,
            unit: TextUnit::Character,
            compare: None,
            previous_selection: None,
        }),
    });
    outpost.settled();
    outpost
        .outpost
        .handle_command(&SupervisorToOutpost::IdleQuery { round: 3 });
    // The worker has reached the query, and nothing is said.
    outpost.settled();
    app.send("caret doc 1");
    app.send("caret-event doc");
    match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Caret(reply))),
            ..
        } if request_id == key => assert!(reply.moved),
        other => panic!("the outpost said {other:?}, not the key's answer"),
    }
    assert_eq!(outpost.next(), OutpostToSupervisor::IdleAnswer { round: 3 });
    outpost.heard(&[
        Heard::UiaTextSelection,
        Heard::Msaa(WinEventKind::TextSelectionChange),
    ]);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "the_answer_goes_behind_a_message_in_the_pipe_and_one_in_the_backlog",
            the_answer_goes_behind_a_message_in_the_pipe_and_one_in_the_backlog,
        ),
        (
            "the_answer_waits_for_an_open_caret_watch",
            the_answer_waits_for_an_open_caret_watch,
        ),
    ]);
}
