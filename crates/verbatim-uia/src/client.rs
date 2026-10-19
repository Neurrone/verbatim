//! The per-thread UIA client wrapper.
//!
//! Every outpost thread that talks to UIA — the focus-registration thread and
//! the outpost's worker — owns its own [`Uia`]. Construction joins the
//! multithreaded apartment (architecture section 4) and creates a fresh
//! `IUIAutomation`; the in-process client library gives each thread an
//! independent object, so nothing is shared across threads and there is no COM
//! marshaling hazard.

use windows::Win32::Foundation::{E_INVALIDARG, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Variant::{InitVariantFromInt32Array, VARIANT};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, CoalesceEventsOptions_Enabled, ConnectionRecoveryBehaviorOptions_Enabled,
    IUIAutomation, IUIAutomation2, IUIAutomation6, IUIAutomationCacheRequest,
    IUIAutomationCondition, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
    TreeScope_Children, TreeScope_Subtree, UIA_InvokePatternId, UIA_PROPERTY_ID,
    UIA_RuntimeIdPropertyId, UIA_SelectionItemPatternId, UIA_TogglePatternId,
};

use windows::core::Interface;

use verbatim_model::{CallKind, NodeSnapshot, QueryKind, TreeNode};

use crate::calls::count;
use crate::com::init_mta;
use crate::element::{ElementExt, current_selection, invoke, select, toggle};
use crate::registry::NodeIdRegistry;
use crate::shape::{
    Ancestor, Cached, Direction, ElementValue, Live, Node, Request, SelectionContainer, Shape,
    TreeDump,
};

/// A UIA client bound to the current thread's multithreaded apartment.
pub struct Uia {
    client: IUIAutomation,
}

/// Where an ancestor walk ([`Uia::ancestors`]) stops short of the root.
#[derive(Clone, Copy)]
pub struct AncestorStops<'a> {
    /// Whether a window is read through the other API: the walk stops at
    /// its root element and returns its handle.
    pub read_by_other_api: &'a dyn Fn(isize) -> bool,
    /// Whether a reported ancestor is already known: the walk stops there.
    pub known: &'a dyn Fn(verbatim_model::NodeId) -> bool,
    /// Whether the walk's time is up, checked before each hop and after a
    /// hop that fails: it then gives up, reporting the chain as incomplete.
    pub out_of_time: Option<&'a dyn Fn() -> bool>,
}

/// How an ancestor walk ended ([`Uia::ancestors`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AncestorWalk {
    /// It reached the root, a window read through the other API, or a hop
    /// that found no parent.
    Complete,
    /// It reached this known ancestor, the outermost one in the chain.
    MetKnown(verbatim_model::NodeId),
    /// It ran out of time, so the chain is incomplete.
    OutOfTime,
}

/// One ancestor as another process read it from its cached element
/// ([`Uia::ancestor_chain_from_parts`]): its parts, its own native window
/// handle (0 for none), and whether UIA counts it both a control and
/// content ([`Ancestor::is_control_and_content`]).
#[derive(Clone, Debug, PartialEq)]
pub struct AncestorParts {
    /// The ancestor's cached parts.
    pub parts: crate::map::CachedUiaParts,
    /// Its own native window handle, 0 for an element that is not a window.
    pub hwnd: isize,
    /// Whether it is both a control and content element.
    pub content: bool,
}

