//! UIA's one-time setup in an application, which the focus listener
//! completes before the application is given an outpost
//! (`docs/design/focus-pipeline.md`, section 20).
//!
//! The Windows defect this works around: the first time any client runs a
//! remote operation in an application, UIA sets itself up inside that
//! application, in COM's modal loop on its UI thread, and while that runs a
//! new UIA connection from any other process to any window of the
//! application fails without an error: `UiaHasServerSideProvider` waits
//! three seconds and answers "no", and `ElementFromHandle` and
//! `GetFocusedElement` wait two seconds and return UIA's client-side window
//! proxy instead of the application's own element. It happens once per
//! application process. The listener's reads are remote operations, so an
//! outpost making its first call into an application while the listener's
//! first read ran there read the window through MSAA, or announced it
//! twice. So the listener runs a trivial remote operation in each new
//! application first, and the supervisor gives the application its outpost
//! only once that has completed: the order alone prevents the collision.
//! Section 20 holds the evidence and the measurements.
//!
//! The setup finds an element served by the application's own provider,
//! since importing UIA's client-side proxy into a remote operation fails:
//! the element the reading thread last read the focus from, when it has
//! one; otherwise the window the application's first fact named, its
//! foreground window, and its other visible top-level windows, each
//! followed by its child windows, probed with `UiaHasServerSideProvider`
//! until one answers "yes", whose element `ElementFromHandle` gives. A
//! remote operation that only imports the element then completes the
//! setup, or finds it done.
//!
//! An application that does not answer: a call of the setup that passes
//! UIA's time limit leaves the setup undone, and the application processes
//! the request when it next pumps, so an outpost connecting then would
//! collide. The setup then waits for the application on a thread of its
//! own ([`wait_for`]): it sends the window `WM_NULL`, which returns once the
//! application's thread has handled it, and runs the setup again, until it
//! completes. A remote operation sent while a setup is in progress
//! completes only after the setup has (checked with mockapp's hold, section
//! 20.6), so the second run's completing proves the setup done.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, UIA_E_TIMEOUT};
use windows::Win32::UI::WindowsAndMessaging::{SMTO_NORMAL, SendMessageTimeoutW, WM_NULL};

use verbatim_model::Pid;
use verbatim_uia::Uia;

use crate::outpost::window::{
    descendant_windows, foreground_window_handle, top_level_of, top_level_windows,
    window_is_visible, window_owner,
};
use crate::protocol::SetUpOutcome;

/// How long a provider probe may take and still be taken as the window's
/// answer: a "no" that took longer is UIA giving up on a window that did not
/// answer (it waits about three seconds), not the window saying it has no
/// provider, so the application is waited for and the probe made again.
/// An application that answers "no" this slowly costs only that second
/// probe.
const PROBE_ANSWERED_WITHIN: Duration = Duration::from_secs(1);

/// How long each `WM_NULL` of [`wait_for`] waits before it is sent again,
/// so the wait can be stopped at the listener's shutdown.
const WAIT_SLICE: Duration = Duration::from_secs(1);

/// What one run of the setup found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetUpResult {
    /// The setup is over, as the outcome says.
    Over(SetUpOutcome),
    /// A call into the window passed UIA's time limit, or its probe went
    /// unanswered: the application is not answering, and the setup waits
    /// for it, from this window.
    NotAnswering(isize),
}

/// Runs the setup in `pid` with `uia`: in `known`, an element of the
/// application's own provider the caller holds, if it has one, else in the
/// first element of the application's own provider found from `hint`, the
/// window its first fact named, or 0 (the module's documentation gives the
/// order).
#[must_use]
pub fn run(uia: &Uia, pid: Pid, hint: isize, known: Option<&IUIAutomationElement>) -> SetUpResult {
    if let Some(element) = known {
        match import(element) {
            Imported::Done => return SetUpResult::Over(SetUpOutcome::Done),
            Imported::TimedOut => {
                return candidates(pid.0, hint)
                    .first()
                    .map_or(SetUpResult::Over(SetUpOutcome::Failed), |&window| {
                        SetUpResult::NotAnswering(window)
                    });
            }
            Imported::Failed(outcome) => return SetUpResult::Over(outcome),
            // A proxy, read the classic way: the search below finds the
            // application's own element.
            Imported::Proxy => {}
        }
    }
    for window in candidates(pid.0, hint) {
        let probed = Instant::now();
        if !verbatim_uia::has_server_side_provider(window) {
            if probed.elapsed() >= PROBE_ANSWERED_WITHIN {
                return SetUpResult::NotAnswering(window);
            }
            continue;
        }
        let element = match uia.window_live(window) {
            Ok(live) => live.element().clone(),
            Err(error) if verbatim_uia::timed_out(&error) => {
                return SetUpResult::NotAnswering(window);
            }
            Err(error) => {
                tracing::debug!(window, %error, "setup: the window's element could not be had");
                continue;
            }
        };
        match import(&element) {
            Imported::Done => return SetUpResult::Over(SetUpOutcome::Done),
            Imported::TimedOut => return SetUpResult::NotAnswering(window),
            Imported::Failed(outcome) => return SetUpResult::Over(outcome),
            Imported::Proxy => {}
        }
    }
    SetUpResult::Over(SetUpOutcome::NoProvider)
}

