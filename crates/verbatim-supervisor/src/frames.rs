//! The reducer's side of an outpost's frames (`docs/design/communication.md`,
//! sections 4.8 and 6.2): the relay reads every outpost's pipe and posts
//! each frame it does not handle itself whole, unparsed, among the
//! reducer's other inputs ([`OutpostMessage::Frame`]); the reducer passes
//! every message the supervisor delivers through [`Frames`], which parses
//! each frame into the [`OutpostMessage::Event`] it carries.
//!
//! Every frame, whatever its size, is parsed here on the reducer's thread
//! as it arrives, so each outpost's messages reach the reducer in the order
//! the outpost wrote them, and its end after all of them. A terminal's
//! whole-document read of 9,000 lines, its text in the frame's raw section,
//! takes 0.02 to 0.3 ms to parse in a release build (`docs/performance.md`).
//!
//! Parsing here does what the outposts' reader threads did: it stamps every
//! node id with the outpost's generation, records when the relay read the
//! frame, and numbers the messages that carry node ids, in pipe order, as
//! the outpost numbers them.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io;

use verbatim_model::{OutpostId, Pid};
use verbatim_process::transport::OwnedFrame;

use super::OutpostMessage;
use super::owner::OwnerEvent;
use super::relay::Mailbox;
use verbatim_model::protocol::OutpostToCore;
use verbatim_process::message::decode_message;

/// A frame from an outpost's pipe, read whole by the relay and not yet
/// parsed. Only [`Frames`] opens it.
pub struct Frame {
    pid: Pid,
    outpost: OutpostId,
    /// The frame's body and raw section: in the reader's own buffer, given
    /// up, for a frame read over several reads, its header still ahead of
    /// them; in a copy otherwise.
    bytes: OwnedFrame,
    /// When the relay read it, in microseconds since the Unix epoch.
    read_at_us: u64,
}

impl Frame {
    /// The frame `outpost` sent.
    pub(super) fn new(pid: Pid, outpost: OutpostId, bytes: OwnedFrame, read_at_us: u64) -> Self {
        Self {
            pid,
            outpost,
            bytes,
            read_at_us,
        }
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts = self.bytes.parts();
        formatter
            .debug_struct("Frame")
            .field("outpost", &self.outpost)
            .field("bytes", &parts.body.len())
            .field("raw", &parts.raw.map(<[u8]>::len))
            .finish_non_exhaustive()
    }
}

/// One outpost generation's frames on the reducer's side.
#[derive(Default)]
struct Stream {
    /// How many messages that carry node ids it has sent, as the outpost
    /// counts them.
    position: u64,
    /// One of its frames was not a message: its later frames are dropped,
    /// and only its end is handed on.
    failed: bool,
}

/// Turns the frames the relay posts into the messages they carry, in each
/// outpost's order. Made by [`super::Supervisor::frames`]; the reducer
/// passes every [`OutpostMessage`] the supervisor delivers through
/// [`Frames::arrived`].
pub struct Frames {
    owner: Mailbox<OwnerEvent>,
    streams: HashMap<OutpostId, Stream>,
}

impl std::fmt::Debug for Frames {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Frames")
            .field("outposts", &self.streams.len())
            .finish_non_exhaustive()
    }
}

impl Frames {
    pub(super) fn new(owner: Mailbox<OwnerEvent>) -> Self {
        Self {
            owner,
            streams: HashMap::new(),
        }
    }

    /// Takes `message`, delivered by the supervisor, and returns what the
    /// reducer handles now: for a frame, the [`OutpostMessage::Event`] it
    /// carries, or nothing when the frame is not a message or its outpost
    /// sent one that was not; any other message itself.
    pub fn arrived(&mut self, message: OutpostMessage) -> Option<OutpostMessage> {
        match message {
            OutpostMessage::Frame(frame) => self.take_frame(&frame),
            OutpostMessage::Ended { outpost, .. } => {
                self.streams.remove(&outpost);
                Some(message)
            }
            other => Some(other),
        }
    }

    /// Parses `frame` and numbers the message it carries. A frame that is
    /// not a message has the outpost ended, as a pipe that fails does, and
    /// nothing it sent after is handed on but its end.
    fn take_frame(&mut self, frame: &Frame) -> Option<OutpostMessage> {
        let stream = self.streams.entry(frame.outpost).or_default();
        if stream.failed {
            // Its outpost is being ended; the frame is dropped.
            return None;
        }
        match parse(frame) {
            Ok(message) => {
                if message.carries_nodes() {
                    stream.position += 1;
                }
                Some(OutpostMessage::Event {
                    pid: frame.pid,
                    outpost: frame.outpost,
                    position: stream.position,
                    message,
                })
            }
            Err(error) => {
                let outpost = frame.outpost;
                tracing::warn!(%outpost, %error, "an outpost's message could not be read");
                stream.failed = true;
                let _ = self.owner.send(OwnerEvent::Unreadable(outpost));
                None
            }
        }
    }
}

/// Parses `frame` into the message it carries, stamped with when the relay
/// read it and with its outpost on every node id: node ids name the
/// generation whose pipe they arrived on, never whatever the body claims.
fn parse(frame: &Frame) -> io::Result<Box<OutpostToCore>> {
    let mut message: Box<OutpostToCore> = Box::new(decode_message(frame.bytes.parts())?);
    message.core_read(frame.read_at_us);
    message.assign_outpost(frame.outpost);
    Ok(message)
}
