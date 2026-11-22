//! The completion of the listener's read of a UI Automation element from
//! its route (`docs/design/readers-tell-core-asks-build1.md`, "The outpost
//! in this build" and "The focus answers"): the outpost finds its own copy
//! of the element by the route, mints node ids from the runtime ids the
//! listener read, replaces the element kept under a reused runtime id,
//! adds a focus's event registration on the registering thread and waits
//! for it before it reads the node's states (Dickson's ruling), and
//! completes the chain above the route: through MSAA from a window it
//! reads that way, and above the process boundary by plain windows.
//!
//! Where the listener's walk ended at an ancestor Core already knows, the
//! route's end names it, so the completion makes no read to find it.

#![forbid(unsafe_code)]

use verbatim_model::property::PropertyValues;
use verbatim_model::protocol::UiaSnapshotFact;

use super::read::UiaAncestorFact;
use verbatim_model::batch::{Missing, StepOutcome, StepRefusal};
use verbatim_model::property::PropertyId;
use verbatim_model::request::{KnownAncestor, Route, RouteEnd};
use verbatim_model::{Backend, NodeId, NodeSnapshot, State};
use verbatim_uia::{Cached, Interning as _, Kept, Node};

use super::RequestContext;
use super::asked::{ancestors_end, snapshot_of};
use super::focus_path;
use super::read::{self, Client};
use super::reducer::FocusPath;
use super::text_reads;
use super::window::hwnd_of;

/// Why a read of a route answered nothing.
pub(super) enum Unread {
    /// The element is not there.
    NotThere,
    /// Core named an ancestor the route's end does not, a defect.
    Unsupported,
}

impl Unread {
    /// The step's outcome it is.
    pub(super) fn outcome(self) -> StepOutcome {
        match self {
            Self::NotThere => StepOutcome::NotThere(Missing::Object),
            Self::Unsupported => StepOutcome::Refused(StepRefusal::Unsupported),
        }
    }
}

/// The outpost's copy of an element a route names, minted, before what a
/// read of it adds.
struct Minted {
    /// The element found by the route, `None` when none was.
    found: Option<Cached<Node>>,
    /// The route's window's element read to find it.
    window_element: Option<Cached<Node>>,
    /// Its node.
    node: NodeId,
    /// What the listener read of it, as its node.
    snapshot: NodeSnapshot,
    /// The route as the outpost finds elements by it.
    path: FocusPath,
    /// The route's window.
    nearest: isize,
    /// Whether its runtime id named another element before.
    reused: bool,
}

/// The element `route` names, found and minted, a focus's (`focus`)
/// registered for its events; `None` for a route with no element.
fn mint(
    context: &RequestContext<'_>,
    client: &mut Client,
    route: &Route,
    focus: bool,
) -> Option<Minted> {
    let (first, first_read) = (route.steps.first()?, route.read.first()?);
    let path = FocusPath::of_route(route).above(known_route(context, &route.end));
    let runtime_id = first.runtime_id.0.clone();
    let (found, window_element) = find(context, client, &path, &runtime_id, focus);
    // A focus whose own element is not found still has the element kept
    // under its runtime id judged, so that a stale one's text is never
    // read as the focus's.
    let reused = (focus || found.is_some()) && replaced_stale(context, client, &runtime_id);
    let node = match &found {
        Some(element) => context
            .nodes()
            .id_for_element(&runtime_id, &element.narrow::<Kept>()),
        None => context.nodes().keep(&runtime_id, None),
    };
    let mut snapshot = snapshot_of(node, Backend::Uia, first_read);
    let nearest = hwnd_of(route.window);
    // A console host's text area is its terminal, as every way of reading
    // a focus makes it.
    let own_window = snapshot.details.window.map_or(nearest, hwnd_of);
    read::console_text_area(&mut snapshot, Some(own_window));
    if focus && let Some(element) = &found {
        register(context, element, node);
    }
    // The node's window, fixed as it is made, as NVDA's object fixes its
    // own (`NVDAObjects/UIA/__init__.py` 1604 to 1607), for a step from it
    // once it is dead.
    context.store.record_nearest_window(node, own_window);
    Some(Minted {
        found,
        window_element,
        node,
        snapshot,
        path,
        nearest,
        reused,
    })
}

