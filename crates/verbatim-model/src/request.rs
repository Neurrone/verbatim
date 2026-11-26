//! Core's requests to the readers, their answers and their cancels (ADR
//! 0003, "Readers tell, Core asks"; "Properties, event kinds and
//! requests"; "A frozen window").
//!
//! Every call into an application is made for a request from Core; the
//! request carries what it needs and gets one answer, with one of five
//! outcomes ([`Outcome`]). One request list is shared by Core's own
//! features and the extension surface: a batch of operations from one
//! closed list ([`crate::batch`]). A new command adds no kind of request.
//! The terminal's three text operations are carried beside it until the
//! terminal's rebuild converts them ([`Question::Terminal`]).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::batch::{Batch, Malformed, Placement, StepOutcome};
use crate::property::{PropertyId, PropertyValue, PropertyValues, RuntimeId};
use crate::{CaretReport, NodeId, Pid, TextAnchor, TextOp, TextReply, WindowHandle};

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

impl Instance {
    /// The id its nodes carry: an outpost's generation.
    #[must_use]
    pub const fn outpost_id(self) -> crate::OutpostId {
        crate::OutpostId(self.generation)
    }
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
    pub request: Question,
}

/// What a request asks: a batch, or one of the terminal's three text
/// operations, which the terminal's rebuild converts to batches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Question {
    /// A batch of operations.
    Batch(Batch),
    /// A read, history read or search of a terminal's text.
    Terminal(TerminalAsk),
}

/// One of the terminal's text operations on its node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAsk {
    /// The terminal's node.
    pub node: NodeId,
    /// What to read.
    pub op: TextOp,
}

impl From<Batch> for Question {
    fn from(batch: Batch) -> Self {
        Self::Batch(batch)
    }
}

impl Question {
    /// The terminal's operation it is, `None` for a batch.
    #[must_use]
    pub const fn terminal(&self) -> Option<&TerminalAsk> {
        match self {
            Self::Terminal(ask) => Some(ask),
            Self::Batch(_) => None,
        }
    }

    /// Whether Core asks it from outside the event queue
    /// ([`Placement::Outside`]): a batch placed there, or a terminal's
    /// read for its output
    /// (`docs/design/readers-tell-core-asks-build3.md`), which a reader's
    /// worker makes after every request of the queue's waiting with it.
    #[must_use]
    pub fn outside_the_queue(&self) -> bool {
        match self {
            Self::Batch(batch) => batch.placement == Placement::Outside,
            Self::Terminal(TerminalAsk {
                op: TextOp::ReadTerminal(read),
                ..
            }) => read.output,
            Self::Terminal(_) => false,
        }
    }
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

impl NodeRead {
    /// The snapshot its values describe, read through `backend`; `None`
    /// when the reader minted no node for it.
    #[must_use]
    pub fn snapshot(&self, backend: crate::Backend) -> Option<crate::NodeSnapshot> {
        let value = |id| {
            self.values
                .answer(id)
                .and_then(|answer| answer.value.as_ref())
        };
        let text = |id| match value(id) {
            Some(PropertyValue::Text(text)) => Some(text.clone()),
            _ => None,
        };
        let count = |id| match value(id) {
            Some(PropertyValue::Count(count)) => Some(*count),
            _ => None,
        };
        Some(crate::NodeSnapshot {
            id: self.node?,
            backend,
            role: match value(PropertyId::Role) {
                Some(PropertyValue::Role(role)) => *role,
                _ => crate::Role::Unknown,
            },
            name: text(PropertyId::Name),
            value: text(PropertyId::Value),
            states: match value(PropertyId::States) {
                Some(PropertyValue::States(states)) => *states,
                _ => crate::StateSet::default(),
            },
            details: crate::NodeDetails {
                description: text(PropertyId::Description),
                keyboard_shortcut: text(PropertyId::KeyboardShortcut),
                position_in_set: count(PropertyId::PositionInSet),
                set_size: count(PropertyId::SetSize),
                level: count(PropertyId::Level),
                rect: match value(PropertyId::Rect) {
                    Some(PropertyValue::Rect(rect)) => Some(*rect),
                    _ => None,
                },
                class_name: text(PropertyId::ClassName),
                cell: match value(PropertyId::Cell) {
                    Some(PropertyValue::Cell(cell)) => Some(*cell),
                    _ => None,
                },
                table_counts: match value(PropertyId::TableCounts) {
                    Some(PropertyValue::TableCounts(counts)) => Some(*counts),
                    _ => None,
                },
                window: match value(PropertyId::NativeWindow) {
                    Some(PropertyValue::Window(window)) if window.0 != 0 => Some(*window),
                    _ => None,
                },
                desktop_window: self.window.desktop,
                ..crate::NodeDetails::default()
            },
        })
    }
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

/// Where the listener's walk above a route ended, which the route carries
/// so that the outpost makes no read of its own to find it.
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
/// the element's nearest window, where the chain goes on, and what the
/// listener read of each, which the outpost's read of the route keeps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// The element's nearest window.
    pub window: WindowHandle,
    /// The element, then its ancestors, nearest first.
    pub steps: Vec<RouteStep>,
    /// Where the chain goes on above them.
    pub end: RouteEnd,
    /// What the listener read of the element and each ancestor, nearest
    /// first: the properties of Core's set, the description whatever the
    /// settings, which the outpost's layout judgment reads.
    pub read: Vec<PropertyValues>,
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
    /// Whether UI Automation does not count it both a control and a
    /// content element ([`crate::NodeDetails::uia_layout`]), which the
    /// reader reports for Core to judge it by; `false` for an MSAA node.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub uia_layout: bool,
    /// Whether it has a description that is not blank, whatever the theme
    /// reports ([`crate::NodeDetails::described`]), which the reader
    /// reports for Core to judge it by; `false` for an MSAA node.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub described: bool,
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
    /// A terminal's text operation's reply.
    Text(TextReply),
    /// A batch's: one outcome per step, in order.
    Steps(Vec<StepOutcome>),
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
    /// The object does not support what was asked; a reader that does
    /// not yet run batches refuses every batch so.
    Unsupported,
    /// The batch did not parse ([`crate::batch::parse`]), and none of it
    /// ran: a defect of Core's.
    Malformed(Malformed),
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
        match &mut self.request {
            Question::Batch(batch) => batch.visit_nodes(&mut unstamp),
            Question::Terminal(ask) => unstamp(&mut ask.node),
        }
    }
}

