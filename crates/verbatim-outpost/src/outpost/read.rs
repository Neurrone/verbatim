//! The worker's reads from the application: windows, the focus, a node's
//! ancestors, navigation, activation, and tree dumps, through whichever
//! backend owns the window. Every function here may call into the
//! application and so runs only on the worker, under its deadline.

#![forbid(unsafe_code)]

use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::CHILDID_SELF;
use verbatim_ia2::equality::Candidates;
use verbatim_model::{
    ActionName, Backend, NodeDetails, NodeId, NodeSnapshot, QueryKind, Role, State, StateSet,
};
use verbatim_uia::Interning as _;
use verbatim_uia::map::with_legacy_checked_state;
use verbatim_uia::{
    Ancestor, AncestorWalk, Cached, DialogText, Fill, FocusInput, Live, Node, NodeGates, Request,
    Shape, Uia,
};
use verbatim_uia_rops::{
    FocusAncestry, FocusQuery, NavigationDirection, Path, StepQuery, focus_ancestry,
    navigation_step,
};

use verbatim_core::focus_rules::{HeldNode, already_entered};

use crate::dialog_text::is_dialog;

use crate::arbitration::{time_limit, window_class_name};
use crate::gates;
use crate::protocol::now_us;
use verbatim_model::protocol::{DumpedTree, FocusNow, FocusedControl};

use super::text_reads::CONSOLE_WINDOW_CLASS;
use super::window::{
    desktop_window, foreground_window_of, main_window_of, parent_window, served_by, top_level_of,
    window_belongs_to_hidden_frame, window_exists, window_facts, window_is_hidden_frame,
    window_style, window_text,
};
use super::{Context, RequestContext};

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
pub(super) const ENRICHMENT_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// What a focus's walk has read so far, told as it reads it, so the
/// worker can keep it in its charge, from which the reducer publishes the
/// focus should a call of the walk still be under way at the walk's bound
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback"; [`super::bound`]).
pub(super) enum WalkStep<'a> {
    /// The focus, read once every check NVDA makes before it queues a
    /// focus is passed, to be reported in `window`: its walk begins, the
    /// chain to be completed by plain windows from `from`, included, and
    /// `left` of its bound is left.
    Focus {
        node: &'a NodeSnapshot,
        window: isize,
        from: isize,
        left: std::time::Duration,
    },
    /// An ancestor read, nearest first, `None` for one not reported (an
    /// MSAA window object), and the window from which plain windows,
    /// included, now complete the chain.
    Ancestor(Option<&'a NodeSnapshot>, isize),
    /// The walk's chain as it stands, outermost first: going on from
    /// this window, by plain windows from it, included, should the bound
    /// be met before it goes on; `None` once the walk has ended and the
    /// chain is whole.
    Chain(&'a [NodeSnapshot], Option<isize>),
}

/// What a focus's walk tells what it reads ([`WalkStep`]), which answers
/// whether the walk goes on: an abandoned worker's walk stops at its next
/// step, as one out of time does.
pub(super) type WalkTold<'a> = &'a dyn Fn(WalkStep<'_>) -> bool;

/// Where a focus's walk tells what it reads; `None` for a walk the reducer
/// never publishes.
pub(super) type WalkProgress<'a> = Option<WalkTold<'a>>;

/// How much of a focus walk's bound is left, the walk's focus in window
/// `window` observed at `observed_at_us` on the run's shared clock: the
/// bound of the design's Terms, [`MOVED_ON_GRACE`](super::window::MOVED_ON_GRACE)
/// when the focus's top-level window is not that of the focus this outpost
/// expects Core to hold, the one before it (`held::Held::expected`), and
/// [`ENRICHMENT_BUDGET`] otherwise, less the time since the observation.
/// Local calls.
pub(super) fn walk_left(
    context: &RequestContext<'_>,
    window: isize,
    observed_at_us: u64,
) -> std::time::Duration {
    let top_level = top_level_of(window);
    let previous = context.held().expected().map(|focus| focus.top_level);
    let bound = if top_level != 0 && previous == Some(top_level) {
        ENRICHMENT_BUDGET
    } else {
        super::window::MOVED_ON_GRACE
    };
    let waited = context.observed_now().saturating_sub(observed_at_us);
    bound.saturating_sub(std::time::Duration::from_micros(waited))
}

/// The window from which plain windows, included, complete a UIA chain read
/// upward one element at a time, as [`verbatim_uia::plain_windows_start`]
/// finds it for a whole chain.
struct Outermost {
    window: std::cell::Cell<isize>,
    own: std::cell::Cell<bool>,
}

impl Outermost {
    /// From `nearest`, the first element's nearest window.
    fn new(nearest: isize) -> Self {
        Self {
            window: std::cell::Cell::new(nearest),
            own: std::cell::Cell::new(false),
        }
    }

    /// The next element up, of window `hwnd` (0 for none of its own): the
    /// window plain windows now complete the chain from, the parent window
    /// of the element's window when the element is that window's own, so
    /// that the window is not in the chain twice. Local calls.
    fn step(&self, hwnd: isize) -> isize {
        if hwnd != 0 {
            self.window.set(hwnd);
            self.own.set(true);
        } else if self.own.get() {
            self.window.set(parent_window(self.window.get()));
            self.own.set(false);
        }
        if self.own.get() {
            parent_window(self.window.get())
        } else {
            self.window.get()
        }
    }
}

/// A focus's ancestors, outermost first, complete to the desktop; and its
/// selected child, for a selection container.
pub(super) type Enrichment = (Vec<NodeSnapshot>, Option<NodeSnapshot>);

/// `chain`, outermost first, whose outermost entry is `met`, extended
/// outward with the part of `previous` above `met`: NVDA reuses the
/// previous focus's ancestors from where the new focus's ancestry meets
/// them, rather than reading them again. Each entry spliced is recorded as
/// carried by the message the chain goes in, which names it, as a node
/// looked up is (`super::store::Recorded`); `previous` holds only entries
/// the store still holds, tagged with the walk's request
/// ([`held_prefix`]).
pub(super) fn splice(
    previous: &[NodeSnapshot],
    chain: Vec<NodeSnapshot>,
    met: NodeId,
) -> Vec<NodeSnapshot> {
    let (chain, _) = splice_onto(previous, chain, met);
    chain
}

/// [`splice`]'s chain, and the entries of `previous` it spliced on.
fn splice_onto(
    previous: &[NodeSnapshot],
    chain: Vec<NodeSnapshot>,
    met: NodeId,
) -> (Vec<NodeSnapshot>, &[NodeSnapshot]) {
    let Some(index) = previous.iter().position(|node| node.id == met) else {
        return (chain, &[]);
    };
    let above = &previous[..index];
    (above.iter().cloned().chain(chain).collect(), above)
}

/// The part of `previous`, a focus chain outermost first, that a walk may
/// stop at and splice from: its entries above the outermost one the store
/// no longer holds, so a walk that meets the chain below a released entry
/// reads on rather than splicing a node no message may name any more. The
/// entries below that one are dropped even where the store still holds
/// them, so the walk reads them again rather than splicing. The entries
/// kept are the walk's copied line, tagged with its request as they are
/// judged held, so no release frees one while the walk may name it
/// ([`super::store::Store::held_prefix`]).
pub(super) fn held_prefix(
    context: &RequestContext<'_>,
    mut previous: Vec<NodeSnapshot>,
) -> Vec<NodeSnapshot> {
    let ids: Vec<NodeId> = previous.iter().map(|node| node.id).collect();
    previous.truncate(context.store.held_prefix(context.tag(), &ids));
    previous
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
    context: &RequestContext<'_>,
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
    context: &RequestContext<'_>,
    client: &mut Client,
    hwnd: isize,
) -> Option<(Backend, NodeSnapshot)> {
    if window_class_name(hwnd) == "#32768" {
        let node = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            OBJID_CLIENT.0,
            CHILDID_SELF,
            context.nodes(),
            verbatim_ia2::acquire::Purpose::Announce,
        )?;
        return Some((Backend::Msaa, node));
    }
    // The window's element is the connection to it.
    if window_uses_uia(context, hwnd) {
        let (uia, nodes) = client.uia_with(context, Context::node_request).ok()?;
        let element = uia.window_element(hwnd, &nodes).ok()?;
        let node = element.snapshot(context.nodes());
        Some((Backend::Uia, node))
    } else {
        // The client area, as NVDA reads a foreground window: a focus event
        // on the client area that follows is then the same node and is not
        // announced again.
        let node = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            OBJID_CLIENT.0,
            CHILDID_SELF,
            context.nodes(),
            verbatim_ia2::acquire::Purpose::Announce,
        )?;
        Some((Backend::Msaa, node))
    }
}

