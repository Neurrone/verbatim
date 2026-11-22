//! The resolver through which the MSAA executor
//! ([`verbatim_ia2::batch`]) reaches the outpost's state: its node
//! registry, the properties it holds of a node, a snapshot's values, the
//! window facts the focused-state test asks, the backend a reached object
//! is read through, and the children of a node MSAA does not list, by the
//! dispatcher's navigation on that node's backend.

#![forbid(unsafe_code)]

use std::cell::RefCell;

use verbatim_ia2::Interning;
use verbatim_ia2::acquire::{AcquireError, ParentWindow};
use verbatim_ia2::batch::{
    FocusFound, FocusResolve, FocusSubject, Found, MsaaFocus, Object, Resolve, Unrun,
};
use verbatim_model::batch::{Done, Missing, Op, StepOutcome, StepRefusal, Tested};
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{FocusRead, HeldIds, KnownAncestor, NodeRead};
use verbatim_model::{NodeId, NodeSnapshot, QueryKind, TextDetails};

use super::RequestContext;
use super::asked;
use super::read::{self, Client, ReadError};

/// The outpost's state as the MSAA executor resolves it, for one step.
pub(super) struct MsaaNodes<'a, 'c> {
    context: &'a RequestContext<'c>,
    client: RefCell<&'a mut Client>,
}

impl<'a, 'c> MsaaNodes<'a, 'c> {
    pub(super) fn new(context: &'a RequestContext<'c>, client: &'a mut Client) -> Self {
        Self {
            context,
            client: RefCell::new(client),
        }
    }
}

impl Resolve for MsaaNodes<'_, '_> {
    fn registry(&self) -> &dyn Interning {
        self.context.nodes()
    }

    fn held(&self, node: NodeId, id: PropertyId) -> Option<PropertyValue> {
        super::uia_object::held(self.context, node, id)
    }

    fn values(&self, node: &NodeSnapshot, set: &[PropertyId]) -> PropertyValues {
        asked::values(self.context, node, set)
    }

    fn hops(&self) -> u32 {
        read::MAX_ANCESTOR_HOPS
    }

    fn plain(&self, hwnd: isize) -> bool {
        read::reads_as_plain(self.context, hwnd)
    }

    fn parent_window(&self, hwnd: isize) -> ParentWindow {
        read::parent_window_for_focused_state(self.context, &mut self.client.borrow_mut(), hwnd)
    }

    fn corrected(
        &self,
        from_window: Option<isize>,
        neighbor: NodeSnapshot,
        kind: QueryKind,
    ) -> NodeSnapshot {
        read::corrected_backend(
            self.context,
            &mut self.client.borrow_mut(),
            from_window,
            neighbor,
            kind,
        )
    }

    fn children(&self, node: NodeId) -> Result<(Vec<NodeSnapshot>, bool), AcquireError> {
        asked::children(self.context, &mut self.client.borrow_mut(), node).map_err(|error| {
            match error {
                ReadError::Gone => AcquireError::Gone,
                ReadError::Failed(reason) => AcquireError::Failed(reason),
            }
        })
    }
}

/// A failed step's outcome on node `node`, `None` for an acquired object:
/// not there for a node gone, else failed, its reason logged.
fn unrun(unrun: Unrun, node: Option<NodeId>) -> StepOutcome {
    match unrun {
        Unrun::Unsupported => StepOutcome::Refused(StepRefusal::Unsupported),
        Unrun::Failed(AcquireError::Gone) => {
            StepOutcome::NotThere(node.map_or(Missing::Object, Missing::Node))
        }
        Unrun::Failed(AcquireError::Failed(reason)) => {
            tracing::debug!(reason, "a batch step's read failed");
            StepOutcome::Failed
        }
    }
}

