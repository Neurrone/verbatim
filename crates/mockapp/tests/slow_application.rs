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
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};

/// Longer than the old focus deadline, well under NVDA's ten seconds.
const STALL: Duration = Duration::from_millis(2500);

#[test]
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
        Box::new(move |_, hwnd, id_object, id_child| {
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

    app.send("stall 2500");
    // Let mockapp's window thread take the stall before the fact arrives.
    std::thread::sleep(Duration::from_millis(200));
    let delivered = Instant::now();
    outpost.handle_command(&SupervisorToOutpost::DeliverFact {
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        fact: DeliveredFact::MsaaFocus {
            hwnd,
            id_object,
            id_child,
        },
    });

    let deadline = delivered + common::WAIT_TIMEOUT;
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        let message = messages
            .recv_timeout(wait)
            .expect("the outpost reports the focus before the wait times out");
        if let OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } = message
        {
            assert_eq!(node.name.as_deref(), Some("Original Name"));
            assert!(
                delivered.elapsed() >= STALL.saturating_sub(Duration::from_millis(300)),
                "the read waited on the stalled window thread, so the test exercised a slow read"
            );
            break;
        }
    }
    app.send("quit");
}

/// Pumps this thread's messages, which out-of-context `WinEvent`s are
/// delivered through, until `value` yields something.
fn pump_until<T>(mut value: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + common::WAIT_TIMEOUT;
    let mut msg = MSG::default();
    loop {
        if let Some(value) = value() {
            return value;
        }
        // SAFETY: a standard non-blocking message pump; `msg` is written by
        // `PeekMessageW` when it returns a message.
        unsafe {
            if PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&raw const msg);
                DispatchMessageW(&raw const msg);
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for mockapp's focus event"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
