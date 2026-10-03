//! The worker's reads from the application: windows, the focus, a node's
//! ancestors, navigation, activation, and tree dumps, through whichever
//! backend owns the window. Every function here may call into the
//! application and so runs only on the worker, under its deadline.

use windows::Win32::UI::Accessibility::{IUIAutomationCacheRequest, IUIAutomationElement};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::CHILDID_SELF;
use verbatim_model::{
    ActionName, Backend, NodeDetails, NodeId, NodeSnapshot, QueryKind, Role, StateSet,
};
use verbatim_uia::map::{cached_native_window_handle, snapshot_from_cached_element};
use verbatim_uia::{Uia, probe_server_side_provider};

use crate::arbitration::{PostProbeCheck, WindowClasses, post_probe_check, window_class_name};
use crate::protocol::{DumpedTree, FocusNow, FocusedControl};

use super::Context;
use super::window::{
    focus_window_of, foreground_window_of, main_window_of, now_ms, top_level_windows,
    window_belongs_to_hidden_frame, window_facts, window_is_hidden_frame, window_text,
};

/// Depth cap for a tree dump (the root is depth 0).
const MAX_DUMP_DEPTH: u32 = 64;

/// Node-count cap for a tree dump.
const MAX_DUMP_NODES: usize = 4096;

/// Cap on the ancestors read for a node.
pub(super) const MAX_ANCESTOR_HOPS: u32 = 64;

/// How long reading a focus's containers may take before the focus is
/// reported with them unknown: a focus is spoken late at worst, never lost
/// because its containers took too long to read.
const ENRICHMENT_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// A focus's ancestors, outermost first, or `None` when they could not be
/// read in time; and its selected child, for a selection container.
pub(super) type Enrichment = (Option<Vec<NodeSnapshot>>, Option<NodeSnapshot>);

/// `chain`, outermost first, whose outermost entry is `met`, extended
/// outward with the part of `previous` above `met`: NVDA reuses the
/// previous focus's ancestors from where the new focus's ancestry meets
/// them, rather than reading them again.
fn splice(previous: &[NodeSnapshot], chain: Vec<NodeSnapshot>, met: NodeId) -> Vec<NodeSnapshot> {
    let Some(index) = previous.iter().position(|node| node.id == met) else {
        return chain;
    };
    previous[..index].iter().cloned().chain(chain).collect()
}

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
/// the provider probe, whose answer the arbitrator keeps (a provider for the
/// window's lifetime, none for a short while). A window that did not answer
/// the probe is read through MSAA for the event at hand, as NVDA does when
/// its probe is cancelled, and is probed again next time. Returns `true` for
/// UIA.
pub(super) fn window_uses_uia(context: &Context, hwnd: isize) -> bool {
    let classes = WindowClasses::of(hwnd);
    if let Some(verdict) = context.arbitrator().verdict(hwnd, &classes) {
        return verdict;
    }
    let started = std::time::Instant::now();
    let answer = probe_server_side_provider(hwnd);
    tracing::debug!(
        hwnd,
        class = classes.raw,
        ?answer,
        elapsed_ms = started.elapsed().as_millis(),
        "arbitration probed"
    );
    let Some(is_uia) = answer else {
        return false;
    };
    if is_uia && let Some(check) = post_probe_check(&classes) {
        // NVDA uses some providers only after checking them.
        let usable = match check {
            PostProbeCheck::Console => verbatim_uia::console_reports_formatting(hwnd),
            PostProbeCheck::WindowsFormsListView => verbatim_uia::is_windows_forms(hwnd),
        };
        tracing::debug!(hwnd, ?check, ?usable, "arbitration checked the provider");
        return match usable {
            Some(true) => {
                context.arbitrator().record_probe(hwnd, true);
                true
            }
            Some(false) => {
                context.arbitrator().record_excluded(hwnd);
                false
            }
            // The application did not answer: asked again next time.
            None => false,
        };
    }
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
        // The client area, as NVDA reads a foreground window: a focus event
        // on the client area that follows is then the same node and is not
        // announced again.
        let node = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            OBJID_CLIENT.0,
            CHILDID_SELF,
            &context.msaa_registry,
        )?;
        Some((Backend::Msaa, node))
    }
}