/// `op`, an object step, on `object`, through the MSAA executor: its
/// outcome, a read carrying its node and its nearest window as the outpost
/// holds them, `window` for an acquired object or a window's own object.
pub(super) fn step(
    context: &RequestContext<'_>,
    client: &mut Client,
    (object, window): (Object<'_>, isize),
    op: &Op,
) -> StepOutcome {
    let node = match &object {
        Object::Node(node) => Some(*node),
        Object::Candidate(_) | Object::Window(_) => None,
    };
    let resolver = MsaaNodes::new(context, client);
    let found = verbatim_ia2::batch::run(op, object, &resolver);
    let read = |values| NodeRead {
        node,
        window: asked::node_window(match node {
            Some(node) => asked::known_nearest_of(context, node).unwrap_or(0),
            None => window,
        }),
        values,
    };
    let set = match op {
        Op::Related { set, .. } | Op::Read { set, .. } => set.as_slice(),
        _ => &[],
    };
    match found {
        Ok(Found::Window(object)) => StepOutcome::Done(Done::Read(asked::node_read(
            context,
            &object,
            Some(window),
            set,
        ))),
        Ok(Found::Read(values)) => StepOutcome::Done(Done::Read(read(values))),
        Ok(Found::Test(matched, values)) => StepOutcome::Done(Done::Test(Tested {
            matched,
            node: read(PropertyValues(values)),
        })),
        Ok(Found::Related(reached)) => StepOutcome::Done(Done::Related(
            reached
                .iter()
                .map(|reached| asked::node_read(context, reached, None, set))
                .collect(),
        )),
        Ok(Found::IsOneOf(place)) => StepOutcome::Done(Done::IsOneOf(place)),
        Ok(Found::Activate(action)) => StepOutcome::Done(Done::Activate(action)),
        // The dispatcher chains a batch's ancestors itself (`read::chain`).
        Ok(Found::Ancestors(..)) => {
            tracing::error!(target: verbatim_model::DEFECT_TARGET, "an ancestor walk reached the MSAA object steps");
            StepOutcome::Failed
        }
        Err(failure) => unrun(failure, node),
    }
}

/// Window `hwnd`'s own object, its client area, through the executor's
/// `Read` arm: `None` when it cannot be read.
pub(super) fn window(
    context: &RequestContext<'_>,
    client: &mut Client,
    hwnd: isize,
) -> Option<NodeSnapshot> {
    let resolver = MsaaNodes::new(context, client);
    let op = Op::Read {
        subject: verbatim_model::batch::Subject::Window {
            which: verbatim_model::batch::WindowOf::Handle(super::window::window_handle(hwnd)),
            backend: verbatim_model::Backend::Msaa,
            visible: false,
        },
        set: Vec::new(),
    };
    match verbatim_ia2::batch::run(&op, Object::Window(hwnd), &resolver) {
        Ok(Found::Window(object)) => Some(*object),
        _ => None,
    }
}

/// Activates exactly the MSAA node `node` through the executor, answering
/// the action's name.
pub(super) fn activate(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: NodeId,
) -> Result<Option<verbatim_model::ActionName>, ReadError> {
    let resolver = MsaaNodes::new(context, client);
    let op = Op::Activate {
        subject: verbatim_model::batch::Subject::Node(node),
    };
    match verbatim_ia2::batch::run(&op, Object::Node(node), &resolver) {
        Ok(Found::Activate(action)) => Ok(action),
        Ok(other) => Err(ReadError::Failed(format!(
            "an activation answered {other:?}"
        ))),
        Err(Unrun::Failed(error)) => Err(ReadError::from(error)),
        Err(Unrun::Unsupported) => Err(ReadError::Failed("nothing to activate".to_owned())),
    }
}

/// The ancestors of the MSAA node `node`, outermost first, through the
/// executor's `Ancestors` arm, to the top.
pub(super) fn ancestors(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: NodeId,
) -> Result<Vec<NodeSnapshot>, ReadError> {
    let resolver = MsaaNodes::new(context, client);
    let op = Op::Ancestors {
        subject: verbatim_model::batch::Subject::Node(node),
        until: Vec::new(),
        depth: verbatim_model::batch::MAX_DEPTH,
        set: Vec::new(),
    };
    match verbatim_ia2::batch::run(&op, Object::Node(node), &resolver) {
        Ok(Found::Ancestors(chain, _)) => Ok(chain),
        Ok(other) => Err(ReadError::Failed(format!(
            "an ancestor walk answered {other:?}"
        ))),
        Err(Unrun::Failed(error)) => Err(ReadError::from(error)),
        Err(Unrun::Unsupported) => Err(ReadError::Failed("no ancestors to walk".to_owned())),
    }
}

/// One navigation step of `kind` from the MSAA node `node` through the
/// executor: the neighbor read through its window's backend, or `None` at
/// a tree edge.
pub(super) fn navigate(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: NodeId,
    kind: QueryKind,
) -> Result<Option<NodeSnapshot>, ReadError> {
    verbatim_ia2::batch::step(&MsaaNodes::new(context, client), node, kind).map_err(ReadError::from)
}

/// The outpost's side of a `FocusRead` through the MSAA executor's arm
/// ([`verbatim_ia2::batch::focus_read`]): the ancestors Core knows, by
/// `known`, and what the answer reads, by `set` and `details`.
struct MsaaFocusRead<'a, 'c> {
    nodes: MsaaNodes<'a, 'c>,
    known: &'a [KnownAncestor],
    candidates: verbatim_ia2::equality::Candidates,
    set: &'a [PropertyId],
    details: TextDetails,
}

impl Resolve for MsaaFocusRead<'_, '_> {
    fn registry(&self) -> &dyn Interning {
        self.nodes.registry()
    }
    fn held(&self, node: NodeId, id: PropertyId) -> Option<PropertyValue> {
        self.nodes.held(node, id)
    }
    fn values(&self, node: &NodeSnapshot, set: &[PropertyId]) -> PropertyValues {
        self.nodes.values(node, set)
    }
    fn hops(&self) -> u32 {
        self.nodes.hops()
    }
    fn plain(&self, hwnd: isize) -> bool {
        self.nodes.plain(hwnd)
    }
    fn parent_window(&self, hwnd: isize) -> ParentWindow {
        self.nodes.parent_window(hwnd)
    }
    fn corrected(
        &self,
        from_window: Option<isize>,
        neighbor: NodeSnapshot,
        kind: QueryKind,
    ) -> NodeSnapshot {
        self.nodes.corrected(from_window, neighbor, kind)
    }
    fn children(&self, node: NodeId) -> Result<(Vec<NodeSnapshot>, bool), AcquireError> {
        self.nodes.children(node)
    }
}

impl FocusResolve for MsaaFocusRead<'_, '_> {
    fn serves(&self, hwnd: isize) -> bool {
        read::serves(self.nodes.context, hwnd)
    }

    fn candidates(&self) -> &verbatim_ia2::equality::Candidates {
        &self.candidates
    }

    fn chain(&self, node: &NodeSnapshot) -> Option<Vec<NodeSnapshot>> {
        read::msaa_enrichment(
            self.nodes.context,
            &mut self.nodes.client.borrow_mut(),
            (node, self.known, Some(&self.candidates)),
        )
        .ok()
    }

    fn cut(&self, chain: Vec<NodeSnapshot>) -> (Vec<NodeSnapshot>, Option<u32>) {
        asked::cut_at_known(self.nodes.context, chain, self.known)
    }

    fn keep_routes(&self, node: &NodeSnapshot, ancestors: &[NodeSnapshot]) {
        super::completion::keep_read_routes(
            self.nodes.context,
            std::iter::once(node).chain(ancestors),
            &[],
        );
    }

    fn wants_dialog_text(&self, node: &NodeSnapshot) -> bool {
        read::wants_dialog_text(self.nodes.context, node)
    }

    fn other_dialog_text(&self, node: &NodeSnapshot) -> Option<String> {
        read::dialog_text_of(
            self.nodes.context,
            &mut self.nodes.client.borrow_mut(),
            node,
            None,
        )
    }

    fn foreground(&self, ancestors: &[NodeSnapshot]) -> Option<NodeSnapshot> {
        read::foreground_object_among(
            self.nodes.context,
            &mut self.nodes.client.borrow_mut(),
            ancestors,
        )
    }

    fn held_ids(&self, node: &NodeSnapshot) -> Option<HeldIds> {
        asked::held_ids(self.nodes.context, node)
    }

    fn answer(&self, focus: MsaaFocus) -> FocusRead {
        let mut client = self.nodes.client.borrow_mut();
        asked::focus_read(
            (self.nodes.context, client.uia()),
            &asked::Focus {
                node: &focus.node,
                ancestors: &focus.ancestors,
                met: focus.met,
                nearest: Some(focus.nearest),
                msaa: focus.msaa,
                foreground: focus.foreground.as_ref(),
                value_from_selection: false,
                runtime_id_reused: false,
                details: self.details,
            },
            self.set,
        )
    }
}

/// The outpost's side of a `FocusRead` or a `Test` of the focus through
/// the MSAA executor, its walk up stopping at the first of `known`, by
/// `set`, its caret fetching `details`.
fn focus_resolver<'a, 'c>(
    context: &'a RequestContext<'c>,
    client: &'a mut Client,
    (known, set, details): (&'a [KnownAncestor], &'a [PropertyId], TextDetails),
) -> MsaaFocusRead<'a, 'c> {
    MsaaFocusRead {
        nodes: MsaaNodes::new(context, client),
        known,
        candidates: asked::candidates_of(known),
        set,
        details,
    }
}

/// `FocusRead` of `subject` through the MSAA executor's arm, its walk up
/// stopping at the first of `known`, by `set`, its caret fetching
/// `details`.
pub(super) fn focus_read(
    context: &RequestContext<'_>,
    client: &mut Client,
    subject: FocusSubject,
    read: (&[KnownAncestor], &[PropertyId], TextDetails),
) -> FocusFound {
    verbatim_ia2::batch::focus_read(subject, &focus_resolver(context, client, read))
}

/// `Test` of the application's focus in `hwnd` through the MSAA
/// executor's arm, with what it found for a `FocusRead` to take, read as
/// [`focus_read`] reads it.
pub(super) fn test_focus(
    context: &RequestContext<'_>,
    client: &mut Client,
    (hwnd, matcher): (isize, &verbatim_model::matcher::Matcher),
    read: (&[KnownAncestor], &[PropertyId], TextDetails),
) -> (
    FocusFound,
    Option<(bool, Vec<verbatim_model::property::PropertyAnswer>)>,
) {
    verbatim_ia2::batch::test_focus(hwnd, matcher, &focus_resolver(context, client, read))
}
