//! The private Core-outpost protocol (architecture section 1).
//!
//! Transport: two anonymous pipes created by the supervisor with inheritable
//! child ends, their handle values passed on the outpost command line — no
//! named endpoint exists, so there is nothing to discover or secure. Framing
//! is newline-delimited compact JSON: debuggable, flight-recorder friendly,
//! and isolated behind [`write_message`] and [`read_message`] so the
//! encoding can be renegotiated later without touching either end's logic.

#![forbid(unsafe_code)]

use std::io::{self, BufRead, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// When an event reached each point on its way through the listener and
/// the outpost, for the latency log Core writes for every announcement:
/// microseconds since the Unix epoch, 0 for a point it did not pass. Also
/// the cross-process calls the outpost made for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventTiming {
    /// How many milliseconds before it was observed Windows raised the
    /// event; only `WinEvents` carry the time they were raised.
    pub raised_ms_ago: Option<u32>,
    /// When the listener or the outpost observed it.
    pub observed_at_us: u64,
    /// When the outpost received it from Core, for an event the listener
    /// observed.
    pub relayed_at_us: u64,
    /// When the outpost's worker took it from its queue.
    pub dequeued_at_us: u64,
    /// When the evidence that answered a caret key's watch, after its first
    /// check found none, was taken from the outpost's queue
    /// (`TextOp::AwaitCaret`); 0 for anything else.
    pub awaited_at_us: u64,
    /// When the outpost sent the resulting event to Core.
    pub published_at_us: u64,
    /// The cross-process calls the outpost's worker made between taking the
    /// entry from its queue and publishing this message, by kind
    /// (`docs/performance.md`). Zero from the listener, which makes none, and
    /// from an older peer.
    pub calls: CallCounts,
    /// The part of `calls` a caret key's watch made in the checks that found
    /// no evidence, before `awaited_at_us`.
    pub awaited_calls: CallCounts,
}

/// Microseconds since the Unix epoch, the clock [`EventTiming`] and the
/// latency log share across processes.
#[must_use]
pub fn now_us() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros(),
    )
    .unwrap_or(u64::MAX)
}
use verbatim_model::{
    Backend, CallCounts, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, Notification,
    OutpostId, Pid, QueryKind, Role, StateSet, TextOp, TextReply, TraceId, TreeNode, WindowFacts,
};

/// The identity-free contents of a UIA focus element, as the focus listener
/// captures them (decision D13): the runtime id plus role, name, value,
/// states, and details — everything a [`NodeSnapshot`](verbatim_model::NodeSnapshot)
/// carries except its [`NodeId`](verbatim_model::NodeId). The node id is
/// deliberately absent: identity is minted per application inside that
/// application's own outpost and never crosses a process boundary, so the
/// receiving outpost mints it from the runtime id when it rebuilds the
/// snapshot. Mirrors `verbatim_uia::map::CachedUiaParts`, which is what the
/// listener reads by cached property access.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiaSnapshotFact {
    /// The UIA runtime id, the key the receiving outpost mints its node id from.
    pub runtime_id: Vec<i32>,
    /// Normalized role (already refined for toggle buttons).
    pub role: Role,
    /// Accessible name, if any.
    pub name: Option<String>,
    /// Current value.
    pub value: Option<String>,
    /// Current states.
    pub states: StateSet,
    /// The optional properties beyond the core four.
    pub details: NodeDetails,
}

/// A fact the listener forwards to Core, tagged with the pid Core routes it
/// to (decision D13). The listener reads only what the event itself carries
/// plus hang-safe local reads (the owning pid, a cached window handle), never
/// a cross-process call: a UIA fact is built from the element's cached
/// properties, an MSAA fact forwards the raw `WinEvent` address untouched.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListenerFact {
    /// The application the fact concerns.
    pub pid: Pid,
    /// The fact itself, as the target outpost receives it.
    pub fact: DeliveredFact,
}

