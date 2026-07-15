//! Control-plane protocol v0.
//!
//! Newline-delimited compact JSON over the named pipe
//! `\\.\pipe\verbatim-control`. Clients send [`RequestEnvelope`]s; the
//! server answers every request with a [`Frame::Reply`] or [`Frame::Error`]
//! carrying the request's id, and pushes [`Frame::Event`] and
//! [`Frame::Speech`] frames to connections that subscribed.

use std::io::{self, BufRead, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use verbatim_model::{Backend, NormalizedEvent, Pid, SnapshotVersion, TraceId, TreeNode};

/// The protocol version this vocabulary defines.
pub const PROTOCOL_VERSION: u32 = 0;

/// The pipe name clients connect to.
pub const PIPE_NAME: &str = r"\\.\pipe\verbatim-control";

/// One client request with its correlation id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    /// Client-chosen id echoed in the matching reply or error.
    pub id: u64,
    /// The request.
    pub request: Request,
}

/// A client request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Request {
    /// Must be the first request on a connection; agrees on a protocol
    /// version.
    Hello {
        /// The highest protocol version the client speaks.
        protocol_version: u32,
    },
    /// Asks for a status snapshot.
    Status,
    /// Starts streaming [`Frame::Event`] frames on this connection.
    SubscribeEvents,
    /// Starts streaming [`Frame::Speech`] frames on this connection.
    SubscribeSpeech,
    /// Routes a gesture identifier through the gesture router as if the
    /// keys had been pressed. Any well-formed identifier is accepted, bound
    /// or not.
    SendGesture {
        /// Identifier such as `kb:verbatim+v`.
        identifier: String,
    },
    /// Synthesizes real OS keyboard input via `SendInput`, so unbound keys
    /// reach the focused application — `tab`, `shift+tab`, `enter`,
    /// `downarrow`. Uses the same key-name vocabulary as gesture
    /// identifiers. Also the seam remote control reuses later.
    SendKeys {
        /// Key strokes in order, each a plus-joined combination such as
        /// `shift+tab`.
        keys: Vec<String>,
    },
    /// Asks for recent latency timelines.
    Latency {
        /// At most this many timelines, newest first.
        last_n: u32,
    },
    /// Asks for a dump of the target application's accessibility tree from
    /// its top-level window.
    DumpTree,
    /// Asks Verbatim to write its flight recorder's current contents to
    /// disk (architecture section 9): the same snapshot a panic writes
    /// automatically, taken on demand.
    DumpRecorder,
    /// Asks Verbatim to exit cleanly.
    Quit,
}

/// One server-to-client frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Frame {
    /// Successful answer to a request.
    Reply {
        /// The request id this answers.
        to: u64,
        /// The payload.
        payload: ReplyPayload,
    },
    /// Failed answer to a request.
    Error {
        /// The request id this answers.
        to: u64,
        /// Human-readable reason.
        message: String,
    },
    /// One normalized accessibility event (subscription frame).
    Event {
        /// Trace ID of the event.
        trace_id: TraceId,
        /// The application the event came from.
        source: Pid,
        /// Which backend sourced it.
        backend: Backend,
        /// Outpost snapshot version at event time.
        version: SnapshotVersion,
        /// The event itself.
        event: NormalizedEvent,
    },
    /// One spoken utterance (subscription frame), captured where text
    /// enters the synth driver.
    Speech {
        /// Trace ID of the utterance.
        trace_id: TraceId,
        /// The rendered text handed to the synth.
        text: String,
        /// Milliseconds since the Unix epoch when the OS event behind this
        /// utterance was first observed; `None` for Core-originated speech
        /// with no triggering event, such as the startup announcement.
        #[serde(default)]
        event_observed_at_ms: Option<u64>,
        /// Milliseconds since the Unix epoch when the utterance was queued.
        queued_at_ms: u64,
        /// Milliseconds since the Unix epoch when audio started, when it
        /// already has by frame time.
        audio_started_at_ms: Option<u64>,
    },
    /// An utterance's audio has finished playing (subscription frame, same
    /// speech subscription as [`Frame::Speech`]). Emitted once per utterance
    /// that plays to completion — not for one interrupted or dropped before
    /// audio — right after its buffers drain. Lets a paced consumer wait for
    /// speech to be heard in full before acting (see `verbatim-e2e`'s
    /// `SpeechCollector` pacing); carries no text, only the `trace_id`, so it
    /// never competes with [`Frame::Speech`] as a matchable utterance.
    SpeechFinished {
        /// Trace ID of the utterance that finished.
        trace_id: TraceId,
    },
}