/// What the trivial remote operation made of an element.
enum Imported {
    /// It completed: the setup is done.
    Done,
    /// It passed UIA's time limit.
    TimedOut,
    /// The element is UIA's client-side proxy, which cannot be imported.
    Proxy,
    /// It failed otherwise, as the outcome says.
    Failed(SetUpOutcome),
}

/// Runs the trivial remote operation, which only imports `element`: one
/// UIA call, which completes the application's setup or finds it done.
fn import(element: &IUIAutomationElement) -> Imported {
    let mut builder = verbatim_uia_rops::Builder::new();
    let _ = builder.import_element(element);
    builder.halt();
    match builder.finish().execute() {
        Ok(_) => Imported::Done,
        Err(verbatim_uia_rops::Error::Import(_)) => Imported::Proxy,
        Err(error)
            if error
                .hresult()
                .is_some_and(|code| code.0.cast_unsigned() == UIA_E_TIMEOUT) =>
        {
            Imported::TimedOut
        }
        Err(error) => {
            tracing::warn!(%error, "setup: the remote operation failed");
            Imported::Failed(SetUpOutcome::Failed)
        }
    }
}

/// The windows of `pid` to look for its own provider in, in order: the top
/// level of `hint`, of the foreground window, and every other visible
/// top-level window of the application, each followed by its descendants.
/// Local calls only.
fn candidates(pid: u32, hint: isize) -> Vec<isize> {
    let owned = |window: isize| window != 0 && window_owner(window).1 == pid;
    let mut tops: Vec<isize> = Vec::new();
    for window in [hint, foreground_window_handle()] {
        let top = if window == 0 { 0 } else { top_level_of(window) };
        if owned(top) && !tops.contains(&top) {
            tops.push(top);
        }
    }
    for top in top_level_windows(pid) {
        if window_is_visible(top) && !tops.contains(&top) {
            tops.push(top);
        }
    }
    let mut windows = Vec::new();
    for top in tops {
        windows.push(top);
        windows.extend(
            descendant_windows(top)
                .into_iter()
                .filter(|&child| owned(child)),
        );
    }
    windows
}

/// Waits for `pid`, whose setup could not complete from `window` because
/// the application is not answering, and runs the setup again, on this
/// thread, until it is over or `stop` is set: sends `window` `WM_NULL`,
/// which returns once the application's thread has handled it, then runs
/// the setup with a UIA client of this thread's. A window destroyed
/// meanwhile is let go, and the setup looks for the application's own
/// provider afresh. Returns the outcome, or `None` once stopped.
#[must_use]
pub fn wait_for(pid: Pid, window: isize, stop: &Arc<AtomicBool>) -> Option<SetUpOutcome> {
    let uia = match Uia::new() {
        Ok(uia) => uia,
        Err(error) => {
            tracing::warn!(%error, %pid, "setup: the waiting thread has no UIA client");
            return Some(SetUpOutcome::Failed);
        }
    };
    let mut window = window;
    let outcome = loop {
        // A window destroyed meanwhile has nothing to wait for: the setup
        // looks afresh.
        if window != 0 {
            let _ = answered(window, stop)?;
        }
        match run(&uia, pid, 0, None) {
            SetUpResult::Over(outcome) => break outcome,
            SetUpResult::NotAnswering(next) => window = next,
        }
    };
    drop(uia);
    verbatim_uia::release_thread_state();
    verbatim_uia::leave_mta();
    Some(outcome)
}

/// Sends `window` `WM_NULL` and waits for its thread to handle it, in
/// slices, until it has (`Some(true)`), the window is destroyed
/// (`Some(false)`), or `stop` is set (`None`).
fn answered(window: isize, stop: &AtomicBool) -> Option<bool> {
    let slice = u32::try_from(WAIT_SLICE.as_millis()).unwrap_or(u32::MAX);
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        // SAFETY: a message with no pointers, to a window handle that may
        // have been destroyed, which the call reports as a failure.
        let sent = unsafe {
            SendMessageTimeoutW(
                HWND(window as *mut _),
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_NORMAL,
                slice,
                None,
            )
        };
        if sent.0 != 0 {
            return Some(true);
        }
        // A send that timed out leaves the window's last error at
        // `ERROR_TIMEOUT`; any other failure is a window that has gone.
        let error = windows::core::Error::from_thread();
        if error.code() != windows::Win32::Foundation::ERROR_TIMEOUT.to_hresult() {
            return Some(false);
        }
    }
}
