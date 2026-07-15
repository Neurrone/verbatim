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
    CUIAutomation8, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern, IUIAutomationTogglePattern,
    IUIAutomationTreeWalker, TreeScope_Subtree, UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId,
    UIA_RuntimeIdPropertyId, UIA_TogglePatternId,
};

use verbatim_model::{NodeSnapshot, TreeNode};

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
        // SAFETY: CUIAutomation8 is a registered in-process COM server; the
        // requested interface matches the class. CUIAutomation8 rather than
        // the older CUIAutomation coclass because only the former's objects
        // implement the newer client interfaces — IUIAutomation5's
        // notification-event registration in particular, where querying a
        // plain CUIAutomation object fails with E_NOINTERFACE (observed
        // live; NVDA likewise creates CUIAutomation8).
        let client = unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)? };
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

    /// Walks the chain of ancestors of `element`, nearest first, via
    /// [`IUIAutomationTreeWalker::GetParentElementBuildCache`]: one hop at a
    /// time, each hop its own cross-process round trip using `cache` — the
    /// same per-hop walk NVDA shipped for years. Capped at `max_hops`
    /// ancestors; stops early (without error) when a hop finds no further
    /// parent. Cross-process; query-pool threads only, guarded by the
    /// caller's deadline since a hung provider can stall any hop.
    ///
    /// This is deliberately the simplest correct implementation, behind this
    /// method as a seam: milestone M4's remote-operations work
    /// (architecture section 4) replaces the per-hop walk with a single
    /// batched round trip executed inside the provider process. Callers
    /// should depend only on the result — the ordered ancestor list — never
    /// on how many round trips producing it took.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the tree walker itself cannot be created;
    /// a hop that finds no parent is not an error, it simply ends the walk.
    ///
    /// # Safety
    ///
    /// `element` must be a live element built with `cache`.
    pub unsafe fn ancestor_chain(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
        registry: &NodeIdRegistry,
        max_hops: u32,
    ) -> windows::core::Result<Vec<NodeSnapshot>> {
        // SAFETY: `self.client` is a live IUIAutomation instance.
        let walker = unsafe { self.client.RawViewWalker() }?;
        let mut chain = Vec::new();
        let mut current = element.clone();
        for _ in 0..max_hops {
            // SAFETY: `current` is either the caller's `element` (per its
            // contract) or a parent built with `cache` by the previous hop.
            let Ok(parent) = (unsafe { walker.GetParentElementBuildCache(&current, cache) })
            else {
                break;
            };
            // SAFETY: `parent` was just built with `cache`.
            let snapshot = unsafe { snapshot_from_cached_element(&parent, registry) };
            chain.push(snapshot);
            current = parent;
        }
        chain.reverse();
        Ok(chain)
    }

    /// Navigates one step from `element` in `direction`, via the raw-view
    /// tree walker's per-hop `*BuildCache` methods — a single cross-process
    /// round trip. Returns `Ok(None)` for a genuine "no such neighbor" (a
    /// root's parent, a last child's next sibling), a first-class outcome
    /// distinct from an error — the same convention [`Uia::walk_tree`]'s
    /// child-walk already relies on. Cross-process; query-pool threads only,
    /// guarded by the caller's deadline.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the tree walker itself cannot be created.
    ///
    /// # Safety
    ///
    /// `element` must be a live element built with `cache`.
    pub unsafe fn navigate(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
        registry: &NodeIdRegistry,
        direction: NavigateDirection,
    ) -> windows::core::Result<Option<NodeSnapshot>> {
        // SAFETY: `self.client` is a live IUIAutomation instance.
        let walker = unsafe { self.client.RawViewWalker() }?;
        // SAFETY: `element` and `cache` are valid per the caller's contract.
        let neighbor = unsafe {
            match direction {
                NavigateDirection::Parent => walker.GetParentElementBuildCache(element, cache),
                NavigateDirection::NextSibling => {
                    walker.GetNextSiblingElementBuildCache(element, cache)
                }
                NavigateDirection::PreviousSibling => {
                    walker.GetPreviousSiblingElementBuildCache(element, cache)
                }
                NavigateDirection::FirstChild => {
                    walker.GetFirstChildElementBuildCache(element, cache)
                }
            }
        };
        match neighbor {
            // SAFETY: `neighbor` was just built with `cache`.
            Ok(neighbor) => Ok(Some(unsafe {
                snapshot_from_cached_element(&neighbor, registry)
            })),
            Err(_) => Ok(None),
        }
    }

    /// Activates `element`: tries `Invoke`, then `Toggle`, then the legacy
    /// `DoDefaultAction` pattern, in that order — the same fallback ladder
    /// NVDA uses for "press the current object" against arbitrary UIA
    /// controls. Each pattern is fetched live (`GetCurrentPatternAs`, not a
    /// cached read), since activation is an infrequent, user-triggered
    /// action rather than something the base cache request prefetches.
    /// Cross-process; query-pool threads only, guarded by the caller's
    /// deadline.
    ///
    /// # Errors
    ///
    /// Returns the COM error from whichever pattern fetch or invocation
    /// failed, or a "not implemented" error if `element` exposes none of the
    /// three patterns.
    ///
    /// # Safety
    ///
    /// `element` must be a live element.
    pub unsafe fn activate(&self, element: &IUIAutomationElement) -> windows::core::Result<()> {
        // SAFETY: `element` is live per the caller's contract; each pattern
        // fetch fails safely (an error) when the pattern is unsupported.
        unsafe {
            if let Ok(invoke) =
                element.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            {
                return invoke.Invoke();
            }
            if let Ok(toggle) =
                element.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            {
                return toggle.Toggle();
            }
            if let Ok(legacy) = element
                .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                    UIA_LegacyIAccessiblePatternId,
                )
            {
                return legacy.DoDefaultAction();
            }
        }
        Err(windows::core::Error::new(
            windows::Win32::Foundation::E_NOTIMPL,
            "element exposes no Invoke, Toggle, or legacy DoDefaultAction pattern",
        ))
    }
}

/// A direction to navigate from an element with [`Uia::navigate`], mirroring
/// the object-navigation commands milestone M3 adds (roadmap: parent, next
/// and previous sibling, first child).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigateDirection {
    /// The element's parent.
    Parent,
    /// The next sibling in tree order.
    NextSibling,
    /// The previous sibling in tree order.
    PreviousSibling,
    /// The first child.
    FirstChild,
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