impl ListenerFact {
    /// The pid the supervisor routes this fact to.
    #[must_use]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Strips the pid, yielding the [`DeliveredFact`] the supervisor hands to
    /// the target outpost once routing is done.
    #[must_use]
    pub fn into_delivered(self) -> DeliveredFact {
        self.fact
    }
}

/// A fact the supervisor delivers to a target outpost (decision D13): what
/// the listener captured, minus the pid, since routing is already done. The
/// outpost's worker reads, arbitrates, and reports it exactly as it does the
/// events it hooks itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeliveredFact {
    /// A UIA focus change: the element's cached native window handle (0 when
    /// the element is not itself a window), the keyboard focus window when
    /// the event was captured, and its cached snapshot parts.
    UiaFocus {
        /// The element's cached native window handle, or 0 when it is not a
        /// window in its own right.
        hwnd: isize,
        /// For an element that is not a window: the foreground thread's
        /// keyboard focus window when the listener captured the event, if it
        /// belongs to the element's process, else 0. When the event is
        /// current it is the window hosting a windowless element, such as
        /// Explorer's items view, read locally where NVDA walks up to the
        /// element's nearest window, so a focus the outpost cannot resolve
        /// can still be judged against the foreground window; for a late
        /// event in an application with several windows it may be another
        /// of them. 0 when the focus had already moved to another process.
        #[serde(default)]
        focus_window: isize,
        /// The element's cached snapshot parts.
        snapshot: UiaSnapshotFact,
    },
    /// An MSAA focus change: the raw `WinEvent` address.
    MsaaFocus {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
    /// A foreground change: the new foreground window.
    Foreground {
        /// The new foreground window handle.
        hwnd: isize,
    },
    /// A popup menu opening: the raw `WinEvent` address.
    MenuPopup {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
    /// A UIA menu opening (`MenuOpened`), which NVDA treats as a focus on
    /// the menu.
    UiaMenuOpened {
        /// The element's cached native window handle, or 0.
        hwnd: isize,
        /// The menu's cached snapshot parts.
        snapshot: UiaSnapshotFact,
    },
    /// A UIA element selected within its container
    /// (`SelectionItem_ElementSelected`).
    UiaSelection {
        /// The element's cached native window handle, or 0.
        hwnd: isize,
        /// The selected element's cached snapshot parts.
        snapshot: UiaSnapshotFact,
    },
    /// A UIA notification (`AutomationNotification`).
    UiaNotification {
        /// The element's cached native window handle, or 0.
        hwnd: isize,
        /// The raising element's cached snapshot parts.
        snapshot: UiaSnapshotFact,
        /// The notification.
        notification: Notification,
    },
    /// An MSAA alert (`EVENT_SYSTEM_ALERT`): the raw `WinEvent` address.
    Alert {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
    /// A tooltip window shown (`EVENT_OBJECT_SHOW` on `tooltips_class32`):
    /// the raw `WinEvent` address.
    Show {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
}

impl DeliveredFact {
    /// Whether this fact may start an outpost for an application that has
    /// none (outpost redesign, "The focus listener"): focus, foreground,
    /// menu, notification, and alert facts may; a selection without an
    /// outpost is dropped.
    #[must_use]
    pub fn may_start_outpost(&self) -> bool {
        !matches!(self, DeliveredFact::UiaSelection { .. })
    }

    /// The object and kind this fact concerns, for NVDA's limiter rule (one
    /// waiting entry per object and kind, a newer one replacing it). `None`
    /// for a notification, whose text makes each one distinct.
    #[must_use]
    pub fn key(&self) -> Option<FactKey> {
        Some(match self {
            DeliveredFact::Foreground { hwnd } => FactKey::Foreground(*hwnd),
            DeliveredFact::MsaaFocus {
                hwnd,
                id_object,
                id_child,
            } => FactKey::MsaaFocus(*hwnd, *id_object, *id_child),
            DeliveredFact::UiaFocus { snapshot, .. } => {
                FactKey::UiaFocus(snapshot.runtime_id.clone())
            }
            DeliveredFact::MenuPopup {
                hwnd,
                id_object,
                id_child,
            } => FactKey::MenuPopup(*hwnd, *id_object, *id_child),
            DeliveredFact::UiaMenuOpened { snapshot, .. } => {
                FactKey::UiaMenuOpened(snapshot.runtime_id.clone())
            }
            DeliveredFact::UiaSelection { snapshot, .. } => {
                FactKey::UiaSelection(snapshot.runtime_id.clone())
            }
            DeliveredFact::Alert {
                hwnd,
                id_object,
                id_child,
            } => FactKey::Alert(*hwnd, *id_object, *id_child),
            DeliveredFact::Show {
                hwnd,
                id_object,
                id_child,
            } => FactKey::Show(*hwnd, *id_object, *id_child),
            DeliveredFact::UiaNotification { .. } => return None,
        })
    }
}

/// The object and kind a [`DeliveredFact`] concerns.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FactKey {
    /// A foreground change to this window.
    Foreground(isize),
    /// An MSAA focus at this address.
    MsaaFocus(isize, i32, i32),
    /// A UIA focus on this runtime id.
    UiaFocus(Vec<i32>),
    /// An MSAA menu opening at this address.
    MenuPopup(isize, i32, i32),
    /// A UIA menu opening on this runtime id.
    UiaMenuOpened(Vec<i32>),
    /// A UIA selection of this runtime id.
    UiaSelection(Vec<i32>),
    /// An MSAA alert at this address.
    Alert(isize, i32, i32),
    /// A tooltip shown at this address.
    Show(isize, i32, i32),
}

/// Messages from the Core-side supervisor to an outpost.
///
/// An outpost's target application is fixed at spawn (decision D9: one
/// outpost per application, for its whole life) and passed on its command
/// line, not by any message here. Core ends an outpost by closing its job
/// handle; there is no shutdown message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SupervisorToOutpost {
    /// Forces one backend for every window of the target application,
    /// overriding arbitration; used by tests and per-app config overrides.
    /// `None` restores normal arbitration.
    SetBackendOverride {
        /// The forced backend, or `None` for normal arbitration.
        backend_override: Option<Backend>,
    },
    /// Delivers a focus fact the focus listener captured, for this outpost's
    /// target application (decision D13), threading the listener's
    /// `trace_id` and `observed_at_ms` through to the emitted event so the
    /// latency timeline starts at the real OS event rather than this
    /// delivery.
    DeliverFact {
        /// Trace ID the listener minted when it observed the OS event.
        trace_id: TraceId,
        /// Milliseconds since the Unix epoch when the listener observed the
        /// OS event — the first point on the keypress-to-audio timeline.
        observed_at_ms: u64,
        /// When it was raised and observed, for the latency log.
        #[serde(default)]
        timing: EventTiming,
        /// The routed fact, minus the pid (routing is done).
        fact: DeliveredFact,
    },
    /// A query, answered by exactly one [`OutpostToSupervisor::Reply`]
    /// carrying the same `request_id`.
    Query {
        /// Trace ID of the input that caused the query.
        trace_id: TraceId,
        /// Core's id for this request, echoed in the reply.
        request_id: u64,
        /// What to do.
        query: Query,
    },
    /// Withdraws a query that has not started: it is answered
    /// [`QueryOutcome::NotStarted`] instead of being run. A query already
    /// running is not affected.
    Cancel {
        /// The query to withdraw.
        request_id: u64,
    },
    /// Liveness probe; answered by [`OutpostToSupervisor::Pong`].
    Ping {
        /// Echoed in the matching pong.
        seq: u64,
    },
    /// The details the active theme wants read for each node
    /// (`SrState::fetches` in `verbatim-core`): a detail whose indication
    /// is off is not read, which saves its cross-process call. Sent when an
    /// outpost starts and whenever the theme in use changes; everything is
    /// read until the first arrives.
    Fetches(verbatim_model::Fetches),
    /// How many of a change's newest lines a terminal read takes
    /// (`ReaderSettings::terminal_read_lines`): as many as the flood
    /// policy's limits can keep. Sent when an outpost starts and whenever
    /// the limits change; 30 until the first arrives.
    TerminalLines(u16),
    /// The nodes from this outpost that Core still holds (outpost redesign,
    /// "Held objects"), as the numbers the outpost issued, and the position
    /// of the last message from this outpost that Core has handled
    /// ([`OutpostToSupervisor::carries_nodes`] says which messages count).
    /// The outpost releases every other node it reported at or before that
    /// position, and answers a query for a released node "gone".
    NodesHeld {
        /// The held nodes' numbers.
        nodes: Vec<u64>,
        /// The text anchors Core holds in this outpost
        /// (`SrState::held_anchors` in `verbatim-core`): the outpost keeps
        /// these, and may forget any other once it has minted newer ones
        /// (the text protocol, `docs/crates/verbatim-model.md`).
        #[serde(default)]
        anchors: Vec<u64>,
        /// The position of the last message Core has handled.
        acknowledged: u64,
    },
}

/// What a [`SupervisorToOutpost::Query`] asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Query {
    /// The application's current foreground window, if it is the system's
    /// foreground window, and its focused control with that control's
    /// ancestors and selected child. Used at startup and after an outpost or
    /// the listener is replaced, in place of retrying until focus settles.
    FocusNow,
    /// One object-navigation step from `node_id` in the direction `kind`
    /// names.
    Navigate {
        /// The node to navigate from.
        node_id: NodeId,
        /// Which way: parent, next or previous sibling, or first child.
        kind: QueryKind,
    },
    /// Activate a node: UIA `Invoke`, `Toggle`, or the legacy default
    /// action; MSAA `accDoDefaultAction`.
    Activate {
        /// The node to activate.
        node_id: NodeId,
    },
    /// The node's ancestors, outermost first, capped at 64.
    Ancestors {
        /// The node whose ancestors are wanted.
        node_id: NodeId,
    },
    /// The application's tree from its top-level window, capped at a depth
    /// of 64 and 4096 nodes.
    DumpTree,
    /// A text request (milestone M4's text protocol, `TextOp` in
    /// `verbatim-model`): read, wait for the caret, select, or move the
    /// caret in a node's text. Answered [`QueryResult::Text`].
    Text {
        /// The node whose text to use.
        node_id: NodeId,
        /// What to do.
        op: TextOp,
    },
}

