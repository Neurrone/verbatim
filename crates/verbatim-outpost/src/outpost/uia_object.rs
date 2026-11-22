//! The resolvers through which the UI Automation executor's object arms
//! (`Read`, `Test`, `IsOneOf`, `Activate`, `Follow`;
//! [`verbatim_uia_rops::batch`]) reach the outpost's state: a node's
//! element, kept or found again by its route, the properties the outpost
//! holds of it, its snapshot's values, and the focus registration. The
//! arms make every call; these resolve and convert, with no call of their
//! own but the route's walk and the text summary's read.

#![forbid(unsafe_code)]

use std::cell::OnceCell;

use verbatim_model::NodeId;
use verbatim_model::batch::{FollowTarget, Missing, Op, Place, Step, StepOutcome, Subject};
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{Followed, HeldIds, MsaaIds};
use verbatim_uia::{Cached, Fill, Node, Request, Uia};
use verbatim_uia_rops::batch::{Element, ElementRead, Ran, Resolve, Unrun};
use windows::Win32::UI::Accessibility::IUIAutomationElement;

use super::RequestContext;
use super::asked;
use super::read::{self, ReadError};
use super::window::window_handle;

/// The UI Automation node one object step runs on, resolved by the
/// dispatcher: its element filled as the step's fill asks, kept or found
/// again by its route.
pub(super) struct UiaNode<'a, 'c> {
    context: &'a RequestContext<'c>,
    uia: &'a Uia,
    node: NodeId,
    fill: Fill<Node>,
    /// The element the outpost keeps, or found by its route, once asked.
    element: OnceCell<IUIAutomationElement>,
}

impl<'a, 'c> UiaNode<'a, 'c> {
    /// Node `node`, its reads filled by `request`.
    pub(super) fn new(
        context: &'a RequestContext<'c>,
        uia: &'a Uia,
        node: NodeId,
        request: &Request<Node>,
    ) -> Self {
        Self {
            context,
            uia,
            node,
            fill: Fill::new(request),
            element: OnceCell::new(),
        }
    }

    /// A read's failure as the step's outcome: not there for a node gone,
    /// else failed, its reason logged.
    fn failed(&self, error: ReadError) -> Ran {
        match error {
            ReadError::Gone => Ran::Unrun(Unrun::NotThere(Missing::Node(self.node))),
            ReadError::Failed(reason) => {
                tracing::debug!(reason, "a batch step's read failed");
                Ran::Failed
            }
        }
    }

    /// The node's window, its own or its nearest ancestor's, as the
    /// outpost holds it.
    fn window(&self) -> Option<isize> {
        read::kept_own_window(self.context, self.node)
            .or_else(|| asked::known_nearest_of(self.context, self.node))
    }
}

