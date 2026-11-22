//! The MSAA executor: the request list's object operations on MSAA
//! subjects, `Read`, `Test`, `Related`, `Descendants`, `IsOneOf` and
//! `Activate`, each one step at a time by calls, as the UI Automation
//! executor's call arms run theirs (the verbatim-ia2 crate guide, "The
//! MSAA executor"). Its subject is a node the outpost holds or an object
//! acquired at its event's ids ([`Object`]); what the outpost holds, the
//! backend a reached object is read through, and a node MSAA does not list
//! the children of, it resolves ([`Resolve`]).

use verbatim_model::batch::{Op, Relation, Walk};
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{HeldIds, MsaaIds};
use verbatim_model::{ActionName, NodeId, NodeSnapshot, QueryKind, WindowHandle};

use crate::Interning;
use crate::acquire::{self, AcquireError, FocusCandidate, ParentWindow};

/// The most children read of one node.
const MAX_CHILDREN: usize = 1000;

/// What a step runs on.
pub enum Object<'a> {
    /// A node the outpost holds.
    Node(NodeId),
    /// An object acquired at its event's ids, read once each property.
    Candidate(&'a mut FocusCandidate),
}

/// What the outpost resolves for the MSAA executor: the node registry, the
/// properties it holds of a node, a snapshot's values, the window facts
/// the focused-state test asks, the backend a reached object is read
/// through, and the children of a node MSAA does not list.
pub trait Resolve {
    /// The registry the outpost's MSAA nodes are minted in.
    fn registry(&self) -> &dyn Interning;

    /// Property `id` of node `node` that the outpost answers from what it
    /// holds: its MSAA ids, whether its window is shown, its window's
    /// class, and its text's summary.
    fn held(&self, node: NodeId, id: PropertyId) -> Option<PropertyValue>;

    /// The properties of `set` of `node`, as its snapshot holds them.
    fn values(&self, node: &NodeSnapshot, set: &[PropertyId]) -> PropertyValues;

    /// How many windows the focused-state test climbs at most.
    fn hops(&self) -> u32;

    /// Whether window `hwnd` reads as a plain window, for the
    /// focused-state test.
    fn plain(&self, hwnd: isize) -> bool;

    /// Window `hwnd` as the focused-state test goes on from it.
    fn parent_window(&self, hwnd: isize) -> ParentWindow;

    /// `neighbor`, reached by a step of `kind` from a node whose window is
    /// `from_window`, read through the backend its window uses.
    fn corrected(
        &self,
        from_window: Option<isize>,
        neighbor: NodeSnapshot,
        kind: QueryKind,
    ) -> NodeSnapshot;

    /// The children of `node`, which MSAA does not list (a window's own
    /// object, a tree view's item, or a node read through UI Automation),
    /// by steps to its first child and on to each next sibling, each on
    /// the backend of the node it goes from, and whether they are all of
    /// them.
    ///
    /// # Errors
    ///
    /// The read's error when the node or its first child cannot be read.
    fn children(&self, node: NodeId) -> Result<(Vec<NodeSnapshot>, bool), AcquireError>;
}

/// A step's result.
#[derive(Debug)]
pub enum Found {
    /// `Read`'s: the properties of its set, in its order.
    Read(PropertyValues),
    /// `Test`'s: whether its matcher held, and each property read, in the
    /// order the matcher asked for it.
    Test(bool, Vec<PropertyAnswer>),
    /// `Related`'s: the objects reached, none at an edge.
    Related(Vec<NodeSnapshot>),
    /// `IsOneOf`'s: the place of the object met among those named.
    IsOneOf(Option<u32>),
    /// `Activate`'s: the action performed.
    Activate(Option<ActionName>),
}

/// Why a step answers no result.
#[derive(Debug, PartialEq, Eq)]
pub enum Unrun {
    /// The read failed: the object is gone, or another failure.
    Failed(AcquireError),
    /// The operation does not run on this subject.
    Unsupported,
}

impl From<AcquireError> for Unrun {
    fn from(error: AcquireError) -> Self {
        Self::Failed(error)
    }
}