/// The one outcome of a query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "a reply is built once and moved, never stored in bulk"
)]
pub enum QueryOutcome {
    /// The query finished.
    Done(QueryResult),
    /// The node the query named is no longer reachable.
    Gone,
    /// The query failed, for the reason given.
    Failed(String),
    /// The query was withdrawn or expired before it started: it had no side
    /// effects.
    NotStarted,
    /// The query started and passed its deadline: side effects, such as an
    /// activation, may already have happened.
    Abandoned,
}

/// What a finished query found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "a reply is built once and moved, never stored in bulk"
)]
pub enum QueryResult {
    /// The answer to [`Query::FocusNow`].
    Focus(FocusNow),
    /// The answer to [`Query::Navigate`]: the neighbor, or `None` for a
    /// genuine tree edge (not an error).
    Navigated(Option<NodeSnapshot>),
    /// The answer to [`Query::Activate`]: the name of the action performed,
    /// if it has one.
    Activated(Option<verbatim_model::ActionName>),
    /// The answer to [`Query::Ancestors`], outermost first.
    Ancestors(Vec<NodeSnapshot>),
    /// The answer to [`Query::DumpTree`].
    Tree(DumpedTree),
    /// The answer to [`Query::Text`]. Every answer the text protocol has,
    /// `NoText` and `Gone` among them, comes back this way, so Core hands it
    /// to the reducer unchanged.
    Text(TextReply),
}

