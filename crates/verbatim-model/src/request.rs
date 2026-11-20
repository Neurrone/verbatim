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
use crate::{
    ActionName, CaretReport, NodeId, Pid, QueryKind, TextAnchor, TextDetails, TextOp, TextReply,
    WindowHandle,
};

/// A reader Core asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Reader {
    /// The focus listener.
    Listener,
    /// The outpost of one application, by its process.
    Outpost(Pid),
}

/// One running instance of a reader: a request out goes to one instance,
/// and only that instance's answer or ending ends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Instance {
    /// The reader.
    pub reader: Reader,
    /// Its generation, never reused.
    pub generation: u64,
}

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
    /// Core's backend verdict for the window as the request is sent, or
    /// "probe first" when none is in force. Only an outpost acts on it:
    /// the focus listener reads the object it was told of and never
    /// probes, and Core takes nothing from its answer about the window.
    pub verdict: WindowVerdict,
    /// What is asked.
    pub request: Request,
}

/// Core's backend verdict for a request's window as it is sent
/// (`docs/design/readers-tell-core-asks-build1.md`, Table 4, column D).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WindowVerdict {
    /// No verdict is in force: the outpost probes the window for a UI
    /// Automation provider before the read and returns the probe's answer.
    ProbeFirst,
    /// The window is read through UI Automation.
    Uia,
    /// The window is read through MSAA.
    Msaa,
}

/// A probe of a window for a UI Automation provider: one raw
/// `WM_GETOBJECT` for the UI Automation root object, waiting at most 3
/// seconds (`docs/parity/backends.md`, "The UIA provider probe").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Probe {
    /// The window.
    pub window: WindowHandle,
    /// The thread that owns it, which keys the verdict with the window.
    pub thread: u32,
    /// Whether it answered with a provider; a probe that timed out did
    /// not.
    pub provider: bool,
    /// When the probe began, in microseconds on the run's shared clock.
    pub began_us: u64,
}

/// The object a read names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectRef {
    /// A UI Automation element the listener kept for a tell: its token,
    /// and the generation of the listener that minted it, since each
    /// listener mints its tokens from 1. A listener answers "not there"
    /// for a token of any other generation.
    Kept {
        /// The listener's generation.
        listener: u64,
        /// The element's token.
        token: ElementToken,
    },
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
    /// Whether the object is one of these nodes: the committed focus first,
    /// met by NVDA's equality, then its ancestors, each met only at the
    /// address it was reported at, as NVDA meets an ancestor
    /// (`docs/parity/focus-announcement.md`).
    OneOf(Vec<NodeId>),
    /// Whether the object is, or is inside, an object the focus controls,
    /// read live from the focus's controller-for relation.
    ControlledBy(NodeId),
    /// A selected item: whether it is, or is inside, an object the focus
    /// controls, read live as [`RelationQuery::ControlledBy`] reads it;
    /// failing that, whether it is one of these nodes, as
    /// [`RelationQuery::OneOf`] tells.
    Selection {
        /// The committed focus.
        focus: NodeId,
        /// The nodes, in the order [`RelationFound::Is`] counts them.
        nodes: Vec<NodeId>,
    },
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
    /// A focus read, whose element the listener keeps as read once the
    /// read ends, until Core's held view lets it go.
    Focus,
    /// A change of a property of one of the nodes its relation names
    /// (`docs/design/readers-tell-core-asks-build1.md`, "Changes on
    /// objects"): an object that is none of them answers not there, read
    /// no further, and one that is reads the set's properties alone, as
    /// NVDA reads only the property its event names.
    Change,
    /// A selection's item, read to be spoken: its name is the one read
    /// afresh, where every other read takes it from its event's cache, so
    /// that the first suggestion Settings selects as its list opens is
    /// said by the name Settings has set by then, not the "list item" its
    /// event carried (`docs/parity/changes-and-selection.md`, "Selection in
    /// a list the focus controls", Dickson, 2026-10-20).
    Selection,
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
    /// The completion of a UI Automation read from the listener's route.
    CompleteUia(Box<CompleteUia>),
    /// One MSAA candidate, read to the depth Core asks.
    MsaaRun(MsaaRun),
    /// The application's current focus, with NVDA's MSAA ids.
    CurrentFocus {
        /// The properties of each node.
        set: Vec<PropertyId>,
        /// The ancestors Core already knows.
        known: Vec<KnownAncestor>,
        /// What the focus's caret read fetches beyond the text, as the
        /// settings report it: its line's formatting ([`FocusCaret`]).
        details: TextDetails,
        /// How far the read goes: [`FocusDepth::Full`] for a read Core makes
        /// itself, [`FocusDepth::FullIfFocused`] for the check after a menu end,
        /// NVDA's fake focus, whose MSAA focus is read in full only when it
        /// or an ancestor, up to the desktop, has the focused state
        /// (`shouldAllowIAccessibleFocusEvent`). The answer carries the
        /// focused state as a fact, and Core judges it.
        depth: FocusDepth,
        /// Whether the read first checks for a hidden dialog
        /// ([`HiddenRoot`]).
        hidden: HiddenRoot,
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
    /// Follow the committed focus's node for its changes, or nothing.
    Follow(Follow),
    /// The application's tree from its top-level window, capped at a
    /// depth of 64 and 4096 nodes, for the control plane: a request from
    /// outside the event queue, naming the target's main window.
    DumpTree {
        /// Whether each element's location and whether it is off the
        /// screen are read ([`crate::protocol::DumpedTree::locations`]).
        locations: bool,
    },
    /// The shell's tray icons or taskbar buttons, for the notification
    /// area list dialog, read within the taskbar's own process
    /// (`books/dev/src/explanation/design/listener-without-connections.md`,
    /// decision 9): a request from outside the event queue, naming
    /// `Shell_TrayWnd`.
    ShellItems {
        /// Which shell surface.
        kind: crate::ShellItemKind,
    },
    /// Where one item [`Request::ShellItems`] listed may be on the screen
    /// now, for a click on it: the surface walked again and the position
    /// read of every button whose runtime id or name is the item's. Which
    /// of them is clicked, if any, the dialog decides. A request from
    /// outside the event queue, naming `Shell_TrayWnd`.
    ShellItemPosition {
        /// Which shell surface the item was listed from.
        kind: crate::ShellItemKind,
        /// The item chosen.
        item: crate::ShellItem,
    },
}

