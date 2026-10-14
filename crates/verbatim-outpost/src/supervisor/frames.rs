//! The reducer's side of an outpost's frames (`docs/design/communication.md`,
//! sections 4.8 and 6.2): the relay reads every outpost's pipe and posts
//! each frame it does not handle itself whole, unparsed, among the
//! reducer's other inputs ([`OutpostMessage::Frame`]); the reducer passes
//! every message the supervisor delivers through [`Frames`], which parses
//! each frame into the [`OutpostMessage::Event`] it carries.
//!
//! A frame larger than [`LARGE_FRAME_BYTES`], a megabyte of terminal output
//! for instance, is parsed by a helper thread while the reducer goes on
//! with its other inputs, keys and other outposts' frames among them. That
//! outpost's later frames, and its end, wait behind the parse, so each
//! outpost's order holds; the helper's result comes back through the
//! supervisor's sink as [`OutpostMessage::Parsed`], and so is ordered among
//! the reducer's inputs at the moment the parse finished, as a reader
//! thread's parse once was.
//!
//! Parsing here does what the outposts' reader threads did: it stamps every
//! node id with the outpost's incarnation, records when the relay read the
//! frame, and numbers the messages that carry node ids, in pipe order, as
//! the outpost numbers them.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::io;
use std::thread;

use crossbeam_channel::{Sender, unbounded};
use verbatim_model::{OutpostId, Pid};

use super::owner::OwnerEvent;
use super::relay::Mailbox;
use super::{EventSink, OutpostMessage};
use crate::protocol::{OutpostToSupervisor, decode_message};

/// The size of body above which a frame is parsed by the helper thread
/// rather than on the reducer's (section 4.2): about half a millisecond of
/// parsing in a release build.
pub const LARGE_FRAME_BYTES: usize = 256 * 1024;

/// A frame from an outpost's pipe, read whole by the relay and not yet
/// parsed. Only [`Frames`] opens it.
pub struct Frame {
    pid: Pid,
    outpost: OutpostId,
    /// The buffer the body is in: the reader's own, given up, for a frame
    /// read over several reads, its header still ahead of the body; a copy
    /// of the body otherwise.
    buffer: Vec<u8>,
    /// Where the body starts in `buffer`.
    start: usize,
    /// When the relay read it, in microseconds since the Unix epoch.
    read_at_us: u64,
}

impl Frame {
    /// The frame `outpost` sent, its body in `buffer` from `start` on.
    pub(super) fn new(
        pid: Pid,
        outpost: OutpostId,
        (buffer, start): (Vec<u8>, usize),
        read_at_us: u64,
    ) -> Self {
        Self {
            pid,
            outpost,
            buffer,
            start,
            read_at_us,
        }
    }

    /// The frame's body.
    fn body(&self) -> &[u8] {
        &self.buffer[self.start..]
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("outpost", &self.outpost)
            .field("bytes", &self.body().len())
            .finish_non_exhaustive()
    }
}

/// A large frame the helper thread has parsed. Only [`Frames`] opens it.
pub struct Parsed {
    pid: Pid,
    outpost: OutpostId,
    message: io::Result<Box<OutpostToSupervisor>>,
}

impl std::fmt::Debug for Parsed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Parsed")
            .field("outpost", &self.outpost)
            .finish_non_exhaustive()
    }
}

/// One outpost incarnation's frames on the reducer's side.
#[derive(Default)]
struct Stream {
    /// How many messages that carry node ids it has sent, as the outpost
    /// counts them.
    position: u64,
    /// Whether one of its frames is with the helper.
    parsing: bool,
    /// What arrived from it, or about it, while a frame was with the
    /// helper, in order.
    behind: VecDeque<OutpostMessage>,
}

/// Turns the frames the relay posts into the messages they carry, in each
/// outpost's order. Made by [`super::Supervisor::frames`]; the reducer
/// passes every [`OutpostMessage`] the supervisor delivers through
/// [`Frames::arrived`].
pub struct Frames {
    sink: EventSink,
    owner: Mailbox<OwnerEvent>,
    /// The helper thread's work, once it has been started.
    helper: Option<Sender<Frame>>,
    streams: HashMap<OutpostId, Stream>,
}

impl std::fmt::Debug for Frames {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Frames")
            .field("parsing", &self.parsing())
            .finish_non_exhaustive()
    }
}

impl Frames {
    pub(super) fn new(sink: EventSink, owner: Mailbox<OwnerEvent>) -> Self {
        Self {
            sink,
            owner,
            helper: None,
            streams: HashMap::new(),
        }
    }

