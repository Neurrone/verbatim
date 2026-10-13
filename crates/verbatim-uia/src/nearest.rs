//! Nearest-window-handle resolution for a UIA element that is not itself a
//! window (architecture section 4).
//!
//! Most elements that raise UIA events are not windows in their own right — a
//! menu item or a list item is a descendant of one — so
//! their cached native window handle (`crate::Kept::hwnd`) reads 0 and cannot
//! answer "which window does the outpost's per-window arbitration verdict
//! apply to?" This module is NVDA's answer, adopted unchanged:
//! `getNearestWindowHandle` in `nvda/source/UIAHandler/__init__.py`
//! (reference only, never modified — see this repository's `CLAUDE.md`). A
//! tree walker whose condition excludes every element without a native
//! window handle, driven by `NormalizeElementBuildCache`, resolves an
//! element to itself or its nearest ancestor with one, in a single
//! cross-process round trip.

use std::cell::RefCell;

use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTreeWalker, UIA_NativeWindowHandlePropertyId,
};

use crate::shape::{Live, Request, WindowLookup};

/// The walker and cache request bound to one thread's own `IUIAutomation`
/// instance, built once per thread and reused for every subsequent call on
/// that thread — the same one-client-per-thread rule
/// [`crate::client::Uia`] follows, so nothing COM here ever crosses a thread
/// boundary.
struct Context {
    walker: IUIAutomationTreeWalker,
    request: Request<WindowLookup>,
}

impl Context {
    fn build() -> windows::core::Result<Self> {
        let client = crate::client::create_client()?;
        // Build the "has no native window handle" condition, negate it, hand
        // the negation to a fresh tree walker, and build the cache request
        // of the one property `NormalizeElementBuildCache` needs to answer,
        // a `WindowLookup` — exactly NVDA's `windowTreeWalker` and
        // `windowCacheRequest`.
        let zero = VARIANT::from(0i32);
        // Local, same-thread COM calls against the client just created,
        // each taking only live COM objects or plain values.
        // SAFETY: as above.
        let has_no_handle =
            unsafe { client.CreatePropertyCondition(UIA_NativeWindowHandlePropertyId, &zero) }?;
        // SAFETY: as above.
        let is_a_window = unsafe { client.CreateNotCondition(&has_no_handle) }?;
        // SAFETY: as above.
        let walker = unsafe { client.CreateTreeWalker(&is_a_window) }?;
        let request = Request::build(&client, ())?;
        Ok(Self { walker, request })
    }
}

thread_local! {
    /// `None` until the first call on this thread; left `None` again after a
    /// failed build so a transient `CoCreateInstance` failure gets retried on
    /// the next call rather than permanently disabling this thread.
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

/// Drops this thread's walker and cache request, if they were built (see
/// [`crate::release_thread_state`]).
pub(crate) fn release_thread_context() {
    let context = CONTEXT.with(|cell| cell.borrow_mut().take());
    drop(context);
}

/// Resolves the native window handle of `element` itself, or of its nearest
/// ancestor that has one — NVDA's `getNearestWindowHandle`. If `element`
/// already has a window handle, the call still works: `NormalizeElement`
/// degenerates to returning the starting element unchanged.
///
/// # Blocking
///
/// This makes exactly one cross-process COM call
/// (`NormalizeElementBuildCache`) and can block on a hung application, like
/// any other cross-process UIA call. It is callable from UIA event-callback
/// threads specifically because each outpost watches a single application
/// (decision D9): a hang here stalls only that application's own outpost,
/// which the supervisor's recovery ladder already covers — the same trade
/// NVDA makes running this same walk on its own UIA event-handler thread.
///
/// Returns `None` on any COM failure, including apartment or client setup
/// failing on this thread, the walk itself failing, or no ancestor carrying
/// a window handle being found (the last should not happen for a live
/// element rooted under the desktop).
#[must_use]
pub fn nearest_window_handle(element: &IUIAutomationElement) -> Option<isize> {
    CONTEXT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Context::build().ok();
        }
        let context = slot.as_ref()?;
        // `context.walker` and `context.request` are this thread's own,
        // either just built or reused unchanged from an earlier call on this
        // same thread.
        let normalized = Live::new(element.clone())
            .normalize(&context.walker, &context.request)
            .ok()?;
        let hwnd = normalized.read().hwnd;
        (hwnd != 0).then_some(hwnd)
    })
}