/// How long UIA waits to connect to an application's provider before it
/// ends the call with `UIA_E_TIMEOUT`: UIA's connection timeout, which
/// bounds a lookup by handle (`ElementFromHandle`), the focused element's
/// lookup, and a first call on an element, and which UIA's own focus
/// tracking waits on twice for a hung window named in a focus `WinEvent`.
/// Two seconds, UIA's own default and NVDA's
/// (`docs/design/listener-without-connections.md`, section 5). Like
/// [`TRANSACTION_TIMEOUT`] it is a setting of the process, not of a
/// client, so it is set once per process, by UIA's first-time setup
/// ([`ensure_ready`]), and creating a client leaves it as it is (measured
/// 2026-10-11).
pub const CONNECTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// How long one call into a provider already connected waits before UIA
/// ends it with `UIA_E_TIMEOUT`: UIA's transaction timeout, which bounds a
/// property read, a cache build, or a remote operation on a held element.
/// Set once per process with [`CONNECTION_TIMEOUT`]. Five seconds is under
/// the outpost worker's ten-second deadline, so a slow call fails and the
/// worker carries on rather than being abandoned, and over the two to
/// three seconds an application that is starting up can take to answer
/// (`docs/design/focus-pipeline.md`, section 6.1; Dickson, 2026-10-10). It
/// bounds each provider call, not an operation: a search such as
/// `FindFirst` waits a full timeout for each call it makes that the
/// application does not answer, so the outpost gives such operations a
/// deadline of their own.
pub const TRANSACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl Uia {
    /// Joins the multithreaded apartment and creates a UIA client on the
    /// current thread. Each of its calls waits up to [`CONNECTION_TIMEOUT`]
    /// to connect to an application's provider and [`TRANSACTION_TIMEOUT`]
    /// for it to answer.
    ///
    /// # Errors
    ///
    /// Returns the COM error if apartment initialization or client creation
    /// fails.
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            client: create_client()?,
        })
    }

    /// Borrows the underlying client for registration modules that need it.
    #[must_use]
    pub fn client(&self) -> &IUIAutomation {
        &self.client
    }

    /// The cache request of `S`, a shape with no gates
    /// (`docs/design/uia-shapes.md`). Local.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the cache request cannot be built.
    pub fn request_fixed<S: crate::Ungated>(&self) -> windows::core::Result<crate::Request<S>> {
        crate::Request::build(&self.client, ())
    }

    /// The cache request of `S` for `gates`, which every element it fills
    /// carries (`docs/design/uia-shapes.md`). Local.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the cache request cannot be built.
    pub fn request<S: Shape>(&self, gates: S::Gates) -> windows::core::Result<Request<S>> {
        Request::build(&self.client, gates)
    }

    /// The raw-view tree walker, the full tree NVDA's own walker uses.
    /// Local.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the walker cannot be created.
    pub fn raw_view_walker(&self) -> windows::core::Result<IUIAutomationTreeWalker> {
        // SAFETY: `self.client` is a live IUIAutomation; creating a walker
        // takes no arguments.
        unsafe { self.client.RawViewWalker() }
    }

    /// A tree walker over UIA's control view, which leaves out elements
    /// that are only layout. Local.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the walker cannot be created.
    pub fn control_view_walker(&self) -> windows::core::Result<IUIAutomationTreeWalker> {
        // SAFETY: `self.client` is a live IUIAutomation; creating a walker
        // takes no arguments.
        unsafe { self.client.ControlViewWalker() }
    }

    /// The desktop's root element, which UIA serves in this process, so the
    /// call is local.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the root cannot be read.
    pub fn root_element(&self) -> windows::core::Result<IUIAutomationElement> {
        // SAFETY: `self.client` is a live IUIAutomation; the call takes no
        // arguments.
        unsafe { self.client.GetRootElement() }
    }

    /// Whether UI Automation counts `first` and `second` as one element
    /// (`CompareElements`), as NVDA compares a focus event's sender with
    /// its last focus. Counted as one cross-process call: UI Automation can
    /// ask the providers for their runtime ids.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the comparison fails.
    pub fn compare_elements(
        &self,
        first: &IUIAutomationElement,
        second: &IUIAutomationElement,
    ) -> windows::core::Result<bool> {
        count(CallKind::Uia);
        // SAFETY: `self.client` is a live IUIAutomation, and both elements
        // are live elements.
        unsafe { self.client.CompareElements(first, second) }.map(windows_core::BOOL::as_bool)
    }

    /// A condition matching elements whose `property` equals `value`.
    /// Local.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the condition cannot be built.
    pub fn property_condition(
        &self,
        property: UIA_PROPERTY_ID,
        value: &VARIANT,
    ) -> windows::core::Result<IUIAutomationCondition> {
        // SAFETY: `self.client` is a live IUIAutomation and `value` a VARIANT
        // the caller owns, which the condition copies.
        unsafe { self.client.CreatePropertyCondition(property, value) }
    }

    /// Fetches the currently focused element with the M1 properties prefetched.
    /// Runs a cross-process call, so callers must invoke it only on a
    /// deadline-guarded worker. An element served only by UIA's own proxies
    /// is refused (`crate::answered`, the proxy rule).
    ///
    /// # Errors
    ///
    /// Returns the COM error if there is no focused element or the call
    /// fails, and an error when the proxy rule refuses the element.
    fn focused_element(
        &self,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia);
        crate::calls::count_connection();
        // A new connection to the focused application, made only by that
        // application's outpost (`docs/design/listener-without-connections.md`,
        // section 2).
        let cache = crate::answered::with_description(cache)?;
        // SAFETY: `cache` is a live cache request from this client; the
        // call is a normal cross-process fetch.
        let element = unsafe { self.client.GetFocusedElementBuildCache(&cache) }?;
        crate::answered::checked(0, element)
    }

    /// The element that has the keyboard focus, its cache filled by
    /// `request`. Cross-process.
    ///
    /// # Errors
    ///
    /// Returns the COM error if there is no focused element or the call fails.
    pub fn focused<S: Shape>(&self, request: &Request<S>) -> windows::core::Result<Cached<S>> {
        let (cache, gates) = request.parts();
        Ok(Cached::filled(self.focused_element(cache)?, gates))
    }

    /// `parent`'s children in the raw view, in order, each with its cache
    /// filled by `request`, in one call: `parent`'s cache is rebuilt
    /// through a copy of `request` scoped to its children alone, as NVDA
    /// fetches children, so `parent` itself is filled with nothing, and the
    /// children are read from it. Cross-process; the outpost's worker only.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the cache request cannot be copied or the
    /// element does not answer.
    pub fn children<S: Shape>(
        &self,
        parent: &Live,
        request: &Request<S>,
    ) -> windows::core::Result<Vec<Cached<S>>> {
        let (cache, gates) = request.parts();
        // SAFETY: `cache` is a live cache request; the copy is this call's.
        let scoped = unsafe { cache.Clone() }?;
        Ok(self
            .children_from(parent.element(), &scoped)?
            .into_iter()
            .map(|child| Cached::filled(child, gates))
            .collect())
    }

    /// `element`'s raw-view children, read by rebuilding its cache with
    /// `request` set to its children alone in the raw view.
    fn children_from(
        &self,
        element: &IUIAutomationElement,
        request: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<Vec<IUIAutomationElement>> {
        // SAFETY: `request` is a live cache request; the scope is a plain
        // value.
        unsafe { request.SetTreeScope(TreeScope_Children) }?;
        // SAFETY: `self.client` is a live IUIAutomation; the call takes no
        // arguments.
        let raw_view = unsafe { self.client.RawViewCondition() }?;
        // SAFETY: `request` is a live cache request and `raw_view` a live
        // condition from the same client.
        unsafe { request.SetTreeFilter(&raw_view) }?;
        let fresh = element.build_updated_cache(request)?;
        // SAFETY: `fresh` is a live element whose cache was just built; an
        // element with no children answers a null array, which `windows`
        // reports as an error carrying no failure code.
        match unsafe { fresh.GetCachedChildren() } {
            Ok(array) => Ok(crate::element::elements_of(&array)),
            Err(error) if error.code().is_ok() => Ok(Vec::new()),
            Err(error) => {
                crate::calls::count_failed_read();
                Err(error)
            }
        }
    }

    /// Fetches the element for a top-level window handle with properties
    /// prefetched. Cross-process; the outpost's worker only, for its own
    /// application's windows that have a provider, so an element served
    /// only by UIA's own proxies is refused (`crate::answered`, the proxy
    /// rule).
    ///
    /// # Errors
    ///
    /// Returns the COM error if the handle has no UIA element or the call
    /// fails, and an error when the proxy rule refuses the element.
    pub(crate) fn element_from_handle(
        &self,
        hwnd: isize,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        let cache = crate::answered::with_description(cache)?;
        crate::answered::checked(hwnd, self.any_element_from_handle(hwnd, &cache)?)
    }

    /// The element for a window handle as UIA serves it, by its own proxies
    /// for a window with no provider, with no proxy rule. Cross-process.
    fn any_element_from_handle(
        &self,
        hwnd: isize,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia);
        crate::calls::count_connection();
        // A new connection to the window by its handle, which returns UIA's
        // client-side window proxy in place of the application's element
        // when it arrives while a remote operation from another process
        // waits to be evaluated there; only the window's own application's
        // outpost makes it (`docs/design/listener-without-connections.md`,
        // section 2).
        // SAFETY: HWND wraps a caller-supplied handle;
        // ElementFromHandleBuildCache tolerates an invalid handle by
        // returning an error.
        unsafe {
            self.client
                .ElementFromHandleBuildCache(HWND(hwnd as *mut _), cache)
        }
    }

    /// [`element_from_handle`](Self::element_from_handle), the element's
    /// cache filled by `request`. Cross-process.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the handle has no UIA element or the call fails.
    pub fn window_element<S: crate::Shape>(
        &self,
        hwnd: isize,
        request: &crate::Request<S>,
    ) -> windows::core::Result<crate::Cached<S>> {
        let (cache, gates) = request.parts();
        Ok(crate::Cached::filled(
            self.element_from_handle(hwnd, cache)?,
            gates,
        ))
    }

    /// The element for a window handle as UIA serves it, its cache filled
    /// by `request`, with no proxy rule: for a window with no provider,
    /// whose element is UIA's own proxies, as a test or a check of such a
    /// window reads it. Cross-process.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the handle has no UIA element or the call fails.
    pub fn any_window_element<S: crate::Shape>(
        &self,
        hwnd: isize,
        request: &crate::Request<S>,
    ) -> windows::core::Result<crate::Cached<S>> {
        let (cache, gates) = request.parts();
        Ok(crate::Cached::filled(
            self.any_element_from_handle(hwnd, cache)?,
            gates,
        ))
    }

    /// The element for a top-level window handle, with no cache: for a
    /// caller that reads nothing of it, and only searches under it or
    /// calls into it. Cross-process.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the handle has no UIA element or the call fails.
    pub fn window_live(&self, hwnd: isize) -> windows::core::Result<Live> {
        count(CallKind::Uia);
        crate::calls::count_connection();
        // SAFETY: HWND wraps a caller-supplied handle; ElementFromHandle
        // tolerates an invalid handle by returning an error.
        unsafe { self.client.ElementFromHandle(HWND(hwnd as *mut _)) }.map(Live::new)
    }

    /// Re-finds the element `runtime_id` names inside `root`'s subtree, its
    /// cache filled by `request`, for answering a node re-read when no live
    /// element is kept (the registry's kept element is the fast path; see
    /// its module doc); `Ok(None)` when it is not there. `root` scopes the
    /// search: `FindFirst` has no index behind it, so an unscoped search
    /// from the desktop root walks every application's tree, the cost that
    /// made object navigation feel stuck on a busy desktop before elements
    /// were kept. Callers pass the target application's own top-level
    /// window elements. Cross-process; the outpost's worker only.
    ///
    /// # Errors
    ///
    /// Returns the COM error if building the condition or the search fails for
    /// a reason other than the element being absent.
    pub fn find_by_runtime_id<S: Shape>(
        &self,
        root: &Live,
        runtime_id: &[i32],
        request: &Request<S>,
    ) -> windows::core::Result<Option<Cached<S>>> {
        let (cache, gates) = request.parts();
        Ok(self
            .element_by_runtime_id(root.element(), runtime_id, cache)?
            .map(|found| Cached::filled(found, gates)))
    }

    /// The element `runtime_id` names, its cache filled by `request`, when
    /// it is a descendant of one of the elements `focused` names in its UIA
    /// `ControllerFor` relation: NVDA's test for a selection in a list the
    /// focus controls, such as a search result while the focus stays in
    /// the search box. `Ok(None)` when `focused` controls nothing or the
    /// element is in none of what it controls. The classic implementation
    /// behind `verbatim_uia_rops::controlled_selection`. Cross-process; the
    /// outpost's worker only, guarded by the caller's deadline.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the relation cannot be read or a search
    /// fails for a reason other than the element being absent.
    pub fn controlled<S: Shape>(
        &self,
        focused: &Live,
        runtime_id: &[i32],
        request: &Request<S>,
    ) -> windows::core::Result<Option<Cached<S>>> {
        let (cache, gates) = request.parts();
        Ok(self
            .controlled_descendant(focused.element(), runtime_id, cache)?
            .map(|found| Cached::filled(found, gates)))
    }

    /// Re-finds an element by its UIA runtime id inside `root`'s subtree,
    /// for answering a node re-read when no live element is cached (the
    /// registry's element cache is the fast path — see its module doc).
    /// Returns `Ok(None)` when the element no longer exists there.
    /// Cross-process; the outpost's worker only.
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
    fn element_by_runtime_id(
        &self,
        root: &IUIAutomationElement,
        runtime_id: &[i32],
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<Option<IUIAutomationElement>> {
        if runtime_id.is_empty() {
            return Ok(None);
        }
        // SAFETY: `InitVariantFromInt32Array` returns a VT_ARRAY | VT_I4
        // VARIANT that owns its array; `windows`'s `VARIANT` frees it once,
        // in its `Drop`, after `CreatePropertyCondition` has copied it into
        // the condition. Nothing else may destroy the array: doing so as well
        // was a double free, the heap corruption an outpost crash-loop traced
        // to this spot under the M3 focus-enrichment query.
        let variant = unsafe { InitVariantFromInt32Array(runtime_id) }?;
        let condition = self.property_condition(UIA_RuntimeIdPropertyId, &variant)?;
        // The search walks the subtree under the caller's `root`.
        root.find_first_build_cache(TreeScope_Subtree, &condition, cache)
    }

    /// The element `runtime_id` names, rebuilt with `cache`, when it is a
    /// descendant of one of the elements `focused` names in its UIA
    /// `ControllerFor` relation: NVDA's test for a selection in a list the
    /// focus controls, such as a search result while the focus stays in
    /// the search box. `Ok(None)` when `focused` controls nothing or the
    /// element is in none of what it controls. Cross-process; the outpost's
    /// worker only, guarded by the caller's deadline.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the relation cannot be read or a search
    /// fails for a reason other than the element being absent.
    fn controlled_descendant(
        &self,
        focused: &IUIAutomationElement,
        runtime_id: &[i32],
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<Option<IUIAutomationElement>> {
        for root in focused.controller_for()? {
            // A descendant, not the controlled element itself.
            // A failed read is no runtime id.
            let root_id = crate::com::runtime_id(&root);
            if root_id == runtime_id {
                continue;
            }
            // A search that fails under one controlled element leaves the
            // others to try.
            if let Ok(Some(found)) = self.element_by_runtime_id(&root, runtime_id, cache) {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    /// Walks the raw-view subtree rooted at `root`, every node read as a
    /// [`TreeDump`] filled by `request`, bounded by `max_depth` (the root is
    /// depth 0) and `max_nodes` (the total number of nodes across the whole
    /// walk, the root included). Returns the walked tree and whether either
    /// cap was hit before the walk reached every node. Cross-process; the
    /// outpost's worker only, guarded by the caller's deadline, since a hung
    /// provider can stall any step.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the tree walker cannot be created.
    pub fn tree(
        &self,
        root: &Cached<TreeDump>,
        request: &Request<TreeDump>,
        registry: &NodeIdRegistry,
        max_depth: u32,
        max_nodes: usize,
    ) -> windows::core::Result<(TreeNode, bool)> {
        let walker = self.raw_view_walker()?;
        let limits = ShapedWalk {
            request,
            registry,
            max_depth,
            max_nodes,
        };
        let mut state = WalkState {
            visited: 1, // the root counts as one node.
            truncated: false,
        };
        let root = walk_shaped(&walker, root, &limits, 0, &mut state);
        Ok((root, state.truncated))
    }

    /// Walks the chain of ancestors of `element`, each read as an
    /// [`Ancestor`] filled by `request`: its node for the report, and its
    /// description whatever the theme, for the layout judgment. One hop at
    /// a time over the raw view, each hop its own cross-process round trip
    /// (`GetParentElementBuildCache`), the per-hop walk NVDA shipped for
    /// years; the outpost reads a focus's ancestors in one remote operation
    /// instead (`verbatim_uia_rops::focus_ancestry`, then
    /// [`ancestors_from`](Self::ancestors_from)) and keeps this walk for
    /// windows read the classic way. Capped at `max_hops` ancestors;
    /// returned outermost first. Cross-process; the outpost's worker only,
    /// guarded by the caller's deadline, since a hung provider can stall
    /// any hop.
    ///
    /// The walk stops at an ancestor that is the root element of a window
    /// `stops.read_by_other_api` claims, and returns that window's handle
    /// with the chain below it, so the caller can continue through the
    /// other API, as NVDA switches API when a walk crosses into a window
    /// read through it (`correctAPIForRelation`). It also stops at the
    /// first reported ancestor `stops.known` recognizes, from the previous
    /// focus's chain, with that ancestor outermost, as NVDA's focus
    /// ancestry stops where it meets the previous focus's ancestors; and
    /// when `stops.out_of_time` says its time is up, or a hop finds the provider did not
    /// answer within UIA's transaction timeout, reporting the chain as
    /// incomplete. Ancestors that are not presentable focus context are
    /// crossed but never reported (NVDA's `isPresentableFocusAncestor`).
    ///
    /// The walk never steps across the process boundary: before each hop,
    /// from an element whose own window, `element_window` for `element`
    /// and the cached handle for an ancestor, has a parent window, or a
    /// visible owner for a top-level window, that another process serves
    /// ([`parent_across_process_boundary`](crate::parent_across_process_boundary)),
    /// it stops and returns that parent or owner window as the window to
    /// continue through the other API, local calls only
    /// (`docs/design/listener-without-connections.md`, section 4.3).
    ///
    /// # Errors
    ///
    /// Returns the COM error if the tree walker itself cannot be created;
    /// a hop that finds no parent is not an error, it simply ends the walk.
    pub fn ancestors(
        &self,
        (element, element_window): (&Live, isize),
        request: &Request<Ancestor>,
        registry: &NodeIdRegistry,
        max_hops: u32,
        stops: &AncestorStops<'_>,
    ) -> windows::core::Result<(Vec<NodeSnapshot>, Option<isize>, AncestorWalk)> {
        let AncestorStops {
            read_by_other_api,
            known,
            out_of_time,
        } = *stops;
        let walker = self.raw_view_walker()?;
        let mut chain = Vec::new();
        let mut current = element.clone();
        let mut current_window = element_window;
        let started = std::time::Instant::now();
        let mut hops = 0u32;
        let log = |hops: u32, ending: &str| {
            tracing::debug!(
                hops,
                ending,
                elapsed_us = started.elapsed().as_micros(),
                "UIA ancestor walk"
            );
        };
        let out_of_time = || out_of_time.is_some_and(|out_of_time| out_of_time());
        for _ in 0..max_hops {
            if out_of_time() {
                chain.reverse();
                log(hops, "out of time");
                return Ok((chain, None, AncestorWalk::OutOfTime));
            }
            if let Some(across) = (current_window != 0)
                .then(|| crate::parent_across_process_boundary(current_window))
                .flatten()
            {
                chain.reverse();
                log(hops, "below the process boundary");
                return Ok((chain, Some(across), AncestorWalk::Complete));
            }
            hops += 1;
            let parent = match current.walk(&walker, Direction::Parent, request) {
                Ok(parent) => parent,
                // A hop that timed out, or ran past the deadline, is an
                // incomplete chain, not the root. Any other failure ends the
                // chain as the root, as NVDA's does: its parent read answers
                // no parent when the call fails.
                Err(error) => {
                    let ending = if crate::timed_out(&error) || out_of_time() {
                        AncestorWalk::OutOfTime
                    } else {
                        AncestorWalk::Complete
                    };
                    chain.reverse();
                    log(hops, "root");
                    return Ok((chain, None, ending));
                }
            };
            current_window = parent.read().node.role.hwnd;
            match take_shaped_ancestor(&parent, registry, read_by_other_api, known, &mut chain) {
                Taken::Continue => current = parent.live().clone(),
                Taken::Crossed(hwnd) => {
                    chain.reverse();
                    log(hops, "crossed into MSAA");
                    return Ok((chain, Some(hwnd), AncestorWalk::Complete));
                }
                Taken::Known(id) => {
                    chain.reverse();
                    log(hops, "met a known ancestor");
                    return Ok((chain, None, AncestorWalk::MetKnown(id)));
                }
            }
        }
        chain.reverse();
        log(hops, "hop limit");
        Ok((chain, None, AncestorWalk::Complete))
    }

    /// The same result as [`ancestors`](Self::ancestors), from `parents`
    /// already fetched as [`Ancestor`]s, nearest first, as one remote
    /// operation returns them: the same stops at a window read through the
    /// other API and at a known ancestor, and the same filtering of what is
    /// reported. A fetch cut short by a depth limit is reported as complete
    /// up to there, like the walk's hop limit.
    #[must_use]
    pub fn ancestors_from(
        parents: &[Cached<Ancestor>],
        registry: &NodeIdRegistry,
        stops: &AncestorStops<'_>,
    ) -> (Vec<NodeSnapshot>, Option<isize>, AncestorWalk) {
        let mut chain = Vec::new();
        for parent in parents {
            match take_shaped_ancestor(
                parent,
                registry,
                stops.read_by_other_api,
                stops.known,
                &mut chain,
            ) {
                Taken::Continue => {}
                Taken::Crossed(hwnd) => {
                    chain.reverse();
                    return (chain, Some(hwnd), AncestorWalk::Complete);
                }
                Taken::Known(id) => {
                    chain.reverse();
                    return (chain, None, AncestorWalk::MetKnown(id));
                }
            }
        }
        chain.reverse();
        (chain, None, AncestorWalk::Complete)
    }

    /// The same result as [`Uia::ancestors_from`], from ancestors
    /// another process read and sent as their parts ([`AncestorParts`]),
    /// nearest first: the focus listener reads a focus's ancestors from the
    /// event's own element, and the outpost, which cannot receive that
    /// element (`docs/design/focus-pipeline.md`, section 12), reports them
    /// from this. Each node is minted from its runtime id with no element
    /// kept; the same stops and the same filtering apply.
    #[must_use]
    pub fn ancestor_chain_from_parts(
        parents: &[AncestorParts],
        registry: &NodeIdRegistry,
        stops: &AncestorStops<'_>,
    ) -> (Vec<NodeSnapshot>, Option<isize>, AncestorWalk) {
        let mut chain = Vec::new();
        for parent in parents {
            match take_ancestor_parts(
                parent,
                registry,
                stops.read_by_other_api,
                stops.known,
                &mut chain,
            ) {
                Taken::Continue => {}
                Taken::Crossed(hwnd) => {
                    chain.reverse();
                    return (chain, Some(hwnd), AncestorWalk::Complete);
                }
                Taken::Known(id) => {
                    chain.reverse();
                    return (chain, None, AncestorWalk::MetKnown(id));
                }
            }
        }
        chain.reverse();
        (chain, None, AncestorWalk::Complete)
    }

    /// The first selected child of a selection container, read as a
    /// [`Node`] filled by `request` ([`selected`](Self::selected)), as a
    /// snapshot. `Ok(None)` is every benign outcome: the element exposes
    /// no selection pattern, or nothing is selected. A multiple selection
    /// reports its first element; the reducer speaks one item.
    /// Cross-process; the outpost's worker only, guarded by the caller's
    /// deadline.
    ///
    /// # Errors
    ///
    /// As [`Uia::selected`].
    pub fn selected_node(
        &self,
        container: &Live,
        request: &Request<Node>,
        registry: &NodeIdRegistry,
    ) -> windows::core::Result<Option<NodeSnapshot>> {
        Ok(self
            .selected(container, request)?
            .map(|selected| selected.snapshot(registry)))
    }

    /// The first element of `container`'s current selection, its cache
    /// filled by `request`, or `None` for every benign outcome: no
    /// `Selection` pattern, nothing selected, or a call that failed in any
    /// other way than the errors below. `verbatim-uia-rops` calls it for
    /// the classic focus ancestry.
    ///
    /// One `BuildUpdatedCache` refreshes the container as a
    /// [`SelectionContainer`], asking for both of its selection patterns at
    /// once: `SelectionPattern2`'s `FirstSelectedItem` property, ignoring
    /// its default, as NVDA reads the newer pattern where the provider has
    /// it, and the `Selection` pattern object. With `SelectionPattern2`,
    /// the first item comes from that call, and its refresh is the second
    /// and last. Without it, the property reads "not supported" and the
    /// cached `Selection` pattern reads the selection, a second call, with
    /// the refresh a third, the count the `Selection` pattern alone always
    /// took.
    ///
    /// # Errors
    ///
    /// Returns the COM error when the cache request cannot be made, and a
    /// call's error when the provider did not answer within UIA's
    /// transaction timeout ([`timed_out`](crate::timed_out)) or the
    /// container or its selected item is gone
    /// ([`element_is_gone`](crate::element_is_gone)): neither says nothing
    /// is selected.
    pub fn selected<S: Shape>(
        &self,
        container: &Live,
        request: &Request<S>,
    ) -> windows::core::Result<Option<Cached<S>>> {
        self.selected_within(container, request, &|| false)
    }

    /// [`selected`](Self::selected) within the caller's time: `out_of_time`
    /// is asked before each call after the first, and the answer is `None`,
    /// the selected child unknown, once it says the time is up.
    ///
    /// # Errors
    ///
    /// As [`selected`](Self::selected).
    pub fn selected_within<S: Shape>(
        &self,
        container: &Live,
        request: &Request<S>,
        out_of_time: &dyn Fn() -> bool,
    ) -> windows::core::Result<Option<Cached<S>>> {
        let selection = self.request_fixed::<SelectionContainer>()?;
        let Some(container) = unanswered_is_error(container.refresh(&selection))? else {
            return Ok(None);
        };
        let read = container.read();
        let first = match read.first_selected {
            None => return Ok(None),
            Some(ElementValue::Element(first)) => Some(first),
            Some(ElementValue::NotSupported) => {
                let Some(pattern) = read.selection else {
                    return Ok(None);
                };
                if out_of_time() {
                    return Ok(None);
                }
                unanswered_is_error(current_selection(&pattern))?
                    .and_then(|selection| selection.into_iter().next())
            }
        };
        let Some(first) = first else {
            return Ok(None);
        };
        if out_of_time() {
            return Ok(None);
        }
        unanswered_is_error(Live::new(first).refresh(request))
    }

    /// Navigates one step from `element` in `direction`, the neighbour read
    /// as a [`Node`] filled by `request`, through the raw-view tree
    /// walker's `*BuildCache` methods: a single cross-process round trip,
    /// and deliberately the full, unfiltered tree.
    ///
    /// A recorded decision, made twice: object navigation matches NVDA
    /// with its simple review mode off, so every element the raw view
    /// exposes is navigable, exactly the tree NVDA's own `baseTreeWalker`
    /// (also the raw view walker) exposes. An intermediate revision
    /// projected NVDA's simple-review filtering here instead; the user
    /// baselines against NVDA with the setting off, where full-tree
    /// navigation is the correct behavior, so the projection was removed.
    /// Spoken focus ancestry is a different matter: NVDA filters it by
    /// `isPresentableFocusAncestor` regardless of the review-mode setting,
    /// and [`Uia::ancestors`] mirrors that.
    ///
    /// Returns `Ok(None)` for a genuine "no such neighbor" (a root's
    /// parent, a last child's next sibling), a first-class outcome distinct
    /// from an error. Cross-process; the outpost's worker only, guarded by
    /// the caller's deadline.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the tree walker cannot be created, or the
    /// step's error when it says the element is gone
    /// ([`element_is_gone`](crate::element_is_gone)).
    pub fn neighbor(
        &self,
        element: &Live,
        request: &Request<Node>,
        registry: &NodeIdRegistry,
        direction: QueryKind,
    ) -> windows::core::Result<Option<NodeSnapshot>> {
        let walker = self.raw_view_walker()?;
        let direction = match direction {
            QueryKind::Parent => Direction::Parent,
            QueryKind::NextSibling => Direction::NextSibling,
            QueryKind::PreviousSibling => Direction::PreviousSibling,
            QueryKind::FirstChild => Direction::FirstChild,
            // Not a navigation direction.
            _ => return Err(windows::core::Error::from(E_INVALIDARG)),
        };
        match element.walk(&walker, direction, request) {
            Ok(neighbor) => Ok(Some(neighbor.snapshot(registry))),
            // A dead element is reported as such; any other failure reads as
            // no neighbor, as NVDA's tree-walker failures do, and so does the
            // null element `windows` reports for a genuine edge.
            Err(error) if crate::element_is_gone(&error) => Err(error),
            Err(_) => Ok(None),
        }
    }

    /// Activates `element`: tries `Invoke`, then `Toggle`, then selecting it
    /// (`SelectionItem`), in that order, NVDA's default action for a UIA
    /// element. Answers the action's name as NVDA names it: "invoke" for
    /// `Invoke`, none for the others. Each pattern is fetched live (`GetCurrentPatternAs`, not a
    /// cached read), since activation is an infrequent, user-triggered
    /// action rather than something a shape's request prefetches.
    /// Cross-process; the outpost's worker only, guarded by the caller's
    /// deadline.
    ///
    /// # Errors
    ///
    /// Returns the COM error from whichever pattern fetch or invocation
    /// failed, or a "not implemented" error if `element` exposes none of the
    /// three patterns.
    pub fn activate(
        &self,
        element: &IUIAutomationElement,
    ) -> windows::core::Result<Option<verbatim_model::ActionName>> {
        // Each pattern fetch fails (an error) when the pattern is
        // unsupported.
        if let Ok(pattern) =
            element.current_pattern::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
        {
            return invoke(&pattern).map(|()| Some(verbatim_model::ActionName::Invoke));
        }
        if let Ok(pattern) =
            element.current_pattern::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
        {
            return toggle(&pattern).map(|()| None);
        }
        if let Ok(pattern) =
            element.current_pattern::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
        {
            return select(&pattern).map(|()| None);
        }
        Err(windows::core::Error::new(
            windows::Win32::Foundation::E_NOTIMPL,
            "element exposes no Invoke, Toggle, or SelectionItem pattern",
        ))
    }
}

/// The first element of a container's selection through the `Selection`
/// pattern object cached on `container`, for a provider without
/// `SelectionPattern2`: one round trip, the selection. A container without
/// the pattern has no cached pattern, and no selection.
/// A call's result, with its failure read as no answer (`Ok(None)`) unless
/// the provider did not answer within UIA's transaction timeout or the
/// element is gone, which stay errors.
fn unanswered_is_error<T>(result: windows::core::Result<T>) -> windows::core::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if crate::timed_out(&error) || crate::element_is_gone(&error) => Err(error),
        Err(_) => Ok(None),
    }
}

/// What one ancestor did to a walk ([`take_shaped_ancestor`]).
enum Taken {
    /// The walk goes on to the next parent.
    Continue,
    /// The ancestor is the root of a window read through the other API.
    Crossed(isize),
    /// The ancestor was reported and is already known.
    Known(verbatim_model::NodeId),
}

/// One ancestor of a walk, nearest first, read as an [`Ancestor`]: the
/// stop at a window read through the other API, then the report, filtered,
/// and the stop at a known ancestor. Non-presentable ancestors are crossed
/// but never reported, NVDA's `isPresentableFocusAncestor`, which filters
/// spoken focus context regardless of its review-mode setting (object
/// navigation, by contrast, sees the full tree; see [`Uia::neighbor`]); a
/// UIA element is content only when UIA counts it both a control and
/// content, as NVDA requires. The layout judgment reads the ancestor's
/// description whatever the theme, while the snapshot reported carries the
/// description only when the theme's gate fetched it.
fn take_shaped_ancestor(
    parent: &Cached<Ancestor>,
    registry: &NodeIdRegistry,
    read_by_other_api: &dyn Fn(isize) -> bool,
    known: &dyn Fn(verbatim_model::NodeId) -> bool,
    chain: &mut Vec<NodeSnapshot>,
) -> Taken {
    let read = parent.read();
    let hwnd = read.node.role.hwnd;
    if hwnd != 0 && read_by_other_api(hwnd) {
        return Taken::Crossed(hwnd);
    }
    let snapshot = parent.snapshot(registry);
    let description = read.layout_description();
    take_reported(
        snapshot,
        read.is_control_and_content(),
        description.as_deref(),
        known,
        chain,
    )
}

/// [`take_shaped_ancestor`] for an ancestor another process read
/// ([`AncestorParts`]): its node is minted from its runtime id, with no
/// element kept.
fn take_ancestor_parts(
    parent: &AncestorParts,
    registry: &NodeIdRegistry,
    read_by_other_api: &dyn Fn(isize) -> bool,
    known: &dyn Fn(verbatim_model::NodeId) -> bool,
    chain: &mut Vec<NodeSnapshot>,
) -> Taken {
    if parent.hwnd != 0 && read_by_other_api(parent.hwnd) {
        return Taken::Crossed(parent.hwnd);
    }
    let parts = &parent.parts;
    let snapshot = NodeSnapshot {
        id: registry.id_for(&parts.runtime_id),
        backend: verbatim_model::Backend::Uia,
        role: parts.role,
        name: parts.name.clone(),
        value: parts.value.clone(),
        states: parts.states,
        details: parts.details.clone(),
    };
    let description = parts.details.description.clone();
    take_reported(
        snapshot,
        parent.content,
        description.as_deref(),
        known,
        chain,
    )
}

/// The report of one ancestor, filtered, and the stop at a known one: the
/// part of [`take_shaped_ancestor`] that does not depend on where the ancestor
/// was read. `description` is the one the layout judgment reads.
fn take_reported(
    snapshot: NodeSnapshot,
    content: bool,
    description: Option<&str>,
    known: &dyn Fn(verbatim_model::NodeId) -> bool,
    chain: &mut Vec<NodeSnapshot>,
) -> Taken {
    let presentable = is_presentable_focus_ancestor(&snapshot, description);
    if !(presentable && content) {
        tracing::debug!(
            role = ?snapshot.role,
            name = ?snapshot.name,
            presentable,
            content,
            "focus ancestor not reported: {}",
            if presentable {
                "not a control and content element"
            } else {
                "layout or an item, not focus context"
            }
        );
    }
    if presentable && content {
        let id = snapshot.id;
        chain.push(snapshot);
        if known(id) {
            return Taken::Known(id);
        }
    }
    Taken::Continue
}

/// Whether this process has finished UIA's first-time setup; see
/// [`ensure_ready`].
static UIA_READY: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

/// The cookie of the process's hold on the multithreaded apartment, taken
/// by the first-time setup ([`ensure_ready`]) and given back by
/// [`release_mta_usage`].
static MTA_USAGE: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);

/// Gives back the process's hold on COM's multithreaded apartment, which
/// the first use of UIA took so the apartment, and UIA's setup with it,
/// outlives whichever thread happens to be in it. For a process that has
/// finished with UIA for good, such as an outpost shutting down: once
/// every thread has left the apartment ([`crate::leave_mta`]), COM tears
/// it down. A later use of UIA sets it up again. Does nothing when UIA
/// was never used.
pub fn release_mta_usage() {
    let mut ready = UIA_READY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cookie = MTA_USAGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(cookie) = cookie {
        // SAFETY: the cookie `CoIncrementMTAUsage` returned, given back
        // once: it was taken out of the slot above.
        let _ = unsafe {
            windows::Win32::System::Com::CoDecrementMTAUsage(
                windows::Win32::System::Com::CO_MTA_USAGE_COOKIE(cookie as *mut _),
            )
        };
    }
    *ready = false;
}

/// Finishes UIA's first-time setup in this process before this thread uses
/// UIA, doing it if no thread has: a client is created and a cache request
/// built from it while holding [`UIA_READY`], so no other thread uses UIA
/// until the setup has finished. The setup runs on a thread of its own,
/// which leaves COM when it is done, so a thread that only probes never
/// joins COM's multithreaded apartment: one that did, once the probe called
/// for the setup, made the test process crash on exit on GitHub's runner.
/// The setup lasts only as long as the apartment, so the apartment is kept
/// for the life of the process (`CoIncrementMTAUsage`) rather than by
/// whichever thread happens to be in it. Every use of UIA in the crate
/// calls this first, the provider probe included, because while one
/// thread is still setting UIA up, another thread's `CreateCacheRequest`
/// fails (`docs/uia-bugs.md`, "A cache request fails while UIA's
/// first-time setup runs on another thread"), and an outpost starts
/// several UIA threads at once.
///
/// # Errors
///
/// Returns the COM error if the apartment, the first client, or its cache
/// request cannot be set up; the next call tries again.
pub(crate) fn ensure_ready() -> windows::core::Result<()> {
    let mut ready = UIA_READY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !*ready {
        let setup = std::thread::Builder::new()
            .name("verbatim-uia-setup".to_owned())
            .spawn(|| -> windows::core::Result<()> {
                // SAFETY: keeps the apartment alive until
                // `release_mta_usage` gives the cookie back.
                let cookie = unsafe { windows::Win32::System::Com::CoIncrementMTAUsage() }?;
                *MTA_USAGE
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cookie.0 as usize);
                init_mta()?;
                // SAFETY: as in `create_client`.
                let result = unsafe {
                    CoCreateInstance::<_, IUIAutomation>(
                        &CUIAutomation8,
                        None,
                        CLSCTX_INPROC_SERVER,
                    )
                }
                // Configured as every other client is, though it lives only
                // for the setup; the process's timeouts are set through it,
                // once, before any other client exists.
                .and_then(|client| {
                    enable_recovery_and_coalescing(&client.cast::<IUIAutomation6>()?)?;
                    // CUIAutomation8 objects implement IUIAutomation2.
                    set_process_timeouts(&client.cast::<IUIAutomation2>()?)?;
                    Ok(client)
                })
                // SAFETY: a local call on the client just created, released
                // before the thread leaves COM.
                .and_then(|client| unsafe { client.CreateCacheRequest() }.map(drop));
                // SAFETY: balances this thread's `init_mta`.
                unsafe { windows::Win32::System::Com::CoUninitialize() };
                result
            })
            .map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        setup
            .join()
            .map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))??;
        *ready = true;
    }
    Ok(())
}