/// The answer to [`Query::FocusNow`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusNow {
    /// The application's top-level window and its facts, when it is the
    /// system's foreground window.
    pub window: Option<(NodeSnapshot, WindowFacts)>,
    /// The focused control and its facts, when the application has one.
    pub focus: Option<FocusedControl>,
    /// When the outpost began reading the answer, in milliseconds since the
    /// Unix epoch: the answer's place among the focus events, which carry
    /// the time they were observed. A focus event observed before it is
    /// older than the answer, and one observed after it newer, as NVDA
    /// queues a focus it reads in order with the events around it.
    pub observed_at_ms: u64,
}

/// The focused control in a [`FocusNow`] answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusedControl {
    /// The control.
    pub node: NodeSnapshot,
    /// Its ancestors, outermost first.
    pub ancestors: Vec<NodeSnapshot>,
    /// The selected child, for a selection container.
    pub selected_child: Option<NodeSnapshot>,
    /// Its window's facts.
    pub window: Option<WindowFacts>,
}

/// Messages from an outpost to the Core-side supervisor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OutpostToSupervisor {
    /// First message after startup.
    Ready {
        /// The outpost's own process id.
        outpost_pid: Pid,
        /// The application it is watching.
        target_pid: Pid,
    },
    /// A normalized accessibility event.
    Event {
        /// Trace ID minted when the OS event was first observed in the
        /// outpost.
        trace_id: TraceId,
        /// Milliseconds since the Unix epoch when the OS event was first
        /// observed, which orders it against other outposts' events. A
        /// foreground change carries instead the time its window was
        /// confirmed as the foreground, since Windows raises the event
        /// before the change completes; `timing` keeps the first
        /// observation, where the latency timeline starts. Defaults to 0
        /// for messages from older peers.
        #[serde(default)]
        observed_at_ms: u64,
        /// Which backend sourced the event.
        backend: Backend,
        /// Facts about the window the event concerns, read with local calls
        /// when the event was observed; `None` when it had no window.
        #[serde(default)]
        window: Option<WindowFacts>,
        /// When the event reached each point in the outpost, for the
        /// latency log.
        #[serde(default)]
        timing: EventTiming,
        /// The event itself.
        event: NormalizedEvent,
    },
    /// The one answer to a [`SupervisorToOutpost::Query`].
    Reply {
        /// Trace ID carried through from the query.
        trace_id: TraceId,
        /// The request this answers.
        request_id: u64,
        /// What became of it.
        outcome: QueryOutcome,
        /// When the query reached each point in the outpost, and the
        /// cross-process calls answering it made, for the latency log.
        /// Default for a query withdrawn or abandoned, and from an older
        /// peer.
        #[serde(default)]
        timing: EventTiming,
    },
    /// Answer to [`SupervisorToOutpost::Ping`].
    Pong {
        /// The probed sequence number.
        seq: u64,
        /// How many of this outpost's workers have been abandoned to calls
        /// that passed their deadline and have not yet returned; the
        /// supervisor ends an outpost that piles them up.
        parked_count: usize,
    },
    /// A backend error worth reporting without dying — a failed event
    /// registration, an arbitration probe that keeps timing out.
    Fault {
        /// Human-readable detail for logs and diagnostics.
        detail: String,
    },
    /// A focus fact the focus listener captured (decision D13). Sent only by
    /// the listener process, never a per-application outpost; the supervisor
    /// intercepts it, routes it to the target's own outpost, and never
    /// forwards it to the app. The listener mints the `trace_id` and stamps
    /// `observed_at_ms` at observation, so the latency timeline starts at the
    /// real OS event.
    FocusFact {
        /// Trace ID minted when the listener observed the OS event.
        trace_id: TraceId,
        /// Milliseconds since the Unix epoch when the OS event was observed.
        observed_at_ms: u64,
        /// When it was raised and observed, for the latency log.
        #[serde(default)]
        timing: EventTiming,
        /// The captured fact, tagged with the pid Core routes it to.
        fact: ListenerFact,
    },
    /// A menu closed, menu mode ended, or the Alt+Tab switcher closed,
    /// anywhere on the desktop, 50 milliseconds ago. Sent only by the
    /// listener; the supervisor passes it to the app, which reads the
    /// foreground application's focused control (NVDA's fake focus) unless a
    /// focus observed since the end has already been applied.
    MenuOrSwitchEnded {
        /// When the menu or switcher ended, in milliseconds since the Unix
        /// epoch.
        ended_at_ms: u64,
    },
}

