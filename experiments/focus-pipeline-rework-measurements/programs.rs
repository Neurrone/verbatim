//! Scratch programs for the focus pipeline rework's step 1, "Measurement"
//! (`docs/design/focus-pipeline-rework.md`, "Order of work"), never merged.
//! Each runs on a desktop of its own through mockapp's test harness, so none
//! needs the interactive desktop or its input.
//!
//! To run them, copy this file to `crates/mockapp/tests/rework_programs.rs`
//! in a worktree of main, add to `crates/mockapp/Cargo.toml`
//!
//! ```toml
//! [[test]]
//! name = "rework_programs"
//! harness = false
//! ```
//!
//! and run `cargo test -p mockapp --test rework_programs -- --test-threads=1
//! <name>` for each program named in `main`.
//!
//! - `server_side_provider_under_stall`: how long `UiaHasServerSideProvider`
//!   takes on mockapp's UIA window while its window thread is stalled for 3,
//!   10 and 30 seconds, five times each, beside the raw probe's answer and
//!   time under a stall of the same length.
//! - `probes_per_new_window`: the raw probes (`WM_GETOBJECT` window
//!   messages, which is all the raw probe counts) the listener's reader and
//!   the outpost make for the first and the second focus in a new window,
//!   for a UIA window and an MSAA window, as `call_counts` measures them.
//! - `provider_description_with_a_focus_event`: the
//!   `UIA_ProviderDescriptionPropertyId` a desktop-wide UIA focus
//!   registration caches with each focus event, for mockapp's `uia`, `msaa`
//!   and `win32` backends, the window given its thread's keyboard focus
//!   first.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use verbatim_outpost::arbitration::NEGATIVE_VERDICT_LIFETIME;
use verbatim_outpost::focus_read::{FocusEvent, Judged};
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationFocusChangedEventHandler,
    IUIAutomationFocusChangedEventHandler_Impl, UIA_ClassNamePropertyId, UIA_ControlTypePropertyId,
    UIA_FrameworkIdPropertyId, UIA_NamePropertyId, UIA_ProcessIdPropertyId,
    UIA_ProviderDescriptionPropertyId, UiaHasServerSideProvider,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Accessibility::NotifyWinEvent;
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, EVENT_OBJECT_FOCUS, FindWindowExW, GetWindowThreadProcessId, OBJID_CLIENT,
};
use windows::core::w;

use common::outpost::OutpostUnderTest;

/// How long `UiaHasServerSideProvider` takes on a stalled UIA window, and
/// the raw probe's answer under a stall of the same length.
fn server_side_provider_under_stall() {
    let title = common::unique_title("rework-uhssp");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    // UIA's first-time setup, outside any timing.
    let _ = verbatim_uia::raw_probe(hwnd.0 as isize);
    // SAFETY: a live window handle; the call only reads.
    let unstalled = unsafe { UiaHasServerSideProvider(hwnd) }.as_bool();
    println!("unstalled: UiaHasServerSideProvider {unstalled}");
    for ms in [3_000_u64, 10_000, 30_000] {
        let stall = Duration::from_millis(ms);
        for run in 1..=5 {
            app.stall(stall);
            let started = Instant::now();
            // SAFETY: as above.
            let answer = unsafe { UiaHasServerSideProvider(hwnd) }.as_bool();
            let took = started.elapsed();
            let returned_at = common::now_us();
            let ended = app.stall_ended(stall);
            let returned_before_end = returned_at < ended;
            app.stall(stall);
            let started = Instant::now();
            let probed = verbatim_uia::raw_probe(hwnd.0 as isize);
            let probe_took = started.elapsed();
            app.stall_ended(stall);
            println!(
                "stall {ms} ms, run {run}: UiaHasServerSideProvider answered {answer} after {} ms \
                 (returned before the stall ended: {returned_before_end}); raw probe answered \
                 {probed:?} after {} ms",
                took.as_millis(),
                probe_took.as_millis(),
            );
        }
    }
    app.quit();
}

/// The raw probes `calls` counts as window messages.
fn probes(calls: verbatim_model::CallCounts) -> u32 {
    calls.window_messages
}

