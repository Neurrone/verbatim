//! The worker's reads from the application: windows, the focus, a node's
//! ancestors, navigation, activation, and tree dumps, through whichever
//! backend owns the window. Every function here may call into the
//! application and so runs only on the worker, under its deadline.

#![forbid(unsafe_code)]

use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::CHILDID_SELF;
use verbatim_model::{
    ActionName, Backend, NodeDetails, NodeId, NodeSnapshot, QueryKind, Role, StateSet,
};
use verbatim_uia::map::with_legacy_checked_state;
use verbatim_uia::{
    Ancestor, AncestorWalk, Cached, DialogText, Fill, FocusInput, Live, Node, Request, Shape, Uia,
};
use verbatim_uia_rops::{
    FocusAncestry, FocusQuery, NavigationDirection, Path, StepQuery, focus_ancestry,
    navigation_step,
};

use crate::arbitration::{WindowClasses, time_limit, window_class_name};
use crate::protocol::{DumpedTree, FocusNow, FocusedControl};

use super::Context;
use super::text_reads::CONSOLE_WINDOW_CLASS;
use super::window::{
    desktop_window, foreground_window_of, main_window_of, now_ms, served_by, top_level_of,
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
/// because its containers took too long to read. Checked between the
/// walk's calls, each of which UIA ends after its connection or
/// transaction timeout ([`verbatim_uia::CONNECTION_TIMEOUT`],
/// [`verbatim_uia::TRANSACTION_TIMEOUT`]), so a walk ends within this budget and
/// one call; the worker's deadline for the entry, enforced by abandonment,
/// bounds the rest.
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

    /// The UIA client and the request `request` builds for the details the
    /// active theme wants read ([`Context::node_request`],
    /// [`Context::ancestor_request`]).
    fn uia_with<S: Shape>(
        &mut self,
        context: &Context,
        request: fn(&Context, &Uia) -> windows::core::Result<Request<S>>,
    ) -> Result<(&Uia, Request<S>), ReadError> {
        let uia = self
            .uia()
            .ok_or_else(|| ReadError::Failed("could not create a UIA client".to_owned()))?;
        let request = request(context, uia).map_err(|error| {
            ReadError::Failed(format!("could not build a cache request: {error}"))
        })?;
        Ok((uia, request))
    }
}

/// Decides a window's backend with the outpost's arbitrator
/// ([`crate::arbitration::uses_uia`]), connecting to the window once when
/// its probe finds a provider ([`crate::arbitration::Connect::AfterYes`]).
/// Returns `true` for UIA.
pub(super) fn window_uses_uia(context: &Context, hwnd: isize) -> bool {
    let (uia, kept) = crate::arbitration::uses_uia_keeping(
        &context.arbitrator,
        hwnd,
        crate::arbitration::Connect::AfterYes,
        || verbatim_uia::console_reports_formatting(hwnd),
    );
    if kept {
        // The event thread may narrow its hook now that it knows.
        context.nudge_event_thread(super::narrowing::Nudge::Decide);
    }
    uia
}

/// Whether the outpost's application serves window `hwnd`, by the
/// window's provider process, read locally ([`served_by`]). An outpost
/// connects to, probes, and walks up into only such windows
/// (`docs/design/listener-without-connections.md`, section 4.3).
pub(super) fn serves(context: &Context, hwnd: isize) -> bool {
    served_by(hwnd, context.target_pid())
}