/// A window snapshot from local window data only: role window, named by its
/// window text, under the registry key the MSAA client area uses, which a
/// foreground report reads when the window can be read.
fn local_window_snapshot(context: &Context, hwnd: isize) -> NodeSnapshot {
    NodeSnapshot {
        id: context
            .msaa_registry
            .id_for((hwnd, OBJID_CLIENT.0, CHILDID_SELF)),
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
/// selected child, read from the element already in hand. The walk stops
/// at a container of `previous` (the last focus's ancestors and the focus
/// itself) and reuses the rest; within [`ENRICHMENT_BUDGET`], each call to
/// the application waiting no longer than that. Failures degrade to no
/// containers or no selected child; running out of time to containers
/// unknown. Enrichment never turns a focus into an error.
pub(super) fn uia_enrichment(
    context: &Context,
    uia: &Uia,
    cache: &IUIAutomationCacheRequest,
    element: &IUIAutomationElement,
    role: Role,
    previous: &[NodeSnapshot],
) -> Enrichment {
    let deadline = std::time::Instant::now() + ENRICHMENT_BUDGET;
    uia.within(ENRICHMENT_BUDGET, |uia| {
        let ancestors = uia_ancestors(context, uia, cache, element, previous, Some(deadline));
        let selected = if wants_selected_child(role) {
            // SAFETY: `element` was built with `cache` by the caller.
            unsafe { uia.selected_child(element, cache, &context.uia_registry) }.unwrap_or(None)
        } else {
            None
        };
        (ancestors, selected)
    })
    .unwrap_or((None, None))
}

/// A UIA element's ancestors, outermost first, or `None` when the walk ran
/// out of time. Where the walk reaches the root of a window read through
/// MSAA, it continues from that window's client area through MSAA, as NVDA
/// switches API when its walk crosses into such a window: in File
/// Explorer, the folder window above the UIA file list is then the client
/// area a foreground report already named, not a second, UIA copy of it.
/// The desktop window ends the walk either way.
fn uia_ancestors(
    context: &Context,
    uia: &Uia,
    cache: &IUIAutomationCacheRequest,
    element: &IUIAutomationElement,
    previous: &[NodeSnapshot],
    deadline: Option<std::time::Instant>,
) -> Option<Vec<NodeSnapshot>> {
    // SAFETY: GetDesktopWindow has no preconditions.
    let desktop = unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() }.0 as isize;
    let read_by_msaa = |hwnd: isize| hwnd != desktop && !window_uses_uia(context, hwnd);
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    // SAFETY: `element` was built with `cache` by the caller.
    let Ok((uia_chain, crossed, walked)) = (unsafe {
        uia.ancestor_chain(
            element,
            cache,
            &context.uia_registry,
            MAX_ANCESTOR_HOPS,
            &verbatim_uia::AncestorStops {
                read_by_other_api: &read_by_msaa,
                known: &known,
                deadline,
            },
        )
    }) else {
        return Some(Vec::new());
    };
    match walked {
        verbatim_uia::AncestorWalk::OutOfTime => return None,
        verbatim_uia::AncestorWalk::MetKnown(met) => return Some(splice(previous, uia_chain, met)),
        verbatim_uia::AncestorWalk::Complete => {}
    }
    let Some(hwnd) = crossed else {
        return Some(uia_chain);
    };
    let Some(client) = verbatim_ia2::acquire::snapshot_from_event(
        hwnd,
        OBJID_CLIENT.0,
        CHILDID_SELF,
        &context.msaa_registry,
    ) else {
        return Some(uia_chain);
    };
    let below: Vec<NodeSnapshot> = std::iter::once(client.clone()).chain(uia_chain).collect();
    if known(client.id) {
        return Some(splice(previous, below, client.id));
    }
    let above = msaa_ancestors(context, Some((uia, cache)), &client, previous, deadline)?;
    Some(above.into_iter().chain(below).collect())
}

/// An MSAA node's ancestors, outermost first, without the window objects
/// above controls (see [`msaa_enrichment`]), stopping at a container of
/// `previous` and reusing the rest, or `None` when `deadline` passed. Where
/// a parent lies in a window read through UIA, the walk continues from that
/// window's UIA element, as NVDA switches API when a parent is in such a
/// window (`correctAPIForRelation`): in a Save As dialog, the file name
/// box's containers above the shell's UIA view are UIA's. Without `uia` the
/// walk stays in MSAA.
fn msaa_ancestors(
    context: &Context,
    uia: Option<(&Uia, &IUIAutomationCacheRequest)>,
    node: &NodeSnapshot,
    previous: &[NodeSnapshot],
    deadline: Option<std::time::Instant>,
) -> Option<Vec<NodeSnapshot>> {
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    let read_by_uia = |hwnd: isize| uia.is_some() && window_uses_uia(context, hwnd);
    let limits = verbatim_ia2::acquire::AncestorLimits {
        max_hops: MAX_ANCESTOR_HOPS,
        known: &known,
        read_by_other_api: &read_by_uia,
        deadline,
    };
    let (chain, walked) =
        verbatim_ia2::acquire::ancestor_chain_until(node.id, &context.msaa_registry, &limits)
            .unwrap_or((Vec::new(), verbatim_ia2::acquire::Walked::Complete));
    let chain: Vec<NodeSnapshot> = chain
        .into_iter()
        .filter(|ancestor| ancestor.role != Role::Window)
        .collect();
    match walked {
        verbatim_ia2::acquire::Walked::OutOfTime => None,
        verbatim_ia2::acquire::Walked::MetKnown(met) => Some(splice(previous, chain, met)),
        verbatim_ia2::acquire::Walked::Complete => Some(chain),
        verbatim_ia2::acquire::Walked::Crossed(hwnd) => {
            let Some((uia, cache)) = uia else {
                return Some(chain);
            };
            let Ok(element) = uia.element_from_handle(hwnd, cache) else {
                return Some(chain);
            };
            // SAFETY: `element` was just built with `cache`.
            let top = unsafe { snapshot_from_cached_element(&element, &context.uia_registry) };
            let top_id = top.id;
            let below: Vec<NodeSnapshot> = std::iter::once(top).chain(chain).collect();
            if known(top_id) {
                return Some(splice(previous, below, top_id));
            }
            let above = uia_ancestors(context, uia, cache, &element, previous, deadline)?;
            Some(above.into_iter().chain(below).collect())
        }
    }
}

/// An MSAA node's ancestors and, for a selection container, its selected
/// child, stopping at a container of `previous` and reusing the rest, within
/// [`ENRICHMENT_BUDGET`] (checked between calls; an MSAA call cannot be
/// given a shorter wait). Failures degrade to no containers or no selected
/// child; running out of time to containers unknown.
pub(super) fn msaa_enrichment(
    context: &Context,
    client: &mut Client,
    node: &NodeSnapshot,
    previous: &[NodeSnapshot],
) -> Enrichment {
    // A window object above a control is layout, never announced as an
    // entered container: NVDA gives a window object reached through its
    // parents the `GenericWindow` class, which is not a presentable focus
    // ancestor. A dialog is still announced, by its client area's dialog
    // role.
    let deadline = std::time::Instant::now() + ENRICHMENT_BUDGET;
    let ancestors = match client.uia_and_cache() {
        // A UIA read in the walk waits no longer than the budget.
        Ok((uia, cache)) => uia
            .within(ENRICHMENT_BUDGET, |uia| {
                msaa_ancestors(context, Some((uia, &cache)), node, previous, Some(deadline))
            })
            .unwrap_or(None),
        Err(_) => msaa_ancestors(context, None, node, previous, Some(deadline)),
    };
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
        let (ancestors, selected_child) =
            uia_enrichment(context, uia, &cache, &element, node.role, &[]);
        let ancestors = ancestors.unwrap_or_default();
        Some(FocusedControl {
            node,
            ancestors,
            selected_child,
            window: Some(window_facts(hwnd)),
        })
    } else {
        let node =
            verbatim_ia2::acquire::focused_snapshot(context.target_pid, &context.msaa_registry)?;
        let (ancestors, selected_child) = msaa_enrichment(context, client, &node, &[]);
        let ancestors = ancestors.unwrap_or_default();
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
    let observed_at_ms = now_ms();
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
        observed_at_ms,
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
        // The full chain: object navigation reuses nothing and waits for
        // the application as long as the query's own deadline allows.
        return Ok(uia_ancestors(context, uia, &cache, &element, &[], None).unwrap_or_default());
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
    let (neighbor, from_window) = if context.uia_registry.runtime_id_of(node_id).is_some() {
        let (uia, cache, element) = uia_node(context, client, node_id)?;
        let from_window = verbatim_uia::nearest_window_handle(&element);
        // SAFETY: `element` was built with `cache`.
        let neighbor = unsafe { uia.navigate(&element, &cache, &context.uia_registry, kind) }
            .map_err(|error| ReadError::Failed(format!("UIA navigation failed: {error}")))?;
        (neighbor, from_window)
    } else {
        let from_window = context.msaa_registry.key_of(node_id).map(|key| key.0);
        let neighbor = verbatim_ia2::acquire::navigate(node_id, &context.msaa_registry, kind)?;
        (neighbor, from_window)
    };
    Ok(neighbor.map(|neighbor| corrected_backend(context, client, from_window, neighbor, kind)))
}

/// `neighbor` through the backend its window uses, as NVDA corrects the API
/// of an object reached by navigation (`correctAPIForRelation`): an MSAA
/// object in a different window from `from_window`, whose window is UIA,
/// becomes that window's UIA element; a UIA element that is the root of a
/// different window, whose window is MSAA, becomes that window's MSAA
/// object (its client area when reached as a parent, its window object
/// otherwise). Anything else, or a read that fails, is kept as it is.
fn corrected_backend(
    context: &Context,
    client: &mut Client,
    from_window: Option<isize>,
    neighbor: NodeSnapshot,
    kind: QueryKind,
) -> NodeSnapshot {
    let Some(from_window) = from_window.filter(|&hwnd| hwnd != 0) else {
        return neighbor;
    };
    if context.uia_registry.runtime_id_of(neighbor.id).is_some() {
        // SAFETY: the registry's element was built with the base cache
        // request, which caches the native window handle.
        let window = context
            .uia_registry
            .element_of(neighbor.id)
            .and_then(|agile| agile.resolve().ok())
            .map_or(0, |element| unsafe {
                cached_native_window_handle(&element)
            });
        if window == 0 || window == from_window || window_uses_uia(context, window) {
            return neighbor;
        }
        let object = if kind == QueryKind::Parent {
            OBJID_CLIENT
        } else {
            OBJID_WINDOW
        };
        return verbatim_ia2::acquire::snapshot_from_event(
            window,
            object.0,
            CHILDID_SELF,
            &context.msaa_registry,
        )
        .unwrap_or(neighbor);
    }
    let Some((window, _, _)) = context.msaa_registry.key_of(neighbor.id) else {
        return neighbor;
    };
    if window == 0 || window == from_window || !window_uses_uia(context, window) {
        return neighbor;
    }
    client
        .uia_and_cache()
        .ok()
        .and_then(|(uia, cache)| uia.element_from_handle(window, &cache).ok())
        // SAFETY: the element was just built with the base cache request.
        .map_or(neighbor, |element| unsafe {
            snapshot_from_cached_element(&element, &context.uia_registry)
        })
}

/// Activates a node.
/// How many ancestors activation tries when the node itself has no action.
const ACTIVATION_PARENT_HOPS: u32 = 8;

/// Activates `node_id`, or failing that its nearest ancestor that can be
/// activated, as NVDA's review activate walks up the navigator object's
/// parents until one performs an action, and answers the action's name.
pub(super) fn activate(
    context: &Context,
    client: &mut Client,
    node_id: NodeId,
) -> Result<Option<ActionName>, ReadError> {
    let mut current = node_id;
    let mut first_error = None;
    for _ in 0..=ACTIVATION_PARENT_HOPS {
        match activate_one(context, client, current) {
            Ok(action) => return Ok(action),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match navigate(context, client, current, QueryKind::Parent) {
            Ok(Some(parent)) => current = parent.id,
            _ => break,
        }
    }
    Err(first_error.unwrap_or_else(|| ReadError::Failed("nothing to activate".to_owned())))
}

/// Activates exactly `node_id`, answering the action's name.
fn activate_one(
    context: &Context,
    client: &mut Client,
    node_id: NodeId,
) -> Result<Option<ActionName>, ReadError> {
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

#[cfg(test)]
mod tests {
    use verbatim_model::{Backend, NodeDetails, NodeId, NodeSnapshot, Role, StateSet};

    use super::splice;

    fn node(id: u64) -> NodeSnapshot {
        NodeSnapshot {
            id: NodeId::new(id),
            backend: Backend::Uia,
            role: Role::Group,
            name: None,
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        }
    }

    fn ids(chain: &[NodeSnapshot]) -> Vec<NodeId> {
        chain.iter().map(|node| node.id).collect()
    }

    #[test]
    fn a_walk_meeting_the_previous_chain_reuses_what_lies_above() {
        let previous = vec![node(1), node(2), node(3)];
        // The new walk read 4 and then met 2, its outermost entry.
        let spliced = splice(&previous, vec![node(2), node(4)], NodeId::new(2));
        assert_eq!(ids(&spliced), ids(&[node(1), node(2), node(4)]));
    }

    #[test]
    fn a_walk_meeting_nothing_known_is_kept_whole() {
        let spliced = splice(&[node(1)], vec![node(5), node(6)], NodeId::new(9));
        assert_eq!(ids(&spliced), ids(&[node(5), node(6)]));
    }
}
