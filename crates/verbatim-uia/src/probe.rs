//! The UIA arbitration probe.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::UiaHasServerSideProvider;

use verbatim_model::CallKind;

/// Asks a window whether it exposes a native UIA server-side provider — the
/// third rung of the arbitration ladder (architecture section 4), after the
/// good- and bad-class lists.
///
/// The window is asked once, and its answer is the answer, as NVDA asks
/// (`docs/parity/backends.md`). `UiaHasServerSideProvider` reports no
/// provider when the window does not answer within UIA's own wait, three to
/// five seconds; that "no" is kept like any other, for the short while the
/// arbitrator trusts a verdict of no provider, after which the window is
/// asked again.
///
/// This sends `WM_GETOBJECT` to the target window and therefore blocks on the
/// target application's message pump, for up to UIA's own wait. It MUST be
/// called only from a deadline-guarded worker, never from an event thread.
#[must_use]
pub fn has_server_side_provider(hwnd: isize) -> bool {
    // The probe is a way into UIA's first-time setup too; a failed setup
    // leaves the probe to try anyway.
    let _ = crate::client::ensure_ready();
    // One `WM_GETOBJECT`, sent to the window.
    crate::calls::count(CallKind::WindowMessage);
    // A new connection to the window by its handle, which answers "no"
    // after about three seconds when it arrives while a remote operation
    // from another process waits to be evaluated in the window's
    // application: made holding that application's lock (`crate::lock`).
    crate::lock::guarded_window("UiaHasServerSideProvider", hwnd, || {
        // SAFETY: UiaHasServerSideProvider tolerates any window handle,
        // returning false for invalid ones; the BOOL is converted, not
        // assumed.
        unsafe { UiaHasServerSideProvider(HWND(hwnd as *mut _)) }.as_bool()
    })
}