/// Payload of a [`Frame::Reply`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReplyPayload {
    /// Answer to [`Request::Hello`]: the version the server will speak.
    Hello {
        /// Agreed protocol version.
        protocol_version: u32,
    },
    /// Plain acknowledgement.
    Ok,
    /// Answer to [`Request::Status`].
    Status(StatusInfo),
    /// Answer to [`Request::Latency`], newest first.
    Latency(Vec<LatencyRecord>),
    /// Answer to [`Request::DumpTree`]: the walked tree, and whether the
    /// depth or node-count cap was hit before the walk covered every node.
    DumpTree {
        /// The root of the walked tree.
        root: TreeNode,
        /// Whether the walk stopped early against the outpost's depth or
        /// node-count cap.
        truncated: bool,
    },
    /// Answer to [`Request::DumpRecorder`]: the path the dump was written
    /// to, in the `dumps` folder next to the executable.
    DumpRecorder {
        /// Absolute path of the written dump file.
        path: String,
    },
}

/// A status snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusInfo {
    /// Core's process id.
    pub pid: Pid,
    /// Verbatim's version string.
    pub version: String,
    /// Active synthesizer id, when speech is up.
    pub active_synth: Option<String>,
    /// One entry per live outpost.
    pub outposts: Vec<OutpostStatus>,
}

/// Status of one outpost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutpostStatus {
    /// The application the outpost watches.
    pub target_pid: Pid,
    /// The outpost's own process id, once running.
    pub outpost_pid: Option<Pid>,
    /// Lifecycle state.
    pub state: OutpostState,
}

/// Lifecycle state of one outpost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OutpostState {
    /// Spawned, not yet ready.
    Starting,
    /// Ready and forwarding events.
    Ready,
    /// Killed or crashed; the supervisor is respawning it.
    Restarting,
}

/// One end-to-end latency timeline (architecture section 9): every
/// timestamp is milliseconds since the Unix epoch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LatencyRecord {
    /// The trace the timeline belongs to.
    pub trace_id: TraceId,
    /// When the OS event or keypress was first observed.
    pub event_observed_at_ms: u64,
    /// When the resulting utterance entered the speech queue.
    pub speech_queued_at_ms: Option<u64>,
    /// When the first audio buffer reached the device.
    pub audio_started_at_ms: Option<u64>,
}

/// Writes one frame or request as a single JSON line and flushes.
///
/// # Errors
///
/// Returns any I/O error from the underlying writer.
pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let line = serde_json::to_string(message).map_err(io::Error::other)?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Reads one JSON-line message; `Ok(None)` means the peer closed the
/// connection.
///
/// # Errors
///
/// Returns an error for I/O failures and for lines that are not valid
/// messages.
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
    use verbatim_model::{NodeId, NodeSnapshot, Role, StateSet};

    #[test]
    fn requests_and_frames_round_trip() {
        let request = RequestEnvelope {
            id: 7,
            request: Request::SendKeys {
                keys: vec!["shift+tab".into(), "enter".into()],
            },
        };
        let frame = Frame::Reply {
            to: 7,
            payload: ReplyPayload::Ok,
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        write_message(&mut buffer, &frame).expect("writes");

        let mut reader = buffer.as_slice();
        let read_request: RequestEnvelope = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_frame: Frame = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_request, request);
        assert_eq!(read_frame, frame);
    }

    #[test]
    fn dump_tree_request_and_reply_round_trip() {
        let request = RequestEnvelope {
            id: 42,
            request: Request::DumpTree,
        };
        let tree_root = TreeNode {
            snapshot: NodeSnapshot {
                id: NodeId::new(1),
                backend: Backend::Uia,
                role: Role::Window,
                name: Some("test window".to_owned()),
                value: None,
                states: StateSet::default(),
            },
            children: vec![TreeNode {
                snapshot: NodeSnapshot {
                    id: NodeId::new(2),
                    backend: Backend::Uia,
                    role: Role::Button,
                    name: Some("test button".to_owned()),
                    value: None,
                    states: StateSet::default(),
                },
                children: vec![],
            }],
        };
        let frame = Frame::Reply {
            to: 42,
            payload: ReplyPayload::DumpTree {
                root: tree_root,
                truncated: false,
            },
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        write_message(&mut buffer, &frame).expect("writes");

        let mut reader = buffer.as_slice();
        let read_request: RequestEnvelope = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_frame: Frame = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_request, request);
        assert_eq!(read_frame, frame);
    }

    #[test]
    fn dump_recorder_request_and_reply_round_trip() {
        let request = RequestEnvelope {
            id: 43,
            request: Request::DumpRecorder,
        };
        let frame = Frame::Reply {
            to: 43,
            payload: ReplyPayload::DumpRecorder {
                path: r"C:\verbatim\dumps\flight-2026-07-14T10-42-32-158Z.jsonl".to_owned(),
            },
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        write_message(&mut buffer, &frame).expect("writes");

        let mut reader = buffer.as_slice();
        let read_request: RequestEnvelope = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_frame: Frame = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_request, request);
        assert_eq!(read_frame, frame);
    }
}
