//! Marks Core's hidden 1x1 main frame so every outpost can recognize and
//! suppress announcing it (architecture section 1, decision D9).
//!
//! The frame transits real focus during the prePopup show/raise/foreground
//! dance around the Verbatim menu and the settings dialog (see
//! [`crate::pre_popup`] and [`crate::post_popup`]); without a marker it can
//! be announced as a nameless "Verbatim" window with role unknown. `mark` is
//! called once, right after the frame is built; `unmark` is called at
//! shutdown so the property does not outlive the window.

use std::ffi::c_void;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::UI::WindowsAndMessaging::{RemovePropW, SetPropW};
use windows::core::HSTRING;

use verbatim_model::HIDDEN_FRAME_WINDOW_PROP;

/// A non-null sentinel value for the property; outposts only test for the
/// property's presence via `GetPropW`, never dereference it.
fn sentinel() -> HANDLE {
    HANDLE(std::ptr::dangling_mut::<c_void>())
}

/// Stamps `hwnd` with the hidden-frame marker property.
pub(crate) fn mark(hwnd: HWND) {
    let name = HSTRING::from(HIDDEN_FRAME_WINDOW_PROP);
    // SAFETY: `hwnd` is a live window handle just obtained from the frame
    // that owns it; the property value is an opaque sentinel, never read
    // back as a real handle.
    unsafe {
        let _ = SetPropW(hwnd, &name, Some(sentinel()));
    }
}

/// Removes the hidden-frame marker property from `hwnd`.
pub(crate) fn unmark(hwnd: HWND) {
    let name = HSTRING::from(HIDDEN_FRAME_WINDOW_PROP);
    // SAFETY: `hwnd` is a live window handle whose property was set by
    // `mark`; removing an absent property is a documented no-op failure.
    unsafe {
        let _ = RemovePropW(hwnd, &name);
    }
}
