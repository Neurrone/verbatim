//! The worker's reads from the application: windows, the focus, a node's
//! ancestors, navigation, activation, and tree dumps, through whichever
//! backend owns the window. Every function here may call into the
//! application and so runs only on the worker. None has a time limit of its
//! own: a program runs until it ends, as NVDA's reads do, and Core bounds
//! its request by the request's due time.

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::collections::BTreeSet;

use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::CHILDID_SELF;
use verbatim_ia2::equality::Candidates;
use verbatim_model::{
    ActionName, Backend, NodeDetails, NodeId, NodeSnapshot, QueryKind, Role, State, StateSet,
};
use verbatim_uia::Interning as _;
use verbatim_uia::map::with_legacy_checked_state;
use verbatim_uia::{
    Ancestor, AncestorWalk, Cached, DialogText, Fill, Live, Node, Request, Shape, Uia,
};

use verbatim_reader_rules::dialog::{Visited as Identity, is_dialog};

use verbatim_model::protocol::UiaSnapshotFact;
use verbatim_model::request::KnownAncestor;
use verbatim_win32::window::window_class_name;

use super::window::{window_style, window_text};
use super::{Context, RequestContext};
use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_win32::provider::served_by;
use verbatim_win32::window::{
    desktop_window, parent, top_level_of, window_belongs_to_hidden_frame, window_exists,
};

/// Cap on the ancestors read for a node.
pub(super) const MAX_ANCESTOR_HOPS: u32 = 64;

/// Told of each ancestor a UI Automation walk steps to, by its runtime id
/// and its own window ([`verbatim_uia::AncestorStops::visited`]).
type Visited<'a> = Option<verbatim_uia::Visited<'a>>;

/// A window a focus's chain went on from, which this outpost reads through
/// UIA, had an element that could not be read: the focus is lost, as
/// NVDA's `ElementFromHandleBuildCache` there is unguarded and its
/// exception ends the event (`NVDAObjects/UIA/__init__.py` 1553 to 1557,
/// `eventHandler.py` 357 to 358; Dickson, 2026-11-07). The program
/// reports nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FocusLost;

/// Whether `id` is among `known`, the ancestors Core already knows, by its
/// node id: a walk stops at it, and the answer is cut there
/// ([`super::asked::cut_at_known`]).
pub(super) fn knows(known: &[KnownAncestor], id: NodeId) -> bool {
    known
        .iter()
        .any(|known| matches!(known, KnownAncestor::Node(node) if *node == id))
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
    /// Shared, so an executor's arm holds it while the resolver it calls
    /// back through reads with the same client.
    uia: Option<std::rc::Rc<Uia>>,
    failed: bool,
}

impl Client {
    /// The UIA client, created on first use. `None` if it cannot be created.
    pub(super) fn uia(&mut self) -> Option<&Uia> {
        if self.uia.is_none() && !self.failed {
            match Uia::new() {
                Ok(client) => self.uia = Some(std::rc::Rc::new(client)),
                Err(error) => {
                    self.failed = true;
                    tracing::warn!(%error, "the worker could not create a UIA client");
                }
            }
        }
        self.uia.as_deref()
    }

    /// The UIA client, created on first use, shared: an executor's arm
    /// holds it while it calls back through a resolver holding `self`.
    pub(super) fn shared_uia(&mut self) -> Option<std::rc::Rc<Uia>> {
        self.uia()?;
        self.uia.clone()
    }

