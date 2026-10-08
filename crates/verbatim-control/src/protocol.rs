//! Control-plane protocol v1: v0 with each latency timeline's stages and
//! cross-process call counts ([`LatencyRecord::stages`]).
//!
//! Newline-delimited compact JSON over the named pipe
//! `\\.\pipe\verbatim-control`. Clients send [`RequestEnvelope`]s; the
//! server answers every request with a [`Frame::Reply`] or [`Frame::Error`]
//! carrying the request's id, and pushes [`Frame::Event`] frames, and
//! [`Frame::Speech`], [`Frame::SpeechStarted`], [`Frame::SpeechEnded`], and
//! [`Frame::Sound`] frames, to connections that subscribed.

use std::io::{self, BufRead, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use verbatim_model::{
    Backend, CallCounts, NormalizedEvent, Pid, TraceId, TreeNode, UtteranceEnding, UtteranceId,
    WindowFacts,
};

/// The protocol version this vocabulary defines. Version 1 added
/// [`LatencyRecord::stages`]; a v0 peer's records read with no stages.
pub const PROTOCOL_VERSION: u32 = 1;

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
    /// Starts streaming [`Frame::Speech`], [`Frame::SpeechStarted`], and
    /// [`Frame::SpeechEnded`] frames on this connection.
    SubscribeSpeech,
    /// Routes a gesture identifier through the gesture router as if the
    /// keys had been pressed. Which identifiers are accepted is up to the
    /// server's handler; Verbatim accepts only bound gestures and answers
    /// any other with an error. A lock key's gesture is refused too: it
    /// reports the key's state after the real key changed it, so a lock key
    /// is sent with [`Request::SendKeys`].
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
        /// Facts about the window the event concerns, when it had one.
        #[serde(default)]
        window: Option<WindowFacts>,
        /// The event itself.
        event: NormalizedEvent,
    },
    /// One utterance queued to be spoken (subscription frame). Every
    /// utterance announced this way is followed, later, by exactly one
    /// [`Frame::SpeechEnded`] (decision D17).
    Speech {
        /// The utterance.
        utterance: UtteranceId,
        /// Trace ID of the event behind it.
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
    },
    /// An utterance's first audio frame has played (subscription frame).
    SpeechStarted {
        /// The utterance.
        utterance: UtteranceId,
        /// Milliseconds since the Unix epoch when it started.
        at_ms: u64,
    },
    /// An utterance has ended (subscription frame): completed when the
    /// device played its last frame, or cancelled, or failed. Carries no
    /// text, so it never competes with [`Frame::Speech`] as a matchable
    /// utterance.
    SpeechEnded {
        /// The utterance.
        utterance: UtteranceId,
        /// How it ended.
        ending: UtteranceEnding,
    },
    /// A sound played at once for an event, outside any utterance
    /// (subscription frame, sent to speech subscribers): the exit sound,
    /// for instance. A sound in the speech stream is named in its
    /// utterance's [`Frame::Speech`] text instead.
    Sound {
        /// The id of the indication the sound reports, such as `exit`; it
        /// reads as `sound: exit`, as a sound in an utterance's text does.
        indication: String,
        /// Milliseconds since the Unix epoch when it started playing.
        at_ms: u64,
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
    /// Whether Verbatim is ready for input: its GUI can act on gestures,
    /// the focus listener is running, the outpost reading Verbatim's own
    /// windows is ready, and Core knows the current focus, if any window
    /// has the foreground. A client that acts right after connecting
    /// waits for this rather than for a fixed time.
    #[serde(default)]
    pub ready: bool,
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
    /// How long each stage the timeline passed took, in pipeline order,
    /// with the cross-process calls made in it for the stages that make them
    /// (a caret key's wait for evidence, and the outpost's read). A stage the timeline did not pass, or has not
    /// reached yet, is left out. Empty from a v0 peer.
    #[serde(default)]
    pub stages: Vec<LatencyStage>,
}

/// One stage of a [`LatencyRecord`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyStage {
    /// Which stage.
    pub kind: LatencyStageKind,
    /// How long it took, in microseconds.
    pub duration_us: u64,
    /// The cross-process calls made in it, by kind, for the stages that make
    /// them (the caret wait and the outpost read); `None` for every other
    /// stage.
    pub calls: Option<CallCounts>,
}

/// The stages of a latency timeline, in pipeline order (architecture
/// section 9). Time spent waiting behind earlier speech is not a stage: it
/// is the queue doing its job, not latency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum LatencyStageKind {
    /// From Windows raising the event to the listener or outpost observing
    /// it; only `WinEvents` carry the time they were raised.
    Windows,
    /// From the keyboard hook seeing a caret key to the reducer having
    /// handled it and asked the outpost for the caret.
    HookToCore,
    /// From the listener observing a focus to its outpost receiving it.
    ListenerToOutpost,
    /// From the reducer asking an outpost (a caret key's watch, a command's
    /// query) to the outpost receiving the request.
    CoreToOutpost,
    /// Waiting in the outpost's queue.
    OutpostQueue,
    /// A caret key's watch for evidence that the key did something, open
    /// while the outpost's worker handled everything else, until the event
    /// that brought the evidence was taken from the queue.
    CaretWait,
    /// The outpost's worker reading the application: the stage that makes
    /// cross-process calls (after the caret wait, for a caret key).
    OutpostRead,
    /// From the outpost publishing to Core receiving it.
    ToCore,
    /// The reducer.
    Reducer,
    /// From the reducer to the speech queue.
    ToSpeech,
    /// From synthesis starting to the synthesizer's first audio.
    Synthesis,
    /// The synthesizer's leading silence, skipped before the mixer.
    LeadingSilence,
    /// From the mixer to the audio device starting.
    MixerAndDevice,
}

