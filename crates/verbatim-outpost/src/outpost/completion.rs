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
use verbatim_model::protocol::{UiaAncestorFact, UiaSnapshotFact};
use verbatim_model::request::{
    CompleteUia, CompletionFor, ObjectRead, Outcome, Refusal, Reply, RouteEnd,
};
use verbatim_model::{Backend, NodeId, NodeSnapshot, Role, State};
use verbatim_uia::map::with_legacy_checked_state;
use verbatim_uia::{Cached, DialogText, Fill, Interning as _, Kept, Node};
use verbatim_uia_rops::{Path, StatesQuery, focus_states};

use super::asked::{
    Focus, ancestor_reads, ancestors_end, controlled_by, cut_at_known, focus_read, known_stops,
    node_read, snapshot_of,
};
use super::focus_path;
use super::read::{self, Client};
use super::reducer::FocusPath;
use super::text_reads;
use super::window::hwnd_of;
use super::{Context, RequestContext};

/// The answer to `complete`, whose window is read through UI Automation.
pub(super) fn complete(
    context: &RequestContext<'_>,
    client: &mut Client,
    complete: &CompleteUia,
) -> Outcome {
    let route = &complete.route;
    let (Some(first), Some(first_read)) = (route.steps.first(), complete.read.first()) else {
        return Outcome::NotThere;
    };
    let focus = complete.completes == CompletionFor::Focus;
    let path = FocusPath::of_route(route);
    let runtime_id = first.runtime_id.0.clone();
    let (found, window_element) = find(context, client, &path, &runtime_id, focus);
    let reused = found
        .as_ref()
        .is_some_and(|found| replaced_stale(context, client, &runtime_id, found));
    let node = match &found {
        Some(element) => context
            .nodes()
            .id_for_element(&runtime_id, &element.narrow::<Kept>()),
        None => context.nodes().keep(&runtime_id, None),
    };
    let mut snapshot = snapshot_of(node, Backend::Uia, first_read);
    if focus && let Some(element) = &found {
        register(context, element, node, snapshot.role);
    }
    let nearest = hwnd_of(route.window);
    // The node's window, fixed as it is made, as NVDA's object fixes its
    // own (`NVDAObjects/UIA/__init__.py` 1604 to 1607), for a step from it
    // once it is dead.
    context
        .store
        .record_nearest_window(node, snapshot.details.window.map_or(nearest, hwnd_of));
    let met = match &route.end {
        RouteEnd::Known(known) => {
            let Some(place) = complete
                .known
                .iter()
                .position(|named| named == known)
                .and_then(|place| u32::try_from(place).ok())
            else {
                // Core names the ancestor the walk met among those it sends.
                tracing::error!("a route's end names an ancestor the completion does not");
                return Outcome::Refused(Refusal::Unsupported);
            };
            Some(place)
        }
        _ => None,
    };
    let Ok((mut ancestors, met)) = ancestors(context, client, complete, met) else {
        return Outcome::NotThere;
    };
    keep_minted(context, (node, &ancestors), &path, window_element.as_ref());
    // A focus that met none of the ancestors Core knows enters a new
    // window: its foreground object is read with it, as NVDA reads it then,
    // taken from the chain when the foreground window is one of it.
    let foreground = (focus && met.is_none())
        .then(|| read::foreground_object_among(context, client, &ancestors))
        .flatten();
    if focus {
        // The node's states, read once its registration is in, so a change
        // raised from now on is heard, with the text of the dialogs it
        // enters whose elements the outpost holds.
        let gathered = found.as_ref().map_or_else(Vec::new, |element| {
            states_and_dialogs(context, client, element, nearest, (&mut snapshot, &ancestors))
        });
        // Every dialog of the chain below the ancestors Core already knows
        // is one the focus enters.
        read::describe_dialogs(
            context,
            client,
            (
                ancestors.iter_mut().chain(std::iter::once(&mut snapshot)),
                Some(&path),
            ),
            &gathered,
        );
    }
    if let CompletionFor::SelectionItem { focus } = complete.completes {
        // A selection's item: its relation to the committed focus, read
        // live, and its ancestors, its parent first (Table 12).
        return Outcome::Result(Reply::Object(Box::new(ObjectRead {
            object: node_read(context, &snapshot, Some(nearest), &complete.set),
            relation: Some(controlled_by(context, client, focus, node)),
            descendants: Vec::new(),
            ancestors: ancestor_reads(context, &ancestors, Some(nearest), &complete.set),
            met,
            route: None,
        })));
    }
    Outcome::Result(Reply::Focus(Box::new(focus_read(
        context,
        &Focus {
            node: &snapshot,
            ancestors: &ancestors,
            met,
            nearest: Some(nearest),
            msaa: None,
            foreground: foreground.as_ref(),
            // As the listener read it.
            value_from_selection: matches!(
                first_read
                    .answer(verbatim_model::property::PropertyId::ValueFromSelection)
                    .and_then(|answer| answer.value.as_ref()),
                Some(verbatim_model::property::PropertyValue::Flag(true))
            ),
            runtime_id_reused: reused,
            baseline: complete.baseline,
        },
        &complete.set,
    ))))
}

