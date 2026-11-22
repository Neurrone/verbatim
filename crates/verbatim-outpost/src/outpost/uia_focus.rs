//! The resolver through which the UI Automation executor's `FocusRead`
//! arms ([`verbatim_uia_rops::batch::focus`]) reach the outpost's state:
//! the subject's resolution (the focused element with its checks, or the
//! listener's route found and minted), the judgements between the arm's
//! reads, the dispatcher's chaining above where a walk ends, the other
//! backend's reads, and the answer made from what was read.

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;

use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyAnswer, PropertyId};
use verbatim_model::request::{FocusRead, KnownAncestor, Route};
use verbatim_model::{NodeId, NodeSnapshot, TextDetails};
use verbatim_uia::{Ancestor, AncestorWalk, Cached, Interning, Live, Node, Request, Uia};
use verbatim_uia_rops::batch::focus::{
    FocusNowFound, FocusResolve, RouteFocus, RouteMiss, UiaFocus,
};

use super::RequestContext;
use super::asked;
use super::completion::{self, RouteKept, Unread};
use super::focus::FocusNow;
use super::read::{self, Client};
use super::window::hwnd_of;

/// What a UI Automation `FocusRead` is of.
enum Of<'r> {
    /// The application's focus.
    Focus,
    /// The element a listener's route names, with what its read minted
    /// once found.
    Route(&'r Route, RefCell<Option<RouteKept>>),
}

/// The outpost's state as the UI Automation `FocusRead` arms resolve it.
struct UiaFocusRead<'a, 'c> {
    context: &'a RequestContext<'c>,
    client: RefCell<&'a mut Client>,
    uia: Rc<Uia>,
    of: Of<'a>,
    known: &'a [KnownAncestor],
    set: &'a [PropertyId],
    details: TextDetails,
}

impl UiaFocusRead<'_, '_> {
    /// The ancestors' request, built for one chaining.
    fn ancestors(&self) -> Option<Request<Ancestor>> {
        self.context.ancestor_request(&self.uia).ok()
    }
}

impl FocusResolve for UiaFocusRead<'_, '_> {
    fn registry(&self) -> &dyn Interning {
        self.context.nodes()
    }

    fn node_request(&self, uia: &Uia) -> Option<Request<Node>> {
        self.context.node_request(uia).ok()
    }

    fn ancestor_request(&self, uia: &Uia) -> Option<Request<Ancestor>> {
        self.context.ancestor_request(uia).ok()
    }

    fn focus_request(&self, uia: &Uia) -> Option<Request<Ancestor>> {
        uia.request(crate::gates::focus_gates(self.context.node_details()))
            .ok()
    }

    fn focused_element(
        &self,
        uia: &Uia,
        hwnd: isize,
        nodes: &Request<Node>,
    ) -> Option<Cached<Node>> {
        read::checked_focused_element(self.context, (uia, nodes), hwnd, read::FocusRead::FocusNow)
            .ok()
    }

    fn place(&self, node: &mut NodeSnapshot, hwnd: isize) {
        read::console_text_area(node, Some(hwnd));
        self.context
            .store
            .record_nearest_window(node.id, node.details.window.map_or(hwnd, hwnd_of));
    }

    fn tries_remote(&self, window: isize) -> bool {
        self.context.tries_remote(Some(window))
    }

    fn fell_back(&self, program: &'static str, window: isize, error: &verbatim_uia_rops::Error) {
        super::remote_fell_back(program, Some(window), error);
        if matches!(error, verbatim_uia_rops::Error::Import(_)) {
            self.context.read_classically(window);
        }
    }

    fn known_uia_nodes(&self) -> (Vec<NodeId>, Vec<Vec<i32>>) {
        read::known_uia_nodes(self.context, self.known)
    }

    fn unreadable(&self, hwnd: isize) -> bool {
        self.context.window_unreadable(hwnd).is_some()
    }

    fn plain_window_above(&self, window: isize) -> bool {
        read::plain_window_above(self.context, window)
    }

