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
//!   end of a stall of twice a limit that ran in real time, so each test fails with its old limit
//!   restored.
//! - A window that hangs while the outpost waits on it is told to Core once
//!   and waited for: a query, and a listener read's marker
//!   waiting with the hung window in front.
//! - The focus listener's own tests for a hung window end its read: before
//!   a call, whether the read is in NVDA's checks or its walk, and while a
//!   call is blocked, by its watchdog's judgement.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::io::BufReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use verbatim_ia2::{WinEvent, WinEventHook, WinEventKind};
use verbatim_listener_logic::focus::{FocusEvent, HUNG, Judged};
use verbatim_model::protocol::AncestorsEnd;
use verbatim_model::protocol::{
    CoreToOutpost, DeliveredFact, OutpostToCore, Query, QueryOutcome, QueryResult, RequestId,
};
use verbatim_model::{
    CallCounts, NormalizedEvent, QueryKind, Role, ShellItem, ShellItemKind, TraceId,
};
use verbatim_outpost::Outpost;
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::Uia;
use verbatim_win32::window::Readability;

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
    let reported = outpost.uia_focus(&common::listener_focus(&uia, hwnd.0 as isize));
    let group = reported
        .ancestors
        .iter()
        .find(|ancestor| ancestor.name.as_deref() == Some("Inner group"))
        .expect("the focus's chain has the inner group")
        .id;

    let stall = Duration::from_millis(800);
    app.stall(stall);
    let request_id = outpost.ask(Query::Navigate {
        node_id: group,
        kind: QueryKind::FirstChild,
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
/// whose text gathering waits on mockapp's window thread stalled for twice
/// the bounded client's old wait of a second, real time, is reported with
/// its text once mockapp answers: the text is gathered inside the
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

    let stall = Duration::from_secs(2);
    app.stall(stall);
    outpost.deliver(fact);
    let reported = outpost.reported_focus();
    app.stall_ended(stall);
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

/// The "Inner group" ancestor of mockapp's ancestry fixture's "Notes"
/// field, focused and reported through the listener's read, a node with no
/// window of its own: its window is the application's focus window.
fn inner_group(
    app: &mut common::MockApp,
    uia: &Uia,
    hwnd: windows::Win32::Foundation::HWND,
    outpost: &OutpostUnderTest,
) -> verbatim_model::NodeId {
    app.send("set-focus notes");
    outpost
        .uia_focus(&common::listener_focus(uia, hwnd.0 as isize))
        .ancestors
        .iter()
        .find(|ancestor| ancestor.name.as_deref() == Some("Inner group"))
        .expect("the focus's chain has the inner group")
        .id
}

/// The outpost's pong, asked for now: its workers parked on windows not
/// judged hung, and those on windows judged hung.
fn parked(outpost: &OutpostUnderTest, seq: u64) -> (usize, usize) {
    outpost
        .outpost
        .handle_command(&CoreToOutpost::Ping { seq, sent_at_us: 0 });
    match outpost.next() {
        OutpostToCore::Pong {
            seq: answered,
            parked_count,
            parked_on_hung,
            ..
        } if answered == seq => (parked_count, parked_on_hung),
        other => panic!("the outpost said {other:?}, not its pong"),
    }
}

/// A query whose call mockapp holds while the outpost's reader comes to
/// answer its window hung, its window being the application's focus
/// window, tells Core once that the window stopped responding and is not
/// abandoned: the pong counts no worker parked, and once mockapp answers,
/// the query is answered with its neighbor.
fn a_query_whose_window_hangs_during_its_call_is_answered_when_it_answers() {
    common::init_com();
    let title = common::unique_title("mockapp-hung-query");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: false,
        },
    );
    let group = inner_group(&mut app, &uia, hwnd, &outpost);
    let mut outpost = outpost;

    app.hold();
    let request_id = outpost.ask(Query::Navigate {
        node_id: group,
        kind: QueryKind::FirstChild,
    });
    assert_eq!(app.held(), "GetObject", "the window's lookup was held");
    outpost.judge_hung(hwnd);
    assert_eq!(
        outpost.next(),
        OutpostToCore::NotResponding {
            window: hwnd.0 as isize
        },
        "Core is told of the hang"
    );
    assert_eq!(parked(&outpost, 1), (0, 0), "no worker abandoned");
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

/// While a listener read's marker waits, with no program in flight, a
/// stall of mockapp's window thread long enough for Windows to report the
/// window hung is found by the outpost's check of the window in front,
/// mockapp's: Core is told once that it stopped responding, and the focus
/// is reported once the listener's read, made before the stall, is
/// delivered after it ends.
fn a_hang_while_the_listeners_read_waits_is_told_once_and_the_focus_spoken_when_it_answers() {
    common::init_com();
    let title = common::unique_title("mockapp-hung-marker");
    let mut app = common::spawn("small.json", "uia", &title);
    let window = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let outpost = OutpostUnderTest::new(&app);
    app.send("set-focus btn1");
    let fact = outpost.listener_fact(&common::listener_focus(&uia, window.0 as isize));

    let stall = Duration::from_secs(8);
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 0 });
    app.stall(stall);
    assert_eq!(
        outpost.next(),
        OutpostToCore::NotResponding {
            window: window.0 as isize
        },
        "Core is told of the hang"
    );
    app.stall_ended(stall);
    outpost.deliver(fact);
    assert_eq!(
        outpost.reported_focus().node.name.as_deref(),
        Some("Original Name"),
        "the focus, once its read comes"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A query whose call mockapp holds is abandoned once the user has moved
/// on to a window of the same application on another UI thread, which the
/// outpost reads as the window with the keyboard focus: the query is
/// answered abandoned, Core is told of no hang, and the pong counts the
/// worker parked apart from those on hung windows. Released, the worker
/// returns.
fn a_query_whose_user_moved_on_is_abandoned() {
    common::init_com();
    let title = common::unique_title("mockapp-moved-on-query");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let other = app.thread_window();
    let uia = Uia::new().expect("a UIA client");
    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: false,
        },
    );
    let group = inner_group(&mut app, &uia, hwnd, &outpost);
    let mut outpost = outpost;

    app.hold();
    let request_id = outpost.ask(Query::Navigate {
        node_id: group,
        kind: QueryKind::FirstChild,
    });
    // The worker named the host window, the focus window then, before the
    // call held.
    assert_eq!(app.held(), "GetObject", "the window's lookup was held");
    outpost.set_focus_window(other);
    assert_eq!(reply_to(&outpost, request_id), QueryOutcome::Abandoned);
    assert_eq!(
        parked(&outpost, 1),
        (1, 0),
        "one worker parked, not on a hung window"
    );
    app.release();
    // The evidence that the abandoned worker returned: its pong counts
    // none parked.
    let returned_by = std::time::Instant::now() + common::WAIT_TIMEOUT;
    let mut seq = 2;
    while parked(&outpost, seq) != (0, 0) {
        assert!(
            std::time::Instant::now() < returned_by,
            "the abandoned worker did not return"
        );
        seq += 1;
    }
    drop(outpost);
    app.quit();
}