/// A window snapshot from local window data only: role window, named by its
/// window text, under the registry key the MSAA client area uses, which a
/// foreground report reads when the window can be read.
fn local_window_snapshot(context: &RequestContext<'_>, hwnd: isize) -> NodeSnapshot {
    NodeSnapshot {
        id: context.nodes().client(hwnd),
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
/// the focus itself) and reuses the rest; within `left`, what is left of
/// its bound, checked between its calls, and completed above as
/// [`complete`] says.
/// A walk that fails is completed by plain windows
/// ([`plain_windows_above`]), and a failed selected child read is none.
/// Enrichment never turns a focus into an error. The automation id of the element's
/// raw-view parent, when the walk's first step read it, is put in
/// `parent_automation_id`.
pub(super) fn uia_enrichment(
    context: &RequestContext<'_>,
    (uia, nodes, ancestors): (&Uia, &Request<Node>, &Request<Ancestor>),
    (element, nearest): (&Cached<Node>, isize),
    (role, parent_automation_id): (Role, &mut Option<String>),
    (previous, left): (&[NodeSnapshot], std::time::Duration),
) -> Enrichment {
    let clock = context.clock();
    let out_of_time = time_limit(&clock, left);
    let ancestors = uia_ancestors(
        context,
        (uia, ancestors),
        (element.live(), element.read().role.hwnd, nearest),
        (previous, parent_automation_id),
        Some(&out_of_time),
    );
    let selected = if wants_selected_child(role) || element.read().value_from_selection() {
        match uia.selected_node(element.live(), nodes, context.nodes()) {
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

/// A UIA element's ancestors, outermost first, its own window handle
/// `element_window` and its nearest window `nearest`, 0 when not known.
/// Where the walk reaches the root of a window read through MSAA or as a
/// plain window ([`uia_walk_leaves`]), or stops below the process
/// boundary, it goes on from that window
/// ([`window_onward`]), as NVDA switches API when its walk crosses into
/// such a window: in File Explorer, the folder window above the UIA file
/// list is then the client area a foreground report already named, not a
/// second, UIA copy of it. A walk cut by its time or its hop limit, or
/// whose step to a parent failed, is completed as [`complete`] says, and
/// one that fails before it reads anything, by plain windows
/// ([`plain_windows_above`]). The desktop window ends the walk. The
/// automation id of the element's
/// raw-view parent, when the walk's first step read it, is put in
/// `parent_automation_id`.
fn uia_ancestors(
    context: &RequestContext<'_>,
    (uia, request): (&Uia, &Request<Ancestor>),
    (element, element_window, nearest): (&Live, isize, isize),
    (previous, parent_automation_id): (&[NodeSnapshot], &mut Option<String>),
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Vec<NodeSnapshot> {
    let read_by_msaa = |hwnd: isize| uia_walk_leaves(context, nearest, hwnd);
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    let Ok((chain, crossed, walked)) = uia.ancestors(
        (element, element_window, nearest),
        request,
        context.nodes(),
        (MAX_ANCESTOR_HOPS, parent_automation_id),
        &verbatim_uia::AncestorStops {
            read_by_other_api: &read_by_msaa,
            known: &known,
            out_of_time,
        },
    ) else {
        return plain_windows_above(context, element_window, nearest);
    };
    // The walk stops below the process boundary itself and answers the
    // parent or owner window there as `crossed`.
    let above = uia_above(crossed, walked, None);
    complete(
        context,
        Some((uia, request)),
        (chain, above),
        (previous, None),
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
/// boundary check and its step. The desktop ends a walk instead. The
/// focus's nearest window `nearest`, 0 when not known, is never taken for
/// a ghost's or a hung one: NVDA switches API only where a parent's window
/// differs from its child's, so the element of the window the focus is in
/// stays a UIA ancestor, as the listener's walk keeps it; another
/// process's window, and one read through MSAA, still end the walk there.
/// Local calls, and the verdict's probe for a window of this application.
fn uia_walk_leaves(context: &Context, nearest: isize, hwnd: isize) -> bool {
    let plain = if hwnd == nearest {
        !serves(context, hwnd)
    } else {
        reads_as_plain(context, hwnd)
    };
    hwnd != desktop_window() && (plain || !window_uses_uia(context, hwnd))
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
    context: &RequestContext<'_>,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    (chain, above): (Vec<NodeSnapshot>, Above),
    (previous, candidates): (&[NodeSnapshot], Option<&Candidates>),
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Vec<NodeSnapshot> {
    match above {
        Above::Nothing => chain,
        Above::Met(met) => splice(previous, chain, met),
        Above::From(hwnd) => window_onward(context, uia, hwnd, (previous, candidates), out_of_time)
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
/// named by `InternalGetWindowText`, unavailable when its style disables
/// it, read from Windows' own state with no call into any application. It
/// is never focused, as NVDA's `Window` states hold only invisible and
/// unavailable (`NVDAObjects/window/__init__.py`, `Window._get_states`),
/// and never invisible either (Fable, 2026-10-30, pending Dickson's
/// acceptance): a window not yet shown when it is read, as a Windows
/// Terminal window at its bound, would lose its title for good, where
/// NVDA reads it through UIA and says it. It is kept at its window
/// object's address, so a node read from it acquires the window's own
/// object.
pub(super) fn plain_window(context: &RequestContext<'_>, hwnd: isize) -> NodeSnapshot {
    window_from_local_state(context, hwnd, false)
}

/// Window `hwnd`, neither a ghost's nor a hung application's, read with no
/// call into its process, as NVDA's MSAA client object for it
/// (`docs/design/focus-pipeline-rework.md`, "The foreground wait"): a
/// pane, marked `top_level_client` when its style has a system menu, as
/// `verbatim_ia2`'s reader marks an MSAA client, so a top-level window is
/// spoken by its title and a child window is structure, and in all else a
/// [`plain_window`], read from Windows' own state.
fn client_pane(context: &RequestContext<'_>, hwnd: isize) -> NodeSnapshot {
    window_from_local_state(context, hwnd, true)
}

/// Window `hwnd` read from Windows' own state alone, one style read and its
/// text: a [`plain_window`], or, as `client`, the pane of
/// [`client_pane`].
fn window_from_local_state(
    context: &RequestContext<'_>,
    hwnd: isize,
    client: bool,
) -> NodeSnapshot {
    let style = window_style(hwnd);
    let mut states = StateSet::new();
    if style.disabled {
        states = states.with(State::Disabled);
    }
    NodeSnapshot {
        id: context.nodes().window(hwnd),
        backend: Backend::Msaa,
        role: if client { Role::Pane } else { Role::Window },
        name: window_text(hwnd),
        value: None,
        states,
        details: NodeDetails {
            window: Some(super::window::window_handle(hwnd)),
            top_level_client: client && style.system_menu,
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

/// The chain of a focus whose walk failed before it read any ancestor,
/// completed to the desktop by the windows above it, read with local calls
/// only ([`plain_windows_from`]): from the parent window of `own`, the
/// window the focus is when it is one, else from `nearest`, the window it
/// is in, included (`docs/design/focus-pipeline-rework.md`, "A window's
/// name, and the one fallback"). Empty, and logged, when neither is known,
/// the one chain that is not complete.
fn plain_windows_above(
    context: &RequestContext<'_>,
    own: isize,
    nearest: isize,
) -> Vec<NodeSnapshot> {
    let from = if own == 0 {
        nearest
    } else {
        parent_window(own)
    };
    if own == 0 && nearest == 0 {
        tracing::warn!("a focus's walk failed with no window known; it is reported with no chain");
    }
    plain_windows_from(context, from)
}

/// The windows from `hwnd` up to the desktop, which is none of them,
/// outermost first, read with local calls only, for a chain a walk did not
/// complete, whether cut at its bound, failed or stopped at the process
/// boundary: each is NVDA's MSAA client for it ([`client_pane`]), but a
/// ghost's or a hung application's window, which is a [`plain_window`], as
/// NVDA's object for it is (Fable, 2026-10-30, pending Dickson's
/// acceptance).
pub(super) fn plain_windows_from(context: &RequestContext<'_>, hwnd: isize) -> Vec<NodeSnapshot> {
    let desktop = desktop_window();
    let mut windows = Vec::new();
    let mut current = hwnd;
    while current != 0 && current != desktop && windows.len() < MAX_ANCESTOR_HOPS as usize {
        windows.push(if context.window_unreadable(current).is_none() {
            client_pane(context, current)
        } else {
            plain_window(context, current)
        });
        current = parent_window(current);
    }
    windows.reverse();
    windows
}

/// The chain from window `hwnd` up, outermost first, `hwnd`'s own entry
/// included: where a walk that ended below `hwnd` goes on
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback"). A window another process serves, and every window above it,
/// is read with no call into that process ([`plain_windows_from`]); a ghost's or
/// a hung application's window is a plain window, and the chain goes on
/// from its parent window. Any other window is read by its own object
/// chosen as NVDA chooses it (`Window._get_parent`): its UIA element when
/// the outpost judges it UIA and `uia` is given, else its MSAA client, each
/// walked up by its backend; a window whose object cannot be acquired is a
/// plain window too, and the chain goes on from its parent window, as
/// NVDA's object for it is then its `Window`. Past `out_of_time`, the rest
/// is read by [`plain_windows_from`]. The desktop ends it.
fn window_onward(
    context: &RequestContext<'_>,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    hwnd: isize,
    (previous, candidates): (&[NodeSnapshot], Option<&Candidates>),
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
            && let Some(chain) =
                window_object_onward(context, uia, current, (previous, candidates), out_of_time)
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
    context: &RequestContext<'_>,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    hwnd: isize,
    (previous, candidates): (&[NodeSnapshot], Option<&Candidates>),
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Option<Vec<NodeSnapshot>> {
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    if let Some((uia, request)) = uia
        && window_uses_uia(context, hwnd)
    {
        let element = uia.window_element(hwnd, request).ok()?;
        let top = element.snapshot(context.nodes());
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
        context.nodes(),
        verbatim_ia2::acquire::Purpose::Context,
    )?;
    if known(client.id) {
        let client_id = client.id;
        return Some(splice(previous, vec![client], client_id));
    }
    let above = msaa_ancestors(
        context,
        uia,
        &client,
        (previous, None, candidates),
        out_of_time,
    );
    Some(above.into_iter().chain(std::iter::once(client)).collect())
}

/// A UIA focus's ancestors, outermost first, from `ancestors`, the
/// raw-view chain the focus listener read from the event's own element,
/// nearest first, `nearest`, the focus's nearest window the listener
/// found, and `end`, how its walk ended: filtered as a walk here
/// filters them, stopping at a container of `previous` and reusing the
/// rest, and completed above as [`complete`] says: from a window read
/// through MSAA that the chain reaches, from the window above the process
/// boundary, where the listener's walk stops, from the window the listener
/// named after a failed step, or by plain windows from the window it named
/// after a cut walk. Each node is minted from its runtime id, with no
/// element: the outpost finds its own copy of an element only when it
/// needs one (`super::focus_path`). The continuation is part of the
/// listener's walk, so it ends at that walk's bound, `bound_at_us` on the
/// run's shared clock, measured from the event's observation
/// (`docs/design/focus-pipeline-rework.md`, the Terms), and the windows it
/// has not reached by then are plain windows, named by their text.
pub(super) fn uia_chain_from_read(
    context: &RequestContext<'_>,
    client: &mut Client,
    (ancestors, own_window, nearest, end): (
        &[verbatim_model::protocol::UiaAncestorFact],
        isize,
        Option<isize>,
        verbatim_model::protocol::AncestorsEnd,
    ),
    (previous, bound_at_us): (&[NodeSnapshot], u64),
) -> Vec<NodeSnapshot> {
    use verbatim_model::protocol::AncestorsEnd;
    let read_by_msaa = |hwnd: isize| uia_walk_leaves(context, nearest.unwrap_or(0), hwnd);
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
        context.nodes(),
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
    // What is left of the walk's bound, timed from here on the outpost's
    // clock, as every walk here is.
    let clock = context.clock();
    let left = std::time::Duration::from_micros(bound_at_us.saturating_sub(context.observed_now()));
    let out_of_time = time_limit(&clock, left);
    // Without a client the continuation reads every window it reaches
    // through MSAA or as a plain window.
    let request = client.uia_with(context, Context::ancestor_request).ok();
    complete(
        context,
        request.as_ref().map(|(uia, request)| (*uia, request)),
        (chain, above),
        (previous, None),
        Some(&out_of_time),
    )
}

/// The UIA nodes of `previous`, a previous chain, by runtime id, for a
/// walk to stop at, and their node ids, in the same order, for splicing.
fn known_uia_nodes(context: &Context, previous: &[NodeSnapshot]) -> (Vec<NodeId>, Vec<Vec<i32>>) {
    previous
        .iter()
        .filter_map(|node| {
            let runtime_id = context.store.runtime_id_of(node.id)?;
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
/// way, is completed by plain windows ([`plain_windows_above`]). A walk
/// that ends at a failed or
/// missing parent below the top
/// ([`verbatim_uia_rops::Ancestry::failed_hop`]) goes on from the parent
/// window of its outermost element's nearest window, and one cut at the
/// depth limit, or the classic walk's time, is completed by plain windows
/// from that nearest window ([`complete`]). The automation id of the
/// element's raw-view parent, the first ancestor read, is put in
/// `parent_automation_id` when the walk read it. The classic walk's time is
/// `left`, what is left of its bound; it tells `progress` each ancestor it
/// keeps ([`WalkStep::Ancestor`]), with the window plain windows would
/// complete the chain from as `outermost` follows it, while a program
/// tells nothing until it returns.
#[expect(
    clippy::too_many_lines,
    reason = "the program, its fallback, and the chain's completion, in the order they run"
)]
fn uia_remote_enrichment(
    context: &RequestContext<'_>,
    (uia, request): (&Uia, &Request<Ancestor>),
    element: &Cached<Node>,
    (previous, parent_automation_id): (&[NodeSnapshot], &mut Option<String>),
    (window, left): (Option<isize>, std::time::Duration),
    progress: Option<(WalkTold<'_>, &Outermost)>,
) -> Option<Result<Enrichment, NotFocused>> {
    if !context.tries_remote(window) {
        return None;
    }
    let clock = context.clock();
    let timed = time_limit(&clock, left);
    // A walk told to stop is out of time at its next step.
    let stopped = std::cell::Cell::new(false);
    let out_of_time = || timed() || stopped.get();
    // The classic walk tells each ancestor it keeps, shaped as the chain
    // reports it, with the window plain windows would complete the chain
    // from; a remote program tells nothing until it returns.
    let told = |read: verbatim_uia_rops::Progress<'_>| {
        let (Some((progress, outermost)), verbatim_uia_rops::Progress::Ancestor(ancestor)) =
            (progress, read)
        else {
            return;
        };
        let (mut shaped, _, _) = Uia::ancestors_from(
            std::slice::from_ref(ancestor),
            context.nodes(),
            &verbatim_uia::AncestorStops {
                read_by_other_api: &|_| false,
                known: &|_| false,
                out_of_time: None,
            },
        );
        let from = outermost.step(ancestor.read().node.role.hwnd);
        if !progress(WalkStep::Ancestor(shaped.pop().as_ref(), from)) {
            stopped.set(true);
        }
    };
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
        progress: progress.map(|_| &told as &dyn Fn(verbatim_uia_rops::Progress<'_>)),
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
            // the focus's chain is its windows, read with no call.
            tracing::debug!(%error, "the focus ancestry could not be read");
            let ancestors =
                plain_windows_above(context, element.read().role.hwnd, window.unwrap_or(0));
            return Some(Ok((ancestors, None)));
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
    let registry = context.nodes();
    let selected = ancestry
        .selected_child
        .as_ref()
        .map(|child| child.snapshot(registry));
    let nearest = ancestry.window.or(window).unwrap_or(0);
    let read_by_msaa = |hwnd: isize| uia_walk_leaves(context, nearest, hwnd);
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
        let hwnds = || {
            std::iter::once(focus_window).chain(
                ancestry
                    .ancestors
                    .iter()
                    .map(|ancestor| ancestor.read().node.role.hwnd),
            )
        };
        let nearest = ancestry.window.or(window).unwrap_or(0);
        if let Some(hop) = ancestry.failed_hop {
            walked = AncestorWalk::Failed(
                hop,
                verbatim_uia::parent_window(verbatim_uia::outermost_nearest_window(
                    nearest,
                    hwnds(),
                )),
            );
        } else if ancestry.depth_limited || ancestry.out_of_time {
            walked = AncestorWalk::Cut(verbatim_uia::plain_windows_start(nearest, hwnds()));
        }
    }
    let above = uia_above(crossed, walked, Some(outermost));
    let ancestors = complete(
        context,
        Some((uia, request)),
        (chain, above),
        (previous, None),
        Some(&out_of_time),
    );
    tracing::debug!(
        ?path,
        ancestors = ancestry.ancestors.len(),
        "UIA focus ancestry read"
    );
    Some(Ok((ancestors, selected)))
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
/// stays in MSAA. `progress` is told each ancestor read, the chain once
/// the walk has stopped, and the chain once it is complete ([`WalkStep`]).
fn msaa_ancestors(
    context: &RequestContext<'_>,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    node: &NodeSnapshot,
    (previous, progress, candidates): (&[NodeSnapshot], WalkProgress<'_>, Option<&Candidates>),
    out_of_time: Option<&dyn Fn() -> bool>,
) -> Vec<NodeSnapshot> {
    let known = |id: NodeId| previous.iter().any(|node| node.id == id);
    // Another process's window is a plain window, and is not probed.
    let read_by_uia = |hwnd: isize| uia.is_some() && own_window_uses_uia(context, hwnd);
    let plain = |hwnd: isize| reads_as_plain(context, hwnd);
    // A window object above a control is not reported, but the walk has
    // reached its window.
    let told = progress.map(|progress| {
        move |ancestor: &NodeSnapshot, hwnd: isize| {
            let _ = progress(WalkStep::Ancestor(
                Some(ancestor).filter(|ancestor| ancestor.role != Role::Window),
                hwnd,
            ));
        }
    });
    let limits = verbatim_ia2::acquire::AncestorLimits {
        max_hops: MAX_ANCESTOR_HOPS,
        known: &known,
        read_by_other_api: &read_by_uia,
        plain: &plain,
        out_of_time,
        progress: told
            .as_ref()
            .map(|told| told as verbatim_ia2::acquire::AncestorProgress<'_>),
        candidates,
    };
    let (chain, walked) =
        match verbatim_ia2::acquire::ancestor_chain_until(node.id, context.nodes(), &limits) {
            Ok(walk) => walk,
            // The node can no longer be reached: its chain is its windows,
            // read with no call.
            Err(error) => {
                tracing::debug!(?error, "the MSAA ancestors could not be read");
                let own = node.details.window.map_or(0, super::window::hwnd_of);
                let nearest = context.store.key_of(node.id).map_or(0, |(hwnd, _, _)| hwnd);
                return plain_windows_above(context, own, nearest);
            }
        };
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
    let Some(progress) = progress else {
        return complete(
            context,
            uia,
            (chain, above),
            (previous, candidates),
            out_of_time,
        );
    };
    if let Above::Plain(hwnd) | Above::From(hwnd) = above {
        let _ = progress(WalkStep::Chain(&chain, Some(hwnd)));
    }
    let chain = complete(
        context,
        uia,
        (chain, above),
        (previous, candidates),
        out_of_time,
    );
    let _ = progress(WalkStep::Chain(&chain, None));
    chain
}

/// An MSAA node's ancestors and, for a selection container, its selected
/// child, stopping at a container of `previous` and reusing the rest, within
/// `left`, what is left of the walk's bound (checked between calls; an MSAA
/// call has no bound of its own, and a UIA call in the walk ends after
/// UIA's call timeout), and completed above as [`msaa_ancestors`] says,
/// which tells `progress` what it reads. A failed selected child read is
/// none.
pub(super) fn msaa_enrichment(
    context: &RequestContext<'_>,
    client: &mut Client,
    (node, previous, candidates): (&NodeSnapshot, &[NodeSnapshot], Option<&Candidates>),
    (left, progress): (std::time::Duration, WalkProgress<'_>),
) -> Enrichment {
    // A window object above a control is layout, never announced as an
    // entered container: NVDA gives a window object reached through its
    // parents the `GenericWindow` class, which is not a presentable focus
    // ancestor. A dialog is still announced, by its client area's dialog
    // role.
    let clock = context.clock();
    let timed = time_limit(&clock, left);
    // A walk told to stop is out of time at its next step.
    let stopped = std::cell::Cell::new(false);
    let out_of_time = || timed() || stopped.get();
    let told = progress.map(|progress| {
        let stopped = &stopped;
        move |step: WalkStep<'_>| {
            let on = progress(step);
            if !on {
                stopped.set(true);
            }
            on
        }
    });
    let request = client.uia_with(context, Context::ancestor_request).ok();
    let ancestors = msaa_ancestors(
        context,
        request.as_ref().map(|(uia, request)| (*uia, request)),
        node,
        (
            previous,
            told.as_ref().map(|told| told as WalkTold<'_>),
            candidates,
        ),
        Some(&out_of_time),
    );
    let selected = if wants_selected_child(node.role) {
        verbatim_ia2::acquire::selected_child(node.id, context.nodes())
    } else {
        None
    };
    (ancestors, selected)
}

/// The kept MSAA nodes a focus walk compares what it reads with, by NVDA's
/// equality (`verbatim_ia2::equality`): the line of every report whose
/// focus node this outpost has not released, one of which is Core's, the
/// newest first, each innermost first. The lines are the walk's copies,
/// their nodes tagged with its request, so no release frees one while the
/// walk may name it ([`super::store::Store::tag_copied`]).
pub(super) fn msaa_candidates(context: &RequestContext<'_>) -> Candidates {
    let lines: Vec<Vec<NodeId>> = {
        let held = context.held();
        let line = |focus: &super::reducer::Focus| {
            std::iter::once(&focus.node)
                .chain(focus.ancestors.iter().rev())
                .filter(|node| node.backend == verbatim_model::Backend::Msaa)
                .map(|node| node.id)
                .collect::<Vec<NodeId>>()
        };
        held.focuses().rev().map(line).collect()
    };
    context
        .store
        .tag_copied(context.tag(), lines.iter().flatten().copied());
    Candidates::new(lines)
}

/// How long gathering a dialog's own text through UI Automation may take,
/// all its calls together: the text is worth a short wait, but the focus it
/// is announced with must not be held back long. Gathering reads the
/// dialog's descendants, many calls, so it has this deadline of its own,
/// enforced by abandonment ([`verbatim_uia::BoundedClient`]).
const DIALOG_TEXT_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// How many of `nodes`, a new focus line outermost first ending with its
/// focus, the focus had already entered from any report's line: those above
/// or at where the line meets one of theirs
/// (`verbatim_core::focus_rules::already_entered`).
fn entered_before<'a>(
    focuses: impl IntoIterator<Item = &'a super::reducer::Focus>,
    nodes: &[&mut NodeSnapshot],
) -> usize {
    let line: Vec<HeldNode> = nodes.iter().map(|node| HeldNode::of(node)).collect();
    let Some((focus, ancestors)) = line.split_last() else {
        return 0;
    };
    focuses
        .into_iter()
        .map(|held| already_entered((&held.ancestors, &held.node), (ancestors, focus)))
        .max()
        .unwrap_or(0)
}

/// The dialog this outpost last gathered text for in a foreground report,
/// and the text: the focus that follows it into the dialog reuses it rather
/// than reading the dialog again.
pub(super) type DialogMemo = Option<(NodeId, Option<String>)>;

/// Gives every dialog among `nodes`, the chain of a focus, outermost first,
/// that has no description of its own, its own text as its description
/// (`docs/nvda/object-model.md`, "A dialog's own text"), while descriptions
/// are read at all. A dialog no report's line is already inside is read,
/// by the rule the reducer speaks entered containers by
/// (`verbatim_core::focus_rules::already_entered`); one a report's line is
/// already inside, a superset of Core's line, is given the text that
/// report carried for it, with no call, so Core, which speaks it only when
/// its own line enters the dialog, always has it
/// (`docs/design/ownership-one-judge.md`, "The reports, from the store").
///
/// `memo` is taken: a dialog it names is given its text without reading it
/// again. With `remember`, the last text read is kept in it for the next
/// report, which is how a foreground report hands its dialog to the focus
/// inside it.
pub(super) fn describe_dialogs<'a>(
    context: &RequestContext<'_>,
    nodes: impl IntoIterator<Item = &'a mut NodeSnapshot>,
    (memo, remember): (&mut DialogMemo, bool),
) {
    let mut remembered = memo.take();
    if !context.fetches().description {
        return;
    }
    let mut nodes: Vec<&mut NodeSnapshot> = nodes.into_iter().collect();
    // The reports the focus enters a dialog from, one of whose lines is
    // Core's. A dialog at or above where the new line meets any of them is
    // not read again: it carries the text a report gave it.
    let (entered, carried) = {
        let held = context.held();
        let entered = entered_before(held.focuses(), &nodes);
        let carried: Vec<Option<Option<String>>> = nodes
            .iter()
            .take(entered)
            .map(|node| {
                is_dialog(node.role)
                    .then(|| held.dialog_text(node.id))
                    .flatten()
            })
            .collect();
        (entered, carried)
    };
    for (index, node) in nodes.iter_mut().enumerate() {
        if !is_dialog(node.role)
            || node
                .details
                .description
                .as_deref()
                .is_some_and(|description| !description.trim().is_empty())
        {
            continue;
        }
        if index < entered {
            if let Some(Some(text)) = carried.get(index) {
                node.details.description.clone_from(text);
            }
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
fn dialog_text_of(context: &RequestContext<'_>, node: &NodeSnapshot) -> Option<String> {
    use crate::dialog_text::{UiaDialog, dialog_text};
    match node.backend {
        Backend::Msaa => verbatim_ia2::dialog::DialogObject::of_node(node.id, context.nodes())
            .and_then(|dialog| dialog_text(&dialog)),
        Backend::Uia => {
            // A dialog the focus listener read in the focus's chain, whose
            // element this outpost does not hold, is found by its path.
            let element = context.store.element_of(node.id);
            let route = if element.is_some() {
                None
            } else {
                let runtime_id = context.store.runtime_id_of(node.id)?;
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
    context: &RequestContext<'_>,
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
                ancestors: plain_windows_from(context, parent_window(hung)),
                selected_child: None,
                window: Some(window_facts(hung)),
                text: None,
            },
        ));
    }
    let hwnd = checked_focus_window(context)?;
    // The focused element is read from the window's provider at once.
    if window_uses_uia(context, hwnd) {
        uia_focused_control(
            context,
            client,
            hwnd,
            (trigger, false),
            (ENRICHMENT_BUDGET, None),
        )
    } else {
        // A focus-now answer has no chain to splice from, so it walks to
        // the desktop, comparing what it reads with Core's line and the
        // newest focus it waits for (`docs/design/focus-pipeline-rework.md`,
        // "MSAA object equality").
        let candidates = msaa_candidates(context);
        let node = verbatim_ia2::acquire::focused_snapshot(
            hwnd,
            context.nodes(),
            (&|window| serves(context, window), Some(&candidates)),
        )
        .ok_or(FocusReadMiss::Failed(None))?;
        let (ancestors, selected_child) = msaa_enrichment(
            context,
            client,
            (&node, &[], Some(&candidates)),
            (ENRICHMENT_BUDGET, None),
        );
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
/// Its walk's bound is measured from `started_us`, when the read begins on
/// the run's shared clock ([`walk_left`]), and `progress` is told what the
/// walk reads ([`WalkStep`]).
pub(super) fn gap_focus_read(
    context: &RequestContext<'_>,
    client: &mut Client,
    (started_us, progress): (u64, WalkProgress<'_>),
) -> Result<(isize, FocusedControl), FocusReadMiss> {
    let hwnd = checked_focus_window(context)?;
    if !window_uses_uia(context, hwnd) {
        tracing::debug!(
            hwnd,
            "the gap's read is refused: the outpost does not judge the focus window UIA"
        );
        return Err(FocusReadMiss::Refused);
    }
    let left = walk_left(context, hwnd, started_us);
    uia_focused_control(
        context,
        client,
        hwnd,
        (FocusRead::Gap, true),
        (left, progress),
    )
}

/// [`focused_control_in`]'s read through UIA, of the focus window `hwnd`,
/// for `trigger`; with `recheck`, the read is refused once the focused
/// element is read if the focus window has changed since to one this
/// outpost does not judge UIA ([`gap_focus_read`]). Its walk has `left` of
/// its bound left, and tells `progress` the focus once read and what it
/// reads after ([`WalkStep`]).
fn uia_focused_control(
    context: &RequestContext<'_>,
    client: &mut Client,
    hwnd: isize,
    (trigger, recheck): (FocusRead, bool),
    (left, progress): (std::time::Duration, WalkProgress<'_>),
) -> Result<(isize, FocusedControl), FocusReadMiss> {
    {
        let (uia, nodes) = client
            .uia_with(context, Context::node_request)
            .map_err(|_| FocusReadMiss::Failed(None))?;
        let ancestor_request = context
            .ancestor_request(uia)
            .map_err(|error| FocusReadMiss::Failed(Some(error.code())))?;
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
        let mut node =
            with_legacy_checked_state(element.live().element(), element.snapshot(context.nodes()));
        // The element's own UIA class, from its cache, for Core's rules for
        // an application's elements (decision D6): no call.
        let fields = element.read();
        node.details.class_name.clone_from(&fields.role.class_name);
        console_text_area(&mut node, Some(hwnd));
        // Its walk begins, the chain completed by plain windows from its
        // nearest window, or that window's parent when the focus is the
        // window's own element, should nothing more be read.
        let outermost = Outermost::new(hwnd);
        if let Some(progress) = progress {
            let _ = progress(WalkStep::Focus {
                node: &node,
                window: hwnd,
                from: outermost.step(fields.role.hwnd),
                left,
            });
        }
        // A focus that has moved on since the focused element was read is
        // still this query's answer, read the classic way. Its parent's
        // automation id, for the same rules, comes with its ancestors.
        let mut parent_automation_id = None;
        let (ancestors, selected_child) = match uia_remote_enrichment(
            context,
            (uia, &ancestor_request),
            &element,
            (&[], &mut parent_automation_id),
            (Some(hwnd), left),
            progress.map(|progress| (progress, &outermost)),
        ) {
            Some(Ok(enrichment)) => enrichment,
            Some(Err(NotFocused)) | None => uia_enrichment(
                context,
                (uia, &nodes, &ancestor_request),
                (&element, hwnd),
                (node.role, &mut parent_automation_id),
                (&[], left),
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
        .map_err(|_| FocusReadMiss::Failed(None))?;
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
    let focused_request = nodes
        .with_process()
        .map_err(|error| FocusReadMiss::Failed(Some(error.code())))?;
    tracing::debug!(?trigger, hwnd, "the application's focused element is read");
    let element = (context.focused_element)(uia, &focused_request)
        .map_err(|error| FocusReadMiss::Failed(Some(error.code())))?;
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
    /// The read itself failed, with the error's `HRESULT` when the failed
    /// call gave one.
    Failed(Option<windows::core::HRESULT>),
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
pub(super) fn focus_now(
    context: &RequestContext<'_>,
    client: &mut Client,
) -> (FocusNow, Option<isize>) {
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
    context: &RequestContext<'_>,
    uia: &Uia,
    nodes: &Request<Node>,
    node_id: NodeId,
) -> Result<Cached<Node>, ReadError> {
    if let Some(agile) = context.store.element_of(node_id) {
        if let Ok(element) = agile.resolve() {
            match element.live().refresh(nodes) {
                Ok(fresh) => return Ok(fresh),
                Err(error) if !verbatim_uia::element_is_gone(&error) => {
                    return Err(ReadError::Failed(format!("reading the element: {error}")));
                }
                Err(_) => {}
            }
        }
        context.store.evict_element(node_id);
    }
    let runtime_id = context
        .store
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
        .nodes()
        .id_for_element(&runtime_id, &element.narrow());
    Ok(element)
}

/// A UIA node's live element, filled as a node, or the error a query
/// reports for it.
fn uia_node<'a>(
    context: &RequestContext<'_>,
    client: &'a mut Client,
    node_id: NodeId,
) -> Result<(&'a Uia, Request<Node>, Cached<Node>), ReadError> {
    let (uia, nodes) = client.uia_with(context, Context::node_request)?;
    let element = resolve_uia_element(context, uia, &nodes, node_id)?;
    Ok((uia, nodes, element))
}

/// A node's ancestors, outermost first.
pub(super) fn ancestors(
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
) -> Result<Vec<NodeSnapshot>, ReadError> {
    if context.store.runtime_id_of(node_id).is_some() {
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
    verbatim_ia2::acquire::ancestor_chain(node_id, context.nodes(), MAX_ANCESTOR_HOPS)
        .map_err(ReadError::from)
}

/// One object-navigation step: the neighbor, or `None` for a genuine tree
/// edge, never conflated with a node that is gone.
pub(super) fn navigate(
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
    kind: QueryKind,
) -> Result<Option<NodeSnapshot>, ReadError> {
    let (neighbor, from_window) = if context.store.runtime_id_of(node_id).is_some() {
        let (neighbor, from_window) =
            if let Some(step) = uia_remote_step(context, client, node_id, kind)? {
                step
            } else {
                let (uia, nodes, element) = uia_node(context, client, node_id)?;
                let from_window = verbatim_uia::nearest_window_handle(element.live().element());
                let neighbor = uia
                    .neighbor(element.live(), &nodes, context.nodes(), kind)
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
                .store
                .element_of(neighbor.id)
                .and_then(|agile| agile.resolve().ok())
            {
                Some(element) => with_legacy_checked_state(element.live().element(), neighbor),
                None => neighbor,
            }
        });
        (neighbor, from_window)
    } else {
        let from_window = context.store.key_of(node_id).map(|key| key.0);
        let neighbor = verbatim_ia2::acquire::navigate(node_id, context.nodes(), kind)?;
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
    context: &RequestContext<'_>,
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
        .store
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
            context.store.evict_element(node_id);
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
        .map(|neighbor| neighbor.snapshot(context.nodes()));
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
    context: &RequestContext<'_>,
    client: &mut Client,
    from_window: Option<isize>,
    neighbor: NodeSnapshot,
    kind: QueryKind,
) -> NodeSnapshot {
    let Some(from_window) = from_window.filter(|&hwnd| hwnd != 0) else {
        return neighbor;
    };
    if context.store.runtime_id_of(neighbor.id).is_some() {
        let window = context
            .store
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
            context.nodes(),
            verbatim_ia2::acquire::Purpose::Announce,
        )
        .unwrap_or(neighbor);
    }
    let Some((window, _, _)) = context.store.key_of(neighbor.id) else {
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
        .map_or(neighbor, |element| element.snapshot(context.nodes()))
}

/// Activates a node.
/// How many ancestors activation tries when the node itself has no action.
const ACTIVATION_PARENT_HOPS: u32 = 8;

/// Activates `node_id`, or failing that its nearest ancestor that can be
/// activated, as NVDA's review activate walks up the navigator object's
/// parents until one performs an action, and answers the action's name.
pub(super) fn activate(
    context: &RequestContext<'_>,
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
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
) -> Result<Option<ActionName>, ReadError> {
    if context.store.runtime_id_of(node_id).is_some() {
        let (uia, _, element) = uia_node(context, client, node_id)?;
        return uia
            .activate(element.live().element())
            .map_err(|error| ReadError::Failed(format!("UIA activation failed: {error}")));
    }
    verbatim_ia2::acquire::activate(node_id, context.nodes()).map_err(ReadError::from)
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
    context: &RequestContext<'_>,
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
            registry: context.nodes(),
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
            context.nodes(),
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
    use verbatim_core::focus_rules::HeldNode;
    use verbatim_model::{
        Backend, NodeDetails, NodeId, NodeSnapshot, Role, StateSet, WindowHandle,
    };

    use super::super::reducer::{Evidence, Focus, Held};
    use super::super::window::window_owner;
    use super::{
        CONSOLE_WINDOW_CLASS, console_text_area_in, control_window, entered_before, splice_onto,
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
        let (spliced, above) = splice_onto(&previous, vec![node(2), node(4)], NodeId::new(2));
        assert_eq!(ids(&spliced), ids(&[node(1), node(2), node(4)]));
        assert_eq!(ids(above), ids(&[node(1)]));
    }

    #[test]
    fn only_the_console_host_reads_the_focus_in_its_console_window() {
        verbatim_test_console::Console::with(|console| {
            let (_, client) = window_owner(console.window);
            assert_eq!(control_window(client, || Some(console.window)), None);
            assert_eq!(
                control_window(console.host, || Some(console.window)),
                Some(console.window)
            );
        });
    }

    #[test]
    fn a_walk_meeting_nothing_known_is_kept_whole() {
        let previous = [node(1)];
        let (spliced, above) = splice_onto(&previous, vec![node(5), node(6)], NodeId::new(9));
        assert_eq!(ids(&spliced), ids(&[node(5), node(6)]));
        assert_eq!(ids(above), Vec::<NodeId>::new());
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
    fn a_dialog_above_where_the_lines_meet_is_not_newly_entered() {
        let held_node = |id, role, name: Option<&str>, window: Option<u64>| HeldNode {
            id: NodeId::new(id),
            backend: Backend::Msaa,
            role,
            name: name.map(str::to_owned),
            states: StateSet::new(),
            window: window.map(WindowHandle),
        };
        let mut held = Held::default();
        held.reported(
            Focus {
                node: held_node(2, Role::Button, Some("OK"), None),
                ancestors: vec![held_node(1, Role::Dialog, Some("Remove"), Some(20))],
                top_level: 7,
                entered_in_front: None,
                event: None,
                simple_child: false,
                dialog_texts: Vec::new(),
            },
            Evidence::Msaa,
        );
        let node = |id, role, window: Option<u64>| NodeSnapshot {
            id: NodeId::new(id),
            backend: Backend::Msaa,
            role,
            name: Some("Remove".to_owned()),
            value: None,
            states: StateSet::new(),
            details: NodeDetails {
                window: window.map(WindowHandle),
                ..NodeDetails::default()
            },
        };
        let mut cancel = node(4, Role::Button, None);
        let mut other = node(3, Role::Dialog, Some(21));
        assert_eq!(
            entered_before(held.focuses(), &[&mut other, &mut cancel]),
            0,
            "a dialog of another window with the same role and name"
        );
        let mut same = node(3, Role::Dialog, Some(20));
        assert_eq!(
            entered_before(held.focuses(), &[&mut same, &mut cancel]),
            1,
            "the dialog window the lines meet at"
        );
    }
}