/// Runs `op` on `object` by calls: its result.
///
/// # Errors
///
/// [`Unrun`] when it answers none.
pub fn run(op: &Op, object: Object<'_>, resolver: &dyn Resolve) -> Result<Found, Unrun> {
    match (op, object) {
        (Op::Read { set, .. }, Object::Node(node)) => Ok(Found::Read(read(resolver, node, set)?)),
        (Op::Read { set, .. }, Object::Candidate(candidate)) => Ok(Found::Read(PropertyValues(
            set.iter()
                .map(|&id| PropertyAnswer {
                    id,
                    value: candidate_property(resolver, candidate, id),
                })
                .collect(),
        ))),
        (Op::Test { matcher, .. }, Object::Node(node)) => test(resolver, node, matcher),
        (Op::Test { matcher, .. }, Object::Candidate(candidate)) => {
            let mut values = Vec::new();
            let Ok(holds) = evaluate::<std::convert::Infallible>(matcher, &mut values, |id| {
                Ok(candidate_property(resolver, candidate, id))
            });
            Ok(Found::Test(holds, values))
        }
        (Op::Related { relation, .. }, Object::Node(node)) => {
            related(resolver, node, *relation).map(Found::Related)
        }
        (
            Op::IsOneOf {
                equal, at_address, ..
            },
            object,
        ) => Ok(Found::IsOneOf(is_one_of(
            resolver,
            object,
            (*equal, at_address),
        ))),
        (Op::Activate { .. }, Object::Node(node)) => Ok(Found::Activate(acquire::activate(
            node,
            resolver.registry(),
        )?)),
        _ => Err(Unrun::Unsupported),
    }
}

/// Whether `Read` reads property `id` apart from the node's snapshot.
fn read_apart(id: PropertyId) -> bool {
    matches!(
        id,
        PropertyId::Suggestions
            | PropertyId::HasValuePattern
            | PropertyId::WindowClass
            | PropertyId::ComboBoxList
            | PropertyId::ChildCount
            | PropertyId::TextSummary
    )
}

/// `Read` of node `node`: its snapshot, read only when the set names a
/// property the snapshot holds, and each property read apart from it: its
/// window's class and its text's summary as the outpost holds them, and
/// how many children a `SysTreeView32` item holds, by the control's
/// messages, as NVDA's tree view item counts them
/// (`NVDAObjects/IAccessible/sysTreeView32.py`), with no read of a child.
/// Suggestions, the `Value` pattern and a combo box's list are UI
/// Automation's, which an MSAA node answers none of.
fn read(
    resolver: &dyn Resolve,
    node: NodeId,
    set: &[PropertyId],
) -> Result<PropertyValues, AcquireError> {
    let snapshot = if set.iter().any(|&id| !read_apart(id)) {
        Some(acquire::read_node(node, resolver.registry())?)
    } else {
        None
    };
    let values = snapshot
        .as_ref()
        .map(|snapshot| resolver.values(snapshot, set));
    Ok(PropertyValues(
        set.iter()
            .enumerate()
            .map(|(place, &id)| PropertyAnswer {
                id,
                value: match id {
                    PropertyId::WindowClass
                    | PropertyId::TextSummary
                    | PropertyId::MsaaAddress
                    | PropertyId::WindowShown => resolver.held(node, id),
                    PropertyId::ChildCount => resolver
                        .registry()
                        .key_of(node)
                        .and_then(|(hwnd, _, child)| acquire::tree_view_child_count(hwnd, child))
                        .map(PropertyValue::Count),
                    PropertyId::Suggestions
                    | PropertyId::HasValuePattern
                    | PropertyId::ComboBoxList => None,
                    _ => values
                        .as_ref()
                        .and_then(|values| values.0.get(place))
                        .and_then(|answer| answer.value.clone()),
                },
            })
            .collect(),
    ))
}

/// Whether a `Test` answers property `id` with no read of the node: those
/// the outpost holds, and those only a focus read or an acquired object
/// answers, which a node held already answers none of (Claude,
/// 2026-11-21).
fn tested_without_read(id: PropertyId) -> bool {
    matches!(
        id,
        PropertyId::FocusedWithin
            | PropertyId::MsaaAddress
            | PropertyId::WindowShown
            | PropertyId::IsContent
    )
}

/// `Test` of node `node`: the matcher evaluated lazily, each property read
/// at most once when the matcher first needs it, the node read at most
/// once, for the first property the outpost does not hold.
fn test(resolver: &dyn Resolve, node: NodeId, matcher: &Matcher) -> Result<Found, Unrun> {
    let mut snapshot: Option<NodeSnapshot> = None;
    let mut values = Vec::new();
    let holds = evaluate::<AcquireError>(matcher, &mut values, |id| {
        if tested_without_read(id) {
            return Ok(match id {
                PropertyId::MsaaAddress | PropertyId::WindowShown => resolver.held(node, id),
                _ => None,
            });
        }
        if snapshot.is_none() {
            snapshot = Some(acquire::read_node(node, resolver.registry())?);
        }
        Ok(snapshot
            .as_ref()
            .and_then(|snapshot| resolver.values(snapshot, &[id]).0.pop())
            .and_then(|answer| answer.value))
    })?;
    Ok(Found::Test(holds, values))
}