impl OutpostToSupervisor {
    /// Whether this message counts toward the message positions that
    /// [`SupervisorToOutpost::NodesHeld`] acknowledges: an event or a
    /// completed query's reply, the messages that carry node ids. The
    /// outpost and Core count the same messages, in pipe order.
    #[must_use]
    pub fn carries_nodes(&self) -> bool {
        matches!(
            self,
            OutpostToSupervisor::Event { .. }
                | OutpostToSupervisor::Reply {
                    outcome: QueryOutcome::Done(_),
                    ..
                }
        )
    }

    /// Stamps `outpost` on every node id this message carries. Core applies
    /// it to everything arriving on an outpost's pipe, so node ids name their
    /// outpost incarnation and never come from the message body.
    pub fn assign_outpost(&mut self, outpost: OutpostId) {
        match self {
            OutpostToSupervisor::Event { event, .. } => event.assign_outpost(outpost),
            OutpostToSupervisor::Reply {
                outcome: QueryOutcome::Done(result),
                ..
            } => match result {
                QueryResult::Focus(focus) => {
                    if let Some((window, _)) = &mut focus.window {
                        window.assign_outpost(outpost);
                    }
                    if let Some(control) = &mut focus.focus {
                        control.node.assign_outpost(outpost);
                        for ancestor in &mut control.ancestors {
                            ancestor.assign_outpost(outpost);
                        }
                        if let Some(selected) = &mut control.selected_child {
                            selected.assign_outpost(outpost);
                        }
                    }
                }
                QueryResult::Navigated(Some(node)) => node.assign_outpost(outpost),
                QueryResult::Ancestors(chain) => {
                    for node in chain {
                        node.assign_outpost(outpost);
                    }
                }
                QueryResult::Tree(dumped) => dumped.root.assign_outpost(outpost),
                _ => {}
            },
            _ => {}
        }
    }
}

