//! A UI Automation node's element, found one way by every reader
//! (`books/dev/src/reference/crates/verbatim-outpost.md`, "A node's
//! element"): the element the store keeps, else the one the node's route
//! finds, which is then kept for the node. The store's lookup of the kept
//! element and its eviction are private to the store and this module, so a
//! reader cannot take the kept element alone and miss a node whose element
//! only its route finds, as a focus is minted when its walk did not find it
//! and every ancestor a completion mints from the listener's read is.
//!
//! A kept element is let go of only while it is still the one kept, so a
//! worker abandoned on a hung application, whose refresh fails late, never
//! evicts the element a later worker kept for the node since.

#![forbid(unsafe_code)]

use verbatim_model::NodeId;
use verbatim_uia::{Agile, Cached, Interning as _, Kept, Live, Node, Request, Uia};
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::core::AgileReference;

use super::Store;
use crate::outpost::RequestContext;
use crate::outpost::focus_path::{self, FocusPath};
use crate::outpost::read::ReadError;

/// A UI Automation node's element, as [`as_kept`] found it.
pub(in crate::outpost) enum Found {
    /// The element the store keeps, taken with no call: refreshing it is
    /// the reader's.
    Kept(Agile<Kept>, Cached<Kept>),
    /// The element the node's route found, filled by the walk's request,
    /// kept for the node from now on.
    Routed(Cached<Node>),
}

impl Found {
    /// The live element.
    pub(in crate::outpost) fn live(&self) -> &Live {
        match self {
            Self::Kept(_, element) => element.live(),
            Self::Routed(element) => element.live(),
        }
    }

    /// The element as the store keeps it.
    pub(in crate::outpost) fn kept(&self) -> Cached<Kept> {
        match self {
            Self::Kept(_, element) => element.clone(),
            Self::Routed(element) => element.narrow(),
        }
    }

    /// An agile reference to the element, for a registration.
    pub(in crate::outpost) fn reference(&self) -> Option<AgileReference<IUIAutomationElement>> {
        match self {
            Self::Kept(agile, _) => Some(agile.reference()),
            Self::Routed(element) => AgileReference::new(element.live().element()).ok(),
        }
    }
}

/// The element the store keeps for the UIA node `node`, with no call and
/// no search: only for a judgment of the kept element itself (whether one
/// is kept, the window it says it is, whether a kept one is stale), never
/// for a read of the node, which takes [`as_kept`] or [`refreshed`].
pub(in crate::outpost) fn kept(store: &Store, node: NodeId) -> Option<Agile<Kept>> {
    store.element_of(node)
}

/// Lets go of `element`, kept for `node`, as gone, if it is still the one
/// kept: the caller drops it.
pub(in crate::outpost) fn let_go(
    store: &Store,
    node: NodeId,
    element: &Agile<Kept>,
) -> Option<Agile<Kept>> {
    store.evict_element_if(node, element)
}

/// The UIA node `node`'s element: the one the store keeps, as kept, with no
/// call; else, when none is kept or the kept one cannot be resolved (it is
/// let go of), the one its route finds ([`by_route`]), the store's route or
/// else the route `path`, the read in progress, gives to it. `Gone` for a
/// node found by neither; `Failed` for a walk that failed, or for no UIA
/// client, `uia` being `None`, where a walk is needed.
pub(in crate::outpost) fn as_kept(
    context: &RequestContext<'_>,
    uia: Option<&Uia>,
    path: Option<&FocusPath>,
    node: NodeId,
) -> Result<Found, ReadError> {
    if let Some(agile) = kept(&context.store, node) {
        match agile.resolve() {
            Ok(element) => return Ok(Found::Kept(agile, element)),
            Err(_) => drop(let_go(&context.store, node, &agile)),
        }
    }
    let uia = uia.ok_or_else(|| ReadError::Failed("no UIA client".to_owned()))?;
    let nodes = context
        .node_request(uia)
        .map_err(|error| ReadError::Failed(format!("could not build a cache request: {error}")))?;
    by_route(context, uia, &nodes, (node, path)).map(Found::Routed)
}

/// The UIA node `node`'s element filled by `nodes`: the kept element
/// refreshed, which both updates it and proves it still answers; else,
/// when it is gone (let go of while still the one kept) or none is kept,
/// the one its route finds ([`by_route`]). A kept element that fails for
/// another reason, such as a busy provider timing out, stays kept and the
/// read fails: it may well still be alive.
pub(in crate::outpost) fn refreshed(
    context: &RequestContext<'_>,
    uia: &Uia,
    nodes: &Request<Node>,
    node: NodeId,
) -> Result<Cached<Node>, ReadError> {
    if let Some(agile) = kept(&context.store, node) {
        if let Ok(element) = agile.resolve() {
            match element.live().refresh(nodes) {
                Ok(fresh) => return Ok(fresh),
                Err(error) if !verbatim_uia::element_is_gone(&error) => {
                    return Err(ReadError::Failed(format!("reading the element: {error}")));
                }
                Err(_) => {}
            }
        }
        drop(let_go(&context.store, node, &agile));
    }
    by_route(context, uia, nodes, (node, None))
}

/// An agile reference to the UIA node `node`'s element ([`as_kept`]), for
/// a registration. `None` for an MSAA node or one whose element is gone.
pub(in crate::outpost) fn reference(
    context: &RequestContext<'_>,
    uia: &Uia,
    node: NodeId,
) -> Option<AgileReference<IUIAutomationElement>> {
    context.store.runtime_id_of(node)?;
    as_kept(context, Some(uia), None, node).ok()?.reference()
}

/// The element of `node`, found by the route to it through the chain a
/// focus listener read (`super::super::focus_path`): the one the store
/// keeps for the node, else the one `path`, the read in progress, gives;
/// kept for the node once found. A node with neither route is gone, as
/// NVDA's object for a gone element is: no search is made for it, since a
/// search of the application's windows enters any child window another
/// process serves
/// (`books/dev/src/reference/adr/0005-calls-that-reach-another-application.md`,
/// "A node whose element is gone"). A walk that fails rather than finishing
/// without the element, against a slow application above all, may have
/// missed a live element, so it fails the read rather than saying gone.
fn by_route(
    context: &RequestContext<'_>,
    uia: &Uia,
    nodes: &Request<Node>,
    (node, path): (NodeId, Option<&FocusPath>),
) -> Result<Cached<Node>, ReadError> {
    let runtime_id = context.store.runtime_id_of(node).ok_or(ReadError::Gone)?;
    let route = context
        .focus_route(&runtime_id)
        .or_else(|| context.route_in(path?, &runtime_id))
        .ok_or(ReadError::Gone)?;
    let remote = context.tries_remote(Some(route.window.get()));
    let (found, import_failed) = focus_path::find(uia, &route, nodes, remote);
    if import_failed {
        context.read_classically(route.window.get());
    }
    let element = found
        .map_err(|reason| ReadError::Failed(format!("walking the node's route: {reason}")))?
        .ok_or(ReadError::Gone)?;
    let _ = context
        .nodes()
        .id_for_element(&runtime_id, &element.narrow());
    Ok(element)
}
