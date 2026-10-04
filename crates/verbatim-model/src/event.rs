//! Normalized events, reducer inputs, and reducer effects.
//!
//! The reducer is strictly accessibility-shaped: its inputs are normalized
//! events, fetch completions, and timer ticks; its effects are speech and
//! fetches. Imperative concerns — showing the menu, quitting — never appear
//! here; they are routed by the shell's gesture router (architecture
//! section 2, amended during M1 planning).

use serde::{Deserialize, Serialize};

use crate::speech::Utterance;
use crate::tree::{Backend, NodeSnapshot, StateSet};
use crate::{NodeId, OutpostId, TraceId};

/// A Windows process identifier, used to name the application an outpost
/// watches and to key supervisor state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pid(pub u32);

impl std::fmt::Display for Pid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A native window handle, as an opaque number.
///
/// Window handles are the one identity that means the same thing in every
/// process, so they are how windows are compared across outposts (node ids
/// are per outpost).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowHandle(pub u64);

/// Facts about the window an event concerns, read by its outpost with local
/// calls when the event is observed. The reducer classifies every event
/// against its attention record with these (decision D14 as amended by the
/// outpost redesign; `docs/parity.md`, "Event acceptance").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowFacts {
    /// The event's top-level window.
    pub top_level: WindowHandle,
    /// The top of the event window's owner chain.
    pub root_owner: WindowHandle,
    /// Whether the window or its top-level window is topmost (menus, combo
    /// box popups, the task switcher).
    pub topmost: bool,
    /// For a `Windows.UI.Core` window only: whether it is the input thread's
    /// active window or inside it, the test NVDA uses for UWP windows.
    /// `None` for every other window class.
    #[serde(default)]
    pub under_active_window: Option<bool>,
    /// Whether the window is in the system's foreground window when the
    /// event is read: its top-level window is the foreground window, or its
    /// root owner is the foreground window or the foreground window's root
    /// owner — NVDA's live foreground test. Windows can raise a window's
    /// foreground event while refusing it the foreground, and raises none
    /// when it is given the foreground later, so this is how the reducer
    /// learns the foreground moved without a foreground fact.
    #[serde(default)]
    pub in_foreground: bool,
}

/// Identifies one in-flight fetch so its completion can re-enter the reducer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueryId(pub u64);

/// A property change carried by [`NormalizedEvent::PropertyChanged`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PropertyChange {
    /// The accessible name changed.
    Name(Option<String>),
    /// The value changed (also has a dedicated event; kept here for
    /// backends that report it as a generic property change).
    Value(Option<String>),
    /// The state set changed, carrying the complete new set (not a delta) —
    /// sourced from MSAA `EVENT_OBJECT_STATECHANGE` and UIA state-bearing
    /// property changes. The reducer diffs against its stored snapshot to
    /// decide what to announce (a check box toggling, a control becoming
    /// unavailable).
    States(StateSet),
}