    /// The UIA client and the request `request` builds for the details the
    /// program's request names ([`RequestContext::node_request`],
    /// [`RequestContext::ancestor_request`]).
    pub(super) fn uia_with<'c, S: Shape>(
        &mut self,
        context: &RequestContext<'c>,
        request: fn(&RequestContext<'c>, &Uia) -> windows::core::Result<Request<S>>,
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

/// Decides a window's backend: by Core's verdict, when the request names
/// the window with one; else with the outpost's arbitrator and the
/// request's copy of the verdicts ([`crate::arbitration::uses_uia`]).
/// Returns `true` for UIA.
pub(super) fn window_uses_uia(context: &RequestContext<'_>, hwnd: isize) -> bool {
    if let Some((window, uia)) = context.given_verdict.get()
        && window == hwnd
    {
        return uia;
    }
    crate::arbitration::uses_uia(
        &context.arbitrator,
        (&context.verdicts, context.observed_now()),
        hwnd,
        || verbatim_uia::console_reports_formatting(hwnd),
    )
}

/// Whether the outpost's application serves window `hwnd`, by the
/// window's provider process, read locally ([`served_by`]). An outpost
/// connects to, probes, and walks up into only such windows
/// (`books/dev/src/explanation/design/listener-without-connections.md`, section 4.3).
pub(super) fn serves(context: &Context, hwnd: isize) -> bool {
    served_by(hwnd, context.target_pid())
}

/// [`window_uses_uia`] for a window of the outpost's own application;
/// `false`, without a probe, for any other process's window, which the
/// outpost reads through MSAA whether it has a provider or not
/// (`books/dev/src/explanation/design/listener-without-connections.md`, sections 4.3 and 4.4).
fn own_window_uses_uia(context: &RequestContext<'_>, hwnd: isize) -> bool {
    serves(context, hwnd) && window_uses_uia(context, hwnd)
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
    if window_uses_uia(context, hwnd) {
        super::uia_object::window(context, client, hwnd).map(|node| (Backend::Uia, node))
    } else {
        super::msaa_object::window(context, client, hwnd).map(|node| (Backend::Msaa, node))
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

/// Where a chain of ancestors starts: a UI Automation element, with its
/// own window handle and its nearest window, 0 when not known, the client
/// that walks it, and what is told of the walk; or an MSAA node, with the
/// UI Automation client the chain goes on through, if any.
pub(super) enum ChainStart<'a> {
    /// A UI Automation element.
    Uia {
        /// The client and how each ancestor is filled.
        uia: (&'a Uia, &'a Request<Ancestor>),
        /// The element, its own window and its nearest window.
        element: (&'a Live, isize, isize),
        /// Given the automation id of the element's raw-view parent, when
        /// the walk's first step read it.
        parent_automation_id: &'a mut Option<String>,
        /// Told of each ancestor visited.
        visited: Visited<'a>,
    },
    /// An MSAA node, its window objects above controls left out (see
    /// [`msaa_enrichment`]); without a UI Automation client the chain
    /// stays in MSAA.
    Msaa(&'a NodeSnapshot, Option<(&'a Uia, &'a Request<Ancestor>)>),
}

/// The chain of ancestors above `start`, outermost first: the walk of its
/// backend's executor's `Ancestors` arm, stopping at an ancestor of
/// `previous`, those Core knows, and chained above where it ends as
/// [`complete`] says, the dispatcher's chaining across backends.
///
/// A UI Automation walk that reaches the root of a window read through
/// MSAA or as a plain window ([`uia_walk_leaves`]), or stops below the
/// process boundary, goes on from that window ([`window_onward`]), as NVDA
/// switches API when its walk crosses into such a window: in File
/// Explorer, the folder window above the UIA file list is then the client
/// area a foreground report already named, not a second, UIA copy of it.
/// An MSAA walk whose parent lies in a window read through UIA goes on
/// from that window's UIA element (`correctAPIForRelation`): in a Save As
/// dialog, the file name box's containers above the shell's UIA view are
/// UIA's; where a parent lies in a window read as a plain window
/// ([`reads_as_plain`]), or an `accParent` fails or names nothing, from
/// that window or the parent window. A walk cut by its hop limit is
/// completed by plain windows, and one that fails before it reads anything
/// goes on from the parent window of the start's nearest window
/// ([`onward_after_failure`]). The desktop window ends the chain.
///
/// # Errors
///
/// [`FocusLost`], from [`window_onward`].
pub(super) fn chain(
    context: &RequestContext<'_>,
    start: ChainStart<'_>,
    (previous, candidates): (&[KnownAncestor], Option<&Candidates>),
) -> Result<Vec<NodeSnapshot>, FocusLost> {
    let known = |id: NodeId| knows(previous, id);
    let (uia, request, element, element_window, nearest, parent_automation_id, visited) =
        match start {
            ChainStart::Msaa(node, uia) => {
                return msaa_chain(context, uia, node, (previous, candidates));
            }
            ChainStart::Uia {
                uia: (uia, request),
                element: (element, element_window, nearest),
                parent_automation_id,
                visited,
            } => (
                uia,
                request,
                element,
                element_window,
                nearest,
                parent_automation_id,
                visited,
            ),
        };
    let read_by_msaa = |hwnd: isize| uia_walk_leaves(context, nearest, hwnd);
    let start = AncestryStart {
        element: element.element().clone(),
        own_window: element_window,
        nearest,
        request,
        registry: context.nodes(),
        read_by_other_api: &read_by_msaa,
        known: &known,
        visited,
    };
    let Some(walked) = start.walk(uia) else {
        return onward_after_failure(
            context,
            Some((uia, request)),
            (element_window, nearest),
            (previous, None),
        );
    };
    parent_automation_id.clone_from(&walked.parent_automation_id);
    // The walk stops below the process boundary itself and answers the
    // parent or owner window there as `crossed`.
    let above = uia_above(walked.crossed, walked.walk, None);
    complete(
        context,
        Some((uia, request)),
        (walked.chain, above),
        (previous, None),
    )
}

/// Where a UI Automation `Ancestors` arm walks from and where it stops,
/// the executor's resolver ([`verbatim_uia_rops::batch::AncestryFrom`]).
struct AncestryStart<'a> {
    element: windows::Win32::UI::Accessibility::IUIAutomationElement,
    own_window: isize,
    nearest: isize,
    request: &'a Request<Ancestor>,
    registry: &'a dyn verbatim_uia::Interning,
    read_by_other_api: &'a dyn Fn(isize) -> bool,
    known: &'a dyn Fn(NodeId) -> bool,
    visited: Visited<'a>,
}

impl AncestryStart<'_> {
    /// The walk, run alone through the UI Automation executor's
    /// `Ancestors` arm: `None` when a step failed before the walk could
    /// answer.
    fn walk(&self, uia: &Uia) -> Option<verbatim_uia_rops::batch::Ancestors> {
        use verbatim_uia_rops::batch::{Found, Ran, Segment};
        let steps = [verbatim_model::batch::Step {
            op: verbatim_model::batch::Op::Ancestors {
                subject: verbatim_model::batch::Subject::Focus,
                until: Vec::new(),
                depth: MAX_ANCESTOR_HOPS,
                set: Vec::new(),
            },
            when: None,
        }];
        let answer = verbatim_uia_rops::batch::run_calls(&Segment {
            uia,
            steps: &steps,
            first: 0,
            earlier: &[],
            resolver: self,
            remote: false,
            window: None,
        });
        match answer.steps.into_iter().next() {
            Some(Ran::Done(Found::Ancestors(walked))) => Some(walked),
            _ => None,
        }
    }
}

impl verbatim_uia_rops::batch::Resolve for AncestryStart<'_> {
    fn ancestry(
        &self,
        _subject: &verbatim_model::batch::Subject,
    ) -> Result<verbatim_uia_rops::batch::AncestryFrom<'_>, verbatim_uia_rops::batch::Ran> {
        Ok(verbatim_uia_rops::batch::AncestryFrom {
            element: self.element.clone(),
            own_window: self.own_window,
            nearest: self.nearest,
            request: self.request,
            registry: self.registry,
            max_hops: MAX_ANCESTOR_HOPS,
            stops: verbatim_uia::AncestorStops {
                read_by_other_api: self.read_by_other_api,
                known: self.known,
                out_of_time: None,
                visited: self.visited,
            },
        })
    }
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
pub(super) fn uia_walk_leaves(context: &RequestContext<'_>, nearest: isize, hwnd: isize) -> bool {
    let plain = if hwnd == nearest {
        !serves(context, hwnd)
    } else {
        reads_as_plain(context, hwnd)
    };
    hwnd != desktop_window() && (plain || !window_uses_uia(context, hwnd))
}

/// How a chain goes on above its outermost entry
/// (`books/dev/src/reference/adr/0001-focus.md`, "When an application's call fails").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Above {
    /// It does not: the walk reached the desktop.
    Nothing,
    /// The walk met this ancestor Core knows, the chain's outermost, where
    /// the answer is cut.
    Met(NodeId),
    /// From this window's own object ([`window_onward`]): the walk reached
    /// a window read through the other API, stopped below the process
    /// boundary, or its step to a parent failed or timed out, when the
    /// window is the parent window of the outermost entry's nearest window.
    From(isize),
    /// By plain windows from this window, included: the walk was cut at
    /// its depth limit, and the window is the outermost element's nearest
    /// window, never read, or its parent when that element is the window's
    /// own ([`verbatim_win32::provider::plain_windows_start`]).
    Plain(isize),
}

/// How a UIA walk that ended as `walked`, having crossed into the window
/// `crossed` names, goes on above its chain. A complete walk with no window
/// crossed into goes on above the process boundary when `outermost`, the
/// window of its outermost element, is below one
/// ([`verbatim_win32::provider::parent_across_process_boundary`]), as a chain the
/// listener or a remote program read stops there.
pub(super) fn uia_above(
    crossed: Option<isize>,
    walked: AncestorWalk,
    outermost: Option<isize>,
) -> Above {
    match (walked, crossed) {
        (AncestorWalk::MetKnown(met), _) => Above::Met(met),
        (_, Some(hwnd)) | (AncestorWalk::Failed(_, hwnd), None) => Above::From(hwnd),
        (AncestorWalk::Cut(hwnd), None) => Above::Plain(hwnd),
        (AncestorWalk::Complete, None) => outermost
            .filter(|&window| window != 0)
            .and_then(verbatim_win32::provider::parent_across_process_boundary)
            .map_or(Above::Nothing, Above::From),
    }
}

/// `chain`, outermost first, completed above as `above` says: ended
/// where it met an ancestor Core knows, continued from a
/// window's own object ([`window_onward`]), or completed by plain windows
/// with no call ([`plain_windows_from`]).
///
/// # Errors
///
/// [`FocusLost`], from [`window_onward`].
pub(super) fn complete(
    context: &RequestContext<'_>,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    (chain, above): (Vec<NodeSnapshot>, Above),
    (previous, candidates): (&[KnownAncestor], Option<&Candidates>),
) -> Result<Vec<NodeSnapshot>, FocusLost> {
    Ok(match above {
        // A chain that met an ancestor Core knows is cut there.
        Above::Nothing | Above::Met(_) => chain,
        Above::From(hwnd) => window_onward(context, uia, hwnd, (previous, candidates))?
            .into_iter()
            .chain(chain)
            .collect(),
        Above::Plain(hwnd) => plain_windows_from(context, hwnd)
            .into_iter()
            .chain(chain)
            .collect(),
    })
}

/// Whether window `hwnd` is read as a plain window, with no call
/// (`books/dev/src/reference/adr/0001-focus.md`, "Object identity"): a window another process
/// serves, the parent or visible
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
/// and never invisible either (Fable, 2026-10-30): a window not yet shown
/// when it is read, as a Windows Terminal window at the listener's bound,
/// would lose its title for good, where
/// NVDA reads it through UIA and says it. It is kept at its window
/// object's address, so a node read from it acquires the window's own
/// object.
pub(super) fn plain_window(context: &RequestContext<'_>, hwnd: isize) -> NodeSnapshot {
    window_from_local_state(context, hwnd, false)
}

/// Window `hwnd`, neither a ghost's nor a hung application's, read with no
/// call into its process, as NVDA's MSAA client object for it
/// (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the window title"): a
/// pane, marked `top_level_client` when its style has a system menu, as
/// `verbatim_ia2`'s reader marks an MSAA client, so a top-level window is
/// spoken by its title and a child window is structure, and in all else a
/// [`plain_window`], read from Windows' own state.
pub(super) fn client_pane(context: &RequestContext<'_>, hwnd: isize) -> NodeSnapshot {
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

/// How far the outpost reads window `hwnd`, judged with local calls in the
/// order NVDA's `Window._get_parent` chooses the object of a window
/// (`findBestAPIClass`, `NVDAObjects/window/__init__.py` 84 to 98): a
/// window another process serves is above the process boundary; a ghost's
/// or a hung application's is a plain window; any other is read by its own
/// object, whose backend the caller judges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    /// Another process serves it.
    AboveBoundary,
    /// A ghost's or a hung application's window.
    Plain,
    /// The application's own window, readable.
    Own,
}

/// How far the outpost reads window `hwnd` ([`Reach`]).
fn reach(context: &RequestContext<'_>, hwnd: isize) -> Reach {
    if !serves(context, hwnd) {
        Reach::AboveBoundary
    } else if context.window_unreadable(hwnd).is_some() {
        Reach::Plain
    } else {
        Reach::Own
    }
}

/// The stand-in for window `hwnd` read from Windows' own state alone, for
/// a window the outpost makes no call into: NVDA's MSAA client for it
/// ([`client_pane`]), but a ghost's or a hung application's window, which
/// is a [`plain_window`], as NVDA's object for it is (Fable, 2026-10-30).
fn stand_in(context: &RequestContext<'_>, hwnd: isize) -> NodeSnapshot {
    if context.window_unreadable(hwnd).is_none() {
        client_pane(context, hwnd)
    } else {
        plain_window(context, hwnd)
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
    context: &RequestContext<'_>,
    client: &mut Client,
    hwnd: isize,
) -> verbatim_ia2::acquire::ParentWindow {
    use verbatim_ia2::acquire::ParentWindow;
    match reach(context, hwnd) {
        Reach::AboveBoundary => return ParentWindow::AboveBoundary,
        Reach::Plain => return ParentWindow::Plain,
        Reach::Own => {}
    }
    if !window_uses_uia(context, hwnd) {
        return ParentWindow::Msaa;
    }
    let focused = client
        .uia_with(context, RequestContext::node_request)
        .ok()
        .and_then(|(uia, nodes)| uia.window_element(hwnd, &nodes).ok())
        .map(|element| element.read().states.has_focus);
    ParentWindow::Uia(focused)
}

/// The chain of a focus whose walk failed before it read any ancestor: it
/// goes on from the parent window of the focus's nearest window, `own`,
/// the window the focus is when it is one, else `nearest`, the window it is
/// in, by that window's own object read live ([`window_onward`]), as NVDA's
/// object, which carries its nearest window, reads the window hierarchy
/// when its parent read fails (`Window._get_parent`,
/// `NVDAObjects/window/__init__.py` 287 to 293), so the nearest window
/// itself is not in the chain. Empty, and logged, when neither is known,
/// the one chain that is not complete.
///
/// # Errors
///
/// [`FocusLost`], from [`window_onward`].
pub(super) fn onward_after_failure(
    context: &RequestContext<'_>,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    (own, nearest): (isize, isize),
    (previous, candidates): (&[KnownAncestor], Option<&Candidates>),
) -> Result<Vec<NodeSnapshot>, FocusLost> {
    let nearest = if own == 0 { nearest } else { own };
    if nearest == 0 {
        tracing::warn!("a focus's walk failed with no window known; it is reported with no chain");
        return Ok(Vec::new());
    }
    window_onward(context, uia, parent(nearest), (previous, candidates))
}

/// The windows from `hwnd` up to the desktop, which is none of them,
/// outermost first, read with local calls only, for a chain cut at its
/// depth limit or stopped at the process boundary: each its
/// [`stand_in`]. Each window from the first another process serves upward
/// is above the process boundary, a stand-in for its own application's
/// object ([`NodeDetails::above_boundary`]).
pub(super) fn plain_windows_from(context: &RequestContext<'_>, hwnd: isize) -> Vec<NodeSnapshot> {
    let desktop = desktop_window();
    let mut windows = Vec::new();
    let mut current = hwnd;
    let mut above_boundary = false;
    while current != 0 && current != desktop && windows.len() < MAX_ANCESTOR_HOPS as usize {
        above_boundary = above_boundary || !serves(context, current);
        let mut window = stand_in(context, current);
        window.details.above_boundary = above_boundary;
        windows.push(window);
        current = parent(current);
    }
    windows.reverse();
    windows
}

/// The chain from window `hwnd` up, outermost first, `hwnd`'s own entry
/// included: where a walk that ended below `hwnd` goes on
/// (`books/dev/src/reference/adr/0001-focus.md`, "Object identity"). A window another process
/// serves, and every window above it,
/// is read with no call into that process ([`plain_windows_from`]); a ghost's or
/// a hung application's window is a plain window, and the chain goes on
/// from its parent window. Any other window is read by its own object
/// chosen as NVDA chooses it (`Window._get_parent`): its UIA element when
/// the outpost judges it UIA and `uia` is given, else its MSAA client, each
/// walked up by its backend; a window whose MSAA client cannot be acquired
/// is a plain window too, and the chain goes on from its parent window, as
/// NVDA's object for it is then its `Window`. The desktop ends it.
///
/// # Errors
///
/// [`FocusLost`] when a window read through UIA has an element that
/// cannot be read, whose exception ends NVDA's event.
fn window_onward(
    context: &RequestContext<'_>,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    hwnd: isize,
    (previous, candidates): (&[KnownAncestor], Option<&Candidates>),
) -> Result<Vec<NodeSnapshot>, FocusLost> {
    let started = std::time::Instant::now();
    let desktop = desktop_window();
    // The plain windows passed on the way, nearest first.
    let mut passed = Vec::new();
    let mut current = hwnd;
    let mut above = Vec::new();
    while current != 0 && current != desktop {
        match reach(context, current) {
            Reach::AboveBoundary => {
                above = plain_windows_from(context, current);
                break;
            }
            Reach::Own => {
                if let Some(chain) =
                    window_object_onward(context, uia, current, (previous, candidates))?
                {
                    above = chain;
                    break;
                }
            }
            Reach::Plain => {}
        }
        passed.push(plain_window(context, current));
        current = parent(current);
    }
    tracing::debug!(
        hwnd,
        plain = passed.len(),
        elapsed_us = started.elapsed().as_micros(),
        "a chain went on from a window"
    );
    Ok(above.into_iter().chain(passed.into_iter().rev()).collect())
}

/// The chain from window `hwnd`'s own object up, outermost first, the
/// object included when it is focus context, for [`window_onward`]: its UIA
/// element and the UIA walk above it, when this outpost judges the window
/// UIA and `uia` is given, else its MSAA client and the MSAA walk above
/// it. `None` when the MSAA client cannot be acquired.
///
/// # Errors
///
/// [`FocusLost`] when the window's UIA element cannot be read.
fn window_object_onward(
    context: &RequestContext<'_>,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    hwnd: isize,
    (previous, candidates): (&[KnownAncestor], Option<&Candidates>),
) -> Result<Option<Vec<NodeSnapshot>>, FocusLost> {
    let known = |id: NodeId| knows(previous, id);
    if let Some((uia, request)) = uia
        && window_uses_uia(context, hwnd)
    {
        let element = uia.window_element(hwnd, request).map_err(|error| {
            tracing::debug!(hwnd, %error, "a UIA window's element could not be read; the focus is lost");
            FocusLost
        })?;
        // The window's own element, the UIA object NVDA switches to there
        // (`correctAPIForRelation`, `NVDAObjects/window/__init__.py` 319),
        // is reported as every UIA ancestor is, whatever it is, for Core
        // to judge.
        let top = element.ancestor_snapshot(context.nodes());
        if known(top.id) {
            return Ok(Some(vec![top]));
        }
        let below = vec![top];
        // The raw view's chain this walk steps through, from the window's
        // element, for the routes to the nodes it mints.
        let walked = RefCell::new(vec![(element.runtime_id(), hwnd)]);
        let visited = |runtime_id: &[i32], window: isize| {
            walked.borrow_mut().push((runtime_id.to_vec(), window));
        };
        let above = chain(
            context,
            ChainStart::Uia {
                uia: (uia, request),
                element: (element.live(), hwnd, hwnd),
                parent_automation_id: &mut None,
                visited: Some(&visited),
            },
            (previous, None),
        )?;
        let chain: Vec<NodeSnapshot> = above.into_iter().chain(below).collect();
        super::completion::keep_read_routes(context, &chain, &walked.into_inner());
        return Ok(Some(chain));
    }
    // The client area, as NVDA reads a parent window through MSAA.
    let Some(client) = verbatim_ia2::acquire::snapshot_from_event(
        hwnd,
        OBJID_CLIENT.0,
        CHILDID_SELF,
        context.nodes(),
        verbatim_ia2::acquire::Purpose::Context,
    ) else {
        return Ok(None);
    };
    if known(client.id) {
        return Ok(Some(vec![client]));
    }
    let above = chain(
        context,
        ChainStart::Msaa(&client, uia),
        (previous, candidates),
    )?;
    Ok(Some(
        above.into_iter().chain(std::iter::once(client)).collect(),
    ))
}

/// One raw-view ancestor of a UIA focus, as the route of a completed focus
/// read gives it: its cached parts, its own native window handle
/// (0 for an element that is not a window), and whether UIA counts it both
/// a control and content. The outpost filters what it reports from these
/// and finds its own elements by the runtime ids below a window
/// (`books/dev/src/reference/adr/0001-focus.md`, "The outpost's own element").
#[derive(Clone, Debug, PartialEq)]
pub(super) struct UiaAncestorFact {
    /// The ancestor's cached parts.
    pub(super) snapshot: UiaSnapshotFact,
    /// Its own native window handle, or 0.
    pub(super) hwnd: isize,
    /// Whether it is both a control and content element.
    pub(super) content: bool,
}

/// Where the chain above a UIA focus read's ancestors goes on, as the
/// listener's walk ended (`books/dev/src/reference/adr/0001-focus.md`, "Object identity"),
/// taken from the route's end (`RouteEnd`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum AncestorsEnd {
    /// The ancestors reach the desktop root, the process boundary, or a
    /// known ancestor, and the rest as read before: the outpost goes on
    /// above the boundary, or from a window it reads through MSAA.
    #[default]
    Top,
    /// A step to a parent failed or found none below the root: the chain
    /// goes on from this window's own object, the parent window of the
    /// outermost ancestor's nearest window, as NVDA's parent read falls
    /// back to the window hierarchy (`Window._get_parent`); 0 for none. Or
    /// a step reached this window, a ghost's or a hung application's,
    /// other than the nearest window: the chain goes on from it, which the
    /// outpost reads as a plain window, as NVDA does after that one step.
    ContinueFrom(isize),
    /// The walk was cut by the read's budget or its depth limit: the chain
    /// is completed by plain windows from this window, included: the
    /// outermost ancestor's nearest window, or its parent window when that
    /// ancestor is the window's own element
    /// ([`verbatim_win32::provider::plain_windows_start`]).
    PlainFrom(isize),
}

/// A UIA focus's ancestors, outermost first, from `ancestors`, the
/// raw-view chain the focus listener read from the event's own element,
/// nearest first, `nearest`, the focus's nearest window the listener
/// found, and `end`, how its walk ended: filtered as a walk here
/// filters them, stopping at an ancestor of `previous`, those
/// Core knows, and completed above as [`complete`] says: from a window read
/// through MSAA that the chain reaches, from the window above the process
/// boundary, where the listener's walk stops, from the window the listener
/// named after a failed step, the parent window of the outermost entry's
/// nearest window, or by plain windows from the window it named after a
/// walk cut at its depth limit. Each node is minted from its runtime id,
/// with no element: the outpost finds its own copy of an element only when
/// it needs one (`super::focus_path`).
///
/// # Errors
///
/// [`FocusLost`], from [`window_onward`].
pub(super) fn uia_chain_from_read(
    context: &RequestContext<'_>,
    client: &mut Client,
    (ancestors, own_window, nearest, end): (&[UiaAncestorFact], isize, Option<isize>, AncestorsEnd),
    previous: &[KnownAncestor],
) -> Result<Vec<NodeSnapshot>, FocusLost> {
    let read_by_msaa = |hwnd: isize| uia_walk_leaves(context, nearest.unwrap_or(0), hwnd);
    let known = |id: NodeId| knows(previous, id);
    let parents: Vec<verbatim_uia::AncestorParts> =
        ancestors.iter().map(super::completion::parts).collect();
    let (chain, crossed, walked) = Uia::ancestor_chain_from_parts(
        &parents,
        context.nodes(),
        &verbatim_uia::AncestorStops {
            read_by_other_api: &read_by_msaa,
            known: &known,
            out_of_time: None,
            visited: None,
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
    // Without a client the continuation reads every window it reaches
    // through MSAA or as a plain window.
    let request = client
        .uia_with(context, RequestContext::ancestor_request)
        .ok();
    complete(
        context,
        request.as_ref().map(|(uia, request)| (*uia, request)),
        (chain, above),
        (previous, None),
    )
}

/// The UIA nodes of `previous`, a previous chain, by runtime id, for a
/// walk to stop at, and their node ids, in the same order, for splicing.
pub(super) fn known_uia_nodes(
    context: &Context,
    previous: &[KnownAncestor],
) -> (Vec<NodeId>, Vec<Vec<i32>>) {
    previous
        .iter()
        .filter_map(|known| {
            let KnownAncestor::Node(id) = known else {
                return None;
            };
            let runtime_id = context.store.runtime_id_of(*id)?;
            Some((*id, runtime_id))
        })
        .unzip()
}

/// Whether a window above `window`, as UIA's raw view goes up
/// ([`verbatim_win32::provider::raw_view_parent_window`]), below the process boundary,
/// cannot be read: a ghost's or a hung application's
/// ([`Context::window_unreadable`]). Local calls, a few per window.
pub(super) fn plain_window_above(context: &Context, window: isize) -> bool {
    let desktop = desktop_window();
    let mut current = window;
    for _ in 0..MAX_ANCESTOR_HOPS {
        let Some(above) = verbatim_win32::provider::raw_view_parent_window(current) else {
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

/// The MSAA half of [`chain`]: the MSAA executor's walk above `node`,
/// its window objects above controls left out, chained above where it
/// ends.
///
/// # Errors
///
/// [`FocusLost`], from [`window_onward`].
fn msaa_chain(
    context: &RequestContext<'_>,
    uia: Option<(&Uia, &Request<Ancestor>)>,
    node: &NodeSnapshot,
    (previous, candidates): (&[KnownAncestor], Option<&Candidates>),
) -> Result<Vec<NodeSnapshot>, FocusLost> {
    let known = |id: NodeId| knows(previous, id);
    // Another process's window is a plain window, and is not probed.
    let read_by_uia = |hwnd: isize| uia.is_some() && own_window_uses_uia(context, hwnd);
    let plain = |hwnd: isize| reads_as_plain(context, hwnd);
    let limits = verbatim_ia2::acquire::AncestorLimits {
        max_hops: MAX_ANCESTOR_HOPS,
        known: &known,
        read_by_other_api: &read_by_uia,
        plain: &plain,
        candidates,
    };
    let (chain, walked) = match verbatim_ia2::batch::ancestors(node.id, context.nodes(), &limits) {
        Ok(walk) => walk,
        // The node can no longer be reached: its chain goes on from its
        // window's parent, read live.
        Err(error) => {
            tracing::debug!(?error, "the MSAA ancestors could not be read");
            let own = node.details.window.map_or(0, super::window::hwnd_of);
            let nearest = context.store.key_of(node.id).map_or(0, |(hwnd, _, _)| hwnd);
            return onward_after_failure(context, uia, (own, nearest), (previous, candidates));
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
    complete(context, uia, (chain, above), (previous, candidates))
}

/// An MSAA node's ancestors, stopping at an ancestor of `previous`, those
/// Core knows, and completed above as [`chain`] says: an MSAA
/// call has no bound of its own, and a UIA call in the walk ends after
/// UIA's call timeout.
///
/// # Errors
///
/// [`FocusLost`], from [`chain`].
pub(super) fn msaa_enrichment(
    context: &RequestContext<'_>,
    client: &mut Client,
    (node, previous, candidates): (&NodeSnapshot, &[KnownAncestor], Option<&Candidates>),
) -> Result<Vec<NodeSnapshot>, FocusLost> {
    // A window object above a control is layout, never announced as an
    // entered container: NVDA gives a window object reached through its
    // parents the `GenericWindow` class, which is not a presentable focus
    // ancestor. A dialog is still announced, by its client area's dialog
    // role.
    let request = client
        .uia_with(context, RequestContext::ancestor_request)
        .ok();
    chain(
        context,
        ChainStart::Msaa(node, request.as_ref().map(|(uia, request)| (*uia, request))),
        (previous, candidates),
    )
}

/// Whether `node` is a dialog whose own text is to be gathered as its
/// description: a dialog with no description of its own, while
/// descriptions are read at all.
pub(super) fn wants_dialog_text(context: &RequestContext<'_>, node: &NodeSnapshot) -> bool {
    context
        .node_details()
        .contains(verbatim_model::property::NodeDetail::Description)
        && is_dialog(node.role)
        && node
            .details
            .description
            .as_deref()
            .is_none_or(|description| description.trim().is_empty())
}

/// The own text of the dialog `node`, through the backend that reported it,
/// a UIA dialog's on `client`, found by `path`, its report's listener path,
/// when this outpost holds no element for it; `None` when it has none or
/// cannot be read.
pub(super) fn dialog_text_of(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: &NodeSnapshot,
    path: Option<&super::focus_path::FocusPath>,
) -> Option<String> {
    use verbatim_reader_rules::dialog::dialog_text;
    use verbatim_uia_rops::dialog::UiaDialog;
    match node.backend {
        Backend::Msaa => verbatim_ia2::dialog::DialogObject::of_node(node.id, context.nodes())
            .and_then(|dialog| dialog_text(&dialog)),
        Backend::Uia => {
            // A dialog the focus listener read in the focus's chain, whose
            // element this outpost does not hold, is found by its report's
            // path.
            let element = context.store.element_of(node.id);
            let route = if element.is_some() {
                None
            } else {
                let runtime_id = context.store.runtime_id_of(node.id)?;
                Some(context.route_in(path?, &runtime_id)?)
            };
            let remote = route
                .as_ref()
                .is_some_and(|route| context.tries_remote(Some(route.window.get())));
            let gates = crate::gates::node_gates(context.node_details());
            let uia = client.uia()?;
            // Nothing of the dialog's own element is read: its text is
            // gathered from its children.
            let dialog = match (element, route) {
                (Some(element), _) => element.resolve().ok()?.live().clone(),
                (None, Some(route)) => {
                    let nodes = uia.request::<Node>(gates).ok()?;
                    let found = super::focus_path::find(uia, &route, &nodes, remote)
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
        }
    }
}

/// The UIA element of this application's focus, as the focus read ([`checked_focused_element`])
/// reads it, with its checks and its process-id check, filled as a node and
/// nothing more read: for the focus whose element no route finds (`books/dev/src/reference/adr/0001-focus.md`, "The outpost's own
/// element"). `Refused` also when the outpost judges the focus
/// window MSAA.
pub(super) fn focused_element_in(
    context: &RequestContext<'_>,
    client: &mut Client,
    trigger: FocusRead,
) -> Result<Cached<Node>, FocusReadMiss> {
    let hwnd = checked_focus_window(context)?;
    if !window_uses_uia(context, hwnd) {
        return Err(FocusReadMiss::Refused);
    }
    let (uia, nodes) = client
        .uia_with(context, RequestContext::node_request)
        .map_err(|_| FocusReadMiss::Failed(None))?;
    checked_focused_element(context, (uia, &nodes), hwnd, trigger)
}

/// The window with the keyboard focus whose focused element
/// the focus read ([`checked_focused_element`]) may read: this application's, not on Core's
/// hidden frame, and readable. Local calls.
pub(super) fn checked_focus_window(context: &Context) -> Result<isize, FocusReadMiss> {
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
pub(super) fn checked_focused_element(
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

/// Why the focus read ([`checked_focused_element`]) answered no focus.
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

/// What asked the focus read ([`checked_focused_element`]) to read the application's focus,
/// logged with each read, so a run's log shows how often each makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FocusRead {
    /// Core's request for the current focus.
    FocusNow,
    /// A focus whose element no route found ([`focused_element_in`]).
    NoRoute,
}

/// The window whose focused element the focus read ([`checked_focused_element`]) reads for the
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

/// [`foreground_object`], taken with no call from `chain`, a focus's
/// ancestors just read, when one of them is the foreground window's own
/// UI Automation element: the same element the window's read would find.
pub(super) fn foreground_object_among(
    context: &RequestContext<'_>,
    client: &mut Client,
    chain: &[NodeSnapshot],
) -> Option<NodeSnapshot> {
    let hwnd = context.foreground_window();
    if hwnd == 0 || !served_by(hwnd, context.target_pid()) {
        return None;
    }
    if let Some(own) = chain.iter().find(|node| {
        node.backend == Backend::Uia
            && node.details.window.map(super::window::hwnd_of) == Some(hwnd)
    }) {
        let mut node = own.clone();
        if node
            .name
            .as_deref()
            .is_none_or(|name| name.trim().is_empty())
        {
            node.name = window_text(hwnd);
        }
        return Some(node);
    }
    Some(
        if window_exists(hwnd) && context.window_unreadable(hwnd).is_some() {
            plain_window(context, hwnd)
        } else {
            foreground_window(context, client, hwnd).1
        },
    )
}

/// Resolves a UIA node to a live element filled by `nodes`: the registry's
/// kept element first (refreshing its cache both updates it and proves it
/// still answers), then, if that element is gone or none is kept, the route
/// to it through the chain the focus listener read for the focus
/// ([`super::focus_path`]). A node found by neither is gone, as NVDA's object
/// for a gone element is: no search is made for it, since a search of the
/// application's windows enters any child window another process serves
/// (`books/dev/src/reference/adr/0005-calls-that-reach-another-application.md`, "A node whose element is gone"). A kept element
/// that fails for another reason, such as a busy provider timing out, is
/// kept and the read fails: it may well still be alive. So does a route's
/// walk that fails rather than finishing without the element
/// ([`super::focus_path::find`]).
pub(super) fn resolve_uia_element(
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
    let remote = context.tries_remote(Some(route.window.get()));
    let (found, import_failed) = super::focus_path::find(uia, &route, nodes, remote);
    if import_failed {
        context.read_classically(route.window.get());
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
pub(super) fn uia_node<'a>(
    context: &RequestContext<'_>,
    client: &'a mut Client,
    node_id: NodeId,
) -> Result<(&'a Uia, Request<Node>, Cached<Node>), ReadError> {
    let (uia, nodes) = client.uia_with(context, RequestContext::node_request)?;
    let element = resolve_uia_element(context, uia, &nodes, node_id)?;
    Ok((uia, nodes, element))
}

/// The UI Automation element of node `node_id`, for a registration: the
/// one the outpost keeps, else found again by the route to it, as an
/// ancestor the completion minted from the listener's read is. `None` for
/// an MSAA node or one whose element is gone.
pub(super) fn element_reference(
    context: &RequestContext<'_>,
    uia: &Uia,
    node_id: NodeId,
) -> Option<windows::core::AgileReference<windows::Win32::UI::Accessibility::IUIAutomationElement>>
{
    if let Some(kept) = context.store.element_of(node_id) {
        return Some(kept.reference());
    }
    context.store.runtime_id_of(node_id)?;
    let nodes = context.read_node_request(uia).ok()?;
    let element = resolve_uia_element(context, uia, &nodes, node_id).ok()?;
    windows::core::AgileReference::new(element.live().element()).ok()
}

/// The window handle the kept element of the UIA node `node_id` says it is,
/// `None` for a windowless element or one no longer kept. No call.
pub(super) fn kept_own_window(context: &RequestContext<'_>, node_id: NodeId) -> Option<isize> {
    context
        .store
        .element_of(node_id)
        .and_then(|agile| agile.resolve().ok())
        .map(|element| element.read().hwnd)
        .filter(|&hwnd| hwnd != 0)
}

/// The nearest window of the dead UIA element `node_id`, for NVDA's falling
/// back to the parent of the element's window: the one recorded when its
/// node was built, as NVDA's object fixes its window when it is made
/// (`NVDAObjects/UIA/__init__.py` 1604 to 1607); else, for a window's own
/// element reached no other way, the handle its kept element or its
/// runtime id holds. Before any step, since the step lets a gone element
/// go.
fn known_window_of(context: &RequestContext<'_>, node_id: NodeId) -> Option<isize> {
    context
        .store
        .nearest_window_of(node_id)
        .or_else(|| kept_own_window(context, node_id))
        .or_else(|| match context.store.runtime_id_of(node_id).as_deref() {
            Some([42, hwnd]) => isize::try_from(*hwnd).ok(),
            _ => None,
        })
}

/// The nearest window of `neighbor`, reached by a step of `kind` from a node
/// whose nearest window is `from_window` and which is itself a window when
/// `from_is_window`, from what the step read, with no call: its own window;
/// else inside the node, or beside or above a windowless node, the same
/// window as the node's, as NVDA reuses it (`NVDAObjects/UIA/__init__.py`
/// 2146 to 2153); else, beside or above a window, that window's parent.
fn neighbor_nearest_window(
    neighbor: &NodeSnapshot,
    kind: QueryKind,
    from_window: Option<isize>,
    from_is_window: bool,
) -> Option<isize> {
    if let Some(own) = neighbor.details.window {
        return Some(super::window::hwnd_of(own));
    }
    if kind == QueryKind::FirstChild || !from_is_window {
        return from_window;
    }
    from_window
        .map(verbatim_win32::window::parent)
        .filter(|&parent| parent != 0)
}

/// The parent of the dead element's window `window`, NVDA's fall back when
/// its walker fails (`NVDAObjects/UIA/__init__.py` 2136 to 2145,
/// `NVDAObjects/window/__init__.py` 287 to 293); gone when the window is
/// too.
fn parent_of_window(
    context: &RequestContext<'_>,
    client: &mut Client,
    window: Option<isize>,
) -> Result<Option<NodeSnapshot>, ReadError> {
    let Some(window) = window.filter(|&window| verbatim_win32::window::window_exists(window))
    else {
        return Err(ReadError::Gone);
    };
    let parent = verbatim_win32::window::parent(window);
    if parent == 0 {
        return Ok(None);
    }
    window_object(context, client, parent).map(Some)
}

/// The object NVDA's `Window._get_parent` makes of window `hwnd`
/// (`findBestAPIClass` with the relation "parent",
/// `NVDAObjects/window/__init__.py` 84 to 98 and 291 to 293): the desktop
/// and a ghost's or a hung application's window stay plain windows; else,
/// by the outpost's verdict, its UIA element or its MSAA client, each with
/// its own role, a window whose MSAA client cannot be acquired being a
/// plain window, as NVDA's object for it is then its `Window`. Another
/// process's window is a stand-in read from Windows' own state, since the
/// outpost makes no connection to it
/// (`books/dev/src/explanation/design/listener-without-connections.md`,
/// section 4.3). The desktop is left for Core to name.
///
/// # Errors
///
/// [`ReadError::Failed`] when a UIA window's element cannot be read, whose
/// exception ends NVDA's command.
fn window_object(
    context: &RequestContext<'_>,
    client: &mut Client,
    hwnd: isize,
) -> Result<NodeSnapshot, ReadError> {
    if hwnd == desktop_window() {
        let mut node = plain_window(context, hwnd);
        node.details.desktop_window = true;
        return Ok(node);
    }
    match reach(context, hwnd) {
        Reach::AboveBoundary => {
            let mut node = stand_in(context, hwnd);
            node.details.above_boundary = true;
            return Ok(node);
        }
        Reach::Plain => return Ok(plain_window(context, hwnd)),
        Reach::Own => {}
    }
    if window_uses_uia(context, hwnd) {
        let (uia, nodes) = client.uia_with(context, RequestContext::node_request)?;
        let element = uia
            .window_element(hwnd, &nodes)
            .map_err(|error| ReadError::Failed(format!("the window's element: {error}")))?;
        let node = element.snapshot(context.nodes());
        context.store.record_nearest_window(node.id, hwnd);
        return Ok(node);
    }
    Ok(verbatim_ia2::acquire::snapshot_from_event(
        hwnd,
        OBJID_CLIENT.0,
        CHILDID_SELF,
        context.nodes(),
        verbatim_ia2::acquire::Purpose::Announce,
    )
    .unwrap_or_else(|| plain_window(context, hwnd)))
}

/// One object-navigation step from a UIA node: the neighbor, or `None` for
/// a genuine tree edge, and the node's nearest window. The step is taken
/// from the element the registry keeps, without first refreshing it; when
/// that element is gone, or the registry keeps none, from the element the
/// focus's route finds, or the node is gone ([`resolve_uia_element`]).
fn uia_step(
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
    relation: verbatim_model::batch::Relation,
) -> Result<Stepped, ReadError> {
    if let Some(element) = context
        .store
        .element_of(node_id)
        .and_then(|agile| agile.resolve().ok())
    {
        let (uia, nodes) = client.uia_with(context, RequestContext::node_request)?;
        let from = (
            element.live().element(),
            element.read().hwnd,
            element.served_only_by_uia_proxies() == Some(true),
        );
        if let Some(stepped) = related_step(context, uia, &nodes, (node_id, from, relation))? {
            return Ok(stepped);
        }
        context.store.evict_element(node_id);
    }
    let (uia, nodes, element) = uia_node(context, client, node_id)?;
    let from = (
        element.live().element(),
        element.read().role.hwnd,
        element.served_only_by_uia_proxies() == Some(true),
    );
    related_step(context, uia, &nodes, (node_id, from, relation))?.ok_or(ReadError::Gone)
}

/// The matchers a walk down the tree judges each node it reads by.
pub(super) type Judge<'a> =
    &'a dyn Fn(&verbatim_uia::Cached<Node>) -> verbatim_uia_rops::batch::Judged;

/// The one element a batch's object step goes from, as the program
/// builder resolves it ([`verbatim_uia_rops::batch::Resolve`]): its own
/// window, whether the step runs by calls whatever the mode, how each node
/// it reaches is filled, and for a walk down the tree the matchers that
/// judge each node.
pub(super) struct OneElement<'a> {
    pub(super) element: &'a windows::Win32::UI::Accessibility::IUIAutomationElement,
    pub(super) window: isize,
    pub(super) calls_only: bool,
    pub(super) fill: &'a Fill<Node>,
    pub(super) judge: Option<Judge<'a>>,
}

impl verbatim_uia_rops::batch::Resolve for OneElement<'_> {
    fn object(
        &self,
        _subject: &verbatim_model::batch::Subject,
    ) -> Result<verbatim_uia_rops::batch::Element<'_>, verbatim_uia_rops::batch::Ran> {
        Ok(verbatim_uia_rops::batch::Element {
            element: self.element,
            window: self.window,
            calls_only: self.calls_only,
        })
    }

    fn fill(&self, _place: verbatim_model::batch::Place) -> Option<&Fill<Node>> {
        Some(self.fill)
    }

    fn judge(
        &self,
        _place: verbatim_model::batch::Place,
        node: &verbatim_uia::Cached<Node>,
    ) -> verbatim_uia_rops::batch::Judged {
        if let Some(judge) = self.judge {
            return judge(node);
        }
        tracing::error!(target: verbatim_model::DEFECT_TARGET, "a walk from an element given no matchers");
        verbatim_uia_rops::batch::Judged {
            matched: false,
            pruned: true,
        }
    }
}

impl OneElement<'_> {
    /// Runs `op`, a batch's object step from this element, alone, through
    /// the program builder, in a window of the application whose programs
    /// `window` says may run.
    pub(super) fn run(
        &self,
        context: &RequestContext<'_>,
        uia: &Uia,
        op: verbatim_model::batch::Op,
        window: Option<isize>,
    ) -> verbatim_uia_rops::batch::Answer {
        let steps = [verbatim_model::batch::Step { op, when: None }];
        verbatim_uia_rops::batch::run_batch(&verbatim_uia_rops::batch::Segment {
            uia,
            steps: &steps,
            first: 0,
            earlier: &[],
            resolver: self,
            remote: context.tries_remote(window),
            window,
        })
    }
}

/// One navigation step `relation` through the program builder
/// (`verbatim_uia_rops::batch`, `Related`) from `element`, node `node_id`'s, whose own
/// window is `own`, 0 for none, each node reached filled as `nodes` asks:
/// `None` when the element is gone. A step from a top-level window, whose
/// relations a program's walk does not reach, or from an element only UI
/// Automation's proxies inside this process serve (`proxied`), which no
/// program can import, is taken by calls.
fn related_step(
    context: &RequestContext<'_>,
    uia: &Uia,
    nodes: &Request<Node>,
    (node_id, (element, own, proxied), relation): (
        NodeId,
        (
            &windows::Win32::UI::Accessibility::IUIAutomationElement,
            isize,
            bool,
        ),
        verbatim_model::batch::Relation,
    ),
) -> Result<Option<Stepped>, ReadError> {
    use verbatim_model::batch::Op;
    use verbatim_uia_rops::batch::{Found, Ran};
    let window = (own != 0).then_some(own);
    let fill = Fill::new(nodes);
    let from = OneElement {
        element,
        window: own,
        calls_only: proxied || (own != 0 && top_level_of(own) == own),
        fill: &fill,
        judge: None,
    };
    let op = Op::Related {
        subject: verbatim_model::batch::Subject::Node(node_id),
        relation,
        set: Vec::new(),
    };
    let answer = from.run(context, uia, op, window);
    let related = match answer.steps.into_iter().next() {
        Some(Ran::Done(Found::Related(related))) => related,
        Some(Ran::Gone) => return Ok(None),
        other => {
            return Err(ReadError::Failed(format!(
                "UIA navigation failed: {other:?}"
            )));
        }
    };
    if answer.read_classically
        && let Some(hwnd) = related.window.or(window)
    {
        context.read_classically(top_level_of(hwnd));
    }
    let neighbor = related.nodes.into_iter().next().map(|neighbor| {
        let mut neighbor = neighbor.snapshot(context.nodes());
        neighbor.details.set_table(related.table);
        neighbor
    });
    Ok(Some((neighbor, related.window)))
}

/// One object-navigation step: the neighbor, or `None` for a genuine tree
/// edge, never conflated with a node that is gone.
pub(super) fn navigate(
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
    kind: QueryKind,
) -> Result<Option<NodeSnapshot>, ReadError> {
    if kind != QueryKind::Parent || context.store.runtime_id_of(node_id).is_none() {
        return step(context, client, node_id, kind);
    }
    let window = known_window_of(context, node_id);
    match step(context, client, node_id, kind) {
        Err(ReadError::Gone) => parent_of_window(context, client, window),
        answered => answered,
    }
}

/// One navigation step, with no falling back to a dead element's window:
/// the neighbor, or `None` for a genuine tree edge, never conflated with a
/// node that is gone.
fn step(
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
    kind: QueryKind,
) -> Result<Option<NodeSnapshot>, ReadError> {
    use verbatim_model::batch::Relation;
    if context.store.runtime_id_of(node_id).is_some() {
        let relation = match kind {
            QueryKind::Parent => Relation::Parent,
            QueryKind::NextSibling => Relation::NextSibling,
            QueryKind::PreviousSibling => Relation::PreviousSibling,
            QueryKind::FirstChild => Relation::FirstChild,
        };
        return uia_neighbor(context, client, node_id, (relation, kind));
    }
    super::msaa_object::navigate(context, client, node_id, kind)
}

/// The last child of the UI Automation node `node_id`, in one step, taken
/// as a step to the first child is: the child, or `None` for a node with
/// none, never conflated with a node that is gone.
pub(super) fn last_child(
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
) -> Result<Option<NodeSnapshot>, ReadError> {
    uia_neighbor(
        context,
        client,
        node_id,
        (
            verbatim_model::batch::Relation::LastChild,
            QueryKind::FirstChild,
        ),
    )
}

/// One step by `relation` from the UI Automation node `node_id`, taken as
/// a step of `kind` is: the neighbor's nearest window recorded, a menu
/// item's legacy checked state read, and its backend corrected.
fn uia_neighbor(
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
    (relation, kind): (verbatim_model::batch::Relation, QueryKind),
) -> Result<Option<NodeSnapshot>, ReadError> {
    let (neighbor, from_window) = uia_step(context, client, node_id, relation)?;
    if let Some(neighbor) = &neighbor {
        let from_is_window = kept_own_window(context, node_id).is_some();
        if let Some(window) = neighbor_nearest_window(neighbor, kind, from_window, from_is_window) {
            context.store.record_nearest_window(neighbor.id, window);
        }
    }
    // A menu item reached by navigation reads its legacy checked state as
    // a focused one does.
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
    Ok(neighbor.map(|neighbor| corrected_backend(context, client, from_window, neighbor, kind)))
}

/// A navigation step's neighbor, `None` at a tree edge, and the nearest
/// window of the node it was taken from.
type Stepped = (Option<NodeSnapshot>, Option<isize>);

/// `neighbor` through the backend its window uses, as NVDA corrects the API
/// of an object reached by navigation (`correctAPIForRelation`): an MSAA
/// object in a different window from `from_window`, whose window is UIA,
/// becomes that window's UIA element; a UIA element that is the root of a
/// different window, whose window is MSAA, becomes that window's MSAA
/// object (its client area when reached as a parent, its window object
/// otherwise). Anything else, or a read that fails, is kept as it is.
pub(super) fn corrected_backend(
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
    // connection to it (`books/dev/src/explanation/design/listener-without-connections.md`,
    // section 4.3).
    if window == 0 || window == from_window || !own_window_uses_uia(context, window) {
        return neighbor;
    }
    client
        .uia_with(context, RequestContext::node_request)
        .ok()
        .and_then(|(uia, nodes)| uia.window_element(window, &nodes).ok())
        .map_or(neighbor, |element| element.snapshot(context.nodes()))
}

/// How many parents activation tries when the node itself has no action:
/// a safety bound against a tree with no identity that loops, far above
/// any real tree, where NVDA walks with no cap (Dickson, 2026-11-21;
/// `docs/design/security-model.md`, U2).
const ACTIVATION_PARENT_HOPS: u32 = 1_024;

/// What activation's walk up the parents calls, a seam for its tests.
pub(super) trait ActivationWalk {
    /// Activates exactly `node`, answering the action's name.
    fn activate(&mut self, node: NodeId) -> Result<Option<ActionName>, ReadError>;
    /// The parent of `node`, `None` when it has none or it cannot be read.
    fn parent(&mut self, node: NodeId) -> Option<NodeId>;
    /// What identifies `node`, `None` for a node with no identity.
    fn visited(&self, node: NodeId) -> Option<Identity>;
}

/// The real walk, through the request's store and client.
pub(super) struct LiveActivation<'a, 'b> {
    pub(super) context: &'a RequestContext<'b>,
    pub(super) client: &'a mut Client,
}

impl ActivationWalk for LiveActivation<'_, '_> {
    fn activate(&mut self, node: NodeId) -> Result<Option<ActionName>, ReadError> {
        activate_one(self.context, self.client, node)
    }
    fn parent(&mut self, node: NodeId) -> Option<NodeId> {
        match step(self.context, self.client, node, QueryKind::Parent) {
            Ok(Some(parent)) => Some(parent.id),
            _ => None,
        }
    }
    fn visited(&self, node: NodeId) -> Option<Identity> {
        let store = &self.context.store;
        store.runtime_id_of(node).map(Identity::Uia).or_else(|| {
            store
                .address_of(node)
                .map(|(window, object, child)| Identity::Msaa {
                    window,
                    object,
                    child,
                })
        })
    }
}

/// The dispatcher's activation of `node_id`, or failing that its nearest
/// ancestor that can be activated, as NVDA's review activate walks up the
/// navigator object's parents until one performs an action, answering the
/// action's name: the node, then up to [`ACTIVATION_PARENT_HOPS`] parents,
/// stopping at a node it has visited before, where the tree loops. Each
/// node is activated by its backend's executor, and each parent is a
/// navigation step on the backend of the node it goes from.
pub(super) fn activate_walk(
    walk: &mut impl ActivationWalk,
    node_id: NodeId,
) -> Result<Option<ActionName>, ReadError> {
    let mut current = node_id;
    let mut first_error = None;
    let mut visited = BTreeSet::new();
    for hop in 0..=ACTIVATION_PARENT_HOPS {
        if let Some(identity) = walk.visited(current)
            && !visited.insert(identity)
        {
            break;
        }
        match walk.activate(current) {
            Ok(action) => return Ok(action),
            // A node that is gone has no parent to reach: a step to it
            // would find the node, and walk its route, again.
            Err(ReadError::Gone) => return Err(first_error.unwrap_or(ReadError::Gone)),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        // The last parent within the cap is not read past.
        if hop == ACTIVATION_PARENT_HOPS {
            break;
        }
        match walk.parent(current) {
            Some(parent) => current = parent,
            None => break,
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
        let uia = client
            .uia()
            .ok_or_else(|| ReadError::Failed("could not create a UIA client".to_owned()))?;
        return super::uia_object::activate(context, uia, node_id);
    }
    super::msaa_object::activate(context, client, node_id)
}

#[cfg(test)]
mod activation_tests {
    use verbatim_model::{ActionName, NodeId};

    use super::{ACTIVATION_PARENT_HOPS, ActivationWalk, Identity, ReadError, activate_walk};

    /// A chain of parents above node 0, node `n`'s parent `n + 1`, whose
    /// node `action` alone activates; `identity` names each node's
    /// identity.
    struct Chain {
        action: u64,
        identity: fn(u64) -> Option<Identity>,
        tried: Vec<u64>,
        /// How many parents were read.
        parents: u64,
    }

    impl ActivationWalk for Chain {
        fn activate(&mut self, node: NodeId) -> Result<Option<ActionName>, ReadError> {
            self.tried.push(node.number());
            if node.number() == self.action {
                Ok(Some(ActionName::Invoke))
            } else {
                Err(ReadError::Failed("no action".to_owned()))
            }
        }
        fn parent(&mut self, node: NodeId) -> Option<NodeId> {
            self.parents += 1;
            Some(NodeId::new(node.number() + 1))
        }
        fn visited(&self, node: NodeId) -> Option<Identity> {
            (self.identity)(node.number())
        }
    }

    fn chain(action: u64, identity: fn(u64) -> Option<Identity>) -> Chain {
        Chain {
            action,
            identity,
            tried: Vec::new(),
            parents: 0,
        }
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the chain's identity, which can be none"
    )]
    fn unique(node: u64) -> Option<Identity> {
        Some(Identity::Uia(vec![42, i32::try_from(node).expect("small")]))
    }

    /// An action 12 parents up is reached and performed, as NVDA's review
    /// activate walks the parents with no cap. Source-derived, approved
    /// by Dickson on 2026-11-21 (`nvda/source/globalCommands.py` 1880 to
    /// 1901).
    #[test]
    fn an_action_12_parents_up_is_performed() {
        let mut walk = chain(12, unique);
        assert_eq!(
            activate_walk(&mut walk, NodeId::new(0)).ok(),
            Some(Some(ActionName::Invoke))
        );
        assert_eq!(walk.tried, (0..=12).collect::<Vec<_>>());
    }

    /// An action one parent past the cap of 1,024 is not reached: the walk
    /// tries the node and 1,024 parents, reading no parent past them, then
    /// answers its first failure (Dickson, 2026-11-21).
    #[test]
    fn an_action_past_the_cap_is_not_reached() {
        let hops = u64::from(ACTIVATION_PARENT_HOPS);
        let mut walk = chain(hops + 1, unique);
        assert!(matches!(
            activate_walk(&mut walk, NodeId::new(0)),
            Err(ReadError::Failed(reason)) if reason == "no action"
        ));
        assert_eq!(walk.tried.last(), Some(&hops));
        assert_eq!(walk.tried.len(), usize::try_from(hops + 1).expect("small"));
        assert_eq!(walk.parents, hops);
    }

    /// A parent met again, a tree that loops every three nodes, ends the
    /// walk at the repeated node, before the action above it.
    #[test]
    fn a_repeated_parent_ends_the_walk() {
        let mut walk = chain(5, |node| {
            Some(Identity::Msaa {
                window: 7,
                object: -4,
                child: i32::try_from(node % 3).expect("small"),
            })
        });
        assert!(activate_walk(&mut walk, NodeId::new(0)).is_err());
        assert_eq!(walk.tried, [0, 1, 2]);
    }

    /// Nodes with no identity are bounded by the cap alone: the walk
    /// tries the node and every parent up to the cap.
    #[test]
    fn nodes_with_no_identity_are_walked_up_to_the_cap() {
        let hops = u64::from(ACTIVATION_PARENT_HOPS);
        let mut walk = chain(hops + 1, |_| None);
        assert!(activate_walk(&mut walk, NodeId::new(0)).is_err());
        assert_eq!(walk.tried, (0..=hops).collect::<Vec<_>>());
        assert_eq!(walk.parents, hops);
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{Backend, NodeDetails, NodeId, NodeSnapshot, Role, StateSet};

    use super::{CONSOLE_WINDOW_CLASS, console_text_area_in, control_window};
    use verbatim_win32::window::window_owner;

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

    /// The console host's own provider for its window supplies no
    /// `IsEnabled`; UI Automation's proxy for the window answers true. The
    /// remote states read, whose cache is filled in the console host's
    /// process, leaves the property out, so the window reads as enabled
    /// both ways rather than disabled, as NVDA reads it (found 2026-11-19,
    /// a new console window said "window unavailable").
    #[test]
    fn a_console_window_reads_enabled_remotely_and_classically() {
        use verbatim_model::State;
        use verbatim_uia::{DialogText, ElementExt as _, Fill, Node, Shape as _, Uia};
        use verbatim_uia_rops::{StatesQuery, focus_states_classic, focus_states_remote};
        use windows::Win32::UI::Accessibility::UIA_IsEnabledPropertyId;
        verbatim_test_console::Console::with(|console| {
            let uia = Uia::new().expect("a UIA client");
            let live = uia
                .window_live(console.window)
                .expect("the console window's element");
            assert_eq!(
                live.element().current_bool(UIA_IsEnabledPropertyId),
                Ok(true),
                "the ordinary read"
            );
            let fill = Fill::new(&uia.request::<Node>(Node::all_on()).expect("a node request"));
            let children = Fill::new(
                &uia.request_fixed::<DialogText>()
                    .expect("a dialog text request"),
            );
            let query = StatesQuery {
                focus: &live,
                fill: &fill,
                dialogs: &[],
                children: &children,
            };
            let read = |states: verbatim_uia_rops::States| {
                let states = states.focus.expect("the window's states").read().states;
                let disabled = states.states(Role::Window, false).contains(State::Disabled);
                (states.enabled, disabled)
            };
            let remote = read(focus_states_remote(&uia, &query).expect("the remote read"));
            let classic = read(focus_states_classic(&uia, &query).expect("the classic read"));
            assert_eq!(remote, (None, false), "the remote read");
            assert_eq!(classic, (Some(true), false), "the classic read");
        });
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
