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
//! outpost's order holds; and while its end waits, so does everything about
//! the same application that came after it, its replacement's start and
//! frames above all, so the app hears of one application's incarnations in
//! the order the supervisor ended and started them; the helper's result comes back through the
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
use std::time::Duration;

use crossbeam_channel::{Sender, unbounded};
use verbatim_model::{OutpostId, Pid};

use super::owner::OwnerEvent;
use super::relay::Mailbox;
use super::{EventSink, OutpostMessage, Undelivered};
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
    /// One of its frames was not a message: its later frames are dropped,
    /// and only its end is handed on.
    failed: bool,
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
    /// By the application it watched, the outpost whose end waits behind
    /// one of its frames with the helper: what arrives about that
    /// application waits behind the end.
    held_ends: HashMap<Pid, OutpostId>,
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
            held_ends: HashMap::new(),
        }
    }

    /// Takes `message`, delivered by the supervisor, and appends to `out`
    /// what the reducer handles now, in order: the message itself; or, for
    /// a frame, the [`OutpostMessage::Event`] it carries, unless it waits
    /// behind a parse of that outpost's or goes to the helper; or, for the
    /// helper's result, that event and everything of the outpost's that
    /// waited behind it, up to its next large frame. An outpost's end
    /// waits behind its frames too, and while it waits, so does everything
    /// about the same application that arrives after it: another outpost's
    /// start, frames and end, and word that the application is not
    /// watched, each handed on after the end, in the order it arrived.
    pub fn arrived(&mut self, message: OutpostMessage, out: &mut Vec<OutpostMessage>) {
        if let Some(pid) = application_of(&message)
            && let Some(&held) = self.held_ends.get(&pid)
            && !matches!(&message, OutpostMessage::Ended { outpost, .. } if *outpost == held)
            && let Some(stream) = self.streams.get_mut(&held)
        {
            stream.behind.push_back(message);
            return;
        }
        match message {
            OutpostMessage::Frame(frame) => {
                let stream = self.streams.entry(frame.outpost).or_default();
                if stream.failed {
                    // Its outpost is being ended; the frame is dropped.
                } else if stream.parsing {
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
            OutpostMessage::Ended {
                outpost,
                target_pid,
                ..
            } => match self.streams.get_mut(&outpost) {
                Some(stream) if stream.parsing => {
                    stream.behind.push_back(message);
                    self.held_ends.insert(target_pid, outpost);
                }
                _ => {
                    if self.held_ends.get(&target_pid) == Some(&outpost) {
                        self.held_ends.remove(&target_pid);
                    }
                    let stream = self.streams.remove(&outpost);
                    out.push(message);
                    // What waited behind the end, about its application.
                    for next in stream.into_iter().flat_map(|stream| stream.behind) {
                        self.arrived(next, out);
                    }
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
    /// fails does, and nothing it sent after is handed on but its end.
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
                self.streams.entry(outpost).or_default().failed = true;
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
                        if !deliver(&sink, OutpostMessage::Parsed(parsed)) {
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

/// The application `message` is about, for those that can follow an
/// outpost's end: an outpost's start, frame or end, and word that an
/// application is not watched. The helper's result is its own outpost's
/// and waits for nothing.
fn application_of(message: &OutpostMessage) -> Option<Pid> {
    match message {
        OutpostMessage::Started { target_pid, .. }
        | OutpostMessage::Ended { target_pid, .. }
        | OutpostMessage::NotWatched { target_pid } => Some(*target_pid),
        OutpostMessage::Frame(frame) => Some(frame.pid),
        _ => None,
    }
}

/// How long the helper waits before it delivers again a result the app
/// refused for now ([`Undelivered::Refused`]).
const REDELIVER_AFTER: Duration = Duration::from_millis(50);

/// Delivers `message`, the helper's result, through `sink`, again after
/// [`REDELIVER_AFTER`] for as long as the app refuses it for now, since
/// its outpost's later messages wait in [`Frames`] until it arrives and the
/// idle barrier waits for it. Returns false once the app has gone.
fn deliver(sink: &EventSink, mut message: OutpostMessage) -> bool {
    let mut logged = false;
    loop {
        match sink.send(message) {
            Ok(()) => return true,
            Err(Undelivered::Gone) => return false,
            Err(Undelivered::Refused(refused)) => {
                if !logged {
                    tracing::error!(
                        "a large frame's parse could not be put in line; it is tried again until it is"
                    );
                    logged = true;
                }
                message = refused;
                thread::sleep(REDELIVER_AFTER);
            }
        }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crossbeam_channel::RecvTimeoutError;
    use verbatim_process::overlapped::Port;

    use super::*;
    use crate::supervisor::relay::mailbox;

    /// How long the test waits for the helper's result before it fails.
    const BOUND: Duration = Duration::from_secs(10);

    /// A frame larger than [`LARGE_FRAME_BYTES`] from outpost 1, which the
    /// helper parses.
    fn large_frame() -> Frame {
        frame(1, &"x".repeat(LARGE_FRAME_BYTES + 1))
    }

    /// A frame from `outpost`, watching application 5, carrying a fault
    /// whose detail is `detail`.
    fn frame(outpost: u64, detail: &str) -> Frame {
        let message = OutpostToSupervisor::Fault {
            detail: detail.to_owned(),
        };
        let body = serde_json::to_vec(&message).expect("encoded");
        Frame::new(Pid(5), OutpostId(outpost), (body, 0), 0)
    }

    /// Names each message handed on, for the order asserted.
    fn names(out: &[OutpostMessage]) -> Vec<String> {
        out.iter()
            .map(|message| match message {
                OutpostMessage::Event {
                    outpost, message, ..
                } => match &**message {
                    OutpostToSupervisor::Fault { detail } if detail.len() > 16 => {
                        format!("{outpost} large")
                    }
                    OutpostToSupervisor::Fault { detail } => format!("{outpost} {detail}"),
                    other => format!("{outpost} {other:?}"),
                },
                OutpostMessage::Started { outpost, .. } => format!("{outpost} started"),
                OutpostMessage::Ended { outpost, .. } => format!("{outpost} ended"),
                OutpostMessage::NotWatched { target_pid } => {
                    format!("{} not watched", target_pid.0)
                }
                other => format!("{other:?}"),
            })
            .collect()
    }

    /// An outpost's end waiting behind its large frame holds everything
    /// about its application that came after it, its replacement's start
    /// and frames and word that the application is not watched, so the app
    /// hears of the application's incarnations in the order the supervisor
    /// ended and started them; another application's messages pass.
    #[test]
    fn what_follows_an_outposts_end_about_its_application_waits_behind_it() {
        let (delivered_tx, delivered) = crossbeam_channel::unbounded();
        let sink = EventSink::from(delivered_tx);
        let (owner, _inbox) = mailbox(Arc::new(Port::new().expect("a port")));
        let mut frames = Frames::new(sink, owner);
        let mut out = Vec::new();
        frames.arrived(OutpostMessage::Frame(large_frame()), &mut out);
        frames.arrived(
            OutpostMessage::Ended {
                outpost: OutpostId(1),
                target_pid: Pid(5),
                reason: crate::supervisor::EndReason::Exited,
            },
            &mut out,
        );
        frames.arrived(
            OutpostMessage::Started {
                outpost: OutpostId(2),
                target_pid: Pid(5),
            },
            &mut out,
        );
        frames.arrived(OutpostMessage::Frame(frame(2, "ready")), &mut out);
        frames.arrived(OutpostMessage::NotWatched { target_pid: Pid(5) }, &mut out);
        frames.arrived(
            OutpostMessage::Started {
                outpost: OutpostId(3),
                target_pid: Pid(6),
            },
            &mut out,
        );
        assert_eq!(names(&out), ["3 started"], "only the other application's");
        out.clear();
        let parsed = delivered.recv_timeout(BOUND).expect("the helper's result");
        frames.arrived(parsed, &mut out);
        assert_eq!(
            names(&out),
            [
                "1 large",
                "1 ended",
                "2 started",
                "2 ready",
                "5 not watched"
            ]
        );
        assert!(!frames.parsing());
    }

    /// The app refusing the helper's result for now, as a port that
    /// refuses a packet does, loses nothing: the helper delivers it again,
    /// and the frame's outpost goes on.
    #[test]
    fn a_parse_the_app_refuses_for_now_is_delivered_again() {
        let (delivered_tx, delivered) = crossbeam_channel::unbounded();
        let refused_once = AtomicBool::new(false);
        let sink = EventSink::new(move |message| {
            if !refused_once.swap(true, Ordering::SeqCst) {
                return Err(Undelivered::Refused(message));
            }
            delivered_tx.send(message).map_err(|_| Undelivered::Gone)
        });
        let (owner, _inbox) = mailbox(Arc::new(Port::new().expect("a port")));
        let mut frames = Frames::new(sink, owner);
        let mut out = Vec::new();
        frames.arrived(OutpostMessage::Frame(large_frame()), &mut out);
        assert!(out.is_empty(), "the frame went to the helper");
        assert!(frames.parsing());
        let parsed = match delivered.recv_timeout(BOUND) {
            Ok(parsed) => parsed,
            Err(RecvTimeoutError::Timeout) => panic!("the refused result never came again"),
            Err(RecvTimeoutError::Disconnected) => panic!("the helper stopped"),
        };
        frames.arrived(parsed, &mut out);
        assert!(
            matches!(
                out.as_slice(),
                [OutpostMessage::Event { outpost: OutpostId(1), position: 0, message, .. }]
                    if matches!(**message, OutpostToSupervisor::Fault { .. })
            ),
            "{out:?}"
        );
        assert!(!frames.parsing());
    }
}
