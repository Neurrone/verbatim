//! The outpost's part of the harness's idle barrier
//! (`docs/design/communication.md`, section 6.7) in a real outpost,
//! against mockapp's UIA text provider: the answer to an idle query goes
//! behind every message the outpost sent before it, one whose write is in
//! flight and one waiting in its backlog included.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::protocol::{
    CoreToOutpost, OutpostToCore, Query, QueryOutcome, QueryResult, RequestId,
};
use verbatim_model::request::FocusCaret;
use verbatim_model::{NodeId, TextOp, TextPoint, TextReply};
use verbatim_process::arguments::OutpostOptions;
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
fn whole_text(outpost: &OutpostUnderTest, request: RequestId) {
    match outpost.next() {
        OutpostToCore::Reply {
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
    let notes = notes_focus(&mut app, &outpost, hwnd);
    let read = TextOp::ReadRange {
        start: TextPoint::Start,
        end: TextPoint::End,
        through: None,
    };
    // The worker is held inside the first read's call while the second
    // read and the idle query are handed over.
    app.hold();
    let first = outpost.ask(Query::Text {
        node_id: notes,
        op: read.clone(),
    });
    let _ = app.held();
    let second = outpost.ask(Query::Text {
        node_id: notes,
        op: read,
    });
    outpost
        .outpost
        .handle_command(&CoreToOutpost::IdleQuery { round: 7 });
    app.release();
    // The first read's answer is in flight, the second's and the idle
    // answer wait behind it; only now does Core read.
    whole_text(&outpost, first);
    whole_text(&outpost, second);
    assert_eq!(outpost.next(), OutpostToCore::IdleAnswer { round: 7 });
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Focuses mockapp's "Notes" document, has the outpost complete the
/// listener's read of it as Core asks, and returns its node, whose answer
/// carries its caret.
fn notes_focus(app: &mut common::MockApp, outpost: &OutpostUnderTest, hwnd: HWND) -> NodeId {
    app.send("set-focus doc");
    let uia = Uia::new().expect("a UIA client");
    let element = common::listener_focus(&uia, hwnd.0 as isize);
    let focused = outpost.uia_focus_answer(&element);
    assert_eq!(
        (
            focused.name(),
            matches!(focused.read.caret, FocusCaret::Read(_))
        ),
        (Some("Notes"), true),
        "the document's focus with its caret"
    );
    outpost.settled();
    focused.node()
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "the_answer_goes_behind_a_message_in_the_pipe_and_one_in_the_backlog",
            the_answer_goes_behind_a_message_in_the_pipe_and_one_in_the_backlog,
        ),
    ]);
}