/// Evaluates `matcher` with `read`, each property read kept in `read_back`
/// in the order read.
fn evaluate<E>(
    matcher: &Matcher,
    read_back: &mut Vec<PropertyAnswer>,
    mut read: impl FnMut(PropertyId) -> Result<Option<PropertyValue>, E>,
) -> Result<bool, E> {
    matcher.evaluate(&mut |id| {
        let value = read(id)?;
        read_back.push(PropertyAnswer {
            id,
            value: value.clone(),
        });
        Ok(value)
    })
}

/// An acquired object's property `id`: whether it or an ancestor up to
/// the desktop has the focused state, its ids, whether its window is
/// shown, or one read alone by the identity check's reads.
fn candidate_property(
    resolver: &dyn Resolve,
    candidate: &mut FocusCandidate,
    id: PropertyId,
) -> Option<PropertyValue> {
    match id {
        PropertyId::FocusedWithin => {
            let plain = |window| resolver.plain(window);
            let mut parent = |window| resolver.parent_window(window);
            Some(PropertyValue::Flag(candidate.has_focused_state(
                resolver.hops(),
                &plain,
                &mut parent,
            )))
        }
        PropertyId::MsaaAddress => {
            let (window, object, child) = candidate.key();
            Some(PropertyValue::MsaaAddress(HeldIds {
                event: MsaaIds {
                    window: WindowHandle(u64::from_ne_bytes(window.to_ne_bytes())),
                    object,
                    child,
                },
                held_child: candidate.resolved_child(),
            }))
        }
        PropertyId::WindowShown => Some(PropertyValue::Flag(
            verbatim_win32::window::window_is_visible(candidate.key().0),
        )),
        id => candidate.property(id, resolver.registry()),
    }
}

/// `Related` from node `node`: a step through the tree by `accNavigate`,
/// `accParent` and the window hierarchy, the object reached read through
/// its window's backend; the last child the last of its children; and the
/// controller-for relation and the first selected item none, since an
/// MSAA object controls nothing and has no selected item of its own.
fn related(
    resolver: &dyn Resolve,
    node: NodeId,
    relation: Relation,
) -> Result<Vec<NodeSnapshot>, Unrun> {
    let kind = match relation {
        Relation::Parent => QueryKind::Parent,
        Relation::FirstChild => QueryKind::FirstChild,
        Relation::NextSibling => QueryKind::NextSibling,
        Relation::PreviousSibling => QueryKind::PreviousSibling,
        Relation::LastChild => {
            return match children(resolver, node)? {
                // A list cut short has no last child known.
                (_, false) => Err(Unrun::Failed(AcquireError::Failed(
                    "the children were not all read".to_owned(),
                ))),
                (children, true) => Ok(children.into_iter().last().into_iter().collect()),
            };
        }
        Relation::ControllerFor | Relation::FirstSelectedItem => return Ok(Vec::new()),
    };
    Ok(step(resolver, node, kind)?.into_iter().collect())
}

/// One step of `kind` from node `node`, the object reached read through
/// its window's backend: `None` at a tree edge.
///
/// # Errors
///
/// The read's error, gone for a node gone.
pub fn step(
    resolver: &dyn Resolve,
    node: NodeId,
    kind: QueryKind,
) -> Result<Option<NodeSnapshot>, AcquireError> {
    let from_window = resolver.registry().key_of(node).map(|key| key.0);
    let neighbor = acquire::navigate(node, resolver.registry(), kind)?;
    Ok(neighbor.map(|neighbor| resolver.corrected(from_window, neighbor, kind)))
}

/// Node `node`'s children, in order, at most [`MAX_CHILDREN`], and whether
/// they are all of them: those its object lists, a gone one among them in
/// its place, each read through its window's backend; a node whose
/// children MSAA does not list has them through the resolver.
fn children(
    resolver: &dyn Resolve,
    node: NodeId,
) -> Result<(Vec<NodeSnapshot>, bool), AcquireError> {
    let listed = if resolver.registry().key_of(node).is_some() {
        acquire::children(node, resolver.registry(), MAX_CHILDREN)?
    } else {
        None
    };
    let Some(listed) = listed else {
        return resolver.children(node);
    };
    let window = resolver.registry().key_of(node).map(|key| key.0);
    Ok((
        listed
            .into_iter()
            .map(|child| resolver.corrected(window, child, QueryKind::FirstChild))
            .collect(),
        true,
    ))
}

