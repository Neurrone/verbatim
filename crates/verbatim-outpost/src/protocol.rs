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
    Backend, FetchResult, NormalizedEvent, Pid, Query, QueryId, SnapshotVersion, TraceId,
};

/// Messages from the Core-side supervisor to an outpost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SupervisorToOutpost {
    /// Sets or retargets the watched application. The outpost (re)binds its
    /// event hooks, then queries the currently focused element on a
    /// query-pool thread and emits a synthetic focus event, so the focus
    /// change that caused this message is announced without having been
    /// witnessed.
    Configure {
        /// The application to watch.
        target_pid: Pid,
        /// Forces one backend for every window, overriding arbitration;
        /// used by tests and per-app config overrides.
        backend_override: Option<Backend>,
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
    /// A backend error worth reporting without dying — a failed event
    /// registration, an arbitration probe that keeps timing out.
    Fault {
        /// Human-readable detail for logs and diagnostics.
        detail: String,
    },
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
}
