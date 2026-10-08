//! A caret key's watch for evidence in a real outpost, against mockapp's
//! UIA text provider: the worker never waits for a caret key, so a focus
//! change made while a key's watch is open is handled at once, a caret move
//! the application reports later still answers the watch, a key that
//! moves nothing is answered by nothing at all, and a text pattern missing
//! as the focus arrives is read once the application raises a caret event.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::{
    CaretReply, CaretReport, CaretWatch, NodeId, NormalizedEvent, TextOp, TextPosition, TextReply,
    TextUnit,
};
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

/// Moves mockapp's focus to `id` and returns the focus fact the listener
/// would forward for it, with the outpost's read of the focused element
/// answering that element.
fn focus_fact(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    hwnd: HWND,
    id: &str,
) -> verbatim_outpost::protocol::DeliveredFact {
    app.send(&format!("set-focus {id}"));
    let element = focused(hwnd);
    outpost.read_focus_as(&element);
    let ListenerFact { pid: _, fact } =
        uia_focus_fact(&element).expect("mockapp's element has its process");
    fact
}

/// Focuses mockapp's "Notes" document and returns its node and the caret
/// the outpost reported after the focus, which must be all it said.
fn focus_notes(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    hwnd: HWND,
) -> (NodeId, CaretReport) {
    outpost.deliver(focus_fact(app, outpost, hwnd, "doc"));
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

/// Right Arrow's watch, with the caret Core knew before the key.
fn right_arrow(before: &CaretReport) -> TextOp {
    TextOp::AwaitCaret(CaretWatch {
        since: Some(TextPosition {
            anchor: before.line.start,
            offset: before.line.offset,
        }),
        pressed_at_ms: 0,
        unit: TextUnit::Character,
        compare: None,
        previous_selection: None,
    })
}

/// The answer to caret key `key`, which must be the outpost's next message.
fn answer_to(outpost: &OutpostUnderTest, key: u64) -> TextReply {
    match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(reply)),
            ..
        } if request_id == key => reply,
        other => panic!("the outpost said {other:?}, not the answer to the key"),
    }
}

/// The caret a key's answer reports, with what the key did.
fn caret_reply(reply: TextReply) -> CaretReply {
    match reply {
        TextReply::Caret(reply) => *reply,
        other => panic!("a caret reply, not {other:?}"),
    }
}

/// mockapp's "caret watch" fixture in its UIA backend, with an outpost
/// watching it.
fn start(name: &str) -> (common::MockApp, HWND, OutpostUnderTest) {
    common::init_com();
    let title = common::unique_title(name);
    let app = common::spawn("caret_watch.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    (app, hwnd, outpost)
}

/// A focus change that arrives while a caret key's watch is open is
/// reported first: the worker does not wait for the key's evidence before
/// handling it. The watch then ends with the focus, answered with nothing.
fn a_focus_change_during_a_caret_watch_is_handled_at_once() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-focus");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let to_ok = focus_fact(&mut app, &outpost, hwnd, "ok");
    // Right Arrow, which the application ignores, then the focus moves.
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: right_arrow(&before),
    });
    outpost.deliver(to_ok);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => assert_eq!(node.name.as_deref(), Some("OK")),
        other => panic!("the outpost said {other:?} before the focus on OK"),
    }
    assert_eq!(answer_to(&outpost, key), TextReply::WatchEnded);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A key the application handles after the watch was opened, reporting its
/// caret with an event as it does: the event answers the watch with the
/// caret where the key took it.
fn a_caret_move_raised_later_answers_the_watch() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-later");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: right_arrow(&before),
    });
    // The watch is open: nothing is said, and the worker has finished.
    outpost.settled();
    app.send("caret doc 1");
    app.send("caret-event doc");
    let reply = caret_reply(answer_to(&outpost, key));
    assert!(reply.moved);
    assert_eq!(reply.caret.line.text, "alpha beta\n");
    assert_eq!(reply.caret.line.offset, 1);
    assert_eq!(reply.unit.expect("the character").text, "l");
    assert_eq!(reply.selection_changes, []);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A key that moves nothing is answered with nothing: its watch stays open
/// without the worker waiting, a caret event that shows no change leaves it
/// open, and the next key's watch replaces it, ending it with nothing to
/// say.
fn a_key_that_moves_nothing_is_answered_with_nothing() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-nothing");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let first = outpost.ask(Query::Text {
        node_id: notes,
        op: right_arrow(&before),
    });
    outpost.settled();
    // The application's late report of a caret that has not moved.
    app.send("caret-event doc");
    outpost.settled();
    let second = outpost.ask(Query::Text {
        node_id: notes,
        op: right_arrow(&before),
    });
    assert_eq!(answer_to(&outpost, first), TextReply::WatchEnded);
    outpost.settled();
    // The second key's watch is open in its turn.
    app.send("caret doc 1");
    app.send("caret-event doc");
    assert!(caret_reply(answer_to(&outpost, second)).moved);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Windows 11 Notepad, starting up, once answered that its document had
/// no text pattern (`phase6-design.md`, "Live caret event checks"), and
/// every caret key in it was silent from then on. A provider that fails the
/// request reaches the client the same way, as no pattern, so the focus is
/// reported as having no text, and Core speaks its value, as NVDA does for
/// an object without text. That answer is not kept past the evidence that
/// it is out of date: a caret key's watch on the document stays open, and
/// once the provider answers, the application's caret event, which only an
/// element with text raises, has the text read again and answers the key.
fn a_text_pattern_missing_at_the_focus_is_read_at_the_next_caret_event() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-not-ready");
    app.send("refuse-text on");
    let fact = focus_fact(&mut app, &outpost, hwnd, "doc");
    outpost.deliver(fact);
    let notes = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => node.id,
        other => panic!("the outpost said {other:?}, not the document's focus"),
    };
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::NoText { node_id },
            ..
        } if node_id == notes => {}
        other => panic!("the outpost said {other:?}, not that the document has no text"),
    }
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::AwaitCaret(CaretWatch {
            since: None,
            pressed_at_ms: 0,
            unit: TextUnit::Character,
            compare: None,
            previous_selection: None,
        }),
    });
    // The watch is open: nothing is said, and the worker has finished.
    outpost.settled();

    app.send("refuse-text off");
    app.send("caret doc 1");
    app.send("caret-event doc");
    let reply = caret_reply(answer_to(&outpost, key));
    assert!(reply.moved);
    assert_eq!(
        (reply.caret.line.text.as_str(), reply.caret.line.offset),
        ("alpha beta\n", 1)
    );
    assert_eq!(reply.unit.expect("the character").text, "l");
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_focus_change_during_a_caret_watch_is_handled_at_once",
            a_focus_change_during_a_caret_watch_is_handled_at_once,
        ),
        (
            "a_caret_move_raised_later_answers_the_watch",
            a_caret_move_raised_later_answers_the_watch,
        ),
        (
            "a_key_that_moves_nothing_is_answered_with_nothing",
            a_key_that_moves_nothing_is_answered_with_nothing,
        ),
        (
            "a_text_pattern_missing_at_the_focus_is_read_at_the_next_caret_event",
            a_text_pattern_missing_at_the_focus_is_read_at_the_next_caret_event,
        ),
    ]);
}
