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
        // The raw view, the same parent chain NVDA's own object hierarchy
        // walks; what gets *reported* out of it is filtered below.
        // SAFETY: `self.client` is a live IUIAutomation instance.
        let walker = unsafe { self.client.RawViewWalker() }?;
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
            // Non-presentable ancestors are crossed but never reported —
            // NVDA's `isPresentableFocusAncestor`, which filters spoken
            // focus context regardless of its review-mode setting (object
            // navigation, by contrast, sees the full tree; see
            // [`Uia::navigate`]).
            if is_presentable_focus_ancestor(&snapshot) {
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

    /// Navigates one step from `element` in `direction`, via the raw-view
    /// tree walker's per-hop `*BuildCache` methods — a single cross-process
    /// round trip, and deliberately the full, unfiltered tree.
    ///
    /// A recorded decision, made twice: object navigation matches NVDA
    /// with its simple review mode off — every element the raw view
    /// exposes is navigable, exactly the tree NVDA's own `baseTreeWalker`
    /// (also the raw view walker) exposes. An intermediate revision
    /// projected NVDA's simple-review filtering here instead; the user
    /// baselines against NVDA with the setting off, where full-tree
    /// navigation is the correct behavior, so the projection was removed.
    /// Spoken focus ancestry is a different matter: NVDA filters it by
    /// `isPresentableFocusAncestor` regardless of the review-mode setting,
    /// and [`Uia::ancestor_chain`] mirrors that.
    ///
    /// Returns `Ok(None)` for a genuine "no such neighbor" (a root's
    /// parent, a last child's next sibling), a first-class outcome distinct
    /// from an error. Cross-process; query-pool threads only, guarded by
    /// the caller's deadline.
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

/// NVDA's layout judgment (`presentationType`), ported from
/// `nvda/source/NVDAObjects/__init__.py` onto Verbatim's role vocabulary: a
/// layout element structures the tree without communicating anything on its
/// own. Unknown and pane roles are always layout; static text with no
/// readable text is layout; a window, property page, or grouping with
/// neither name nor description is layout. NVDA's "unavailable" tier
/// (invisible objects) is deliberately not ported: our `State::Offscreen`
/// maps UIA's `IsOffscreen`, which also marks scrolled-out but perfectly
/// real content.
///
/// Used only to filter *spoken focus ancestry* (see
/// [`is_presentable_focus_ancestor`]); object navigation deliberately sees
/// the full tree, matching NVDA with its simple review mode off (the
/// recorded decision on [`Uia::navigate`]).
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

/// NVDA's `isPresentableFocusAncestor`: whether an ancestor is worth
/// speaking as focus context. Layout elements are not; neither are roles
/// that never meaningfully contain the focus for announcement purposes —
/// list items, tree items, and editable text (NVDA also lists progress
/// bars, a role this vocabulary does not have yet). NVDA applies this to
/// focus ancestry regardless of its review-mode setting.
fn is_presentable_focus_ancestor(snapshot: &NodeSnapshot) -> bool {
    use verbatim_model::Role;
    if is_layout(snapshot) {
        return false;
    }
    !matches!(
        snapshot.role,
        Role::ListItem | Role::TreeItem | Role::EditableText
    )
}

#[cfg(test)]
mod presentation_tests {
    use verbatim_model::{Backend, NodeDetails, NodeId, NodeSnapshot, Role, StateSet};

    use super::{is_layout, is_presentable_focus_ancestor};

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

    #[test]
    fn item_and_text_roles_are_not_presentable_focus_ancestors() {
        assert!(!is_presentable_focus_ancestor(&snapshot(
            Role::ListItem,
            Some("Desktop"),
            None
        )));
        assert!(!is_presentable_focus_ancestor(&snapshot(
            Role::TreeItem,
            Some("Home"),
            None
        )));
        assert!(!is_presentable_focus_ancestor(&snapshot(
            Role::EditableText,
            Some("Search"),
            None
        )));
        assert!(is_presentable_focus_ancestor(&snapshot(
            Role::Group,
            Some("Quick access"),
            None
        )));
        assert!(!is_presentable_focus_ancestor(&snapshot(
            Role::Pane,
            Some("named"),
            None
        )));
    }
}