/// A query on a node with no window of its own, reached by navigation off
/// the followed focus's chain, whose application's focus window the
/// outpost's reader answers hung, is refused at its check before its first
/// call: Core is told the window stopped responding, the query is answered
/// abandoned, and no call reaches mockapp.
fn a_query_on_a_windowless_node_of_a_hung_window_is_refused_with_no_call() {
    common::init_com();
    let title = common::unique_title("mockapp-hung-refused");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let mut outpost = OutpostUnderTest::new(&app);
    let group = inner_group(&mut app, &uia, hwnd, &outpost);
    let (deep, _) = outpost.navigate(group, QueryKind::FirstChild);
    assert_eq!(
        deep.name.as_deref(),
        Some("Deep button"),
        "off the focus's chain"
    );

    outpost.judge_hung(hwnd);
    common::reset_hits(hwnd);
    let request_id = outpost.ask(Query::Navigate {
        node_id: deep.id,
        kind: QueryKind::NextSibling,
    });
    assert_eq!(
        outpost.next(),
        OutpostToCore::NotResponding {
            window: hwnd.0 as isize
        }
    );
    match outpost.next() {
        OutpostToCore::Reply {
            request_id: answered,
            outcome: QueryOutcome::Abandoned,
            timing,
            ..
        } if answered == request_id => assert_eq!(timing.calls, CallCounts::default()),
        other => panic!("the outpost said {other:?}, not the refusal"),
    }
    outpost.settled();
    assert_eq!(common::read_hits(hwnd), [], "no call reached mockapp");
    drop(outpost);
    app.quit();
}