/// An accessibility event, normalized by an outpost from either backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NormalizedEvent {
    /// Keyboard focus moved to a node.
    FocusChanged {
        /// Snapshot of the newly focused node.
        node: NodeSnapshot,
        /// Whether this focus is a foreground change: the window became the
        /// system's foreground window, and `node` is that window. Following
        /// NVDA, a foreground change is a focus on the window; the reducer
        /// always accepts it and ignores it when focus is already inside
        /// that window (`docs/parity.md`, "Window announcement on switching
        /// applications").
        #[serde(default)]
        foreground: bool,
        /// The focused node's ancestors, outermost first, walked by the
        /// outpost before emitting (deadline-guarded; empty when the walk
        /// timed out or the backend could not answer). Carried on the event
        /// rather than fetched afterward so the reducer can speak entered
        /// containers *before* the control, in NVDA's order, without
        /// holding an Interrupt announcement hostage to an async round
        /// trip. `#[serde(default)]` keeps events recorded before this
        /// field existed deserializing unchanged.
        #[serde(default)]
        ancestors: Vec<NodeSnapshot>,
        /// The outpost could not read the ancestors in time (a busy
        /// application), so `ancestors` is empty for want of an answer, not
        /// because the node has none: the reducer keeps the previous focus's
        /// chain rather than treating every container as newly entered.
        #[serde(default)]
        ancestors_unknown: bool,
        /// The selected child of a newly focused selection container (a
        /// list's selected item, a tab control's active tab), fetched by
        /// the outpost alongside the ancestors — only for container roles,
        /// `None` otherwise or when nothing is selected or the query
        /// failed. Carried on the event for the same reason the ancestors
        /// are: the reducer speaks it immediately after the container
        /// without a round trip. `#[serde(default)]` for wire
        /// compatibility.
        #[serde(default)]
        selected_child: Option<NodeSnapshot>,
    },
    /// A property of a node changed.
    PropertyChanged {
        /// The node whose property changed.
        node_id: NodeId,
        /// Which property, with its new value.
        change: PropertyChange,
    },
    /// The value of a node changed (slider drag, combo selection, text edit).
    ValueChanged {
        /// The node whose value changed.
        node_id: NodeId,
        /// The new value.
        value: Option<String>,
    },
    /// A node was selected within its container (MSAA `EVENT_OBJECT_SELECTION`
    /// and its `SELECTIONADD`/`SELECTIONREMOVE`/`SELECTIONWITHIN` siblings;
    /// UIA `SelectionItem_ElementSelected`). The reducer announces it while
    /// focus rests on a selection container, once per newly selected item.
    SelectionChanged {
        /// Snapshot of the selected node.
        node: NodeSnapshot,
    },
    /// A node was selected inside an element the focus controls (its UIA
    /// `ControllerFor` relation), such as a search result while the focus
    /// stays in the search box. The reducer speaks it as a focus and moves
    /// the navigator to it, while `controller` is still the focus.
    ControlledSelection {
        /// The focus that controls the list the node is in.
        controller: NodeId,
        /// Snapshot of the selected node.
        node: NodeSnapshot,
    },
    /// A UIA `AutomationNotification` event: an app-initiated announcement
    /// (for example Windows 11's snap-layout hints) carried through
    /// verbatim. The reducer speaks its display string, if any, interrupting
    /// for `MostRecent`/`ImportantMostRecent` processing and queuing
    /// otherwise (NVDA's `event_UIA_notification`).
    Notification {
        /// The node the notification concerns.
        node_id: NodeId,
        /// The notification payload.
        notification: Notification,
    },
    /// A toast notification (an MSAA `EVENT_SYSTEM_ALERT` from a window whose
    /// parent has the class `ToastChildWindowClass`): the alerting object,
    /// spoken queued and accepted from any application (decision D14), as
    /// NVDA's notification behavior speaks it.
    Alert {
        /// The alerting object.
        node: NodeSnapshot,
    },
}

impl NormalizedEvent {
    /// Stamps `outpost` on every node id this event carries, the stamp Core
    /// applies according to the pipe the event arrived on.
    pub fn assign_outpost(&mut self, outpost: OutpostId) {
        match self {
            NormalizedEvent::FocusChanged {
                node,
                ancestors,
                selected_child,
                ..
            } => {
                node.assign_outpost(outpost);
                for ancestor in ancestors {
                    ancestor.assign_outpost(outpost);
                }
                if let Some(selected) = selected_child {
                    selected.assign_outpost(outpost);
                }
            }
            NormalizedEvent::SelectionChanged { node } | NormalizedEvent::Alert { node } => {
                node.assign_outpost(outpost);
            }
            NormalizedEvent::ControlledSelection { controller, node } => {
                *controller = controller.with_outpost(outpost);
                node.assign_outpost(outpost);
            }
            NormalizedEvent::PropertyChanged { node_id, .. }
            | NormalizedEvent::ValueChanged { node_id, .. }
            | NormalizedEvent::Notification { node_id, .. } => {
                *node_id = node_id.with_outpost(outpost);
            }
        }
    }
}

/// What kind of change a [`NormalizedEvent::Notification`] reports —
/// UIA's `NotificationKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NotificationKind {
    /// An item was added.
    ItemAdded,
    /// An item was removed.
    ItemRemoved,
    /// An action completed.
    ActionCompleted,
    /// An action was aborted.
    ActionAborted,
    /// Any other kind of notification.
    Other,
}

/// How urgently a [`NormalizedEvent::Notification`] should be processed —
/// UIA's `NotificationProcessing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NotificationProcessing {
    /// Important; process every notification of this kind.
    ImportantAll,
    /// Important; only the most recent notification of this kind matters.
    ImportantMostRecent,
    /// Process every notification of this kind.
    All,
    /// Only the most recent notification of this kind matters.
    MostRecent,
    /// Process the current notification, then only the most recent of any
    /// further ones that arrive while it is being processed.
    CurrentThenMostRecent,
}

