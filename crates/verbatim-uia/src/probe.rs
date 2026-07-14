//! The UIA arbitration probe.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::UiaHasServerSideProvider;

/// Asks a window whether it exposes a native UIA server-side provider — the
/// third rung of the arbitration ladder (architecture section 4), after the
/// good- and bad-class lists.
///
/// This sends `WM_GETOBJECT` to the target window and therefore blocks on the
/// target application's message pump. A hung app will hang this call. It MUST
/// be called only from a deadline-guarded query-pool thread, never from an
/// event thread; the outpost wraps it in a timeout and treats expiry as
/// non-UIA. See [`crate::probe`] module docs and the outpost arbitration.
#[must_use]
pub fn has_server_side_provider(hwnd: isize) -> bool {
    // SAFETY: UiaHasServerSideProvider tolerates any window handle, returning
    // false for invalid ones; the returned BOOL is converted, not assumed.
    unsafe { UiaHasServerSideProvider(HWND(hwnd as *mut _)).as_bool() }
}
