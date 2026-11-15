//! Core's requests to the readers, their answers and their cancels (ADR
//! 0003, "Readers tell, Core asks"; "Properties, event kinds and
//! requests"; "A frozen window").
//!
//! Every call into an application is made for a request from Core; the
//! request carries what it needs and gets one answer, with one of five
//! outcomes ([`Outcome`]). One request list is shared by Core's own
//! features and the extension surface ([`Request`]): properties of an
//! object, navigation, find a descendant by property, a node's children,
//! text reads and actions, and one generic read of the object an event
//! names ([`ReadObject`]). A new command adds no kind of request.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::property::{PropertyId, PropertyValue, PropertyValues, RuntimeId};
use crate::tell::ElementToken;
use crate::{ActionName, NodeId, QueryKind, TextOp, TextReply, WindowHandle};

/// Core's number for one request, unique for the run, by which its answer
/// is routed and a late answer dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RequestNumber(pub u64);

impl fmt::Display for RequestNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "request {}", self.0)
    }
}

/// A request as Core sends it to a reader.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    /// Its number.
    pub number: RequestNumber,
    /// The window it names, which Core's shell checks for a hang before
    /// sending it and the foreground rule tests.
    pub window: WindowHandle,
    /// Whether the reader probes the window for a UI Automation provider
    /// before the read, Core having no verdict in force for it.
    pub probe_first: bool,
    /// What is asked.
    pub request: Request,
}

/// The object a read names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectRef {
    /// A UI Automation element the listener kept for a tell.
    Kept(ElementToken),
    /// An MSAA object, by the ids its event named.
    Msaa {
        /// The window.
        window: WindowHandle,
        /// The object id.
        object: i32,
        /// The child id.
        child: i32,
    },
    /// A node an outpost holds.
    Node(NodeId),
}

/// An ancestor Core already knows, at which a walk up the ancestor chain
/// stops: one of the committed focus's ancestors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KnownAncestor {
    /// A UI Automation element, by its runtime id.
    Uia(RuntimeId),
    /// A window, matched by its handle.
    Window(WindowHandle),
    /// A node of the answering outpost generation.
    Node(NodeId),
}

/// What a read finds out of the object's relation to nodes Core names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationQuery {
    /// Whether the object is one of these nodes: the committed focus's
    /// ancestors.
    OneOf(Vec<NodeId>),
    /// Whether the object is, or is inside, an object the focus controls,
    /// read live from the focus's controller-for relation.
    ControlledBy(NodeId),
}

/// The object's relation, as a read found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationFound {
    /// It is the node at this place in [`RelationQuery::OneOf`]'s list.
    Is(u32),
    /// It is an object the focus controls.
    Controlled,
    /// It is inside an object the focus controls.
    InsideControlled,
    /// None of these.
    Unrelated,
}

/// Why a read of an event's object is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Purpose {
    /// A focus read, whose element becomes "read, not yet named"; it names
    /// the readers' last focus's token, which the reader keeps.
    Focus {
        /// The readers' last focus's token, `None` while it is empty or
        /// not this reader's.
        keep: Option<ElementToken>,
    },
    /// Any other read.
    Other,
}

/// The generic read: the properties of the object an event names, by
/// property set, optionally reading up its ancestors until it reaches an
/// ancestor Core already knows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadObject {
    /// The object.
    pub object: ObjectRef,
    /// The properties to read: a set fixed at compile time
    /// ([`crate::property::PropertySet::PROPERTIES`]).
    pub set: Vec<PropertyId>,
    /// Its relation to nodes Core names, when asked.
    pub relation: Option<RelationQuery>,
    /// Whether its descendants are read too, as an alert's are.
    pub descendants: bool,
    /// The ancestors Core already knows, when the read walks up to them;
    /// an empty list walks to the top. `None` reads no ancestors.
    pub walk: Option<Vec<KnownAncestor>>,
    /// Why it is read.
    pub purpose: Purpose,
}

/// How far a find looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindScope {
    /// The node's children.
    Children,
    /// The node's descendants.
    Descendants,
}

/// One request of the shared list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    /// The generic read of the object an event names.
    ReadObject(ReadObject),
    /// The properties of a node, by set.
    Properties {
        /// The node.
        node: NodeId,
        /// The set.
        set: Vec<PropertyId>,
    },
    /// One navigation step from a node.
    Navigate {
        /// The node.
        node: NodeId,
        /// Which way.
        direction: QueryKind,
        /// The properties of the node reached.
        set: Vec<PropertyId>,
    },
    /// The first child or descendant of a node with a property's value.
    Find {
        /// The node.
        node: NodeId,
        /// How far to look.
        scope: FindScope,
        /// The property matched.
        property: PropertyId,
        /// The value it must have.
        value: PropertyValue,
        /// The properties of the node found.
        set: Vec<PropertyId>,
    },
    /// A node's children.
    Children {
        /// The node.
        node: NodeId,
        /// The properties of each child.
        set: Vec<PropertyId>,
    },
    /// A read or move in a node's text.
    Text {
        /// The node.
        node: NodeId,
        /// What to do.
        op: TextOp,
    },
    /// Activate a node: its default action.
    Activate {
        /// The node.
        node: NodeId,
    },
}

