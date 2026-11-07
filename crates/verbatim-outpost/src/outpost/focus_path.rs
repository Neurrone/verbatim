//! How the outpost finds its own copy of a UIA element the focus listener
//! read (`books/dev/src/reference/adr/0001-focus.md`, "The outpost's own element"). The listener
//! reads
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

use verbatim_uia::{Cached, Fill, Node, Request, Uia};
use verbatim_uia_rops::{Path, PathQuery, element_by_path};

use verbatim_win32::window::window_exists;

pub(super) use super::reducer::{FocusPath, Route};

/// The element `route` leads to, filled by `nodes`: the route's
/// window's element, read with one connection call, then the walk down the
/// route's runtime ids, one remote operation where `remote` allows it, else
/// the classic walk, with no time budget of its own: the program's deadline
/// bounds it, as NVDA's watchdog bounds its walk. `Ok(None)` when the walk
/// finished and the path does not lead to the element, or a call said the
/// window or an element on the way is gone; an error when the walk failed,
/// a lookup of the window that timed out or was refused, or any other
/// failed call, since an application that is only slow has not shown the
/// element gone. With it, whether a
/// remote operation could not import the window's element (a client-side
/// proxy), so the caller reads that window the classic way from then on.
pub(crate) fn find(
    uia: &Uia,
    route: &Route,
    nodes: &Request<Node>,
    remote: bool,
) -> (Result<Option<Cached<Node>>, String>, bool) {
    let root = match uia.window_element(route.window.get(), nodes) {
        Ok(root) => root,
        Err(error)
            if verbatim_uia::element_is_gone(&error) || !window_exists(route.window.get()) =>
        {
            tracing::debug!(window = route.window.get(), %error, "a path's window is gone");
            return (Ok(None), false);
        }
        Err(error) => {
            tracing::debug!(window = route.window.get(), %error, "a path's window could not be read");
            return (Err(format!("reading the route's window: {error}")), false);
        }
    };
    if route.path.is_empty() {
        let same = root.runtime_id() == route.runtime_id;
        return (Ok(same.then_some(root)), false);
    }
    let fill = Fill::new(nodes);
    let query = PathQuery {
        root: root.live(),
        path: &route.path,
        fill: &fill,
        out_of_time: None,
    };
    match element_by_path(uia, &query, remote) {
        Ok((found, path)) => {
            let import_failed =
                matches!(&path, Path::Fallback(verbatim_uia_rops::Error::Import(_)));
            if let Path::Fallback(error) = &path {
                tracing::warn!(window = route.window.get(), %error, "a remote operation failed; read the classic way");
            }
            (Ok(found), import_failed)
        }
        Err(error)
            if error.hresult().is_some_and(|code| {
                verbatim_uia::element_is_gone(&windows::core::Error::from(code))
            }) =>
        {
            tracing::debug!(window = route.window.get(), %error, "an element on a path is gone");
            (Ok(None), false)
        }
        Err(error) => {
            tracing::debug!(window = route.window.get(), %error, "a path could not be walked");
            (Err(format!("walking the route: {error}")), false)
        }
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{NodeDetails, Role, StateSet};

    use super::*;
    use verbatim_model::protocol::{UiaAncestorFact, UiaFocusRead, UiaSnapshotFact};

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
            ancestors: (2..=4)
                .zip(windows)
                .map(|(id, hwnd)| UiaAncestorFact {
                    snapshot: snapshot(id),
                    hwnd,
                    content: true,
                })
                .collect(),
            ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
            selected_child: None,
            value_from_selection: false,
            window: None,
            same_as_last: None,
            answers: None,
            cached_from: None,
        }
    }

    #[test]
    fn a_windowless_focus_is_found_from_its_nearest_window_by_the_ids_below_it() {
        let path = FocusPath::of(&read(0, [0, 70, 80]));
        assert_eq!(
            path.route_to(&[42, 1]),
            Some(Route {
                window: verbatim_reader_rules::Window::new(70).expect("70 is not 0"),
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
                .map(|route| (route.window.get(), route.path)),
            Some((70, Vec::new()))
        );
    }

    #[test]
    fn a_focus_that_is_a_window_is_its_own_start() {
        let path = FocusPath::of(&read(60, [0, 70, 80]));
        assert_eq!(
            path.route_to(&[42, 1])
                .map(|route| (route.window.get(), route.path)),
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