/// [`window_uses_uia`] for a window of the outpost's own application;
/// `false`, without a probe, for any other process's window, which the
/// outpost reads through MSAA whether it has a provider or not
/// (`docs/design/listener-without-connections.md`, sections 4.3 and 4.4).
fn own_window_uses_uia(context: &Context, hwnd: isize) -> bool {
    serves(context, hwnd) && window_uses_uia(context, hwnd)
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
            verbatim_ia2::acquire::Purpose::Announce,
        )?;
        return Some((Backend::Msaa, node));
    }
    if window_uses_uia(context, hwnd) {
        let (uia, nodes) = client.uia_with(context, Context::node_request).ok()?;
        let element = uia.window_element(hwnd, &nodes).ok()?;
        let node = element.snapshot(&context.uia_registry);
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
            verbatim_ia2::acquire::Purpose::Announce,
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
/// itself) and reuses the rest; within [`ENRICHMENT_BUDGET`], checked
/// between its calls. Failures degrade to no containers or no selected
/// child; running out of time to containers unknown. Enrichment never
/// turns a focus into an error.
pub(super) fn uia_enrichment(
    context: &Context,
    (uia, nodes, ancestors): (&Uia, &Request<Node>, &Request<Ancestor>),
    element: &Cached<Node>,
    role: Role,
    previous: &[NodeSnapshot],
) -> Enrichment {
    let clock = context.clock();
    let out_of_time = time_limit(&clock, ENRICHMENT_BUDGET);
    let ancestors = uia_ancestors(
        context,
        (uia, ancestors),
        (element.live(), element.read().role.hwnd),
        previous,
        Some(&out_of_time),
    );
    let selected = if wants_selected_child(role) {
        match uia.selected_node(element.live(), nodes, &context.uia_registry) {
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
    (uia, request): (&Uia, &Request<Ancestor>),
    (element, element_window): (&Live, isize),
    previous: &[NodeSnapshot],
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Option<Vec<NodeSnapshot>> {
    let desktop = desktop_window();
    // Another process's window is read through MSAA without a probe: a
    // step can still reach one, as when an owner is shown between the
    // walk's boundary check and its step.
    let read_by_msaa = |hwnd: isize| hwnd != desktop && !own_window_uses_uia(context, hwnd);
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    let Ok((uia_chain, crossed, walked)) = uia.ancestors(
        (element, element_window),
        request,
        &context.uia_registry,
        MAX_ANCESTOR_HOPS,
        &verbatim_uia::AncestorStops {
            read_by_other_api: &read_by_msaa,
            known: &known,
            out_of_time,
        },
    ) else {
        return Some(Vec::new());
    };
    // The walk stops below the process boundary and answers the parent or
    // owner window there, which is continued through MSAA, as it continues
    // a window of its own application read through MSAA.
    uia_chain_onward(
        context,
        (uia, request),
        (uia_chain, crossed, walked, None),
        previous,
        out_of_time,
    )
}

/// A UIA ancestor chain, outermost first, as a walk ended (`walked`),
/// completed outward: spliced onto `previous` where it met a known
/// ancestor, or continued through MSAA from the window `crossed` names, as
/// NVDA switches API when its walk crosses into a window read through
/// MSAA. A walk that stopped below the process boundary, its outermost
/// element's own window (`outermost`) the child of another process's
/// window or a top-level window another process's owns
/// ([`verbatim_uia::parent_across_process_boundary`]), is continued from
/// that parent or owner window through MSAA in the same way, whether that
/// window has a UIA provider or not, without probing it, as the Settings
/// page's content is continued from its frame
/// (`docs/design/listener-without-connections.md`, sections 4.2 to 4.4).
/// `None` when the walk ran out of time.
fn uia_chain_onward(
    context: &Context,
    (uia, request): (&Uia, &Request<Ancestor>),
    (uia_chain, crossed, walked, outermost): (
        Vec<NodeSnapshot>,
        Option<isize>,
        AncestorWalk,
        Option<isize>,
    ),
    previous: &[NodeSnapshot],
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Option<Vec<NodeSnapshot>> {
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    match walked {
        AncestorWalk::OutOfTime => return None,
        AncestorWalk::MetKnown(met) => return Some(splice(previous, uia_chain, met)),
        AncestorWalk::Complete => {}
    }
    let crossed = crossed.or_else(|| {
        outermost
            .filter(|&window| window != 0)
            .and_then(verbatim_uia::parent_across_process_boundary)
    });
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
            verbatim_ia2::acquire::Purpose::Context,
        ) else {
            return Some(uia_chain);
        };
        let below: Vec<NodeSnapshot> = std::iter::once(client.clone()).chain(uia_chain).collect();
        if known(client.id) {
            return Some(splice(previous, below, client.id));
        }
        let above = msaa_ancestors(
            context,
            Some((uia, request)),
            &client,
            previous,
            out_of_time,
        )?;
        Some(above.into_iter().chain(below).collect())
    })();
    tracing::debug!(
        hwnd,
        elapsed_us = crossing.elapsed().as_micros(),
        "UIA ancestors continued through MSAA"
    );
    chain
}

/// A UIA focus's ancestors, outermost first, from `ancestors`, the
/// raw-view chain the focus listener read from the event's own element,
/// nearest first: filtered as a walk here filters them, stopping at a
/// container of `previous` and reusing the rest, and, where the chain
/// reaches the root of a window read through MSAA, continued from that
/// window's client area through MSAA, as [`uia_ancestors`] does. Each node
/// is minted from its runtime id, with no element: the outpost finds its
/// own copy of an element only when it needs one (`super::focus_path`).
/// `None` when the continuation through MSAA ran out of time.
pub(super) fn uia_chain_from_read(
    context: &Context,
    client: &mut Client,
    (ancestors, own_window): (&[crate::protocol::UiaAncestorFact], isize),
    previous: &[NodeSnapshot],
) -> Option<Vec<NodeSnapshot>> {
    let desktop = desktop_window();
    // Another process's window is read through MSAA without a probe: a
    // step can still reach one, as when an owner is shown between the
    // walk's boundary check and its step.
    let read_by_msaa = |hwnd: isize| hwnd != desktop && !own_window_uses_uia(context, hwnd);
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    let parents: Vec<verbatim_uia::AncestorParts> = ancestors
        .iter()
        .map(|ancestor| verbatim_uia::AncestorParts {
            parts: verbatim_uia::map::CachedUiaParts {
                runtime_id: ancestor.snapshot.runtime_id.clone(),
                role: ancestor.snapshot.role,
                name: ancestor.snapshot.name.clone(),
                value: ancestor.snapshot.value.clone(),
                states: ancestor.snapshot.states,
                details: ancestor.snapshot.details.clone(),
            },
            hwnd: ancestor.hwnd,
            content: ancestor.content,
        })
        .collect();
    let (chain, crossed, walked) = Uia::ancestor_chain_from_parts(
        &parents,
        &context.uia_registry,
        &verbatim_uia::AncestorStops {
            read_by_other_api: &read_by_msaa,
            known: &known,
            out_of_time: None,
        },
    );
    // The listener's walk stops below the process boundary, at the window
    // its last ancestor, or the focus itself, has.
    let outermost = ancestors
        .last()
        .map_or(own_window, |ancestor| ancestor.hwnd);
    let clock = context.clock();
    let out_of_time = time_limit(&clock, ENRICHMENT_BUDGET);
    match client.uia_with(context, Context::ancestor_request) {
        Ok((uia, request)) => uia_chain_onward(
            context,
            (uia, &request),
            (chain, crossed, walked, Some(outermost)),
            previous,
            Some(&out_of_time),
        ),
        // Without a client the MSAA continuation cannot cross back into
        // UIA, so it is not made; the chain read is reported as it is.
        Err(_) => match walked {
            AncestorWalk::MetKnown(met) => Some(splice(previous, chain, met)),
            AncestorWalk::Complete | AncestorWalk::OutOfTime => Some(chain),
        },
    }
}

/// The element a focus-now query read no longer has the keyboard focus,
/// read live inside the provider ([`uia_remote_enrichment`]).
pub(super) struct NotFocused;

/// The ancestors and selected child of `element`, the focused element a
/// focus-now query read, in one round trip run inside the application's
/// provider ([`verbatim_uia_rops::focus_ancestry`]), with the same stops and
/// the same continuation through MSAA as [`uia_enrichment`]. An element
/// that no longer has the keyboard focus, read live, is answered
/// [`NotFocused`] and nothing else is read. `None`, having made no call,
/// when remote operations are off or `window`, the window the focus is in,
/// is read the classic way: the caller reads it as before. A UIA focus
/// event's ancestors are the focus listener's read instead
/// ([`uia_chain_from_read`]).
///
/// A program that fails is answered by the classic walk for this call,
/// and logged with the instruction that failed and the Rust line that
/// emitted it; one whose element could not be imported (a client-side
/// proxy) also has `window` read the classic way from then on. A walk that
/// fails, the program by UIA's transaction timeout or the classic walk any
/// way, reports the containers unknown.
pub(super) fn uia_remote_enrichment(
    context: &Context,
    (uia, request): (&Uia, &Request<Ancestor>),
    element: &Cached<Node>,
    previous: &[NodeSnapshot],
    window: Option<isize>,
) -> Option<Result<Enrichment, NotFocused>> {
    if !context.tries_remote(window) {
        return None;
    }
    let clock = context.clock();
    let out_of_time = time_limit(&clock, ENRICHMENT_BUDGET);
    // The previous chain's UIA nodes, by runtime id, for the walk to stop
    // at; their node ids, in the same order, for splicing.
    let (known_ids, known): (Vec<NodeId>, Vec<Vec<i32>>) = previous
        .iter()
        .filter_map(|node| {
            let runtime_id = context.uia_registry.runtime_id_of(node.id)?;
            Some((node.id, runtime_id))
        })
        .unzip();
    // The query reads the focus's control type and window; its ancestors
    // and selected child come back as ancestors, the child reported as
    // its node.
    let input = element.narrow::<FocusInput>();
    let fill = Fill::new(request);
    let query = FocusQuery {
        element: &input,
        known: &known,
        previous: None,
        depth_limit: MAX_ANCESTOR_HOPS,
        fill: &fill,
        out_of_time: Some(&out_of_time),
        require_focus: true,
        refresh: false,
        nearest_window: None,
        before_call: None,
    };
    // One remote operation, one transaction, which UIA's transaction
    // timeout bounds; the classic walk it falls back to checks the
    // budget between its calls.
    let (ancestry, path) = match focus_ancestry(uia, &query, true) {
        Ok((FocusAncestry::Focused(ancestry), path)) => (ancestry, path),
        Ok((FocusAncestry::NotFocused, _)) => return Some(Err(NotFocused)),
        Err(error) => {
            // The walk failed, by UIA's transaction timeout or otherwise:
            // the focus is reported with its containers unknown, as when
            // the walk runs out of time, never as having none.
            tracing::debug!(%error, "the focus ancestry could not be read");
            return Some(Ok((None, None)));
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
        .map(|child| child.snapshot(registry));
    let ancestors = if ancestry.out_of_time {
        None
    } else {
        let desktop = desktop_window();
        // Another process's window is read through MSAA without a probe,
        // as in the outpost's other walks.
        let read_by_msaa = |hwnd: isize| hwnd != desktop && !own_window_uses_uia(context, hwnd);
        let is_known = |id: NodeId| previous.iter().any(|node| node.id == id);
        let (chain, crossed, mut walked) = Uia::ancestors_from(
            &ancestry.ancestors,
            registry,
            &verbatim_uia::AncestorStops {
                read_by_other_api: &read_by_msaa,
                known: &is_known,
                out_of_time: None,
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
        // The program, and the classic walk it falls back to, stop below
        // the process boundary, at the window the last ancestor, or the
        // focus itself, has.
        let outermost = ancestry.ancestors.last().map_or_else(
            || element.read().role.hwnd,
            |ancestor| ancestor.read().node.role.hwnd,
        );
        uia_chain_onward(
            context,
            (uia, request),
            (chain, crossed, walked, Some(outermost)),
            previous,
            Some(&out_of_time),
        )
    };
    tracing::debug!(
        ?path,
        ancestors = ancestry.ancestors.len(),
        "UIA focus ancestry read"
    );
    Some(Ok((ancestors, selected)))
}

/// An MSAA node's ancestors, outermost first, without the window objects
/// above controls (see [`msaa_enrichment`]), stopping at a container of
/// `previous` and reusing the rest, or `None` when `out_of_time` said its time was up. Where
/// a parent lies in a window read through UIA, the walk continues from that
/// window's UIA element, as NVDA switches API when a parent is in such a
/// window (`correctAPIForRelation`): in a Save As dialog, the file name
/// box's containers above the shell's UIA view are UIA's. The window's own
/// element, the UIA object NVDA switches to there (`correctAPIForRelation`,
/// `NVDAObjects/window/__init__.py` 319), is reported only when it is focus
/// context by UIA's rule, as every UIA ancestor is
/// ([`Ancestor::is_focus_context`]). Without `uia` the walk stays in
/// MSAA.
fn msaa_ancestors(
    context: &Context,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    node: &NodeSnapshot,
    previous: &[NodeSnapshot],
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Option<Vec<NodeSnapshot>> {
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    // Another process's window is read through MSAA, whether it has a
    // provider or not, and is not probed.
    let read_by_uia = |hwnd: isize| uia.is_some() && own_window_uses_uia(context, hwnd);
    let limits = verbatim_ia2::acquire::AncestorLimits {
        max_hops: MAX_ANCESTOR_HOPS,
        known: &known,
        read_by_other_api: &read_by_uia,
        out_of_time,
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
            let Some((uia, request)) = uia else {
                return Some(chain);
            };
            let Ok(element) = uia.window_element(hwnd, request) else {
                return Some(chain);
            };
            let top = element.snapshot(&context.uia_registry);
            let below: Vec<NodeSnapshot> = if element.read().is_focus_context(&top) {
                let top_id = top.id;
                let below: Vec<NodeSnapshot> = std::iter::once(top).chain(chain).collect();
                if known(top_id) {
                    return Some(splice(previous, below, top_id));
                }
                below
            } else {
                chain
            };
            let above = uia_ancestors(
                context,
                (uia, request),
                (element.live(), hwnd),
                previous,
                out_of_time,
            )?;
            Some(above.into_iter().chain(below).collect())
        }
    }
}

/// An MSAA node's ancestors and, for a selection container, its selected
/// child, stopping at a container of `previous` and reusing the rest, within
/// [`ENRICHMENT_BUDGET`] (checked between calls; an MSAA call has no bound
/// of its own, and a UIA call in the walk ends after UIA's call timeout).
/// Failures degrade to no containers or no selected child; running out of
/// time to containers unknown.
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
    let clock = context.clock();
    let out_of_time = time_limit(&clock, ENRICHMENT_BUDGET);
    let ancestors = match client.uia_with(context, Context::ancestor_request) {
        Ok((uia, request)) => msaa_ancestors(
            context,
            Some((uia, &request)),
            node,
            previous,
            Some(&out_of_time),
        ),
        Err(_) => msaa_ancestors(context, None, node, previous, Some(&out_of_time)),
    };
    let selected = if wants_selected_child(node.role) {
        verbatim_ia2::acquire::selected_child(node.id, &context.msaa_registry)
    } else {
        None
    };
    (ancestors, selected)
}

/// How long gathering a dialog's own text through UI Automation may take,
/// all its calls together: the text is worth a short wait, but the focus it
/// is announced with must not be held back long. Gathering reads the
/// dialog's descendants, many calls, so it has this deadline of its own,
/// enforced by abandonment ([`verbatim_uia::BoundedClient`]).
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
                let text = dialog_text_of(context, node);
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
fn dialog_text_of(context: &Context, node: &NodeSnapshot) -> Option<String> {
    use crate::dialog_text::{UiaDialog, dialog_text};
    match node.backend {
        Backend::Msaa => {
            verbatim_ia2::dialog::DialogObject::of_node(node.id, &context.msaa_registry)
                .and_then(|dialog| dialog_text(&dialog))
        }
        Backend::Uia => {
            // A dialog the focus listener read in the focus's chain, whose
            // element this outpost does not hold, is found by its path.
            let element = context.uia_registry.element_of(node.id);
            let route = if element.is_some() {
                None
            } else {
                let runtime_id = context.uia_registry.runtime_id_of(node.id)?;
                Some(context.focus_route(&runtime_id)?)
            };
            let remote = route
                .as_ref()
                .is_some_and(|route| context.tries_remote(Some(route.window)));
            let gates = crate::gates::node_gates(&context.fetches());
            let clock = context.clock();
            let gathered = context.bounded.run(DIALOG_TEXT_WAIT, move |uia| {
                // Nothing of the dialog's own element is read: its text is
                // gathered from its children. A kept element is resolved
                // here, on the bounded client's thread, as `Kept`.
                let dialog = match (element, route) {
                    (Some(element), _) => element.resolve().ok()?.live().clone(),
                    (None, Some(route)) => {
                        let nodes = uia.request::<Node>(gates).ok()?;
                        let found =
                            super::focus_path::find(uia, &route, &nodes, remote, &clock).0?;
                        found.live().clone()
                    }
                    (None, None) => return None,
                };
                // Each child's description whatever the theme, as NVDA's
                // dialog text includes it.
                let children = uia.request_fixed::<DialogText>().ok()?;
                dialog_text(&UiaDialog::new(uia, &children, dialog))
            });
            gathered.unwrap_or_else(|unanswered| {
                tracing::debug!(?unanswered, "a dialog's text was not gathered");
                None
            })
        }
    }
}

/// The application's focused control with its ancestors, selected child,
/// and window, through the focus window's backend. `None` when the keyboard
/// focus is not in a window this application serves ([`control_window`]:
/// a console window is its console host's), or is on Core's hidden frame,
/// so the focused element is never read in another application, whose
/// window this outpost would connect to
/// (`docs/design/listener-without-connections.md`, section 4.3).
pub(super) fn focused_control(context: &Context, client: &mut Client) -> Option<FocusedControl> {
    let hwnd = control_window(context.target_pid(), || context.focus_window())?;
    if window_belongs_to_hidden_frame(hwnd) {
        return None;
    }
    if window_uses_uia(context, hwnd) {
        let (uia, nodes) = client.uia_with(context, Context::node_request).ok()?;
        let ancestor_request = context.ancestor_request(uia).ok()?;
        let element = (context.focused_element)(uia, &nodes).ok()?;
        let mut node = with_legacy_checked_state(
            element.live().element(),
            element.snapshot(&context.uia_registry),
        );
        console_text_area(&mut node, Some(hwnd));
        // A focus that has moved on since the focused element was read is
        // still this query's answer, read the classic way.
        let (ancestors, selected_child) = match uia_remote_enrichment(
            context,
            (uia, &ancestor_request),
            &element,
            &[],
            Some(hwnd),
        ) {
            Some(Ok(enrichment)) => enrichment,
            Some(Err(NotFocused)) | None => uia_enrichment(
                context,
                (uia, &nodes, &ancestor_request),
                &element,
                node.role,
                &[],
            ),
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
            verbatim_ia2::acquire::focused_snapshot(context.target_pid(), &context.msaa_registry)?;
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

/// The window whose focused element [`focused_control`] reads for the
/// application `target_pid`: the focus window `focus` answers, only when
/// that application serves it ([`served_by`]), so a console's client never
/// reads its console window's focus, which its console host serves.
fn control_window(target_pid: u32, focus: impl FnOnce() -> Option<isize>) -> Option<isize> {
    focus().filter(|&hwnd| served_by(hwnd, target_pid))
}

/// Makes `node`, a focus read through UIA in `window`, the console host's
/// text area when it is one: a terminal, known by its console window
/// (Windows Terminal's control is known by its UIA class), with its name,
/// "Text Area", which is not localized, dropped, as NVDA's console class
/// drops it (`WinConsoleUIA`, chosen by the "Text Area" automation id in
/// `NVDAObjects/UIA/winConsoleUIA.py`, `findExtraOverlayClasses`, whichever
/// way the object was reached), so the focus says "terminal". Every way a
/// focus is read applies it: its event and a focus-now query alike.
pub(super) fn console_text_area(node: &mut NodeSnapshot, window: Option<isize>) {
    console_text_area_in(node, || window.map(window_class_name));
}

/// [`console_text_area`] with the class of the focus's window read by
/// `window_class`, only for a node that may be a text area.
fn console_text_area_in(node: &mut NodeSnapshot, window_class: impl FnOnce() -> Option<String>) {
    if matches!(node.role, Role::EditableText | Role::Document)
        && window_class().as_deref() == Some(CONSOLE_WINDOW_CLASS)
    {
        node.role = Role::Terminal;
        node.name = None;
    }
}

/// The answer to a focus-now query: the application's foreground window, if
/// it holds the system foreground, and its focused control. A console host's
/// is its console window ([`foreground_window_of`]).
pub(super) fn focus_now(context: &Context, client: &mut Client) -> FocusNow {
    let observed_at_ms = now_ms();
    let window = foreground_window_of(context.target_pid())
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

/// Resolves a UIA node to a live element filled by `nodes`: the registry's
/// kept element first (refreshing its cache both updates it and proves it
/// still answers), then, if that element is gone, a runtime-id search of the
/// application's top-level windows. A kept element that fails for another
/// reason, such as a busy provider timing out, is kept and the read fails:
/// it may well still be alive, and the search would miss a virtualized one.
fn resolve_uia_element(
    context: &Context,
    uia: &Uia,
    nodes: &Request<Node>,
    node_id: NodeId,
) -> Result<Cached<Node>, ReadError> {
    if let Some(agile) = context.uia_registry.element_of(node_id) {
        if let Ok(element) = agile.resolve() {
            match element.live().refresh(nodes) {
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
    // An element of the chain the focus listener read for the focus is
    // found by its path, without a search.
    if let Some(route) = context.focus_route(&runtime_id) {
        let remote = context.tries_remote(Some(route.window));
        let (found, import_failed) =
            super::focus_path::find(uia, &route, nodes, remote, &context.clock());
        if import_failed {
            context.read_classically(route.window);
        }
        if let Some(element) = found {
            let _ = context
                .uia_registry
                .id_for_element(&runtime_id, &element.narrow());
            return Ok(element);
        }
    }
    // Gone only when every search finished without finding it; a search
    // that failed, a timeout above all, may have missed a live element.
    // Nothing of a window's own element is read: it is searched under. A
    // window a kept probe found to have a provider of its own, and whose
    // classes do not decide it, is looked up under the proxy rule, and a
    // lookup refused, as one of an application busy past the connection
    // timeout is, is a search that failed, not one that found nothing. A
    // verdict from the class lists or a forced one does not say the window
    // has a provider: the taskbar's `Shell_TrayWnd` is UIA by class and
    // served only by UIA's proxies, and the focus listener keeps a probe for
    // every UIA focus it reports, those of windows UIA by class included.
    let mut failed = None;
    for hwnd in top_level_windows(context.target_pid()) {
        let root = if probe_found_provider(context, hwnd) {
            match uia.answered_window_live(hwnd) {
                Ok(root) => root,
                Err(error) => {
                    failed = Some(error);
                    continue;
                }
            }
        } else {
            let Ok(root) = uia.window_live(hwnd) else {
                continue;
            };
            root
        };
        match uia.find_by_runtime_id(&root, &runtime_id, nodes) {
            Ok(Some(element)) => return Ok(element),
            Ok(None) => {}
            Err(error) => failed = Some(error),
        }
    }
    Err(failed.map_or(ReadError::Gone, |error| {
        ReadError::Failed(format!("searching for the element: {error}"))
    }))
}

/// Whether a kept probe found that `hwnd` has a UIA provider of its own on
/// a window its classes do not decide, without probing it. Local.
fn probe_found_provider(context: &Context, hwnd: isize) -> bool {
    let classes = WindowClasses::of(hwnd);
    context.arbitrator().probe_found_provider(hwnd, &classes)
}

/// A UIA node's live element, filled as a node, or the error a query
/// reports for it.
fn uia_node<'a>(
    context: &Context,
    client: &'a mut Client,
    node_id: NodeId,
) -> Result<(&'a Uia, Request<Node>, Cached<Node>), ReadError> {
    let (uia, nodes) = client.uia_with(context, Context::node_request)?;
    let element = resolve_uia_element(context, uia, &nodes, node_id)?;
    Ok((uia, nodes, element))
}

/// A node's ancestors, outermost first.
pub(super) fn ancestors(
    context: &Context,
    client: &mut Client,
    node_id: NodeId,
) -> Result<Vec<NodeSnapshot>, ReadError> {
    if context.uia_registry.runtime_id_of(node_id).is_some() {
        let (uia, _, element) = uia_node(context, client, node_id)?;
        let request = context.ancestor_request(uia).map_err(|error| {
            ReadError::Failed(format!("could not build a cache request: {error}"))
        })?;
        // The full chain: object navigation reuses nothing and waits for
        // the application as long as the query's own deadline allows.
        return Ok(uia_ancestors(
            context,
            (uia, &request),
            (element.live(), element.read().role.hwnd),
            &[],
            None,
        )
        .unwrap_or_default());
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
                let (uia, nodes, element) = uia_node(context, client, node_id)?;
                let from_window = verbatim_uia::nearest_window_handle(element.live().element());
                let neighbor = uia
                    .neighbor(element.live(), &nodes, &context.uia_registry, kind)
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
                Some(element) => with_legacy_checked_state(element.live().element(), neighbor),
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
    let own = element.read().hwnd;
    let window = (own != 0).then_some(own);
    if !context.tries_remote(window) || (own != 0 && top_level_of(own) == own) {
        return Ok(None);
    }
    let (uia, nodes) = client.uia_with(context, Context::node_request)?;
    let fill = Fill::new(&nodes);
    let query = StepQuery {
        element: &element,
        direction,
        fill: &fill,
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
        .map(|neighbor| neighbor.snapshot(&context.uia_registry));
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
        let window = context
            .uia_registry
            .element_of(neighbor.id)
            .and_then(|agile| agile.resolve().ok())
            .map_or(0, |element| element.read().hwnd);
        if window == 0 || window == from_window || own_window_uses_uia(context, window) {
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
            verbatim_ia2::acquire::Purpose::Announce,
        )
        .unwrap_or(neighbor);
    }
    let Some((window, _, _)) = context.msaa_registry.key_of(neighbor.id) else {
        return neighbor;
    };
    // Another process's window is kept as read: the outpost makes no
    // connection to it (`docs/design/listener-without-connections.md`,
    // section 4.3).
    if window == 0 || window == from_window || !own_window_uses_uia(context, window) {
        return neighbor;
    }
    client
        .uia_with(context, Context::node_request)
        .ok()
        .and_then(|(uia, nodes)| uia.window_element(window, &nodes).ok())
        .map_or(neighbor, |element| element.snapshot(&context.uia_registry))
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
        let (uia, _, element) = uia_node(context, client, node_id)?;
        return uia
            .activate(element.live().element())
            .map_err(|error| ReadError::Failed(format!("UIA activation failed: {error}")));
    }
    verbatim_ia2::acquire::activate(node_id, &context.msaa_registry).map_err(ReadError::from)
}

/// The application's tree from its top-level window.
pub(super) fn dump_tree(context: &Context, client: &mut Client) -> Result<DumpedTree, ReadError> {
    let hwnd = main_window_of(context.target_pid()).ok_or_else(|| {
        ReadError::Failed("the target application has no top-level window".to_owned())
    })?;
    if window_uses_uia(context, hwnd) {
        let (uia, nodes) = client.uia_with(context, Context::node_request)?;
        let element = uia.window_element(hwnd, &nodes).map_err(|error| {
            ReadError::Failed(format!(
                "could not fetch the top-level UIA element: {error}"
            ))
        })?;
        let (root, truncated) = uia
            .tree(
                &element,
                &nodes,
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

    use super::super::window::window_owner;
    use super::{CONSOLE_WINDOW_CLASS, console_text_area_in, control_window, splice};

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
    fn only_the_console_host_reads_the_focus_in_its_console_window() {
        let console = verbatim_test_console::Console::start();
        let (_, client) = window_owner(console.window);
        assert_eq!(control_window(client, || Some(console.window)), None);
        assert_eq!(
            control_window(console.host, || Some(console.window)),
            Some(console.window)
        );
    }

    #[test]
    fn a_walk_meeting_nothing_known_is_kept_whole() {
        let spliced = splice(&[node(1)], vec![node(5), node(6)], NodeId::new(9));
        assert_eq!(ids(&spliced), ids(&[node(5), node(6)]));
    }

    fn text_area(role: Role) -> NodeSnapshot {
        NodeSnapshot {
            role,
            name: Some("Text Area".to_owned()),
            ..node(2)
        }
    }

    #[test]
    fn a_text_area_in_a_console_window_is_a_nameless_terminal() {
        for role in [Role::EditableText, Role::Document] {
            let mut read = text_area(role);
            console_text_area_in(&mut read, || Some(CONSOLE_WINDOW_CLASS.to_owned()));
            assert_eq!((read.role, read.name), (Role::Terminal, None), "{role:?}");
        }
    }

    #[test]
    fn a_text_area_anywhere_else_is_left_as_read() {
        let mut read = text_area(Role::Document);
        console_text_area_in(&mut read, || Some("Notepad".to_owned()));
        assert_eq!(read, text_area(Role::Document));
        let mut window = text_area(Role::Window);
        console_text_area_in(&mut window, || panic!("a window's class is not read"));
        assert_eq!(window, text_area(Role::Window));
    }
}
