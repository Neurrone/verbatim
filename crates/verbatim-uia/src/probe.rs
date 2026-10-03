//! The UIA arbitration probe.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::UiaHasServerSideProvider;
use windows::Win32::UI::WindowsAndMessaging::{SMTO_NORMAL, SendMessageTimeoutW, WM_NULL};

/// How quickly a window that answers says whether it has a provider. Live,
/// every answer took 0 to 89 ms; `UiaHasServerSideProvider` instead gives up
/// on a busy window after three to five seconds and reports no provider, as
/// Windows 11 Notepad's text control did while Notepad was starting, though
/// the same window answered "yes" 18 ms later. A "no" slower than this is
/// silence, not an answer.
const ANSWERED_WITHIN: Duration = Duration::from_secs(1);

/// How long the whole probe may take, within the outpost's ten-second
/// deadline for handling one event: UIA's own check can take five seconds
/// on a busy window, and the wait for the window to respond takes the rest.
const PROBE_BUDGET: Duration = Duration::from_secs(8);

/// Asks a window whether it exposes a native UIA server-side provider — the
/// third rung of the arbitration ladder (architecture section 4), after the
/// good- and bad-class lists. `None` when the window did not answer.
///
/// Only the window's own answer counts. `UiaHasServerSideProvider` reports
/// no provider when the window does not answer in time, so a "no" that took
/// longer than a real answer takes is not trusted: the probe waits for the
/// window to process messages again, within [`PROBE_BUDGET`], and asks once
/// more. A window that stays silent gets `None`; NVDA treats such a window
/// as not using UIA for the event at hand, and the caller should too,
/// without keeping that as the window's answer.
///
/// This sends `WM_GETOBJECT` to the target window and therefore blocks on the
/// target application's message pump, for up to about [`PROBE_BUDGET`]. It
/// MUST be called only from a deadline-guarded worker, never from an event
/// thread.
#[must_use]
pub fn probe_server_side_provider(hwnd: isize) -> Option<bool> {
    let window = HWND(hwnd as *mut _);
    let started = Instant::now();
    let ask = || {
        let asked = Instant::now();
        // SAFETY: UiaHasServerSideProvider tolerates any window handle,
        // returning false for invalid ones; the BOOL is converted, not
        // assumed.
        let has = unsafe { UiaHasServerSideProvider(window) }.as_bool();
        (has, asked.elapsed() < ANSWERED_WITHIN)
    };
    match ask() {
        (true, _) => return Some(true),
        (false, true) => return Some(false),
        (false, false) => {}
    }
    let remaining = PROBE_BUDGET.saturating_sub(started.elapsed());
    if remaining.is_zero() || !responds(window, remaining) {
        return None;
    }
    match ask() {
        (true, _) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    }
}

/// [`probe_server_side_provider`], taking a window that did not answer as
/// having no provider.
#[must_use]
pub fn has_server_side_provider(hwnd: isize) -> bool {
    probe_server_side_provider(hwnd).unwrap_or(false)
}

/// Whether `window` processes a message within `wait`.
fn responds(window: HWND, wait: Duration) -> bool {
    let timeout = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: WM_NULL carries no data and does nothing; the call tolerates an
    // invalid window, returning 0.
    unsafe {
        SendMessageTimeoutW(
            window,
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_NORMAL,
            timeout,
            None,
        )
    }
    .0 != 0
}