impl Answer {
    /// Stamps `outpost` on every node id the answer carries, as
    /// [`crate::protocol::OutpostToCore::assign_outpost`] stamps every
    /// message on an outpost's pipe.
    pub fn assign_outpost(&mut self, outpost: crate::OutpostId) {
        self.visit(&mut |node| *node = node.with_outpost(outpost), &mut |_| {});
    }

    /// The node ids and text anchors the answer carries, in order, but for
    /// a text reply's anchors, which the protocol's own walk of a text
    /// reply names.
    #[must_use]
    pub fn named(&self) -> (Vec<NodeId>, Vec<TextAnchor>) {
        let mut copy = self.clone();
        let mut nodes = Vec::new();
        let mut anchors = Vec::new();
        copy.visit(&mut |node| nodes.push(*node), &mut |anchor| {
            anchors.push(*anchor);
        });
        (nodes, anchors)
    }

    /// Calls `nodes` on every node id and `anchors` on every text anchor
    /// the answer carries, but for a text reply's anchors.
    fn visit(
        &mut self,
        nodes: &mut dyn FnMut(&mut NodeId),
        anchors: &mut dyn FnMut(&mut TextAnchor),
    ) {
        if let Outcome::Result(Reply::Steps(outcomes)) = &mut self.outcome {
            crate::batch::visit_outcomes(outcomes, nodes, anchors);
        }
    }
}