/// Creates a UIA client on this thread, in the multithreaded apartment,
/// once UIA's first-time setup has finished ([`ensure_ready`]), which set
/// the process's timeouts ([`CONNECTION_TIMEOUT`], [`TRANSACTION_TIMEOUT`]),
/// with its proxies told not to
/// raise the events Verbatim handles from `WinEvent`s
/// ([`crate::proxy::ignore_win_events`]). Nothing here changes the
/// timeouts, which belong to the process: a test that shortens them keeps
/// its setting while other threads create clients.
///
/// # Errors
///
/// Returns the COM error if the setup, the client, or its proxy mapping
/// fails.
pub(crate) fn create_client() -> windows::core::Result<IUIAutomation> {
    ensure_ready()?;
    init_mta()?;
    // SAFETY: CUIAutomation8 is a registered in-process COM server; the
    // requested interface matches the class. CUIAutomation8 rather than
    // the older CUIAutomation coclass because only the former's objects
    // implement the newer client interfaces — IUIAutomation5's
    // notification-event registration in particular, where querying a
    // plain CUIAutomation object fails with E_NOINTERFACE (observed
    // live; NVDA likewise creates CUIAutomation8).
    let client: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)? };
    enable_recovery_and_coalescing(&client.cast::<IUIAutomation6>()?)?;
    crate::proxy::ignore_win_events(&client)?;
    Ok(client)
}