    /// Takes `message`, delivered by the supervisor, and appends to `out`
    /// what the reducer handles now, in order: the message itself; or, for
    /// a frame, the [`OutpostMessage::Event`] it carries, unless it waits
    /// behind a parse of that outpost's or goes to the helper; or, for the
    /// helper's result, that event and everything of the outpost's that
    /// waited behind it, up to its next large frame. An outpost's end
    /// waits behind its frames too.
    pub fn arrived(&mut self, message: OutpostMessage, out: &mut Vec<OutpostMessage>) {
        match message {
            OutpostMessage::Frame(frame) => {
                let stream = self.streams.entry(frame.outpost).or_default();
                if stream.parsing {
                    stream.behind.push_back(OutpostMessage::Frame(frame));
                } else {
                    self.take_frame(frame, out);
                }
            }
            OutpostMessage::Parsed(parsed) => {
                let outpost = parsed.outpost;
                if let Some(stream) = self.streams.get_mut(&outpost) {
                    stream.parsing = false;
                }
                self.finish(parsed.pid, outpost, parsed.message, out);
                self.release(outpost, out);
            }
            OutpostMessage::Ended { outpost, .. } => match self.streams.get_mut(&outpost) {
                Some(stream) if stream.parsing => stream.behind.push_back(message),
                _ => {
                    self.streams.remove(&outpost);
                    out.push(message);
                }
            },
            other => out.push(other),
        }
    }

    /// Whether a frame is with the helper, its outpost's later messages
    /// waiting behind it: the idle barrier waits for it.
    #[must_use]
    pub fn parsing(&self) -> bool {
        self.streams.values().any(|stream| stream.parsing)
    }

    /// Parses `frame` now, or hands it to the helper when it is large.
    fn take_frame(&mut self, frame: Frame, out: &mut Vec<OutpostMessage>) {
        if frame.body().len() > LARGE_FRAME_BYTES {
            let outpost = frame.outpost;
            match self.helper().send(frame) {
                Ok(()) => {
                    self.streams.entry(outpost).or_default().parsing = true;
                    return;
                }
                // No helper: parsed here instead.
                Err(unsent) => {
                    let frame = unsent.into_inner();
                    let message = parse(&frame);
                    self.finish(frame.pid, frame.outpost, message, out);
                    return;
                }
            }
        }
        let message = parse(&frame);
        self.finish(frame.pid, frame.outpost, message, out);
    }

    /// Numbers `message`, parsed from `outpost`'s frame, and appends it; a
    /// frame that is not a message has the outpost ended, as a pipe that
    /// fails does.
    fn finish(
        &mut self,
        pid: Pid,
        outpost: OutpostId,
        message: io::Result<Box<OutpostToSupervisor>>,
        out: &mut Vec<OutpostMessage>,
    ) {
        match message {
            Ok(message) => {
                let stream = self.streams.entry(outpost).or_default();
                if message.carries_nodes() {
                    stream.position += 1;
                }
                out.push(OutpostMessage::Event {
                    pid,
                    outpost,
                    position: stream.position,
                    message,
                });
            }
            Err(error) => {
                tracing::warn!(%outpost, %error, "an outpost's message could not be read");
                let _ = self.owner.send(OwnerEvent::Unreadable(outpost));
            }
        }
    }

    /// Hands on what waited behind `outpost`'s parse, in order, until it
    /// is all handed on or another of its frames goes to the helper.
    fn release(&mut self, outpost: OutpostId, out: &mut Vec<OutpostMessage>) {
        loop {
            let Some(stream) = self.streams.get_mut(&outpost) else {
                return;
            };
            if stream.parsing {
                return;
            }
            let Some(next) = stream.behind.pop_front() else {
                return;
            };
            self.arrived(next, out);
        }
    }

    /// The helper thread's work queue, starting the thread on first use.
    /// It waits on its queue, so it wakes only for a large frame.
    fn helper(&mut self) -> &Sender<Frame> {
        self.helper.get_or_insert_with(|| {
            let (work_tx, work_rx) = unbounded::<Frame>();
            let sink = self.sink.clone();
            let spawned = thread::Builder::new()
                .name("verbatim-parse".to_owned())
                .spawn(move || {
                    for frame in work_rx {
                        let message = parse(&frame);
                        let parsed = Parsed {
                            pid: frame.pid,
                            outpost: frame.outpost,
                            message,
                        };
                        if !sink.send(OutpostMessage::Parsed(parsed)) {
                            return;
                        }
                    }
                });
            if let Err(error) = spawned {
                tracing::error!(%error, "no thread to parse large frames; they are parsed on the reducer's");
                // A queue nobody reads: every send to it fails, and the
                // frame is parsed where it arrived.
                let (closed_tx, _) = unbounded();
                return closed_tx;
            }
            work_tx
        })
    }
}

/// Parses `frame` into the message it carries, stamped with when the relay
/// read it and with its outpost on every node id: node ids name the
/// incarnation whose pipe they arrived on, never whatever the body claims.
fn parse(frame: &Frame) -> io::Result<Box<OutpostToSupervisor>> {
    let mut message: Box<OutpostToSupervisor> = Box::new(decode_message(frame.body())?);
    message.core_read(frame.read_at_us);
    message.assign_outpost(frame.outpost);
    Ok(message)
}