impl Resolve for UiaNode<'_, '_> {
    fn object(&self, _subject: &Subject) -> Result<Element<'_>, Ran> {
        if self.element.get().is_none() {
            let kept = self
                .context
                .store
                .element_of(self.node)
                .and_then(|agile| agile.resolve().ok());
            let element = match kept {
                Some(kept) => kept.live().element().clone(),
                None => read::resolve_uia_element(
                    self.context,
                    self.uia,
                    self.fill.request(),
                    self.node,
                )
                .map_err(|error| self.failed(error))?
                .live()
                .element()
                .clone(),
            };
            let _ = self.element.set(element);
        }
        let element = self.element.get().ok_or(Ran::Failed)?;
        Ok(Element {
            element,
            window: 0,
            calls_only: true,
        })
    }

    fn fill(&self, _place: Place) -> Option<&Fill<Node>> {
        Some(&self.fill)
    }

    fn filled(&self, _subject: &Subject, _place: Place) -> Result<Cached<Node>, Ran> {
        read::resolve_uia_element(self.context, self.uia, self.fill.request(), self.node)
            .map_err(|error| self.failed(error))
    }

    fn node_id(&self, _subject: &Subject) -> Option<NodeId> {
        Some(self.node)
    }

    fn held(&self, _subject: &Subject, id: PropertyId) -> Option<PropertyValue> {
        let context = self.context;
        match id {
            PropertyId::MsaaAddress => {
                context
                    .store
                    .key_of(self.node)
                    .map(|(window, object, child)| {
                        PropertyValue::MsaaAddress(HeldIds {
                            event: MsaaIds {
                                window: window_handle(window),
                                object,
                                child,
                            },
                            held_child: child,
                        })
                    })
            }
            PropertyId::WindowShown => self
                .window()
                .map(|hwnd| PropertyValue::Flag(verbatim_win32::window::window_is_visible(hwnd))),
            PropertyId::WindowClass => self
                .window()
                .map(|hwnd| PropertyValue::Text(verbatim_win32::window::window_class_name(hwnd))),
            PropertyId::ChildCount => context
                .store
                .key_of(self.node)
                .and_then(|(hwnd, _, child)| {
                    verbatim_ia2::acquire::tree_view_child_count(hwnd, child)
                })
                .map(PropertyValue::Count),
            PropertyId::TextSummary => {
                super::text_reads::text_summary(context, Some(self.uia), self.node)
                    .map(PropertyValue::TextSummary)
            }
            _ => None,
        }
    }

    fn value(&self, read: &ElementRead, id: PropertyId) -> Option<PropertyValue> {
        let mut snapshot = read.node.snapshot(self.context.nodes());
        snapshot.details.set_table(read.table);
        snapshot.states = read.states;
        if let Some(value) = &read.selected {
            snapshot.value.clone_from(value);
        }
        asked::values(self.context, &snapshot, &[id])
            .0
            .pop()
            .and_then(|answer| answer.value)
    }
}

/// The focus registration a `Follow` moves, and the node it names,
/// resolved by the dispatcher.
pub(super) struct Following<'a, 'c> {
    context: &'a RequestContext<'c>,
    uia: &'a Uia,
    followed: Option<Followed>,
}

impl Resolve for Following<'_, '_> {
    fn follow(&self, _target: Option<&FollowTarget>) -> Result<(), Ran> {
        let named = self.followed.as_ref().map(|followed| followed.node);
        match super::followed::follow(self.context, self.uia, self.followed.as_ref()) {
            super::followed::Moved::Done => Ok(()),
            super::followed::Moved::NotThere => Err(Ran::Unrun(Unrun::NotThere(
                named.map_or(Missing::Object, Missing::Node),
            ))),
            super::followed::Moved::Unregistered => Err(Ran::Failed),
        }
    }
}

/// Runs `op`, an object step, alone through the UI Automation executor,
/// resolved by `resolver`: its one step's outcome.
fn run(context: &RequestContext<'_>, uia: &Uia, resolver: &dyn Resolve, op: Op) -> Ran {
    let steps = [Step { op, when: None }];
    let answer = verbatim_uia_rops::batch::run_batch(&verbatim_uia_rops::batch::Segment {
        uia,
        steps: &steps,
        first: 0,
        earlier: &[],
        resolver,
        remote: context.tries_remote(None),
        window: None,
    });
    answer.steps.into_iter().next().unwrap_or(Ran::Failed)
}

/// The outcome of an object step on node `node` that ran `ran`, its result
/// made by `done`.
fn outcome(
    ran: Ran,
    node: Option<NodeId>,
    done: impl FnOnce(verbatim_uia_rops::batch::Found) -> Option<StepOutcome>,
) -> StepOutcome {
    match ran {
        Ran::Done(found) => done(found).unwrap_or_else(|| {
            tracing::error!(target: verbatim_model::DEFECT_TARGET, "an object step answered another step's result");
            StepOutcome::Failed
        }),
        Ran::Unrun(Unrun::NotThere(missing)) => StepOutcome::NotThere(missing),
        Ran::Unrun(Unrun::Refused(refusal)) => StepOutcome::Refused(refusal),
        Ran::Unrun(Unrun::Skipped) => StepOutcome::Skipped,
        Ran::Gone => StepOutcome::NotThere(node.map_or(Missing::Object, Missing::Node)),
        // A read that timed out is a failed read, as every read of a node
        // outside a program is.
        Ran::TimedOut | Ran::Failed => StepOutcome::Failed,
    }
}

