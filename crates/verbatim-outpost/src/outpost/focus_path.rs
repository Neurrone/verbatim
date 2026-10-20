//! How the outpost finds its own copy of a UIA element the focus listener
//! read (`docs/design/focus-pipeline.md`, section 12.5). The listener reads
//! each UIA focus from the event's own element, which no other process can
//! be handed (experiment E-marshal), and sends the focus's raw-view chain:
//! the focus and its ancestors, each with its runtime id and its own window
//! handle. From the nearest element of the chain that is a window of its
//! own, the outpost reads that window's element (one connection call) and
//! walks down the chain's runtime ids to the element it wants, in one
//! remote operation ([`verbatim_uia_rops::element_by_path`]). It does so
//! after it has reported the focus, so nothing on the way to speech waits
//! for it, and again whenever it needs an element of that chain it does not
//! hold: the focus's controller relation, a dialog's own text, or a query
//! on an ancestor.

#![forbid(unsafe_code)]

use std::time::Duration;

use verbatim_uia::{Cached, Fill, Node, Request, Uia};
use verbatim_uia_rops::{Path, PathQuery, element_by_path};

use crate::arbitration::{Clock, time_limit};
use crate::protocol::UiaFocusRead;

/// How long the classic walk down a path, used where a remote operation
/// cannot import the window's element, may take: checked between its
/// calls, each of which UIA ends after its call timeout.
const PATH_BUDGET: Duration = Duration::from_secs(1);

/// The raw-view chain of the UIA focus the outpost last reported from the
/// listener's read: the focus, then its ancestors, nearest first, each with
/// its runtime id and its own window handle (0 for none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FocusPath {
    chain: Vec<(Vec<i32>, isize)>,
}

/// Where an element of a [`FocusPath`] is found from: the window whose
/// element the walk starts at, and the runtime ids below it, outermost
/// first, the last the element's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    /// The window whose element the walk starts at.
    pub(crate) window: isize,
    /// The runtime ids below it, outermost first.
    pub(crate) path: Vec<Vec<i32>>,
    /// The runtime id of the element wanted.
    pub(crate) runtime_id: Vec<i32>,
}

impl FocusPath {
    /// The chain of the focus `read` names.
    pub(super) fn of(read: &UiaFocusRead) -> Self {
        let chain = std::iter::once((read.snapshot.runtime_id.clone(), read.hwnd))
            .chain(
                read.ancestors
                    .iter()
                    .flatten()
                    .map(|ancestor| (ancestor.snapshot.runtime_id.clone(), ancestor.hwnd)),
            )
            .collect();
        Self { chain }
    }

    /// The route to the element of the chain whose runtime id is
    /// `runtime_id`: from the nearest element at or above it that has a
    /// window of its own. `None` when no element of the chain has that
    /// runtime id, or none at or above it has a window.
    pub(super) fn route_to(&self, runtime_id: &[i32]) -> Option<Route> {
        let index = self.chain.iter().position(|(id, _)| id == runtime_id)?;
        let above = self.chain[index..]
            .iter()
            .position(|&(_, hwnd)| hwnd != 0)?;
        let root = index + above;
        let path = self.chain[index..root]
            .iter()
            .rev()
            .map(|(id, _)| id.clone())
            .collect();
        Some(Route {
            window: self.chain[root].1,
            path,
            runtime_id: runtime_id.to_vec(),
        })
    }
}

/// The element `route` leads to, filled by `nodes`: the route's
/// window's element, read with one connection call, then the walk down the
/// route's runtime ids, one remote operation where `remote` allows it, else
/// the classic walk within [`PATH_BUDGET`] by `clock`. `None` when the
/// window's element cannot be read or the path does not lead to the
/// element; and whether a
/// remote operation could not import the window's element (a client-side
/// proxy), so the caller reads that window the classic way from then on.
pub(crate) fn find(
    uia: &Uia,
    route: &Route,
    nodes: &Request<Node>,
    remote: bool,
    clock: &Clock,
) -> (Option<Cached<Node>>, bool) {
    let root = match uia.window_element(route.window, nodes) {
        Ok(root) => root,
        Err(error) => {
            tracing::debug!(window = route.window, %error, "a path's window could not be read");
            return (None, false);
        }
    };
    if route.path.is_empty() {
        let same = root.runtime_id() == route.runtime_id;
        return (same.then_some(root), false);
    }
    let fill = Fill::new(nodes);
    let out_of_time = time_limit(clock, PATH_BUDGET);
    let query = PathQuery {
        root: root.live(),
        path: &route.path,
        fill: &fill,
        out_of_time: Some(&out_of_time),
    };
    match element_by_path(uia, &query, remote) {
        Ok((found, path)) => {
            let import_failed =
                matches!(&path, Path::Fallback(verbatim_uia_rops::Error::Import(_)));
            if let Path::Fallback(error) = &path {
                tracing::warn!(window = route.window, %error, "a remote operation failed; read the classic way");
            }
            (found, import_failed)
        }
        Err(error) => {
            tracing::debug!(window = route.window, %error, "a path could not be walked");
            (None, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{NodeDetails, Role, StateSet};

    use super::*;
    use crate::protocol::{UiaAncestorFact, UiaSnapshotFact};

    fn snapshot(id: i32) -> UiaSnapshotFact {
        UiaSnapshotFact {
            runtime_id: vec![42, id],
            role: Role::Group,
            name: None,
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        }
    }

    /// A focus, runtime id `[42, 1]`, in window `own` (0 for none), with
    /// ancestors `[42, 2]`, `[42, 3]`, and `[42, 4]`, nearest first, in the
    /// windows `windows` gives.
    fn read(own: isize, windows: [isize; 3]) -> UiaFocusRead {
        UiaFocusRead {
            hwnd: own,
            snapshot: snapshot(1),
            ancestors: Some(
                (2..=4)
                    .zip(windows)
                    .map(|(id, hwnd)| UiaAncestorFact {
                        snapshot: snapshot(id),
                        hwnd,
                        content: true,
                    })
                    .collect(),
            ),
            selected_child: None,
            window: None,
            same_as_last: None,
        }
    }

    #[test]
    fn a_windowless_focus_is_found_from_its_nearest_window_by_the_ids_below_it() {
        let path = FocusPath::of(&read(0, [0, 70, 80]));
        assert_eq!(
            path.route_to(&[42, 1]),
            Some(Route {
                window: 70,
                path: vec![vec![42, 2], vec![42, 1]],
                runtime_id: vec![42, 1],
            })
        );
        // An ancestor of the chain, from the same window.
        assert_eq!(
            path.route_to(&[42, 2]).map(|route| route.path),
            Some(vec![vec![42, 2]])
        );
        // The window's own element is its route's start.
        assert_eq!(
            path.route_to(&[42, 3])
                .map(|route| (route.window, route.path)),
            Some((70, Vec::new()))
        );
    }

    #[test]
    fn a_focus_that_is_a_window_is_its_own_start() {
        let path = FocusPath::of(&read(60, [0, 70, 80]));
        assert_eq!(
            path.route_to(&[42, 1])
                .map(|route| (route.window, route.path)),
            Some((60, Vec::new()))
        );
    }

    #[test]
    fn no_route_without_a_window_above_or_for_an_element_not_in_the_chain() {
        let path = FocusPath::of(&read(0, [0, 0, 0]));
        assert_eq!(path.route_to(&[42, 1]), None);
        let path = FocusPath::of(&read(0, [0, 70, 80]));
        assert_eq!(path.route_to(&[42, 9]), None);
    }
}