/// The listener's reader and the outpost, each on a new window's first and
/// second focus, with the outpost's clock standing still, as `call_counts`
/// runs them: a verdict of no provider is kept for
/// [`NEGATIVE_VERDICT_LIFETIME`] and then probed again.
fn probes_per_new_window() {
    println!(
        "a no-provider verdict is kept for {} ms",
        NEGATIVE_VERDICT_LIFETIME.as_millis()
    );
    // A UIA window: the listener reads each focus, the outpost reports it.
    {
        let title = common::unique_title("rework-probes-uia");
        let mut app = common::spawn("small.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let uia = Uia::new().expect("a UIA client");
        let outpost = OutpostUnderTest::new(&app);
        for (round, id) in [(1, "btn1"), (2, "slider1")] {
            app.send(&format!("set-focus {id}"));
            let focused = common::listener_focus(&uia, hwnd.0 as isize);
            let (judged, listener_calls) = outpost.listener_read(&focused);
            let read = matches!(judged, Judged::Read(_));
            let reported = outpost.focus(judged.into_fact(0, FocusEvent::Focus));
            println!(
                "uia window, focus {round} ({id}): listener probes {}, read {read}; outpost probes {} \
                 for its report",
                probes(listener_calls),
                probes(reported.calls),
            );
        }
        drop(outpost);
        app.quit();
    }
    // An MSAA window: the outpost reads each MSAA focus; UIA's MSAA proxy
    // raises a UIA focus for it too, which the listener judges.
    {
        let title = common::unique_title("rework-probes-msaa");
        let mut app = common::spawn("small.json", "msaa", &title);
        let hwnd = common::find_window(&title);
        let uia = Uia::new().expect("a UIA client");
        let outpost = OutpostUnderTest::new(&app);
        // small.json's nodes depth first, the root at 0: btn1 is 1, slider1 2.
        for (round, id, index) in [(1, "btn1", 1), (2, "slider1", 2)] {
            app.send(&format!("set-focus {id}"));
            let reported = outpost.msaa_focus(hwnd, index);
            let focused = common::listener_focus(&uia, hwnd.0 as isize);
            let (judged, listener_calls) = outpost.listener_read(&focused);
            let outcome = match &judged {
                Judged::Read(_) => "read".to_owned(),
                Judged::Dropped(reason) => format!("dropped: {reason}"),
                Judged::Passed(_) => "passed".to_owned(),
            };
            if let Judged::Dropped(_) = &judged {
                outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
                outpost.settled();
            }
            println!(
                "msaa window, focus {round} ({id}): outpost probes {} for its report; listener \
                 probes {} for UIA's proxy focus, {outcome}",
                probes(reported.calls),
                probes(listener_calls),
            );
        }
        drop(outpost);
        app.quit();
    }
}

/// A focus handler that sends each event's cached properties.
#[windows::core::implement(IUIAutomationFocusChangedEventHandler)]
struct Handler {
    sent: mpsc::Sender<String>,
}

impl IUIAutomationFocusChangedEventHandler_Impl for Handler_Impl {
    fn HandleFocusChangedEvent(
        &self,
        sender: windows::core::Ref<IUIAutomationElement>,
    ) -> windows::core::Result<()> {
        let Some(element) = sender.as_ref() else {
            return Ok(());
        };
        let cached = |id| {
            // SAFETY: a property the event's cache request holds.
            unsafe { element.GetCachedPropertyValue(id) }.map_or_else(
                |error| format!("<{error}>"),
                |value: VARIANT| value.to_string(),
            )
        };
        let _ = self.sent.send(format!(
            "name {:?}, control type {}, class {:?}, framework {:?}, process {}, provider \
             description {:?}",
            cached(UIA_NamePropertyId),
            cached(UIA_ControlTypePropertyId),
            cached(UIA_ClassNamePropertyId),
            cached(UIA_FrameworkIdPropertyId),
            cached(UIA_ProcessIdPropertyId),
            cached(UIA_ProviderDescriptionPropertyId),
        ));
        Ok(())
    }
}

/// The focus events of mockapp `backend`'s window, each with its cached
/// provider description, after `raise` makes mockapp's focus move.
fn describe_focus(backend: &str, fixture: &str, raise: impl FnOnce(&mut common::MockApp, HWND)) {
    let title = common::unique_title(&format!("rework-description-{backend}"));
    let mut app = common::spawn(fixture, backend, &title);
    let hwnd = common::find_window(&title);
    // SAFETY: joins this thread to the multithreaded apartment.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: a registered in-process COM server, as `verbatim_uia` makes it.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .expect("a UIA client");
    // SAFETY: plain COM calls on a live client.
    let request = unsafe { automation.CreateCacheRequest() }.expect("a cache request");
    for property in [
        UIA_NamePropertyId,
        UIA_ControlTypePropertyId,
        UIA_ClassNamePropertyId,
        UIA_FrameworkIdPropertyId,
        UIA_ProcessIdPropertyId,
        UIA_ProviderDescriptionPropertyId,
    ] {
        // SAFETY: as above.
        unsafe { request.AddProperty(property) }.expect("a property cached");
    }
    let (sent, received) = mpsc::channel();
    let handler: IUIAutomationFocusChangedEventHandler = Handler { sent }.into();
    // SAFETY: as above; the handler lives until it is removed below.
    unsafe { automation.AddFocusChangedEventHandler(&request, &handler) }
        .expect("the focus registration");
    // Events from before the move, such as the window's own, are left out.
    std::thread::sleep(Duration::from_millis(500));
    while received.try_recv().is_ok() {}
    raise(&mut app, hwnd);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut seen = 0;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match received.recv_timeout(left) {
            Ok(line) => {
                seen += 1;
                println!("{backend}: focus event {seen}: {line}");
            }
            Err(_) => break,
        }
    }
    if seen == 0 {
        println!("{backend}: no focus event within 3 s");
    }
    // SAFETY: as above.
    unsafe { automation.RemoveAllEventHandlers() }.expect("the registration removed");
    app.quit();
}

