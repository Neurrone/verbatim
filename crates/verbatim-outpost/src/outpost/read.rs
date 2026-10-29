//! The worker's reads from the application: windows, the focus, a node's
//! ancestors, navigation, activation, and tree dumps, through whichever
//! backend owns the window. Every function here may call into the
//! application and so runs only on the worker, under its deadline.

#![forbid(unsafe_code)]

use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::CHILDID_SELF;
use verbatim_model::{
    ActionName, Backend, NodeDetails, NodeId, NodeSnapshot, QueryKind, Role, State, StateSet,
};
use verbatim_uia::map::with_legacy_checked_state;
use verbatim_uia::{
    Ancestor, AncestorWalk, Cached, DialogText, Fill, FocusInput, Live, Node, NodeGates, Request,
    Shape, Uia,
};
use verbatim_uia_rops::{
    FocusAncestry, FocusQuery, NavigationDirection, Path, StepQuery, focus_ancestry,
    navigation_step,
};

use verbatim_core::focus_rules::already_entered;

use crate::dialog_text::is_dialog;

use crate::arbitration::{time_limit, window_class_name};
use crate::gates;
use crate::protocol::{DumpedTree, FocusNow, FocusedControl, now_us};

use super::Context;
use super::text_reads::CONSOLE_WINDOW_CLASS;
use super::window::{
    desktop_window, foreground_window_of, main_window_of, parent_window, served_by, top_level_of,
    window_belongs_to_hidden_frame, window_disabled, window_exists, window_facts,
    window_is_hidden_frame, window_text,
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
/// ([`crate::arbitration::uses_uia`]). Returns `true` for UIA.
pub(super) fn window_uses_uia(context: &Context, hwnd: isize) -> bool {
    let (uia, kept) = crate::arbitration::uses_uia_keeping(&context.arbitrator, hwnd, || {
        verbatim_uia::console_reports_formatting(hwnd)
    });
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

/// The backend of window `hwnd`, which an event or the listener names, as
/// [`window_uses_uia`] judges it, `Some(true)` for UIA, only when the
/// outpost's application serves it ([`serves`]): `None`, with no probe and
/// a debug line naming the window, for another process's window, which the
/// worker neither reads nor reports. Any process can raise a `WinEvent`
/// naming another's window, and the probe, and a console window's check,
/// would call into that process. Local calls before the verdict.
pub(super) fn own_window_verdict(context: &Context, hwnd: isize) -> Option<bool> {
    if !serves(context, hwnd) {
        tracing::debug!(
            hwnd,
            target_pid = context.target_pid(),
            "an event names a window the outpost's application does not serve; it is neither probed, read nor reported"
        );
        return None;
    }
    Some(window_uses_uia(context, hwnd))
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
    // The window's element is the connection to it.
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
        details: NodeDetails {
            window: Some(super::window::window_handle(hwnd)),
            ..NodeDetails::default()
        },
    }
}

/// Whether a focused node's selected child is reported with it: a list's
/// selected item, a tab control's active tab.
fn wants_selected_child(role: Role) -> bool {
    matches!(role, Role::List | Role::TabControl)
}

/// A focused UIA element's ancestors and, for a selection container or a
/// node whose value is its first selected item
/// (`verbatim_uia::map::value_from_selection`), its selected child, read
/// from the element already in hand, `nearest` its nearest window. The
/// walk stops at a container of `previous` (the last focus's ancestors and
/// the focus itself) and reuses the rest; within [`ENRICHMENT_BUDGET`],
/// checked between its calls, and completed above as [`complete`] says.
/// Failures degrade to no containers or no selected child. Enrichment
/// never turns a focus into an error. The automation id of the element's
/// raw-view parent, when the walk's first step read it, is put in
/// `parent_automation_id`.
pub(super) fn uia_enrichment(
    context: &Context,
    (uia, nodes, ancestors): (&Uia, &Request<Node>, &Request<Ancestor>),
    (element, nearest): (&Cached<Node>, isize),
    (role, parent_automation_id): (Role, &mut Option<String>),
    previous: &[NodeSnapshot],
) -> Enrichment {
    let clock = context.clock();
    let out_of_time = time_limit(&clock, ENRICHMENT_BUDGET);
    let ancestors = uia_ancestors(
        context,
        (uia, ancestors),
        (element.live(), element.read().role.hwnd, nearest),
        (previous, parent_automation_id),
        Some(&out_of_time),
    );
    let selected = if wants_selected_child(role) || element.read().value_from_selection() {
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
    (Some(ancestors), selected)
}

/// A UIA element's ancestors, outermost first, its own window handle
/// `element_window` and its nearest window `nearest`, 0 when not known.
/// Where the walk reaches the root of a window read through MSAA or as a
/// plain window ([`uia_walk_leaves`]), or stops below the process
/// boundary, it goes on from that window
/// ([`window_onward`]), as NVDA switches API when its walk crosses into
/// such a window: in File Explorer, the folder window above the UIA file
/// list is then the client area a foreground report already named, not a
/// second, UIA copy of it. A walk cut by its time or its hop limit, or
/// whose step to a parent failed, is completed as [`complete`] says. The
/// desktop window ends the walk. The automation id of the element's
/// raw-view parent, when the walk's first step read it, is put in
/// `parent_automation_id`.
fn uia_ancestors(
    context: &Context,
    (uia, request): (&Uia, &Request<Ancestor>),
    (element, element_window, nearest): (&Live, isize, isize),
    (previous, parent_automation_id): (&[NodeSnapshot], &mut Option<String>),
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Vec<NodeSnapshot> {
    let read_by_msaa = |hwnd: isize| uia_walk_leaves(context, hwnd);
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    let Ok((chain, crossed, walked)) = uia.ancestors(
        (element, element_window, nearest),
        request,
        &context.uia_registry,
        (MAX_ANCESTOR_HOPS, parent_automation_id),
        &verbatim_uia::AncestorStops {
            read_by_other_api: &read_by_msaa,
            known: &known,
            out_of_time,
        },
    ) else {
        return Vec::new();
    };
    // The walk stops below the process boundary itself and answers the
    // parent or owner window there as `crossed`.
    let above = uia_above(crossed, walked, None);
    complete(
        context,
        Some((uia, request)),
        (chain, above),
        previous,
        out_of_time,
    )
}

/// Whether a UIA walk stops at the root element of window `hwnd`, the
/// chain going on from that window ([`window_onward`]): a window read as a
/// plain window ([`reads_as_plain`]), another process's, a ghost's or a
/// hung one, asked first, so that no probe calls into it; or one this
/// outpost reads through MSAA. NVDA makes the step that reached it and,
/// for a plain window, reads that window as a plain window and goes on by
/// the window hierarchy with no further call (`correctAPIForRelation`,
/// `NVDAObjects/window/__init__.py` 85 to 97); a step can also reach
/// another process's window, as when an owner is shown between the walk's
/// boundary check and its step. The desktop ends a walk instead. Local
/// calls, and the verdict's probe for a window of this application.
fn uia_walk_leaves(context: &Context, hwnd: isize) -> bool {
    hwnd != desktop_window() && (reads_as_plain(context, hwnd) || !window_uses_uia(context, hwnd))
}

/// How a chain goes on above its outermost entry
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Above {
    /// It does not: the walk reached the desktop.
    Nothing,
    /// The previous chain takes over at this entry of it, the chain's
    /// outermost.
    Met(NodeId),
    /// From this window's own object ([`window_onward`]): the walk reached
    /// a window read through the other API, stopped below the process
    /// boundary, or its step to a parent failed.
    From(isize),
    /// By plain windows from this window, included: the walk was cut by
    /// its time or its depth limit.
    Plain(isize),
}

/// How a UIA walk that ended as `walked`, having crossed into the window
/// `crossed` names, goes on above its chain. A complete walk with no window
/// crossed into goes on above the process boundary when `outermost`, the
/// window of its outermost element, is below one
/// ([`verbatim_uia::parent_across_process_boundary`]), as a chain the
/// listener or a remote program read stops there.
fn uia_above(crossed: Option<isize>, walked: AncestorWalk, outermost: Option<isize>) -> Above {
    match (walked, crossed) {
        (AncestorWalk::MetKnown(met), _) => Above::Met(met),
        (_, Some(hwnd)) | (AncestorWalk::Failed(_, hwnd), None) => Above::From(hwnd),
        (AncestorWalk::Cut(hwnd), None) => Above::Plain(hwnd),
        (AncestorWalk::Complete, None) => outermost
            .filter(|&window| window != 0)
            .and_then(verbatim_uia::parent_across_process_boundary)
            .map_or(Above::Nothing, Above::From),
    }
}

/// `chain`, outermost first, completed above as `above` says: spliced
/// onto `previous` where it met a known ancestor, continued from a
/// window's own object ([`window_onward`]), or completed by plain windows
/// with no call ([`plain_windows_from`]).
fn complete(
    context: &Context,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    (chain, above): (Vec<NodeSnapshot>, Above),
    previous: &[NodeSnapshot],
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Vec<NodeSnapshot> {
    match above {
        Above::Nothing => chain,
        Above::Met(met) => splice(previous, chain, met),
        Above::From(hwnd) => window_onward(context, uia, hwnd, previous, out_of_time)
            .into_iter()
            .chain(chain)
            .collect(),
        Above::Plain(hwnd) => plain_windows_from(context, hwnd)
            .into_iter()
            .chain(chain)
            .collect(),
    }
}

/// Whether window `hwnd` is read as a plain window, with no call
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback"): a window another process serves, the parent or visible
/// owner above the process boundary and the windows above it, by the
/// decision of 2026-10-29, and a window of this application that cannot be
/// read, a ghost's or a hung one, by NVDA's rule
/// (`NVDAObjects/window/__init__.py` 85 to 97). Local calls.
pub(super) fn reads_as_plain(context: &Context, hwnd: isize) -> bool {
    !serves(context, hwnd) || context.window_unreadable(hwnd).is_some()
}

/// Window `hwnd` as a plain window (the design's Terms): role window,
/// named by `InternalGetWindowText`, and unavailable when its style
/// disables it, read from Windows' own state with no call into any
/// application. It is never focused, as NVDA's `Window` states hold only
/// invisible and unavailable. It is kept at its window object's address,
/// so a node read from it acquires the window's own object. Invisibility,
/// which NVDA also gives it, waits for the model to carry that state (the
/// order of work's step 8).
pub(super) fn plain_window(context: &Context, hwnd: isize) -> NodeSnapshot {
    let mut states = StateSet::new();
    if window_disabled(hwnd) {
        states = states.with(State::Disabled);
    }
    NodeSnapshot {
        id: context
            .msaa_registry
            .id_for((hwnd, OBJID_WINDOW.0, CHILDID_SELF)),
        backend: Backend::Msaa,
        role: Role::Window,
        name: window_text(hwnd),
        value: None,
        states,
        details: NodeDetails {
            window: Some(super::window::window_handle(hwnd)),
            ..NodeDetails::default()
        },
    }
}

/// How the MSAA focused-state test reads window `hwnd`, a parent window it
/// goes on to past a failed `accParent`
/// (`verbatim_ia2::acquire::FocusCandidate::has_focused_state`), by the
/// object NVDA's `Window._get_parent` chooses: above the process boundary,
/// a plain window, as is every window above it; a ghost's or a hung one, a
/// plain window; else by the outpost's verdict, its UIA element's focused
/// state, one call, or its MSAA client, which the test reads. A plain
/// window is never focused.
pub(super) fn parent_window_for_focused_state(
    context: &Context,
    client: &mut Client,
    hwnd: isize,
) -> verbatim_ia2::acquire::ParentWindow {
    use verbatim_ia2::acquire::ParentWindow;
    if !serves(context, hwnd) {
        return ParentWindow::AboveBoundary;
    }
    if context.window_unreadable(hwnd).is_some() {
        return ParentWindow::Plain;
    }
    if !window_uses_uia(context, hwnd) {
        return ParentWindow::Msaa;
    }
    let focused = client
        .uia_with(context, Context::node_request)
        .ok()
        .and_then(|(uia, nodes)| uia.window_element(hwnd, &nodes).ok())
        .map(|element| element.read().states.has_focus);
    ParentWindow::Uia(focused)
}

/// The plain windows from `hwnd` up to the desktop, which is none of them,
/// outermost first ([`plain_window`]): local calls only.
fn plain_windows_from(context: &Context, hwnd: isize) -> Vec<NodeSnapshot> {
    let desktop = desktop_window();
    let mut windows = Vec::new();
    let mut current = hwnd;
    while current != 0 && current != desktop && windows.len() < MAX_ANCESTOR_HOPS as usize {
        windows.push(plain_window(context, current));
        current = parent_window(current);
    }
    windows.reverse();
    windows
}

/// The chain from window `hwnd` up, outermost first, `hwnd`'s own entry
/// included: where a walk that ended below `hwnd` goes on
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback"). A window another process serves is a plain window, and so
/// is every window above it, with no call into that process; a ghost's or
/// a hung application's window is a plain window, and the chain goes on
/// from its parent window. Any other window is read by its own object
/// chosen as NVDA chooses it (`Window._get_parent`): its UIA element when
/// the outpost judges it UIA and `uia` is given, else its MSAA client, each
/// walked up by its backend; a window whose object cannot be acquired is a
/// plain window too, and the chain goes on from its parent window, as
/// NVDA's object for it is then its `Window`. Past `out_of_time`, the rest
/// is plain windows. The desktop ends it.
fn window_onward(
    context: &Context,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    hwnd: isize,
    previous: &[NodeSnapshot],
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Vec<NodeSnapshot> {
    let started = std::time::Instant::now();
    let desktop = desktop_window();
    // The plain windows passed on the way, nearest first.
    let mut passed = Vec::new();
    let mut current = hwnd;
    let mut above = Vec::new();
    while current != 0 && current != desktop {
        if !serves(context, current) || out_of_time.is_some_and(|out_of_time| out_of_time()) {
            above = plain_windows_from(context, current);
            break;
        }
        if context.window_unreadable(current).is_none()
            && let Some(chain) = window_object_onward(context, uia, current, previous, out_of_time)
        {
            above = chain;
            break;
        }
        passed.push(plain_window(context, current));
        current = parent_window(current);
    }
    tracing::debug!(
        hwnd,
        plain = passed.len(),
        elapsed_us = started.elapsed().as_micros(),
        "a chain went on from a window"
    );
    above.into_iter().chain(passed.into_iter().rev()).collect()
}

/// The chain from window `hwnd`'s own object up, outermost first, the
/// object included when it is focus context, for [`window_onward`]: its UIA
/// element and the UIA walk above it, when this outpost judges the window
/// UIA and `uia` is given, else its MSAA client and the MSAA walk above
/// it. `None` when the object cannot be acquired.
fn window_object_onward(
    context: &Context,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    hwnd: isize,
    previous: &[NodeSnapshot],
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Option<Vec<NodeSnapshot>> {
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    if let Some((uia, request)) = uia
        && window_uses_uia(context, hwnd)
    {
        let element = uia.window_element(hwnd, request).ok()?;
        let top = element.snapshot(&context.uia_registry);
        let top_id = top.id;
        // The window's own element, the UIA object NVDA switches to there
        // (`correctAPIForRelation`, `NVDAObjects/window/__init__.py` 319),
        // is reported only when it is focus context by UIA's rule, as
        // every UIA ancestor is ([`Ancestor::is_focus_context`]).
        let below = if element.read().is_focus_context(&top) {
            if known(top_id) {
                return Some(splice(previous, vec![top], top_id));
            }
            vec![top]
        } else {
            Vec::new()
        };
        let above = uia_ancestors(
            context,
            (uia, request),
            (element.live(), hwnd, hwnd),
            (previous, &mut None),
            out_of_time,
        );
        return Some(above.into_iter().chain(below).collect());
    }
    // The client area, as NVDA reads a parent window through MSAA.
    let client = verbatim_ia2::acquire::snapshot_from_event(
        hwnd,
        OBJID_CLIENT.0,
        CHILDID_SELF,
        &context.msaa_registry,
        verbatim_ia2::acquire::Purpose::Context,
    )?;
    if known(client.id) {
        let client_id = client.id;
        return Some(splice(previous, vec![client], client_id));
    }
    let above = msaa_ancestors(context, uia, &client, previous, out_of_time);
    Some(above.into_iter().chain(std::iter::once(client)).collect())
}

/// A UIA focus's ancestors, outermost first, from `ancestors`, the
/// raw-view chain the focus listener read from the event's own element,
/// nearest first, and `end`, how its walk ended: filtered as a walk here
/// filters them, stopping at a container of `previous` and reusing the
/// rest, and completed above as [`complete`] says: from a window read
/// through MSAA that the chain reaches, from the window above the process
/// boundary, where the listener's walk stops, from the window the listener
/// named after a failed step, or by plain windows from the window it named
/// after a cut walk. Each node is minted from its runtime id, with no
/// element: the outpost finds its own copy of an element only when it
/// needs one (`super::focus_path`).
pub(super) fn uia_chain_from_read(
    context: &Context,
    client: &mut Client,
    (ancestors, own_window, end): (
        &[crate::protocol::UiaAncestorFact],
        isize,
        crate::protocol::AncestorsEnd,
    ),
    previous: &[NodeSnapshot],
) -> Vec<NodeSnapshot> {
    use crate::protocol::AncestorsEnd;
    let read_by_msaa = |hwnd: isize| uia_walk_leaves(context, hwnd);
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
    // A stop within the chain read comes first; else the chain goes on as
    // the listener's walk ended. Its walk stops below the process
    // boundary, at the window its last ancestor, or the focus itself, has.
    let above = match (walked, crossed, end) {
        (AncestorWalk::MetKnown(_), _, _) | (_, Some(_), _) | (_, None, AncestorsEnd::Top) => {
            let outermost = ancestors
                .last()
                .map_or(own_window, |ancestor| ancestor.hwnd);
            uia_above(crossed, walked, Some(outermost))
        }
        (_, None, AncestorsEnd::ContinueFrom(hwnd)) => Above::From(hwnd),
        (_, None, AncestorsEnd::PlainFrom(hwnd)) => Above::Plain(hwnd),
    };
    let clock = context.clock();
    let out_of_time = time_limit(&clock, ENRICHMENT_BUDGET);
    // Without a client the continuation reads every window it reaches
    // through MSAA or as a plain window.
    let request = client.uia_with(context, Context::ancestor_request).ok();
    complete(
        context,
        request.as_ref().map(|(uia, request)| (*uia, request)),
        (chain, above),
        previous,
        Some(&out_of_time),
    )
}

/// The UIA nodes of `previous`, a previous chain, by runtime id, for a
/// walk to stop at, and their node ids, in the same order, for splicing.
fn known_uia_nodes(context: &Context, previous: &[NodeSnapshot]) -> (Vec<NodeId>, Vec<Vec<i32>>) {
    previous
        .iter()
        .filter_map(|node| {
            let runtime_id = context.uia_registry.runtime_id_of(node.id)?;
            Some((node.id, runtime_id))
        })
        .unzip()
}

/// The element a focus-now query read no longer has the keyboard focus,
/// read live inside the provider ([`uia_remote_enrichment`]).
pub(super) struct NotFocused;

/// The ancestors and selected child of `element`, the focused element a
/// focus-now query read, in one round trip run inside the application's
/// provider ([`verbatim_uia_rops::focus_ancestry`]), with the same stops and
/// the same completion above as [`uia_enrichment`]. An element that no
/// longer has the keyboard focus, read live, is answered [`NotFocused`] and
/// nothing else is read. `None`, having made no call, when remote
/// operations are off or `window`, the window the focus is in, is read the
/// classic way: the caller reads it as before. A UIA focus event's
/// ancestors are the focus listener's read instead
/// ([`uia_chain_from_read`]). A focus with a ghost's or a hung
/// application's window above its own ([`plain_window_above`]) is read by
/// the classic walk instead, since the program cannot judge a window
/// between its steps: the walk stops at its step into that window, and
/// the chain goes on from it as a plain window ([`window_onward`]).
///
/// A program that fails is answered by the classic walk for this call,
/// and logged with the instruction that failed and the Rust line that
/// emitted it; one whose element could not be imported (a client-side
/// proxy) also has `window` read the classic way from then on. A walk that
/// fails, the program by UIA's transaction timeout or the classic walk any
/// way, reports the containers unknown. A walk that ends at a failed or
/// missing parent below the top
/// ([`verbatim_uia_rops::Ancestry::failed_hop`]) goes on from the parent
/// window of its outermost element's nearest window, and one cut at the
/// depth limit, or the classic walk's time, is completed by plain windows
/// from that nearest window ([`complete`]). The automation id of the
/// element's raw-view parent, the first ancestor read, is put in
/// `parent_automation_id` when the walk read it.
#[expect(
    clippy::too_many_lines,
    reason = "the program, its fallback, and the chain's completion, in the order they run"
)]
pub(super) fn uia_remote_enrichment(
    context: &Context,
    (uia, request): (&Uia, &Request<Ancestor>),
    element: &Cached<Node>,
    (previous, parent_automation_id): (&[NodeSnapshot], &mut Option<String>),
    window: Option<isize>,
) -> Option<Result<Enrichment, NotFocused>> {
    if !context.tries_remote(window) {
        return None;
    }
    let clock = context.clock();
    let out_of_time = time_limit(&clock, ENRICHMENT_BUDGET);
    let (known_ids, known) = known_uia_nodes(context, previous);
    // The query reads the focus's control type and window; its ancestors
    // and selected child come back as ancestors, the child reported as
    // its node.
    let input = element.narrow::<FocusInput>();
    let fill = Fill::new(request);
    // The selected child is read alone, with its place (`verbatim_uia::Place`).
    let element_fill = Fill::new(&uia.request(gates::focus_gates(&context.fetches())).ok()?);
    let query = FocusQuery {
        element: &input,
        known: &known,
        previous: None,
        depth_limit: MAX_ANCESTOR_HOPS,
        fill: &fill,
        element_fill: &element_fill,
        out_of_time: Some(&out_of_time),
        require_focus: true,
        refresh: false,
        nearest_window: None,
        before_call: None,
        // The node's cache holds it, so the classic fallback asks nothing
        // of a combo box for it.
        value_available: Some(element.read().states.value_available),
        plain: Some(&|hwnd| context.window_unreadable(hwnd).is_some()),
    };
    // The program cannot judge a window between its steps, so a focus
    // with a ghost's or a hung application's window above its own is read
    // by the classic walk, which stops at the step into it.
    let remote = !window.is_some_and(|window| plain_window_above(context, window));
    // One remote operation, one transaction, which UIA's transaction
    // timeout bounds; the classic walk it falls back to checks the
    // budget between its calls.
    let (mut ancestry, path) = match focus_ancestry(uia, &query, remote) {
        Ok((FocusAncestry::Focused(ancestry), path)) => (ancestry, path),
        Ok((FocusAncestry::NotFocused, _)) => return Some(Err(NotFocused)),
        Err(error) => {
            // The walk failed, by UIA's transaction timeout or otherwise:
            // the focus is reported with its containers unknown, never as
            // having none.
            tracing::debug!(%error, "the focus ancestry could not be read");
            return Some(Ok((None, None)));
        }
    };
    if let Some(parent) = ancestry.ancestors.first() {
        *parent_automation_id = parent.read().automation_id;
    }
    if let Path::Fallback(error) = &path {
        tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
        if let (verbatim_uia_rops::Error::Import(_), Some(window)) = (error, window) {
            context.read_classically(window);
        }
    }
    // A window's own element read inside the provider can lack what the
    // client side's window proxy gives it, its handle and title among them
    // (`docs/uia-bugs.md`, "Inside a remote operation, conhost's window has
    // no handle or title"): such an ancestor of this application's is read
    // again through the proxy, one call, as the listener's classic walk
    // reads every ancestor.
    for ancestor in &mut ancestry.ancestors {
        let Some(hwnd) =
            window_read_without_its_handle(ancestor.read().node.role.hwnd, &ancestor.runtime_id())
                .filter(|&hwnd| serves(context, hwnd))
        else {
            continue;
        };
        match uia.window_element(hwnd, request) {
            Ok(read) => *ancestor = read,
            Err(error) => {
                tracing::debug!(hwnd, %error, "a window ancestor could not be read again through its proxy");
            }
        }
    }
    let registry = &context.uia_registry;
    let selected = ancestry
        .selected_child
        .as_ref()
        .map(|child| child.snapshot(registry));
    let read_by_msaa = |hwnd: isize| uia_walk_leaves(context, hwnd);
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
    // The classic walk stopped at a step into a window read as a plain
    // window: the chain goes on from it.
    let crossed = crossed.or(ancestry.plain_window);
    // The program stops at any known ancestor, while only a reported one
    // ends the chain: a known ancestor that is not reported (the previous
    // focus, a list item, say) is still where the previous chain takes
    // over.
    if let (AncestorWalk::Complete, None, Some(index)) = (walked, crossed, ancestry.met_known)
        && let Some(&met) = known_ids.get(index)
    {
        walked = AncestorWalk::MetKnown(met);
    }
    // The program, and the classic walk it falls back to, stop below the
    // process boundary, at the window the last ancestor, or the focus
    // itself, has; a walk that failed or was cut goes on from its
    // outermost element's nearest window.
    let focus_window = element.read().role.hwnd;
    let outermost = ancestry
        .ancestors
        .last()
        .map_or(focus_window, |ancestor| ancestor.read().node.role.hwnd);
    if walked == AncestorWalk::Complete
        && crossed.is_none()
        && (ancestry.failed_hop.is_some() || ancestry.depth_limited || ancestry.out_of_time)
    {
        let nearest = verbatim_uia::outermost_nearest_window(
            ancestry.window.or(window).unwrap_or(0),
            std::iter::once(focus_window).chain(
                ancestry
                    .ancestors
                    .iter()
                    .map(|ancestor| ancestor.read().node.role.hwnd),
            ),
        );
        if let Some(hop) = ancestry.failed_hop {
            walked = AncestorWalk::Failed(hop, verbatim_uia::parent_window(nearest));
        } else if ancestry.depth_limited || ancestry.out_of_time {
            walked = AncestorWalk::Cut(nearest);
        }
    }
    let above = uia_above(crossed, walked, Some(outermost));
    let ancestors = complete(
        context,
        Some((uia, request)),
        (chain, above),
        previous,
        Some(&out_of_time),
    );
    tracing::debug!(
        ?path,
        ancestors = ancestry.ancestors.len(),
        "UIA focus ancestry read"
    );
    Some(Ok((Some(ancestors), selected)))
}

/// Whether a window above `window`, as UIA's raw view goes up
/// ([`verbatim_uia::raw_view_parent_window`]), below the process boundary,
/// cannot be read: a ghost's or a hung application's
/// ([`Context::window_unreadable`]). Local calls, a few per window.
fn plain_window_above(context: &Context, window: isize) -> bool {
    let desktop = desktop_window();
    let mut current = window;
    for _ in 0..MAX_ANCESTOR_HOPS {
        let Some(above) = verbatim_uia::raw_view_parent_window(current) else {
            return false;
        };
        if above == desktop || !serves(context, above) {
            return false;
        }
        if context.window_unreadable(above).is_some() {
            return true;
        }
        current = above;
    }
    false
}

/// The window whose own element an ancestor is, by its runtime id, `[42,
/// handle]` as UI Automation gives a window's element, when the ancestor
/// was read without that handle (`hwnd`, 0): a read inside the provider
/// that lacked the client side's window proxy. `None` for any other
/// element, and for a window element read with its handle.
fn window_read_without_its_handle(hwnd: isize, runtime_id: &[i32]) -> Option<isize> {
    match runtime_id {
        &[42, handle] if hwnd == 0 && handle != 0 => isize::try_from(handle).ok(),
        _ => None,
    }
}

/// An MSAA node's ancestors, outermost first, without the window objects
/// above controls (see [`msaa_enrichment`]), stopping at a container of
/// `previous` and reusing the rest, and completed above as [`complete`]
/// says: where a parent lies in a window read through UIA, the walk goes
/// on from that window's UIA element, as NVDA switches API when a parent
/// is in such a window (`correctAPIForRelation`): in a Save As dialog, the
/// file name box's containers above the shell's UIA view are UIA's; where
/// a parent lies in a window read as a plain window
/// ([`reads_as_plain`]), or an `accParent` fails or names nothing, from
/// that window or the parent window ([`window_onward`]); and a walk cut by
/// `out_of_time` or the hop limit, by plain windows. Without `uia` the walk
/// stays in MSAA.
fn msaa_ancestors(
    context: &Context,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    node: &NodeSnapshot,
    previous: &[NodeSnapshot],
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Vec<NodeSnapshot> {
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    // Another process's window is a plain window, and is not probed.
    let read_by_uia = |hwnd: isize| uia.is_some() && own_window_uses_uia(context, hwnd);
    let plain = |hwnd: isize| reads_as_plain(context, hwnd);
    let limits = verbatim_ia2::acquire::AncestorLimits {
        max_hops: MAX_ANCESTOR_HOPS,
        known: &known,
        read_by_other_api: &read_by_uia,
        plain: &plain,
        out_of_time,
    };
    let (chain, walked) =
        verbatim_ia2::acquire::ancestor_chain_until(node.id, &context.msaa_registry, &limits)
            .unwrap_or((Vec::new(), verbatim_ia2::acquire::Walked::Complete));
    let chain: Vec<NodeSnapshot> = chain
        .into_iter()
        .filter(|ancestor| ancestor.role != Role::Window)
        .collect();
    let above = match walked {
        verbatim_ia2::acquire::Walked::Complete => Above::Nothing,
        verbatim_ia2::acquire::Walked::MetKnown(met) => Above::Met(met),
        verbatim_ia2::acquire::Walked::Cut(hwnd) => Above::Plain(hwnd),
        verbatim_ia2::acquire::Walked::Onward(hwnd) => Above::From(hwnd),
    };
    complete(context, uia, (chain, above), previous, out_of_time)
}

/// An MSAA node's ancestors and, for a selection container, its selected
/// child, stopping at a container of `previous` and reusing the rest, within
/// [`ENRICHMENT_BUDGET`] (checked between calls; an MSAA call has no bound
/// of its own, and a UIA call in the walk ends after UIA's call timeout),
/// and completed above as [`msaa_ancestors`] says. Failures degrade to no
/// containers or no selected child.
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
    let request = client.uia_with(context, Context::ancestor_request).ok();
    let ancestors = msaa_ancestors(
        context,
        request.as_ref().map(|(uia, request)| (*uia, request)),
        node,
        previous,
        Some(&out_of_time),
    );
    let selected = if wants_selected_child(node.role) {
        verbatim_ia2::acquire::selected_child(node.id, &context.msaa_registry)
    } else {
        None
    };
    (Some(ancestors), selected)
}

/// How long gathering a dialog's own text through UI Automation may take,
/// all its calls together: the text is worth a short wait, but the focus it
/// is announced with must not be held back long. Gathering reads the
/// dialog's descendants, many calls, so it has this deadline of its own,
/// enforced by abandonment ([`verbatim_uia::BoundedClient`]).
const DIALOG_TEXT_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether any of `focuses` had already entered `node`, a container of a
/// focus reported in the top-level window `top_level` (0 when not known),
/// by the rule the reducer speaks entered containers by
/// (`verbatim_core::focus_rules::already_entered`).
fn entered_before(focuses: &[super::held::Focus], top_level: isize, node: &NodeSnapshot) -> bool {
    focuses.iter().any(|focus| {
        already_entered(
            focus.ancestors.iter().chain(std::iter::once(&focus.node)),
            top_level != 0 && focus.top_level == top_level,
            node,
        )
    })
}

/// The dialog this outpost last gathered text for in a foreground report,
/// and the text: the focus that follows it into the dialog reuses it rather
/// than reading the dialog again.
pub(super) type DialogMemo = Option<(NodeId, Option<String>)>;

/// Gives every dialog among `nodes`, the chain of a focus reported in
/// `window`, that the focus Core holds is not already inside and that has
/// no description of its own, its own text as its description
/// (`docs/nvda/object-model.md`, "A dialog's own text"), while descriptions
/// are read at all: a dialog the focus newly enters, by the rule the
/// reducer speaks entered containers by
/// (`verbatim_core::focus_rules::already_entered`; decision D14, "Where the
/// rules run"). A dialog the focus stays inside is not read again: the
/// chain the focus is reported with keeps the text it was given.
///
/// `memo` is taken: a dialog it names is given its text without reading it
/// again. With `remember`, the last text read is kept in it for the next
/// report, which is how a foreground report hands its dialog to the focus
/// inside it.
pub(super) fn describe_dialogs<'a>(
    context: &Context,
    (nodes, window): (
        impl IntoIterator<Item = &'a mut NodeSnapshot>,
        Option<isize>,
    ),
    (memo, remember): (&mut DialogMemo, bool),
) {
    let mut remembered = memo.take();
    if !context.fetches().description {
        return;
    }
    // The focuses the focus enters a dialog from: the focus Core holds,
    // when it is this outpost's, and those this outpost reported that Core
    // has not judged yet, one of which Core's focus may yet be. A dialog
    // inside any of them is not read again.
    let focuses: Vec<super::held::Focus> = context.held().focuses().cloned().collect();
    let top_level = window.map_or(0, super::window::top_level_of);
    for node in nodes {
        if !is_dialog(node.role)
            || node
                .details
                .description
                .as_deref()
                .is_some_and(|description| !description.trim().is_empty())
            || entered_before(&focuses, top_level, node)
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
                        let found = super::focus_path::find(uia, &route, &nodes, remote, &clock)
                            .0
                            .ok()??;
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
/// (`docs/design/listener-without-connections.md`, section 4.3); and when
/// the focus window is no longer a window. A focus window stood in for by
/// a ghost window, or hung (`window::event_window_unreadable`, local calls), is
/// answered as a plain window, with the plain windows above it, and nothing
/// is asked of the application that does not answer: a deliberate
/// difference, since NVDA says nothing then (Dickson, 2026-10-29). Returned
/// with the window whose focused element it read.
///
/// Through MSAA, the focused object is the focus window's client's
/// `accFocus`, whose answer is checked as an object of a window this
/// application serves before it is read
/// ([`verbatim_ia2::acquire::focused_snapshot`]): another process's
/// answers no focused control.
///
/// Through UIA, the focused element is read with
/// `GetFocusedElementBuildCache`, which reads the keyboard focus again
/// itself and connects to whichever window has it then, so an element of
/// the application the focus has moved to can come back
/// (`experiments/getfocusedelement-after-the-focus-moves/`). Its cached
/// process is compared with the one the check above proved serves the
/// focus window, this outpost's application, before anything else is done
/// with it: on a mismatch, or with no process cached, nothing more is read
/// and `None` is answered, so a stray element is never reported, followed,
/// or sent a remote operation. `trigger`, what asked for the read, is
/// logged with it. Answering nothing, it says whether these checks refused
/// the read or the read failed ([`FocusReadMiss`]).
pub(super) fn focused_control_in(
    context: &Context,
    client: &mut Client,
    trigger: FocusRead,
) -> Result<(isize, FocusedControl), FocusReadMiss> {
    if let Some(hung) = hung_focus_window(context) {
        tracing::debug!(
            ?trigger,
            hwnd = hung,
            "the focus window cannot be read; it is answered as a plain window"
        );
        return Ok((
            hung,
            FocusedControl {
                node: plain_window(context, hung),
                ancestors: Some(plain_windows_from(context, parent_window(hung))),
                selected_child: None,
                window: Some(window_facts(hung)),
                text: None,
            },
        ));
    }
    let hwnd = checked_focus_window(context)?;
    // The focused element is read from the window's provider at once.
    if window_uses_uia(context, hwnd) {
        uia_focused_control(context, client, hwnd, (trigger, false))
    } else {
        let node =
            verbatim_ia2::acquire::focused_snapshot(hwnd, &context.msaa_registry, &|window| {
                serves(context, window)
            })
            .ok_or(FocusReadMiss::Failed)?;
        let (ancestors, selected_child) = msaa_enrichment(context, client, &node, &[]);
        Ok((
            hwnd,
            FocusedControl {
                node,
                ancestors,
                selected_child,
                window: Some(window_facts(hwnd)),
                text: None,
            },
        ))
    }
}

/// The gap's read (`docs/design/focus-pipeline-rework.md`, "Who owns each
/// duplicated fact", the gap's evidence): [`focused_control_in`] through UIA
/// only, refused with local calls when the focus window is not one this
/// outpost judges UIA, since a focus in an MSAA window is that window's
/// copy's to speak, and refused after `GetFocusedElementBuildCache` when the
/// foreground thread's focus window, taken again with `GetGUIThreadInfo`, a
/// local call, has changed to a window it does not judge UIA without a
/// probe: a focus moved into an MSAA window between the check and the read
/// comes back as UIA's MSAA proxy for it, which that window's copy speaks
/// too, and a proxied list item has no window handle of its own to judge.
pub(super) fn gap_focus_read(
    context: &Context,
    client: &mut Client,
) -> Result<(isize, FocusedControl), FocusReadMiss> {
    let hwnd = checked_focus_window(context)?;
    if !window_uses_uia(context, hwnd) {
        tracing::debug!(
            hwnd,
            "the gap's read is refused: the outpost does not judge the focus window UIA"
        );
        return Err(FocusReadMiss::Refused);
    }
    uia_focused_control(context, client, hwnd, (FocusRead::Gap, true))
}

/// [`focused_control_in`]'s read through UIA, of the focus window `hwnd`,
/// for `trigger`; with `recheck`, the read is refused once the focused
/// element is read if the focus window has changed since to one this
/// outpost does not judge UIA ([`gap_focus_read`]).
fn uia_focused_control(
    context: &Context,
    client: &mut Client,
    hwnd: isize,
    (trigger, recheck): (FocusRead, bool),
) -> Result<(isize, FocusedControl), FocusReadMiss> {
    {
        let (uia, nodes) = client
            .uia_with(context, Context::node_request)
            .map_err(|_| FocusReadMiss::Failed)?;
        let ancestor_request = context
            .ancestor_request(uia)
            .map_err(|_| FocusReadMiss::Failed)?;
        let element = checked_focused_element(context, (uia, &nodes), hwnd, trigger)?;
        if recheck {
            let now = context.focus_window();
            if now != Some(hwnd)
                && !now.is_some_and(|now| {
                    serves(context, now)
                        && crate::arbitration::known_to_use_uia(&context.arbitrator, now)
                })
            {
                tracing::debug!(
                    ?trigger,
                    hwnd,
                    ?now,
                    "the focus window changed during the read to one the outpost does not judge UIA; nothing more is read"
                );
                return Err(FocusReadMiss::Refused);
            }
        }
        let mut node = with_legacy_checked_state(
            element.live().element(),
            element.snapshot(&context.uia_registry),
        );
        // The element's own UIA class, from its cache, for Core's rules for
        // an application's elements (decision D6): no call.
        let fields = element.read();
        node.details.class_name.clone_from(&fields.role.class_name);
        console_text_area(&mut node, Some(hwnd));
        // A focus that has moved on since the focused element was read is
        // still this query's answer, read the classic way. Its parent's
        // automation id, for the same rules, comes with its ancestors.
        let mut parent_automation_id = None;
        let (ancestors, selected_child) = match uia_remote_enrichment(
            context,
            (uia, &ancestor_request),
            &element,
            (&[], &mut parent_automation_id),
            Some(hwnd),
        ) {
            Some(Ok(enrichment)) => enrichment,
            Some(Err(NotFocused)) | None => uia_enrichment(
                context,
                (uia, &nodes, &ancestor_request),
                (&element, hwnd),
                (node.role, &mut parent_automation_id),
                &[],
            ),
        };
        node.details.parent_automation_id = parent_automation_id;
        // A combo box without the `Value` pattern, or a looping selector:
        // its first selected item is its value.
        let mut selected_child = selected_child;
        if fields.value_from_selection() {
            node.value = selected_child.take().and_then(|child| child.name);
        }
        Ok((
            hwnd,
            FocusedControl {
                node,
                ancestors,
                selected_child,
                window: Some(window_facts(hwnd)),
                text: None,
            },
        ))
    }
}

/// The UIA element of this application's focus, as [`focused_control_in`]
/// reads it, with its checks and its process-id check, filled as a node and
/// nothing more read: for the windowless text focus whose element the
/// focus's route cannot find (`docs/design/focus-pipeline-rework.md`, the
/// audit's difference 5). `Refused` also when the outpost judges the focus
/// window MSAA.
pub(super) fn focused_element_in(
    context: &Context,
    client: &mut Client,
    trigger: FocusRead,
) -> Result<Cached<Node>, FocusReadMiss> {
    let hwnd = checked_focus_window(context)?;
    if !window_uses_uia(context, hwnd) {
        return Err(FocusReadMiss::Refused);
    }
    let (uia, nodes) = client
        .uia_with(context, Context::node_request)
        .map_err(|_| FocusReadMiss::Failed)?;
    checked_focused_element(context, (uia, &nodes), hwnd, trigger)
}

/// The window with the keyboard focus when it is this application's, not
/// on Core's hidden frame, and a window that exists but cannot be read, a
/// ghost's or a hung application's: [`focused_control_in`] answers it as a
/// plain window. Local calls.
fn hung_focus_window(context: &Context) -> Option<isize> {
    control_window(context.target_pid(), || context.focus_window()).filter(|&hwnd| {
        !window_belongs_to_hidden_frame(hwnd)
            && window_exists(hwnd)
            && context.window_unreadable(hwnd).is_some()
    })
}

/// The window with the keyboard focus whose focused element
/// [`focused_control_in`] may read: this application's, not on Core's
/// hidden frame, and readable. Local calls.
fn checked_focus_window(context: &Context) -> Result<isize, FocusReadMiss> {
    let hwnd = control_window(context.target_pid(), || context.focus_window())
        .ok_or(FocusReadMiss::Refused)?;
    if window_belongs_to_hidden_frame(hwnd) {
        return Err(FocusReadMiss::Refused);
    }
    if let Some(reason) = context.window_unreadable(hwnd) {
        tracing::debug!(
            hwnd,
            reason,
            "the focus window cannot be read; its focus is not read"
        );
        return Err(FocusReadMiss::Refused);
    }
    Ok(hwnd)
}

/// The focused element, read with `GetFocusedElementBuildCache` filled by
/// `nodes` and the element's process, and taken only when that process is
/// the one serving `hwnd`, this outpost's application.
fn checked_focused_element(
    context: &Context,
    (uia, nodes): (&Uia, &Request<Node>),
    hwnd: isize,
    trigger: FocusRead,
) -> Result<Cached<Node>, FocusReadMiss> {
    let focused_request = nodes.with_process().map_err(|_| FocusReadMiss::Failed)?;
    tracing::debug!(?trigger, hwnd, "the application's focused element is read");
    let element =
        (context.focused_element)(uia, &focused_request).map_err(|_| FocusReadMiss::Failed)?;
    let serving = context.target_pid();
    let found = element.cached_process();
    if found != Some(serving) {
        tracing::debug!(
            ?trigger,
            hwnd,
            serving,
            ?found,
            "the focused element read belongs to another process than the one serving the focus window; nothing more is read"
        );
        return Err(FocusReadMiss::Refused);
    }
    Ok(element)
}

/// Why [`focused_control_in`] answered no focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FocusReadMiss {
    /// Its own checks refused the read: the focus window is not this
    /// application's, or is on Core's hidden frame, or cannot be read, or
    /// the focused element read belongs to another process.
    Refused,
    /// The read itself failed.
    Failed,
}

/// What asked [`focused_control_in`] to read the application's focus,
/// logged with each read, so a run's log shows how often each makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FocusRead {
    /// A focus-now query ([`focus_now`]).
    FocusNow,
    /// The gap's read: no UIA focus answered a `WinEvent` focus this
    /// outpost dropped because it judged its window UIA, by the gap
    /// record's bound, or a UIA focus whose sender had no window above it
    /// came beside it ([`gap_focus_read`]).
    Gap,
    /// A windowless text focus whose walk up the listener cut, so the
    /// chain it sent holds only the focus and no route leads to its
    /// element ([`focused_element_in`]).
    CutWalk,
}

/// The window whose focused element [`focused_control_in`] reads for the
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
/// it holds the system foreground, and its focused control. A console
/// host's is its console window ([`foreground_window_of`]). A foreground
/// window that cannot be read, a ghost's
/// or a hung application's, is a plain window, as its focus is
/// ([`focused_control_in`]). Returned with the window whose focused element
/// it read, the control's own window.
pub(super) fn focus_now(context: &Context, client: &mut Client) -> (FocusNow, Option<isize>) {
    let observed_at_us = now_us();
    // A foreground window that cannot be read is a plain window, with no
    // call, as the focus in it is ([`focused_control_in`]).
    let window = foreground_window_of(context.target_pid())
        .filter(|&hwnd| !window_is_hidden_frame(hwnd))
        .map(|hwnd| {
            let node = if window_exists(hwnd) && context.window_unreadable(hwnd).is_some() {
                plain_window(context, hwnd)
            } else {
                foreground_window(context, client, hwnd).1
            };
            (node, window_facts(hwnd))
        });
    let (control_window, focus) = focused_control_in(context, client, FocusRead::FocusNow)
        .ok()
        .unzip();
    (
        FocusNow {
            window,
            focus,
            observed_at_us,
        },
        control_window,
    )
}

/// Resolves a UIA node to a live element filled by `nodes`: the registry's
/// kept element first (refreshing its cache both updates it and proves it
/// still answers), then, if that element is gone or none is kept, the route
/// to it through the chain the focus listener read for the focus
/// ([`super::focus_path`]). A node found by neither is gone, as NVDA's object
/// for a gone element is: no search is made for it, since a search of the
/// application's windows enters any child window another process serves
/// (`docs/design/cross-process-calls-study.md`, instance 3). A kept element
/// that fails for another reason, such as a busy provider timing out, is
/// kept and the read fails: it may well still be alive. So does a route's
/// walk that fails rather than finishing without the element, one past its
/// time budget above all ([`super::focus_path::find`]).
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
    // found by its path.
    let route = context.focus_route(&runtime_id).ok_or(ReadError::Gone)?;
    let remote = context.tries_remote(Some(route.window));
    let (found, import_failed) =
        super::focus_path::find(uia, &route, nodes, remote, &context.clock());
    if import_failed {
        context.read_classically(route.window);
    }
    // A walk that failed, against a slow application above all, may have
    // missed a live element; only one that finished without it says gone.
    let element = found
        .map_err(|reason| ReadError::Failed(format!("walking the focus's route: {reason}")))?
        .ok_or(ReadError::Gone)?;
    let _ = context
        .uia_registry
        .id_for_element(&runtime_id, &element.narrow());
    Ok(element)
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
        let own_window = element.read().role.hwnd;
        return Ok(uia_ancestors(
            context,
            (uia, &request),
            (element.live(), own_window, own_window),
            (&[], &mut None),
            None,
        ));
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
/// gone, which the classic way answers from the focus's route or as gone
/// ([`resolve_uia_element`]).
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
            // Found by the focus's route, or gone, the classic way.
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

/// The application's tree from its top-level window, each element's
/// location and whether it is off the screen read only with `locations`:
/// a read of many elements leaves both out, since for some elements, a
/// top-level window's title bar and its buttons among them, UIA asks the
/// application's UI thread for them (`verbatim_uia::Place`), and the dump
/// says it did not read them ([`DumpedTree::locations`]). Over UIA each
/// element's children are read in one call, by remote programs where the
/// window allows them ([`verbatim_uia_rops::tree_dump`]).
pub(super) fn dump_tree(
    context: &Context,
    client: &mut Client,
    locations: bool,
) -> Result<DumpedTree, ReadError> {
    let hwnd = main_window_of(context.target_pid()).ok_or_else(|| {
        ReadError::Failed("the target application has no top-level window".to_owned())
    })?;
    if window_uses_uia(context, hwnd) {
        let uia = client
            .uia()
            .ok_or_else(|| ReadError::Failed("could not create a UIA client".to_owned()))?;
        let gates = NodeGates {
            place: locations.then_some(()),
            ..gates::many_node_gates(&context.fetches())
        };
        let nodes = uia
            .request::<Node>(gates)
            .map_err(|error| ReadError::Failed(format!("UIA cache request failed: {error}")))?;
        let element = uia.window_element(hwnd, &nodes).map_err(|error| {
            ReadError::Failed(format!(
                "could not fetch the top-level UIA element: {error}"
            ))
        })?;
        let fill = Fill::new(&nodes);
        let query = verbatim_uia_rops::TreeQuery {
            root: &element,
            fill: &fill,
            registry: &context.uia_registry,
            max_depth: MAX_DUMP_DEPTH,
            max_nodes: MAX_DUMP_NODES,
        };
        let remote = context.tries_remote(Some(hwnd));
        let ((root, truncated), path) = verbatim_uia_rops::tree_dump(uia, &query, remote)
            .map_err(|error| ReadError::Failed(format!("UIA tree walk failed: {error}")))?;
        if let Path::Fallback(error) = &path {
            tracing::warn!(hwnd, %error, "a remote operation failed; read the classic way");
            if let verbatim_uia_rops::Error::Import(_) = error {
                context.read_classically(hwnd);
            }
        }
        Ok(DumpedTree {
            root,
            truncated,
            locations,
        })
    } else {
        let (root, truncated) = verbatim_ia2::acquire::walk_tree(
            hwnd,
            &context.msaa_registry,
            (MAX_DUMP_DEPTH, MAX_DUMP_NODES),
            locations,
        )?;
        Ok(DumpedTree {
            root,
            truncated,
            locations,
        })
    }
}

#[cfg(test)]
mod tests {
    use verbatim_core::focus_rules::{HeldFocus, HeldNode};
    use verbatim_model::{
        Backend, NodeDetails, NodeId, NodeSnapshot, Role, StateSet, TraceId, WindowHandle,
    };

    use super::super::held::Held;
    use super::super::window::window_owner;
    use super::{
        CONSOLE_WINDOW_CLASS, console_text_area_in, control_window, entered_before, splice,
        window_read_without_its_handle,
    };

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

    /// Only a window's own element read without its handle, as the console
    /// host's window is read inside its provider, is read again through
    /// its proxy: its handle comes from its runtime id.
    #[test]
    fn a_window_element_read_without_its_handle_is_named_by_its_runtime_id() {
        assert_eq!(
            window_read_without_its_handle(0, &[42, 76_482_230]),
            Some(76_482_230)
        );
        assert_eq!(
            window_read_without_its_handle(76_482_230, &[42, 76_482_230]),
            None
        );
        assert_eq!(
            window_read_without_its_handle(0, &[42, 76_482_230, 4, -1]),
            None
        );
        assert_eq!(window_read_without_its_handle(0, &[7, 76_482_230]), None);
    }

    /// The focus Core holds is a button in a child dialog "Remove", window
    /// 20, of the top-level window 7. A second dialog "Remove", window 21,
    /// in the same top-level window, is newly entered, as Core judges it,
    /// so its text is gathered; a node of window 20 is the dialog the focus
    /// is already in.
    #[test]
    fn a_dialog_of_another_window_with_the_same_role_and_name_is_newly_entered() {
        let held_node = |id, role, name: Option<&str>, window: Option<u64>| HeldNode {
            id: NodeId::new(id),
            backend: Backend::Msaa,
            role,
            name: name.map(str::to_owned),
            states: StateSet::new(),
            window: window.map(WindowHandle),
        };
        let mut held = Held::default();
        held.core_focus(
            None,
            Some(HeldFocus {
                trace_id: TraceId::mint(),
                observed_at_us: 1,
                focus: held_node(2, Role::Button, Some("OK"), None),
                ancestors: vec![held_node(1, Role::Dialog, Some("Remove"), Some(20))],
                top_level: Some(WindowHandle(7)),
                alive: true,
            }),
        );
        let focuses: Vec<_> = held.focuses().cloned().collect();
        let dialog = |window| NodeSnapshot {
            id: NodeId::new(3),
            backend: Backend::Msaa,
            role: Role::Dialog,
            name: Some("Remove".to_owned()),
            value: None,
            states: StateSet::new(),
            details: NodeDetails {
                window: Some(WindowHandle(window)),
                ..NodeDetails::default()
            },
        };
        assert!(!entered_before(&focuses, 7, &dialog(21)));
        assert!(entered_before(&focuses, 7, &dialog(20)));
    }
}
