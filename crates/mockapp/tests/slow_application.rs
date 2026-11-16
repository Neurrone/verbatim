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
//!   the outpost's frozen clock moved past a limit that ran on it, or the
//!   end of a stall longer than a limit that ran in real time, so each test fails with its old
//!   limit restored.
//! - The shell's items are answered complete or not at all: a walk whose
//!   window's lookup UIA ends with its timeout fails whole.
//!
//! A window that hangs while the outpost waits on it is Core's to judge,
//! by its hang check (`docs/design/readers-tell-core-asks-build1.md`), and
//! is not tested here.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use verbatim_ia2::{WinEvent, WinEventHook, WinEventKind};
use verbatim_model::protocol::{OutpostToCore, Query, QueryOutcome, QueryResult, RequestId};
use verbatim_model::request::{MsaaIds, Outcome, Reply, Request, WindowVerdict};
use verbatim_model::{CallCounts, NodeId, QueryKind, Role, ShellItem, ShellItemKind, WindowHandle};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::Uia;

use common::outpost::OutpostUnderTest;

/// Longer than the old focus deadline, well under NVDA's ten seconds.
const STALL: Duration = Duration::from_millis(2500);

fn a_focus_read_that_waits_on_a_busy_application_is_still_reported() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-focus");
    let mut app = common::spawn("small.json", "msaa", &title);
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
    let outpost = OutpostUnderTest::new(&app);

    // The window thread is stalled before Core's request arrives.
    app.stall(STALL);
    let focused = outpost.msaa_focus_answer(MsaaIds {
        window: WindowHandle(u64::try_from(hwnd).expect("a window handle")),
        object: id_object,
        child: id_child,
    });
    let answered_at = common::outpost::now_us();
    assert_eq!(focused.name(), Some("Original Name"));
    // The answer came after the stall ended: the read waited on the stalled
    // window thread for the whole stall, longer than the old deadline.
    let ended = app.stall_ended(STALL);
    assert!(
        answered_at >= ended,
        "the focus was answered at {answered_at} us, before the stall ended at {ended} us"
    );
    drop(outpost);
    app.quit();
}

/// The text of mockapp's dialog fixture's dialog, its question's name and
/// description.
const QUESTION: &str = "Remove the theme Mine? This cannot be undone. Its sounds go with it.";

/// The node of the focus's ancestor named `name`, from its answer.
fn ancestor_named(focused: &common::outpost::Focused, name: &str) -> NodeId {
    focused
        .read
        .ancestors
        .iter()
        .find(|ancestor| common::outpost::name_of(&ancestor.node) == Some(name))
        .and_then(|ancestor| ancestor.node.read.node)
        .unwrap_or_else(|| panic!("the focus's chain has no {name:?}"))
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

/// A navigation step asked while mockapp's window thread is stalled for
/// twice the step's old limit of 400 milliseconds, which ran on the
/// outpost's reducer's clock in real time, answers its neighbor once
/// mockapp answers: the step has the handling deadline. The stall's end,
/// which mockapp prints, is the evidence that the old limit passed.
fn a_slow_navigation_step_answers_its_neighbor() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-step");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let mut outpost = OutpostUnderTest::new(&app);
    app.send("set-focus notes");
    let focused = outpost.uia_focus_answer(&common::listener_focus(&uia, hwnd.0 as isize));
    let group = ancestor_named(&focused, "Inner group");

    let stall = Duration::from_millis(800);
    app.stall(stall);
    let request_id = outpost.ask(Query::Navigate {
        node_id: group,
        kind: QueryKind::FirstChild,
        details: verbatim_model::property::NodeDetailSet::ALL,
    });
    match reply_to(&outpost, request_id) {
        QueryOutcome::Done(QueryResult::Navigated(Some(neighbor))) => {
            assert_eq!(neighbor.name.as_deref(), Some("Deep button"));
        }
        other => panic!("the step answered {other:?}, not its neighbor"),
    }
    app.stall_ended(stall);
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
    let focused = outpost.uia_focus_answer(&common::listener_focus(&uia, hwnd.0 as isize));
    let group = ancestor_named(&focused, "Inner group");

    outpost.run_out_of_time_after(2);
    let (neighbor, _) = outpost.navigate(group, QueryKind::FirstChild);
    assert_eq!(neighbor.name.as_deref(), Some("Deep button"));
    drop(outpost);
    app.quit();
}

