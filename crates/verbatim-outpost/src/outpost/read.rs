//! The worker's reads from the application: windows, the focus, a node's
//! ancestors, navigation, activation, and tree dumps, through whichever
//! backend owns the window. Every function here may call into the
//! application and so runs only on the worker, under its deadline.

use windows::Win32::UI::Accessibility::{IUIAutomationCacheRequest, IUIAutomationElement};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::CHILDID_SELF;
use verbatim_model::{Backend, NodeDetails, NodeId, NodeSnapshot, QueryKind, Role, StateSet};
use verbatim_uia::map::snapshot_from_cached_element;
use verbatim_uia::{Uia, has_server_side_provider};

use crate::arbitration::window_class_name;
use crate::protocol::{DumpedTree, FocusNow, FocusedControl};

use super::Context;
use super::window::{
    focus_window_of, foreground_window_of, main_window_of, top_level_windows,
    window_belongs_to_hidden_frame, window_facts, window_is_hidden_frame, window_text,
};

/// Depth cap for a tree dump (the root is depth 0).
const MAX_DUMP_DEPTH: u32 = 64;

/// Node-count cap for a tree dump.
const MAX_DUMP_NODES: usize = 4096;

/// Cap on the ancestors read for a node.
pub(super) const MAX_ANCESTOR_HOPS: u32 = 64;

/// Why a read failed.
#[derive(Debug)]
pub(super) enum ReadError {
    /// The node is no longer reachable, or this outpost never issued it.
    Gone,
    /// The read failed for another reason.
    Failed(String),
}

impl From<verbatim_ia2::acquire::AcquireError> for ReadError {
    fn from(error: verbatim_ia2::acquire::AcquireError) -> Self {
        match error {
            verbatim_ia2::acquire::AcquireError::Gone => ReadError::Gone,
            verbatim_ia2::acquire::AcquireError::Failed(reason) => ReadError::Failed(reason),
        }
    }
}

/// The worker's own UIA client, created on first use on the worker's thread
/// so it never crosses threads.
#[derive(Default)]
pub(super) struct Client {
    uia: Option<Uia>,
    failed: bool,
}

impl Client {
    /// The UIA client, created on first use. `None` if it cannot be created.
    pub(super) fn uia(&mut self) -> Option<&Uia> {
        if self.uia.is_none() && !self.failed {
            match Uia::new() {
                Ok(client) => self.uia = Some(client),
                Err(error) => {
                    self.failed = true;
                    tracing::warn!(%error, "the worker could not create a UIA client");
                }
            }
        }
        self.uia.as_ref()
    }

    fn uia_and_cache(&mut self) -> Result<(&Uia, IUIAutomationCacheRequest), ReadError> {
        let uia = self
            .uia()
            .ok_or_else(|| ReadError::Failed("could not create a UIA client".to_owned()))?;
        let cache = uia.base_cache_request().map_err(|error| {
            ReadError::Failed(format!("could not build a cache request: {error}"))
        })?;
        Ok((uia, cache))
    }
}

/// Decides a window's backend: the class lists and kept verdicts first, then
/// the `UiaHasServerSideProvider` probe, whose answer is kept for the
/// window's lifetime. Returns `true` for UIA.
pub(super) fn window_uses_uia(context: &Context, hwnd: isize) -> bool {
    let class = window_class_name(hwnd);
    if let Some(verdict) = context.arbitrator().verdict(hwnd, &class) {
        return verdict;
    }
    let is_uia = has_server_side_provider(hwnd);
    context.arbitrator().record_probe(hwnd, is_uia);
    is_uia
}

/// The snapshot of the top-level window `hwnd` for a foreground report. A
/// window whose accessible object cannot be read (a freshly created msinfo32
/// window, found live) is still reported, from local window data, and a
/// window whose accessible name is still empty takes its window text: NVDA
/// names a top-level window by its text.
pub(super) fn foreground_window(
    context: &Context,
    client: &mut Client,
    hwnd: isize,
) -> (Backend, NodeSnapshot) {
    let (backend, mut node) = window_snapshot(context, client, hwnd)
        .unwrap_or_else(|| (Backend::Msaa, local_window_snapshot(context, hwnd)));
    if node
        .name
        .as_deref()
        .is_none_or(|name| name.trim().is_empty())
    {
        node.name = window_text(hwnd);
    }
    (backend, node)
}

/// A window's own snapshot, through its backend. A popup menu window (the
/// Win32 menu class) reads as its client object, role menu.
fn window_snapshot(
    context: &Context,
    client: &mut Client,
    hwnd: isize,
) -> Option<(Backend, NodeSnapshot)> {
    if window_class_name(hwnd) == "#32768" {
        let node = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            OBJID_CLIENT.0,
            CHILDID_SELF,
            &context.msaa_registry,
        )?;
        return Some((Backend::Msaa, node));
    }
    if window_uses_uia(context, hwnd) {
        let (uia, cache) = client.uia_and_cache().ok()?;
        let element = uia.element_from_handle(hwnd, &cache).ok()?;
        // SAFETY: `element` was built with the base cache request.
        let node = unsafe { snapshot_from_cached_element(&element, &context.uia_registry) };
        Some((Backend::Uia, node))
    } else {
        // The window object itself, not its client area: the client reads
        // as role "client", the window as role "window".
        let node = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            OBJID_WINDOW.0,
            CHILDID_SELF,
            &context.msaa_registry,
        )?;
        Some((Backend::Msaa, node))
    }
}

