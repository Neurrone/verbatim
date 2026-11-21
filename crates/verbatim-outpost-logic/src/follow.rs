//! What a program's commit changes (`books/dev/src/explanation/design/ownership.md`):
//! its message to Core and its changes of text facts, reducer state with no
//! application object, and the focus line's paths a UI Automation
//! completion finds its elements by. A program is given a copy ([`Facts`]),
//! to which its text calls apply their changes as they make them, so what
//! it reads after a commit is what the reducer will hold.

use std::sync::Arc;
use verbatim_model::protocol::OutpostToCore;

use verbatim_reader_rules::Window;

use super::text::{RangeKey, TextChange, TextFacts};

/// The raw-view chain of a UI Automation element the listener read: the
/// element, then its ancestors, nearest first, each with its runtime id
/// and its own window, if it has one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusPath {
    chain: Vec<(Vec<i32>, Option<Window>)>,
    /// The focus's nearest window, as the read carries it, for a chain that
    /// was cut: where a route starts when no element of the chain at or
    /// above the one wanted has a window of its own.
    nearest: Option<Window>,
    /// For a chain whose walk met an ancestor Core already knows, which is
    /// not among its elements, the route to that ancestor, from which the
    /// chain's elements are found when none at or above the one wanted has
    /// a window of its own ([`Self::above`]).
    above: Option<Route>,
}

/// Where an element of a [`FocusPath`] is found from: the window whose
/// element the walk starts at, and the runtime ids below it, outermost
/// first, the last the element's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// The window whose element the walk starts at.
    pub window: Window,
    /// The runtime ids below it, outermost first.
    pub path: Vec<Vec<i32>>,
    /// The runtime id of the element wanted.
    pub runtime_id: Vec<i32>,
}

impl FocusPath {
    /// The chain of `route`, the listener's route to an element it read
    /// for a request ([`verbatim_model::request::Route`]).
    #[must_use]
    pub fn of_route(route: &verbatim_model::request::Route) -> Self {
        let window = |handle: verbatim_model::WindowHandle| {
            Window::new(isize::from_ne_bytes(handle.0.to_ne_bytes()))
        };
        let chain = route
            .steps
            .iter()
            .map(|step| (step.runtime_id.0.clone(), step.window.and_then(window)))
            .collect();
        let cut = matches!(
            route.end,
            verbatim_model::request::RouteEnd::ContinueFrom(_)
                | verbatim_model::request::RouteEnd::PlainFrom(_)
        );
        Self {
            chain,
            nearest: window(route.window).filter(|_| cut),
            above: None,
        }
    }

    /// The chain of a UI Automation element the outpost read itself, a
    /// current-focus read's focus: the element, then its ancestors, nearest
    /// first, each with its runtime id and its own window, if it has one,
    /// the walk up from it having reached its outermost element, or met an
    /// ancestor Core already knows, which [`Self::above`] gives.
    #[must_use]
    pub fn of_chain(chain: Vec<(Vec<i32>, Option<Window>)>) -> Self {
        Self {
            chain,
            nearest: None,
            above: None,
        }
    }

    /// This chain going on above at `above`, the route to the ancestor Core
    /// already knows that the listener's walk met and left out of the
    /// chain: a window's own element, or an element the outpost has a
    /// route to.
    #[must_use]
    pub fn above(self, above: Option<Route>) -> Self {
        Self { above, ..self }
    }

    /// The route to the element of the chain whose runtime id is
    /// `runtime_id`: from the nearest element at or above it that has a
    /// window of its own; or, for a chain whose walk up was cut below the
    /// focus's nearest window, so that none at or above it has one, from
    /// that nearest window, down by every runtime id of the chain from its
    /// outermost element, which finds the element when the outermost is
    /// the window's child, as a walk whose first step failed leaves a
    /// child of its window. `None` when no element of the chain has that
    /// runtime id, or none at or above it has a window and the chain was
    /// not cut.
    #[must_use]
    pub fn route_to(&self, runtime_id: &[i32]) -> Option<Route> {
        let index = self.chain.iter().position(|(id, _)| id == runtime_id)?;
        let from = self.chain.get(index..)?;
        let below = |above: usize| {
            from.get(..above)
                .map(|below| below.iter().rev().map(|(id, _)| id.clone()))
        };
        if let Some((above, window)) = from
            .iter()
            .enumerate()
            .find_map(|(above, &(_, window))| Some((above, window?)))
            .or_else(|| Some((from.len(), self.nearest?)))
        {
            return Some(Route {
                window,
                path: below(above)?.collect(),
                runtime_id: runtime_id.to_vec(),
            });
        }
        let known = self.above.as_ref()?;
        Some(Route {
            window: known.window,
            path: known
                .path
                .iter()
                .cloned()
                .chain(below(from.len())?)
                .collect(),
            runtime_id: runtime_id.to_vec(),
        })
    }
}

