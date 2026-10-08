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
use std::time::{Duration, Instant};

use verbatim_ia2::{WinEventHook, WinEventKind};
use verbatim_model::{NormalizedEvent, TraceId};
use verbatim_outpost::Outpost;
use verbatim_outpost::protocol::{
    DeliveredFact, OutpostToSupervisor, SupervisorToOutpost, read_message,
};
use windows::Win32::Foundation::WAIT_TIMEOUT;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage,
};

/// Longer than the old focus deadline, well under NVDA's ten seconds.
const STALL: Duration = Duration::from_millis(2500);

fn a_focus_read_that_waits_on_a_busy_application_is_still_reported() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-focus");
    let mut app = common::spawn("small.json", "msaa", &title);
    let _hwnd = common::find_window(&title);
    let pid = app.pid();

    // The focus event's real MSAA address, captured from mockapp itself.
    let focus = Arc::new(Mutex::new(None::<(isize, i32, i32)>));
    let seen = Arc::clone(&focus);
    let _hook = WinEventHook::install(
        pid,
        &[WinEventKind::Focus],
        Box::new(move |_, hwnd, id_object, id_child, _| {
            *seen.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((hwnd, id_object, id_child));
        }),
    )
    .expect("WinEventHook::install");
    app.send("focus btn1");
    let (hwnd, id_object, id_child) =
        pump_until(|| *focus.lock().unwrap_or_else(PoisonError::into_inner));

    let (pipe_in, pipe_out) = std::io::pipe().expect("an anonymous pipe");
    let outpost = Outpost::new(Box::new(pipe_out), pid);
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

/// Pumps this thread's messages, which out-of-context `WinEvent`s are
/// delivered through, until `value` yields something, waiting for each
/// message rather than polling.
fn pump_until<T>(mut value: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + common::WAIT_TIMEOUT;
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a local the call writes when it returns a message.
        while unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            // SAFETY: `msg` is the message just retrieved.
            let _ = unsafe { TranslateMessage(&raw const msg) };
            // SAFETY: as above.
            unsafe { DispatchMessageW(&raw const msg) };
        }
        if let Some(value) = value() {
            return value;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let left = u32::try_from(left.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: no handles; wakes when a message arrives or the time is up.
        let woke =
            unsafe { MsgWaitForMultipleObjectsEx(None, left, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        assert_ne!(woke, WAIT_TIMEOUT, "mockapp's focus event did not arrive");
    }
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[(
        "a_focus_read_that_waits_on_a_busy_application_is_still_reported",
        a_focus_read_that_waits_on_a_busy_application_is_still_reported,
    )]);
}
