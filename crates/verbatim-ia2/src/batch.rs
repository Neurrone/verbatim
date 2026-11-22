//! The MSAA executor: the request list's object operations on MSAA
//! subjects, `Read`, `Test`, `Related`, `Ancestors`, `Descendants`,
//! `IsOneOf` and `Activate`, each one step at a time by calls, as the UI Automation
//! executor's call arms run theirs (the verbatim-ia2 crate guide, "The
//! MSAA executor"). Its subject is a node the outpost holds, an object
//! acquired at its event's ids, or a window's own object ([`Object`]); what the outpost holds, the
//! backend a reached object is read through, and a node MSAA does not list
//! the children of, it resolves ([`Resolve`]).

use verbatim_model::batch::{Op, Relation, Walk};
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{FocusCaret, HeldIds, MsaaIds};
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
    /// A window's own object: its client area, as NVDA reads a foreground
    /// window, so that a focus event on the client area that follows is
    /// the same node and is not announced again.
    Window(isize),
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
    /// `Ancestors`': the ancestors, outermost first, and how the walk
    /// ended.
    Ancestors(Vec<NodeSnapshot>, acquire::Walked),
    /// `Read`'s of a window's own object: the object, whose node and
    /// window the answer carries.
    Window(Box<NodeSnapshot>),
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
        (Op::Read { .. }, Object::Window(hwnd)) => window_object(hwnd, resolver.registry())
            .map(|object| Found::Window(Box::new(object)))
            .ok_or(Unrun::Failed(AcquireError::Gone)),
        (Op::Test { matcher, .. }, Object::Window(hwnd)) => {
            let mut object: Option<Option<NodeSnapshot>> = None;
            let mut values = Vec::new();
            let Ok(holds) = evaluate::<std::convert::Infallible>(matcher, &mut values, |id| {
                Ok(
                    window_property(hwnd, id, acquire::client_role).unwrap_or_else(|| {
                        object
                            .get_or_insert_with(|| window_object(hwnd, resolver.registry()))
                            .as_ref()
                            .and_then(|object| resolver.values(object, &[id]).0.pop())
                            .and_then(|answer| answer.value)
                    }),
                )
            });
            Ok(Found::Test(holds, values))
        }
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
            Object::Node(node),
        ) => Ok(Found::IsOneOf(is_one_of(
            resolver,
            Ok(node),
            (*equal, at_address),
        ))),
        (
            Op::IsOneOf {
                equal, at_address, ..
            },
            Object::Candidate(candidate),
        ) => Ok(Found::IsOneOf(is_one_of(
            resolver,
            Err(candidate),
            (*equal, at_address),
        ))),
        (Op::Activate { .. }, Object::Node(node)) => Ok(Found::Activate(acquire::activate(
            node,
            resolver.registry(),
        )?)),
        (Op::Ancestors { .. }, Object::Node(node)) => {
            let (chain, walked) = ancestors(
                node,
                resolver.registry(),
                &acquire::AncestorLimits {
                    max_hops: resolver.hops(),
                    known: &|_| false,
                    read_by_other_api: &|_| false,
                    plain: &|_| false,
                    candidates: None,
                },
            )?;
            Ok(Found::Ancestors(chain, walked))
        }
        _ => Err(Unrun::Unsupported),
    }
}

/// `Ancestors`' arm: the walk up `node`'s parents by `accParent`, one call
/// a hop, stopping as `limits` say: at nothing for a batch's own step, and
/// at the outpost's stops where it chains a focus's ancestors.
///
/// # Errors
///
/// The node's read failure when it can no longer be reached.
pub fn ancestors(
    node: NodeId,
    registry: &dyn Interning,
    limits: &acquire::AncestorLimits<'_>,
) -> Result<(Vec<NodeSnapshot>, acquire::Walked), AcquireError> {
    acquire::ancestor_chain_until(node, registry, limits)
}

/// Window `hwnd`'s own object, its client area: `None` when it cannot be
/// read.
fn window_object(hwnd: isize, registry: &dyn Interning) -> Option<NodeSnapshot> {
    acquire::snapshot_from_event(
        hwnd,
        windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
        crate::com::CHILDID_SELF,
        registry,
        acquire::Purpose::Announce,
    )
}

