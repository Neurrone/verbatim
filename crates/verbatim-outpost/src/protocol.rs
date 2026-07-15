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
    Backend, FetchResult, NormalizedEvent, Pid, Query, QueryId, SnapshotVersion, TraceId, TreeNode,
};

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
    /// Announces the target application's foreground: the newly
    /// authoritative outpost for this pid (freshly spawned, or one Core
    /// already had running for it) emits a synthetic `FocusChanged` for the
    /// application's top-level foreground window, then the synthetic focus
    /// for its focused control, retrying the control query a bounded number
    /// of times against a control that has not focused itself yet. Sent on
    /// every foreground change to this pid, including the first (spawn
    /// triggers one implicitly by way of the supervisor sending this right
    /// after; see `verbatim-outpost::supervisor`).
    AnnounceFocus {
        /// Trace ID of the foreground-change observation that caused this
        /// announcement, for diagnostics; the emitted events mint their own
        /// trace IDs, since each is its own observably-caused utterance.
        trace_id: TraceId,
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
    /// Asks the outpost to exit cleanly.
    Shutdown,
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
    /// A backend error worth reporting without dying — a failed event
    /// registration, an arbitration probe that keeps timing out.
    Fault {
        /// Human-readable detail for logs and diagnostics.
        detail: String,
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
    use verbatim_model::{NodeId, NodeSnapshot, Role, State, StateSet};

    #[test]
    fn messages_round_trip_over_a_byte_stream() {
        let event = OutpostToSupervisor::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: 1_752_000_000_000,
            backend: Backend::Msaa,
            version: SnapshotVersion(3),
            event: NormalizedEvent::FocusChanged {
                node: NodeSnapshot {
                    id: NodeId::new(1),
                    backend: Backend::Msaa,
                    role: Role::MenuItem,
                    name: Some("Settings...".into()),
                    value: None,
                    states: StateSet::new().with(State::Focused),
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
}