/// A UIA dialog the focus enters, a top-level window of a dialog's class,
/// whose text gathering waits on mockapp's window thread stalled for longer
/// than the bounded client's old wait of a second, real time, is reported
/// with its text once mockapp answers: the text is gathered inside the
/// focus's program, on the worker's own client, whose calls the report
/// counts. The stall ends well within UIA's connection timeout, which ends
/// the lookup of the dialog's window from the timeout's own length to about
/// 200 milliseconds past it, so a stall that long can lose the text.
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
    let _ = outpost.uia_focus_answer(&common::listener_focus(&uia, hwnd.0 as isize));
    outpost.forget_focus();
    outpost.settle();
    app.send("set-focus no");
    let (focus, _) = outpost.listener_focus(&common::listener_focus(&uia, hwnd.0 as isize));

    let stall = verbatim_uia::UIA_CONNECTION_TIMEOUT * 3 / 4;
    app.stall(stall);
    let (outcome, calls) = outpost.complete_focus(focus);
    app.stall_ended(stall);
    let focused = common::outpost::Focused::of(outcome, calls);
    assert_eq!(focused.name(), Some("No"));
    let dialog = focused
        .read
        .ancestors
        .iter()
        .find(|ancestor| common::outpost::role_of(&ancestor.node) == Role::Dialog)
        .unwrap_or_else(|| {
            panic!(
                "the focus's chain has no dialog: {:?}",
                focused.read.ancestors
            )
        });
    assert_eq!(dialog.dialog_text.as_deref(), Some(QUESTION));
    // Three calls: the dialog window's element, the walk down the route to
    // the focus, and one program reading the focus's states with the
    // dialog's children. The old report made two, but never registered
    // for the focus's events before reading its states; Dickson's ruling
    // has the outpost register first, as NVDA's `event_gainFocus`
    // registers before speaking (`NVDAObjects/UIA/__init__.py` 1623 to
    // 1626), so the states and the dialog's text are read after it.
    assert_eq!(
        focused.calls,
        CallCounts {
            uia: 3,
            msaa: 0,
            window_messages: 0,
            connections: 1,
        },
        "the dialog's calls, counted on the worker's thread"
    );
    drop(outpost);
    app.quit();
}

/// The taskbar's items, asked as Core asks them from outside the event
/// queue of the outpost of a mockapp window of the taskbar's class, its
/// verdict in force, whose walk mockapp holds while the outpost's frozen
/// clock passes the walk's old budget of two seconds, are answered in full
/// once mockapp answers: the walk has no budget of its own. The window has
/// no frame, as the real taskbar has none, so the walk meets no title bar,
/// whose buttons' reads wait on the Desktop Window Manager for seconds
/// under a parallel run and past UIA's connection timeout come back as no
/// element.
fn a_slow_shell_items_walk_answers_the_whole_list() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-taskbar");
    let mut app = common::spawn("taskbar.json", "uia", &title);
    let window = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    app.hold();
    let number = outpost.request_with(
        window,
        WindowVerdict::Uia,
        Request::ShellItems {
            kind: ShellItemKind::Taskbar,
        },
    );
    app.held();
    outpost.pass_time(Duration::from_secs(3));
    app.release();
    let names = match outpost.answer(number).outcome {
        Outcome::Result(Reply::ShellItems(items)) => items
            .into_iter()
            .map(|ShellItem { name, .. }| name)
            .collect::<Vec<_>>(),
        other => panic!("the request answered {other:?}, not the taskbar's items"),
    };
    assert_eq!(names, ["Mail", "Files", "Music"]);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The taskbar's items, asked as Core asks them of the outpost of a
/// mockapp window of the taskbar's class, its verdict in force, while
/// mockapp's window thread is stalled past UIA's connection timeout, which
/// ends the window's lookup with UIA's timeout error: the request fails
/// whole, answered "ended or stopped", rather than answering the buttons
/// read by then, none, as the list. The stall's end, which mockapp prints, is the
/// evidence that the timeout passed; it is one and a half times the
/// timeout, under the five seconds after which Windows judges the window
/// hung.
fn a_shell_items_walk_whose_lookup_times_out_fails_whole() {
    common::init_com();
    let title = common::unique_title("mockapp-timed-out-taskbar");
    let mut app = common::spawn("taskbar.json", "uia", &title);
    let window = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    let stall = verbatim_uia::UIA_CONNECTION_TIMEOUT * 3 / 2;
    app.stall(stall);
    let number = outpost.request_with(
        window,
        WindowVerdict::Uia,
        Request::ShellItems {
            kind: ShellItemKind::Taskbar,
        },
    );
    app.stall_ended(stall);
    assert_eq!(outpost.answer(number).outcome, Outcome::Ended);
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
        (
            "a_shell_items_walk_whose_lookup_times_out_fails_whole",
            a_shell_items_walk_whose_lookup_times_out_fails_whole,
        ),
    ]);
}
