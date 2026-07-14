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
    IUIAutomationTreeWalker, TreeScope_Subtree, UIA_RuntimeIdPropertyId,
};

use verbatim_model::TreeNode;

use crate::cache::base_cache_request;
use crate::com::init_mta;
use crate::map::snapshot_from_cached_element;
use crate::registry::NodeIdRegistry;

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

    /// Walks the raw-view subtree rooted at `element` (already built with
    /// `cache`), bounded by `max_depth` (the root is depth 0) and
    /// `max_nodes` (the total number of nodes across the whole walk,
    /// including the root). Cross-process; query-pool threads only, guarded
    /// by the caller's deadline since a hung provider can stall any step.
    /// Returns the walked tree and whether either cap was hit before the
    /// walk reached every node.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the tree walker cannot be created.
    ///
    /// # Safety
    ///
    /// `element` must be a live element built with `cache`.
    pub unsafe fn walk_tree(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
        registry: &NodeIdRegistry,
        max_depth: u32,
        max_nodes: usize,
    ) -> windows::core::Result<(TreeNode, bool)> {
        // SAFETY: `self.client` is a live IUIAutomation instance.
        let walker = unsafe { self.client.RawViewWalker() }?;
        let limits = WalkLimits {
            cache,
            registry,
            max_depth,
            max_nodes,
        };
        let mut state = WalkState {
            visited: 1, // the root counts as one node.
            truncated: false,
        };
        // SAFETY: `element` and `cache` are valid per the caller's contract;
        // `walker` was just created and is used only within this call.
        let root = unsafe { walk_recursive(&walker, element, &limits, 0, &mut state) };
        Ok((root, state.truncated))
    }
}

/// The per-walk parameters threaded through every level of
/// [`walk_recursive`]: the caching and identity plumbing plus the walk's
/// caps.
struct WalkLimits<'a> {
    cache: &'a IUIAutomationCacheRequest,
    registry: &'a NodeIdRegistry,
    max_depth: u32,
    max_nodes: usize,
}

/// Mutable state accumulated across the whole walk, shared by every level
/// of recursion.
struct WalkState {
    visited: usize,
    truncated: bool,
}

/// Recursive worker for [`Uia::walk_tree`]. `state.visited` already counts
/// `element` itself; children are counted as they are accepted into the
/// walk, before recursing into them.
///
/// # Safety
///
/// `walker` must be a live tree walker; `element` and `limits.cache` must
/// satisfy [`Uia::walk_tree`]'s contract.
unsafe fn walk_recursive(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    limits: &WalkLimits<'_>,
    depth: u32,
    state: &mut WalkState,
) -> TreeNode {
    // SAFETY: `element` carries every base-cache-request property, either as
    // the walk's root (caller's contract) or because every child reached
    // below is fetched with a `*BuildCache` call using the same cache
    // request.
    let snapshot = unsafe { snapshot_from_cached_element(element, limits.registry) };

    if depth >= limits.max_depth {
        // Peek only: is there a child we are declining to descend into?
        // SAFETY: forwarded to this function's contract.
        if unsafe { walker.GetFirstChildElementBuildCache(element, limits.cache) }.is_ok() {
            state.truncated = true;
        }
        return TreeNode {
            snapshot,
            children: Vec::new(),
        };
    }

    let mut children = Vec::new();
    // SAFETY: forwarded to this function's contract; a `Err` here means "no
    // first child", the same convention `element_by_runtime_id` uses for
    // `FindFirstBuildCache`.
    let mut next_child =
        unsafe { walker.GetFirstChildElementBuildCache(element, limits.cache) }.ok();
    while let Some(current) = next_child {
        if state.visited >= limits.max_nodes {
            state.truncated = true;
            break;
        }
        state.visited += 1;
        // SAFETY: `current` was built with `limits.cache` by the call above
        // or below; forwarded to this function's own contract otherwise.
        let child_node = unsafe { walk_recursive(walker, &current, limits, depth + 1, state) };
        children.push(child_node);
        // SAFETY: forwarded; `Err` means "no next sibling".
        next_child = unsafe { walker.GetNextSiblingElementBuildCache(&current, limits.cache) }.ok();
    }

    TreeNode { snapshot, children }
}