/// A window snapshot from local window data only: role window, named by its
/// window text, under the registry key the MSAA window object uses.
fn local_window_snapshot(context: &Context, hwnd: isize) -> NodeSnapshot {
    NodeSnapshot {
        id: context
            .msaa_registry
            .id_for((hwnd, OBJID_WINDOW.0, CHILDID_SELF)),
        backend: Backend::Msaa,
        role: Role::Window,
        name: window_text(hwnd),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

/// Whether a focused node's selected child is reported with it: a list's
/// selected item, a tab control's active tab.
fn wants_selected_child(role: Role) -> bool {
    matches!(role, Role::List | Role::TabControl)
}

/// A focused UIA element's ancestors and, for a selection container, its
/// selected child, read from the element already in hand. Failures degrade
/// to an empty chain or `None`: enrichment never turns a focus into an error.
pub(super) fn uia_enrichment(
    context: &Context,
    uia: &Uia,
    cache: &IUIAutomationCacheRequest,
    element: &IUIAutomationElement,
    role: Role,
) -> (Vec<NodeSnapshot>, Option<NodeSnapshot>) {
    // SAFETY: `element` was built with `cache` by the caller.
    let ancestors =
        unsafe { uia.ancestor_chain(element, cache, &context.uia_registry, MAX_ANCESTOR_HOPS) }
            .unwrap_or_default();
    let selected = if wants_selected_child(role) {
        // SAFETY: as above.
        unsafe { uia.selected_child(element, cache, &context.uia_registry) }.unwrap_or(None)
    } else {
        None
    };
    (ancestors, selected)
}

/// An MSAA node's ancestors and, for a selection container, its selected
/// child. Failures degrade to an empty chain or `None`.
pub(super) fn msaa_enrichment(
    context: &Context,
    node: &NodeSnapshot,
) -> (Vec<NodeSnapshot>, Option<NodeSnapshot>) {
    let ancestors =
        verbatim_ia2::acquire::ancestor_chain(node.id, &context.msaa_registry, MAX_ANCESTOR_HOPS)
            .unwrap_or_default();
    let selected = if wants_selected_child(node.role) {
        verbatim_ia2::acquire::selected_child(node.id, &context.msaa_registry)
    } else {
        None
    };
    (ancestors, selected)
}

/// The application's focused control with its ancestors, selected child,
/// and window, through the focus window's backend. `None` when the keyboard
/// focus is not in this application, or is on Core's hidden frame.
pub(super) fn focused_control(context: &Context, client: &mut Client) -> Option<FocusedControl> {
    let hwnd = focus_window_of(context.target_pid)?;
    if window_belongs_to_hidden_frame(hwnd) {
        return None;
    }
    if window_uses_uia(context, hwnd) {
        let (uia, cache) = client.uia_and_cache().ok()?;
        let element = uia.focused_element(&cache).ok()?;
        // SAFETY: `element` was built with the base cache request.
        let node = unsafe { snapshot_from_cached_element(&element, &context.uia_registry) };
        let (ancestors, selected_child) = uia_enrichment(context, uia, &cache, &element, node.role);
        Some(FocusedControl {
            node,
            ancestors,
            selected_child,
            window: Some(window_facts(hwnd)),
        })
    } else {
        let node =
            verbatim_ia2::acquire::focused_snapshot(context.target_pid, &context.msaa_registry)?;
        let (ancestors, selected_child) = msaa_enrichment(context, &node);
        Some(FocusedControl {
            node,
            ancestors,
            selected_child,
            window: Some(window_facts(hwnd)),
        })
    }
}

/// The answer to a focus-now query: the application's foreground window, if
/// it holds the system foreground, and its focused control.
pub(super) fn focus_now(context: &Context, client: &mut Client) -> FocusNow {
    let window = foreground_window_of(context.target_pid)
        .filter(|&hwnd| !window_is_hidden_frame(hwnd))
        .map(|hwnd| {
            (
                foreground_window(context, client, hwnd).1,
                window_facts(hwnd),
            )
        });
    FocusNow {
        window,
        focus: focused_control(context, client),
    }
}

/// Resolves a UIA node to a live element built with `cache`: the registry's
/// kept element first (refreshing its cache both updates it and proves it
/// still answers), then a runtime-id search of the application's top-level
/// windows.
fn resolve_uia_element(
    context: &Context,
    uia: &Uia,
    cache: &IUIAutomationCacheRequest,
    node_id: NodeId,
) -> Option<IUIAutomationElement> {
    if let Some(agile) = context.uia_registry.element_of(node_id) {
        if let Ok(element) = agile.resolve() {
            // SAFETY: a dead underlying element fails the call rather than
            // crashing.
            if let Ok(fresh) = unsafe { element.BuildUpdatedCache(cache) } {
                return Some(fresh);
            }
        }
        context.uia_registry.evict_element(node_id);
    }
    let runtime_id = context.uia_registry.runtime_id_of(node_id)?;
    for hwnd in top_level_windows(context.target_pid) {
        if let Ok(root) = uia.element_from_handle(hwnd, cache)
            && let Ok(Some(element)) = uia.element_by_runtime_id(&root, &runtime_id, cache)
        {
            return Some(element);
        }
    }
    None
}

/// A UIA node's live element, or the error a query reports for it.
fn uia_node<'a>(
    context: &Context,
    client: &'a mut Client,
    node_id: NodeId,
) -> Result<(&'a Uia, IUIAutomationCacheRequest, IUIAutomationElement), ReadError> {
    let (uia, cache) = client.uia_and_cache()?;
    let element = resolve_uia_element(context, uia, &cache, node_id).ok_or(ReadError::Gone)?;
    Ok((uia, cache, element))
}

/// A node's ancestors, outermost first.
pub(super) fn ancestors(
    context: &Context,
    client: &mut Client,
    node_id: NodeId,
) -> Result<Vec<NodeSnapshot>, ReadError> {
    if context.uia_registry.runtime_id_of(node_id).is_some() {
        let (uia, cache, element) = uia_node(context, client, node_id)?;
        // SAFETY: `element` was built with `cache`.
        return unsafe {
            uia.ancestor_chain(&element, &cache, &context.uia_registry, MAX_ANCESTOR_HOPS)
        }
        .map_err(|error| ReadError::Failed(format!("UIA ancestor walk failed: {error}")));
    }
    verbatim_ia2::acquire::ancestor_chain(node_id, &context.msaa_registry, MAX_ANCESTOR_HOPS)
        .map_err(ReadError::from)
}

/// One object-navigation step: the neighbor, or `None` for a genuine tree
/// edge, never conflated with a node that is gone.
pub(super) fn navigate(
    context: &Context,
    client: &mut Client,
    node_id: NodeId,
    kind: QueryKind,
) -> Result<Option<NodeSnapshot>, ReadError> {
    if context.uia_registry.runtime_id_of(node_id).is_some() {
        let (uia, cache, element) = uia_node(context, client, node_id)?;
        // SAFETY: `element` was built with `cache`.
        return unsafe { uia.navigate(&element, &cache, &context.uia_registry, kind) }
            .map_err(|error| ReadError::Failed(format!("UIA navigation failed: {error}")));
    }
    verbatim_ia2::acquire::navigate(node_id, &context.msaa_registry, kind).map_err(ReadError::from)
}

/// Activates a node.
pub(super) fn activate(
    context: &Context,
    client: &mut Client,
    node_id: NodeId,
) -> Result<(), ReadError> {
    if context.uia_registry.runtime_id_of(node_id).is_some() {
        let (uia, _cache, element) = uia_node(context, client, node_id)?;
        // SAFETY: `element` is live.
        return unsafe { uia.activate(&element) }
            .map_err(|error| ReadError::Failed(format!("UIA activation failed: {error}")));
    }
    verbatim_ia2::acquire::activate(node_id, &context.msaa_registry).map_err(ReadError::from)
}

/// The application's tree from its top-level window.
pub(super) fn dump_tree(context: &Context, client: &mut Client) -> Result<DumpedTree, ReadError> {
    let hwnd = main_window_of(context.target_pid).ok_or_else(|| {
        ReadError::Failed("the target application has no top-level window".to_owned())
    })?;
    if window_uses_uia(context, hwnd) {
        let (uia, cache) = client.uia_and_cache()?;
        let element = uia.element_from_handle(hwnd, &cache).map_err(|error| {
            ReadError::Failed(format!(
                "could not fetch the top-level UIA element: {error}"
            ))
        })?;
        // SAFETY: `element` was built with `cache`.
        let (root, truncated) = unsafe {
            uia.walk_tree(
                &element,
                &cache,
                &context.uia_registry,
                MAX_DUMP_DEPTH,
                MAX_DUMP_NODES,
            )
        }
        .map_err(|error| ReadError::Failed(format!("UIA tree walk failed: {error}")))?;
        Ok(DumpedTree { root, truncated })
    } else {
        let (root, truncated) = verbatim_ia2::acquire::walk_tree(
            hwnd,
            &context.msaa_registry,
            MAX_DUMP_DEPTH,
            MAX_DUMP_NODES,
        )
        .ok_or_else(|| {
            ReadError::Failed("could not acquire the top-level MSAA object".to_owned())
        })?;
        Ok(DumpedTree { root, truncated })
    }
}