impl FocusRead {
    /// Calls `nodes` on every node id the read carries, the focus, its
    /// ancestors and the foreground, and `anchors` on every text anchor of
    /// its caret: its line's start, the selection's ends and the baseline.
    pub fn visit(
        &mut self,
        nodes: &mut dyn FnMut(&mut NodeId),
        anchors: &mut dyn FnMut(&mut TextAnchor),
    ) {
        let reads = std::iter::once(&mut self.focus.read)
            .chain(
                self.ancestors
                    .iter_mut()
                    .map(|ancestor| &mut ancestor.node.read),
            )
            .chain(
                self.foreground
                    .iter_mut()
                    .map(|foreground| &mut foreground.read),
            );
        for read in reads {
            if let Some(node) = &mut read.node {
                nodes(node);
            }
        }
        if let FocusCaret::Read(read) = &mut self.caret {
            anchors(&mut read.caret.line.start);
            if let Some(selection) = &mut read.caret.selection {
                anchors(&mut selection.range.start.anchor);
                anchors(&mut selection.range.end.anchor);
            }
            anchors(&mut read.baseline);
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

/// What Windows names in front when a check after a menu, popup or
/// switcher end comes due ([`QueueInput::Due`]), read by Core's shell with
/// calls to Windows only, none into an application.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveFront {
    /// This window is in front.
    Window(FrontWindow),
    /// No window is in front (`GetForegroundWindow` names none).
    Nothing,
    /// The window in front went before its owner was read, or is a console
    /// window whose console host cannot be read: no reader can be asked
    /// about it.
    Gone,
}

/// The foreground window as a request names it, read live
/// ([`LiveFront::Window`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontWindow {
    /// The window `GetForegroundWindow` names, or for a ghost window the
    /// hung window it stands in for, as the listener's report names it
    /// ([`ForegroundReport::window`]).
    pub window: WindowHandle,
    /// The top of its owner chain.
    pub root_owner: WindowHandle,
    /// The process whose outpost reads it: the window's owner, or for a
    /// console window its console host.
    pub process: Pid,
    /// The thread that owns the window.
    pub thread: u32,
    /// Its class.
    pub class: String,
}

/// The foreground record as the listener reports it to the event queue:
/// the window `GetForegroundWindow` names, once it names it, from which
/// Core decides attention and what the keyboard hook reads of the
/// foreground ([`crate::HookForeground`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForegroundReport {
    /// The window, or for a ghost window in front, the hung window it
    /// stands in for, which attention and the foreground test judge.
    pub window: WindowHandle,
    /// The ghost window in front, when one stands in for `window`: the
    /// keyboard hook's window is `ghost.unwrap_or(window)`, the window
    /// `GetForegroundWindow` names.
    pub ghost: Option<WindowHandle>,
    /// The top of its owner chain.
    pub root_owner: WindowHandle,
    /// The application holding attention: the window's owner, or for a
    /// console window its console host.
    pub process: Pid,
    /// The thread that owns the window, which keys its backend verdict.
    pub thread: u32,
    /// Its class.
    pub class: String,
    /// Whether the window in front is a console host window, whose
    /// keyboard layout is its event's thread's: `thread` then names the
    /// console host's own thread, 0 when no event has named it yet.
    pub console: bool,
    /// The integrity level of the process that owns the window in front
    /// (its mandatory label's relative identifier, such as `0x2000` for
    /// medium and `0x3000` for high), or `None` when it cannot be read.
    pub integrity: Option<u32>,
    /// Verbatim's own integrity level, or `None` when it cannot be read.
    pub own_integrity: Option<u32>,
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
    /// ([`crate::batch::Batch::reads_focus_if_focused`]) finds no
    /// foreground window, as NVDA's fake focus then reads the desktop.
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
    /// A reader of `from` reported a window hung
    /// ([`crate::protocol::ListenerToCore::Hung`],
    /// [`crate::protocol::OutpostToCore::Hung`]).
    WindowHung {
        /// The reader's instance, whose frames Core admits only while it
        /// is the current one.
        from: Instance,
        /// The report.
        hung: crate::tell::Hung,
    },
    /// The time Core last asked to be woken at came.
    Due {
        /// What Windows names in front, read live by Core's shell as it
        /// makes this input, while a check after a menu, popup or switcher
        /// end waits for its time; `None` while none waits. The checks this
        /// wake brings ask by it, as NVDA's fake focus asks Windows for
        /// the foreground window when its timer runs, since the listener's
        /// foreground report can come later than the check (Dickson,
        /// 2026-11-26). The one read of Windows' state the shell makes for
        /// a decision of Core's.
        in_front: Option<LiveFront>,
    },
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
        request: Question,
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
        /// How it ended: the reader's answer, or ended or stopped when
        /// Core no longer awaits it.
        outcome: Outcome,
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
    /// waiting on it, and forgets the window's MSAA nodes, text facts and
    /// classic-read mark before any later work.
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
    /// A handle of one numbered batch was given to another, a defect in
    /// Core: log it. The step that took it, or the read of it, was
    /// answered failed.
    ForeignHandle {
        /// The number of the batch the handle was given to, `None` for a
        /// batch not numbered.
        batch: Option<u64>,
        /// The number of the batch the handle belongs to, likewise.
        handle: Option<u64>,
    },
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
        use crate::batch::{Descendant, Descended, Done, StepOutcome};
        let mut answer = Answer {
            number: RequestNumber(1),
            outcome: Outcome::Result(Reply::Steps(vec![
                StepOutcome::Done(Done::Read(node_read(1))),
                StepOutcome::Done(Done::Descendants(Descended {
                    nodes: [2, 3]
                        .map(|number| Descendant {
                            node: node_read(number),
                            parent: None,
                            root: 0,
                        })
                        .to_vec(),
                    complete: true,
                })),
            ])),
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
    fn a_batch_and_its_answer_travel_as_json_and_back() {
        use crate::batch::{Batch, Done, Op, Placement, Step, StepOutcome, Subject};
        let ask = Ask {
            number: RequestNumber(7),
            window: WindowHandle(5),
            verdict: WindowVerdict::ProbeFirst,
            request: Batch {
                steps: vec![Step {
                    op: Op::Read {
                        subject: Subject::Kept {
                            listener: 1,
                            token: crate::tell::ElementToken(3),
                            keep: false,
                        },
                        set: vec![PropertyId::Name, PropertyId::Role],
                    },
                    when: None,
                }],
                placement: Placement::Queue,
                spare: None,
            }
            .into(),
        };
        let json = serde_json::to_string(&ask).unwrap();
        assert_eq!(serde_json::from_str::<Ask>(&json).unwrap(), ask);

        let mut read = node_read(1);
        read.node = None;
        let answer = Answer {
            number: RequestNumber(7),
            outcome: Outcome::Result(Reply::Steps(vec![StepOutcome::Done(Done::Read(read))])),
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