/// `op`, a `Read`, `Test` or `IsOneOf` on the UI Automation node `node`,
/// through the executor's call arm: its outcome, the node's read carrying
/// its nearest window as the outpost holds it.
pub(super) fn step(context: &RequestContext<'_>, uia: &Uia, node: NodeId, op: &Op) -> StepOutcome {
    use verbatim_model::batch::{Done, Tested};
    use verbatim_model::request::NodeRead;
    use verbatim_uia_rops::batch::Found;
    let request = match op {
        Op::Read { .. } | Op::Test { .. } => context.read_node_request(uia),
        _ => context.node_request(uia),
    };
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            tracing::debug!(%error, "could not build a cache request");
            return StepOutcome::Failed;
        }
    };
    let resolver = UiaNode::new(context, uia, node, &request);
    let ran = run(context, uia, &resolver, with_subject(op, node));
    let read = |values| NodeRead {
        node: Some(node),
        window: asked::node_window(asked::known_nearest_of(context, node).unwrap_or(0)),
        values,
    };
    outcome(ran, Some(node), |found| match found {
        Found::Read(values) => Some(StepOutcome::Done(Done::Read(read(values)))),
        Found::Test(tested) => Some(StepOutcome::Done(Done::Test(Tested {
            matched: tested.matched,
            node: read(PropertyValues(tested.values)),
        }))),
        Found::IsOneOf(place) => Some(StepOutcome::Done(Done::IsOneOf(place))),
        _ => None,
    })
}

/// Activates exactly the UI Automation node `node` through the executor's
/// call arm, answering the action's name.
pub(super) fn activate(
    context: &RequestContext<'_>,
    uia: &Uia,
    node: NodeId,
) -> Result<Option<verbatim_model::ActionName>, ReadError> {
    use verbatim_uia_rops::batch::Found;
    let request = context
        .node_request(uia)
        .map_err(|error| ReadError::Failed(format!("could not build a cache request: {error}")))?;
    let resolver = UiaNode::new(context, uia, node, &request);
    match run(
        context,
        uia,
        &resolver,
        Op::Activate {
            subject: Subject::Node(node),
        },
    ) {
        Ran::Done(Found::Activate(action)) => Ok(action),
        Ran::Unrun(Unrun::NotThere(_)) | Ran::Gone => Err(ReadError::Gone),
        other => Err(ReadError::Failed(format!(
            "UIA activation failed: {other:?}"
        ))),
    }
}

/// `Follow` of `followed`, or of nothing, through the executor's call arm.
pub(super) fn follow(
    context: &RequestContext<'_>,
    uia: &Uia,
    (target, followed): (Option<&FollowTarget>, Option<Followed>),
) -> StepOutcome {
    use verbatim_model::batch::Done;
    use verbatim_uia_rops::batch::Found;
    let node = followed.as_ref().map(|followed| followed.node);
    let resolver = Following {
        context,
        uia,
        followed,
    };
    let ran = run(context, uia, &resolver, Op::Follow(target.cloned()));
    outcome(ran, node, |found| match found {
        Found::Follow => Some(StepOutcome::Done(Done::Follow)),
        _ => None,
    })
}

/// `op` with its subject the node `node` the dispatcher resolved it to.
fn with_subject(op: &Op, node: NodeId) -> Op {
    let subject = Subject::Node(node);
    match op {
        Op::Read { set, .. } => Op::Read {
            subject,
            set: set.clone(),
        },
        Op::Test { matcher, .. } => Op::Test {
            subject,
            matcher: matcher.clone(),
        },
        Op::IsOneOf {
            equal, at_address, ..
        } => Op::IsOneOf {
            subject,
            equal: *equal,
            at_address: at_address.clone(),
        },
        op => op.clone(),
    }
}