/// The events a followed node's registration takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FollowGroup {
    /// The base group: property changes and the caret event, for every
    /// focus but those below, text fields included.
    Base,
    /// The extended group, adding text changes, for Word's document alone
    /// (`docs/parity/backends.md`, "Which focus takes text-change events").
    Extended,
    /// A terminal: the extended group, but a console host's terminal
    /// without its text and caret events, which its console's own events
    /// tell.
    Terminal {
        /// Whether its window is a console host's.
        console: bool,
    },
}

/// The node `Follow` names: the outpost keeps its registration, adding it
/// when absent, and removes every other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Followed {
    /// The committed focus's node.
    pub node: NodeId,
    /// Its event group.
    pub group: FollowGroup,
    /// An extra object followed for its value changes alone: the focus's
    /// parent when it is a slider that focuses its thumb.
    pub extra: Option<NodeId>,
}

/// Core's request that the committed focus's outpost follow its node, or
/// nothing (`docs/design/readers-tell-core-asks-build1.md`, Table 10). It
/// is answered with one of the five outcomes; a newer `Follow` replaces it,
/// which answers it "ended or stopped".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Follow {
    /// The node followed; `None` removes every registration.
    pub node: Option<Followed>,
}

/// The nearest window of a node an answer read, with the facts a request
/// on the node names it by: the reader reads them from Windows, with no
/// call into the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeWindow {
    /// The node's own window or its nearest ancestor's.
    pub handle: WindowHandle,
    /// Its top-level window (`GA_ROOT`).
    pub top_level: WindowHandle,
    /// The top of its owner chain (`GA_ROOTOWNER`).
    pub root_owner: WindowHandle,
    /// The process that owns it.
    pub process: Pid,
    /// The thread that owns it, read by the same call as its process,
    /// which keys its backend verdict.
    pub thread: u32,
    /// Whether the window is the desktop window (`GetDesktopWindow`), so
    /// the node is the desktop's own, which Core names itself as NVDA
    /// names its desktop object (`docs/parity/review.md`, "Dead
    /// objects"). Every other node lies inside a top-level window. Left
    /// out of the message when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub desktop: bool,
}

/// A node an answer read, minted by the outpost that read it, with its
/// nearest window, which a request on the node names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRead {
    /// The node, `None` when the reader mints no node (the listener).
    pub node: Option<NodeId>,
    /// Its own window or its nearest ancestor's, with that window's facts.
    pub window: NodeWindow,
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
    /// met; `None` when it walked to the top, and for a read with a route,
    /// whose end names what the walk met ([`RouteEnd::Known`]).
    pub met: Option<u32>,
    /// For the listener's read of a UI Automation element, the element's
    /// route, from which the application's outpost completes the read.
    pub route: Option<Route>,
}

/// One element of a route: a UI Automation element as the listener read
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteStep {
    /// Its runtime id.
    pub runtime_id: RuntimeId,
    /// Its own native window, when it is a window's element.
    pub window: Option<WindowHandle>,
    /// Whether UI Automation counts it both a control and a content
    /// element.
    pub content: bool,
}

/// Where the listener's walk above a route ended, which Core passes on in
/// [`CompleteUia`] so that the outpost makes no read of its own to find
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteEnd {
    /// It met this ancestor Core already knows, one of the request's list,
    /// which is not among the route's steps.
    Known(KnownAncestor),
    /// It reached a window below the process boundary, the route's
    /// outermost step: the chain goes on above the boundary by plain
    /// windows.
    ProcessBoundary,
    /// It reached a top-level window, the route's outermost step, whose
    /// parent is the desktop root.
    Root,
    /// A step to a parent failed or found none below the root, or reached
    /// a ghost's or a hung application's window: the chain goes on from
    /// this window's own object.
    ContinueFrom(WindowHandle),
    /// The walk was cut by its depth limit: the chain goes on by plain
    /// windows from this window, included.
    PlainFrom(WindowHandle),
}

/// How the listener reached a UI Automation element, so that the
/// application's outpost finds its own copy of it and of its ancestors:
/// the element and each ancestor the listener read, nearest first, below
/// the element's nearest window, and where the chain goes on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// The element's nearest window.
    pub window: WindowHandle,
    /// The element, then its ancestors, nearest first.
    pub steps: Vec<RouteStep>,
    /// Where the chain goes on above them.
    pub end: RouteEnd,
}

/// What a UI Automation completion is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionFor {
    /// A focus: the node's `Follow` registration is added before its
    /// states are read.
    Focus,
    /// A selection's item, with its relation to the committed focus, read
    /// live from the focus's controller-for relation: answered
    /// [`Reply::Object`], its ancestors the item's parent first.
    SelectionItem {
        /// The committed focus.
        focus: NodeId,
    },
}

/// The outpost completes the listener's read of a UI Automation element
/// from its route: it mints node ids, reads the MSAA window above it and
/// the chain above the process boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteUia {
    /// The route the listener answered.
    pub route: Route,
    /// What the listener read of the element and each ancestor on the
    /// route, nearest first, which the completion keeps.
    pub read: Vec<PropertyValues>,
    /// The properties reported of each node, as Core's settings choose
    /// them: the listener's read holds these, and the description too,
    /// which the completion's layout judgment reads whatever the settings.
    pub set: Vec<PropertyId>,
    /// The ancestors Core already knows.
    pub known: Vec<KnownAncestor>,
    /// What it is for.
    pub completes: CompletionFor,
    /// What the focus's caret read fetches beyond the text, as the
    /// settings report it: its line's formatting ([`FocusCaret`]).
    pub details: TextDetails,
    /// Whether the completion first checks for a hidden dialog
    /// ([`HiddenRoot`]).
    pub hidden: HiddenRoot,
}

/// One MSAA focus, foreground or menu start candidate, by the ids its
/// event named, with its window's verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCandidate {
    /// The ids its event named.
    pub ids: MsaaIds,
    /// What kind of event named it.
    pub kind: CandidateKind,
    /// Core's verdict for its window.
    pub verdict: WindowVerdict,
}