    fn serves(&self, hwnd: isize) -> bool {
        read::serves(self.context, hwnd)
    }

    fn leaves(&self, nearest: isize, hwnd: isize) -> bool {
        read::uia_walk_leaves(self.context, nearest, hwnd)
    }

    fn knows(&self, node: NodeId) -> bool {
        read::knows(self.known, node)
    }

    fn onward_after_failure(&self, own: isize, nearest: isize) -> Option<Vec<NodeSnapshot>> {
        let request = self.ancestors()?;
        read::onward_after_failure(
            self.context,
            Some((&self.uia, &request)),
            (own, nearest),
            (self.known, None),
        )
        .ok()
    }

    fn complete(
        &self,
        chain: Vec<NodeSnapshot>,
        (crossed, walked, outermost): (Option<isize>, AncestorWalk, isize),
    ) -> Option<Vec<NodeSnapshot>> {
        let request = self.ancestors()?;
        read::complete(
            self.context,
            Some((&self.uia, &request)),
            (chain, read::uia_above(crossed, walked, Some(outermost))),
            (self.known, None),
        )
        .ok()
    }

    fn chain(
        &self,
        element: &Live,
        (own, nearest): (isize, isize),
        parent_automation_id: &mut Option<String>,
        visited: verbatim_uia::Visited<'_>,
    ) -> Option<Vec<NodeSnapshot>> {
        let request = self.ancestors()?;
        read::chain(
            self.context,
            read::ChainStart::Uia {
                uia: (&self.uia, &request),
                element: (element, own, nearest),
                parent_automation_id,
                visited: Some(visited),
            },
            (self.known, None),
        )
        .ok()
    }

    fn cut(&self, chain: Vec<NodeSnapshot>) -> (Vec<NodeSnapshot>, Option<u32>) {
        asked::cut_at_known(self.context, chain, self.known)
    }

    fn keep_routes(
        &self,
        node: &NodeSnapshot,
        ancestors: &[NodeSnapshot],
        walked: &[(Vec<i32>, isize)],
    ) {
        completion::keep_read_routes(self.context, std::iter::once(node).chain(ancestors), walked);
    }

    fn foreground(&self, ancestors: &[NodeSnapshot]) -> Option<NodeSnapshot> {
        read::foreground_object_among(self.context, &mut self.client.borrow_mut(), ancestors)
    }

    fn wants_dialog_text(&self, node: &NodeSnapshot) -> bool {
        read::wants_dialog_text(self.context, node)
    }

    fn dialog_element(&self, uia: &Uia, node: &NodeSnapshot) -> Option<Live> {
        // A dialog the focus listener read in the focus's chain, whose
        // element this outpost does not hold, is found by its report's
        // path.
        if let Some(element) = self.context.store.element_of(node.id) {
            return Some(element.resolve().ok()?.live().clone());
        }
        let Of::Route(_, kept) = &self.of else {
            return None;
        };
        let kept = kept.borrow();
        let runtime_id = self.context.store.runtime_id_of(node.id)?;
        let route = self.context.route_in(&kept.as_ref()?.path, &runtime_id)?;
        let remote = self.context.tries_remote(Some(route.window.get()));
        let nodes = uia
            .request::<Node>(crate::gates::node_gates(self.context.node_details()))
            .ok()?;
        let found = super::focus_path::find(uia, &route, &nodes, remote)
            .0
            .ok()??;
        Some(found.live().clone())
    }

    fn held_element(&self, node: NodeId) -> Option<Live> {
        Some(
            self.context
                .store
                .element_of(node)?
                .resolve()
                .ok()?
                .live()
                .clone(),
        )
    }

    fn other_dialog_text(&self, node: &NodeSnapshot) -> Option<String> {
        read::dialog_text_of(self.context, &mut self.client.borrow_mut(), node, None)
    }

    fn caret(&self, node: &NodeSnapshot) -> verbatim_model::request::FocusCaret {
        if super::text_reads::may_have_text(self.context, node.id, node.role) {
            let mut client = self.client.borrow_mut();
            super::text_reads::focus_caret(self.context, client.uia(), node.id, self.details)
        } else {
            verbatim_model::request::FocusCaret::NoText
        }
    }