/// The element `route` names, alone, found, minted and kept, as the
/// resolver of a `Read` of a route resolves it (a selection's item): what
/// the listener read of it, as its node, and its nearest window.
pub(super) fn resolve_route(
    context: &RequestContext<'_>,
    client: &mut Client,
    route: &Route,
) -> Option<(NodeSnapshot, isize)> {
    let minted = mint(context, client, route, false)?;
    keep_minted(
        context,
        (minted.node, &[]),
        &minted.path,
        minted.window_element.as_ref(),
    );
    Some((minted.snapshot, minted.nearest))
}

/// The outpost's own copy of the element `runtime_id` names, by the route:
/// from the nearest element at or above it with a window of its own; for a
/// focus, `focus`, that no route finds, the application's focused element
/// when its runtime id is the focus's.
fn find(
    context: &RequestContext<'_>,
    client: &mut Client,
    path: &FocusPath,
    runtime_id: &[i32],
    focus: bool,
) -> (Option<Cached<Node>>, Option<Cached<Node>>) {
    let mut window_element = None;
    if let Some(route) = path.route_to(runtime_id) {
        let Ok((uia, nodes)) = client.uia_with(context, RequestContext::node_request) else {
            return (None, None);
        };
        let remote = context.tries_remote(Some(route.window.get()));
        let (found, import_failed, read_window) =
            focus_path::find_from_window(uia, &route, &nodes, remote);
        if import_failed {
            context.read_classically(route.window.get());
        }
        if let Ok(Some(found)) = found {
            return (Some(found), read_window);
        }
        window_element = read_window;
    }
    // A focus that no route found is read as the application's focused
    // element, taken only when it is the focus's own element by its runtime
    // id and its process (`books/dev/src/reference/adr/0001-focus.md`,
    // "The outpost's own element").
    if !focus {
        return (None, window_element);
    }
    let element = read::focused_element_in(context, client, read::FocusRead::NoRoute).ok();
    (
        element.filter(|element| element.runtime_id() == runtime_id),
        window_element,
    )
}

/// The route to the ancestor Core already knows that the listener's walk
/// met, `end`, which the route's steps leave out ([`known_route_to`]).
/// `None` for any other end.
fn known_route(context: &RequestContext<'_>, end: &RouteEnd) -> Option<super::focus_path::Route> {
    match end {
        RouteEnd::Known(known) => known_route_to(context, known),
        _ => None,
    }
}

/// The route to `known`, an ancestor Core already knows: a window's own
/// element, or an element whose node the outpost keeps a route to. `None`
/// for a known element the outpost has no route to.
fn known_route_to(
    context: &RequestContext<'_>,
    known: &KnownAncestor,
) -> Option<super::focus_path::Route> {
    let runtime_id = known_runtime_id(context, known)?;
    match known {
        KnownAncestor::Window(window) => Some(super::focus_path::Route {
            window: verbatim_reader_rules::Window::new(hwnd_of(*window))?,
            path: Vec::new(),
            runtime_id,
        }),
        KnownAncestor::Uia(_) | KnownAncestor::Node(_) => context.store.route_to(&runtime_id),
    }
}

/// Each UI Automation node `nodes` names that one of the outpost's own
/// walks up minted keeps the route to its element, by `walked`, the raw
/// view's chain that walk stepped through, nearest first, by runtime id
/// and own window: from the nearest element at or above it with a window
/// of its own; or, where the chain's outermost element already has a route,
/// the ancestor Core already knows that the walk met, or a window reached
/// before, from that route. Every walk a read makes records its own chain:
/// a focus-now read's from the focus, and each walk on from a window
/// (`read::window_object_onward`), so no node a read mints from a walk
/// lacks the route a later completion needs to find the focus below it.
pub(super) fn keep_read_routes<'a>(
    context: &RequestContext<'_>,
    nodes: impl IntoIterator<Item = &'a NodeSnapshot>,
    walked: &[(Vec<i32>, isize)],
) {
    let (below, above) = match walked.split_last() {
        Some(((outermost, _), below)) => match context.store.route_to(outermost) {
            Some(route) => (below, Some(route)),
            None => (walked, None),
        },
        None => (walked, None),
    };
    let path = FocusPath::of_chain(
        below
            .iter()
            .map(|(runtime_id, hwnd)| {
                (
                    runtime_id.clone(),
                    verbatim_reader_rules::Window::new(*hwnd),
                )
            })
            .collect(),
    )
    .above(above);
    for node in nodes {
        if node.backend != Backend::Uia {
            continue;
        }
        if let Some(route) = context
            .store
            .runtime_id_of(node.id)
            .and_then(|runtime_id| path.route_to(&runtime_id))
        {
            context.store.record_route(node.id, route);
        }
    }
}

