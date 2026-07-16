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
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_ARRAY, VT_I4,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern,
    IUIAutomationSelectionPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
    TreeScope_Subtree, UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId,
    UIA_RuntimeIdPropertyId, UIA_SelectionPatternId, UIA_TogglePatternId,
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

    /// Re-finds an element by its UIA runtime id inside `root`'s subtree,
    /// for answering a node re-read when no live element is cached (the
    /// registry's element cache is the fast path — see its module doc).
    /// Returns `Ok(None)` when the element no longer exists there.
    /// Cross-process; query-pool threads only.
    ///
    /// `root` scopes the search: `FindFirst` has no index behind it, so an
    /// unscoped search from the desktop root walks every application's
    /// tree — the cost that made pre-cache object navigation feel stuck on
    /// a busy desktop. Callers pass the target application's own top-level
    /// window element(s).
    ///
    /// # Errors
    ///
    /// Returns the COM error if building the condition or the search fails for
    /// a reason other than the element being absent.
    pub fn element_by_runtime_id(
        &self,
        root: &IUIAutomationElement,
        runtime_id: &[i32],
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<Option<IUIAutomationElement>> {
        if runtime_id.is_empty() {
            return Ok(None);
        }
        // SAFETY: a VT_ARRAY | VT_I4 VARIANT is built around a freshly created
        // i32 SAFEARRAY sized to the runtime id; the array is filled by index
        // within bounds. The VARIANT owns the array: `windows`'s `VARIANT`
        // has a `Drop` impl that calls `VariantClear`, which destroys the
        // `parray` for a VT_ARRAY variant, so the array is freed exactly once
        // when `variant` drops at the end of this scope — after
        // `CreatePropertyCondition` has copied it into the condition and after
        // the search below. It must NOT also be destroyed explicitly: that
        // was a double free (`SafeArrayDestroy` then `VariantClear` on the
        // same pointer), the heap corruption an outpost crash-loop traced to
        // this exact spot under the M3 focus-enrichment query. The search
        // walks the subtree under the caller's `root`.
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
            let condition = self
                .client
                .CreatePropertyCondition(UIA_RuntimeIdPropertyId, &variant)?;
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
        // The control view, not the raw view: ancestry feeds spoken
        // focus context and object navigation, and the raw view's purely
        // structural wrappers (unnamed lists, duplicated groupings —
        // observed live against Explorer's quick-access list) make spoken
        // chains feel broken. UIA documents the control view as the view
        // assistive technology should present; the raw view stays for
        // `walk_tree` (dump-tree), which is a debugging surface.
        // SAFETY: `self.client` is a live IUIAutomation instance.
        let walker = unsafe { self.client.ControlViewWalker() }?;
        let mut chain = Vec::new();
        let mut current = element.clone();
        for _ in 0..max_hops {
            // SAFETY: `current` is either the caller's `element` (per its
            // contract) or a parent built with `cache` by the previous hop.
            let Ok(parent) = (unsafe { walker.GetParentElementBuildCache(&current, cache) }) else {
                break;
            };
            // SAFETY: `parent` was just built with `cache`.
            let snapshot = unsafe { snapshot_from_cached_element(&parent, registry) };
            // Layout ancestors (see `is_layout`) are crossed but never
            // reported: spoken focus context and simple navigation must
            // agree on which containers exist.
            if !is_layout(&snapshot) {
                chain.push(snapshot);
            }
            current = parent;
        }
        chain.reverse();
        Ok(chain)
    }

    /// The first selected child of a selection container, via the
    /// container's `Selection` pattern: `GetCurrentSelection`, then the
    /// first element of the result rebuilt with `cache` so its snapshot
    /// reads entirely from cached properties. `Ok(None)` is every benign
    /// outcome — the element does not expose the pattern, or nothing is
    /// selected. Multi-selections report their first element; the reducer
    /// speaks one item, and richer multi-selection reporting is deliberately
    /// out of M3's scope. Cross-process; query-pool threads only, guarded
    /// by the caller's deadline.
    ///
    /// # Errors
    ///
    /// Never fails today: pattern and selection failures all map to
    /// `Ok(None)` because "no reportable selection" is the correct reading
    /// of each. The `Result` stays in the signature so a genuinely
    /// distinguishable failure can surface later without breaking callers.
    ///
    /// # Safety
    ///
    /// `element` must be a live element built with `cache`.
    pub unsafe fn selected_child(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
        registry: &NodeIdRegistry,
    ) -> windows::core::Result<Option<NodeSnapshot>> {
        // SAFETY: `element` is live per the caller's contract; a missing
        // pattern surfaces as an error mapped to None.
        let Ok(pattern) = (unsafe {
            element.GetCurrentPatternAs::<IUIAutomationSelectionPattern>(UIA_SelectionPatternId)
        }) else {
            return Ok(None);
        };
        // SAFETY: `pattern` was just obtained from a live element.
        let Ok(selection) = (unsafe { pattern.GetCurrentSelection() }) else {
            return Ok(None);
        };
        // SAFETY: `selection` is a live element array.
        if unsafe { selection.Length() }.unwrap_or(0) == 0 {
            return Ok(None);
        }
        // SAFETY: index 0 exists per the length check above.
        let Ok(first) = (unsafe { selection.GetElement(0) }) else {
            return Ok(None);
        };
        // SAFETY: `first` is live; rebuilding with `cache` prefetches the
        // full snapshot property set in one round trip.
        let Ok(cached) = (unsafe { first.BuildUpdatedCache(cache) }) else {
            return Ok(None);
        };
        // SAFETY: `cached` was just built with `cache`.
        Ok(Some(unsafe {
            snapshot_from_cached_element(&cached, registry)
        }))
    }

    /// Navigates one step from `element` in `direction`, with NVDA's simple
    /// navigation semantics over the control view: purely presentational
    /// ("layout") elements are never landed on. Parent walks up to the first
    /// content ancestor; first-child descends through layout containers to
    /// the first content descendant; a sibling walk treats a layout
    /// sibling's content children as siblings (the layout container is
    /// entered, not announced) and bubbles up through layout parents when a
    /// level is exhausted — the projection NVDA's `_findSimpleNext`
    /// implements, ported from `nvda/source/NVDAObjects/__init__.py`.
    /// Confirmed necessary live against Explorer: its control view is full
    /// of unnamed lists and doubled groupings that make unfiltered
    /// navigation feel broken.
    ///
    /// Returns `Ok(None)` for a genuine "no such neighbor" (a root's
    /// parent, a last content sibling's next). Cross-process, several
    /// walker hops per call, bounded by [`SIMPLE_NAV_HOP_BUDGET`];
    /// query-pool threads only, guarded by the caller's deadline.
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
        let walker = unsafe { self.client.ControlViewWalker() }?;
        let mut budget = SIMPLE_NAV_HOP_BUDGET;
        // SAFETY: forwarded from the caller's contract.
        let found = unsafe {
            match direction {
                NavigateDirection::Parent => {
                    simple_parent(&walker, element, cache, registry, &mut budget)
                }
                NavigateDirection::FirstChild => {
                    simple_first_child(&walker, element, cache, registry, &mut budget)
                }
                NavigateDirection::NextSibling => find_simple_next(
                    element,
                    &walker,
                    cache,
                    registry,
                    false,
                    false,
                    true,
                    &mut budget,
                ),
                NavigateDirection::PreviousSibling => find_simple_next(
                    element,
                    &walker,
                    cache,
                    registry,
                    true,
                    false,
                    true,
                    &mut budget,
                ),
            }
        };
        Ok(found.map(|(_, snapshot)| snapshot))
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

/// Cap on cross-process walker hops one simple-navigation call may spend
/// crossing layout runs. Generous — observed layout runs are a handful of
/// elements — while still bounding a pathological provider; a call that
/// exhausts it reports "no neighbor" rather than wandering forever.
const SIMPLE_NAV_HOP_BUDGET: u32 = 64;

/// NVDA's layout judgment (`presentationType`), ported from
/// `nvda/source/NVDAObjects/__init__.py` onto Verbatim's role vocabulary: a
/// layout element structures the tree without communicating anything a
/// user navigates for, so simple navigation never lands on one. Unknown
/// and pane roles are always layout; static text with no readable text is
/// layout; a window, property page, or grouping with neither name nor
/// description is layout. NVDA's "unavailable" tier (invisible objects) is
/// deliberately not ported: our `State::Offscreen` maps UIA's `IsOffscreen`,
/// which also marks scrolled-out but perfectly real content.
fn is_layout(snapshot: &NodeSnapshot) -> bool {
    use verbatim_model::Role;
    fn blank(text: Option<&str>) -> bool {
        text.is_none_or(|text| text.trim().is_empty())
    }
    match snapshot.role {
        Role::Unknown | Role::Pane => true,
        Role::StaticText => blank(snapshot.name.as_deref()),
        Role::Window | Role::PropertyPage | Role::Group => {
            blank(snapshot.name.as_deref()) && blank(snapshot.details.description.as_deref())
        }
        _ => false,
    }
}

/// One raw control-view walker hop, `None` at a tree edge. Decrements
/// `budget` and reports the edge once it is spent.
///
/// # Safety
///
/// `element` must be a live element built with `cache`.
unsafe fn hop(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    cache: &IUIAutomationCacheRequest,
    kind: Hop,
    budget: &mut u32,
) -> Option<IUIAutomationElement> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    // SAFETY: forwarded from this function's contract; an `Err` from any
    // walker step means "no element there", the convention the whole file
    // uses.
    unsafe {
        match kind {
            Hop::Parent => walker.GetParentElementBuildCache(element, cache),
            Hop::Next => walker.GetNextSiblingElementBuildCache(element, cache),
            Hop::Previous => walker.GetPreviousSiblingElementBuildCache(element, cache),
            Hop::FirstChild => walker.GetFirstChildElementBuildCache(element, cache),
            Hop::LastChild => walker.GetLastChildElementBuildCache(element, cache),
        }
    }
    .ok()
}