/// What kind of event named a candidate, which decides what facts the
/// outpost reads of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CandidateKind {
    /// A focus event: whether the object or an ancestor has the focused
    /// state is read.
    Focus,
    /// A foreground event.
    Foreground,
    /// A menu start or popup start: its role is read, which Core judges.
    MenuStart,
}

/// How far a candidate's read goes, which Core chooses so that the
/// outpost holds no rule (`docs/parity/focus-events.md`, "Event
/// acceptance").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Depth {
    /// Only the facts: whether it is there, and its focused state or role.
    Facts,
    /// The facts, and the full read when the object or an ancestor has the
    /// focused state.
    FullIfFocused,
    /// The facts and the full read.
    Full,
    /// As [`Depth::FullIfFocused`], unless the object reached, after the
    /// redirect, is at `ids` and reads as `identity`: then the facts alone,
    /// answered [`Reply::CandidateSame`] (`docs/parity/focus-events.md`,
    /// "Duplicate focus suppression", the identity check).
    FullUnlessSame {
        /// The ids of the readers' last focus.
        ids: MsaaIds,
        /// What the readers' last focus reads as.
        identity: Box<Identity>,
    },
}

/// What an MSAA object reads as, which the identity check compares, each
/// value as a full read gives it: its states, its role, its position in its
/// set and the set's size, its name and its value. The outpost compares
/// them in that order and stops at the first that differs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Its states.
    pub states: crate::StateSet,
    /// Its role.
    pub role: crate::Role,
    /// Its position in its set, compared only where the control gives it
    /// with no walk: a `SysListView32` item's.
    pub position_in_set: Option<u32>,
    /// Its set's size, compared as its position is.
    pub set_size: Option<u32>,
    /// Its name.
    pub name: Option<String>,
    /// Its value.
    pub value: Option<String>,
}

/// How far a read of the application's current focus goes: always at
/// least as far as the focus, so it has no depth that stops at its facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FocusDepth {
    /// The focused state as a fact, and the full read only when focused.
    FullIfFocused,
    /// The full read.
    Full,
}

impl From<FocusDepth> for Depth {
    fn from(depth: FocusDepth) -> Self {
        match depth {
            FocusDepth::FullIfFocused => Self::FullIfFocused,
            FocusDepth::Full => Self::Full,
        }
    }
}

/// The outpost reads one MSAA candidate to the depth Core asks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MsaaRun {
    /// The candidate.
    pub candidate: RunCandidate,
    /// How far its read goes.
    pub depth: Depth,
    /// A node and text anchor of another request Core keeps held while
    /// this one is out, a caret key's starting point while it confirms a
    /// focus; the outpost reads nothing of it.
    pub spare: Option<(NodeId, TextAnchor)>,
    /// The properties of each node.
    pub set: Vec<PropertyId>,
    /// The ancestors Core already knows.
    pub known: Vec<KnownAncestor>,
    /// What the focus's caret read fetches beyond the text, as the
    /// settings report it: its line's formatting ([`FocusCaret`]).
    pub details: TextDetails,
    /// Whether the read first checks for a hidden dialog ([`HiddenRoot`]).
    pub hidden: HiddenRoot,
}

/// Whether a focus read first checks whether its window's top-level
/// window is a hidden dialog, answering [`Reply::HiddenDialog`] in place
/// of the read when it is: a dialog's text is read only once it is shown
/// (Dickson, 2026-11-19). Core decides; the outpost obeys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HiddenRoot {
    /// The outpost checks, and answers a hidden dialog unread.
    Check,
    /// The outpost reads as it is, hidden or not: a read that postpones
    /// no dialog (a caret key's confirmation, a menu start, a selection's
    /// item), or the read at a press while a dialog is postponed.
    ReadAnyway,
}

/// What an outpost found of a candidate, which Core judges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateFacts {
    /// Its window is read through UI Automation, so it was not read.
    Skipped,
    /// The object is not there.
    NotThere,
    /// The object was found.
    Found {
        /// Whether it or an ancestor has the focused state; `None` when
        /// not asked, for a foreground or menu start, or a UI Automation
        /// focus.
        focused: Option<bool>,
        /// Its role, read for a menu start; `None` otherwise.
        role: Option<crate::Role>,
    },
}

/// The answer to [`MsaaRun`] and [`Request::CurrentFocus`]: the facts
/// found, and the full read when the depth asked for one and the object
/// was found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateAnswer {
    /// The facts.
    pub facts: CandidateFacts,
    /// The full read.
    pub read: Option<Box<FocusRead>>,
}

impl CandidateAnswer {
    /// The answer of a focused candidate read in full, its role taken from
    /// the read.
    #[must_use]
    pub fn focused(read: FocusRead) -> Self {
        Self {
            facts: CandidateFacts::Found {
                focused: Some(true),
                role: None,
            },
            read: Some(Box::new(read)),
        }
    }
}

/// An MSAA object's ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MsaaIds {
    /// The window.
    pub window: WindowHandle,
    /// The object id.
    pub object: i32,
    /// The child id.
    pub child: i32,
}

/// The MSAA ids a focus was read by, with the child id the outpost holds
/// the object by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HeldIds {
    /// The ids the event named; for a focus a redirect took to its
    /// focused child, that child's ids, as NVDA builds the redirected
    /// object with the child's id as its event's; and for a focus read
    /// through `accFocus`, the ids it is held by, as NVDA's object gets its
    /// ids from `IAccIdentity` (`NVDAObjects/IAccessible/__init__.py` 803
    /// to 808).
    pub event: MsaaIds,
    /// The child id the outpost holds the object by.
    pub held_child: i32,
}

/// The classes whose rules change how Core judges an object, such as the
/// MSAA duplicate test. Word's, Outlook's and Internet Explorer's join with
/// the milestones that read them. File Explorer's items that raise a
/// second focus event have no class: the identity check in the duplicate
/// test says their second focus once (Dickson, 2026-11-18;
/// `docs/parity/focus-events.md`, "Duplicate focus suppression").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObjectClass {
    /// No class with rules of its own.
    Plain,
}

/// One node of a focus answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusNode {
    /// The node, its nearest window and its properties, by the request's
    /// set.
    pub read: NodeRead,
    /// Its UI Automation runtime id, `None` for an MSAA node.
    pub runtime_id: Option<RuntimeId>,
}