/// The payload of a UIA `AutomationNotification` event, normalized
/// (architecture section 4). Announced by the reducer through
/// [`NormalizedEvent::Notification`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// What kind of change this reports.
    pub kind: NotificationKind,
    /// How urgently it should be processed.
    pub processing: NotificationProcessing,
    /// The human-readable text the source app supplied, if any.
    pub display_string: Option<String>,
    /// An opaque id the source app uses to correlate related notifications,
    /// if any.
    pub activity_id: Option<String>,
}

/// What a completed fetch produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FetchResult {
    /// A fresh snapshot of the requested node.
    Node(NodeSnapshot),
    /// The node no longer exists.
    Gone,
    /// A navigation query found no node in the requested direction — a
    /// root's parent, a last child's next sibling, a leaf's first child.
    /// A first-class outcome, distinct from `Gone` (the starting node is
    /// fine, the neighbor simply does not exist).
    NoNeighbor,
    /// The application did not answer in time, or the read failed for
    /// another reason: nothing is known about the node or its neighbor, so
    /// the navigator stays where it is, as NVDA's stays when a call to a
    /// busy application is cancelled.
    Unanswered,
}

impl FetchResult {
    /// Stamps `outpost` on the node id a found node carries.
    pub fn assign_outpost(&mut self, outpost: OutpostId) {
        if let FetchResult::Node(node) = self {
            node.assign_outpost(outpost);
        }
    }
}

/// What to fetch: one object-navigation step from a node to a neighbor
/// (roadmap M3). Later milestones add text ranges and subtree queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum QueryKind {
    /// The node's parent.
    Parent,
    /// The node's next sibling in tree order.
    NextSibling,
    /// The node's previous sibling in tree order.
    PreviousSibling,
    /// The node's first child.
    FirstChild,
}

/// A fetch request emitted by the reducer and executed by an outpost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Correlates the eventual [`FetchResult`] back to this request.
    pub query_id: QueryId,
    /// The node to read. Its outpost is the one asked.
    pub node_id: NodeId,
    /// What to read.
    pub kind: QueryKind,
}

/// One input to the reducer. Strictly accessibility-shaped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "an input is built once per event and moved, never stored in bulk"
)]
pub enum Input {
    /// A normalized accessibility event from an outpost.
    Event {
        /// Trace ID minted when the OS event was first observed.
        trace_id: TraceId,
        /// Milliseconds since the Unix epoch when the OS event was first
        /// observed — the same stamp the outpost put on the wire. The
        /// latency record reads it, and so does the reducer, to drop a focus
        /// observed before the focus it already applied
        /// (`docs/parity.md`, "Stale focus events").
        #[serde(default)]
        observed_at_ms: u64,
        /// The application the event came from: information about its
        /// source, for attention and logs. Node ids carry the outpost.
        source: Pid,
        /// Which backend sourced the event (diagnostics only).
        backend: Backend,
        /// Facts about the window the event concerns, or `None` when the
        /// event has no window to read them from.
        #[serde(default)]
        window: Option<WindowFacts>,
        /// The event itself.
        event: NormalizedEvent,
    },
    /// A previously requested fetch finished.
    FetchCompleted {
        /// Trace ID of the input that caused the fetch.
        trace_id: TraceId,
        /// The request this result answers.
        query_id: QueryId,
        /// What the request asked for, echoed by the outpost.
        kind: QueryKind,
        /// What the outpost found.
        result: FetchResult,
    },
    /// An outpost incarnation ended: it exited, was killed, or was retired.
    /// Every node id it issued is dead from now on.
    OutpostEnded {
        /// The incarnation that ended.
        outpost: OutpostId,
    },
    /// A review or object-navigation command, from a bound gesture
    /// (roadmap M3). The imperative shell translates a keyboard script
    /// into this; the reducer runs it against its navigator object and
    /// review cursor.
    Command {
        /// Trace ID minted when the triggering key was observed, carried
        /// through so a command's speech joins the latency timeline.
        trace_id: TraceId,
        /// Which command.
        command: ReviewCommand,
        /// How many times the gesture was pressed in quick succession, zero
        /// for the first press: report-current-object reports on 0, spells
        /// on 1, copies on 2 (NVDA's multi-press semantics). Other commands
        /// ignore it.
        repeat: u8,
    },
    /// An activation the reducer asked for finished: `activated` is whether
    /// the navigator object, or one of its ancestors, was activated, and
    /// `action` the name of the action performed, if it has one. The
    /// reducer speaks the action's name, NVDA's "Activate" for an action
    /// with none, or "No action".
    ActivationCompleted {
        /// Trace ID of the command that asked for the activation.
        trace_id: TraceId,
        /// Whether anything was activated.
        activated: bool,
        /// The name of the action performed; `None` for an unnamed one.
        action: Option<ActionName>,
    },
    /// Periodic timer tick, for time-based policies. Unused by M1 logic but
    /// part of the frozen vocabulary so adding policies is not a breaking
    /// change.
    Tick,
}