/// The raw walker steps [`hop`] can take.
#[derive(Clone, Copy)]
enum Hop {
    Parent,
    Next,
    Previous,
    FirstChild,
    LastChild,
}

/// The first content ancestor of `element`: raw parent hops, skipping
/// layout, `None` at the root.
///
/// # Safety
///
/// `element` must be a live element built with `cache`.
unsafe fn simple_parent(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    cache: &IUIAutomationCacheRequest,
    registry: &NodeIdRegistry,
    budget: &mut u32,
) -> Option<(IUIAutomationElement, NodeSnapshot)> {
    // SAFETY: forwarded from this function's contract; every hop result was
    // built with `cache`.
    unsafe {
        let mut current = hop(walker, element, cache, Hop::Parent, budget)?;
        loop {
            let snapshot = snapshot_from_cached_element(&current, registry);
            if !is_layout(&snapshot) {
                return Some((current, snapshot));
            }
            current = hop(walker, &current, cache, Hop::Parent, budget)?;
        }
    }
}

/// The first content child of `element`: the raw first child, or — when
/// that child is layout — its first content descendant via
/// [`find_simple_next`] entering it. NVDA's `simpleFirstChild`.
///
/// # Safety
///
/// `element` must be a live element built with `cache`.
unsafe fn simple_first_child(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    cache: &IUIAutomationCacheRequest,
    registry: &NodeIdRegistry,
    budget: &mut u32,
) -> Option<(IUIAutomationElement, NodeSnapshot)> {
    // SAFETY: forwarded from this function's contract.
    unsafe {
        let child = hop(walker, element, cache, Hop::FirstChild, budget)?;
        let snapshot = snapshot_from_cached_element(&child, registry);
        if !is_layout(&snapshot) {
            return Some((child, snapshot));
        }
        find_simple_next(&child, walker, cache, registry, false, true, false, budget)
    }
}