/// One ancestor of a focus answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusAncestor {
    /// The node.
    pub node: FocusNode,
    /// Whether it is a simple child, never spoken as a state change's
    /// ancestor.
    pub simple_child: bool,
    /// Its dialog text, when it is a dialog the walk read.
    pub dialog_text: Option<String>,
}

/// A focus answer: what Core keeps with the committed focus, each carried
/// here (`docs/design/readers-tell-core-asks-build1.md`, "The focus
/// answers").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusRead {
    /// The focus; its rectangle, states, name and value come with its
    /// properties.
    pub focus: FocusNode,
    /// The MSAA ids it was read by, `None` for a UI Automation focus.
    pub msaa: Option<HeldIds>,
    /// Its ancestors, nearest first, up to the ancestor Core already knows
    /// that the walk met, which is not among them.
    pub ancestors: Vec<FocusAncestor>,
    /// The place, in the request's list, of the known ancestor the walk
    /// met; `None` when it walked to the top, and for a read with a route,
    /// whose end names what the walk met ([`RouteEnd::Known`]).
    pub met: Option<u32>,
    /// The foreground window it entered under, as the read found it;
    /// `None` when it could not be read.
    pub foreground: Option<FocusNode>,
    /// Whether its value comes from its selection.
    pub value_from_selection: bool,
    /// Its class verdict.
    pub class: ObjectClass,
    /// Whether the event's runtime id named another element before: the
    /// node keeps its id, and Core forgets what it kept of the earlier
    /// element's text.
    pub runtime_id_reused: bool,
    /// The focus's caret, read after its states
    /// (`docs/design/readers-tell-core-asks-build1.md`, Table 17, columns
    /// A to C).
    pub caret: FocusCaret,
}

/// A focus's caret, as its focus answer carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusCaret {
    /// The node has no text the outpost can read (an MSAA object that is
    /// not an edit control, a UI Automation element with no text
    /// pattern): Core speaks its value, as NVDA speaks the value of an
    /// object with no text interface.
    NoText,
    /// The node has text, or may have, but it could not be read now: Core
    /// speaks neither its line nor its value, and holds no caret or
    /// selection baseline for it.
    NotRead,
    /// The caret was read, and the outpost minted the next selection
    /// baseline from the selection.
    Read(Box<FocusCaretRead>),
}

/// A focus's caret read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusCaretRead {
    /// The line at the caret, with the caret at its offset, and the
    /// selection.
    pub caret: CaretReport,
    /// The selected text, when something is selected and it could be
    /// read; `None` otherwise, when the line is spoken instead.
    pub selected_text: Option<String>,
    /// The next selection baseline, minted from the selection read: a
    /// focus read becomes the baseline silently, as NVDA's
    /// `initAutoSelectDetection` sets it (`editableText.py` 417 to 427).
    pub baseline: TextAnchor,
}

/// A result: the first of the five outcomes, by request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    /// The answer to [`Request::ReadObject`].
    Object(Box<ObjectRead>),
    /// The answer to [`Request::Properties`].
    Properties(PropertyValues),
    /// The answer to [`Request::CompleteUia`].
    Focus(Box<FocusRead>),
    /// The answer to [`Request::MsaaRun`] and [`Request::CurrentFocus`].
    Candidate(CandidateAnswer),
    /// The answer to [`Request::MsaaRun`] at [`Depth::FullUnlessSame`]
    /// when the object reached is at the ids named, has the focused state,
    /// and reads as the identity named: nothing more read.
    CandidateSame,
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
    /// The answer to [`Request::Follow`]: applied, with no read.
    Followed,
    /// The answer to [`Request::DumpTree`].
    Tree(Box<crate::protocol::DumpedTree>),
    /// The answer to [`Request::ShellItems`], in the shell's order.
    ShellItems(Vec<crate::ShellItem>),
    /// The answer to [`Request::ShellItemPosition`]: the buttons that may
    /// be the item.
    ShellItemPosition(crate::ShellCandidates),
    /// The answer to [`Request::MsaaRun`], [`Request::CurrentFocus`] and a
    /// focus's [`Request::CompleteUia`] asked with [`HiddenRoot::Check`],
    /// when the window's top-level window is hidden and its client object's
    /// role is a dialog's: that window's facts, and nothing read.
    HiddenDialog(NodeWindow),
}

/// Why a reader refused a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// A focus read found the object without the focused state.
    NotFocused,
    /// The probe found the window read through UI Automation, so an MSAA
    /// read of it is not made.
    ReadThroughUia,
    /// The probe found the window read through MSAA, so a UI Automation
    /// completion of it is not made.
    ReadThroughMsaa,
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
    /// The probes the request made, each a verdict Core records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<Probe>,
    /// A diagnostic detail for the log, such as the failed call; empty
    /// when there is none. Core judges nothing by it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl Ask {
    /// Clears the outpost part of every node id the request names, as its
    /// outpost knows them ([`NodeId::unstamped`]).
    pub fn unstamp(&mut self) {
        fn unstamp(node: &mut NodeId) {
            *node = node.unstamped();
        }
        fn known(known: &mut [KnownAncestor]) {
            for ancestor in known {
                if let KnownAncestor::Node(node) = ancestor {
                    unstamp(node);
                }
            }
        }
        match &mut self.request {
            Request::ReadObject(read) => {
                if let ObjectRef::Node(node) = &mut read.object {
                    unstamp(node);
                }
                match &mut read.relation {
                    Some(RelationQuery::OneOf(nodes)) => nodes.iter_mut().for_each(unstamp),
                    Some(RelationQuery::ControlledBy(node)) => unstamp(node),
                    Some(RelationQuery::Selection { focus, nodes }) => {
                        unstamp(focus);
                        nodes.iter_mut().for_each(unstamp);
                    }
                    None => {}
                }
                if let Some(walk) = &mut read.walk {
                    known(walk);
                }
            }
            Request::Properties { node, .. }
            | Request::Navigate { node, .. }
            | Request::Find { node, .. }
            | Request::Children { node, .. }
            | Request::Text { node, .. }
            | Request::Activate { node } => unstamp(node),
            Request::CompleteUia(complete) => {
                known(&mut complete.known);
                if let CompletionFor::SelectionItem { focus } = &mut complete.completes {
                    unstamp(focus);
                }
            }
            Request::MsaaRun(run) => known(&mut run.known),
            Request::CurrentFocus { known: list, .. } => known(list),
            Request::Follow(follow) => {
                if let Some(followed) = &mut follow.node {
                    unstamp(&mut followed.node);
                    if let Some(extra) = &mut followed.extra {
                        unstamp(extra);
                    }
                }
            }
            Request::DumpTree { .. }
            | Request::ShellItems { .. }
            | Request::ShellItemPosition { .. } => {}
        }
    }
}

