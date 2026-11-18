//! An outpost's clean shutdown, against mockapp's UIA provider: told to
//! shut down while a read into mockapp is in progress, a remote operation
//! in one test and classic reads in the other, the outpost lets the read
//! finish and drops its answer, as the design's worker table has it
//! (`docs/design/readers-tell-core-asks-build1.md`, Table 14, "Shutdown"),
//! then removes every UIA event handler it
//! registered, which mockapp sees through the registrations UIA reports
//! to its fragment root (`IRawElementProviderAdviseEvents`), and closes
//! its pipe. mockapp keeps running and answers another client afterwards.
//! While a read is held, a ping is still answered: the outpost's I/O
//! thread writes the pong itself (`docs/design/communication.md`, section
//! 6.8), so a worker stuck in a call never makes the outpost look dead.
//!
//! mockapp's `hold` command holds the read: the provider call it makes
//! first waits, and mockapp says which call it is, until the test releases
//! it, so the shutdown is asked for while the call is in progress, never
//! before it starts or after it ends.
//!
//! The outpost's observation point at the start of shutdown's wait for the
//! calls in progress (`verbatim_faults::Fault::ShutdownWaitsForCalls`)
//! tells a test that the shutdown has reached that wait, so the binary
//! needs the `faults` feature and only `cargo xtask faults` builds it.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so the
//! only registrations on mockapp's window are the outpost's.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::mpsc::{self, RecvTimeoutError};

use verbatim_faults::Fault;
use verbatim_model::protocol::{CoreToOutpost, OutpostToCore};
use verbatim_model::request::{Answer, Follow, FollowGroup, Followed, Outcome, Reply, Request};
use verbatim_model::{CallCounts, NodeId, TextOp, TextPoint, TextRead, TextReply, TextUnit};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::{Node, Shape, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    UIA_ActiveTextPositionChangedEventId, UIA_AutomationPropertyChangedEventId,
    UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId,
};

use common::outpost::OutpostUnderTest;

/// The events an outpost may register for on a focused document: its name,
/// value, and state changes and its caret changes, the base group's, and
/// its text and active text position changes, which the extended group
/// adds.
const EVENTS: [i32; 4] = [
    UIA_AutomationPropertyChangedEventId.0,
    UIA_Text_TextSelectionChangedEventId.0,
    UIA_Text_TextChangedEventId.0,
    UIA_ActiveTextPositionChangedEventId.0,
];

/// The live registrations on mockapp's window for each of [`EVENTS`].
fn registrations(hwnd: HWND) -> Vec<i64> {
    EVENTS
        .iter()
        .map(|&event| common::advised(hwnd, event))
        .collect()
}

/// The element mockapp reports as having the keyboard focus, built with the
/// listener's cache request, as a focus event's element arrives.
fn focused(hwnd: HWND) -> verbatim_uia::Cached<verbatim_uia::ListenerEvent> {
    let uia = Uia::new().expect("a UIA client");
    common::listener_focus(&uia, hwnd.0 as isize)
}

/// Focuses mockapp's "Notes" document and returns its node: the focus
/// read as Core reads it, then Core's `Follow` of the committed document
/// with the base group, as Core names it for a text field other than
/// Word's document, which must answer applied, as Core sends it at
/// the commit.
fn focus_notes(app: &mut common::MockApp, outpost: &OutpostUnderTest, hwnd: HWND) -> NodeId {
    app.send("set-focus doc");
    let focus = outpost.uia_focus_answer(&focused(hwnd));
    assert_eq!(focus.name(), Some("Notes"));
    let node = focus.node();
    let followed = outpost.requested(
        hwnd,
        Request::Follow(Follow {
            node: Some(Followed {
                node,
                group: FollowGroup::Base,
                extra: None,
            }),
        }),
    );
    assert_eq!(followed.outcome, Outcome::Result(Reply::Followed));
    outpost.settled();
    node
}

/// Core's read of the line at the caret of `node`.
fn read_line(node: NodeId) -> Request {
    Request::Text {
        node,
        op: TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
        }),
    }
}

/// The line `answer` read, which must be a read of the line at the caret
/// that did not move.
fn line_of(answer: Answer) -> (i32, String) {
    match answer.outcome {
        Outcome::Result(Reply::Text(TextReply::Read { moved, chunk })) => (moved, chunk.text),
        other => panic!("the outpost answered {other:?}, not the read's line"),
    }
}

/// What a test reads, and what it expects of the read.
struct Read {
    /// Whether the outpost may use remote operations.
    remote: bool,
    /// The provider call the read makes first, which mockapp holds.
    first_call: &'static str,
    /// The cross-process calls the read makes.
    calls: CallCounts,
}