impl LatencyStageKind {
    /// The stage's developer-facing name, as the latency log line and
    /// `verbatim-inspect latency` print it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::HookToCore => "hook to Core",
            Self::ListenerToOutpost => "listener to outpost",
            Self::CoreToOutpost => "Core to outpost",
            Self::OutpostQueue => "outpost queue",
            Self::CaretWait => "caret wait",
            Self::OutpostRead => "outpost read",
            Self::ToCore => "to Core",
            Self::Reducer => "reducer",
            Self::ToSpeech => "to speech",
            Self::Synthesis => "synthesis",
            Self::LeadingSilence => "leading silence",
            Self::MixerAndDevice => "mixer and device",
        }
    }

    /// Whether the stage is on the event side, before the speech queue;
    /// the others are on the speech side.
    #[must_use]
    pub fn is_event_side(self) -> bool {
        !matches!(
            self,
            Self::Synthesis | Self::LeadingSilence | Self::MixerAndDevice
        )
    }
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
/// Only for readers whose reads never time out: a read that fails partway
/// through a line discards the part already consumed, so a retry would
/// start in the middle of a message. A reader with a timeout uses
/// [`MessageReader`] instead, which keeps that part.
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

/// Reads JSON-line messages from a connection whose reads may time out,
/// keeping the bytes of a message that is only partly received when a read
/// times out, so the next read continues it rather than starting in its
/// middle. Each complete message is decoded exactly once.
pub struct MessageReader<R> {
    reader: R,
    /// Bytes of the current message received so far, without its newline.
    partial: Vec<u8>,
}

impl<R: BufRead> MessageReader<R> {
    /// Wraps `reader`.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            partial: Vec::new(),
        }
    }

    /// Reads the next message; `Ok(None)` means the peer closed the
    /// connection between messages.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O failures, including a read timeout, after
    /// which calling this again resumes the same message. Also returns an
    /// error if the peer closed the connection partway through a message,
    /// or a line is not a valid message.
    pub fn read<T: DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
        // On an error, `read_until` has already appended every byte it
        // consumed to `partial`, which is what preserves them.
        self.reader.read_until(b'\n', &mut self.partial)?;
        if self.partial.is_empty() {
            return Ok(None);
        }
        if self.partial.last() != Some(&b'\n') {
            self.partial.clear();
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the connection closed partway through a message",
            ));
        }
        let line = std::mem::take(&mut self.partial);
        let text = std::str::from_utf8(&line).map_err(io::Error::other)?;
        serde_json::from_str(text.trim_end())
            .map(Some)
            .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::{NodeDetails, NodeId, NodeSnapshot, Role, StateSet};

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

    /// A reader that hands out scripted chunks, one per read, with a
    /// timeout wherever the script says so, then end of stream.
    struct ScriptedReader(std::collections::VecDeque<Option<Vec<u8>>>);

    impl io::Read for ScriptedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.pop_front() {
                None => Ok(0),
                Some(None) => Err(io::ErrorKind::TimedOut.into()),
                Some(Some(chunk)) => {
                    assert!(chunk.len() <= buf.len(), "test chunks fit one read");
                    buf[..chunk.len()].copy_from_slice(&chunk);
                    Ok(chunk.len())
                }
            }
        }
    }

    fn frame_line(frame: &Frame) -> Vec<u8> {
        let mut line = Vec::new();
        write_message(&mut line, frame).expect("writes");
        line
    }

    #[test]
    fn a_timeout_partway_through_a_message_loses_nothing() {
        let first = Frame::Reply {
            to: 1,
            payload: ReplyPayload::Ok,
        };
        let second = Frame::Error {
            to: 2,
            message: "second".to_owned(),
        };
        let first_line = frame_line(&first);
        let third = first_line.len() / 3;
        let (head, tail) = first_line.split_at(third);
        let (middle, tail) = tail.split_at(third);
        let mut rest = tail.to_vec();
        rest.extend(frame_line(&second));
        let script = [
            Some(head.to_vec()),
            None,
            Some(middle.to_vec()),
            None,
            Some(rest),
        ]
        .into_iter()
        .collect();
        let mut reader = MessageReader::new(io::BufReader::new(ScriptedReader(script)));

        for _ in 0..2 {
            let timed_out = reader.read::<Frame>().expect_err("the read times out");
            assert_eq!(timed_out.kind(), io::ErrorKind::TimedOut);
        }
        assert_eq!(reader.read::<Frame>().expect("reads"), Some(first));
        assert_eq!(reader.read::<Frame>().expect("reads"), Some(second));
        assert_eq!(reader.read::<Frame>().expect("reads"), None);
    }

    #[test]
    fn a_close_partway_through_a_message_is_an_error_not_a_clean_end() {
        let line = frame_line(&Frame::Reply {
            to: 1,
            payload: ReplyPayload::Ok,
        });
        let script = [Some(line[..line.len() - 3].to_vec())]
            .into_iter()
            .collect();
        let mut reader = MessageReader::new(io::BufReader::new(ScriptedReader(script)));

        let error = reader
            .read::<Frame>()
            .expect_err("a cut-off message is an error");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
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
                details: NodeDetails::default(),
            },
            children: vec![TreeNode {
                snapshot: NodeSnapshot {
                    id: NodeId::new(2),
                    backend: Backend::Uia,
                    role: Role::Button,
                    name: Some("test button".to_owned()),
                    value: None,
                    states: StateSet::default(),
                    details: NodeDetails::default(),
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
