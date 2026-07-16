//! The private Core-outpost protocol (architecture section 1).
//!
//! Transport: two anonymous pipes created by the supervisor with inheritable
//! child ends, their handle values passed on the outpost command line — no
//! named endpoint exists, so there is nothing to discover or secure. Framing
//! is newline-delimited compact JSON: debuggable, flight-recorder friendly,
//! and isolated behind [`write_message`] and [`read_message`] so the
//! encoding can be renegotiated later without touching either end's logic.

use std::io::{self, BufRead, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use verbatim_model::{
    Backend, FetchResult, NodeDetails, NormalizedEvent, Pid, Query, QueryId, Role, SnapshotVersion,
    StateSet, TraceId, TreeNode,
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

/// A focus fact the listener forwards to Core, tagged with the pid Core routes
/// it to (decision D13). The listener reads only what the event itself carries
/// plus hang-safe local reads (the owning pid, a cached window handle), never
/// a cross-process call: a UIA focus fact is built from the element's cached
/// properties, an MSAA fact forwards the raw `WinEvent` address untouched.
///
/// [`DeliveredFact`] is the same address minus the pid, which the supervisor
/// hands to the target's own outpost once routing is done.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ListenerFact {
    /// A UIA focus change: the owning pid, the element's cached native window
    /// handle (0 when the element is not itself a window), and its cached
    /// snapshot parts.
    UiaFocus {
        /// The owning application's pid.
        pid: Pid,
        /// The element's cached native window handle, or 0 when it is not a
        /// window in its own right (a menu item, a list item).
        hwnd: isize,
        /// The element's cached snapshot parts.
        snapshot: UiaSnapshotFact,
    },
    /// An MSAA focus change: the owning pid and the raw `WinEvent` address.
    MsaaFocus {
        /// The owning application's pid.
        pid: Pid,
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
    /// A foreground change: the owning pid and the new foreground window.
    Foreground {
        /// The new foreground window's owning pid.
        pid: Pid,
        /// The new foreground window handle.
        hwnd: isize,
    },
    /// A popup menu opening: the owning pid and the raw `WinEvent` address.
    MenuPopup {
        /// The owning application's pid.
        pid: Pid,
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
}

impl ListenerFact {
    /// The pid the supervisor routes this fact to.
    #[must_use]
    pub fn pid(&self) -> Pid {
        match *self {
            ListenerFact::UiaFocus { pid, .. }
            | ListenerFact::MsaaFocus { pid, .. }
            | ListenerFact::Foreground { pid, .. }
            | ListenerFact::MenuPopup { pid, .. } => pid,
        }
    }

    /// Strips the pid, yielding the [`DeliveredFact`] the supervisor hands to
    /// the target outpost once routing is done.
    #[must_use]
    pub fn into_delivered(self) -> DeliveredFact {
        match self {
            ListenerFact::UiaFocus { hwnd, snapshot, .. } => {
                DeliveredFact::UiaFocus { hwnd, snapshot }
            }
            ListenerFact::MsaaFocus {
                hwnd,
                id_object,
                id_child,
                ..
            } => DeliveredFact::MsaaFocus {
                hwnd,
                id_object,
                id_child,
            },
            ListenerFact::Foreground { hwnd, .. } => DeliveredFact::Foreground { hwnd },
            ListenerFact::MenuPopup {
                hwnd,
                id_object,
                id_child,
                ..
            } => DeliveredFact::MenuPopup {
                hwnd,
                id_object,
                id_child,
            },
        }
    }
}

/// A focus fact the supervisor delivers to a target outpost (decision D13):
/// the same address a [`ListenerFact`] carries, minus the pid, since routing
/// is already done. The outpost turns it back into an announcement on its own
/// deadline-guarded query pool — acquiring, arbitrating, enriching, and
/// emitting exactly as it does for the events it still hooks itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeliveredFact {
    /// A UIA focus change (see [`ListenerFact::UiaFocus`]).
    UiaFocus {
        /// The element's cached native window handle, or 0 when it is not a
        /// window in its own right.
        hwnd: isize,
        /// The element's cached snapshot parts.
        snapshot: UiaSnapshotFact,
    },
    /// An MSAA focus change (see [`ListenerFact::MsaaFocus`]).
    MsaaFocus {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
    /// A foreground change (see [`ListenerFact::Foreground`]).
    Foreground {
        /// The new foreground window handle.
        hwnd: isize,
    },
    /// A popup menu opening (see [`ListenerFact::MenuPopup`]).
    MenuPopup {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
}

/// Messages from the Core-side supervisor to an outpost.
///
/// An outpost's target application is fixed at spawn (decision D9: one
/// outpost per application, for its whole life) and passed on its command
/// line, not by any message here — there is no cross-pid retarget in this
/// protocol. What remains after that split are three genuinely independent
/// concerns the old M1 `Configure` conflated: backend-override
/// configuration ([`SetBackendOverride`](Self::SetBackendOverride)),
/// announcing a foreground change ([`AnnounceFocus`](Self::AnnounceFocus)),
/// and everything else this outpost already answered on its own.
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
    /// Announces the target application's foreground by polling: the outpost
    /// emits a synthetic `FocusChanged` for the application's top-level
    /// foreground window, then the synthetic focus for its focused control,
    /// retrying a bounded number of times against a control or window that
    /// has not readied itself yet.
    ///
    /// Under decision D13 this is a fallback, no longer the mechanism of
    /// record. The focus listener detects focus from the OS event directly
    /// and Core delivers it as a [`DeliverFact`](Self::DeliverFact); the poll
    /// remains only for the supervisor's own startup target and to re-announce
    /// the current foreground across a listener respawn gap. It is still sent
    /// when Core spawns or re-targets an outpost for a foreground application
    /// (see `verbatim-outpost::supervisor`).
    AnnounceFocus {
        /// Trace ID of the foreground-change observation that caused this
        /// announcement, for diagnostics; the emitted events mint their own
        /// trace IDs, since each is its own observably-caused utterance.
        trace_id: TraceId,
    },
    /// Delivers a focus fact the focus listener captured, for this outpost's
    /// target application (decision D13). The outpost acquires, arbitrates,
    /// enriches, and announces it on its query pool exactly as it does for
    /// the events it hooks itself, but threading the listener's `trace_id`
    /// and `observed_at_ms` through to the emitted event so the latency
    /// timeline starts at the real OS event rather than this delivery.
    DeliverFact {
        /// Trace ID the listener minted when it observed the OS event.
        trace_id: TraceId,
        /// Milliseconds since the Unix epoch when the listener observed the
        /// OS event — the first point on the keypress-to-audio timeline.
        observed_at_ms: u64,
        /// The routed fact, minus the pid (routing is done).
        fact: DeliveredFact,
    },
    /// Asks for more data about a node; answered by
    /// [`OutpostToSupervisor::FetchReply`].
    Fetch {
        /// Trace ID of the reducer input that caused this fetch.
        trace_id: TraceId,
        /// What to read.
        query: Query,
    },
    /// Liveness probe; answered by [`OutpostToSupervisor::Pong`].
    Ping {
        /// Echoed in the matching pong.
        seq: u64,
    },
    /// Asks the outpost to walk the target application's tree from its
    /// top-level window and return it; answered by
    /// [`OutpostToSupervisor::DumpTreeReply`]. Runs on a query-pool thread
    /// with a deadline, so a hung application abandons the call rather than
    /// wedging the outpost.
    DumpTree {
        /// Trace ID of the request that caused this dump.
        trace_id: TraceId,
    },
    /// Asks for the chain of ancestors of a node, outermost first, as
    /// [`NodeSnapshot`]s; answered by
    /// [`OutpostToSupervisor::AncestorChainReply`]. Runs on a query-pool
    /// thread with a deadline (the same pattern as
    /// [`DumpTree`](Self::DumpTree)), so a hung application abandons the
    /// call rather than wedging the outpost. Capped at 64 hops.
    AncestorChain {
        /// Trace ID of the request that caused this walk.
        trace_id: TraceId,
        /// The node whose ancestors are wanted.
        node_id: verbatim_model::NodeId,
    },
    /// Asks for a node found by navigating from `node_id`; answered by
    /// [`OutpostToSupervisor::NavigateReply`]. Runs on a query-pool thread
    /// with a deadline, the same pattern as [`DumpTree`](Self::DumpTree).
    Navigate {
        /// Trace ID of the request that caused this navigation.
        trace_id: TraceId,
        /// The node to navigate from.
        node_id: verbatim_model::NodeId,
        /// Which direction to navigate.
        direction: NavigateDirection,
    },
    /// Asks the outpost to activate a node (UIA `Invoke`/`Toggle`/legacy
    /// `DoDefaultAction`; MSAA `accDoDefaultAction`); answered by
    /// [`OutpostToSupervisor::ActivateReply`]. Runs on a query-pool thread
    /// with a deadline, the same pattern as [`DumpTree`](Self::DumpTree).
    Activate {
        /// Trace ID of the request that caused this activation.
        trace_id: TraceId,
        /// The node to activate.
        node_id: verbatim_model::NodeId,
    },
    /// Asks the outpost to exit cleanly.
    Shutdown,
}

/// A direction to navigate from a node, for [`SupervisorToOutpost::Navigate`]
/// (roadmap M3's object-navigation bullet: parent, next and previous
/// sibling, and first child).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigateDirection {
    /// The node's parent.
    Parent,
    /// The next sibling in tree order.
    NextSibling,
    /// The previous sibling in tree order.
    PreviousSibling,
    /// The first child.
    FirstChild,
}

/// The answer to a [`SupervisorToOutpost::Navigate`] or an
/// [`SupervisorToOutpost::AncestorChain`] hop's single-node counterpart: a
/// found node is distinguished from "no such neighbor" as a first-class
/// outcome, never conflated with an error (a genuinely absent parent of a
/// root node, or a list's last item asked for its next sibling, are not
/// failures).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigateOutcome {
    /// A node was found in that direction.
    Found(verbatim_model::NodeSnapshot),
    /// There is no neighbor in that direction (not an error).
    NoNeighbor,
}

/// Messages from an outpost to the Core-side supervisor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OutpostToSupervisor {
    /// First message after startup or reconfiguration.
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
        /// observed — the first point on the keypress-to-audio latency
        /// timeline. Defaults to 0 for messages from older peers.
        #[serde(default)]
        observed_at_ms: u64,
        /// Which backend sourced the event.
        backend: Backend,
        /// The outpost's tree snapshot version at event time.
        version: SnapshotVersion,
        /// The event itself.
        event: NormalizedEvent,
    },
    /// Answer to [`SupervisorToOutpost::Fetch`].
    FetchReply {
        /// Trace ID carried through from the fetch.
        trace_id: TraceId,
        /// The request this answers.
        query_id: QueryId,
        /// What was found.
        result: FetchResult,
    },
    /// Answer to [`SupervisorToOutpost::Ping`].
    Pong {
        /// The probed sequence number.
        seq: u64,
        /// This outpost's current count of query-pool workers parked on
        /// abandoned calls (recovery ladder rung 2's bounded garbage); the
        /// supervisor watches this, alongside missed pongs, to detect a
        /// wedged-but-alive outpost (recovery ladder rung 3).
        parked_count: usize,
    },
    /// Answer to [`SupervisorToOutpost::DumpTree`].
    DumpTreeReply {
        /// Trace ID carried through from the request.
        trace_id: TraceId,
        /// `Ok` with the walked tree, or `Err` with a human-readable reason
        /// the walk could not complete (no accessible top-level window, or
        /// the query-pool deadline expired against a hung application).
        result: Result<DumpedTree, String>,
    },
    /// Answer to [`SupervisorToOutpost::AncestorChain`].
    AncestorChainReply {
        /// Trace ID carried through from the request.
        trace_id: TraceId,
        /// `Ok` with the ancestor chain (outermost first, possibly empty for
        /// a root node), or `Err` with a human-readable reason the walk
        /// could not complete.
        result: Result<Vec<verbatim_model::NodeSnapshot>, String>,
    },
    /// Answer to [`SupervisorToOutpost::Navigate`].
    NavigateReply {
        /// Trace ID carried through from the request.
        trace_id: TraceId,
        /// `Ok` with the navigation outcome (a found node, or a first-class
        /// "no such neighbor"), or `Err` with a human-readable reason the
        /// navigation could not complete.
        result: Result<NavigateOutcome, String>,
    },
    /// Answer to [`SupervisorToOutpost::Activate`].
    ActivateReply {
        /// Trace ID carried through from the request.
        trace_id: TraceId,
        /// `Ok(())` if the activation was invoked, or `Err` with a
        /// human-readable reason it could not be (the node has no
        /// activation action, or the call failed).
        result: Result<(), String>,
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
        /// The captured fact, tagged with the pid Core routes it to.
        fact: ListenerFact,
    },
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
            observed_at_ms: 1_752_000_000_000,
            backend: Backend::Msaa,
            version: SnapshotVersion(3),
            event: NormalizedEvent::FocusChanged {
                ancestors: Vec::new(),
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
    fn garbage_is_a_protocol_error() {
        let mut reader: &[u8] = b"not json\n";
        let result: io::Result<Option<SupervisorToOutpost>> = read_message(&mut reader);
        assert!(result.is_err());
    }

    #[test]
    fn dump_tree_request_and_reply_round_trip() {
        let request = SupervisorToOutpost::DumpTree {
            trace_id: TraceId::mint(),
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: SupervisorToOutpost = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, request);

        let success = OutpostToSupervisor::DumpTreeReply {
            trace_id: TraceId::mint(),
            result: Ok(DumpedTree {
                root: TreeNode {
                    snapshot: NodeSnapshot {
                        id: NodeId::new(1),
                        backend: Backend::Uia,
                        role: Role::Window,
                        name: Some("Verbatim".into()),
                        value: None,
                        states: StateSet::new(),
                        details: NodeDetails::default(),
                    },
                    children: Vec::new(),
                },
                truncated: true,
            }),
        };
        let failure = OutpostToSupervisor::DumpTreeReply {
            trace_id: TraceId::mint(),
            result: Err("no accessible top-level window".to_owned()),
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &success).expect("writes");
        write_message(&mut buffer, &failure).expect("writes");
        let mut reader = buffer.as_slice();
        let read_success: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_failure: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_success, success);
        assert_eq!(read_failure, failure);
    }

    #[test]
    fn ancestor_chain_request_and_reply_round_trip() {
        let request = SupervisorToOutpost::AncestorChain {
            trace_id: TraceId::mint(),
            node_id: NodeId::new(7),
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: SupervisorToOutpost = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, request);

        let ancestor = NodeSnapshot {
            id: NodeId::new(1),
            backend: Backend::Uia,
            role: Role::Window,
            name: Some("Verbatim".into()),
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        };
        let success = OutpostToSupervisor::AncestorChainReply {
            trace_id: TraceId::mint(),
            result: Ok(vec![ancestor]),
        };
        let failure = OutpostToSupervisor::AncestorChainReply {
            trace_id: TraceId::mint(),
            result: Err("the node no longer exists".to_owned()),
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &success).expect("writes");
        write_message(&mut buffer, &failure).expect("writes");
        let mut reader = buffer.as_slice();
        let read_success: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_failure: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_success, success);
        assert_eq!(read_failure, failure);
    }

    #[test]
    fn navigate_request_and_reply_round_trip() {
        let request = SupervisorToOutpost::Navigate {
            trace_id: TraceId::mint(),
            node_id: NodeId::new(3),
            direction: NavigateDirection::NextSibling,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: SupervisorToOutpost = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, request);

        let found = OutpostToSupervisor::NavigateReply {
            trace_id: TraceId::mint(),
            result: Ok(NavigateOutcome::Found(NodeSnapshot {
                id: NodeId::new(4),
                backend: Backend::Msaa,
                role: Role::Button,
                name: Some("OK".into()),
                value: None,
                states: StateSet::new(),
                details: NodeDetails::default(),
            })),
        };
        let no_neighbor = OutpostToSupervisor::NavigateReply {
            trace_id: TraceId::mint(),
            result: Ok(NavigateOutcome::NoNeighbor),
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &found).expect("writes");
        write_message(&mut buffer, &no_neighbor).expect("writes");
        let mut reader = buffer.as_slice();
        let read_found: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_no_neighbor: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_found, found);
        assert_eq!(read_no_neighbor, no_neighbor);
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
        let fact = ListenerFact::UiaFocus {
            pid: Pid(1234),
            hwnd: 0,
            snapshot: snapshot.clone(),
        };
        let focus_fact = OutpostToSupervisor::FocusFact {
            trace_id: TraceId::mint(),
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
            DeliveredFact::UiaFocus { hwnd: 0, snapshot }
        );

        let deliver = SupervisorToOutpost::DeliverFact {
            trace_id: TraceId::mint(),
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
    fn every_listener_fact_variant_routes_and_strips_its_pid() {
        let variants = [
            (
                ListenerFact::MsaaFocus {
                    pid: Pid(1),
                    hwnd: 10,
                    id_object: -4,
                    id_child: 0,
                },
                DeliveredFact::MsaaFocus {
                    hwnd: 10,
                    id_object: -4,
                    id_child: 0,
                },
            ),
            (
                ListenerFact::Foreground {
                    pid: Pid(2),
                    hwnd: 20,
                },
                DeliveredFact::Foreground { hwnd: 20 },
            ),
            (
                ListenerFact::MenuPopup {
                    pid: Pid(3),
                    hwnd: 30,
                    id_object: -3,
                    id_child: 0,
                },
                DeliveredFact::MenuPopup {
                    hwnd: 30,
                    id_object: -3,
                    id_child: 0,
                },
            ),
        ];
        for (index, (fact, expected)) in variants.into_iter().enumerate() {
            let pid = Pid(u32::try_from(index).unwrap() + 1);
            assert_eq!(fact.pid(), pid);
            assert_eq!(fact.into_delivered(), expected);
        }
    }

    #[test]
    fn activate_request_and_reply_round_trip() {
        let request = SupervisorToOutpost::Activate {
            trace_id: TraceId::mint(),
            node_id: NodeId::new(9),
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: SupervisorToOutpost = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, request);

        let success = OutpostToSupervisor::ActivateReply {
            trace_id: TraceId::mint(),
            result: Ok(()),
        };
        let failure = OutpostToSupervisor::ActivateReply {
            trace_id: TraceId::mint(),
            result: Err("element exposes no activation pattern".to_owned()),
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &success).expect("writes");
        write_message(&mut buffer, &failure).expect("writes");
        let mut reader = buffer.as_slice();
        let read_success: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_failure: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_success, success);
        assert_eq!(read_failure, failure);
    }
}