impl Request {
    /// Whether Core asks it from outside the event queue, which a reader's
    /// worker makes after every request of the queue's waiting with it
    /// (`docs/design/readers-tell-core-asks-build1.md`, "Requests in Core"
    /// and Table 14): a tree dump, the taskbar list's two reads, and a
    /// terminal's reads for its output
    /// (`docs/design/readers-tell-core-asks-build3.md`). `Follow` and a
    /// terminal's baseline are the queue's, as are every other request.
    #[must_use]
    pub fn outside_the_queue(&self) -> bool {
        match self {
            Request::DumpTree { .. }
            | Request::ShellItems { .. }
            | Request::ShellItemPosition { .. } => true,
            Request::Text {
                op: crate::TextOp::ReadTerminal(read),
                ..
            } => read.output,
            _ => false,
        }
    }
}

impl Answer {
    /// Stamps `outpost` on every node id the answer carries, as
    /// [`crate::protocol::OutpostToCore::assign_outpost`] stamps every
    /// message on an outpost's pipe.
    pub fn assign_outpost(&mut self, outpost: crate::OutpostId) {
        self.visit_nodes(&mut |node| *node = node.with_outpost(outpost));
    }

    /// The node ids and selection baselines the answer carries, in order,
    /// but for a text reply's anchors, which the protocol's own walk of a
    /// text reply names.
    #[must_use]
    pub fn named(&self) -> (Vec<NodeId>, Vec<TextAnchor>) {
        let mut copy = self.clone();
        let mut nodes = Vec::new();
        copy.visit_nodes(&mut |node| nodes.push(*node));
        let mut anchors = Vec::new();
        if let Outcome::Result(reply) = &self.outcome {
            let mut caret = |caret: &FocusCaret| {
                if let FocusCaret::Read(read) = caret {
                    anchors.push(read.caret.line.start);
                    if let Some(selection) = &read.caret.selection {
                        anchors.push(selection.range.start.anchor);
                        anchors.push(selection.range.end.anchor);
                    }
                    anchors.push(read.baseline);
                }
            };
            match reply {
                Reply::Focus(focus) => caret(&focus.caret),
                Reply::Candidate(answer) => {
                    if let Some(focus) = &answer.read {
                        caret(&focus.caret);
                    }
                }
                Reply::Object(_)
                | Reply::Properties(_)
                | Reply::Navigated(_)
                | Reply::Found(_)
                | Reply::Children(_)
                | Reply::Text(_)
                | Reply::Activated(_)
                | Reply::Followed
                | Reply::CandidateSame
                | Reply::Tree(_)
                | Reply::ShellItems(_)
                | Reply::ShellItemPosition(_)
                | Reply::HiddenDialog(_) => {}
            }
        }
        (nodes, anchors)
    }

    /// Calls `visit` on every node id the answer carries.
    fn visit_nodes(&mut self, visit: &mut dyn FnMut(&mut NodeId)) {
        fn read(read: &mut NodeRead, visit: &mut dyn FnMut(&mut NodeId)) {
            if let Some(node) = &mut read.node {
                visit(node);
            }
        }
        fn tree(node: &mut crate::TreeNode, visit: &mut dyn FnMut(&mut NodeId)) {
            visit(&mut node.snapshot.id);
            for child in &mut node.children {
                tree(child, visit);
            }
        }
        fn focus(focus: &mut FocusRead, visit: &mut dyn FnMut(&mut NodeId)) {
            read(&mut focus.focus.read, visit);
            for ancestor in &mut focus.ancestors {
                read(&mut ancestor.node.read, visit);
            }
            if let Some(foreground) = &mut focus.foreground {
                read(&mut foreground.read, visit);
            }
        }
        let Outcome::Result(reply) = &mut self.outcome else {
            return;
        };
        match reply {
            Reply::Object(object) => {
                read(&mut object.object, visit);
                for node in object
                    .descendants
                    .iter_mut()
                    .chain(object.ancestors.iter_mut())
                {
                    read(node, visit);
                }
            }
            Reply::Focus(read) => focus(read, visit),
            Reply::Candidate(answer) => {
                if let Some(read) = &mut answer.read {
                    focus(read, visit);
                }
            }
            Reply::Navigated(Some(node)) | Reply::Found(Some(node)) => read(node, visit),
            Reply::Children(children) => {
                for child in children {
                    read(child, visit);
                }
            }
            Reply::Tree(dumped) => tree(&mut dumped.root, visit),
            Reply::Navigated(None)
            | Reply::Found(None)
            | Reply::Properties(_)
            | Reply::Text(_)
            | Reply::Activated(_)
            | Reply::Followed
            | Reply::CandidateSame
            | Reply::ShellItems(_)
            | Reply::ShellItemPosition(_)
            | Reply::HiddenDialog(_) => {}
        }
    }
}

/// The session's lock state, as the shell's session notifications and
/// its query at startup tell it (`docs/design/readers-tell-core-asks-build1.md`,
/// "The secure desktop and the lock state"). Core counts unknown as
/// locked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionLock {
    /// Unlocked.
    #[default]
    Unlocked,
    /// Locked.
    Locked,
    /// The query could not tell.
    Unknown,
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

