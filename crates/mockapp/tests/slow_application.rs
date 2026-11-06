//! An application that is slow to answer is waited for, as NVDA waits,
//! rather than cut off by a short limit of Verbatim's own: an outpost
//! watching `mockapp` is handed work while mockapp's window thread is
//! stalled or holds a call, and its answer comes once mockapp answers.
//!
//! - A focus read that waits on a stalled window is still reported. Found
//!   live in the end-to-end suite: Notepad starting up and the Start menu's
//!   search window took two to three seconds to answer reads that then
//!   succeeded, the outpost abandoned them at its old 1.5 second deadline,
//!   and the focus was never announced.
//! - A navigation step, a walk down a focus's path, a dialog's text and the
//!   shell's items each wait past the short limit they once had, and answer
//!   in full. Each hold ends after evidence that the old limit has passed:
//!   the outpost's frozen clock moved past a limit that ran on it, or twice
//!   a limit that ran in real time, so each test fails with its old limit
//!   restored.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::io::BufReader;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use verbatim_ia2::{WinEvent, WinEventHook, WinEventKind};
use verbatim_model::protocol::{
    CoreToOutpost, DeliveredFact, OutpostToCore, Query, QueryOutcome, QueryResult, RequestId,
};
use verbatim_model::{
    CallCounts, NormalizedEvent, QueryKind, Role, ShellItem, ShellItemKind, TraceId,
};
use verbatim_outpost::Outpost;
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::Uia;

use common::outpost::OutpostUnderTest;
use verbatim_process::message::read_message;
use verbatim_process::overlapped::{End, Flow, pipe};
use verbatim_process::transport::{BlockingReader, EVENT_PIPE_BUFFER};

/// Longer than the old focus deadline, well under NVDA's ten seconds.
const STALL: Duration = Duration::from_millis(2500);

fn a_focus_read_that_waits_on_a_busy_application_is_still_reported() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-focus");
    let mut app = common::spawn("small.json", "msaa", &title);
    let window = common::find_window(&title).0 as isize;
    let pid = app.pid();

    // The focus event's real MSAA address, captured from mockapp itself.
    let focus = Arc::new(Mutex::new(None::<(isize, i32, i32)>));
    let seen = Arc::clone(&focus);
    let _hook = WinEventHook::install(
        pid,
        &[WinEventKind::Focus],
        Box::new(move |event: WinEvent| {
            *seen.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((event.hwnd, event.id_object, event.id_child));
        }),
    )
    .expect("WinEventHook::install");
    app.send("focus btn1");
    let (hwnd, id_object, id_child) =
        common::pump_until(|| *focus.lock().unwrap_or_else(PoisonError::into_inner));

    let (pipe_in, pipe_out) =
        pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped).expect("a pipe");
    let outpost = Outpost::new(pipe_out, pid);
    // No window takes the foreground on the desktop the test runs on:
    // mockapp's is the foreground, so its focus passes the acceptance test.
    outpost.set_foreground_reader(Arc::new(move || window));
    let (messages_tx, messages) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(BlockingReader::new(pipe_in).expect("a reader"));
        while let Ok(Some(message)) = read_message::<_, OutpostToCore>(&mut reader) {
            if messages_tx.send(message).is_err() {
                return;
            }
        }
    });
    assert_eq!(
        messages.recv_timeout(common::WAIT_TIMEOUT).ok(),
        Some(OutpostToCore::Ready {
            outpost_pid: verbatim_model::Pid(std::process::id()),
            target_pid: verbatim_model::Pid(pid),
        }),
        "the outpost announces itself first"
    );

    // The window thread is stalled before the fact arrives.
    app.stall(STALL);
    outpost.handle_command(&CoreToOutpost::DeliverFact {
        trace_id: TraceId::mint(),
        timing: verbatim_model::protocol::EventTiming::default(),
        fact: DeliveredFact::MsaaFocus {
            hwnd,
            id_object,
            id_child,
        },
        win_event: None,
    });

    let message = messages
        .recv_timeout(STALL + common::WAIT_TIMEOUT)
        .expect("the outpost reports the focus before the wait times out");
    let OutpostToCore::Event {
        event: NormalizedEvent::FocusChanged { node, .. },
        timing,
        ..
    } = message
    else {
        panic!("the outpost said {message:?}, not the focus");
    };
    assert_eq!(node.name.as_deref(), Some("Original Name"));
    // The outpost's worker took the fact, and began reading, before the
    // stall ended, and reported the focus after it: the read waited on the
    // stalled window thread for the whole stall, longer than the old
    // deadline, and the test exercised a slow read.
    let ended = app.stall_ended(STALL);
    assert!(
        timing.dequeued_at_us < ended,
        "the read began at {} us, after the stall ended at {ended} us",
        timing.dequeued_at_us
    );
    assert!(
        timing.published_at_us >= ended,
        "the focus was reported at {} us, before the stall ended at {ended} us",
        timing.published_at_us
    );
    app.quit();
}

