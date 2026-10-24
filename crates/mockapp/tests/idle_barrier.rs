//! The outpost's part of the harness's idle barrier
//! (`docs/design/communication.md`, section 6.7) in a real outpost,
//! against mockapp's UIA text provider: the answer to an idle query goes
//! behind every message the outpost sent before it, one whose write is in
//! flight and one waiting in its backlog included, and is held back while
//! a caret key's watch is open, going behind the watch's answer; a held
//! terminal's noted caret holds it back only for that caret's read.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::{
    CaretReport, CaretWatch, FocusText, NodeId, NormalizedEvent, TextOp, TextPoint, TextPosition,
    TextReply, TextUnit,
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
        through: None,
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
/// reported with the focus, which must be all it said.
fn notes_focus(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    hwnd: HWND,
) -> (NodeId, CaretReport) {
    app.send("set-focus doc");
    let uia = Uia::new().expect("a UIA client");
    let element = common::listener_focus(&uia, hwnd.0 as isize);
    outpost.deliver(outpost.listener_fact(&element));
    let (node, caret) = match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    text: Some(text),
                    ..
                },
            ..
        } if matches!(*text, FocusText::Caret { .. }) => {
            let FocusText::Caret { caret, .. } = *text else {
                unreachable!("matched by the guard");
            };
            (node, caret)
        }
        other => panic!("the outpost said {other:?}, not the document's focus with its caret"),
    };
    assert_eq!(node.name.as_deref(), Some("Notes"));
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
            deletes: false,
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

/// mockapp's terminal fixture in its UIA backend, with an outpost watching
/// it, the terminal focused, its focus and caret said, and nothing else;
/// then held, as Core holds it for a flood, and written to, so that its
/// caret events are only noted. The noted caret is read once nothing else
/// waits, and finds the caret Core has: nothing is said.
fn held_terminal(name: &str) -> (common::MockApp, OutpostUnderTest, NodeId) {
    common::init_com();
    let title = common::unique_title(name);
    let mut app = common::spawn("terminal_reading.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut outpost = OutpostUnderTest::new(&app);
    // The screen: the text's last three rows, and its empty last line.
    app.send("screen term 4");
    app.send("set-focus term");
    let uia = Uia::new().expect("a UIA client");
    let element = common::listener_focus(&uia, hwnd.0 as isize);
    outpost.deliver(outpost.listener_fact(&element));
    let term = match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    text: Some(text),
                    ..
                },
            ..
        } if matches!(*text, FocusText::Caret { .. }) => node.id,
        other => panic!("the outpost said {other:?}, not the terminal's focus with its caret"),
    };
    outpost.settled();
    let hold = outpost.ask(Query::Text {
        node_id: term,
        op: TextOp::TerminalHold,
    });
    match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Done)),
            ..
        } if request_id == hold => {}
        other => panic!("the outpost said {other:?}, not the answer to the hold"),
    }
    app.send(&format!(
        "set-text term {}",
        ["one", "two", "ready>", "three"]
            .map(|row| format!(r"{row:20}\r\n"))
            .concat()
    ));
    app.send("text-changed term");
    outpost.heard(&[
        Heard::UiaTextChanged,
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
    outpost.settled();
    (app, outpost, term)
}

/// An idle query reached while a held terminal's caret is noted goes
/// directly behind the noted caret's read, not behind what arrived after
/// the query: Core's read of the terminal, asked after the query, is
/// answered after the idle answer. During a flood, what arrives after the
/// query notes the caret again on nearly every write, so a query sent
/// behind it would never be answered.
fn a_noted_caret_holds_the_answer_back_only_for_its_own_read() {
    let (mut app, mut outpost, term) = held_terminal("mockapp-idle-noted-caret");
    outpost.hold_worker();
    // Noted when handled, the terminal being held with its text changed.
    app.send("caret-event term");
    outpost.heard(&[
        Heard::UiaTextSelection,
        Heard::Msaa(WinEventKind::TextSelectionChange),
    ]);
    outpost
        .outpost
        .handle_command(&SupervisorToOutpost::IdleQuery { round: 5 });
    let read = outpost.ask(Query::Text {
        node_id: term,
        op: TextOp::TerminalRead { hold: false },
    });
    outpost.release_worker();
    assert_eq!(outpost.next(), OutpostToSupervisor::IdleAnswer { round: 5 });
    // The read reads the caret with the text, and answers with the line
    // written while the terminal was held.
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, .. },
            ..
        } if node_id == term => {}
        other => panic!("the outpost said {other:?}, not the terminal's caret"),
    }
    match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Terminal(output))),
            ..
        } if request_id == read => assert_eq!(
            output.items,
            [verbatim_model::TerminalItem::Output("three".to_owned())]
        ),
        other => panic!("the outpost said {other:?}, not the answer to the read"),
    }
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_noted_caret_holds_the_answer_back_only_for_its_own_read",
            a_noted_caret_holds_the_answer_back_only_for_its_own_read,
        ),
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