/// What the shell read in front when it answered a read of an
/// application's current focus unsent ([`QueueInput::InFront`]): the window
/// in front read live, as NVDA's fake focus and its other reads of the
/// focus take it (`kwargsFromSuper`, `NVDAObjects/window/__init__.py` 162
/// to 181): `GetForegroundWindow`, the desktop window when there is none,
/// then that window's thread's focus window if it has one, else that
/// window itself. Every fact is a read of Windows' own, with no message to
/// any application.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Front {
    /// The window in front is a ghost window, which Windows shows in
    /// place of a hung top-level window.
    Ghost(GhostFront),
    /// No window is in front, and the desktop window's thread has no focus
    /// window: the desktop window, which NVDA makes its plain `Desktop`
    /// window object of (`NVDAObjects/window/__init__.py` 83 to 87, 123 to
    /// 127, 409 to 413).
    Desktop(DesktopFront),
    /// The window in front is hung and no ghost stands in front of it.
    Hung,
    /// The window in front responds.
    Responsive,
    /// The window in front is gone.
    Gone,
}

/// The desktop window, in front when no window is, with its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopFront {
    /// The desktop window (`GetDesktopWindow`).
    pub desktop: WindowHandle,
    /// The process that owns it.
    pub process: crate::Pid,
    /// The thread that owns it.
    pub thread: u32,
}

/// A ghost window in front, with the facts NVDA's plain window object of
/// it reads (`NVDAObjects/window/__init__.py` 83 to 98, 299 to 306;
/// `winUser.py` 652 to 655).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostFront {
    /// The ghost window.
    pub ghost: WindowHandle,
    /// The hung window it stands for (`HungWindowFromGhostWindow`).
    pub stands_for: WindowHandle,
    /// The application of the hung window it stands for, never the
    /// ghost's own (the desktop window manager's).
    pub process: crate::Pid,
    /// The thread that owns the hung window it stands for.
    pub thread: u32,
    /// Its window text (`InternalGetWindowText`), empty when it has none.
    pub name: String,
    /// Whether it is disabled (`WS_DISABLED`), the one state NVDA's plain
    /// window gives from its style.
    pub unavailable: bool,
}

/// The foreground record as the listener reports it to the event queue:
/// the window `GetForegroundWindow` names, once it names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForegroundReport {
    /// The window.
    pub window: WindowHandle,
    /// The top of its owner chain.
    pub root_owner: WindowHandle,
    /// The application holding attention: the window's owner, or for a
    /// console window its console host.
    pub process: Pid,
    /// The thread that owns the window, which keys its backend verdict.
    pub thread: u32,
    /// Its class.
    pub class: String,
}

/// An input to Core's event queue (ADR 0003, "Core's inbox and the event
/// queue").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueInput {
    /// A reader's tell.
    Tell {
        /// The instance that told it.
        from: Instance,
        /// The tell.
        tell: Box<crate::tell::Event>,
    },
    /// A reader's answer.
    Answer {
        /// The instance that answered.
        from: Instance,
        /// The answer.
        answer: Answer,
    },
    /// The shell found the request's window hung before sending it, and
    /// answers it "not responding" unsent.
    RefusedHung(RequestNumber),
    /// The shell answers a read of an application's current focus unsent
    /// with what it read in front ([`Front`]): when the read's window is
    /// hung, where every other request is [`QueueInput::RefusedHung`]; and
    /// when the check after a menu, popup or switcher end
    /// ([`FocusDepth::FullIfFocused`]) finds no foreground window, as
    /// NVDA's fake focus then reads the desktop.
    InFront {
        /// The request.
        number: RequestNumber,
        /// What is in front.
        front: Front,
    },
    /// The shell's half-second check of a window.
    HangCheck {
        /// The window.
        window: WindowHandle,
        /// Whether Windows reports it hung.
        hung: bool,
    },
    /// The listener's foreground report.
    Foreground(ForegroundReport),
    /// A reader's instance can take requests.
    ReaderReady(Instance),
    /// A reader's instance ended.
    ReaderEnded(Instance),
    /// The listener of `from` heard a window destroyed
    /// ([`crate::protocol::ListenerToCore::Destroyed`]).
    WindowDestroyed {
        /// The listener's instance, whose frames Core admits only while it
        /// is the current one.
        from: Instance,
        /// The window.
        window: WindowHandle,
        /// The process of the thread that raised its destruction, whose
        /// outpost forgets what it holds of it; `None` when the listener
        /// could not open that thread.
        process: Option<Pid>,
    },
    /// The time Core last asked to be woken at came.
    Due,
    /// The processes whose windows nothing is read from: the end-to-end
    /// harness's own, given once at startup; none for real users.
    Ignored(Vec<Pid>),
    /// A key or command the hook reported, its action chosen at the press.
    Press(Box<Press>),
    /// Playback reached an index mark Core placed in an utterance.
    MarkReached(crate::SpeechMark),
    /// A key transition the hook deferred behind a held key, joining
    /// Core's stroke line at once.
    Deferred {
        /// The key.
        stroke: crate::stroke::Stroke,
        /// A press, or else a release.
        pressed: bool,
    },
    /// Alt went down, or up: the hook reports each change of whether
    /// either Alt key is down, which joins Core's key state at once, so a
    /// tell reads it at its arrival in the inbox's order (design
    /// `docs/design/readers-tell-core-asks-build1.md`, "Keys and commands in
    /// this build").
    Alt {
        /// Whether Alt is now down.
        down: bool,
    },
    /// The hook's line was reset (its reinstall): every stroke not yet
    /// sent is dropped.
    StrokesReset,
    /// Verbatim is exiting and the hook is gone: Core names the keys whose
    /// last transition in line is a release
    /// ([`QueueEffect::ReleaseAtExit`]), and the line is dropped.
    Exit,
    /// A request from outside the event queue: a read for one of
    /// Verbatim's dialogs, the control plane, a tree dump, or either of
    /// the taskbar list's two reads, which name `Shell_TrayWnd`. It holds
    /// no key, has the queue's due time and live hang check, and Core
    /// does not stop waiting for it when the foreground changes. Its end
    /// is [`QueueEffect::Answered`] under `asker`.
    AskOutside {
        /// The token the shell keeps the asker's responder under.
        asker: u64,
        /// The window the request names, which its owning process's
        /// outpost is asked about: the node's nearest window, the
        /// target's main window for a tree dump.
        window: NodeWindow,
        /// The request, one of the shared list's.
        request: Request,
    },
    /// The asker of a request outside the queue no longer needs it: its
    /// dialog closed or its client left.
    WithdrawOutside(u64),
    /// The session's lock state, from a session notification or the
    /// query at startup.
    Lock(SessionLock),
    /// A process fact the supervisor's owner thread reports for Core's
    /// outpost generation record.
    Process(ProcessFact),
    /// The keyboard hook cut speech off at a key press, at the input's
    /// time: handled at once in the inbox, as the speech side's cut
    /// (`docs/design/readers-tell-core-asks-build1.md`, "The inbox, handled
    /// at once"); say-all stops, as NVDA's stops on any key. The speech
    /// queue drops speech of keys numbered below `key`, so Core's speech
    /// from then on carries at least `key`, a modifier's number included.
    SpeechCut {
        /// The number of the key whose press cut speech.
        key: crate::KeySeq,
    },
    /// An utterance Core placed index marks in was cut off before playback
    /// reached all of them, however it was cut; handled at once in the
    /// inbox, so what waited on its marks stops.
    UtteranceCancelled {
        /// The marks the utterance carried, in order; those playback had
        /// already reached were reported as [`QueueInput::MarkReached`]
        /// first.
        marks: Vec<crate::SpeechMark>,
        /// When it ended, in milliseconds since the Unix epoch; 0 when
        /// unknown.
        at_ms: u64,
    },
    /// An utterance Core placed index marks in failed in synthesis or
    /// audio output before playback reached all of them; handled at once
    /// in the inbox, counted as heard.
    UtteranceFailed {
        /// The marks the utterance carried, in order; those playback had
        /// already reached were reported as [`QueueInput::MarkReached`]
        /// first.
        marks: Vec<crate::SpeechMark>,
    },
    /// The focused terminal's quiet-period timer came due at `at_ms`, the
    /// due time Core's state gives.
    TerminalDue {
        /// When it came due, in milliseconds on the clock every process of
        /// a run shares.
        at_ms: u64,
    },
    /// A lock key's state, read by the shell once the key's read was due
    /// and Windows had changed it.
    LockKeyRead {
        /// Which key.
        key: crate::LockKey,
        /// Whether it is now on.
        on: bool,
    },
}