/// The text of mockapp's dialog fixture's dialog, its question's name and
/// description.
const QUESTION: &str = "Remove the theme Mine? This cannot be undone. Its sounds go with it.";

/// Waits until `deadline` has come in real time: the evidence a test waits
/// for when the limit it shows gone ran in real time, twice that limit
/// after the work began, so the test fails with the limit restored.
fn wait_until(deadline: Instant) {
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
}

/// The outcome of query `request_id`, the outpost's next message.
fn reply_to(outpost: &OutpostUnderTest, request_id: RequestId) -> QueryOutcome {
    match outpost.next() {
        OutpostToCore::Reply {
            request_id: answered,
            outcome,
            ..
        } if answered == request_id => outcome,
        other => panic!("the outpost said {other:?}, not the query's answer"),
    }
}

/// A navigation step whose call mockapp holds past twice the step's old
/// limit of 400 milliseconds, which ran on the outpost's reducer's clock in
/// real time, answers its neighbor once mockapp answers: the step has the
/// handling deadline.
fn a_slow_navigation_step_answers_its_neighbor() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-step");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let mut outpost = OutpostUnderTest::new(&app);
    app.send("set-focus notes");
    let reported = outpost.uia_focus(&common::listener_focus(&uia, hwnd.0 as isize));
    let group = reported
        .ancestors
        .iter()
        .find(|ancestor| ancestor.name.as_deref() == Some("Inner group"))
        .expect("the focus's chain has the inner group")
        .id;

    app.hold();
    let asked = Instant::now();
    let request_id = outpost.ask(Query::Navigate {
        node_id: group,
        kind: QueryKind::FirstChild,
    });
    app.held();
    wait_until(asked + Duration::from_millis(800));
    app.release();
    match reply_to(&outpost, request_id) {
        QueryOutcome::Done(QueryResult::Navigated(Some(neighbor))) => {
            assert_eq!(neighbor.name.as_deref(), Some("Deep button"));
        }
        other => panic!("the step answered {other:?}, not its neighbor"),
    }
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A query on an ancestor of the focus whose element the outpost does not
/// hold finds it by the classic walk down the focus's path; a walk during
/// which the outpost's frozen clock passes the path's old budget of a
/// second answers its node all the same. The time jumps an hour on at the
/// outpost's third reading of it after the query is asked, the first
/// between the walk's calls where the old budget was asked, its entry
/// reading it first and the old budget's start second.
fn a_slow_path_walk_answers_its_node() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-path");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let mut outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: false,
        },
    );
    app.send("set-focus notes");
    let reported = outpost.uia_focus(&common::listener_focus(&uia, hwnd.0 as isize));
    let group = reported
        .ancestors
        .iter()
        .find(|ancestor| ancestor.name.as_deref() == Some("Inner group"))
        .expect("the focus's chain has the inner group")
        .id;

    outpost.run_out_of_time_after(2);
    let (neighbor, _) = outpost.navigate(group, QueryKind::FirstChild);
    assert_eq!(neighbor.name.as_deref(), Some("Deep button"));
    drop(outpost);
    app.quit();
}