/// The outpost's own copy of the element `runtime_id` names, by the route:
/// from the nearest element at or above it with a window of its own; for a
/// focus whose route reaches no window, the application's focused element
/// when its runtime id is the focus's.
fn find(
    context: &RequestContext<'_>,
    client: &mut Client,
    path: &FocusPath,
    runtime_id: &[i32],
    focus: bool,
) -> (Option<Cached<Node>>, Option<Cached<Node>>) {
    if let Some(route) = path.route_to(runtime_id) {
        let Ok((uia, nodes)) = client.uia_with(context, Context::node_request) else {
            return (None, None);
        };
        let remote = context.tries_remote(Some(route.window.get()));
        let (found, import_failed, window_element) =
            focus_path::find_from_window(uia, &route, &nodes, remote);
        if import_failed {
            context.read_classically(route.window.get());
        }
        return (found.ok().flatten(), window_element);
    }
    if !focus {
        return (None, None);
    }
    let element = read::focused_element_in(context, client, read::FocusRead::CutWalk).ok();
    (
        element.filter(|element| element.runtime_id() == runtime_id),
        None,
    )
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
/// names: gone, or without the keyboard focus that the element found has.
/// Its element is then evicted and its text forgotten, so the node keeps
/// its id and the found element replaces it (`docs/parity/focus-events.md`,
/// "Duplicate focus suppression").
fn replaced_stale(
    context: &RequestContext<'_>,
    client: &mut Client,
    runtime_id: &[i32],
    found: &Cached<Node>,
) -> bool {
    let Some(node) = context.nodes().existing(runtime_id) else {
        return false;
    };
    let Some(kept) = context.store.element_of(node) else {
        return false;
    };
    let Ok(found) = found.agile() else {
        return false;
    };
    let found = found.reference();
    if kept.reference() == found {
        return false;
    }
    let stale = match kept.resolve() {
        Err(_) => true,
        Ok(element) => match client.uia_with(context, Context::node_request) {
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

/// Adds the registration of the focus `element`, node `node` of `role`,
/// on the registering thread, keeping what `Follow` named, and waits until
/// it is in: the extended group for a node that may have text, else the
/// base group.
fn register(context: &RequestContext<'_>, element: &Cached<Node>, node: NodeId, role: Role) {
    let Ok(agile) = element.agile() else {
        return;
    };
    let group = if text_reads::may_have_text(context, node, role) {
        verbatim_uia::Group::Extended
    } else {
        verbatim_uia::Group::Base
    };
    super::followed::add_focus(context, (node, agile.reference()), group);
}

/// Reads the states of the focus `element`, whose registration is in, into
/// `snapshot`, and gathers the text of each dialog among `ancestors` and
/// the focus that wants it and whose element the outpost holds, in one
/// remote program where `window` allows one
/// ([`verbatim_uia_rops::focus_states`]): NVDA's `event_gainFocus`
/// registers for the focus's events before it speaks it, dialog text
/// included (`NVDAObjects/UIA/__init__.py` 1623 to 1626). Answers each
/// dialog's text gathered, for [`read::describe_dialogs`] to give; the
/// states stay as the listener read them when the read fails.
fn states_and_dialogs(
    context: &RequestContext<'_>,
    client: &mut Client,
    element: &Cached<Node>,
    window: isize,
    (snapshot, ancestors): (&mut NodeSnapshot, &[NodeSnapshot]),
) -> Vec<(NodeId, Option<String>)> {
    use crate::dialog_text::{UiaDialog, dialog_text};
    let dialogs: Vec<(NodeId, verbatim_uia::Live)> = ancestors
        .iter()
        .chain(std::iter::once(&*snapshot))
        .filter(|node| node.backend == Backend::Uia && read::wants_dialog_text(context, node))
        .filter_map(|node| {
            let held = context.store.element_of(node.id)?.resolve().ok()?;
            Some((node.id, held.live().clone()))
        })
        .collect();
    let Ok((uia, nodes)) = client.uia_with(context, Context::node_request) else {
        return Vec::new();
    };
    let Ok(children) = uia.request_fixed::<DialogText>() else {
        return Vec::new();
    };
    let lives: Vec<verbatim_uia::Live> = dialogs.iter().map(|(_, live)| live.clone()).collect();
    let fill = Fill::new(&nodes);
    let children_fill = Fill::new(&children);
    let query = StatesQuery {
        focus: element.live(),
        fill: &fill,
        dialogs: &lives,
        children: &children_fill,
    };
    let (states, path) = match focus_states(uia, &query, context.tries_remote(Some(window))) {
        Ok(answer) => answer,
        Err(error) => {
            tracing::debug!(%error, "a focus's states could not be read");
            return Vec::new();
        }
    };
    if let Path::Fallback(error) = &path {
        tracing::warn!(window, %error, "a remote operation failed; read the classic way");
        if matches!(error, verbatim_uia_rops::Error::Import(_)) {
            context.read_classically(window);
        }
    }
    if let Some(fresh) = states.focus {
        let fresh = with_legacy_checked_state(
            fresh.live().element(),
            fresh.snapshot_in_table(context.nodes()),
        );
        let focused = snapshot.states.contains(State::Focused);
        snapshot.states = fresh.states;
        if focused {
            snapshot.states = snapshot.states.with(State::Focused);
        }
    }
    dialogs
        .into_iter()
        .zip(states.children)
        .map(|((id, live), read)| {
            let text = match read {
                Some(read) => dialog_text(&UiaDialog::with_children(uia, &children, live, read)),
                None => dialog_text(&UiaDialog::new(uia, &children, live)),
            };
            (id, text)
        })
        .collect()
}

/// The element's ancestors, nearest first, up to the first Core already
/// knows, and its place in the request's list: the route's ancestors as the
/// listener read them, ending at `met`, the place of the known ancestor
/// the listener's walk met, or, where its walk went on to the top, the
/// chain above them.
fn ancestors(
    context: &RequestContext<'_>,
    client: &mut Client,
    complete: &CompleteUia,
    met: Option<u32>,
) -> Result<(Vec<NodeSnapshot>, Option<u32>), read::FocusLost> {
    let route = &complete.route;
    let parents: Vec<UiaAncestorFact> = route
        .steps
        .iter()
        .zip(&complete.read)
        .skip(1)
        .map(|(step, values)| UiaAncestorFact {
            snapshot: fact(&step.runtime_id.0, values),
            hwnd: step.window.map_or(0, hwnd_of),
            content: step.content,
        })
        .collect();
    let own_window = route
        .steps
        .first()
        .and_then(|step| step.window)
        .map_or(0, hwnd_of);
    let nearest = hwnd_of(route.window);
    if let Some(met) = met {
        let stops = known_stops(&complete.known);
        let known = |id: NodeId| stops.iter().any(|stop| stop.id == id);
        let read_by_msaa = |hwnd: isize| read::uia_walk_leaves(context, nearest, hwnd);
        let parts: Vec<verbatim_uia::AncestorParts> = parents.iter().map(parts).collect();
        let (chain, _, _) = verbatim_uia::Uia::ancestor_chain_from_parts(
            &parts,
            context.nodes(),
            &verbatim_uia::AncestorStops {
                read_by_other_api: &read_by_msaa,
                known: &known,
                out_of_time: None,
            },
        );
        let nearest_first = chain.into_iter().rev().collect();
        return Ok((nearest_first, Some(met)));
    }
    let stops = known_stops(&complete.known);
    let chain = read::uia_chain_from_read(
        context,
        client,
        (
            &parents,
            own_window,
            Some(nearest),
            ancestors_end(&route.end),
        ),
        &stops,
    )?;
    Ok(cut_at_known(context, chain, &complete.known))
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
fn parts(ancestor: &UiaAncestorFact) -> verbatim_uia::AncestorParts {
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