/// A process fact the supervisor's owner thread reports for Core's outpost
/// generation record (`docs/design/readers-tell-core-asks-build1.md`,
/// Table 5): the owner judges no generation itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessFact {
    /// The owner thread started an outpost process for `process`, as
    /// Core's [`QueueEffect::StartOutpost`] asked: a spare given or a
    /// launch, which the owner chooses.
    OutpostStarted {
        /// The application.
        process: Pid,
        /// The new generation, never reused.
        generation: u64,
    },
    /// The owner thread could not start an outpost for `process`.
    LaunchFailed {
        /// The application.
        process: Pid,
        /// Whether it failed because the application's process is gone.
        application_gone: bool,
    },
    /// An outpost or the listener answered a ping.
    Pong {
        /// The instance that answered.
        instance: Instance,
        /// How many of an outpost's workers were abandoned and have not
        /// returned, but for those parked on a window judged hung: Core
        /// ends an outpost that piles them up. 0 for the listener.
        abandoned: usize,
    },
    /// The application's own process exited, as the relay's held handle
    /// tells it.
    ApplicationExited(Pid),
    /// The owner thread is about to launch a listener of this generation:
    /// frames of any other listener are dropped from now on.
    ListenerStarted(u64),
    /// How many spare outposts wait, as the owner thread last counted
    /// them: a fact for the test harness's readiness.
    SparesReady(usize),
    /// Verbatim's own process, at startup: its outpost is started at once.
    OwnProcess(Pid),
}

/// A key or command as the hook reports it to the event queue (ADR 0003,
/// "Keys and commands"): its number and time, and the action chosen at
/// the press against the hook's view of the committed focus and the
/// gesture map. A modifier alone is never reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Press {
    /// The key's number, which its speech's fence and the marks compare.
    pub key_seq: crate::KeySeq,
    /// When the hook saw the key, in microseconds on the run's shared
    /// clock.
    pub pressed_at_us: u64,
    /// What the press does, chosen at the press.
    pub action: PressAction,
}

/// What a press does, chosen at the press. Only a focus-bound action names
/// the node its press was bound to; a global command resolves its object
/// at its turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PressAction {
    /// A review, object-navigation or reader command: global.
    Command {
        /// Which command.
        command: crate::ReviewCommand,
        /// How many times it was pressed in quick succession, zero for the
        /// first press.
        repeat: u8,
    },
    /// A script that is not a review command: global.
    Script {
        /// Which script.
        script: crate::Script,
        /// As [`PressAction::Command`]'s.
        repeat: u8,
    },
    /// A caret key, bound to the committed focus's record at its press.
    CaretKey {
        /// Which key.
        key: crate::CaretKey,
        /// The stroke the hook held back from the application, `None` when
        /// it passed the key at once.
        held: Option<crate::stroke::HeldStroke>,
        /// The committed focus's record at the press, `None` when there was
        /// none or it has no text.
        bound: Option<Box<CaretBinding>>,
    },
    /// Text typed: one character, or an input method's composition.
    Typed {
        /// The text, a tab as a tab character and Enter as a carriage
        /// return.
        text: String,
    },
    /// A key that ends or clears the command line being typed: Escape,
    /// Control+C, Control+D or Control+Break.
    Clearing,
    /// An editing chord passed to the application: Control+Z, Control+Y,
    /// Control+X, Control+V or Escape.
    EditingChord,
    /// Any other key passed to the application, which types no text.
    Passed,
    /// A key passed to the application that types a control character
    /// other than Tab, a carriage return, Backspace's and Control+Backspace's
    /// (Control+C, Control+Enter and the like; never Escape), which ends the word
    /// being typed and is never spelled, as NVDA's typed-character rule
    /// takes it (`docs/parity/input.md`, "Typed-character echo"). It comes
    /// after the key's other report, if any.
    Control,
}

/// The committed focus's record a caret key is bound to at its press, as
/// the hook's view publishes it: the node with its window, its role and how
/// its text is read. The key runs on this node at its turn even when the
/// focus has moved on (Dickson, 2026-11-16).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretBinding {
    /// The node.
    pub node: NodeId,
    /// Its nearest window.
    pub window: NodeWindow,
    /// Its role, by which Core alone judges a terminal.
    pub role: crate::Role,
    /// How its text is read.
    pub provider: crate::TextProvider,
    /// The focus record it was bound to: the trace of the event Core took
    /// that focus from, which with the node names the record, since each
    /// focus taken is a new record.
    #[serde(default)]
    pub record: Option<crate::TraceId>,
    /// The record's formatting memory at the press: the formatting last
    /// spoken in its text, which the key compares with and updates
    /// (`docs/design/readers-tell-core-asks-build2.md`, "The formatting
    /// memory").
    #[serde(default)]
    pub formatting: crate::TextAttributes,
}