/// NVDA's `_findSimpleNext`, ported: the next (or previous) content
/// element in the simple projection of the tree rooted around `element`.
/// With `use_child`, `element`'s own subtree is searched first (entering a
/// layout container instead of announcing it); with `use_parent`, an
/// exhausted sibling level bubbles up through layout parents. The argument
/// order after `element` mirrors the walker-first style of the other
/// helpers.
///
/// # Safety
///
/// `element` must be a live element built with `cache`.
#[allow(
    clippy::too_many_arguments,
    reason = "a faithful port of NVDA's four-flag recursion; bundling the flags into a struct would only rename them"
)]
unsafe fn find_simple_next(
    element: &IUIAutomationElement,
    walker: &IUIAutomationTreeWalker,
    cache: &IUIAutomationCacheRequest,
    registry: &NodeIdRegistry,
    go_previous: bool,
    use_child: bool,
    use_parent: bool,
    budget: &mut u32,
) -> Option<(IUIAutomationElement, NodeSnapshot)> {
    let child_hop = if go_previous {
        Hop::LastChild
    } else {
        Hop::FirstChild
    };
    let sibling_hop = if go_previous {
        Hop::Previous
    } else {
        Hop::Next
    };

    // SAFETY: forwarded from this function's contract; every hop result was
    // built with `cache`.
    unsafe {
        if use_child && let Some(child) = hop(walker, element, cache, child_hop, budget) {
            let snapshot = snapshot_from_cached_element(&child, registry);
            let found = if is_layout(&snapshot) {
                find_simple_next(
                    &child,
                    walker,
                    cache,
                    registry,
                    go_previous,
                    true,
                    false,
                    budget,
                )
            } else {
                Some((child, snapshot))
            };
            if found.is_some() {
                return found;
            }
        }

        if let Some(sibling) = hop(walker, element, cache, sibling_hop, budget) {
            let snapshot = snapshot_from_cached_element(&sibling, registry);
            let found = if is_layout(&snapshot) {
                find_simple_next(
                    &sibling,
                    walker,
                    cache,
                    registry,
                    go_previous,
                    true,
                    false,
                    budget,
                )
            } else {
                Some((sibling, snapshot))
            };
            if found.is_some() {
                return found;
            }
        }

        if !use_parent {
            return None;
        }
        let mut parent = hop(walker, element, cache, Hop::Parent, budget)?;
        loop {
            let snapshot = snapshot_from_cached_element(&parent, registry);
            if !is_layout(&snapshot) {
                return None;
            }
            if let Some(found) = find_simple_next(
                &parent,
                walker,
                cache,
                registry,
                go_previous,
                false,
                false,
                budget,
            ) {
                return Some(found);
            }
            parent = hop(walker, &parent, cache, Hop::Parent, budget)?;
        }
    }
}