/// The name of an action an activation performed, which NVDA speaks after
/// performing it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ActionName {
    /// UIA's Invoke pattern, which NVDA names "invoke".
    Invoke,
    /// The application's own name for an object's default action (MSAA's
    /// default action, such as "Press"), spoken as it is.
    Named(String),
}

/// A review-cursor or object-navigation command (roadmap M3), the
/// model-level vocabulary the keyboard layer's scripts map onto so the
/// reducer never depends on input-crate types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReviewCommand {
    /// Report the current navigator object (spell on the second press in a
    /// streak, copy its name and value on the third).
    ReportObject,
    /// Move the navigator object to its parent.
    Parent,
    /// Move the navigator object to its next sibling.
    NextSibling,
    /// Move the navigator object to its previous sibling.
    PreviousSibling,
    /// Move the navigator object to its first child.
    FirstChild,
    /// Move the navigator object (and review cursor) back to the focus.
    ToFocus,
    /// Activate the current navigator object (invoke, toggle, or default
    /// action).
    Activate,
    /// Move the review cursor to the first line of the navigator object.
    ReviewTop,
    /// Move the review cursor to the previous line.
    ReviewPreviousLine,
    /// Report the review cursor's current line.
    ReviewCurrentLine,
    /// Move the review cursor to the next line.
    ReviewNextLine,
    /// Move the review cursor to the previous word.
    ReviewPreviousWord,
    /// Report the review cursor's current word.
    ReviewCurrentWord,
    /// Move the review cursor to the next word.
    ReviewNextWord,
    /// Move the review cursor to the start of the current line.
    ReviewStartOfLine,
    /// Move the review cursor to the previous character.
    ReviewPreviousCharacter,
    /// Report the review cursor's current character.
    ReviewCurrentCharacter,
    /// Move the review cursor to the next character.
    ReviewNextCharacter,
    /// Move the review cursor to the end of the current line.
    ReviewEndOfLine,
    /// Move the review cursor to the last line of the navigator object.
    ReviewBottom,
}

/// A non-speech sound the reducer can ask for, named semantically so
/// presentation themes (milestone M11) decide what it actually sounds like.
///
/// Reserved vocabulary grows variant by variant as policies land; the first
/// consumer is the recovery ladder's not-responding cue (milestone M3
/// outpost hardening).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Earcon {
    /// A deadline expired inside a cross-process accessibility call: the
    /// application is not responding and the reducer proceeded with stale
    /// data (architecture section 1, recovery ladder rung one).
    AppNotResponding,
}

/// One effect emitted by the reducer and executed by the imperative shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Effect {
    /// Queue an utterance in the speech pipeline.
    Speak(Utterance),
    /// Cancel current and queued speech.
    StopSpeech,
    /// The focus has changed: drop queued and playing focus speech that is
    /// no longer valid ([`crate::FocusValidity`]), together with everything
    /// queued before the newest such utterance.
    DropExpiredSpeech(crate::FocusNow),
    /// Ask an outpost for more data; completion re-enters as
    /// [`Input::FetchCompleted`].
    Fetch(Query),
    /// Play a non-speech sound (decision D12; themed in milestone M11).
    PlayEarcon(Earcon),
    /// Activate a node — invoke, toggle, or its default action — in the
    /// application that owns it. Fire-and-forget from the reducer's view;
    /// the shell routes it to the outpost.
    Activate {
        /// The node to activate. Its outpost is the one asked.
        node_id: NodeId,
    },
    /// Copy text to the system clipboard through the shell's shared
    /// clipboard helper, which owns the spoken confirmation. The reducer
    /// stays pure — it never touches the clipboard itself — so the
    /// report-object triple-press emits this rather than doing the copy.
    CopyToClipboard(String),
}