    fn answer(&self, focus: UiaFocus) -> FocusRead {
        asked::focus_read(
            self.context,
            asked::Focus {
                node: &focus.node,
                ancestors: &focus.ancestors,
                met: focus.met,
                nearest: Some(focus.nearest),
                msaa: None,
                foreground: focus.foreground.as_ref(),
                value_from_selection: focus.value_from_selection,
                runtime_id_reused: focus.runtime_id_reused,
                caret: focus.caret,
            },
            self.set,
        )
    }

    fn route(&self) -> Result<RouteFocus, RouteMiss> {
        let Of::Route(route, kept) = &self.of else {
            tracing::error!(target: verbatim_model::DEFECT_TARGET, "a route's read asked of the application's focus");
            return Err(RouteMiss::Unsupported);
        };
        let (found, minted) = completion::route_focus(
            self.context,
            &mut self.client.borrow_mut(),
            route,
            self.known,
        )
        .map_err(|unread| match unread {
            Unread::NotThere => RouteMiss::NotThere,
            Unread::Unsupported => RouteMiss::Unsupported,
        })?;
        *kept.borrow_mut() = Some(minted);
        Ok(found)
    }

    fn route_chain(&self) -> Option<Vec<NodeSnapshot>> {
        let Of::Route(route, _) = &self.of else {
            return None;
        };
        completion::route_chain(
            self.context,
            &mut self.client.borrow_mut(),
            route,
            self.known,
        )
        .ok()
    }

    fn keep_minted(&self, node: NodeId, ancestors: &[NodeSnapshot]) {
        if let Of::Route(_, kept) = &self.of
            && let Some(kept) = kept.borrow().as_ref()
        {
            completion::keep_route(self.context, (node, ancestors), kept);
        }
    }
}

/// The application's focus in `hwnd`, a window read through UI
/// Automation, through the executor's `FocusRead` arm; with `matcher`,
/// `Test` of it through the executor's arm, with the test's result.
pub(super) fn focus_now(
    context: &RequestContext<'_>,
    client: &mut Client,
    (hwnd, matcher): (isize, Option<&Matcher>),
    (known, set, details): (&[KnownAncestor], &[PropertyId], TextDetails),
) -> (FocusNow, Option<(bool, Vec<PropertyAnswer>)>) {
    let Some(uia) = client.shared_uia() else {
        return (FocusNow::Unread, None);
    };
    let resolver = UiaFocusRead {
        context,
        client: RefCell::new(client),
        uia: Rc::clone(&uia),
        of: Of::Focus,
        known,
        set,
        details,
    };
    let (found, tested) = match matcher {
        Some(matcher) => {
            verbatim_uia_rops::batch::focus::test_focus(&uia, hwnd, matcher, &resolver)
        }
        None => (
            verbatim_uia_rops::batch::focus::focus_now(&uia, hwnd, &resolver),
            None,
        ),
    };
    let now = match found {
        FocusNowFound::Read(read) => FocusNow::Read(read),
        FocusNowFound::NotThere => FocusNow::Unread,
    };
    (now, tested)
}

/// The focus `route` names, through the executor's `FocusRead` arm.
pub(super) fn route(
    context: &RequestContext<'_>,
    client: &mut Client,
    route: &Route,
    (known, set, details): (&[KnownAncestor], &[PropertyId], TextDetails),
) -> Result<Box<FocusRead>, Unread> {
    let Some(uia) = client.shared_uia() else {
        return Err(Unread::NotThere);
    };
    let resolver = UiaFocusRead {
        context,
        client: RefCell::new(client),
        uia: Rc::clone(&uia),
        of: Of::Route(route, RefCell::new(None)),
        known,
        set,
        details,
    };
    verbatim_uia_rops::batch::focus::route(&uia, &resolver).map_err(|miss| match miss {
        RouteMiss::NotThere => Unread::NotThere,
        RouteMiss::Unsupported => Unread::Unsupported,
    })
}