/// The runtime id of `known`, an ancestor Core already knows: a window's
/// own element's, its own, or its node's.
fn known_runtime_id(context: &RequestContext<'_>, known: &KnownAncestor) -> Option<Vec<i32>> {
    match known {
        KnownAncestor::Window(window) => {
            let low = u32::try_from(window.0 & 0xFFFF_FFFF).ok()?;
            Some(vec![42, low.cast_signed()])
        }
        KnownAncestor::Uia(runtime_id) => Some(runtime_id.0.clone()),
        KnownAncestor::Node(node) => context.store.runtime_id_of(*node),
    }
}

/// Each node minted from the listener's read, `node` and `ancestors`,
/// keeps the route to the outpost's copy of its element, by `path`; and
/// `window_element`, the route's window's element read to find the
/// element, is kept for the ancestor it is, so a dialog's text or a later
/// request on it connects to the window no second time.
fn keep_minted(
    context: &RequestContext<'_>,
    (node, ancestors): (NodeId, &[NodeSnapshot]),
    path: &FocusPath,
    window_element: Option<&Cached<Node>>,
) {
    for minted in std::iter::once(node).chain(ancestors.iter().map(|ancestor| ancestor.id)) {
        if let Some(route) = context
            .store
            .runtime_id_of(minted)
            .and_then(|runtime_id| path.route_to(&runtime_id))
        {
            context.store.record_route(minted, route);
        }
    }
    if let Some(window_element) = window_element {
        keep_window_element(context, ancestors, window_element);
    }
}

/// Keeps `window_element`, the element of the route's window read to find
/// the completion's element, for the ancestor among `ancestors` it is, when
/// the outpost holds no element for that node.
fn keep_window_element(
    context: &RequestContext<'_>,
    ancestors: &[NodeSnapshot],
    window_element: &Cached<Node>,
) {
    let runtime_id = window_element.runtime_id();
    let Some(node) = context.nodes().existing(&runtime_id) else {
        return;
    };
    if ancestors.iter().any(|ancestor| ancestor.id == node)
        && context.store.element_of(node).is_none()
    {
        let _ = context
            .nodes()
            .id_for_element(&runtime_id, &window_element.narrow::<Kept>());
    }
}

/// Whether the element kept under `runtime_id` is no longer the one the id
/// names: gone, or without the keyboard focus that the element found has,
/// or that the focus has when its own element was not found (`found` is
/// `None`). Its element is then evicted and its text forgotten, so the
/// node keeps its id and the found element, if any, replaces it
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression"). The
/// kept element is always read: no local comparison tells it from the
/// element found, since the kept one is found by the same runtime id and
/// a new agile reference is never equal to a kept one.
fn replaced_stale(context: &RequestContext<'_>, client: &mut Client, runtime_id: &[i32]) -> bool {
    let Some(node) = context.nodes().existing(runtime_id) else {
        return false;
    };
    let Some(kept) = context.store.element_of(node) else {
        return false;
    };
    let stale = match kept.resolve() {
        Err(_) => true,
        Ok(element) => match client.uia_with(context, RequestContext::node_request) {
            Ok((_, request)) => match element.live().refresh(&request) {
                Ok(fresh) => !fresh
                    .read()
                    .parts(fresh.runtime_id())
                    .states
                    .contains(State::Focused),
                Err(error) => verbatim_uia::element_is_gone(&error),
            },
            Err(_) => false,
        },
    };
    if stale && !context.charge().is_dead() {
        tracing::debug!(
            "a runtime id names a new element: the node's element and text are dropped"
        );
        text_reads::forget_text(context, node);
        drop(context.store.evict_element_if(node, &kept));
    }
    stale
}

/// Adds the registration of the focus `element`, node `node`, on the
/// registering thread, keeping what `Follow` named, and waits until it is
/// in. Core alone decides a node's group: the completion asks for the base
/// group, and the node `Follow` names keeps the group Core named for it
/// ([`super::followed::add_focus`]).
fn register(context: &RequestContext<'_>, element: &Cached<Node>, node: NodeId) {
    let Ok(agile) = element.agile() else {
        return;
    };
    super::followed::add_focus(context, (node, agile.reference()));
}