/// One completed tree walk from a target application's top-level window
/// (architecture section 1). The walk is bounded by a depth cap of 64 and a
/// node-count cap of 4096; `truncated` notes when it stopped early against
/// either cap rather than reaching every node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpedTree {
    /// The root node reached.
    pub root: TreeNode,
    /// Whether the walk stopped early against the depth or node-count cap.
    pub truncated: bool,
}

/// Writes one message as a single JSON line and flushes.
///
/// # Errors
///
/// Returns any I/O error from the underlying writer; serialization of these
/// message types cannot fail.
pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let line = serde_json::to_string(message).map_err(io::Error::other)?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Reads one JSON-line message. Returns `Ok(None)` on clean end of stream —
/// the peer closed its pipe end.
///
/// # Errors
///
/// Returns an error for I/O failures and for lines that are not valid
/// messages (a protocol violation, not a recoverable condition).
pub fn read_message<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> io::Result<Option<T>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    serde_json::from_str(line.trim_end())
        .map(Some)
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::{NodeDetails, NodeId, NodeSnapshot, Role, State, StateSet};

    #[test]
    fn messages_round_trip_over_a_byte_stream() {
        let event = OutpostToSupervisor::Event {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            observed_at_ms: 1_752_000_000_000,
            backend: Backend::Msaa,
            window: None,
            event: NormalizedEvent::FocusChanged {
                foreground: false,
                ancestors: Vec::new(),
                ancestors_unknown: false,
                selected_child: None,
                node: NodeSnapshot {
                    id: NodeId::new(1),
                    backend: Backend::Msaa,
                    role: Role::MenuItem,
                    name: Some("Settings...".into()),
                    value: None,
                    states: StateSet::new().with(State::Focused),
                    details: NodeDetails::default(),
                },
            },
        };
        let ready = OutpostToSupervisor::Ready {
            outpost_pid: Pid(4242),
            target_pid: Pid(1234),
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &event).expect("writes");
        write_message(&mut buffer, &ready).expect("writes");

        let mut reader = buffer.as_slice();
        let first: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let second: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(first, event);
        assert_eq!(second, ready);
        let end: Option<OutpostToSupervisor> = read_message(&mut reader).expect("reads");
        assert!(end.is_none(), "end of stream reads as None");
    }

    #[test]
    fn stamping_names_every_node_id_in_a_message_with_its_outpost() {
        let snapshot = |number| NodeSnapshot {
            id: NodeId::new(number),
            backend: Backend::Uia,
            role: Role::Button,
            name: None,
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        };
        let mut event = OutpostToSupervisor::Event {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            observed_at_ms: 0,
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::FocusChanged {
                node: snapshot(1),
                foreground: false,
                ancestors: vec![snapshot(2)],
                ancestors_unknown: false,
                selected_child: Some(snapshot(3)),
            },
        };
        let mut chain = OutpostToSupervisor::Reply {
            trace_id: TraceId::mint(),
            request_id: 1,
            outcome: QueryOutcome::Done(QueryResult::Ancestors(vec![snapshot(4)])),
            timing: EventTiming::default(),
        };
        event.assign_outpost(OutpostId(7));
        chain.assign_outpost(OutpostId(7));

        let OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    ancestors,
                    ancestors_unknown: false,
                    selected_child,
                    ..
                },
            ..
        } = event
        else {
            panic!("still a focus event");
        };
        let OutpostToSupervisor::Reply {
            outcome: QueryOutcome::Done(QueryResult::Ancestors(chain)),
            ..
        } = chain
        else {
            panic!("still an ancestor chain");
        };
        for (id, number) in [
            (node.id, 1),
            (ancestors[0].id, 2),
            (selected_child.expect("kept").id, 3),
            (chain[0].id, 4),
        ] {
            assert_eq!(id, NodeId::in_outpost(OutpostId(7), number));
        }
    }

    #[test]
    fn ping_and_pong_round_trip() {
        let ping = SupervisorToOutpost::Ping { seq: 5 };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &ping).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: SupervisorToOutpost = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, ping);

        let expected_pong = OutpostToSupervisor::Pong {
            seq: 5,
            parked_count: 3,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &expected_pong).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, expected_pong);
    }

    #[test]
    fn a_reply_or_timing_from_an_older_peer_reads_with_no_calls() {
        let reply: OutpostToSupervisor =
            serde_json::from_str(r#"{"Reply":{"trace_id":7,"request_id":3,"outcome":"Gone"}}"#)
                .expect("an older reply still reads");
        let OutpostToSupervisor::Reply { timing, .. } = reply else {
            panic!("still a reply");
        };
        assert_eq!(timing, EventTiming::default());
        let timing: EventTiming =
            serde_json::from_str(r#"{"observed_at_us":5}"#).expect("older timing still reads");
        assert!(timing.calls.is_empty());
    }

    #[test]
    fn garbage_is_a_protocol_error() {
        let mut reader: &[u8] = b"not json\n";
        let result: io::Result<Option<SupervisorToOutpost>> = read_message(&mut reader);
        assert!(result.is_err());
    }

    #[test]
    fn focus_fact_and_deliver_fact_round_trip() {
        let snapshot = UiaSnapshotFact {
            runtime_id: vec![42, 7],
            role: Role::MenuItem,
            name: Some("Settings...".into()),
            value: None,
            states: StateSet::new().with(State::Focused),
            details: NodeDetails::default(),
        };
        let fact = ListenerFact {
            pid: Pid(1234),
            fact: DeliveredFact::UiaFocus {
                hwnd: 0,
                focus_window: 0,
                snapshot: snapshot.clone(),
            },
        };
        let focus_fact = OutpostToSupervisor::FocusFact {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            observed_at_ms: 1_752_000_000_000,
            fact: fact.clone(),
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &focus_fact).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, focus_fact);

        // The delivered fact is the same address minus the pid.
        assert_eq!(fact.pid(), Pid(1234));
        assert_eq!(
            fact.into_delivered(),
            DeliveredFact::UiaFocus {
                hwnd: 0,
                focus_window: 0,
                snapshot
            }
        );

        let deliver = SupervisorToOutpost::DeliverFact {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            observed_at_ms: 1_752_000_000_001,
            fact: DeliveredFact::MsaaFocus {
                hwnd: 0x1234,
                id_object: -4,
                id_child: 0,
            },
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &deliver).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: SupervisorToOutpost = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, deliver);
    }

    #[test]
    fn only_a_selection_may_not_start_an_outpost() {
        let snapshot = UiaSnapshotFact {
            runtime_id: vec![1],
            role: Role::ListItem,
            name: None,
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        };
        let selection = DeliveredFact::UiaSelection {
            hwnd: 0,
            snapshot: snapshot.clone(),
        };
        assert!(!selection.may_start_outpost());
        for fact in [
            DeliveredFact::Foreground { hwnd: 1 },
            DeliveredFact::UiaFocus {
                hwnd: 0,
                focus_window: 0,
                snapshot: snapshot.clone(),
            },
            DeliveredFact::UiaMenuOpened {
                hwnd: 0,
                snapshot: snapshot.clone(),
            },
            DeliveredFact::Alert {
                hwnd: 1,
                id_object: -4,
                id_child: 0,
            },
        ] {
            assert!(fact.may_start_outpost(), "{fact:?}");
        }
    }

    #[test]
    fn queries_and_every_outcome_round_trip() {
        let node = NodeSnapshot {
            id: NodeId::new(9),
            backend: Backend::Uia,
            role: Role::Window,
            name: Some("Verbatim".into()),
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        };
        let mut buffer = Vec::new();
        let queries = [
            Query::FocusNow,
            Query::Navigate {
                node_id: NodeId::new(9),
                kind: QueryKind::NextSibling,
            },
            Query::Activate {
                node_id: NodeId::new(9),
            },
            Query::Ancestors {
                node_id: NodeId::new(9),
            },
            Query::DumpTree,
        ];
        let commands: Vec<SupervisorToOutpost> = queries
            .into_iter()
            .map(|query| SupervisorToOutpost::Query {
                trace_id: TraceId::mint(),
                request_id: 3,
                query,
            })
            .chain([SupervisorToOutpost::Cancel { request_id: 3 }])
            .collect();
        for command in &commands {
            write_message(&mut buffer, command).expect("writes");
        }
        let outcomes = [
            QueryOutcome::Done(QueryResult::Focus(FocusNow {
                window: None,
                focus: Some(FocusedControl {
                    node: node.clone(),
                    ancestors: Vec::new(),
                    selected_child: None,
                    window: None,
                }),
                observed_at_ms: 42,
            })),
            QueryOutcome::Done(QueryResult::Navigated(None)),
            QueryOutcome::Done(QueryResult::Tree(DumpedTree {
                root: TreeNode {
                    snapshot: node,
                    children: Vec::new(),
                },
                truncated: true,
            })),
            QueryOutcome::Gone,
            QueryOutcome::Failed("no activation pattern".to_owned()),
            QueryOutcome::NotStarted,
            QueryOutcome::Abandoned,
        ];
        let replies: Vec<OutpostToSupervisor> = outcomes
            .into_iter()
            .map(|outcome| OutpostToSupervisor::Reply {
                trace_id: TraceId::mint(),
                request_id: 3,
                outcome,
                timing: EventTiming {
                    dequeued_at_us: 10,
                    published_at_us: 20,
                    calls: CallCounts {
                        uia: 3,
                        msaa: 0,
                        window_messages: 1,
                    },
                    ..EventTiming::default()
                },
            })
            .collect();
        for reply in &replies {
            write_message(&mut buffer, reply).expect("writes");
        }

        let mut reader = buffer.as_slice();
        for command in &commands {
            let read: SupervisorToOutpost = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            assert_eq!(&read, command);
        }
        for reply in &replies {
            let read: OutpostToSupervisor = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            assert_eq!(&read, reply);
        }
    }
}
