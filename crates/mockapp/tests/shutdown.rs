//! An outpost's clean shutdown, against mockapp's UIA provider: told to
//! shut down while a read into mockapp is in progress, a remote operation
//! in one test and classic reads in the other, the outpost lets the read
//! finish and sends its answer, then removes every UIA event handler it
//! registered, which mockapp sees through the registrations UIA reports
//! to its fragment root (`IRawElementProviderAdviseEvents`), and closes
//! its pipe. mockapp keeps running and answers another client afterwards.
//!
//! mockapp's `hold` command holds the read: the provider call it makes
//! first waits, and mockapp says which call it is, until the test releases
//! it, so the shutdown is asked for while the call is in progress, never
//! before it starts or after it ends.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so the
//! only registrations on mockapp's window are the outpost's.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::mpsc::{self, RecvTimeoutError};

use verbatim_model::{
    CallCounts, NodeId, NormalizedEvent, TextOp, TextPoint, TextRead, TextReply, TextUnit,
};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::listener::uia_focus_fact;
use verbatim_outpost::protocol::{
    ListenerFact, OutpostToSupervisor, Query, QueryOutcome, QueryResult,
};
use verbatim_uia::{ElementExt, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_ActiveTextPositionChangedEventId,
    UIA_AutomationPropertyChangedEventId, UIA_HasKeyboardFocusPropertyId, UIA_NamePropertyId,
    UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId,
};

use common::outpost::OutpostUnderTest;

/// The events an outpost registers for on a focused document: its name,
/// value, and state changes, and its caret, text, and active text position
/// changes.
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

/// Focuses mockapp's "Notes" document and returns its node, once the
/// outpost has reported the focus and its caret, which must be all it
/// said.
fn focus_notes(app: &mut common::MockApp, outpost: &OutpostUnderTest, hwnd: HWND) -> NodeId {
    app.send("set-focus doc");
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
        other => panic!("the outpost said {other:?}, not the document's focus"),
    };
    assert_eq!(node.name.as_deref(), Some("Notes"));
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, .. },
            ..
        } if node_id == node.id => {}
        other => panic!("the outpost said {other:?}, not the document's caret"),
    }
    outpost.settled();
    node.id
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
/// `read`'s first provider call is held, and must finish the read, send its
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

    let mut outpost = OutpostUnderTest::with_options(
        app.pid(),
        OutpostOptions {
            remote_operations: read.remote,
        },
    );
    let notes = focus_notes(&mut app, &outpost, hwnd);
    assert_eq!(
        registrations(hwnd),
        [1, 1, 1, 1],
        "the outpost's handlers follow the focused document"
    );

    app.hold();
    let request = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
        }),
    });
    assert_eq!(
        app.held(),
        read.first_call,
        "the read's first provider call is in progress"
    );

    // The shutdown is asked for while the call is held, on a thread of its
    // own, since it waits for the call.
    let messages = outpost.messages;
    let under_shutdown = outpost.outpost;
    let (shut_down_tx, shut_down) = mpsc::channel();
    std::thread::spawn(move || {
        under_shutdown.shutdown();
        let _ = shut_down_tx.send(());
    });
    assert_eq!(
        shut_down.try_recv(),
        Err(mpsc::TryRecvError::Empty),
        "the shutdown does not end while the call is in progress"
    );
    app.release();

    // The read finished, and its answer was sent before the pipe closed.
    match messages.recv_timeout(common::WAIT_TIMEOUT) {
        Ok(OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Read { moved, chunk })),
            timing,
            ..
        }) if request_id == request => {
            assert_eq!((moved, chunk.text.as_str()), (0, "alpha beta\n"));
            assert_eq!(timing.calls, read.calls, "the read's calls");
        }
        other => panic!("the outpost said {other:?}, not the read's answer"),
    }
    shut_down
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the shutdown ends once the call has returned");
    assert_eq!(
        messages.recv_timeout(common::WAIT_TIMEOUT),
        Err(RecvTimeoutError::Disconnected),
        "the outpost said nothing more and closed its pipe"
    );
    assert_eq!(
        registrations(hwnd),
        [0, 0, 0, 0],
        "every handler the outpost registered was removed"
    );

    // mockapp keeps running, and answers another client.
    let uia = Uia::new().expect("a UIA client");
    let cache = uia.base_cache_request().expect("a cache request");
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("mockapp's root element, read by another client");
    assert_eq!(
        root.cached_string(UIA_NamePropertyId).as_deref(),
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
            },
        },
    );
}

fn main() {
    harness::run_isolated(&[
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
