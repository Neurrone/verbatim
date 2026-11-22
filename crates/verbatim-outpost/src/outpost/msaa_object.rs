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
use verbatim_ia2::batch::{Found, Object, Resolve, Unrun};
use verbatim_model::batch::{Done, Missing, Op, StepOutcome, StepRefusal, Tested};
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::NodeRead;
use verbatim_model::{NodeId, NodeSnapshot, QueryKind};

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
/// holds them, `window` for an acquired object.
pub(super) fn step(
    context: &RequestContext<'_>,
    client: &mut Client,
    (object, window): (Object<'_>, isize),
    op: &Op,
) -> StepOutcome {
    let node = match &object {
        Object::Node(node) => Some(*node),
        Object::Candidate(_) => None,
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
        Op::Related { set, .. } => set.as_slice(),
        _ => &[],
    };
    match found {
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
        Err(failure) => unrun(failure, node),
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