/// `IsOneOf` of `object`: an acquired object by NVDA's equality for the
/// equal node and by address for the others, the role the equality read
/// kept for the reads after it; a node by its id.
fn is_one_of(
    resolver: &dyn Resolve,
    object: Object<'_>,
    (equal, at_address): (Option<NodeId>, &[NodeId]),
) -> Option<u32> {
    let nodes: Vec<NodeId> = equal
        .into_iter()
        .chain(at_address.iter().copied())
        .collect();
    let found = match object {
        Object::Node(node) => nodes.iter().position(|&each| each == node),
        Object::Candidate(candidate) => {
            let mut held = candidate.object();
            let found = nodes.iter().enumerate().position(|(place, &node)| {
                if place == 0 && equal.is_some() {
                    held.is_equal_to(node, resolver.registry())
                } else {
                    held.which_of(&[node], resolver.registry()) == Some(node)
                }
            });
            candidate.keep_role_of(&held);
            found
        }
    };
    found.and_then(|place| u32::try_from(place).ok())
}

/// One node a walk down the tree answered.
#[derive(Debug)]
pub struct Reached {
    /// The node.
    pub node: NodeSnapshot,
    /// Its parent's place among every node the walk answered, over its
    /// starts, `None` when its parent was not answered.
    pub parent: Option<u32>,
}

/// What a walk down the tree has answered and visited so far, over its
/// starts.
#[derive(Debug)]
pub struct Descended {
    /// How many nodes the walk answered before this start.
    pub before: usize,
    /// The nodes this start answered, in tree order.
    pub reached: Vec<Reached>,
    /// The nodes visited over every start so far.
    pub visited: usize,
    /// Whether no cap or failed read cut it.
    pub complete: bool,
}

impl Descended {
    /// How many nodes the walk answered in all.
    fn answered(&self) -> usize {
        self.before + self.reached.len()
    }
}

/// `Descendants` from `start`, its snapshot `snapshot` when resolving it
/// read one, the walk so far `walked`: its subtree in tree order, each
/// node judged by the matcher and the pruning, by the children walk, into
/// `walked`; `Ok(true)` when the walk met its limit or its node cap.
///
/// # Errors
///
/// The start's read's error, when the walk includes it and it cannot be
/// read.
pub fn descend(
    resolver: &dyn Resolve,
    (start, snapshot): (NodeId, Option<NodeSnapshot>),
    walk: &Walk,
    walked: &mut Descended,
) -> Result<bool, AcquireError> {
    let limit = usize::try_from(walk.limit).unwrap_or(usize::MAX);
    let cap = usize::try_from(walk.nodes).unwrap_or(usize::MAX);
    let mut parent = None;
    if walk.include_subject {
        let snapshot = match snapshot {
            Some(snapshot) => snapshot,
            None => acquire::read_node(start, resolver.registry())?,
        };
        let (matched, pruned) = judged(resolver, &snapshot, walk);
        if pruned {
            return Ok(false);
        }
        if matched {
            parent = Some(u32::try_from(walked.answered()).unwrap_or(u32::MAX));
            walked.reached.push(Reached {
                node: snapshot,
                parent: None,
            });
            if walked.answered() >= limit {
                return Ok(true);
            }
        }
    }
    // Each level's children with the depth below the start and the place
    // of the nearest answered ancestor.
    let mut stack = vec![(start, 0u32, parent)];
    while let Some((node, depth, parent)) = stack.pop() {
        if depth >= walk.depth {
            continue;
        }
        // A node gone, its children unknown, or not read, leaves the walk
        // incomplete.
        let Ok((children, complete)) = children(resolver, node) else {
            walked.complete = false;
            continue;
        };
        walked.complete &= complete;
        let mut below = Vec::new();
        for child in children {
            walked.visited += 1;
            if walked.visited > cap {
                walked.complete = false;
                return Ok(true);
            }
            let (matched, pruned) = judged(resolver, &child, walk);
            if pruned {
                continue;
            }
            let mut own = parent;
            let id = child.id;
            if matched {
                own = Some(u32::try_from(walked.answered()).unwrap_or(u32::MAX));
                walked.reached.push(Reached {
                    node: child,
                    parent,
                });
                if walked.answered() >= limit {
                    return Ok(true);
                }
            }
            below.push((id, depth + 1, own));
        }
        // Depth first in tree order: the first child's subtree next.
        stack.extend(below.into_iter().rev());
    }
    Ok(false)
}

/// Whether `node`, read, is answered by `walk`, and whether its subtree is
/// left out.
fn judged(resolver: &dyn Resolve, node: &NodeSnapshot, walk: &Walk) -> (bool, bool) {
    let holds = |matcher: &Matcher| {
        let Ok(holds) = matcher.evaluate::<std::convert::Infallible>(&mut |id| {
            Ok(resolver
                .values(node, &[id])
                .0
                .pop()
                .and_then(|answer| answer.value))
        });
        holds
    };
    (
        walk.matcher.as_ref().is_none_or(holds),
        walk.prune.as_ref().is_some_and(holds),
    )
}
