//! Nearest-window-handle resolution for a UIA element that is not itself a
//! window (architecture section 4).
//!
//! Most elements that raise UIA events are not windows in their own right — a
//! menu item or a list item is a descendant of one — so
//! [`crate::map::cached_native_window_handle`] reads 0 for them and cannot
//! answer "which window does the outpost's per-window arbitration verdict
//! apply to?" This module is NVDA's answer, adopted unchanged:
//! `getNearestWindowHandle` in `nvda/source/UIAHandler/__init__.py`
//! (reference only, never modified — see this repository's `CLAUDE.md`). A
//! tree walker whose condition excludes every element without a native
//! window handle, driven by `NormalizeElementBuildCache`, resolves an
//! element to itself or its nearest ancestor with one, in a single
//! cross-process round trip.

use std::cell::RefCell;

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationTreeWalker, UIA_NativeWindowHandlePropertyId,
};

use crate::com::init_mta;
use crate::map::cached_native_window_handle;

/// The walker and cache request bound to one thread's own `IUIAutomation`
/// instance, built once per thread and reused for every subsequent call on
/// that thread — the same one-client-per-thread rule
/// [`crate::client::Uia`] follows, so nothing COM here ever crosses a thread
/// boundary.
struct Context {
    walker: IUIAutomationTreeWalker,
    cache: IUIAutomationCacheRequest,
}

impl Context {
    fn build() -> windows::core::Result<Self> {
        init_mta()?;
        // SAFETY: CUIAutomation is a registered in-process COM server; the
        // requested interface matches the class. Every call below is a
        // local, same-thread COM call against the instance just created:
        // build the "has no native window handle" condition, negate it, hand
        // the negation to a fresh tree walker, and build a cache request
        // that prefetches just the one property `NormalizeElementBuildCache`
        // needs to answer — exactly NVDA's `windowTreeWalker` and
        // `windowCacheRequest`.
        unsafe {
            let client: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let zero = VARIANT::from(0i32);
            let has_no_handle =
                client.CreatePropertyCondition(UIA_NativeWindowHandlePropertyId, &zero)?;
            let is_a_window = client.CreateNotCondition(&has_no_handle)?;
            let walker = client.CreateTreeWalker(&is_a_window)?;
            let cache = client.CreateCacheRequest()?;
            cache.AddProperty(UIA_NativeWindowHandlePropertyId)?;
            Ok(Self { walker, cache })
        }
    }
}

thread_local! {
    /// `None` until the first call on this thread; left `None` again after a
    /// failed build so a transient `CoCreateInstance` failure gets retried on
    /// the next call rather than permanently disabling this thread.
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
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
        // SAFETY: `element` is a live element per the caller's contract;
        // `context.walker` and `context.cache` are this thread's own, either
        // just built or reused unchanged from an earlier call on this same
        // thread.
        let normalized = unsafe {
            context
                .walker
                .NormalizeElementBuildCache(element, &context.cache)
        }
        .ok()?;
        // SAFETY: `normalized` was just built with `context.cache`, which
        // caches exactly the native window handle property this reads.
        let hwnd = unsafe { cached_native_window_handle(&normalized) };
        (hwnd != 0).then_some(hwnd)
    })
}