/// Gives mockapp's window the keyboard focus of its thread, as a click
/// would, through the test thread's input attached to it: Windows raises
/// the focus event itself, from mockapp's thread.
fn give_window_focus(hwnd: HWND) {
    // SAFETY: plain calls on a live window and this thread; the input is
    // detached again before returning.
    unsafe {
        let thread = GetWindowThreadProcessId(hwnd, None);
        let this = GetCurrentThreadId();
        assert!(AttachThreadInput(this, thread, true).as_bool(), "input attached");
        let _ = SetFocus(None);
        let _ = SetFocus(Some(hwnd));
        let _ = AttachThreadInput(this, thread, false);
    }
}

/// Whether the provider description cached with a focus event tells UIA's
/// MSAA proxy apart, for mockapp's `uia`, `msaa` and `win32` backends: each
/// window is first given its thread's keyboard focus, which Windows raises
/// the focus event of, and then, on the `uia` and `msaa` backends, mockapp
/// moves its own focus to a node (`focus slider1`), raising its UIA or its
/// MSAA focus; on the `win32` backend, which has no tree of its own to move
/// a focus in, the fixture's native Win32 button is given the focus next.
fn provider_description_with_a_focus_event() {
    describe_focus("uia", "small.json", |app, hwnd| {
        give_window_focus(hwnd);
        app.send("focus slider1");
    });
    describe_focus("msaa", "small.json", |app, hwnd| {
        give_window_focus(hwnd);
        app.send("focus slider1");
    });
    describe_focus("win32", "group_box.json", |_, hwnd| {
        give_window_focus(hwnd);
        // SAFETY: a search among a live window's children by class and text.
        let button = unsafe { FindWindowExW(Some(hwnd), None, w!("Button"), w!("Apply")) }
            .expect("the fixture's native Apply button");
        give_window_focus(button);
        // And the event raised again by this process, on the button that
        // now has its thread's focus.
        // SAFETY: raises an event naming a live window; no pointer.
        unsafe { NotifyWinEvent(EVENT_OBJECT_FOCUS, button, OBJID_CLIENT.0, CHILDID_SELF as i32) };
    });
}

fn main() {
    harness::run_isolated(&[
        (
            "server_side_provider_under_stall",
            server_side_provider_under_stall,
        ),
        ("probes_per_new_window", probes_per_new_window),
        (
            "provider_description_with_a_focus_event",
            provider_description_with_a_focus_event,
        ),
    ]);
}
