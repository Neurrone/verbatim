//! The resolvers through which the UI Automation executor's object arms
//! (`Read`, `Test`, `IsOneOf`, `Activate`, `Follow`;
//! [`verbatim_uia_rops::batch`]) reach the outpost's state: a node's
//! element, kept or found again by its route, the properties the outpost
//! holds of it, its snapshot's values, a window subject's window, a
//! listener's route found and minted, and the focus registration. The
//! arms make every call; these resolve and convert, with no call of their
//! own but the routes' walks and the text summary's read.

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
    /// Its text, opened once asked, for a `Read` of its text summary.
    text: OnceCell<Result<crate::text::uia::UiaText, verbatim_model::TextFailure>>,
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
            text: OnceCell::new(),
        }
    }

    /// Its text, opened at the first ask: the step's outcome when it has
    /// none, is gone, or did not answer.
    fn source(&self) -> Result<&crate::text::uia::UiaText, Ran> {
        use verbatim_model::TextFailure;
        match self.text.get_or_init(|| {
            super::text_reads::uia_text(self.context, self.node, verbatim_model::TextDetails::NONE)
        }) {
            Ok(source) => Ok(source),
            Err(TextFailure::NoText) => Err(Ran::Unrun(Unrun::Refused(
                verbatim_model::batch::StepRefusal::NoText,
            ))),
            Err(TextFailure::Gone) => Err(Ran::Unrun(Unrun::NotThere(Missing::Node(self.node)))),
            Err(_) => Err(Ran::Failed),
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
        held(self.context, self.node, id)
    }

    fn text(&self, _subject: &Subject) -> Result<verbatim_uia_rops::TextTarget<'_>, Ran> {
        self.source().map(crate::text::uia::UiaText::target)
    }

    fn units(
        &self,
        read: (
            Option<verbatim_model::TextMovement>,
            verbatim_model::TextUnit,
            u16,
        ),
        details: verbatim_model::TextDetails,
    ) -> Result<verbatim_uia_rops::batch::UnitsRead, Ran> {
        crate::text::fused::units_read(self.source()?, read, (details, false))
    }

    fn text_summary(
        &self,
        (line, selected): (&[u16], Option<&[u16]>),
        read_classically: bool,
    ) -> Option<PropertyValue> {
        if read_classically && let Some(window) = asked::known_nearest_of(self.context, self.node) {
            self.context.read_classically(window);
        }
        Some(PropertyValue::TextSummary(crate::text::summary_of(
            line, selected,
        )))
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

/// Property `id` of node `node` that the outpost answers from what it
/// holds rather than from the node's own read: its MSAA ids, whether its
/// window, its own or its nearest ancestor's, is shown, and that window's
/// class, from Windows; a tree view item's children, by the control's
/// messages; and an edit control's text summary, through its messages (a
/// UI Automation node's is the executor's `Read` arm's).
pub(super) fn held(
    context: &RequestContext<'_>,
    node: NodeId,
    id: PropertyId,
) -> Option<PropertyValue> {
    let window =
        || read::kept_own_window(context, node).or_else(|| asked::known_nearest_of(context, node));
    match id {
        PropertyId::MsaaAddress => context.store.key_of(node).map(|(window, object, child)| {
            PropertyValue::MsaaAddress(HeldIds {
                event: MsaaIds {
                    window: window_handle(window),
                    object,
                    child,
                },
                held_child: child,
            })
        }),
        PropertyId::WindowShown => window()
            .map(|hwnd| PropertyValue::Flag(verbatim_win32::window::window_is_visible(hwnd))),
        PropertyId::WindowClass => window()
            .map(|hwnd| PropertyValue::Text(verbatim_win32::window::window_class_name(hwnd))),
        PropertyId::ChildCount => context
            .store
            .key_of(node)
            .and_then(|(hwnd, _, child)| verbatim_ia2::acquire::tree_view_child_count(hwnd, child))
            .map(PropertyValue::Count),
        PropertyId::TextSummary => {
            verbatim_ia2::text::summary(&super::text_reads::EditFacts(context), node)
                .map(PropertyValue::TextSummary)
        }
        _ => None,
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

/// A window's own element, as a `Read` or `Test` of a window subject
/// resolves it.
struct UiaWindow<'a, 'c> {
    context: &'a RequestContext<'c>,
    hwnd: isize,
    fill: Fill<Node>,
}

impl Resolve for UiaWindow<'_, '_> {
    fn window(&self, _subject: &Subject) -> Option<isize> {
        Some(self.hwnd)
    }

    fn fill(&self, _place: Place) -> Option<&Fill<Node>> {
        Some(&self.fill)
    }

    fn node_value(&self, node: &Cached<Node>, id: PropertyId) -> Option<PropertyValue> {
        asked::values(self.context, &node.snapshot(self.context.nodes()), &[id])
            .0
            .pop()
            .and_then(|answer| answer.value)
    }
}

/// A listener's route, as a `Read` of it resolves it.
struct RouteObject<'a, 'c> {
    context: &'a RequestContext<'c>,
    client: std::cell::RefCell<&'a mut read::Client>,
    route: &'a verbatim_model::request::Route,
}

impl Resolve for RouteObject<'_, '_> {
    fn route(&self, _subject: &Subject) -> Result<(verbatim_model::NodeSnapshot, isize), Ran> {
        super::completion::resolve_route(self.context, &mut self.client.borrow_mut(), self.route)
            .ok_or(Ran::Unrun(Unrun::NotThere(Missing::Object)))
    }
}

/// `op`, a `Read` or `Test` of window `hwnd`'s own element, through the
/// executor's call arm: its outcome, a read carrying the element's node
/// and the window.
pub(super) fn window_step(
    context: &RequestContext<'_>,
    client: &mut read::Client,
    hwnd: isize,
    op: &Op,
) -> StepOutcome {
    use verbatim_model::batch::{Done, Tested};
    use verbatim_model::request::NodeRead;
    use verbatim_uia_rops::batch::Found;
    let Ok((uia, nodes)) = client.uia_with(context, RequestContext::node_request) else {
        return StepOutcome::Failed;
    };
    let resolver = UiaWindow {
        context,
        hwnd,
        fill: Fill::new(&nodes),
    };
    let ran = run(context, (uia, Some(hwnd)), &resolver, op.clone());
    outcome(ran, None, |found| match found {
        Found::Window(element) => Some(StepOutcome::Done(Done::Read(asked::node_read(
            context,
            &element.snapshot(context.nodes()),
            Some(hwnd),
            match op {
                Op::Read { set, .. } => set,
                _ => &[],
            },
        )))),
        Found::Test(tested) => Some(StepOutcome::Done(Done::Test(Tested {
            matched: tested.matched,
            node: NodeRead {
                node: None,
                window: asked::node_window(hwnd),
                values: PropertyValues(tested.values),
            },
        }))),
        _ => None,
    })
}

/// Window `hwnd`'s own element, through the executor's `Read` arm: `None`
/// when it cannot be read.
pub(super) fn window(
    context: &RequestContext<'_>,
    client: &mut read::Client,
    hwnd: isize,
) -> Option<verbatim_model::NodeSnapshot> {
    let op = Op::Read {
        subject: Subject::Window {
            which: verbatim_model::batch::WindowOf::Handle(window_handle(hwnd)),
            backend: verbatim_model::Backend::Uia,
            visible: false,
        },
        set: Vec::new(),
    };
    let (uia, nodes) = client
        .uia_with(context, RequestContext::node_request)
        .ok()?;
    let resolver = UiaWindow {
        context,
        hwnd,
        fill: Fill::new(&nodes),
    };
    match run(context, (uia, Some(hwnd)), &resolver, op) {
        Ran::Done(verbatim_uia_rops::batch::Found::Window(element)) => {
            Some(element.snapshot(context.nodes()))
        }
        _ => None,
    }
}

/// `op`, a `Read` of the element `route` names, through the executor's
/// call arm: what the listener read of it, as its node, carrying the
/// route's window.
pub(super) fn route_read(
    context: &RequestContext<'_>,
    client: &mut read::Client,
    route: &verbatim_model::request::Route,
    op: &Op,
) -> StepOutcome {
    use verbatim_model::batch::Done;
    use verbatim_uia_rops::batch::Found;
    let Some(uia) = client.shared_uia() else {
        return StepOutcome::Failed;
    };
    let resolver = RouteObject {
        context,
        client: std::cell::RefCell::new(client),
        route,
    };
    let set = match op {
        Op::Read { set, .. } => set.as_slice(),
        _ => &[],
    };
    let ran = run(context, (&uia, None), &resolver, op.clone());
    outcome(ran, None, |found| match found {
        Found::Route(read, window) => Some(StepOutcome::Done(Done::Read(asked::node_read(
            context,
            &read,
            Some(window),
            set,
        )))),
        _ => None,
    })
}

/// What a step that may run as a program answers from a UI Automation
/// node: the executor's result, or the step's outcome when it answers
/// none.
pub(super) type Programmed = Result<verbatim_uia_rops::batch::Found, StepOutcome>;

/// `op`, a `Related` by the controller-for relation or to the first
/// selected item, or a `Descendants`, from the UI Automation node `node`,
/// alone through the executor, by a program where the window allows one:
/// the element the outpost keeps, its own window and its nearest, each
/// node reached filled by the walk's request and judged by `judge`.
/// `Err` with the whole batch's outcome when the application did not
/// answer within UI Automation's transaction timeout, which a program
/// fails with no fallback; a program that could not run at all has its
/// window read classically from now on.
pub(super) fn programmed(
    context: &RequestContext<'_>,
    client: &mut read::Client,
    (node, op): (NodeId, Op),
    judge: Option<read::Judge<'_>>,
) -> Result<Programmed, verbatim_model::request::Outcome> {
    let Some(element) = context
        .store
        .element_of(node)
        .and_then(|agile| agile.resolve().ok())
    else {
        return Ok(Err(StepOutcome::NotThere(Missing::Node(node))));
    };
    let request = match &op {
        Op::Descendants { walk, .. } if walk.set.contains(&PropertyId::Rect) => {
            RequestContext::located_many_node_request
        }
        Op::Descendants { .. } => RequestContext::many_node_request,
        _ => RequestContext::node_request,
    };
    let Ok((uia, nodes)) = client.uia_with(context, request) else {
        return Ok(Err(StepOutcome::Failed));
    };
    let fill = Fill::new(&nodes);
    let window = asked::known_nearest_of(context, node);
    let from = read::OneElement {
        element: element.live().element(),
        window: read::kept_own_window(context, node).unwrap_or(0),
        calls_only: false,
        fill: &fill,
        judge,
    };
    let answer = from.run(context, uia, op, window);
    if answer.read_classically
        && let Some(window) = window
    {
        context.read_classically(verbatim_win32::window::top_level_of(window));
    }
    Ok(match answer.steps.into_iter().next() {
        Some(Ran::Done(found)) => Ok(found),
        Some(Ran::Gone) => Err(StepOutcome::NotThere(Missing::Node(node))),
        Some(Ran::TimedOut) => return Err(verbatim_model::request::Outcome::NotResponding),
        _ => Err(StepOutcome::Failed),
    })
}

/// Runs `op`, an object step, alone through the UI Automation executor,
/// resolved by `resolver`, its subject in `window` when known: its one
/// step's outcome.
fn run(
    context: &RequestContext<'_>,
    (uia, window): (&Uia, Option<isize>),
    resolver: &dyn Resolve,
    op: Op,
) -> Ran {
    let steps = [Step { op, when: None }];
    let answer = verbatim_uia_rops::batch::run_batch(&verbatim_uia_rops::batch::Segment {
        uia,
        steps: &steps,
        first: 0,
        earlier: &[],
        resolver,
        remote: context.tries_remote(window),
        window,
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
    let window = asked::known_nearest_of(context, node);
    let ran = run(context, (uia, window), &resolver, with_subject(op, node));
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
        (uia, None),
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
    let ran = run(context, (uia, None), &resolver, Op::Follow(target.cloned()));
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