/// mockapp's window's own element, the one element of mockapp's with a
/// window of its own, focused, as the listener's registration delivers it
/// in a focus event.
fn window_focus(
    app: &mut common::MockApp,
    uia: &Uia,
    window: isize,
) -> verbatim_uia::Cached<verbatim_uia::ListenerEvent> {
    app.send("set-focus root");
    let request = uia
        .request_fixed::<verbatim_uia::ListenerEvent>()
        .expect("the listener's cache request");
    uia.window_live(window)
        .expect("mockapp's window element")
        .refresh(&request)
        .expect("the window's element as a focus event delivers it")
}

/// A window readability reader that answers `window` hung once `hung` is
/// set, and every other window, and `window` before, by the system's
/// judgement.
fn hung_once_set(
    window: isize,
    hung: &Arc<AtomicBool>,
) -> verbatim_win32::window::WindowReadabilityReader {
    let hung = Arc::clone(hung);
    Arc::new(move |hwnd| {
        if hwnd == window && hung.load(Ordering::SeqCst) {
            Readability::Hung
        } else {
            verbatim_win32::window::readability(hwnd)
        }
    })
}

/// The read mockapp's window's own element is published as from its
/// progress before any of its walk's steps: as the event's cache gives it,
/// no ancestor read, and its chain completed by plain windows above its
/// own window.
fn assert_published_before_its_steps(judged: Judged, window: isize) {
    let Judged::Read(read) = judged else {
        panic!("the read was not published from its progress: {judged:?}");
    };
    assert_eq!(
        (
            read.hwnd,
            read.ancestors.len(),
            read.ancestors_end,
            read.window
        ),
        (
            window,
            0,
            AncestorsEnd::PlainFrom(verbatim_uia::plain_windows_start(window, [window])),
            Some(window)
        ),
        "the focus as read before its walk's first step"
    );
}

/// The listener's read of a focus with a window of its own, on the frozen
/// clocks, so its walk never reaches its bound: its walk's first call, the
/// focus's keyboard focus, is held, and while it is held the listener's
/// reader comes to answer the window hung, as Windows judges a window whose
/// thread has taken no message for five seconds. Once mockapp answers, the
/// read tests its window before its next call, makes no further call, and
/// returns the focus published from its progress, in its marker's place;
/// its reading thread, here the test's, is not abandoned, but returns.
fn the_listeners_read_ends_before_a_call_into_a_window_that_hangs() {
    common::init_com();
    let title = common::unique_title("mockapp-hangs-in-read");
    let mut app = common::spawn("small.json", "uia", &title);
    let window = common::find_window(&title).0 as isize;
    let uia = Uia::new().expect("a UIA client");
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let focus = window_focus(&mut app, &uia, window);
    let hung = Arc::new(AtomicBool::new(false));
    outpost.listener_judges_readability(hung_once_set(window, &hung));

    app.hold();
    let ((judged, calls), held) = std::thread::scope(|scope| {
        let releasing = scope.spawn(|| {
            let held = app.held();
            hung.store(true, Ordering::SeqCst);
            app.release();
            held
        });
        let read = outpost.listener_read(&focus);
        (read, releasing.join().expect("the held call released"))
    });
    assert_eq!(
        held, "GetPropertyValue",
        "the focus's keyboard focus was held"
    );
    assert_eq!(
        calls,
        CallCounts {
            uia: 1,
            msaa: 0,
            window_messages: 0,
            connections: 0,
        },
        "the held call was the read's last"
    );
    assert_published_before_its_steps(judged, window);
    drop(outpost);
    app.quit();
}