/// The scenario both tests run: the outpost is told to shut down while
/// `read`'s first provider call is held, and must finish the read, drop its
/// answer, remove its handlers, and close its pipe, leaving mockapp
/// running.
fn shut_down_during(name: &str, read: &Read) {
    common::init_com();
    let title = common::unique_title(name);
    let mut app = common::spawn_counting_registrations("caret_watch.json", &title);
    let hwnd = common::find_window(&title);
    assert_eq!(
        registrations(hwnd),
        [0, 0, 0, 0],
        "nothing has registered on mockapp's window before the outpost"
    );

    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: read.remote,
        },
    );
    let notes = focus_notes(&mut app, &outpost, hwnd);
    assert_eq!(
        registrations(hwnd),
        [1, 1, 0, 0],
        "the outpost's handlers of the base group follow the focused document"
    );

    app.hold();
    let request = outpost.request(hwnd, read_line(notes));
    assert_eq!(
        app.held(),
        read.first_call,
        "the read's first provider call is in progress"
    );

    // The shutdown is asked for while the call is held, on a thread of its
    // own, since it waits for the call. Once it has begun to wait for the
    // calls in progress, the held call keeps it there.
    let pid = app.pid();
    let waiting = verbatim_faults::arm(
        Fault::ShutdownWaitsForCalls,
        move |target: &u32| *target == pid,
        1,
    );
    let messages = outpost.messages;
    let request_calls = outpost.request_calls;
    let under_shutdown = outpost.outpost;
    let (shut_down_tx, shut_down) = mpsc::channel();
    std::thread::spawn(move || {
        under_shutdown.shutdown();
        let _ = shut_down_tx.send(());
    });
    assert!(
        waiting.wait_fired(common::WAIT_TIMEOUT),
        "the shutdown begins to wait for the calls in progress"
    );
    assert_eq!(
        shut_down.try_recv(),
        Err(mpsc::TryRecvError::Empty),
        "the shutdown waits while the call is in progress"
    );
    app.release();

    // The read finished, the shutdown waiting for it, and its answer was
    // dropped (Table 14, "Shutdown" with a program running).
    shut_down
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the shutdown ends once the call has returned");
    assert_eq!(
        request_calls.recv_timeout(common::WAIT_TIMEOUT),
        Ok((request, read.calls)),
        "the read's calls"
    );
    assert_eq!(
        messages.recv_timeout(common::WAIT_TIMEOUT),
        Err(RecvTimeoutError::Disconnected),
        "the outpost said nothing more and closed its pipe"
    );
    // Every handler the outpost registered was removed.
    for event in EVENTS {
        app.await_advised(event, 0);
    }

    // mockapp keeps running, and answers another client.
    let uia = Uia::new().expect("a UIA client");
    let request = uia
        .request::<Node>(Node::all_on())
        .expect("a cache request");
    let root = uia
        .window_element(hwnd.0 as isize, &request)
        .expect("mockapp's root element, read by another client");
    assert_eq!(
        root.read().name.as_deref(),
        Some("Mockapp Caret Watch Fixture")
    );
    app.quit();
}

/// A remote operation in progress finishes before the outpost goes: the
/// read is one round trip, held at its first call inside the provider.
fn a_remote_operation_in_progress_finishes_before_the_outpost_shuts_down() {
    shut_down_during(
        "mockapp-shutdown-remote",
        &Read {
            remote: true,
            first_call: "ProviderOptions",
            calls: CallCounts {
                uia: 1,
                msaa: 0,
                window_messages: 0,
                connections: 0,
            },
        },
    );
}

/// Classic reads in progress finish before the outpost goes: the read is
/// held at its first call, and every call after it is made.
fn a_classic_read_in_progress_finishes_before_the_outpost_shuts_down() {
    shut_down_during(
        "mockapp-shutdown-classic",
        &Read {
            remote: false,
            first_call: "TextGetSelection",
            calls: CallCounts {
                uia: 9,
                msaa: 0,
                window_messages: 0,
                connections: 0,
            },
        },
    );
}

/// A ping handed to the outpost, as its I/O thread hands each command on,
/// while its worker is held in a call into mockapp is answered at once,
/// and the held read still answers once it is released.
fn a_ping_is_answered_while_the_worker_is_held_in_a_call() {
    common::init_com();
    let title = common::unique_title("mockapp-ping-held");
    let mut app = common::spawn_counting_registrations("caret_watch.json", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: false,
        },
    );
    let notes = focus_notes(&mut app, &outpost, hwnd);

    app.hold();
    let request = outpost.request(hwnd, read_line(notes));
    assert_eq!(
        app.held(),
        "TextGetSelection",
        "the worker is in the read's first provider call"
    );
    outpost
        .outpost
        .handle_command(&CoreToOutpost::Ping { sent_at_us: 0 });
    match outpost.next() {
        OutpostToCore::Pong {
            parked_count: 0,
            parked_on_hung: 0,
            ..
        } => {}
        other => panic!("the outpost said {other:?}, not the pong"),
    }
    app.release();
    assert_eq!(
        line_of(outpost.answer(request)),
        (0, "alpha beta\n".to_owned())
    );
    outpost.settled();
    app.quit();
}

fn main() {
    harness::run_isolated(&[
        (
            "a_ping_is_answered_while_the_worker_is_held_in_a_call",
            a_ping_is_answered_while_the_worker_is_held_in_a_call,
        ),
        (
            "a_remote_operation_in_progress_finishes_before_the_outpost_shuts_down",
            a_remote_operation_in_progress_finishes_before_the_outpost_shuts_down,
        ),
        (
            "a_classic_read_in_progress_finishes_before_the_outpost_shuts_down",
            a_classic_read_in_progress_finishes_before_the_outpost_shuts_down,
        ),
    ]);
}