/// A UIA dialog the focus enters, a top-level window of a dialog's class,
/// whose text gathering mockapp holds past
/// twice the bounded client's old wait of a second, real time, is reported
/// with its text once mockapp answers: the text is gathered inside the
/// focus's program, on the worker's own client, whose calls the report
/// counts.
fn a_slow_dialog_text_is_reported_with_the_focus() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-dialog");
    let mut app = common::spawn("dialog_class_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let outpost = OutpostUnderTest::new(&app);
    // A first focus in the dialog commits the chain the listener read, by
    // which the outpost finds the dialog's element for the next focus; Core
    // then holds none of the outpost's nodes, as when it moved to another
    // application, so the next focus enters the dialog anew.
    app.send("set-focus yes");
    outpost.uia_focus(&common::listener_focus(&uia, hwnd.0 as isize));
    outpost.release_all();
    outpost.settle();
    app.send("set-focus no");
    let fact = outpost.listener_fact(&common::listener_focus(&uia, hwnd.0 as isize));

    app.hold();
    let delivered = Instant::now();
    outpost.deliver(fact);
    app.held();
    wait_until(delivered + Duration::from_secs(2));
    app.release();
    let reported = outpost.reported_focus();
    assert_eq!(reported.node.name.as_deref(), Some("No"));
    let dialog = reported
        .ancestors
        .iter()
        .find(|ancestor| ancestor.role == Role::Dialog)
        .unwrap_or_else(|| panic!("the focus's chain has no dialog: {:?}", reported.ancestors));
    assert_eq!(
        dialog.details.description.as_deref(),
        Some(QUESTION),
        "{:?}",
        reported.ancestors
    );
    assert_eq!(
        reported.calls,
        CallCounts {
            uia: 2,
            msaa: 0,
            window_messages: 0,
            connections: 1,
        },
        "the dialog's calls, counted on the worker's thread"
    );
    drop(outpost);
    app.quit();
}

/// The taskbar's items, asked of the outpost of a mockapp window of the
/// taskbar's class, whose walk mockapp holds while the outpost's frozen
/// clock passes the walk's old budget of two seconds, are answered in full
/// once mockapp answers: the walk has no budget of its own.
fn a_slow_shell_items_walk_answers_the_whole_list() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-taskbar");
    let mut app = common::spawn("taskbar.json", "uia", &title);
    let mut outpost = OutpostUnderTest::new(&app);

    app.hold();
    let answer_by_us = common::now_us()
        + u64::try_from(verbatim_model::HANDLING_DEADLINE.as_micros()).expect("small");
    let request_id = outpost.ask(Query::ShellItems {
        kind: ShellItemKind::Taskbar,
        answer_by_us,
    });
    app.held();
    outpost.pass_time(Duration::from_secs(3));
    app.release();
    let names = match reply_to(&outpost, request_id) {
        QueryOutcome::Done(QueryResult::ShellItems(items)) => items
            .into_iter()
            .map(|ShellItem { name, .. }| name)
            .collect::<Vec<_>>(),
        other => panic!("the query answered {other:?}, not the taskbar's items"),
    };
    assert_eq!(
        names,
        ["Minimize", "Maximize", "Close", "Mail", "Files", "Music"]
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_focus_read_that_waits_on_a_busy_application_is_still_reported",
            a_focus_read_that_waits_on_a_busy_application_is_still_reported,
        ),
        (
            "a_slow_navigation_step_answers_its_neighbor",
            a_slow_navigation_step_answers_its_neighbor,
        ),
        (
            "a_slow_path_walk_answers_its_node",
            a_slow_path_walk_answers_its_node,
        ),
        (
            "a_slow_dialog_text_is_reported_with_the_focus",
            a_slow_dialog_text_is_reported_with_the_focus,
        ),
        (
            "a_slow_shell_items_walk_answers_the_whole_list",
            a_slow_shell_items_walk_answers_the_whole_list,
        ),
    ]);
}