#[cfg(test)]
mod simple_nav_tests {
    use verbatim_model::{Backend, NodeDetails, NodeId, NodeSnapshot, Role, StateSet};

    use super::is_layout;

    fn snapshot(role: Role, name: Option<&str>, description: Option<&str>) -> NodeSnapshot {
        let details = NodeDetails {
            description: description.map(str::to_owned),
            ..NodeDetails::default()
        };
        NodeSnapshot {
            id: NodeId::new(1),
            backend: Backend::Uia,
            role,
            name: name.map(str::to_owned),
            value: None,
            states: StateSet::default(),
            details,
        }
    }

    #[test]
    fn panes_and_unknowns_are_always_layout() {
        assert!(is_layout(&snapshot(Role::Pane, Some("named"), None)));
        assert!(is_layout(&snapshot(Role::Unknown, Some("named"), None)));
    }

    #[test]
    fn unnamed_groupings_and_windows_are_layout_but_named_ones_are_content() {
        assert!(is_layout(&snapshot(Role::Group, None, None)));
        assert!(is_layout(&snapshot(Role::Window, Some("  "), None)));
        assert!(!is_layout(&snapshot(
            Role::Group,
            Some("Quick access"),
            None
        )));
        assert!(!is_layout(&snapshot(Role::Group, None, Some("described"))));
    }

    #[test]
    fn blank_static_text_is_layout_and_real_text_is_content() {
        assert!(is_layout(&snapshot(Role::StaticText, None, None)));
        assert!(!is_layout(&snapshot(
            Role::StaticText,
            Some("3 items"),
            None
        )));
    }

    #[test]
    fn interactive_roles_are_content_even_unnamed() {
        assert!(!is_layout(&snapshot(Role::List, None, None)));
        assert!(!is_layout(&snapshot(Role::ListItem, None, None)));
        assert!(!is_layout(&snapshot(Role::Button, None, None)));
    }
}
