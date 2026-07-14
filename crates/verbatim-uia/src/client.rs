//! The per-thread UIA client wrapper.
//!
//! Every outpost thread that talks to UIA — the focus-registration thread and
//! each query-pool worker — owns its own [`Uia`]. Construction joins the
//! multithreaded apartment (architecture section 4) and creates a fresh
//! `IUIAutomation`; the in-process client library gives each thread an
//! independent object, so nothing is shared across threads and there is no COM
//! marshaling hazard.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_ARRAY, VT_I4,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    TreeScope_Subtree, UIA_RuntimeIdPropertyId,
};

use crate::cache::base_cache_request;
use crate::com::init_mta;

/// A UIA client bound to the current thread's multithreaded apartment.
pub struct Uia {
    client: IUIAutomation,
}

impl Uia {
    /// Joins the multithreaded apartment and creates a UIA client on the
    /// current thread.
    ///
    /// # Errors
    ///
    /// Returns the COM error if apartment initialization or client creation
    /// fails.
    pub fn new() -> windows::core::Result<Self> {
        init_mta()?;
        // SAFETY: CUIAutomation is a registered in-process COM server; the
        // requested interface matches the class.
        let client = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)? };
        Ok(Self { client })
    }

    /// Borrows the underlying client for registration modules that need it.
    #[must_use]
    pub fn client(&self) -> &IUIAutomation {
        &self.client
    }

    /// Builds a base cache request bound to this client.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the cache request cannot be built.
    pub fn base_cache_request(&self) -> windows::core::Result<IUIAutomationCacheRequest> {
        base_cache_request(&self.client)
    }

    /// Fetches the currently focused element with the M1 properties prefetched.
    /// Runs a cross-process call, so callers must invoke it only on a
    /// deadline-guarded query-pool thread.
    ///
    /// # Errors
    ///
    /// Returns the COM error if there is no focused element or the call fails.
    pub fn focused_element(
        &self,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        // SAFETY: `cache` is a live cache request from this client; the call is
        // a normal cross-process fetch.
        unsafe { self.client.GetFocusedElementBuildCache(cache) }
    }

    /// Fetches the element for a top-level window handle with properties
    /// prefetched. Cross-process; query-pool threads only.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the handle has no UIA element or the call fails.
    pub fn element_from_handle(
        &self,
        hwnd: isize,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        // SAFETY: HWND wraps a caller-supplied handle; ElementFromHandleBuildCache
        // tolerates an invalid handle by returning an error.
        unsafe {
            self.client
                .ElementFromHandleBuildCache(HWND(hwnd as *mut _), cache)
        }
    }

    /// Re-fetches an element by its UIA runtime id, for answering a node
    /// re-read fetch. Returns `Ok(None)` when the element no longer exists.
    /// Cross-process; query-pool threads only.
    ///
    /// # Errors
    ///
    /// Returns the COM error if building the condition or the search fails for
    /// a reason other than the element being absent.
    pub fn element_by_runtime_id(
        &self,
        runtime_id: &[i32],
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<Option<IUIAutomationElement>> {
        if runtime_id.is_empty() {
            return Ok(None);
        }
        // SAFETY: a VT_ARRAY | VT_I4 VARIANT is built around a freshly created
        // i32 SAFEARRAY sized to the runtime id; the array is filled by index
        // within bounds. `VARIANT` has no Drop in this crate, so the array is
        // destroyed explicitly after `CreatePropertyCondition` copies the value
        // into the condition. The search walks from the root of this tree.
        unsafe {
            let count = u32::try_from(runtime_id.len()).unwrap_or(0);
            let array = SafeArrayCreateVector(VT_I4, 0, count);
            if array.is_null() {
                return Ok(None);
            }
            for (index, &value) in runtime_id.iter().enumerate() {
                let idx = i32::try_from(index).unwrap_or(0);
                let cell = value;
                let _ = SafeArrayPutElement(array, &raw const idx, (&raw const cell).cast());
            }
            let variant = VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                        vt: VARENUM(VT_ARRAY.0 | VT_I4.0),
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: VARIANT_0_0_0 { parray: array },
                    }),
                },
            };
            let root = self.client.GetRootElement()?;
            let condition = self
                .client
                .CreatePropertyCondition(UIA_RuntimeIdPropertyId, &variant);
            let _ = SafeArrayDestroy(array);
            let condition = condition?;
            match root.FindFirstBuildCache(TreeScope_Subtree, &condition, cache) {
                Ok(element) => Ok(Some(element)),
                Err(_) => Ok(None),
            }
        }
    }
}