/// The listener's read of a focus with no window of its own, on the frozen
/// clocks: its first call looks up its nearest window, mockapp's, which the
/// listener's reader answers hung, so its test before the next call, made
/// while the read is still in NVDA's checks, drops the event, for that
/// reason, with that lookup its only call. The drop takes its marker's
/// place at the outpost, which releases the focus read waiting behind it.
fn the_listeners_read_in_its_checks_ends_before_a_later_call() {
    common::init_com();
    let title = common::unique_title("mockapp-hangs-in-checks");
    let mut app = common::spawn("small.json", "uia", &title);
    let window = common::find_window(&title).0 as isize;
    let uia = Uia::new().expect("a UIA client");
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    app.send("set-focus btn1");
    let button = common::listener_focus(&uia, window);
    assert_eq!(button.read().role.hwnd, 0, "the button has no window");
    // Read before the window hangs: the focus the outpost holds behind the
    // hung read's marker.
    let (earlier, _) = outpost.listener_read(&button);

    let hung = Arc::new(AtomicBool::new(true));
    outpost.listener_judges_readability(hung_once_set(window, &hung));
    let (judged, calls) = outpost.listener_read(&button);
    assert_eq!(
        (judged.dropped_reason(), calls),
        (
            Some(HUNG),
            CallCounts {
                uia: 1,
                msaa: 0,
                window_messages: 0,
                connections: 0,
            }
        ),
        "dropped once its window was looked up, with no other call"
    );

    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 1 });
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 2 });
    outpost.deliver(earlier.into_fact(2, FocusEvent::Focus));
    outpost.deliver(judged.into_fact(1, FocusEvent::Focus));
    assert_eq!(
        outpost.reported_focus().node.name.as_deref(),
        Some("Original Name"),
        "the focus behind the drop is released"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The listener's reading threads, run here (`ReadingThreads`), their
/// readers on the frozen clocks and the schedule on the system's: a focus
/// with a window of its own whose walk's first call is held, and whose
/// window the listener's reader then answers hung. The watchdog's judgement
/// at the grace's end, or half a second later, finds it hung and abandons
/// the read, which it publishes from its progress first; a new thread
/// takes the application's next focus while the abandoned one is still
/// held, and drops it at its first test of the window, with no call. The
/// listener sends nothing but each event's outcome: no hang is reported.
fn the_listeners_blocked_read_is_abandoned_when_judged_hung() {
    use verbatim_listener::focus_read::ReadingThreads;
    common::init_com();
    let title = common::unique_title("mockapp-hangs-blocked");
    let mut app = common::spawn("small.json", "uia", &title);
    let window = common::find_window(&title).0 as isize;
    let uia = Uia::new().expect("a UIA client");
    let focus = window_focus(&mut app, &uia, window);
    let frozen = Instant::now();
    let frozen_us = common::now_us();
    let hung = Arc::new(AtomicBool::new(false));
    let (threads, outcomes) = ReadingThreads::new(
        Box::new(move |reader| {
            reader.set_clock(Arc::new(move || frozen));
            reader.set_observation_clock(Arc::new(move || frozen_us));
            // No window takes the foreground where the test runs.
            reader.set_foreground_reader(Arc::new(move || window));
        }),
        hung_once_set(window, &hung),
    );
    let pid = verbatim_model::Pid(app.pid());

    app.hold();
    let first = threads
        .push(pid, &focus, frozen_us)
        .expect("the focus's element kept for its read");
    assert_eq!(
        app.held(),
        "GetPropertyValue",
        "the focus's keyboard focus is held"
    );
    hung.store(true, Ordering::SeqCst);
    let (published, judged) = outcomes
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the held read published");
    assert_eq!(published, first);
    assert_published_before_its_steps(judged, window);

    let next = threads
        .push(pid, &focus, frozen_us)
        .expect("the next focus's element kept for its read");
    let (dropped, judged) = outcomes
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the next focus read by a new thread while the first is held");
    assert_eq!((dropped, judged.dropped_reason()), (next, Some(HUNG)));

    app.release();
    drop(threads);
    assert!(
        outcomes.try_recv().is_err(),
        "nothing more is said of either focus"
    );
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "the_listeners_read_ends_before_a_call_into_a_window_that_hangs",
            the_listeners_read_ends_before_a_call_into_a_window_that_hangs,
        ),
        (
            "the_listeners_read_in_its_checks_ends_before_a_later_call",
            the_listeners_read_in_its_checks_ends_before_a_later_call,
        ),
        (
            "the_listeners_blocked_read_is_abandoned_when_judged_hung",
            the_listeners_blocked_read_is_abandoned_when_judged_hung,
        ),
        (
            "a_query_whose_window_hangs_during_its_call_is_answered_when_it_answers",
            a_query_whose_window_hangs_during_its_call_is_answered_when_it_answers,
        ),
        (
            "a_hang_while_the_listeners_read_waits_is_told_once_and_the_focus_spoken_when_it_answers",
            a_hang_while_the_listeners_read_waits_is_told_once_and_the_focus_spoken_when_it_answers,
        ),
        (
            "a_query_whose_user_moved_on_is_abandoned",
            a_query_whose_user_moved_on_is_abandoned,
        ),
        (
            "a_query_on_a_windowless_node_of_a_hung_window_is_refused_with_no_call",
            a_query_on_a_windowless_node_of_a_hung_window_is_refused_with_no_call,
        ),
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