/// A node an answer read, minted by the outpost that read it, with its
/// nearest window, which a request on the node names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRead {
    /// The node, `None` when the reader mints no node (the listener).
    pub node: Option<NodeId>,
    /// Its own window or its nearest ancestor's.
    pub window: WindowHandle,
    /// The properties read.
    pub values: PropertyValues,
}

/// The answer to [`ReadObject`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRead {
    /// The object.
    pub object: NodeRead,
    /// Its relation, when asked.
    pub relation: Option<RelationFound>,
    /// Its descendants, when asked.
    pub descendants: Vec<NodeRead>,
    /// Its ancestors, nearest first, up to the ancestor Core already knows
    /// that the walk met, which is not among them.
    pub ancestors: Vec<NodeRead>,
    /// The place, in the request's list, of the known ancestor the walk
    /// met; `None` when it walked to the top.
    pub met: Option<u32>,
}

/// A result: the first of the five outcomes, by request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    /// The answer to [`Request::ReadObject`].
    Object(Box<ObjectRead>),
    /// The answer to [`Request::Properties`].
    Properties(PropertyValues),
    /// The answer to [`Request::Navigate`]: the node reached, `None` at a
    /// genuine edge of the tree.
    Navigated(Option<NodeRead>),
    /// The answer to [`Request::Find`]: the node found, `None` when none
    /// matches.
    Found(Option<NodeRead>),
    /// The answer to [`Request::Children`].
    Children(Vec<NodeRead>),
    /// The answer to [`Request::Text`].
    Text(TextReply),
    /// The answer to [`Request::Activate`]: the action's name, when it has
    /// one.
    Activated(Option<ActionName>),
}

/// Why a reader refused a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// A focus read found the object without the focused state.
    NotFocused,
    /// The probe found the window read through UI Automation, so an MSAA
    /// read of it is not made.
    ReadThroughUia,
    /// The object does not support what was asked.
    Unsupported,
}

/// The five outcomes of a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// A result.
    Result(Reply),
    /// Refused, with a reason.
    Refused(Refusal),
    /// The object is not there.
    NotThere,
    /// Not responding.
    NotResponding,
    /// The reader ended or stopped.
    Ended,
}

/// A reader's answer to one request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// The request's number.
    pub number: RequestNumber,
    /// Its outcome.
    pub outcome: Outcome,
    /// A diagnostic detail for the log, such as the failed call; empty
    /// when there is none. Core judges nothing by it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// Why Core stopped waiting for a request, or withdrew it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CancelReason {
    /// The foreground moved to a window outside the request's window's
    /// tree and owner chain.
    Foreground,
    /// Core's live check found the request's window hung.
    Hung,
    /// The request's due time passed.
    DueTime,
    /// Its entry no longer needs it.
    Withdrawn,
}

impl CancelReason {
    /// Whether a reader whose thread is still inside the request's call
    /// abandons that thread and starts a fresh one: only when the
    /// foreground moved away, since a thread in a hung or slow window would
    /// stick again, and a withdrawal must not cost a thread.
    #[must_use]
    pub const fn abandons_call(self) -> bool {
        matches!(self, Self::Foreground)
    }
}

/// Core tells a reader to cancel a request: a request not yet started is
/// dropped, whatever the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancel {
    /// The request's number.
    pub number: RequestNumber,
    /// Why.
    pub reason: CancelReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_foreground_cancel_abandons_a_running_call() {
        assert!(CancelReason::Foreground.abandons_call());
        assert!(!CancelReason::Hung.abandons_call());
        assert!(!CancelReason::DueTime.abandons_call());
        assert!(!CancelReason::Withdrawn.abandons_call());
    }

    #[test]
    fn a_generic_read_and_its_answer_travel_as_json_and_back() {
        let ask = Ask {
            number: RequestNumber(7),
            window: WindowHandle(5),
            probe_first: false,
            request: Request::ReadObject(ReadObject {
                object: ObjectRef::Kept(ElementToken(3)),
                set: vec![PropertyId::Name, PropertyId::Role],
                relation: Some(RelationQuery::OneOf(vec![NodeId::new(4)])),
                descendants: false,
                walk: Some(vec![KnownAncestor::Uia(RuntimeId(vec![42, 1]))]),
                purpose: Purpose::Focus {
                    keep: Some(ElementToken(2)),
                },
            }),
        };
        let json = serde_json::to_string(&ask).unwrap();
        assert_eq!(serde_json::from_str::<Ask>(&json).unwrap(), ask);

        let answer = Answer {
            number: RequestNumber(7),
            outcome: Outcome::Result(Reply::Object(Box::new(ObjectRead {
                object: NodeRead {
                    node: None,
                    window: WindowHandle(5),
                    values: PropertyValues::default(),
                },
                relation: Some(RelationFound::Unrelated),
                descendants: Vec::new(),
                ancestors: Vec::new(),
                met: Some(0),
            }))),
            detail: String::new(),
        };
        let json = serde_json::to_string(&answer).unwrap();
        assert!(!json.contains("detail"));
        assert_eq!(serde_json::from_str::<Answer>(&json).unwrap(), answer);
    }

    #[test]
    fn a_cancel_travels_with_its_reason() {
        let cancel = Cancel {
            number: RequestNumber(9),
            reason: CancelReason::Withdrawn,
        };
        let json = serde_json::to_string(&cancel).unwrap();
        assert_eq!(serde_json::from_str::<Cancel>(&json).unwrap(), cancel);
    }
}