/// How a request from outside the event queue ended, as its asker learns:
/// the five outcomes, a result that did not decode, or Core no longer
/// awaiting it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutsideOutcome {
    /// A result.
    Result(Reply),
    /// Refused by the reader, with its reason.
    Refused(Refusal),
    /// A result that did not decode as the request's reply: a malformed
    /// answer.
    Undecodable,
    /// The object is not there.
    NotThere,
    /// Not responding.
    NotResponding,
    /// The reader ended or stopped.
    ReaderEnded,
    /// Core no longer awaits it, for this reason.
    NoLongerAwaited(CancelReason),
}

/// What Core's event queue asks of the shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueEffect {
    /// Check the request's window for a hang, then send it to the
    /// instance, or answer it "not responding" unsent
    /// ([`QueueInput::RefusedHung`]).
    Send {
        /// The instance.
        to: Instance,
        /// The request.
        ask: Ask,
    },
    /// Tell the instance to cancel a request.
    Cancel {
        /// The instance.
        to: Instance,
        /// The cancel.
        cancel: Cancel,
    },
    /// A request from outside the event queue ended: answer its asker.
    Answered {
        /// The token the shell keeps the asker's responder under.
        asker: u64,
        /// How it ended.
        outcome: OutsideOutcome,
    },
    /// Check these windows for a hang ([`QueueInput::HangCheck`]).
    CheckHung(Vec<WindowHandle>),
    /// Send these transitions, in order, with one `SendInput`.
    SendStrokes(Vec<crate::stroke::SentTransition>),
    /// At exit, release each of these keys that Windows still has down, in
    /// order, and send nothing else.
    ReleaseAtExit(Vec<crate::stroke::Stroke>),
    /// Tell the instance a window was destroyed: it answers "not there"
    /// the queued reads whose MSAA ids name the window and the watches
    /// waiting on it, and forgets the window's MSAA nodes, text facts,
    /// kept verdicts and classic-read mark before any later work.
    WindowDestroyed {
        /// The outpost instance of the process that raised it.
        to: Instance,
        /// The window.
        window: WindowHandle,
    },
    /// An answer was dropped: log it.
    AnswerDropped {
        /// The request it named.
        number: RequestNumber,
        /// Whether it was late or named no request out to its sender.
        late: bool,
    },
    /// A result did not decode as its request's reply: log it as a
    /// malformed answer, since every answer a correct reader gives decodes.
    Undecodable(RequestNumber),
    /// The front entry's paused function could not be rebuilt from its
    /// checkpoint, a defect in Core: log it; the input was not handled.
    Mismatch {
        /// What the rebuilt function asked and had.
        replayed: String,
        /// What the checkpoint records.
        original: String,
    },
    /// Wake Core with [`QueueInput::Due`] at this time, in microseconds on
    /// the run's shared clock; `None` for no wake.
    WakeAt(Option<u64>),
    /// Start an outpost for this application: give it a spare or launch
    /// one, and report [`ProcessFact::OutpostStarted`] or
    /// [`ProcessFact::LaunchFailed`].
    StartOutpost(Pid),
    /// End this outpost generation: it stopped answering pings.
    EndOutpost(Instance),
    /// A frame of an instance that is not its reader's current one was
    /// dropped: log it.
    StaleFrame(Instance),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_read(number: u64) -> NodeRead {
        NodeRead {
            node: Some(NodeId::new(number)),
            window: NodeWindow {
                handle: WindowHandle(5),
                top_level: WindowHandle(5),
                root_owner: WindowHandle(5),
                process: Pid(9),
                thread: 2,
                desktop: false,
            },
            values: PropertyValues::default(),
        }
    }

    #[test]
    fn an_answer_names_and_stamps_every_node_it_carries() {
        let mut answer = Answer {
            number: RequestNumber(1),
            outcome: Outcome::Result(Reply::Object(Box::new(ObjectRead {
                object: node_read(1),
                relation: None,
                descendants: vec![node_read(2)],
                ancestors: vec![node_read(3)],
                met: None,
                route: None,
            }))),
            probes: Vec::new(),
            detail: String::new(),
        };
        assert_eq!(
            answer.named(),
            (
                vec![NodeId::new(1), NodeId::new(2), NodeId::new(3)],
                Vec::new()
            )
        );
        answer.assign_outpost(crate::OutpostId(4));
        assert_eq!(
            answer.named().0,
            [1, 2, 3].map(|number| NodeId::in_outpost(crate::OutpostId(4), number))
        );
    }

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
            verdict: WindowVerdict::ProbeFirst,
            request: Request::ReadObject(ReadObject {
                object: ObjectRef::Kept {
                    listener: 1,
                    token: ElementToken(3),
                },
                set: vec![PropertyId::Name, PropertyId::Role],
                relation: Some(RelationQuery::OneOf(vec![NodeId::new(4)])),
                descendants: false,
                walk: Some(vec![KnownAncestor::Uia(RuntimeId(vec![42, 1]))]),
                purpose: Purpose::Focus,
            }),
        };
        let json = serde_json::to_string(&ask).unwrap();
        assert_eq!(serde_json::from_str::<Ask>(&json).unwrap(), ask);

        let answer = Answer {
            number: RequestNumber(7),
            outcome: Outcome::Result(Reply::Object(Box::new(ObjectRead {
                object: NodeRead {
                    node: None,
                    window: NodeWindow {
                        handle: WindowHandle(5),
                        top_level: WindowHandle(5),
                        root_owner: WindowHandle(5),
                        process: Pid(1),
                        thread: 2,
                        desktop: false,
                    },
                    values: PropertyValues::default(),
                },
                relation: Some(RelationFound::Unrelated),
                descendants: Vec::new(),
                ancestors: Vec::new(),
                met: Some(0),
                route: None,
            }))),
            probes: Vec::new(),
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
