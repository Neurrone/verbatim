//! A focus in an application that is slow to answer is still reported: an
//! outpost watching `mockapp` is handed an MSAA focus fact while mockapp's
//! window thread is stalled, so every read of the focused object waits.
//!
//! Found live in the end-to-end suite: Notepad starting up and the Start
//! menu's search window took two to three seconds to answer reads that then
//! succeeded, the outpost abandoned them at its old 1.5 second deadline, and
//! the focus was never announced. NVDA waits up to ten seconds; so does the
//! outpost now.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::io::BufReader;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use verbatim_ia2::{WinEvent, WinEventHook, WinEventKind};
use verbatim_model::{NormalizedEvent, TraceId};
use verbatim_outpost::Outpost;
use verbatim_outpost::protocol::{
    DeliveredFact, OutpostToSupervisor, SupervisorToOutpost, read_message,
};

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

    let (pipe_in, pipe_out) = std::io::pipe().expect("an anonymous pipe");
    let outpost = Outpost::new(Box::new(pipe_out), pid);
    // No window takes the foreground on the desktop the test runs on:
    // mockapp's is the foreground, so its focus passes the acceptance test.
    outpost.set_foreground_reader(Arc::new(move || window));
    let (messages_tx, messages) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe_in);
        while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
            if messages_tx.send(message).is_err() {
                return;
            }
        }
    });
    assert_eq!(
        messages.recv_timeout(common::WAIT_TIMEOUT).ok(),
        Some(OutpostToSupervisor::Ready {
            outpost_pid: verbatim_model::Pid(std::process::id()),
            target_pid: verbatim_model::Pid(pid),
        }),
        "the outpost announces itself first"
    );

    // The window thread is stalled before the fact arrives.
    app.stall(STALL);
    outpost.handle_command(&SupervisorToOutpost::DeliverFact {
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        timing: verbatim_outpost::protocol::EventTiming::default(),
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
    let OutpostToSupervisor::Event {
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

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[(
        "a_focus_read_that_waits_on_a_busy_application_is_still_reported",
        a_focus_read_that_waits_on_a_busy_application_is_still_reported,
    )]);
}