/// Turns on the two `IUIAutomation6` behaviors NVDA enables on its client
/// whenever Windows has them (Windows 10 1809 and later, so always on
/// Verbatim's Windows 11 24H2): event coalescing, where UIA drops an event
/// that duplicates one still waiting to be delivered to this client, so a
/// burst of identical events from one element reaches the handler once;
/// and connection recovery, where UIA adjusts its waits for a provider
/// that has stopped responding and reconnects when it answers again.
/// Both are local settings of the client object.
fn enable_recovery_and_coalescing(client: &IUIAutomation6) -> windows::core::Result<()> {
    // SAFETY: `client` is a live IUIAutomation6; the option is a plain
    // value.
    unsafe { client.SetCoalesceEvents(CoalesceEventsOptions_Enabled) }?;
    // SAFETY: as above.
    unsafe { client.SetConnectionRecoveryBehavior(ConnectionRecoveryBehaviorOptions_Enabled) }
}

/// Sets the process's connection and transaction timeouts to
/// [`CONNECTION_TIMEOUT`] and [`TRANSACTION_TIMEOUT`] through `client`:
/// process-wide settings, though made
/// through a client (`docs/uia-bugs.md`, "The connection timeout applies
/// to the whole process").
fn set_process_timeouts(client: &IUIAutomation2) -> windows::core::Result<()> {
    let milliseconds =
        |timeout: std::time::Duration| u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: `client` is a live IUIAutomation2; each timeout is a plain
    // integer.
    unsafe { client.SetConnectionTimeout(milliseconds(CONNECTION_TIMEOUT)) }?;
    // SAFETY: as above.
    unsafe { client.SetTransactionTimeout(milliseconds(TRANSACTION_TIMEOUT)) }
}

