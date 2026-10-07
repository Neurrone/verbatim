//! The worker's reads from the application: windows, the focus, a node's
//! ancestors, navigation, activation, and tree dumps, through whichever
//! backend owns the window. Every function here may call into the
//! application and so runs only on the worker, under its deadline.

#![forbid(unsafe_code)]

use windows::Win32::UI::Accessibility::{IUIAutomationCacheRequest, IUIAutomationElement};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::CHILDID_SELF;
use verbatim_model::{
    ActionName, Backend, NodeDetails, NodeId, NodeSnapshot, QueryKind, Role, StateSet,
};
use verbatim_uia::map::{
    cached_native_window_handle, snapshot_from_cached_element, with_legacy_checked_state,
};
use verbatim_uia::{AncestorWalk, ElementExt, Uia, probe_server_side_provider};
use verbatim_uia_rops::{
    FocusAncestry, FocusQuery, NavigationDirection, Path, StepQuery, focus_ancestry,
    navigation_step,
};

use crate::arbitration::{PostProbeCheck, WindowClasses, post_probe_check, window_class_name};
use crate::protocol::{DumpedTree, FocusNow, FocusedControl};

use super::Context;
use super::window::{
    desktop_window, focus_window_of, foreground_window_of, main_window_of, now_ms, top_level_of,
    top_level_windows, window_belongs_to_hidden_frame, window_facts, window_is_hidden_frame,
    window_text,
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

    /// The UIA client and a cache request for the details the active theme
    /// wants read.
    fn uia_and_cache(
        &mut self,
        context: &Context,
    ) -> Result<(&Uia, IUIAutomationCacheRequest), ReadError> {
        let uia = self
            .uia()
            .ok_or_else(|| ReadError::Failed("could not create a UIA client".to_owned()))?;
        let cache = context.uia_cache(uia).map_err(|error| {
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
        let (uia, cache) = client.uia_and_cache(context).ok()?;
        let element = uia.element_from_handle(hwnd, &cache).ok()?;
        let node = snapshot_from_cached_element(&element, &context.uia_registry);
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
            match uia.selected_child(element, cache, &context.uia_registry) {
                Ok(selected) => selected,
                // The application did not answer in time, or the list is
                // gone: there is no selected child to report, and the log
                // says it was not read rather than that none was selected.
                Err(error) => {
                    tracing::debug!(%error, "the selected child could not be read");
                    None
                }
            }
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
    let desktop = desktop_window();
    let read_by_msaa = |hwnd: isize| hwnd != desktop && !window_uses_uia(context, hwnd);
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    let Ok((uia_chain, crossed, walked)) = uia.ancestor_chain(
        element,
        cache,
        &context.uia_registry,
        MAX_ANCESTOR_HOPS,
        &verbatim_uia::AncestorStops {
            read_by_other_api: &read_by_msaa,
            known: &known,
            deadline,
        },
    ) else {
        return Some(Vec::new());
    };
    uia_chain_onward(
        context,
        (uia, cache),
        (uia_chain, crossed, walked),
        previous,
        deadline,
    )
}

/// A UIA ancestor chain, outermost first, as a walk ended (`walked`),
/// completed outward: spliced onto `previous` where it met a known
/// ancestor, or continued through MSAA from the window `crossed` names, as
/// NVDA switches API when its walk crosses into a window read through
/// MSAA. `None` when the walk ran out of time.
fn uia_chain_onward(
    context: &Context,
    (uia, cache): (&Uia, &IUIAutomationCacheRequest),
    (uia_chain, crossed, walked): (Vec<NodeSnapshot>, Option<isize>, AncestorWalk),
    previous: &[NodeSnapshot],
    deadline: Option<std::time::Instant>,
) -> Option<Vec<NodeSnapshot>> {
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    match walked {
        AncestorWalk::OutOfTime => return None,
        AncestorWalk::MetKnown(met) => return Some(splice(previous, uia_chain, met)),
        AncestorWalk::Complete => {}
    }
    let Some(hwnd) = crossed else {
        return Some(uia_chain);
    };
    let crossing = std::time::Instant::now();
    let chain = (|| {
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
    })();
    tracing::debug!(
        hwnd,
        elapsed_us = crossing.elapsed().as_micros(),
        "UIA ancestors continued through MSAA"
    );
    chain
}

/// What reading a focused UIA element's context with a remote operation
/// found ([`uia_remote_enrichment`]).
#[expect(
    clippy::large_enum_variant,
    reason = "made once per focus and taken apart at once"
)]
pub(super) enum RemoteEnrichment {
    /// The element no longer has the keyboard focus, read live inside the
    /// provider: the focus it was read for is out of date.
    NotFocused,
    /// Its ancestors and selected child, and its nearest window, which the
    /// same round trip found (NVDA's `getNearestWindowHandle`).
    Read {
        /// The ancestors and selected child.
        enrichment: Enrichment,
        /// The element's own window or its nearest ancestor's.
        window: Option<isize>,
        /// Whether the element held under the focus's runtime id still has
        /// the keyboard focus, read live in the same round trip: `false`
        /// when it does not or cannot be read, `None` when none was given
        /// or the read failed as a whole.
        held_focused: Option<bool>,
    },
}

/// A focused UIA element's ancestors, selected child, and nearest window,
/// in one round trip run inside the application's provider
/// ([`verbatim_uia_rops::focus_ancestry`]), with the same stops and the
/// same continuation through MSAA as [`uia_enrichment`], and whether
/// `held`, the element the registry holds under the focus's runtime id,
/// still has the keyboard focus. `None`, having
/// made no call, when remote operations are off or `window`, the window
/// the focus is in, is read the classic way: the caller reads it as
/// before.
///
/// A program that fails is answered by the classic walk for this call,
/// and logged with the instruction that failed and the Rust line that
/// emitted it; one whose element could not be imported (a client-side
/// proxy) also has `window` read the classic way from then on. A walk that
/// fails, the program by UIA's transaction timeout or the classic walk any
/// way, reports the containers unknown.
pub(super) fn uia_remote_enrichment(
    context: &Context,
    uia: &Uia,
    cache: &IUIAutomationCacheRequest,
    element: &IUIAutomationElement,
    held: Option<&IUIAutomationElement>,
    previous: &[NodeSnapshot],
    window: Option<isize>,
) -> Option<RemoteEnrichment> {
    if !context.tries_remote(window) {
        return None;
    }
    let deadline = std::time::Instant::now() + ENRICHMENT_BUDGET;
    // The previous chain's UIA nodes, by runtime id, for the walk to stop
    // at; their node ids, in the same order, for splicing.
    let (known_ids, known): (Vec<NodeId>, Vec<Vec<i32>>) = previous
        .iter()
        .filter_map(|node| {
            let runtime_id = context.uia_registry.runtime_id_of(node.id)?;
            Some((node.id, runtime_id))
        })
        .unzip();
    let properties = verbatim_uia::cached_properties(context.fetches());
    let query = FocusQuery {
        element,
        known: &known,
        previous: held,
        depth_limit: MAX_ANCESTOR_HOPS,
        properties: &properties,
        deadline: Some(deadline),
    };
    let answer = uia
        .within(ENRICHMENT_BUDGET, |uia| focus_ancestry(uia, &query, true))
        .map_err(verbatim_uia_rops::Error::Uia)
        .and_then(|answer| answer);
    let (ancestry, path) = match answer {
        Ok((FocusAncestry::Focused(ancestry), path)) => (ancestry, path),
        Ok((FocusAncestry::NotFocused, _)) => return Some(RemoteEnrichment::NotFocused),
        Err(error) => {
            // The walk failed, by UIA's transaction timeout or otherwise:
            // the focus is reported with its containers unknown, as when
            // the walk runs out of time, never as having none, and its
            // window is found the usual way.
            tracing::debug!(%error, "the focus ancestry could not be read");
            return Some(RemoteEnrichment::Read {
                enrichment: (None, None),
                window: verbatim_uia::nearest_window_handle(element),
                held_focused: None,
            });
        }
    };
    if let Path::Fallback(error) = &path {
        tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
        if let (verbatim_uia_rops::Error::Import(_), Some(window)) = (error, window) {
            context.read_classically(window);
        }
    }
    let registry = &context.uia_registry;
    let selected = ancestry
        .selected_child
        .as_ref()
        .map(|child| snapshot_from_cached_element(child, registry));
    let ancestors = if ancestry.out_of_time {
        None
    } else {
        let desktop = desktop_window();
        let read_by_msaa = |hwnd: isize| hwnd != desktop && !window_uses_uia(context, hwnd);
        let is_known = |id: NodeId| previous.iter().any(|node| node.id == id);
        let (chain, crossed, mut walked) = Uia::ancestor_chain_from(
            &ancestry.ancestors,
            registry,
            &verbatim_uia::AncestorStops {
                read_by_other_api: &read_by_msaa,
                known: &is_known,
                deadline: None,
            },
        );
        // The program stops at any known ancestor, while only a reported
        // one ends the chain: a known ancestor that is not reported (the
        // previous focus, a list item, say) is still where the previous
        // chain takes over.
        if let (AncestorWalk::Complete, None, Some(index)) = (walked, crossed, ancestry.met_known)
            && let Some(&met) = known_ids.get(index)
        {
            walked = AncestorWalk::MetKnown(met);
        }
        uia_chain_onward(
            context,
            (uia, cache),
            (chain, crossed, walked),
            previous,
            Some(deadline),
        )
    };
    tracing::debug!(
        ?path,
        ancestors = ancestry.ancestors.len(),
        "UIA focus ancestry read"
    );
    Some(RemoteEnrichment::Read {
        enrichment: (ancestors, selected),
        window: ancestry.window,
        held_focused: ancestry.previous_focused,
    })
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
            let top = snapshot_from_cached_element(&element, &context.uia_registry);
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
    let ancestors = match client.uia_and_cache(context) {
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

/// How long gathering a dialog's own text through UI Automation waits for
/// the application at each call: the text is worth a short wait, but the
/// focus it is announced with must not be held back long.
const DIALOG_TEXT_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// The dialog this outpost last gathered text for in a foreground report,
/// and the text: the focus that follows it into the dialog reuses it rather
/// than reading the dialog again.
pub(super) type DialogMemo = Option<(NodeId, Option<String>)>;

/// Gives every dialog among `nodes` that is not among `previous`, the last
/// focus's chain, and has no description of its own, its own text as its
/// description (`docs/nvda/object-model.md`, "A dialog's own text"), while
/// descriptions are read at all. A dialog the focus stays inside is not read
/// again: the chain the focus is reported with keeps the text it was given.
///
/// `memo` is taken: a dialog it names is given its text without reading it
/// again. With `remember`, the last text read is kept in it for the next
/// report, which is how a foreground report hands its dialog to the focus
/// inside it.
pub(super) fn describe_dialogs<'a>(
    context: &Context,
    client: &mut Client,
    nodes: impl IntoIterator<Item = &'a mut NodeSnapshot>,
    (previous, memo, remember): (&[NodeSnapshot], &mut DialogMemo, bool),
) {
    let mut remembered = memo.take();
    if !context.fetches().description {
        return;
    }
    for node in nodes {
        if !crate::dialog_text::is_dialog(node.role)
            || node
                .details
                .description
                .as_deref()
                .is_some_and(|description| !description.trim().is_empty())
            || previous.iter().any(|known| known.id == node.id)
        {
            continue;
        }
        let text = match &remembered {
            Some((id, text)) if *id == node.id => text.clone(),
            _ => {
                let started = std::time::Instant::now();
                let text = dialog_text_of(context, client, node);
                tracing::debug!(
                    found = text.is_some(),
                    elapsed_us = started.elapsed().as_micros(),
                    "a dialog's text gathered"
                );
                text
            }
        };
        // A dialog that is both the foreground window and the focus's
        // ancestor in one report is read once.
        remembered = Some((node.id, text.clone()));
        node.details.description = text;
    }
    if remember {
        *memo = remembered;
    }
}

/// The own text of the dialog `node`, through the backend that reported it;
/// `None` when it has none or cannot be read.
fn dialog_text_of(context: &Context, client: &mut Client, node: &NodeSnapshot) -> Option<String> {
    use crate::dialog_text::{UiaObject, dialog_text};
    match node.backend {
        Backend::Msaa => {
            verbatim_ia2::dialog::DialogObject::of_node(node.id, &context.msaa_registry)
                .and_then(|dialog| dialog_text(&dialog))
        }
        Backend::Uia => {
            let element = context.uia_registry.element_of(node.id)?.resolve().ok()?;
            let properties = verbatim_uia::cached_properties(context.fetches());
            let uia = client.uia()?;
            uia.within(DIALOG_TEXT_WAIT, |uia| {
                dialog_text(&UiaObject::new(uia, &properties, element))
            })
            .ok()
            .flatten()
        }
    }
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
        let (uia, cache) = client.uia_and_cache(context).ok()?;
        let element = (context.focused_element)(uia, &cache).ok()?;
        let node = with_legacy_checked_state(
            &element,
            snapshot_from_cached_element(&element, &context.uia_registry),
        );
        // A focus that has moved on since the focused element was read is
        // still this query's answer, read the classic way.
        let (ancestors, selected_child) =
            match uia_remote_enrichment(context, uia, &cache, &element, None, &[], Some(hwnd)) {
                Some(RemoteEnrichment::Read { enrichment, .. }) => enrichment,
                Some(RemoteEnrichment::NotFocused) | None => {
                    uia_enrichment(context, uia, &cache, &element, node.role, &[])
                }
            };
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
/// still answers), then, if that element is gone, a runtime-id search of the
/// application's top-level windows. A kept element that fails for another
/// reason, such as a busy provider timing out, is kept and the read fails:
/// it may well still be alive, and the search would miss a virtualized one.
fn resolve_uia_element(
    context: &Context,
    uia: &Uia,
    cache: &IUIAutomationCacheRequest,
    node_id: NodeId,
) -> Result<IUIAutomationElement, ReadError> {
    if let Some(agile) = context.uia_registry.element_of(node_id) {
        if let Ok(element) = agile.resolve() {
            match element.build_updated_cache(cache) {
                Ok(fresh) => return Ok(fresh),
                Err(error) if !verbatim_uia::element_is_gone(&error) => {
                    return Err(ReadError::Failed(format!("reading the element: {error}")));
                }
                Err(_) => {}
            }
        }
        context.uia_registry.evict_element(node_id);
    }
    let runtime_id = context
        .uia_registry
        .runtime_id_of(node_id)
        .ok_or(ReadError::Gone)?;
    // Gone only when every search finished without finding it; a search
    // that failed, a timeout above all, may have missed a live element.
    let mut failed = None;
    for hwnd in top_level_windows(context.target_pid) {
        let Ok(root) = uia.element_from_handle(hwnd, cache) else {
            continue;
        };
        match uia.element_by_runtime_id(&root, &runtime_id, cache) {
            Ok(Some(element)) => return Ok(element),
            Ok(None) => {}
            Err(error) => failed = Some(error),
        }
    }
    Err(failed.map_or(ReadError::Gone, |error| {
        ReadError::Failed(format!("searching for the element: {error}"))
    }))
}

/// A UIA node's live element, or the error a query reports for it.
fn uia_node<'a>(
    context: &Context,
    client: &'a mut Client,
    node_id: NodeId,
) -> Result<(&'a Uia, IUIAutomationCacheRequest, IUIAutomationElement), ReadError> {
    let (uia, cache) = client.uia_and_cache(context)?;
    let element = resolve_uia_element(context, uia, &cache, node_id)?;
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
        let (neighbor, from_window) =
            if let Some(step) = uia_remote_step(context, client, node_id, kind)? {
                step
            } else {
                let (uia, cache, element) = uia_node(context, client, node_id)?;
                let from_window = verbatim_uia::nearest_window_handle(&element);
                let neighbor = uia
                    .navigate(&element, &cache, &context.uia_registry, kind)
                    .map_err(|error| {
                        if verbatim_uia::element_is_gone(&error) {
                            ReadError::Gone
                        } else {
                            ReadError::Failed(format!("UIA navigation failed: {error}"))
                        }
                    })?;
                (neighbor, from_window)
            };
        // A menu item reached by navigation reads its legacy checked state
        // as a focused one does.
        let neighbor = neighbor.map(|neighbor| {
            match context
                .uia_registry
                .element_of(neighbor.id)
                .and_then(|agile| agile.resolve().ok())
            {
                Some(element) => with_legacy_checked_state(&element, neighbor),
                None => neighbor,
            }
        });
        (neighbor, from_window)
    } else {
        let from_window = context.msaa_registry.key_of(node_id).map(|key| key.0);
        let neighbor = verbatim_ia2::acquire::navigate(node_id, &context.msaa_registry, kind)?;
        (neighbor, from_window)
    };
    Ok(neighbor.map(|neighbor| corrected_backend(context, client, from_window, neighbor, kind)))
}

/// A navigation step's neighbor, `None` at a tree edge, and the nearest
/// window of the node it was taken from.
type Stepped = (Option<NodeSnapshot>, Option<isize>);

/// One navigation step from a UIA node in one round trip, the neighbor and
/// the node's nearest window read inside the application's provider
/// ([`verbatim_uia_rops::navigation_step`]), from the element the registry
/// keeps, without first refreshing it: the program fails at once when the
/// element is gone. `None`, having made no call, when the step is taken
/// the classic way instead: remote operations are off or failed to import
/// for the node's window, the registry keeps no live element, the node is
/// a top-level window (a program's walk ends there, while the step from it
/// reaches the desktop or another application's window), or the element is
/// gone, which the classic way answers by searching for it.
fn uia_remote_step(
    context: &Context,
    client: &mut Client,
    node_id: NodeId,
    kind: QueryKind,
) -> Result<Option<Stepped>, ReadError> {
    let direction = match kind {
        QueryKind::Parent => NavigationDirection::Parent,
        QueryKind::NextSibling => NavigationDirection::NextSibling,
        QueryKind::PreviousSibling => NavigationDirection::PreviousSibling,
        QueryKind::FirstChild => NavigationDirection::FirstChild,
        _ => return Ok(None),
    };
    let Some(element) = context
        .uia_registry
        .element_of(node_id)
        .and_then(|agile| agile.resolve().ok())
    else {
        return Ok(None);
    };
    // The registry's element was built with the base cache request, which
    // caches the native window handle.
    let own = cached_native_window_handle(&element);
    let window = (own != 0).then_some(own);
    if !context.tries_remote(window) || (own != 0 && top_level_of(own) == own) {
        return Ok(None);
    }
    let (uia, _) = client.uia_and_cache(context)?;
    let properties = verbatim_uia::cached_properties(context.fetches());
    let query = StepQuery {
        element: &element,
        direction,
        properties: &properties,
    };
    let (step, path) = match navigation_step(uia, &query, true) {
        Ok(answer) => answer,
        Err(error)
            if error.hresult().is_some_and(|code| {
                verbatim_uia::element_is_gone(&windows::core::Error::from(code))
            }) =>
        {
            // Searched for, the classic way.
            context.uia_registry.evict_element(node_id);
            return Ok(None);
        }
        Err(error) => return Err(ReadError::Failed(format!("UIA navigation failed: {error}"))),
    };
    if let Path::Fallback(error) = &path {
        tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
        if let (verbatim_uia_rops::Error::Import(_), Some(hwnd)) = (error, step.window) {
            context.read_classically(top_level_of(hwnd));
        }
    }
    let neighbor = step
        .neighbor
        .map(|neighbor| snapshot_from_cached_element(&neighbor, &context.uia_registry));
    Ok(Some((neighbor, step.window)))
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
        // The registry's element was built with the base cache request,
        // which caches the native window handle.
        let window = context
            .uia_registry
            .element_of(neighbor.id)
            .and_then(|agile| agile.resolve().ok())
            .map_or(0, |element| cached_native_window_handle(&element));
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
        .uia_and_cache(context)
        .ok()
        .and_then(|(uia, cache)| uia.element_from_handle(window, &cache).ok())
        // The element was just built with the base cache request.
        .map_or(neighbor, |element| {
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
        return uia
            .activate(&element)
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
        let (uia, cache) = client.uia_and_cache(context)?;
        let element = uia.element_from_handle(hwnd, &cache).map_err(|error| {
            ReadError::Failed(format!(
                "could not fetch the top-level UIA element: {error}"
            ))
        })?;
        let (root, truncated) = uia
            .walk_tree(
                &element,
                &cache,
                &context.uia_registry,
                MAX_DUMP_DEPTH,
                MAX_DUMP_NODES,
            )
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