/// Property `id` of window `hwnd`'s own object when it is one read of the
/// window itself: whether it is shown, from Windows with no call into the
/// application, and its client object's role as `client_role` reads it,
/// as the hidden-dialog check reads it whatever the window's verdict (two
/// counted calls). `None` for a property read from the window's object.
#[expect(
    clippy::option_option,
    reason = "a value or none for the window's own properties, none for the rest"
)]
pub(crate) fn window_property(
    hwnd: isize,
    id: PropertyId,
    client_role: impl Fn(isize) -> Option<verbatim_model::Role>,
) -> Option<Option<PropertyValue>> {
    match id {
        PropertyId::WindowShown => Some(Some(PropertyValue::Flag(
            verbatim_win32::window::window_is_visible(hwnd),
        ))),
        PropertyId::Role => Some(client_role(hwnd).map(PropertyValue::Role)),
        _ => None,
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
    object: Result<NodeId, &mut FocusCandidate>,
    (equal, at_address): (Option<NodeId>, &[NodeId]),
) -> Option<u32> {
    let nodes: Vec<NodeId> = equal
        .into_iter()
        .chain(at_address.iter().copied())
        .collect();
    let found = match object {
        Ok(node) => nodes.iter().position(|&each| each == node),
        Err(candidate) => {
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

/// What a `FocusRead` reads through MSAA.
pub enum FocusSubject {
    /// An object acquired at its event's ids, the focus redirect applied:
    /// read in full.
    Candidate(FocusCandidate),
    /// The focus of window `hwnd`, found by following `accFocus` down from
    /// its client. With `if_focused`, whether it or an ancestor up to the
    /// desktop has the focused state is read first, as NVDA tests its fake
    /// focus, and the rest is read only when one has.
    Focus {
        /// The window with the keyboard focus, taken already.
        hwnd: isize,
        /// Whether the focused state is read first.
        if_focused: bool,
    },
}

/// What `FocusRead`'s arm found.
pub enum FocusFound {
    /// The focus's answer, with its focused state when the subject asked
    /// for it.
    Read(Option<bool>, Box<verbatim_model::request::FocusRead>),
    /// Neither the focus nor any ancestor has the focused state: nothing
    /// more is read.
    Unfocused,
    /// No focus could be read.
    NotThere,
}

/// The focus a `FocusRead` arm read, for the outpost to make its answer
/// from ([`FocusResolve::answer`]).
pub struct MsaaFocus {
    /// The focus.
    pub node: NodeSnapshot,
    /// Its ancestors, nearest first, up to the first Core knows.
    pub ancestors: Vec<NodeSnapshot>,
    /// The place among the ancestors Core knows of the one the walk met.
    pub met: Option<u32>,
    /// The window it was read in.
    pub nearest: isize,
    /// The ids it was read at.
    pub msaa: Option<HeldIds>,
    /// The foreground window's own object, read when the focus met none of
    /// the ancestors Core knows.
    pub foreground: Option<NodeSnapshot>,
    /// Its caret, line and selection, read last ([`focus_caret`]).
    pub caret: FocusCaret,
}

/// The outpost's side of a `FocusRead`, or a `Test` of the focus, through MSAA: the windows it
/// serves, the ancestors Core knows, the chain above the focus across
/// backends, which dialogs want their text, the other backend's reads,
/// and the answer made from what was read.
pub trait FocusResolve: Resolve + crate::text::TextResolve {
    /// Whether this outpost's application serves window `hwnd`.
    fn serves(&self, hwnd: isize) -> bool;

    /// The ancestors Core knows, for NVDA's equality.
    fn candidates(&self) -> &crate::equality::Candidates;

    /// The chain of ancestors above `node`, outermost first: the
    /// `Ancestors` arm's walk, chained above where it ends across
    /// backends, stopping at an ancestor Core knows; `None` when the focus
    /// was lost on its way.
    fn chain(&self, node: &NodeSnapshot) -> Option<Vec<NodeSnapshot>>;

    /// `chain`, outermost first, cut at the first ancestor Core knows:
    /// nearest first, with that ancestor's place among those Core knows.
    fn cut(&self, chain: Vec<NodeSnapshot>) -> (Vec<NodeSnapshot>, Option<u32>);

    /// Keeps the routes to the nodes a read of the application's focus
    /// minted.
    fn keep_routes(&self, node: &NodeSnapshot, ancestors: &[NodeSnapshot]);

    /// Whether `node` is a dialog whose own text is gathered as its
    /// description.
    fn wants_dialog_text(&self, node: &NodeSnapshot) -> bool;

    /// The own text of a dialog read through UI Automation, the other
    /// backend.
    fn other_dialog_text(&self, node: &NodeSnapshot) -> Option<String>;

    /// The foreground window's own object, taken from `ancestors` when one
    /// is it, and read through its own window's backend otherwise.
    fn foreground(&self, ancestors: &[NodeSnapshot]) -> Option<NodeSnapshot>;

    /// The MSAA ids `node` is kept under.
    fn held_ids(&self, node: &NodeSnapshot) -> Option<HeldIds>;

    /// The answer made from what the arm read: each node's properties as
    /// Core asked, with the focus's caret.
    fn answer(&self, focus: MsaaFocus) -> verbatim_model::request::FocusRead;
}

/// `FocusRead`'s arm through MSAA, NVDA's focus as its pump reads it: the
/// object, its ancestors up to the first Core knows, the text of the
/// dialogs it enters, and, when it met none of those, the foreground
/// object. An acquired candidate's dialogs are read before the foreground
/// object, the application's focus's after it, as each was read before
/// the arm.
pub fn focus_read(subject: FocusSubject, resolver: &dyn FocusResolve) -> FocusFound {
    match subject {
        FocusSubject::Candidate(candidate) => candidate_read(candidate, resolver),
        FocusSubject::Focus { hwnd, if_focused } => focus_now(hwnd, if_focused, resolver),
    }
}

/// `Test` of the application's focus in window `hwnd`, the window with
/// the keyboard focus, taken already: the focus read as [`focus_read`]
/// reads it with its focused state first, and `matcher` evaluated
/// lazily over that read, `FocusedWithin` answering the focused state.
/// Answers what was found, which a `FocusRead` of the focus later in the
/// batch takes instead of reading again, and the test's result, `None`
/// when no focus could be read.
pub fn test_focus(
    hwnd: isize,
    matcher: &Matcher,
    resolver: &dyn FocusResolve,
) -> (FocusFound, Option<(bool, Vec<PropertyAnswer>)>) {
    let found = focus_now(hwnd, true, resolver);
    let (focused, values) = match &found {
        FocusFound::Read(focused, focus) => (
            focused.map(PropertyValue::Flag),
            Some(&focus.focus.read.values),
        ),
        FocusFound::Unfocused => (Some(PropertyValue::Flag(false)), None),
        FocusFound::NotThere => return (found, None),
    };
    let mut read = Vec::new();
    let Ok(holds) = evaluate::<std::convert::Infallible>(matcher, &mut read, |id| {
        Ok(match id {
            PropertyId::FocusedWithin => focused.clone(),
            id => values
                .and_then(|values| values.answer(id))
                .and_then(|answer| answer.value.clone()),
        })
    });
    (found, Some((holds, read)))
}

/// An acquired candidate's full read.
fn candidate_read(candidate: FocusCandidate, resolver: &dyn FocusResolve) -> FocusFound {
    // The ids the object was reached at, after the redirect.
    let (window, object, child) = candidate.key();
    let event = MsaaIds {
        window: WindowHandle(u64::from_ne_bytes(window.to_ne_bytes())),
        object,
        child,
    };
    let held_child = candidate.resolved_child();
    let mut node = candidate.read(resolver.registry(), Some(resolver.candidates()));
    let Some(chain) = resolver.chain(&node) else {
        return FocusFound::NotThere;
    };
    let (mut ancestors, met) = resolver.cut(chain);
    describe_dialogs(
        resolver,
        ancestors.iter_mut().chain(std::iter::once(&mut node)),
    );
    let foreground = met.is_none().then(|| resolver.foreground(&[])).flatten();
    let caret = focus_caret(resolver, &node);
    FocusFound::Read(
        None,
        Box::new(resolver.answer(MsaaFocus {
            node,
            ancestors,
            met,
            nearest: window,
            msaa: Some(HeldIds { event, held_child }),
            foreground,
            caret,
        })),
    )
}

/// The application's focus in window `hwnd`.
fn focus_now(hwnd: isize, if_focused: bool, resolver: &dyn FocusResolve) -> FocusFound {
    let Some(mut object) = acquire::focused_object(hwnd, &|window| resolver.serves(window)) else {
        return FocusFound::NotThere;
    };
    let focused = if_focused.then(|| {
        object.has_focused_state(
            resolver.hops(),
            &|window| resolver.plain(window),
            &mut |window| resolver.parent_window(window),
        )
    });
    if focused == Some(false) {
        tracing::debug!(
            hwnd,
            "the focused MSAA object has no focused state, nor any ancestor"
        );
        return FocusFound::Unfocused;
    }
    let mut node = object.read(resolver.registry(), Some(resolver.candidates()));
    let Some(chain) = resolver.chain(&node) else {
        return FocusFound::NotThere;
    };
    let (mut ancestors, met) = resolver.cut(chain);
    resolver.keep_routes(&node, &ancestors);
    let foreground = met
        .is_none()
        .then(|| resolver.foreground(&ancestors))
        .flatten();
    describe_dialogs(
        resolver,
        ancestors.iter_mut().chain(std::iter::once(&mut node)),
    );
    let msaa = resolver.held_ids(&node);
    let caret = focus_caret(resolver, &node);
    FocusFound::Read(
        focused,
        Box::new(resolver.answer(MsaaFocus {
            node,
            ancestors,
            met,
            nearest: hwnd,
            msaa,
            foreground,
            caret,
        })),
    )
}

/// The focus's caret, read last of the arm's reads: an edit control's
/// client area's by the text arms ([`crate::text::focus_caret`]); any
/// other object has no text.
fn focus_caret(resolver: &dyn FocusResolve, node: &NodeSnapshot) -> FocusCaret {
    let has_text = resolver
        .key_of(node.id)
        .is_some_and(|(hwnd, object, child)| {
            crate::text::edit_version(hwnd, object, child).is_some()
        });
    if has_text {
        crate::text::focus_caret(resolver, node.id)
    } else {
        FocusCaret::NoText
    }
}

/// Gives every dialog among `nodes`, a focus's chain, that wants its own
/// text that text as its description (`docs/nvda/object-model.md`, "A
/// dialog's own text"), a dialog met twice read once: an MSAA dialog's
/// gathered here, by `verbatim_reader_rules::dialog`'s rule, a UI
/// Automation dialog's through the outpost.
fn describe_dialogs<'a>(
    resolver: &dyn FocusResolve,
    nodes: impl IntoIterator<Item = &'a mut NodeSnapshot>,
) {
    let mut read: Vec<(NodeId, Option<String>)> = Vec::new();
    for node in nodes {
        if !resolver.wants_dialog_text(node) {
            continue;
        }
        let text = if let Some((_, text)) = read.iter().find(|(id, _)| *id == node.id) {
            text.clone()
        } else {
            let started = std::time::Instant::now();
            let text = match node.backend {
                verbatim_model::Backend::Msaa => {
                    crate::dialog::DialogObject::of_node(node.id, resolver.registry())
                        .and_then(|dialog| verbatim_reader_rules::dialog::dialog_text(&dialog))
                }
                verbatim_model::Backend::Uia => resolver.other_dialog_text(node),
            };
            tracing::debug!(
                found = text.is_some(),
                elapsed_us = started.elapsed().as_micros(),
                "a dialog's text gathered"
            );
            read.push((node.id, text.clone()));
            text
        };
        node.details.description = text;
    }
}

#[cfg(test)]
#[path = "hidden_root_tests.rs"]
mod hidden_root_tests;