/// Mutable state accumulated across the whole walk, shared by every level
/// of recursion.
struct WalkState {
    visited: usize,
    truncated: bool,
}

/// The per-walk parameters threaded through every level of
/// [`walk_shaped`]: every node read as a [`TreeDump`] filled by the
/// request, the registry that keeps each, and the walk's caps.
struct ShapedWalk<'a> {
    request: &'a Request<TreeDump>,
    registry: &'a NodeIdRegistry,
    max_depth: u32,
    max_nodes: usize,
}

/// The recursive worker for [`Uia::tree`]. `state.visited` already counts
/// `element` itself; children are counted as they are accepted into the
/// walk, before recursing into them.
fn walk_shaped(
    walker: &IUIAutomationTreeWalker,
    element: &Cached<TreeDump>,
    limits: &ShapedWalk<'_>,
    depth: u32,
    state: &mut WalkState,
) -> TreeNode {
    let snapshot = element.snapshot(limits.registry);
    let live = element.live();
    if depth >= limits.max_depth {
        // Peek only: is there a child we are declining to descend into?
        if live
            .walk(walker, Direction::FirstChild, limits.request)
            .is_ok()
        {
            state.truncated = true;
        }
        return TreeNode {
            snapshot,
            children: Vec::new(),
        };
    }
    let mut children = Vec::new();
    // An `Err` here means "no first child".
    let mut next_child = live
        .walk(walker, Direction::FirstChild, limits.request)
        .ok();
    while let Some(current) = next_child {
        if state.visited >= limits.max_nodes {
            state.truncated = true;
            break;
        }
        state.visited += 1;
        children.push(walk_shaped(walker, &current, limits, depth + 1, state));
        // `Err` means "no next sibling".
        next_child = current
            .live()
            .walk(walker, Direction::NextSibling, limits.request)
            .ok();
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
/// recorded decision on [`Uia::neighbor`]). `description` is the node's
/// description whatever the theme, as NVDA's judgment reads it whatever
/// its setting (decision 4 of `docs/design/uia-shapes.md`).
fn is_layout(snapshot: &NodeSnapshot, description: Option<&str>) -> bool {
    use verbatim_model::Role;
    fn blank(text: Option<&str>) -> bool {
        text.is_none_or(|text| text.trim().is_empty())
    }
    match snapshot.role {
        Role::Unknown | Role::Pane | Role::TitleBar => true,
        Role::StaticText => blank(snapshot.name.as_deref()),
        Role::Window | Role::PropertyPage | Role::Group => {
            blank(snapshot.name.as_deref()) && blank(description)
        }
        _ => false,
    }
}

impl Ancestor {
    /// Whether the element read as this ancestor, and reported as
    /// `snapshot`, is worth speaking as focus context by NVDA's rule for a
    /// UIA object: presentable by its role, name, and description
    /// (`is_presentable_focus_ancestor`), and counted by UI Automation both
    /// a control and content, without which NVDA takes a UIA object for
    /// layout (`_get_presentationType`, `NVDAObjects/UIA/__init__.py` 2121
    /// to 2130). Every UIA ancestor a walk reports passes it; the outpost
    /// applies it as well to the element of a window read through UIA that
    /// an MSAA walk crosses into. Judged with its description whatever the
    /// theme. Local.
    #[must_use]
    pub fn is_focus_context(&self, snapshot: &NodeSnapshot) -> bool {
        let description = self.layout_description();
        is_presentable_focus_ancestor(snapshot, description.as_deref())
            && self.is_control_and_content()
    }
}

/// NVDA's `isPresentableFocusAncestor`: whether an ancestor is worth
/// speaking as focus context. Layout elements are not; neither are roles
/// that never meaningfully contain the focus for announcement purposes —
/// list items, tree items, progress bars, and editable text. NVDA applies
/// this to focus ancestry regardless of its review-mode setting.
/// `description` is the one [`is_layout`] reads.
fn is_presentable_focus_ancestor(snapshot: &NodeSnapshot, description: Option<&str>) -> bool {
    use verbatim_model::Role;
    if is_layout(snapshot, description) {
        return false;
    }
    !matches!(
        snapshot.role,
        Role::ListItem | Role::TreeItem | Role::ProgressBar | Role::EditableText
    )
}

#[cfg(test)]
mod presentation_tests {
    use verbatim_model::{Backend, NodeDetails, NodeId, NodeSnapshot, Role, StateSet};

    /// [`super::is_layout`], judged with the snapshot's own description.
    fn is_layout(snapshot: &NodeSnapshot) -> bool {
        super::is_layout(snapshot, snapshot.details.description.as_deref())
    }

    /// [`super::is_presentable_focus_ancestor`], judged with the snapshot's
    /// own description.
    fn is_presentable_focus_ancestor(snapshot: &NodeSnapshot) -> bool {
        super::is_presentable_focus_ancestor(snapshot, snapshot.details.description.as_deref())
    }

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

    /// An ancestor another process read, named `name` with role `role`,
    /// runtime id `[42, id]`, window `hwnd`, both a control and content.
    fn parts(id: i32, role: Role, name: &str, hwnd: isize) -> super::AncestorParts {
        super::AncestorParts {
            parts: crate::map::CachedUiaParts {
                runtime_id: vec![42, id],
                role,
                name: Some(name.to_owned()),
                value: None,
                states: StateSet::default(),
                details: NodeDetails::default(),
            },
            hwnd,
            content: true,
        }
    }

    fn names(chain: &[NodeSnapshot]) -> Vec<Option<&str>> {
        chain.iter().map(|node| node.name.as_deref()).collect()
    }

    #[test]
    fn ancestors_read_elsewhere_are_filtered_and_stop_at_a_window_of_the_other_api() {
        let registry =
            crate::NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
        let mut hidden = parts(3, Role::Group, "not content", 0);
        hidden.content = false;
        // Nearest first: a list, a pane (layout), a group UIA counts no
        // content, a named group, and the root of a window read through
        // MSAA, where the chain stops with that window.
        let parents = [
            parts(1, Role::List, "Items", 0),
            parts(2, Role::Pane, "Shell", 0),
            hidden,
            parts(4, Role::Group, "Folder", 0),
            parts(5, Role::Window, "Explorer", 77),
            parts(6, Role::Window, "Desktop", 0),
        ];
        let stops = super::AncestorStops {
            read_by_other_api: &|hwnd| hwnd == 77,
            known: &|_| false,
            out_of_time: None,
        };
        let (chain, crossed, walked) =
            super::Uia::ancestor_chain_from_parts(&parents, &registry, &stops);
        assert_eq!(names(&chain), [Some("Folder"), Some("Items")]);
        assert_eq!((crossed, walked), (Some(77), super::AncestorWalk::Complete));
        assert_eq!(
            registry.runtime_id_of(chain[1].id),
            Some(vec![42, 1]),
            "each node is minted from its runtime id"
        );
        assert!(
            registry.element_of(chain[1].id).is_none(),
            "with no element kept"
        );
    }

    #[test]
    fn ancestors_read_elsewhere_stop_at_a_known_one() {
        let registry =
            crate::NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
        let known = registry.id_for(&[42, 4]);
        let parents = [
            parts(1, Role::List, "Items", 0),
            parts(4, Role::Group, "Folder", 0),
            parts(5, Role::Window, "Explorer", 0),
        ];
        let stops = super::AncestorStops {
            read_by_other_api: &|_| false,
            known: &|id| id == known,
            out_of_time: None,
        };
        let (chain, crossed, walked) =
            super::Uia::ancestor_chain_from_parts(&parents, &registry, &stops);
        assert_eq!(names(&chain), [Some("Folder"), Some("Items")]);
        assert_eq!(
            (crossed, walked),
            (None, super::AncestorWalk::MetKnown(known))
        );
    }
}

#[cfg(test)]
mod client_tests {
    use windows::Win32::UI::Accessibility::{
        CoalesceEventsOptions_Enabled, ConnectionRecoveryBehaviorOptions_Enabled, IUIAutomation2,
        IUIAutomation6,
    };
    use windows::core::Interface;

    #[test]
    fn a_client_coalesces_events_and_recovers_connections() {
        let uia = super::Uia::new().expect("a client");
        let client: IUIAutomation6 = uia.client().cast().expect("IUIAutomation6");
        // SAFETY: reading a local setting of a live client.
        let coalesce = unsafe { client.CoalesceEvents() }.expect("the coalescing setting");
        // SAFETY: as above.
        let recovery =
            unsafe { client.ConnectionRecoveryBehavior() }.expect("the recovery setting");
        assert_eq!(coalesce, CoalesceEventsOptions_Enabled);
        assert_eq!(recovery, ConnectionRecoveryBehaviorOptions_Enabled);
    }

    #[test]
    fn a_client_waits_the_connection_and_transaction_timeouts() {
        let uia = super::Uia::new().expect("a client");
        let client: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
        // SAFETY: reading the process's settings through a live client.
        let connection = unsafe { client.ConnectionTimeout() }.expect("the connection timeout");
        // SAFETY: as above.
        let transaction = unsafe { client.TransactionTimeout() }.expect("the transaction timeout");
        assert_eq!((connection, transaction), (2000, 5000));
    }
}