/// What the listener read of an element, as the continuation above a route
/// takes it.
fn fact(runtime_id: &[i32], values: &PropertyValues) -> UiaSnapshotFact {
    let snapshot = snapshot_of(NodeId::new(0), Backend::Uia, values);
    UiaSnapshotFact {
        runtime_id: runtime_id.to_vec(),
        role: snapshot.role,
        name: snapshot.name,
        value: snapshot.value,
        states: snapshot.states,
        details: snapshot.details,
    }
}

/// An ancestor's parts, as the chain from parts takes them.
pub(super) fn parts(ancestor: &UiaAncestorFact) -> verbatim_uia::AncestorParts {
    verbatim_uia::AncestorParts {
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
    }
}

/// What the route's read minted that the outpost keeps once the arm has
/// the focus's ancestors ([`keep_route`]), and the path its dialogs are
/// found by.
pub(super) struct RouteKept {
    /// The route as the outpost finds elements by it.
    pub(super) path: FocusPath,
    /// The route's window's element read to find the element.
    window_element: Option<Cached<Node>>,
}

/// The element `route` names, found and minted for the `FocusRead` arm
/// ([`verbatim_uia_rops::batch::focus::route`]), its registration added,
/// with the place of the ancestor Core knows that the listener's walk met
/// and the route's ancestors as the listener read them, as [`focus`]
/// takes them.
pub(super) fn route_focus(
    context: &RequestContext<'_>,
    client: &mut Client,
    route: &Route,
    known: &[KnownAncestor],
) -> Result<(verbatim_uia_rops::batch::focus::RouteFocus, RouteKept), Unread> {
    let Minted {
        found,
        window_element,
        node: _,
        snapshot,
        path,
        nearest,
        reused,
    } = mint(context, client, route, true).ok_or(Unread::NotThere)?;
    let met = match &route.end {
        RouteEnd::Known(end) => {
            let Some(place) = known
                .iter()
                .position(|named| named == end)
                .and_then(|place| u32::try_from(place).ok())
            else {
                tracing::error!(target: verbatim_model::DEFECT_TARGET, "a route's end names an ancestor the completion does not");
                return Err(Unread::Unsupported);
            };
            Some(place)
        }
        _ => None,
    };
    let parents = route_parents(route).iter().map(parts).collect();
    let value_from_selection = route.read.first().is_some_and(|read| {
        matches!(
            read.answer(PropertyId::ValueFromSelection)
                .and_then(|answer| answer.value.as_ref()),
            Some(verbatim_model::property::PropertyValue::Flag(true))
        )
    });
    Ok((
        verbatim_uia_rops::batch::focus::RouteFocus {
            found,
            snapshot,
            nearest,
            reused,
            met,
            parents,
            value_from_selection,
        },
        RouteKept {
            path,
            window_element,
        },
    ))
}

/// The route's ancestors as the listener read them, nearest first.
fn route_parents(route: &Route) -> Vec<UiaAncestorFact> {
    route
        .steps
        .iter()
        .zip(&route.read)
        .skip(1)
        .map(|(step, values)| UiaAncestorFact {
            snapshot: fact(&step.runtime_id.0, values),
            hwnd: step.window.map_or(0, hwnd_of),
            content: step.content,
        })
        .collect()
}

/// The chain above `route`'s own ancestors, outermost first, where the
/// listener's walk met none of `known`: the dispatcher's chaining.
///
/// # Errors
///
/// [`read::FocusLost`] when the focus is lost on the chain's way.
pub(super) fn route_chain(
    context: &RequestContext<'_>,
    client: &mut Client,
    route: &Route,
    known: &[KnownAncestor],
) -> Result<Vec<NodeSnapshot>, read::FocusLost> {
    let own_window = route
        .steps
        .first()
        .and_then(|step| step.window)
        .map_or(0, hwnd_of);
    read::uia_chain_from_read(
        context,
        client,
        (
            &route_parents(route),
            own_window,
            Some(hwnd_of(route.window)),
            ancestors_end(&route.end),
        ),
        known,
    )
}

/// Keeps what the route's read minted, `node` and `ancestors`, as
/// [`focus`] keeps it.
pub(super) fn keep_route(
    context: &RequestContext<'_>,
    (node, ancestors): (NodeId, &[NodeSnapshot]),
    kept: &RouteKept,
) {
    keep_minted(
        context,
        (node, ancestors),
        &kept.path,
        kept.window_element.as_ref(),
    );
}