/// What one commit of a program changes, its pure part: built by the
/// program with every call it needs already made, and applied by the
/// reducer, after the shell has made its store writes: its message sent,
/// then its text facts' changes.
#[derive(Clone, Debug, Default)]
pub struct Change {
    /// The program's message to Core: a tell.
    pub result: Option<OutpostToCore>,
    /// The program's changes of text facts, in order: a program applies
    /// each to its own copies as its text call makes it, so its own commit
    /// carries them only for the reducer.
    pub text: Vec<TextChange>,
}

/// What a program is given a copy of: everything it reads of what the
/// outpost remembers.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// Each node's text facts, shared until a change ([`super::text`]).
    pub text: Arc<TextFacts>,
}

/// Applies the pure part of a commit, `change`, but for its message, which
/// the caller sends first, to the reducer's facts. Returns the UIA ranges
/// no text fact names any more, by node and key, for the store to let go
/// of.
pub fn apply(facts: &mut Facts, change: &Change) -> Vec<(u64, RangeKey)> {
    let mut ranges = Vec::new();
    if !change.text.is_empty() {
        let text = Arc::make_mut(&mut facts.text);
        for text_change in &change.text {
            ranges.extend(text.apply(text_change));
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use verbatim_model::WindowHandle;
    use verbatim_model::property::RuntimeId;
    use verbatim_model::request::{Route as ReadRoute, RouteEnd, RouteStep};

    use super::*;

    /// The route to a focus, runtime id `[42, 1]`, in window `own` (0 for
    /// none), with ancestors `[42, 2]` and `[42, 3]`, nearest first, in the
    /// windows `windows` gives, its walk ending as `end` says, its nearest
    /// window `nearest` (0 for none).
    fn focus_route(own: isize, windows: [isize; 2], end: RouteEnd, nearest: isize) -> ReadRoute {
        let handle = |hwnd: isize| WindowHandle(u64::from_ne_bytes(hwnd.to_ne_bytes()));
        let step = |id, hwnd: isize| RouteStep {
            runtime_id: RuntimeId(vec![42, id]),
            window: (hwnd != 0).then(|| handle(hwnd)),
            content: true,
        };
        ReadRoute {
            window: handle(nearest),
            steps: std::iter::once(step(1, own))
                .chain((2..=3).zip(windows).map(|(id, hwnd)| step(id, hwnd)))
                .collect(),
            end,
        }
    }

    /// [`focus_route`] with no nearest window.
    fn focus_read(own: isize, windows: [isize; 2], end: RouteEnd) -> ReadRoute {
        focus_route(own, windows, end, 0)
    }

    /// A windowless chain whose walk up failed below the focus's nearest
    /// window, which the read carries, is routed from that window down by
    /// every runtime id of the chain; a whole one, with no window of its
    /// own, has no route.
    #[test]
    fn a_cut_windowless_chain_is_routed_from_its_nearest_window() {
        let nearest = |end| focus_route(0, [0, 0], end, 70);
        assert_eq!(
            FocusPath::of_route(&nearest(RouteEnd::ContinueFrom(WindowHandle(90))))
                .route_to(&[42, 1]),
            Some(Route {
                window: Window::new(70).expect("70 is not 0"),
                path: vec![vec![42, 3], vec![42, 2], vec![42, 1]],
                runtime_id: vec![42, 1],
            })
        );
        assert_eq!(
            FocusPath::of_route(&nearest(RouteEnd::Root)).route_to(&[42, 1]),
            None
        );
    }

    /// A windowless chain whose walk met an ancestor Core already knows,
    /// which the chain leaves out, is routed from that ancestor's route,
    /// down by every runtime id of the chain; a chain with a window of its
    /// own is routed from that window still.
    #[test]
    fn a_windowless_chain_that_met_a_known_ancestor_is_routed_from_it() {
        let known = Route {
            window: Window::new(70).expect("70 is not 0"),
            path: vec![vec![42, 4]],
            runtime_id: vec![42, 4],
        };
        assert_eq!(
            FocusPath::of_route(&focus_read(0, [0, 0], RouteEnd::Root))
                .above(Some(known.clone()))
                .route_to(&[42, 2]),
            Some(Route {
                window: Window::new(70).expect("70 is not 0"),
                path: vec![vec![42, 4], vec![42, 3], vec![42, 2]],
                runtime_id: vec![42, 2],
            })
        );
        assert_eq!(
            FocusPath::of_route(&focus_read(0, [0, 80], RouteEnd::Root))
                .above(Some(known))
                .route_to(&[42, 1]),
            Some(Route {
                window: Window::new(80).expect("80 is not 0"),
                path: vec![vec![42, 2], vec![42, 1]],
                runtime_id: vec![42, 1],
            })
        );
    }
}
