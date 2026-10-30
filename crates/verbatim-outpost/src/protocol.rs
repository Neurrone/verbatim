//! The private Core-outpost protocol (architecture section 1).
//!
//! Transport: two pipes with no name, made by the supervisor, whose child
//! ends the child inherits, their handle values passed on its command line,
//! so there is nothing to discover or secure. Each message is one frame of
//! `verbatim_process::transport`: a `Content-Length` header, then the
//! message as compact JSON, readable by eye and in captures, and, for a
//! message carrying a terminal's text or colours, a raw section holding
//! them, the JSON giving their places (the `raw` module); all of it
//! isolated behind [`Message`], [`write_message`], [`encode_message`] and
//! [`read_message`] so neither end's logic sees the framing.

#![forbid(unsafe_code)]

use std::io::{self, BufRead, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;
use verbatim_model::protocol::{
    CoreToListener, CoreToOutpost, DeliveredFact, ListenerToCore, OutpostToCore,
};
use verbatim_process::transport::Parts;

mod raw;

/// Microseconds since the Unix epoch on the run's shared clock
/// ([`verbatim_process::clock`]), which [`EventTiming`], the latency log,
/// and every time compared across processes share.
#[must_use]
pub fn now_us() -> u64 {
    verbatim_process::clock::now_us()
}

/// The fact an MSAA `WinEvent` of `kind` at the address `(hwnd, id_object,
/// id_child)` makes, whether the listener's hook or an outpost's own took it
/// in: a focus, a foreground change, a menu opening, an alert, or a tooltip
/// shown. `None` for any other kind, and for an event with no window, which
/// neither routes nor reads.
#[must_use]
pub fn fact_of_win_event(
    kind: verbatim_ia2::WinEventKind,
    (hwnd, id_object, id_child): (isize, i32, i32),
) -> Option<DeliveredFact> {
    use verbatim_ia2::WinEventKind;
    if hwnd == 0 {
        return None;
    }
    Some(match kind {
        WinEventKind::Focus => DeliveredFact::MsaaFocus {
            hwnd,
            id_object,
            id_child,
        },
        // Taken unchecked: whether the window is still the foreground
        // window is checked when the fact is handled, as NVDA checks it
        // when it processes the event rather than in its event callback.
        WinEventKind::Foreground => DeliveredFact::Foreground { hwnd },
        WinEventKind::MenuPopupStart => DeliveredFact::MenuPopup {
            hwnd,
            id_object,
            id_child,
        },
        WinEventKind::Alert => DeliveredFact::Alert {
            hwnd,
            id_object,
            id_child,
        },
        WinEventKind::Show => DeliveredFact::Show {
            hwnd,
            id_object,
            id_child,
        },
        _ => return None,
    })
}

/// The most bytes one message may take, its frame's header aside, in either
/// direction between Core and a child (`docs/design/communication.md`, section
/// 4.2; Dickson, 2026-10-07): room for 10 MB
/// of terminal output and JSON's escaping. A writer never sends more, and a
/// reader that is told of more treats the other end as failed rather than
/// reading on, so an enormous message cannot make it allocate without
/// bound.
pub const MAX_MESSAGE_BYTES: usize = verbatim_process::transport::MAX_FRAME_BYTES;

/// A message of the Core-outpost protocol as a frame carries it: its JSON
/// body, and for a message that carries a terminal's text and colours, a
/// raw section holding them, the body giving their places
/// (`docs/design/communication.md`, section 4.2). Every other message is a
/// body alone, and is refused with a raw section.
pub trait Message: Sized {
    /// Appends this message's frame to `buffer`.
    ///
    /// # Errors
    ///
    /// Returns the encoding's error, and [`io::ErrorKind::InvalidInput`]
    /// for a message larger than [`MAX_MESSAGE_BYTES`].
    fn put_frame(&self, buffer: &mut Vec<u8>) -> io::Result<()>;

    /// Reads a message from its frame's parts.
    ///
    /// # Errors
    ///
    /// Returns an error for parts that are not one.
    fn from_parts(parts: Parts<'_>) -> io::Result<Self>;
}

/// Appends `message`'s frame, a JSON body alone, to `buffer`.
fn put_json<T: Serialize>(buffer: &mut Vec<u8>, message: &T) -> io::Result<()> {
    verbatim_process::transport::put_frame(buffer, |body| {
        serde_json::to_writer(body, message).map_err(io::Error::other)
    })
}

/// Reads a message that is a JSON body alone.
fn from_json<T: DeserializeOwned>(parts: Parts<'_>) -> io::Result<T> {
    serde_json::from_slice(parts.without_raw()?).map_err(io::Error::other)
}

impl Message for CoreToOutpost {
    fn put_frame(&self, buffer: &mut Vec<u8>) -> io::Result<()> {
        put_json(buffer, self)
    }

    fn from_parts(parts: Parts<'_>) -> io::Result<Self> {
        from_json(parts)
    }
}

impl Message for CoreToListener {
    fn put_frame(&self, buffer: &mut Vec<u8>) -> io::Result<()> {
        put_json(buffer, self)
    }

    fn from_parts(parts: Parts<'_>) -> io::Result<Self> {
        from_json(parts)
    }
}

impl Message for ListenerToCore {
    fn put_frame(&self, buffer: &mut Vec<u8>) -> io::Result<()> {
        put_json(buffer, self)
    }

    fn from_parts(parts: Parts<'_>) -> io::Result<Self> {
        from_json(parts)
    }
}

impl Message for OutpostToCore {
    fn put_frame(&self, buffer: &mut Vec<u8>) -> io::Result<()> {
        match raw::Terminal::of(self) {
            Some(terminal) => terminal.put_frame(buffer),
            None => put_json(buffer, self),
        }
    }

    fn from_parts(parts: Parts<'_>) -> io::Result<Self> {
        match parts.raw {
            Some(section) => raw::decode(parts.body, section),
            None => serde_json::from_slice(parts.body).map_err(io::Error::other),
        }
    }
}

/// A string, as the framing's tests send one.
#[cfg(test)]
impl Message for String {
    fn put_frame(&self, buffer: &mut Vec<u8>) -> io::Result<()> {
        put_json(buffer, self)
    }

    fn from_parts(parts: Parts<'_>) -> io::Result<Self> {
        from_json(parts)
    }
}

/// Writes one message as one frame and flushes.
///
/// The frame goes to `writer` in one `write_all`, so on a bare pipe, as the
/// listener and every outpost write to, a message is one `WriteFile`
/// (`docs/design/communication.md`, section 4.3).
///
/// # Errors
///
/// Returns any I/O error from the underlying writer, and
/// [`io::ErrorKind::InvalidInput`] for a message larger than
/// [`MAX_MESSAGE_BYTES`], which is not sent; serialization of these
/// message types cannot fail.
pub fn write_message<W: Write, T: Message>(writer: &mut W, message: &T) -> io::Result<()> {
    let frame = encode_message(message)?;
    writer.write_all(&frame)?;
    writer.flush()
}

/// One message as the frame [`write_message`] writes.
///
/// # Errors
///
/// As [`write_message`], without the I/O.
pub fn encode_message<T: Message>(message: &T) -> io::Result<Vec<u8>> {
    let mut frame = Vec::new();
    put_message(&mut frame, message)?;
    Ok(frame)
}

/// Appends one message's frame to `buffer`, so a sender encodes straight
/// into the buffer it writes from.
///
/// # Errors
///
/// As [`write_message`], without the I/O.
pub fn put_message<T: Message>(buffer: &mut Vec<u8>, message: &T) -> io::Result<()> {
    message.put_frame(buffer)
}

/// Reads one message's frame. Returns `Ok(None)` on clean end of stream:
/// the peer closed its pipe end between messages.
///
/// # Errors
///
/// Returns an error for I/O failures, for the stream ending inside a
/// frame, for a frame larger than [`MAX_MESSAGE_BYTES`]
/// ([`io::ErrorKind::InvalidData`], read no further than its header), and
/// for a frame that is not a valid message (a protocol violation, not a
/// recoverable condition).
pub fn read_message<R: BufRead, T: Message>(reader: &mut R) -> io::Result<Option<T>> {
    let Some(frame) = verbatim_process::transport::read_frame(reader)? else {
        return Ok(None);
    };
    decode_message(frame.parts()).map(Some)
}

/// The kind of message a frame's body holds, read from the body's first
/// bytes without parsing the rest: the name of its variant, which serde's
/// external tagging writes first, as `{"Pong":...}` for a variant with
/// fields and `"TargetExited"` for one without. `None` for a body that
/// starts neither way. verbatim.exe's relay reads it on every frame from
/// an outpost, to handle pongs, `Ready` and `TargetExited` itself and pass
/// every other frame on whole (`docs/design/communication.md`, section
/// 6.2).
#[must_use]
pub fn message_kind(body: &[u8]) -> Option<&str> {
    let name = body
        .strip_prefix(b"{\"")
        .or_else(|| body.strip_prefix(b"\""))?;
    let end = name.iter().position(|&byte| byte == b'"')?;
    std::str::from_utf8(&name[..end]).ok()
}

/// What verbatim.exe's relay does with a frame from an outpost, read from
/// its kind alone ([`message_kind`]): the variants of [`OutpostToCore`] it
/// handles itself, by name, and every other frame, which it hands on whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Peeked {
    /// [`OutpostToCore::Pong`].
    Pong,
    /// [`OutpostToCore::TargetExited`].
    TargetExited,
    /// [`OutpostToCore::Ready`].
    Ready,
    /// [`OutpostToCore::SpareReady`].
    SpareReady,
    /// [`OutpostToCore::Fault`].
    Fault,
    /// Anything else, or a body that names no variant.
    Other,
}

/// What the relay does with a frame from an outpost whose body is `body`.
#[must_use]
pub fn peek(body: &[u8]) -> Peeked {
    match message_kind(body) {
        Some("Pong") => Peeked::Pong,
        Some("TargetExited") => Peeked::TargetExited,
        Some("Ready") => Peeked::Ready,
        Some("SpareReady") => Peeked::SpareReady,
        Some("Fault") => Peeked::Fault,
        _ => Peeked::Other,
    }
}

/// Parses one frame's parts as a message.
///
/// # Errors
///
/// Returns an error for parts that are not a valid message.
pub fn decode_message<T: Message>(parts: Parts<'_>) -> io::Result<T> {
    T::from_parts(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::protocol::{
        DumpedTree, EventTiming, FocusNow, FocusedControl, GapAnswer, ListenerFact, Query,
        QueryOutcome, QueryResult, UiaAncestorFact, UiaFocusRead, UiaSnapshotFact, Urgent,
        WinEventStamp,
    };
    use verbatim_model::{
        Backend, CallCounts, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Pid,
        QueryKind, Role, State, StateSet, TraceId, TreeNode,
    };

    /// What the relay does with `message`'s frame.
    fn peeked(message: &OutpostToCore) -> Peeked {
        let frame = encode_message(message).expect("encoded");
        let frame = verbatim_process::transport::read_frame(&mut frame.as_slice())
            .expect("a frame")
            .expect("not the end");
        peek(frame.parts().body)
    }

    /// The relay handles by name exactly the frames `outpost_frame` and
    /// `spare_frame` match on, and hands every other frame on. Every
    /// message it consumes rather than hands on, and every one an outpost
    /// sends as urgent, carries no node, so a frame verbatim.exe does not
    /// number never carries nodes, and none the outpost does not number
    /// does either.
    #[test]
    fn the_relay_peeks_each_variant_by_name_and_consumes_none_that_carries_nodes() {
        // What the relay is to do with each variant, by a match with no
        // wildcard over every sample, so a new variant is given its place.
        let expected = |message: &OutpostToCore| match message {
            OutpostToCore::Ready { .. } => Peeked::Ready,
            OutpostToCore::SpareReady { .. } => Peeked::SpareReady,
            OutpostToCore::Pong { .. } => Peeked::Pong,
            OutpostToCore::Fault { .. } => Peeked::Fault,
            OutpostToCore::TargetExited => Peeked::TargetExited,
            OutpostToCore::Event { .. }
            | OutpostToCore::Reply { .. }
            | OutpostToCore::IdleAnswer { .. } => Peeked::Other,
        };
        let mut samples = verbatim_model::protocol::samples::outpost_to_core();
        // A completed reply too, which carries nodes.
        samples.push(OutpostToCore::Reply {
            trace_id: TraceId::mint(),
            request_id: 1,
            outcome: QueryOutcome::Done(QueryResult::Navigated(None)),
            timing: EventTiming::default(),
        });
        let cases: Vec<(OutpostToCore, Peeked)> = samples
            .into_iter()
            .map(|message| {
                let peek = expected(&message);
                (message, peek)
            })
            .collect();
        assert_eq!(cases.len(), 9);
        for (message, expected) in &cases {
            assert_eq!(peeked(message), *expected, "{message:?}");
        }
        let consumed = cases
            .iter()
            .filter(|(_, peek)| {
                matches!(
                    peek,
                    Peeked::Pong | Peeked::TargetExited | Peeked::SpareReady
                )
            })
            .map(|(message, _)| message.clone());
        let urgent = [
            Urgent::Pong {
                seq: 1,
                parked_count: 0,
            },
            Urgent::Ready {
                outpost_pid: Pid(1),
                target_pid: Pid(2),
            },
            Urgent::TargetExited,
            Urgent::SpareReady {
                outpost_pid: Pid(1),
            },
        ]
        .into_iter()
        .map(OutpostToCore::from);
        let carrying: Vec<OutpostToCore> = consumed
            .chain(urgent)
            .filter(OutpostToCore::carries_nodes)
            .collect();
        assert_eq!(carrying, Vec::<OutpostToCore>::new());
    }

    /// The kind read from a body's first bytes is the variant's name, for
    /// a variant with fields and for one without.
    #[test]
    fn a_frames_kind_is_its_variants_name() {
        let kind_of = |message: &OutpostToCore| {
            let frame = encode_message(message).expect("encoded");
            let body = verbatim_process::transport::read_frame(&mut frame.as_slice())
                .expect("a frame")
                .expect("not the end");
            message_kind(body.parts().body).map(str::to_owned)
        };
        let pong = OutpostToCore::Pong {
            seq: 1,
            parked_count: 0,
            written_at_us: 0,
        };
        assert_eq!(kind_of(&pong).as_deref(), Some("Pong"));
        assert_eq!(
            kind_of(&OutpostToCore::TargetExited).as_deref(),
            Some("TargetExited")
        );
        assert_eq!(
            kind_of(&OutpostToCore::Ready {
                outpost_pid: Pid(1),
                target_pid: Pid(2),
            })
            .as_deref(),
            Some("Ready")
        );
        assert_eq!(message_kind(b"[]"), None);
    }

    /// Each urgent message is the [`OutpostToCore`] variant of its name.
    #[test]
    fn each_urgent_message_converts_to_the_variant_of_its_name() {
        let pong = OutpostToCore::Pong {
            seq: 1,
            parked_count: 0,
            written_at_us: 0,
        };
        let ready = OutpostToCore::Ready {
            outpost_pid: Pid(1),
            target_pid: Pid(2),
        };
        assert_eq!(
            OutpostToCore::from(Urgent::TargetExited),
            OutpostToCore::TargetExited
        );
        assert_eq!(
            OutpostToCore::from(Urgent::SpareReady {
                outpost_pid: Pid(1)
            }),
            OutpostToCore::SpareReady {
                outpost_pid: Pid(1)
            }
        );
        assert_eq!(
            OutpostToCore::from(Urgent::Pong {
                seq: 1,
                parked_count: 0
            }),
            pong
        );
        assert_eq!(
            OutpostToCore::from(Urgent::Ready {
                outpost_pid: Pid(1),
                target_pid: Pid(2)
            }),
            ready
        );
    }

    #[test]
    fn a_message_larger_than_the_limit_is_neither_sent_nor_read() {
        let mut written = Vec::new();
        let large = "x".repeat(MAX_MESSAGE_BYTES);
        let error = write_message(&mut written, &large).expect_err("too large to send");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(written, Vec::<u8>::new());
        // A peer that sends one anyway: the reader stops at its header.
        let header = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 2);
        let mut stream = std::io::Cursor::new(format!("{header}\"{large}\"").into_bytes());
        let error = read_message::<_, String>(&mut stream).expect_err("too large to read");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            usize::try_from(stream.position()).expect("a position"),
            header.len()
        );
        // One at the limit is read.
        let fits = "y".repeat(MAX_MESSAGE_BYTES - 2);
        let mut written = Vec::new();
        write_message(&mut written, &fits).expect("fits");
        let read: Option<String> = read_message(&mut std::io::Cursor::new(written)).expect("reads");
        assert_eq!(read.as_deref(), Some(fits.as_str()));
    }

    #[test]
    fn a_message_and_its_header_are_one_write() {
        /// Records each write it is given.
        #[derive(Default)]
        struct Writes(Vec<Vec<u8>>);
        impl Write for Writes {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.push(bytes.to_vec());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut writes = Writes::default();
        write_message(&mut writes, &"one".to_owned()).expect("written");
        assert_eq!(
            writes.0,
            vec![b"Content-Length: 0000000005\r\n\r\n\"one\"".to_vec()]
        );
    }

    #[test]
    fn messages_round_trip_over_a_byte_stream() {
        let event = OutpostToCore::Event {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            observed_at_us: 1_752_000_000_000_000,
            backend: Backend::Msaa,
            window: None,
            application: Some(Pid(20)),
            event: NormalizedEvent::FocusChanged {
                foreground: false,
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
                text: None,
                earlier: None,
                group: None,
            },
        };
        let ready = OutpostToCore::Ready {
            outpost_pid: Pid(4242),
            target_pid: Pid(1234),
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &event).expect("writes");
        write_message(&mut buffer, &ready).expect("writes");

        let mut reader = buffer.as_slice();
        let first: OutpostToCore = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let second: OutpostToCore = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(first, event);
        assert_eq!(second, ready);
        let end: Option<OutpostToCore> = read_message(&mut reader).expect("reads");
        assert!(end.is_none(), "end of stream reads as None");
    }

    #[test]
    fn ping_and_pong_round_trip() {
        let ping = CoreToOutpost::Ping {
            seq: 5,
            sent_at_us: 4_000,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &ping).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: CoreToOutpost = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, ping);

        let expected_pong = OutpostToCore::Pong {
            seq: 5,
            parked_count: 3,
            written_at_us: 4_500,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &expected_pong).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: OutpostToCore = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, expected_pong);
    }

    #[test]
    fn garbage_is_a_protocol_error() {
        let mut reader: &[u8] = b"not json\n";
        let result: io::Result<Option<CoreToOutpost>> = read_message(&mut reader);
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
        let read = UiaFocusRead {
            hwnd: 0,
            snapshot: snapshot.clone(),
            ancestors: vec![UiaAncestorFact {
                snapshot: snapshot.clone(),
                hwnd: 0x40,
                content: true,
            }],
            ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
            selected_child: None,
            value_from_selection: false,
            window: Some(0x40),
            same_as_last: Some(verbatim_model::EarlierElement::Gone),
            bound_us: 500_000,
        };
        let fact = ListenerFact {
            pid: Pid(1234),
            fact: DeliveredFact::UiaFocusRead {
                marker: 9,
                read: Box::new(read.clone()),
                focus: true,
            },
        };
        let focus_fact = ListenerToCore::FocusFact {
            trace_id: TraceId::mint(),
            timing: EventTiming {
                observed_at_us: 1_752_000_000_000_000,
                ..EventTiming::default()
            },
            fact: fact.clone(),
            win_event: None,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &focus_fact).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: ListenerToCore = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, focus_fact);

        // The delivered fact is the same address minus the pid.
        assert_eq!(fact.pid(), Pid(1234));
        assert_eq!(
            fact.into_delivered(),
            DeliveredFact::UiaFocusRead {
                marker: 9,
                read: Box::new(read),
                focus: true,
            }
        );
        for fact in [
            DeliveredFact::UiaFocusMarker { marker: 9 },
            DeliveredFact::UiaFocusDropped {
                marker: 9,
                focus: true,
                answer: GapAnswer::LeavesOpen,
            },
            DeliveredFact::UiaFocusReadsEnded,
        ] {
            let deliver = CoreToOutpost::DeliverFact {
                trace_id: TraceId::mint(),
                timing: EventTiming {
                    observed_at_us: 1,
                    ..EventTiming::default()
                },
                fact,
                win_event: None,
            };
            let mut buffer = Vec::new();
            write_message(&mut buffer, &deliver).expect("writes");
            let read_back: Option<CoreToOutpost> =
                read_message(&mut buffer.as_slice()).expect("reads");
            assert_eq!(read_back, Some(deliver));
        }

        let deliver = CoreToOutpost::DeliverFact {
            trace_id: TraceId::mint(),
            timing: EventTiming {
                observed_at_us: 1_752_000_000_001_000,
                ..EventTiming::default()
            },
            fact: DeliveredFact::MsaaFocus {
                hwnd: 0x1234,
                id_object: -4,
                id_child: 0,
            },
            win_event: Some(WinEventStamp {
                thread: 77,
                time: 1000,
            }),
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &deliver).expect("writes");
        let read_back: Option<CoreToOutpost> = read_message(&mut buffer.as_slice()).expect("reads");
        assert_eq!(read_back, Some(deliver));
    }

    #[test]
    fn the_hand_over_messages_round_trip() {
        let pid = Pid(1234);
        let outpost = OutpostId(5);
        for command in [
            CoreToListener::HookInstalled {
                pid,
                outpost,
                sent_at_us: 4_000,
            },
            CoreToListener::HookRemoved {
                pid,
                sent_at_us: 4_000,
            },
        ] {
            let mut buffer = Vec::new();
            write_message(&mut buffer, &command).expect("writes");
            let read_back: Option<CoreToListener> =
                read_message(&mut buffer.as_slice()).expect("reads");
            assert_eq!(read_back, Some(command));
        }
        let command = CoreToOutpost::ForwardingEnded { through: 99 };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &command).expect("writes");
        let read_back: Option<CoreToOutpost> = read_message(&mut buffer.as_slice()).expect("reads");
        assert_eq!(read_back, Some(command));
        let ended = ListenerToCore::ForwardingEnded {
            pid,
            outpost,
            through: 99,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &ended).expect("writes");
        let read_back: Option<ListenerToCore> =
            read_message(&mut buffer.as_slice()).expect("reads");
        assert_eq!(read_back, Some(ended));
    }

    #[test]
    fn the_idle_barriers_query_and_answer_round_trip() {
        let query = CoreToOutpost::IdleQuery { round: 12 };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &query).expect("writes");
        let read_back: Option<CoreToOutpost> = read_message(&mut buffer.as_slice()).expect("reads");
        assert_eq!(read_back, Some(query));
        let answer = OutpostToCore::IdleAnswer { round: 12 };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &answer).expect("writes");
        let read_back: Option<OutpostToCore> = read_message(&mut buffer.as_slice()).expect("reads");
        assert_eq!(read_back, Some(answer));
        // It carries no node, so Core and the outpost do not count it.
        assert!(!OutpostToCore::IdleAnswer { round: 12 }.carries_nodes());
    }

    #[test]
    fn only_the_five_win_event_kinds_with_a_window_make_a_fact() {
        use verbatim_ia2::WinEventKind;
        for kind in [
            WinEventKind::Focus,
            WinEventKind::Foreground,
            WinEventKind::MenuPopupStart,
            WinEventKind::Alert,
            WinEventKind::Show,
        ] {
            let fact = fact_of_win_event(kind, (7, -4, 0))
                .unwrap_or_else(|| panic!("{kind:?} makes a fact"));
            assert!(fact.is_win_event(), "{kind:?}");
            assert_eq!(fact_of_win_event(kind, (0, -4, 0)), None);
        }
        assert_eq!(
            fact_of_win_event(WinEventKind::NameChange, (7, -4, 0)),
            None
        );
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
            Query::DumpTree { locations: false },
        ];
        let commands: Vec<CoreToOutpost> = queries
            .into_iter()
            .map(|query| CoreToOutpost::Query {
                trace_id: TraceId::mint(),
                request_id: 3,
                query,
                timing: EventTiming::default(),
            })
            .chain([CoreToOutpost::Cancel { request_id: 3 }])
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
                    text: None,
                }),
                observed_at_us: 42,
            })),
            QueryOutcome::Done(QueryResult::Navigated(None)),
            QueryOutcome::Done(QueryResult::Tree(DumpedTree {
                root: TreeNode {
                    snapshot: node,
                    children: Vec::new(),
                },
                truncated: true,
                locations: false,
            })),
            QueryOutcome::Gone,
            QueryOutcome::Failed("no activation pattern".to_owned()),
            QueryOutcome::NotStarted,
            QueryOutcome::Abandoned,
        ];
        let replies: Vec<OutpostToCore> = outcomes
            .into_iter()
            .map(|outcome| OutpostToCore::Reply {
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
                        connections: 0,
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
            let read: CoreToOutpost = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            assert_eq!(&read, command);
        }
        for reply in &replies {
            let read: OutpostToCore = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            assert_eq!(&read, reply);
        }
    }
}
