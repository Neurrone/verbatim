//! The lifecycle owner: one thread that makes every lifecycle decision and
//! owns the per-application records and the listener record (outpost
//! redesign, "The supervisor"). Everything else only reports to it.
//!
//! It is also the relay (`docs/design/communication.md`, sections 4.7 and
//! 6.2): it waits only on the relay's completion port ([`super::relay`]),
//! reads the listener's frames and routes each fact in the pass that read
//! it, writing it to its outpost inline; reads every outpost's frames,
//! handling pongs, `Ready`, `TargetExited` and the pipe's end itself and
//! delivering every other frame to the app whole, in the pass that read
//! it; finishes every child's writes that pended; and takes everything
//! other threads report from its mailbox when the port says there is
//! something. Its heartbeat, which also watches the reducer, and its sweep
//! run when the wait's deadline passes. It never blocks: launches,
//! retirements, and the check of whether an application is hung run on
//! threads of their own and report back.

#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io;
use std::os::windows::io::OwnedHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use verbatim_model::{OutpostId, Pid, TraceId};
use verbatim_process::overlapped::{Completion, Completions, Port};
use verbatim_process::port_queue::Progress;
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};
use verbatim_process::transport::{FrameBody, Reading};

use crate::protocol::{
    DeliveredFact, EventTiming, ListenerFact, LockGrant, OutpostToSupervisor, SupervisorToOutpost,
    WinEventStamp, decode_message, hop_us, message_kind, now_us,
};

use super::frames::Frame;
use super::ignored::IgnoredProcesses;
use super::policy::{
    Backoff, HeldFact, HeldFacts, Spares, WedgeReason, retirement_decision, wedge_decision,
};
use super::process::{self, Launched, Role, Target};
use super::relay::{COMPLETIONS_PER_WAIT, CommandPipes, Inbox, Mailbox, Relay, Source, Taken};
use super::retire::{Ending, ShutdownSummary, retire};
use super::writer::{Outgoing, Received, WriterHandle};
use super::{
    ABANDONED_WORKER_LIMIT, BACKOFF, EndReason, EventSink, IDLE_RETIREMENT, MISSED_PONG_THRESHOLD,
    OutpostMessage, PING_INTERVAL, ReducerStall, SPARES, SWEEP_INTERVAL, Undelivered, Writers,
};

/// The relay thread's priority: it reads each of the listener's facts and
/// routes it to its outpost, reads each outpost's frame and hands it on,
/// finishes writes that pended, and handles each child's reports, for
/// microseconds per wake (`docs/design/communication.md`, section 6.1).
const RELAY_PRIORITY: ThreadPriority = ThreadPriority::Highest;

/// A fact reported to the owner.
#[expect(
    clippy::large_enum_variant,
    reason = "an event is sent once and moved, never stored in bulk"
)]
pub(super) enum OwnerEvent {
    /// From the app: start an outpost for this application if none exists.
    EnsureSpawned(Pid),
    /// From the app: the views derived from the reducer state.
    Views {
        attention: Option<Pid>,
        holding: BTreeSet<OutpostId>,
    },
    /// From the app: Core took the event traced `trace_id` as the focus,
    /// after a focus `superseded` reported, when that was another
    /// outpost's.
    FocusQueued {
        trace_id: TraceId,
        observed_at_ms: u64,
        superseded: Option<OutpostId>,
    },
    /// Read from the listener's pipe: a focus fact to route.
    Fact {
        trace_id: TraceId,
        observed_at_ms: u64,
        timing: EventTiming,
        fact: ListenerFact,
        win_event: Option<WinEventStamp>,
        /// When verbatim.exe read it from the listener.
        received: Received,
    },
    /// Read from the listener's pipe: it forwards none of `pid`'s
    /// `WinEvent` facts raised after `through`, the answer to the hand-over
    /// for `outpost`.
    ForwardingEnded {
        pid: Pid,
        outpost: OutpostId,
        through: u32,
    },
    /// Read from the listener's pipe: a menu or the Alt+Tab switcher
    /// closed.
    MenuOrSwitchEnded { ended_at_ms: u64 },
    /// Read from a child's pipe: the child sent `Ready`.
    Ready(OutpostId),
    /// Read from a child's pipe: the child answered a ping.
    Pong {
        outpost: OutpostId,
        abandoned: usize,
    },
    /// From the app's [`super::Frames`]: a frame `outpost` sent is not a
    /// message, so it is ended as though its pipe had failed.
    Unreadable(OutpostId),
    /// From a launch helper: the launch finished.
    Launched {
        outpost: OutpostId,
        result: io::Result<Launch>,
    },
    /// From a retirement's thread: a child has ended, this way.
    Retired(Ending),
    /// From a hang check's thread: whether `outpost`'s application has a
    /// visible window the system reports hung.
    HangChecked { outpost: OutpostId, hung: bool },
    /// From the app: run the idle barrier's round through the children.
    IdleRound(u64),
    /// From the app: Verbatim is exiting. Every child is shut down, and the
    /// summary of every ending in Verbatim's life is sent back once all
    /// have ended; the owner then ends.
    Shutdown(Sender<ShutdownSummary>),
}

/// A running child: its process, held so ending the record kills it, and
/// its writer.
pub(super) struct Child {
    pub(super) process: Launched,
    pub(super) writer: WriterHandle,
}

/// A launched child, with its pipe to Core, which the relay reads.
pub(super) struct Launch {
    child: Child,
    from_child: OwnedHandle,
    source: Source,
}

/// What the owner thread starts with.
pub(super) struct Setup {
    pub(super) exe_path: PathBuf,
    pub(super) options: crate::OutpostOptions,
    pub(super) shutdown_limit: Duration,
    pub(super) events_tx: EventSink,
    pub(super) writers: Writers,
    pub(super) own_tx: Mailbox<OwnerEvent>,
    pub(super) events: Inbox<OwnerEvent>,
    /// The relay, whose port `own_tx` wakes.
    pub(super) relay: Relay,
    /// The processes ignored entirely.
    pub(super) ignored: std::sync::Arc<IgnoredProcesses>,
}

/// One application's outpost: starting until its launch reports back, or
/// until a spare given to it sends `Ready`, then running; or, after its
/// outpost failed, waiting for the failure's backoff to end, with no
/// incarnation (`outpost` is `OutpostId(0)` and its backoff is due).
struct Record {
    outpost: OutpostId,
    /// `None` while the launch helper is still at work.
    child: Option<Child>,
    /// Whether this incarnation was a spare
    /// (`docs/design/focus-pipeline.md`, section 19), whose end before
    /// `Ready` is a spare's failure, not the application's crash, and
    /// whether it has been sent its `Assign`.
    spare: Option<Assigned>,
    /// Whether the outpost has sent `Ready`.
    ready: bool,
    /// Facts that arrived before `Ready`, released in order on `Ready`.
    held: HeldFacts,
    last_pong_at: Instant,
    abandoned: usize,
    /// Whether a check of whether its application is hung is running.
    hang_check: bool,
    /// When the application last held attention.
    last_attention_at: Instant,
    /// The idle barrier's round, held behind the held facts until `Ready`.
    idle_query: Option<u64>,
    /// The last fault a spare given to the application reported before its
    /// `Ready`, such as its hook failing: logged with its failure.
    fault: Option<String>,
    /// When the current incarnation was given to the application or
    /// launched for it: how long it had been up when it fails.
    since: Instant,
    /// The failures of the application's outposts while serving it, and,
    /// while a failure's wait runs, when the next incarnation is given.
    backoff: Backoff,
    /// The incarnation that failed before its `Ready`, while its backoff
    /// runs: the app hears it replaced ([`OutpostMessage::Reincarnated`])
    /// when the next is given.
    unready: Option<Unready>,
}

/// An incarnation that failed before its `Ready`, whose replacement waits
/// for the failure's backoff.
#[derive(Clone, Copy, Debug)]
struct Unready {
    outpost: OutpostId,
    reason: EndReason,
    /// Whether the app heard it started, and so hears it ended.
    started: bool,
}

/// Where the idle barrier's round is in the relay
/// (`docs/design/communication.md`, section 6.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdleStage {
    /// The listener has been asked and has not answered.
    Listener(u64),
    /// The listener has answered, or has gone, and an outpost is still
    /// being launched: the outposts are asked once every launch has
    /// reported, so the round covers the facts held for it.
    Launches(u64),
}

/// Whether a spare given to an application has been sent its `Assign`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Assigned {
    /// Sent, and the app told the incarnation `Started`.
    Sent,
    /// Not yet: it was given while its launch was still at work, and is
    /// sent `Assign`, and the app `Started`, once it has reported.
    Pending,
}

/// The focus listener.
struct ListenerRecord {
    outpost: OutpostId,
    child: Option<Child>,
    last_pong_at: Instant,
    /// Whether this listener replaces an earlier one: facts were lost in the
    /// gap, so the attention application is asked to report again on `Ready`.
    replacement: bool,
    /// The hand-over's news for it, held while its launch is still at work,
    /// in order, and written once it has a writer.
    pending: Vec<SupervisorToOutpost>,
}

/// The relay's messages to the app, delivered through its sink in the
/// order the relay sends them. One the app refuses for now
/// ([`Undelivered::Refused`]: its port refusing the packet, which only a
/// system out of memory does) is kept, and so is every message sent after
/// it, behind it, and they are delivered again, in order, at the relay's
/// next send and at every heartbeat, as the relay's own mailbox is taken
/// again then after a wake its port refused. So nothing the relay sends is
/// lost, and nothing overtakes a message before it: an outpost's `Ended`
/// still comes before its replacement's `Started`. Everything is dropped
/// once the app has gone, since Verbatim is exiting.
struct Outbox {
    sink: EventSink,
    /// Refused and not yet delivered, in order.
    refused: VecDeque<OutpostMessage>,
}

impl Outbox {
    fn new(sink: EventSink) -> Self {
        Self {
            sink,
            refused: VecDeque::new(),
        }
    }

    /// Delivers `message`, behind any refused before it.
    fn send(&mut self, message: OutpostMessage) {
        if !self.refused.is_empty() {
            self.refused.push_back(message);
            self.retry();
            return;
        }
        match self.sink.send(message) {
            Ok(()) | Err(Undelivered::Gone) => {}
            Err(Undelivered::Refused(message)) => {
                tracing::error!(
                    "a message to the app could not be put in line; it and every later one are tried again at each send and heartbeat"
                );
                self.refused.push_back(message);
            }
        }
    }

    /// Delivers again, in order, what was refused, stopping at the first
    /// message refused again.
    fn retry(&mut self) {
        while let Some(message) = self.refused.pop_front() {
            match self.sink.send(message) {
                Ok(()) => {}
                Err(Undelivered::Gone) => {
                    self.refused.clear();
                    return;
                }
                Err(Undelivered::Refused(message)) => {
                    self.refused.push_front(message);
                    return;
                }
            }
        }
    }
}

/// The owner thread's state.
struct Owner {
    exe_path: PathBuf,
    /// What every outpost is launched with.
    options: crate::OutpostOptions,
    outbox: Outbox,
    writers: Writers,
    own_tx: Mailbox<OwnerEvent>,
    /// The port this thread waits on, and the pipes it serves.
    relay: Relay,
    /// The relay's port, which a reading of the listener's pipe is given.
    port: Arc<Port>,
    /// Numbers every launch, outposts and listener alike; never reused.
    generation: u64,
    records: HashMap<Pid, Record>,
    listener: Option<ListenerRecord>,
    /// The applications given a record, each until its process exits: the
    /// process, held open so the pid it is known by names that process and
    /// no other ([`Target`]), and its lock (`docs/design/focus-pipeline.md`,
    /// section 20.4), which outlives the record.
    apps: HashMap<Pid, App>,
    attention: Option<Pid>,
    holding: BTreeSet<OutpostId>,
    ping_seq: u64,
    own_pid: Pid,
    /// How long an ending child has to exit after the shutdown message.
    shutdown_limit: Duration,
    /// How many children are being ended, each on a thread of its own
    /// ([`retire`]), which reports [`OwnerEvent::Retired`] when done.
    retiring: usize,
    /// How the children that have finished ending ended.
    summary: ShutdownSummary,
    /// Set once Verbatim is exiting: where the summary goes once every
    /// child has ended.
    shutting_down: Option<Sender<ShutdownSummary>>,
    /// The processes ignored entirely: no outpost is started for them, and
    /// the listener, told of them, drops their facts.
    ignored: std::sync::Arc<IgnoredProcesses>,
    /// The idle barrier's round under way in the relay, until the
    /// outposts are asked.
    idle: Option<IdleStage>,
    /// The watch over the reducer, read at each heartbeat.
    reducer: ReducerWatch,
    /// The spare outposts kept waiting, and the replacements owed
    /// (`docs/design/focus-pipeline.md`, section 19).
    spares: Spares,
    /// Every spare's process, by outpost id, from its launch until it is
    /// given or ends: `None` while its launch is at work.
    spare_children: HashMap<OutpostId, SpareChild>,
}

/// An application the supervisor has given a record, from its first record
/// until its process exits, through any retirement and every outpost
/// failure (`docs/design/focus-pipeline.md`, section 20.4).
struct App {
    /// Its process, held open.
    target: Target,
    /// The process's creation time, which with its id names it.
    created: u64,
    /// The mutex with no name made for it, which the listener and its
    /// outposts take around the UIA calls that collide
    /// (`verbatim_uia::lock`); `None` when it could not be made, when their
    /// calls run unguarded.
    lock: Option<Arc<OwnedHandle>>,
}

/// A spare outpost's process while it waits.
struct SpareChild {
    /// `None` while the launch helper is still at work.
    child: Option<Child>,
    last_pong_at: Instant,
    /// The last fault it reported: logged with its failure.
    fault: Option<String>,
    /// When it was launched: how long it had been up when it fails.
    started_at: Instant,
}

/// The relay's watch over the thread taking the app's inputs, the reducer
/// (`docs/design/communication.md`, section 6.8): a fault of its own,
/// reported and never acted on.
#[derive(Default)]
struct ReducerWatch {
    /// Its progress at the last heartbeat, and when that was.
    last: Option<(Progress, Instant)>,
    /// How many heartbeats in a row have found it taking nothing while
    /// inputs waited, and since when.
    stuck: Option<(u32, Instant)>,
    /// Whether this stall has been reported.
    reported: bool,
}

impl ReducerWatch {
    /// Notes the reducer's `progress` at `now`, a heartbeat, and returns
    /// the stall to report, once per stall: none of its inputs taken for
    /// [`MISSED_PONG_THRESHOLD`] heartbeats in a row while some waited.
    fn note(&mut self, progress: Progress, now: Instant) -> Option<ReducerStall> {
        let stuck = self
            .last
            .is_some_and(|(last, _)| last.waiting() > 0 && last.taken == progress.taken);
        let since = self.last.map_or(now, |(_, at)| at);
        self.last = Some((progress, now));
        if !stuck {
            self.stuck = None;
            self.reported = false;
            return None;
        }
        let (count, since) = self.stuck.get_or_insert((0, since));
        *count += 1;
        if *count < MISSED_PONG_THRESHOLD || self.reported {
            return None;
        }
        self.reported = true;
        Some(ReducerStall {
            stalled_for: now.saturating_duration_since(*since),
            waiting: progress.waiting(),
        })
    }
}

/// Starts the owner thread, which starts the listener at once.
pub(super) fn start(setup: Setup) -> io::Result<()> {
    let Setup {
        exe_path,
        options,
        shutdown_limit,
        events_tx,
        writers,
        own_tx,
        events,
        relay,
        ignored,
    } = setup;
    let mut owner = Owner {
        exe_path,
        options,
        shutdown_limit,
        retiring: 0,
        summary: ShutdownSummary::default(),
        shutting_down: None,
        outbox: Outbox::new(events_tx),
        writers,
        own_tx,
        port: Arc::clone(relay.port()),
        relay,
        generation: 0,
        records: HashMap::new(),
        listener: None,
        apps: HashMap::new(),
        attention: None,
        holding: BTreeSet::new(),
        ping_seq: 0,
        own_pid: Pid(std::process::id()),
        ignored,
        idle: None,
        reducer: ReducerWatch::default(),
        spares: Spares::default(),
        spare_children: HashMap::new(),
    };
    thread::Builder::new()
        .name("verbatim-relay".to_owned())
        .spawn(move || {
            prioritize_role("relay", RELAY_PRIORITY);
            owner.start_listener(false);
            owner.run(&events);
        })?;
    Ok(())
}

/// The next time a periodic duty with `interval` is due after `due`, which
/// has just been done at `now`: a missed time is skipped, never made up.
fn next_due(due: Instant, now: Instant, interval: Duration) -> Instant {
    let next = due + interval;
    if next > now { next } else { now + interval }
}

impl Owner {
    /// The relay's loop: waits on the port until the heartbeat or the
    /// sweep is due, and handles what the wait took, in the port's order.
    /// Returns once Verbatim's exit has ended every child and every pipe
    /// has finished.
    fn run(&mut self, events: &Inbox<OwnerEvent>) {
        let start = Instant::now();
        let mut heartbeat_due = start + PING_INTERVAL;
        let mut sweep_due = start + SWEEP_INTERVAL;
        let mut completions = Completions::with_capacity(COMPLETIONS_PER_WAIT);
        loop {
            if self.finish_shutdown() {
                return;
            }
            let timeout = [Some(heartbeat_due), Some(sweep_due), self.next_due()]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(heartbeat_due)
                .saturating_duration_since(Instant::now());
            if let Err(error) = self.relay.wait(&mut completions, Some(timeout)) {
                tracing::error!(%error, "the relay's wait failed; the supervisor stops");
                return;
            }
            for completion in completions.iter() {
                match self.relay.take(&completion) {
                    Taken::Mailbox => {
                        for event in events.take() {
                            self.handle(event);
                        }
                    }
                    Taken::Write => {}
                    Taken::Read(key) => self.read_pipe(key, &completion),
                }
            }
            let now = Instant::now();
            self.start_due(now);
            if now >= heartbeat_due {
                // What was sent while the port refused the mailbox's wake,
                // and what the app's port refused.
                for event in events.take() {
                    self.handle(event);
                }
                self.outbox.retry();
                self.heartbeat();
                heartbeat_due = next_due(heartbeat_due, now, PING_INTERVAL);
            }
            if now >= sweep_due {
                self.sweep();
                sweep_due = next_due(sweep_due, now, SWEEP_INTERVAL);
            }
        }
    }

    /// Reads the child's pipe under `key` on `completion`: handles each
    /// message read, in order, and the pipe's end after them.
    fn read_pipe(&mut self, key: usize, completion: &Completion) {
        let Some(mut read) = self.relay.take_reader(key) else {
            // A wake for a pipe whose reading has ended.
            return;
        };
        let port = Arc::clone(&self.port);
        let outpost = read.outpost;
        let source = read.source;
        let reading = read
            .pipe
            .take_bodies(completion, &port, |body| match source {
                Source::Listener => {
                    // When verbatim.exe took the message in, before it is
                    // parsed (`docs/design/communication.md`, section 10.1).
                    let received = Received::now();
                    self.listener_message(outpost, received, decode_message(body.bytes())?);
                    Ok(())
                }
                Source::Outpost(pid) => self.outpost_frame(outpost, pid, body),
                Source::Spare => self.spare_frame(outpost, &body),
            });
        match (reading, source) {
            (Reading::Waiting | Reading::Yielded, _) => self.relay.put_reader(key, read),
            (Reading::Ended(Ok(())), Source::Listener) => {
                tracing::info!(%outpost, "the focus listener's pipe closed");
                self.on_pipe_closed(outpost);
            }
            (Reading::Ended(Err(error)), Source::Listener) => {
                tracing::warn!(%outpost, %error, "the focus listener's pipe ended in a fault");
                self.on_pipe_closed(outpost);
            }
            (Reading::Ended(Ok(())), Source::Outpost(_) | Source::Spare) => {
                self.on_pipe_closed(outpost);
            }
            (Reading::Ended(Err(error)), Source::Outpost(_) | Source::Spare) => {
                // A message too large, or not a message: the outpost is
                // treated as failed, as when its pipe closes.
                tracing::warn!(%outpost, %error, "an outpost's message could not be read");
                self.on_pipe_closed(outpost);
            }
        }
    }

    /// A frame read from the pipe of `outpost`, watching `pid`, dropped
    /// once the owner has ended that outpost: a pong and
    /// `TargetExited` are handled here, `Ready` is handled here and handed
    /// on, and every other frame is handed on to the app whole, unparsed,
    /// with when it was read (`docs/design/communication.md`, section
    /// 6.2). Only the frame's kind is read, from its first bytes; a pong,
    /// a few dozen bytes, is parsed for its count of abandoned workers. A
    /// frame handed on keeps the reader's buffer when the reader gave it
    /// up, a large frame read over several reads, and is copied out of the
    /// buffer it was lent from otherwise.
    fn outpost_frame(
        &mut self,
        outpost: OutpostId,
        pid: Pid,
        body: FrameBody<'_>,
    ) -> io::Result<()> {
        if self
            .records
            .get(&pid)
            .is_none_or(|record| record.outpost != outpost)
        {
            // The owner has ended this incarnation itself, and told the
            // app, which drops whatever it sent after; its pipe is read
            // until it ends, and the rest is dropped here.
            return Ok(());
        }
        let read_at_us = now_us();
        match message_kind(body.bytes()) {
            Some("Pong") => {
                let OutpostToSupervisor::Pong {
                    parked_count,
                    written_at_us,
                    ..
                } = decode_message(body.bytes())?
                else {
                    return Err(io::Error::other("a pong that is not a pong"));
                };
                if let Some(us) = hop_us(written_at_us, read_at_us) {
                    tracing::trace!(target: "verbatim::stage", stage = "outpost's pipe to verbatim.exe", %outpost, us, "pong");
                }
                self.handle(OwnerEvent::Pong {
                    outpost,
                    abandoned: parked_count,
                });
            }
            Some("TargetExited") => {
                if let Some(pid) = self.pid_of(outpost) {
                    self.target_exited(pid, "target exited");
                }
            }
            Some("Ready") => {
                self.hand_on(Frame::new(pid, outpost, body.into_owned(), read_at_us));
                self.on_ready(outpost);
            }
            // Written by a spare before it read its `Assign`, and read
            // since: it was given already.
            Some("SpareReady") => {}
            Some("Fault") => {
                // A given spare's fault before its `Ready`, such as its
                // hook failing, is kept for the log of its failure.
                if let Some(record) = self
                    .records
                    .get_mut(&pid)
                    .filter(|record| record.spare.is_some() && !record.ready)
                    && let OutpostToSupervisor::Fault { detail } = decode_message(body.bytes())?
                {
                    record.fault = Some(detail);
                }
                self.hand_on(Frame::new(pid, outpost, body.into_owned(), read_at_us));
            }
            _ => self.hand_on(Frame::new(pid, outpost, body.into_owned(), read_at_us)),
        }
        Ok(())
    }

    /// A frame read from the pipe of the spare `outpost`, before it was
    /// given: its `SpareReady`, its pongs, and its faults. Dropped once the
    /// spare has ended.
    fn spare_frame(&mut self, outpost: OutpostId, body: &FrameBody<'_>) -> io::Result<()> {
        let Some(spare) = self.spare_children.get_mut(&outpost) else {
            return Ok(());
        };
        match message_kind(body.bytes()) {
            Some("Pong") => spare.last_pong_at = Instant::now(),
            Some("SpareReady") => {
                let OutpostToSupervisor::SpareReady { outpost_pid } = decode_message(body.bytes())?
                else {
                    return Err(io::Error::other("a spare's ready that is not one"));
                };
                if let Some(waiting) = self.spares.ready(outpost) {
                    tracing::info!(%outpost, %outpost_pid, waiting, "spare outpost ready");
                    self.outbox.send(OutpostMessage::SpareReady {
                        spare: outpost,
                        outpost_pid,
                        waiting,
                    });
                }
            }
            Some("Fault") => {
                if let OutpostToSupervisor::Fault { detail } = decode_message(body.bytes())? {
                    tracing::warn!(%outpost, detail, "spare outpost fault");
                    spare.fault = Some(detail);
                }
            }
            kind => tracing::debug!(%outpost, ?kind, "a spare outpost's message dropped"),
        }
        Ok(())
    }

    /// Hands an outpost's frame on to the app. One the app no longer takes
    /// is dropped: it is exiting.
    fn hand_on(&mut self, frame: Frame) {
        self.outbox.send(OutpostMessage::Frame(frame));
    }

    /// A message read from the listener `outpost`'s pipe at `received`:
    /// focus facts, pongs, and `Ready` are handled as the owner's events;
    /// faults are logged.
    fn listener_message(
        &mut self,
        outpost: OutpostId,
        received: Received,
        message: OutpostToSupervisor,
    ) {
        let event = match message {
            OutpostToSupervisor::FocusFact {
                trace_id,
                observed_at_ms,
                timing,
                fact,
                win_event,
            } => OwnerEvent::Fact {
                trace_id,
                observed_at_ms,
                timing,
                fact,
                win_event,
                received,
            },
            OutpostToSupervisor::ForwardingEnded {
                pid,
                outpost,
                through,
            } => OwnerEvent::ForwardingEnded {
                pid,
                outpost,
                through,
            },
            OutpostToSupervisor::Pong { written_at_us, .. } => {
                if let Some(us) = hop_us(written_at_us, received.at_us()) {
                    tracing::trace!(target: "verbatim::stage", stage = "listener's pipe to verbatim.exe", us, "pong");
                }
                OwnerEvent::Pong {
                    outpost,
                    abandoned: 0,
                }
            }
            OutpostToSupervisor::MenuOrSwitchEnded {
                ended_at_ms,
                timing,
            } => {
                tracing::trace!(
                    target: "verbatim::stage",
                    handling_us = hop_us(timing.observed_at_us, timing.listener_handled_at_us),
                    queue_us = hop_us(timing.listener_handled_at_us, timing.listener_written_at_us),
                    pipe_us = hop_us(timing.listener_written_at_us, received.at_us()),
                    "a menu or switcher end: listener handling, listener queue, listener's pipe to verbatim.exe"
                );
                OwnerEvent::MenuOrSwitchEnded { ended_at_ms }
            }
            OutpostToSupervisor::Ready { .. } => OwnerEvent::Ready(outpost),
            OutpostToSupervisor::IdleAnswer { round } => {
                self.listener_idle(outpost, round);
                return;
            }
            OutpostToSupervisor::Fault { detail } => {
                tracing::warn!(detail, "focus listener fault");
                return;
            }
            _ => return, // The listener sends nothing else.
        };
        self.handle(event);
    }

    fn handle(&mut self, event: OwnerEvent) {
        match event {
            OwnerEvent::Shutdown(reply) => self.shut_down_all(reply),
            OwnerEvent::EnsureSpawned(pid) => {
                if self.shutting_down.is_none() && !self.records.contains_key(&pid) {
                    let _ = self.start_outpost(pid, None);
                }
            }
            OwnerEvent::FocusQueued {
                trace_id,
                observed_at_ms,
                superseded,
            } => {
                let notice = || {
                    Outgoing::HandOver(SupervisorToOutpost::FocusQueued {
                        trace_id,
                        observed_at_ms,
                        sent_at_us: 0,
                    })
                };
                // A listener still launching has accepted no focus to
                // forget.
                if let Some(child) = self.listener.as_ref().and_then(|l| l.child.as_ref())
                    && let Err(error) = child.writer.push(notice())
                {
                    tracing::warn!(%error, "a queued focus could not be told to the listener");
                }
                // An outpost that has ended, or been replaced, holds no
                // focus to forget.
                if let Some(outpost) = superseded
                    && let Some(child) = self
                        .record_of(outpost)
                        .and_then(|record| record.child.as_ref())
                    && let Err(error) = child.writer.push(notice())
                {
                    tracing::warn!(%error, %outpost, "a queued focus could not be told to an outpost");
                }
            }
            OwnerEvent::Views { attention, holding } => {
                if attention != self.attention {
                    let now = Instant::now();
                    for pid in [self.attention, attention].into_iter().flatten() {
                        if let Some(record) = self.records.get_mut(&pid) {
                            record.last_attention_at = now;
                        }
                    }
                    self.attention = attention;
                }
                self.holding = holding;
            }
            OwnerEvent::Fact { .. } if self.shutting_down.is_some() => {}
            OwnerEvent::Fact {
                trace_id,
                observed_at_ms,
                timing,
                fact,
                win_event,
                received,
            } => self.route_fact(
                HeldFact {
                    trace_id,
                    observed_at_ms,
                    timing,
                    fact: fact.fact,
                    win_event,
                    received,
                },
                fact.pid,
            ),
            OwnerEvent::ForwardingEnded {
                pid,
                outpost,
                through,
            } => self.forwarding_ended(pid, outpost, through),
            OwnerEvent::MenuOrSwitchEnded { ended_at_ms } => {
                self.outbox
                    .send(OutpostMessage::MenuOrSwitchEnded { ended_at_ms });
            }
            OwnerEvent::Ready(outpost) => self.on_ready(outpost),
            OwnerEvent::Pong { outpost, abandoned } => {
                if let Some(record) = self.record_of(outpost) {
                    record.last_pong_at = Instant::now();
                    record.abandoned = abandoned;
                } else if let Some(listener) = self
                    .listener
                    .as_mut()
                    .filter(|listener| listener.outpost == outpost)
                {
                    listener.last_pong_at = Instant::now();
                }
            }
            OwnerEvent::Unreadable(outpost) => self.on_pipe_closed(outpost),
            OwnerEvent::Launched { outpost, result } => self.on_launched(outpost, result),
            OwnerEvent::Retired(ending) => {
                self.retiring = self.retiring.saturating_sub(1);
                self.summary.count(ending);
            }
            OwnerEvent::HangChecked { outpost, hung } => self.on_hang_checked(outpost, hung),
            OwnerEvent::IdleRound(round) => self.idle_round(round),
        }
    }

    /// Starts the idle barrier's round `round` (section 6.7): the listener
    /// is asked behind every command sent to it before, or, while it is
    /// launching, as soon as it can be; with no listener, the outposts are
    /// asked at once.
    fn idle_round(&mut self, round: u64) {
        let exiting = self.shutting_down.is_some();
        let listener = self.listener.as_mut().filter(|_| !exiting);
        let Some(listener) = listener else {
            self.query_outposts(round);
            return;
        };
        let query = SupervisorToOutpost::IdleQuery { round };
        let reached = if let Some(child) = &listener.child {
            let pushed = child.writer.push(Outgoing::IdleQuery(query));
            if let Err(error) = &pushed {
                tracing::warn!(%error, "the idle query could not reach the listener");
            }
            pushed.is_ok()
        } else {
            listener.pending.push(query);
            true
        };
        self.idle = Some(IdleStage::Listener(round));
        if !reached {
            self.listener_gone();
        }
    }

    /// The listener `listener` answered round `round`, behind every fact it
    /// sent before, each routed in the pass that read it: the outposts are
    /// asked now, each behind them.
    fn listener_idle(&mut self, listener: OutpostId, round: u64) {
        let current = self
            .listener
            .as_ref()
            .is_some_and(|current| current.outpost == listener);
        if current && self.idle == Some(IdleStage::Listener(round)) {
            self.query_outposts(round);
        }
    }

    /// The listener has gone, its pipe closed or its launch failed, or it
    /// is replaced: one that ends during a round counts as having answered.
    fn listener_gone(&mut self) {
        if let Some(IdleStage::Listener(round)) = self.idle {
            self.query_outposts(round);
        }
    }

    /// Asks every outpost, ready or starting, for round `round`, in this
    /// one pass, behind every fact relayed to it before, and tells the app
    /// which first ([`OutpostMessage::IdleQueried`]). One still starting is
    /// asked behind its held facts once it is ready. While an outpost is
    /// still being launched, the app may not have heard it started, so the
    /// outposts are asked once every launch has reported. An application
    /// whose failed outpost's replacement waits for its backoff has no
    /// incarnation to ask.
    fn query_outposts(&mut self, round: u64) {
        if self.shutting_down.is_none()
            && self
                .records
                .values()
                .any(|record| record.child.is_none() && record.backoff.due().is_none())
        {
            self.idle = Some(IdleStage::Launches(round));
            return;
        }
        self.idle = None;
        let outposts = self
            .records
            .values()
            .filter(|record| record.child.is_some())
            .map(|record| record.outpost)
            .collect();
        self.outbox
            .send(OutpostMessage::IdleQueried { round, outposts });
        for record in self.records.values_mut() {
            match record.child.as_ref().filter(|_| record.ready) {
                Some(child) => ask_idle(&child.writer, round),
                None if record.backoff.due().is_none() => record.idle_query = Some(round),
                None => {}
            }
        }
    }

    /// The record of the outpost incarnation `outpost`, if it is current.
    fn record_of(&mut self, outpost: OutpostId) -> Option<&mut Record> {
        self.records
            .values_mut()
            .find(|record| record.outpost == outpost)
    }

    /// The application `outpost` watches, if it is current.
    fn pid_of(&self, outpost: OutpostId) -> Option<Pid> {
        self.records
            .iter()
            .find(|(_, record)| record.outpost == outpost)
            .map(|(pid, _)| *pid)
    }

    fn next_outpost_id(&mut self) -> OutpostId {
        self.generation += 1;
        OutpostId(self.generation)
    }

    /// Records `pid` as starting, once its process is held
    /// ([`hold_target`](Self::hold_target)), and gives it the oldest spare
    /// outpost, or, with none, hands its launch to a helper thread
    /// ([`incarnate`](Self::incarnate)); says whether it did. An
    /// application whose process has exited, or cannot be held, gets no
    /// outpost, and the app is told ([`OutpostMessage::NotWatched`]), so
    /// nothing waits for one.
    fn start_outpost(&mut self, pid: Pid, first: Option<HeldFact>) -> bool {
        if self.ignored.contains(pid) {
            tracing::debug!(%pid, "no outpost is started for a process Verbatim ignores");
            self.outbox
                .send(OutpostMessage::NotWatched { target_pid: pid });
            return false;
        }
        if let Err(not_held) = self.hold_target(pid) {
            tracing::info!(%pid, %not_held, "no outpost is started for an application that is not running");
            // The listener's reading thread for it reads without a lock:
            // no outpost will call into it.
            let created = match not_held {
                process::NotHeld::Exited(created) => created,
                process::NotHeld::Denied(_) => None,
            };
            self.tell_listener(SupervisorToOutpost::AppLock {
                pid,
                created,
                lock: None,
            });
            self.release_target(pid);
            self.not_watched(pid);
            return false;
        }
        let now = Instant::now();
        let mut held = HeldFacts::default();
        if let Some(fact) = first {
            held.hold(fact);
        }
        self.records.insert(
            pid,
            Record {
                // Set by `incarnate` below.
                outpost: OutpostId(0),
                child: None,
                spare: None,
                ready: false,
                held,
                last_pong_at: now,
                abandoned: 0,
                hang_check: false,
                last_attention_at: now,
                idle_query: None,
                fault: None,
                since: now,
                backoff: Backoff::default(),
                unready: None,
            },
        );
        self.incarnate(pid);
        true
    }

    /// Gives `pid`'s record, starting or after a failure, a new
    /// incarnation ([`next_incarnation`](Self::next_incarnation)) and
    /// starts it ([`start_incarnation`](Self::start_incarnation)). When
    /// the incarnation before failed before its `Ready`, the app hears
    /// first that this one takes its place
    /// ([`OutpostMessage::Reincarnated`]), then the failed one's end if it
    /// heard it started.
    fn incarnate(&mut self, pid: Pid) {
        let unready = self
            .records
            .get_mut(&pid)
            .and_then(|record| record.unready.take());
        let Some(next) = self.next_incarnation(pid) else {
            return;
        };
        if let Some(failed) = unready {
            self.outbox.send(OutpostMessage::Reincarnated {
                outpost: failed.outpost,
                next,
                target_pid: pid,
            });
            if failed.started {
                self.outbox.send(OutpostMessage::Ended {
                    outpost: failed.outpost,
                    target_pid: pid,
                    reason: failed.reason,
                });
            }
        }
        self.start_incarnation(pid);
    }

    /// Gives `pid`'s record its next incarnation without starting it: the
    /// oldest spare waiting, ready or not, whose replacement starts at
    /// once, or, with no spare, a new outpost to launch for it. Returns the
    /// incarnation.
    fn next_incarnation(&mut self, pid: Pid) -> Option<OutpostId> {
        if !self.records.contains_key(&pid) {
            return None;
        }
        let now = Instant::now();
        let spare = self.spares.take();
        let (outpost, child) = match spare {
            Some(spare) => (
                spare,
                self.spare_children
                    .remove(&spare)
                    .and_then(|spare| spare.child),
            ),
            None => (self.next_outpost_id(), None),
        };
        let record = self.records.get_mut(&pid)?;
        record.outpost = outpost;
        record.spare = spare.map(|_| {
            if child.is_some() {
                Assigned::Sent
            } else {
                Assigned::Pending
            }
        });
        record.child = child;
        record.ready = false;
        record.last_pong_at = now;
        record.abandoned = 0;
        record.hang_check = false;
        record.fault = None;
        record.since = now;
        if spare.is_some() {
            self.replenish_spares();
        }
        Some(outpost)
    }

    /// Starts the incarnation [`next_incarnation`](Self::next_incarnation)
    /// gave `pid`'s record: a spare is sent `Assign` and the app `Started`
    /// at once, or once its launch has reported; an outpost for it is
    /// launched.
    fn start_incarnation(&mut self, pid: Pid) {
        let Some(record) = self.records.get(&pid) else {
            return;
        };
        match record.spare {
            Some(Assigned::Sent) => self.assign(pid),
            Some(Assigned::Pending) => {}
            None => self.launch(record.outpost, Role::Outpost(pid)),
        }
    }

    /// Sends `pid`'s spare its `Assign`, ahead of everything the app will
    /// send it, reads its pipe as the application's from now on, and tells
    /// the app the incarnation started.
    fn assign(&mut self, pid: Pid) {
        let Some(record) = self.records.get(&pid) else {
            return;
        };
        let outpost = record.outpost;
        // The application's lock, duplicated into the spare as `Assign` is
        // written: the spare sets it before it installs its hook.
        let lock = record
            .child
            .as_ref()
            .and_then(|child| self.grant_lock(pid, &child.process.contained.process));
        if let Some(child) = &record.child
            && let Err(error) = child
                .writer
                .push(Outgoing::HandOver(SupervisorToOutpost::Assign {
                    target_pid: pid,
                    lock,
                }))
        {
            // Its pipe's end follows, and is the spare's failure.
            tracing::warn!(%error, %outpost, %pid, "a spare could not be sent its application");
        }
        self.relay.set_source(outpost, Source::Outpost(pid));
        tracing::info!(%outpost, %pid, "a spare outpost is given to the application");
        self.outbox.send(OutpostMessage::Started {
            outpost,
            target_pid: pid,
        });
    }

    /// Starts keeping spare outposts, once: Core's own outpost is ready, or
    /// has ended without becoming ready.
    fn keep_spares(&mut self) {
        if self.shutting_down.is_none() && self.spares.keep() {
            tracing::info!(count = SPARES, "spare outposts are kept from now on");
            self.replenish_spares();
        }
    }

    /// Launches spares until [`SPARES`] wait, unless a spare's failure's
    /// backoff runs.
    fn replenish_spares(&mut self) {
        if self.shutting_down.is_some() {
            return;
        }
        let now = Instant::now();
        for _ in 0..self.spares.wanted(SPARES, now) {
            let spare = self.next_outpost_id();
            self.spares.add(spare);
            self.spare_children.insert(
                spare,
                SpareChild {
                    child: None,
                    last_pong_at: now,
                    fault: None,
                    started_at: now,
                },
            );
            self.launch(spare, Role::Spare(spare));
        }
    }

    /// Logs the failure, for `why`, of a waiting spare that had been up for
    /// `up` as an error, the evidence to find its cause from, and replaces
    /// it, at once or after its backoff: spares are always replaced.
    fn spare_failed(&mut self, why: &str, up: Duration) {
        let wait = self.spares.failed(up, Instant::now(), BACKOFF);
        tracing::error!(
            why,
            backoff_ms = wait.as_millis(),
            "a spare outpost failed; it is replaced"
        );
        self.replenish_spares();
    }

    /// The earliest time a failure's backoff ends, an application's or the
    /// spares'.
    fn next_due(&self) -> Option<Instant> {
        self.records
            .values()
            .filter_map(|record| record.backoff.due())
            .chain(self.spares.due())
            .min()
    }

    /// Ends every backoff due at `now`: each application waiting is given
    /// its next incarnation, and the spares are replenished.
    fn start_due(&mut self, now: Instant) {
        if self.shutting_down.is_some() {
            return;
        }
        let due: Vec<Pid> = self
            .records
            .iter_mut()
            .filter(|(_, record)| record.backoff.due().is_some())
            .filter_map(|(pid, record)| record.backoff.ended(now).then_some(*pid))
            .collect();
        for pid in due {
            tracing::info!(%pid, "a failed outpost's backoff has ended; its application is given the next");
            self.incarnate(pid);
        }
        if self.spares.due().is_some_and(|due| due <= now) {
            self.replenish_spares();
        }
    }

    /// `pid`'s outpost failed, for `why`, as `reason` says: it crashed or
    /// exited, its hook failed, its launch failed, it stopped answering
    /// pings, or it piled up abandoned workers, whether a given spare or an
    /// outpost started for the application, before or after its `Ready`.
    /// It is ended, unless its application has exited, which ends it as
    /// the application's exit. Otherwise the record keeps its held facts,
    /// and the idle query held for it, for the next incarnation, given at
    /// once for the application's first failure and after the backoff for
    /// each further one ([`Backoff`]); facts arriving meanwhile are held
    /// too. One that was ready is ended for the app now, which asks the
    /// next for the focus where it needs to; one that failed before its
    /// `Ready` lost nothing, and the app hears it replaced
    /// ([`OutpostMessage::Reincarnated`]) as the next is given.
    fn outpost_failed(&mut self, pid: Pid, why: &str, reason: EndReason) {
        if self.has_exited(pid) {
            self.target_exited(pid, why);
            return;
        }
        if self.shutting_down.is_some() {
            self.end_quietly(pid, why);
            return;
        }
        let Some(record) = self.records.get_mut(&pid) else {
            return;
        };
        let failed = record.outpost;
        let ready = record.ready;
        let child = record.child.take();
        // What it reported before it failed is the evidence of why.
        let why = match record.fault.take() {
            Some(fault) => format!("{why}, after its fault: {fault}"),
            None => why.to_owned(),
        };
        let wait = record
            .backoff
            .failed(record.since.elapsed(), Instant::now(), BACKOFF);
        if !ready {
            record.unready = Some(Unready {
                outpost: failed,
                reason,
                started: child.is_some(),
            });
        }
        record.outpost = OutpostId(0);
        record.spare = None;
        record.ready = false;
        record.abandoned = 0;
        record.hang_check = false;
        self.forget_writer(failed);
        if let Some(child) = child {
            self.retire_child(child, format!("outpost {failed} for {pid}"), why.clone());
        }
        // A failure before `Ready`, such as a hook that fails, is an error,
        // whose sound a user hears, as a spare's failure is; an outpost
        // that crashes or is stuck after it is a warning, as it was before
        // every failure took this path.
        if ready {
            tracing::warn!(
                %failed,
                %pid,
                why,
                backoff_ms = wait.as_millis(),
                "a ready outpost failed; its application is given the next"
            );
        } else {
            tracing::error!(
                %failed,
                %pid,
                why,
                backoff_ms = wait.as_millis(),
                "an outpost failed before it was ready; its application is given the next"
            );
        }
        if ready {
            // Its hook goes with it: the listener forwards the
            // application's MSAA events again until the next is ready.
            self.tell_listener(SupervisorToOutpost::HookRemoved { pid, sent_at_us: 0 });
            self.outbox.send(OutpostMessage::Ended {
                outpost: failed,
                target_pid: pid,
                reason,
            });
        } else if pid == self.own_pid {
            self.keep_spares();
        }
        if wait.is_zero() {
            self.incarnate(pid);
        }
    }

    /// Removes `outpost`'s writer from those the app can reach.
    fn forget_writer(&self, outpost: OutpostId) {
        self.writers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&outpost);
    }

    /// Tells the app that no outpost will watch `pid` for now
    /// ([`OutpostMessage::NotWatched`]), so nothing waits for one.
    fn not_watched(&mut self, pid: Pid) {
        self.outbox
            .send(OutpostMessage::NotWatched { target_pid: pid });
    }

    /// Holds `pid`'s process, unless it is held already: the pid then names
    /// that process for as long as it is held. Its lock is made with it, and
    /// the listener is sent it ([`SupervisorToOutpost::AppLock`]). Fails
    /// when the process has exited, the one held included, or cannot be
    /// opened. A process that has exited is let go, and its lock closed.
    fn hold_target(&mut self, pid: Pid) -> Result<(), process::NotHeld> {
        match self.apps.get(&pid) {
            Some(app) if app.target.has_exited() => {
                let created = app.created;
                self.forget_app(pid);
                Err(process::NotHeld::Exited(Some(created)))
            }
            Some(_) => Ok(()),
            None => {
                let target = Target::open(pid)?;
                let created = target
                    .created()
                    .map_err(|error| process::NotHeld::Denied(windows::core::Error::from(error)))?;
                let lock = verbatim_uia::lock::new_mutex()
                    .inspect_err(|error| {
                        tracing::error!(%pid, %error, "an application's lock could not be made; UIA calls into it run unguarded");
                    })
                    .ok()
                    .map(Arc::new);
                self.apps.insert(
                    pid,
                    App {
                        target,
                        created,
                        lock,
                    },
                );
                self.send_lock_to_listener(pid);
                Ok(())
            }
        }
    }

    /// Whether `pid`'s process has exited, or is not held.
    fn has_exited(&self, pid: Pid) -> bool {
        self.apps
            .get(&pid)
            .is_none_or(|app| app.target.has_exited())
    }

    /// Lets `pid`'s process go once nothing is kept for it: it has no
    /// record and it has exited. An application whose outpost retires keeps
    /// its process held, and its lock, until it exits, which the sweep finds.
    fn release_target(&mut self, pid: Pid) {
        if !self.records.contains_key(&pid) && self.has_exited(pid) {
            self.forget_app(pid);
        }
    }

    /// Forgets `pid`'s process, whose exit the supervisor has learned:
    /// closes its handle and its lock, and tells the listener
    /// ([`SupervisorToOutpost::AppLockClosed`]), whose reading threads keep
    /// their own copies until they let them go.
    fn forget_app(&mut self, pid: Pid) {
        if let Some(app) = self.apps.remove(&pid) {
            tracing::debug!(%pid, "an application's process exited; its lock is closed");
            if app.lock.is_some() {
                self.tell_listener(SupervisorToOutpost::AppLockClosed {
                    pid,
                    created: app.created,
                });
            }
        }
    }

    /// `pid`'s lock as `process` is given it: a handle to the mutex
    /// duplicated into that process with only the rights to wait and
    /// release, and the application's creation time. `None` when the
    /// application has no lock or the duplication fails, when that
    /// process's calls run unguarded.
    fn grant_lock(&self, pid: Pid, process: &OwnedHandle) -> Option<LockGrant> {
        let app = self.apps.get(&pid)?;
        let lock = app.lock.as_ref()?;
        match verbatim_uia::lock::duplicate_into(lock, process) {
            Ok(handle) => Some(LockGrant {
                handle: handle as u64,
                created: app.created,
            }),
            Err(error) => {
                tracing::warn!(%pid, %error, "an application's lock could not be given to a process");
                None
            }
        }
    }

    /// Sends the listener `pid`'s lock ([`SupervisorToOutpost::AppLock`]),
    /// duplicated into its process, when it has one running; a listener
    /// still launching is sent every lock once it has reported.
    fn send_lock_to_listener(&self, pid: Pid) {
        let Some(child) = self
            .listener
            .as_ref()
            .and_then(|listener| listener.child.as_ref())
        else {
            return;
        };
        let Some(app) = self.apps.get(&pid) else {
            return;
        };
        let lock = self
            .grant_lock(pid, &child.process.contained.process)
            .map(|grant| grant.handle);
        let command = SupervisorToOutpost::AppLock {
            pid,
            created: Some(app.created),
            lock,
        };
        if let Err(error) = child.writer.push(Outgoing::HandOver(command)) {
            tracing::warn!(%error, %pid, "an application's lock could not be sent to the listener");
        }
    }

    /// `pid`'s application has exited: its outpost is ended, never replaced,
    /// and everything kept for it is forgotten.
    fn target_exited(&mut self, pid: Pid, why: &str) {
        self.end_because(pid, EndReason::TargetExited, why);
        self.release_target(pid);
    }

    /// Records a new listener as starting and hands its launch to a helper.
    /// It is told on its command line which applications' outposts have
    /// their own hooks in now, and of every change after this as soon as it
    /// has a writer ([`tell_listener`](Self::tell_listener)).
    fn start_listener(&mut self, replacement: bool) {
        let outpost = self.next_outpost_id();
        self.listener = Some(ListenerRecord {
            outpost,
            child: None,
            last_pong_at: Instant::now(),
            replacement,
            pending: Vec::new(),
        });
        self.launch(outpost, Role::Listener);
    }

    /// The applications whose outposts have their own `WinEvent` hooks in:
    /// those that have said they are ready, each with its outpost.
    fn hooks_in(&self) -> Vec<(Pid, OutpostId)> {
        self.records
            .iter()
            .filter(|(_, record)| record.ready)
            .map(|(pid, record)| (*pid, record.outpost))
            .collect()
    }

    /// Sends the listener a message of the MSAA hand-over
    /// (`docs/design/focus-pipeline.md`, section 12.4), or holds it, in
    /// order, while the listener's launch is at work. With no listener, it
    /// is not needed: the next one is told the applications whose outposts
    /// have their hooks in on its command line.
    fn tell_listener(&mut self, command: SupervisorToOutpost) {
        let Some(listener) = self.listener.as_mut() else {
            return;
        };
        match &listener.child {
            Some(child) => {
                if let Err(error) = child.writer.push(Outgoing::HandOver(command)) {
                    tracing::warn!(%error, "the hand-over could not be told to the listener");
                }
            }
            None => listener.pending.push(command),
        }
    }

    /// Relays the listener's "forwarding ended" to the outpost it was for,
    /// behind every fact the listener sent before it, if that outpost still
    /// watches `pid`; one for an earlier incarnation is dropped.
    fn forwarding_ended(&self, pid: Pid, outpost: OutpostId, through: u32) {
        let Some(child) = self
            .records
            .get(&pid)
            .filter(|record| record.outpost == outpost && record.ready)
            .and_then(|record| record.child.as_ref())
        else {
            tracing::debug!(%pid, %outpost, "forwarding ended for an outpost that has gone");
            return;
        };
        let command = SupervisorToOutpost::ForwardingEnded { through };
        if let Err(error) = child.writer.push(Outgoing::HandOver(command)) {
            tracing::warn!(%error, %pid, "forwarding ended could not be relayed");
        }
    }

    /// Launches a child on a helper thread. The helper makes the child's
    /// writer on the relay's port and, for an outpost, registers it; then
    /// it reports [`OwnerEvent::Launched`]. The relay sends `Started` to the
    /// app as it takes the launch, through the same [`Outbox`] as the end
    /// of the incarnation before, and only then reads the child's pipe. So
    /// the app hears of an incarnation after the end of the one before and
    /// before any of its messages, a query can reach the outpost as soon as
    /// the app has heard, and the owner always learns of the launch before
    /// the child's `Ready` or the end of its pipe.
    fn launch(&self, outpost: OutpostId, role: Role) {
        let ignored = self.ignored.list();
        let hooks_in = match role {
            Role::Listener => crate::listener::format_hooks_in(&self.hooks_in()),
            Role::Outpost(_) | Role::Spare(_) => String::new(),
        };
        // An outpost started for its application inherits the application's
        // lock: a copy with only the rights to wait and release, made here
        // and closed once the launch returns (section 20.4).
        let lock = match role {
            Role::Outpost(pid) => self.apps.get(&pid).and_then(|app| {
                let lock = app.lock.as_ref()?;
                verbatim_uia::lock::restricted_copy(lock)
                    .inspect_err(|error| {
                        tracing::warn!(%pid, %error, "an application's lock could not be copied for its outpost");
                    })
                    .ok()
                    .map(|copy| (copy, app.created))
            }),
            Role::Spare(_) | Role::Listener => None,
        };
        let exe_path = self.exe_path.clone();
        let options = self.options;
        let own_tx = self.own_tx.clone();
        let writers = self.writers.clone();
        let commands = self.relay.commands().clone();
        let spawned = thread::Builder::new()
            .name("verbatim-launch".to_owned())
            .spawn(move || {
                let result = launch_child(
                    &exe_path,
                    (options, &ignored, &hooks_in),
                    (outpost, role, lock),
                    (&writers, &commands),
                );
                let _ = own_tx.send(OwnerEvent::Launched { outpost, result });
            });
        if let Err(error) = spawned {
            let _ = self.own_tx.send(OwnerEvent::Launched {
                outpost,
                result: Err(error),
            });
        }
    }

    fn on_launched(&mut self, outpost: OutpostId, result: io::Result<Launch>) {
        if let Ok(Launch {
            source: Source::Outpost(pid),
            ..
        }) = &result
        {
            self.outbox.send(OutpostMessage::Started {
                outpost,
                target_pid: *pid,
            });
        }
        // The child's pipe to Core is read from now on, whoever ends up
        // ending the child, so its end is always seen. A spare given while
        // it was launching is read as its application's.
        let given = self.pid_of(outpost);
        let mut unread = None;
        let result = result.map(|launch| {
            let source = match (launch.source, given) {
                (Source::Spare, Some(pid)) => Source::Outpost(pid),
                (source, _) => source,
            };
            if let Err(error) = self.relay.add_reader(outpost, source, launch.from_child) {
                unread = Some(error);
            }
            launch.child
        });
        self.launched(outpost, result);
        if let Some(error) = unread {
            // No reading, no messages: end it as though its pipe had closed.
            tracing::error!(%error, %outpost, "a child's pipe to Core cannot be read");
            self.on_pipe_closed(outpost);
        }
        // An idle round waiting for the launches asks the outposts once
        // none is left.
        if let Some(IdleStage::Launches(round)) = self.idle {
            self.query_outposts(round);
        }
    }

    fn launched(&mut self, outpost: OutpostId, result: io::Result<Child>) {
        if let Some(listener) = self
            .listener
            .as_mut()
            .filter(|listener| listener.outpost == outpost)
        {
            match result {
                Ok(child) => {
                    let pending = std::mem::take(&mut listener.pending);
                    listener.child = Some(child);
                    listener.last_pong_at = Instant::now();
                    // Every live lock first, Core's own among them, ahead of
                    // every other command (section 20.4).
                    let pids: Vec<Pid> = self.apps.keys().copied().collect();
                    for pid in pids {
                        self.send_lock_to_listener(pid);
                    }
                    let Some(child) = self
                        .listener
                        .as_ref()
                        .and_then(|listener| listener.child.as_ref())
                    else {
                        return;
                    };
                    for command in pending {
                        if let Err(error) = child.writer.push(Outgoing::HandOver(command)) {
                            tracing::warn!(%error, "the hand-over could not be told to the listener");
                        }
                    }
                    if self.shutting_down.is_some() {
                        self.end_listener("Verbatim is exiting");
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "failed to launch the focus listener; retrying on the next heartbeat");
                    self.listener = None;
                    self.listener_gone();
                }
            }
            return;
        }
        if self.spare_children.contains_key(&outpost) {
            self.spare_launched(outpost, result);
            return;
        }
        let Some(pid) = self.pid_of(outpost) else {
            // A child nobody wants any more: unreachable while records are
            // only ended once their launch has reported, but if it happens
            // the child must still die.
            if let Ok(child) = result {
                tracing::warn!(%outpost, "launched a child that is no longer wanted");
                self.writers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&outpost);
                self.retire_child(
                    child,
                    format!("outpost {outpost}"),
                    "launched after it was no longer wanted".to_owned(),
                );
            }
            return;
        };
        match result {
            Ok(child) => {
                let mut assign = false;
                if let Some(record) = self.records.get_mut(&pid) {
                    record.child = Some(child);
                    record.last_pong_at = Instant::now();
                    assign = record.spare == Some(Assigned::Pending);
                    if assign {
                        record.spare = Some(Assigned::Sent);
                    }
                }
                if assign {
                    self.assign(pid);
                }
                if self.shutting_down.is_some() {
                    self.end_quietly(pid, "Verbatim is exiting");
                }
            }
            Err(error) if self.shutting_down.is_none() => {
                tracing::error!(%error, %pid, "failed to launch an outpost");
                self.outpost_failed(
                    pid,
                    &format!("its launch failed: {error}"),
                    EndReason::Exited,
                );
            }
            Err(error) => {
                tracing::error!(%error, %pid, "failed to launch an outpost while Verbatim exits");
                self.records.remove(&pid);
                self.release_target(pid);
            }
        }
    }

    /// The launch of the spare `outpost` reported: it waits from now on,
    /// unless Verbatim is exiting, when it is ended; a failed launch is a
    /// spare's failure.
    fn spare_launched(&mut self, outpost: OutpostId, result: io::Result<Child>) {
        match result {
            Ok(child) if self.shutting_down.is_some() => {
                self.spare_children.remove(&outpost);
                self.forget_writer(outpost);
                self.retire_child(
                    child,
                    format!("spare outpost {outpost}"),
                    "Verbatim is exiting".to_owned(),
                );
            }
            Ok(child) => {
                if let Some(spare) = self.spare_children.get_mut(&outpost) {
                    spare.child = Some(child);
                    spare.last_pong_at = Instant::now();
                }
            }
            Err(error) => {
                tracing::error!(%error, %outpost, "failed to launch a spare outpost");
                let up = self
                    .spare_children
                    .remove(&outpost)
                    .map_or(Duration::ZERO, |spare| spare.started_at.elapsed());
                if self.spares.remove(outpost) {
                    self.spare_failed(&format!("spare {outpost}'s launch failed: {error}"), up);
                }
            }
        }
    }

    /// Releases held facts on `Ready`, in arrival order. A listener's
    /// `Ready` is reported to the app, saying whether it is a replacement:
    /// facts were then lost in the gap, so the app asks for the current
    /// focus.
    fn on_ready(&mut self, outpost: OutpostId) {
        if let Some(listener) = self
            .listener
            .as_ref()
            .filter(|listener| listener.outpost == outpost)
        {
            tracing::info!(%outpost, "focus listener ready");
            let replacement = listener.replacement;
            self.outbox
                .send(OutpostMessage::ListenerReady { replacement });
            return;
        }
        let Some(pid) = self.pid_of(outpost) else {
            return;
        };
        let Some(record) = self.records.get_mut(&pid) else {
            return;
        };
        record.ready = true;
        if let Some(held_at) = record.held.first_held_at() {
            // The time the supervisor held the application's first fact
            // for its outpost's start (`docs/design/focus-pipeline.md`,
            // section 19.6).
            tracing::info!(
                %outpost,
                %pid,
                first_fact_held_us = u64::try_from(held_at.elapsed().as_micros()).unwrap_or(u64::MAX),
                "outpost ready, its application's first fact held until now"
            );
        }
        let Some(writer) = record.child.as_ref().map(|child| child.writer.clone()) else {
            return;
        };
        for mut held in record.held.take() {
            // The time it was held for the outpost's start, reported apart
            // from the relay's own (`docs/design/communication.md`, section 10.1).
            held.timing.core_held_us = held.received.elapsed_us();
            deliver(&writer, held);
        }
        if let Some(round) = record.idle_query.take() {
            ask_idle(&writer, round);
        }
        if pid == self.own_pid {
            self.keep_spares();
        }
        // An outpost is ready once its own `WinEvent` hook is in: the
        // listener stops forwarding the application's MSAA events raised
        // from now on, and says so behind the last it forwarded.
        self.tell_listener(SupervisorToOutpost::HookInstalled {
            pid,
            outpost,
            sent_at_us: 0,
        });
    }

    /// Routes a listener fact to its application's outpost: delivered at
    /// once if the outpost is ready, held if it is starting or its failed
    /// outpost's replacement waits for its backoff, and starting one if
    /// there is none. A foreground change to the application starts its
    /// backoff's count again, and ends a backoff that runs: its next
    /// outpost is given now.
    fn route_fact(&mut self, held: HeldFact, pid: Pid) {
        if self.ignored.contains(pid) {
            // The listener drops these already; nothing is routed either way.
            return;
        }
        if matches!(held.fact, DeliveredFact::Foreground { .. })
            && let Some(record) = self.records.get_mut(&pid)
            && record.backoff.foreground()
        {
            tracing::info!(%pid, "its application came to the foreground; a failed outpost's backoff ends");
            self.incarnate(pid);
        }
        match self.records.get_mut(&pid) {
            Some(record) => match record.child.as_ref().filter(|_| record.ready) {
                Some(child) => deliver(&child.writer, held),
                None => record.held.hold(held),
            },
            None => {
                // A selection, or a notification Core would not speak, in
                // an application with no outpost is not worth starting one
                // for.
                if held.fact.may_start_outpost() {
                    let _ = self.start_outpost(pid, Some(held));
                }
            }
        }
    }

    fn on_pipe_closed(&mut self, outpost: OutpostId) {
        if self
            .listener
            .as_ref()
            .is_some_and(|listener| listener.outpost == outpost)
        {
            if self.shutting_down.is_some() {
                self.end_listener("its pipe closed while Verbatim is exiting");
                return;
            }
            tracing::warn!(%outpost, "focus listener exited; replacing it");
            self.end_listener("its pipe closed; it is replaced");
            self.focus_reads_ended();
            self.listener_gone();
            self.start_listener(true);
            return;
        }
        if let Some(spare) = self.spare_children.remove(&outpost) {
            let listed = self.spares.remove(outpost);
            let up = spare.started_at.elapsed();
            self.forget_writer(outpost);
            if let Some(child) = spare.child {
                self.retire_child(
                    child,
                    format!("spare outpost {outpost}"),
                    "its pipe closed".to_owned(),
                );
            }
            if listed && self.shutting_down.is_none() {
                let fault = spare
                    .fault
                    .map(|fault| format!(", after its fault: {fault}"))
                    .unwrap_or_default();
                tracing::warn!(%outpost, fault, "a spare outpost ended while it waited");
                self.spare_failed(&format!("spare {outpost} ended while it waited{fault}"), up);
            }
            return;
        }
        if let Some(pid) = self.pid_of(outpost) {
            if self.shutting_down.is_some() {
                self.end_quietly(pid, "its pipe closed while Verbatim is exiting");
                return;
            }
            self.outpost_failed(pid, "its pipe closed", EndReason::Exited);
        }
        // Otherwise the owner already ended it, and the app already heard.
    }

    /// Ends `pid`'s outpost: removes its record and writer, shuts the
    /// process down cleanly, killing it only if it does not exit in time
    /// ([`retire`], on a thread of its own), and tells the app at once.
    fn end(&mut self, pid: Pid, reason: EndReason) {
        self.end_because(pid, reason, &reason.to_string());
    }

    /// [`end`](Self::end), saying `why` in the log of the process's end.
    /// An application whose failed outpost's replacement waits for its
    /// backoff has no incarnation to end, but an incarnation that failed
    /// before its `Ready`, and whose end the app has not heard yet, ends
    /// now.
    fn end_because(&mut self, pid: Pid, reason: EndReason, why: &str) {
        let Some((outpost, unready)) = self.take_record(pid, why) else {
            return;
        };
        let unready = unready
            .filter(|unready| unready.started)
            .map(|unready| unready.outpost);
        for outpost in [Some(outpost).filter(|outpost| outpost.0 != 0), unready]
            .into_iter()
            .flatten()
        {
            tracing::info!(%outpost, %pid, %reason, "outpost ended");
            self.outbox.send(OutpostMessage::Ended {
                outpost,
                target_pid: pid,
                reason,
            });
        }
    }

    /// Ends `pid`'s outpost as [`end`](Self::end) does, without telling the
    /// app: Verbatim is exiting.
    fn end_quietly(&mut self, pid: Pid, why: &str) {
        let _ = self.take_record(pid, why);
    }

    /// Removes `pid`'s record and writer and retires its process, if it
    /// has one yet, for `why`. Returns the record's outpost id, `OutpostId(0)`
    /// while a failure's backoff runs, and the incarnation that failed
    /// before its `Ready` whose replacement that backoff holds.
    fn take_record(&mut self, pid: Pid, why: &str) -> Option<(OutpostId, Option<Unready>)> {
        let record = self.records.remove(&pid)?;
        if pid == self.own_pid && !record.ready {
            self.keep_spares();
        }
        if record.ready {
            // Its hook goes with it: the listener forwards the
            // application's MSAA events again, to start the next outpost.
            self.tell_listener(SupervisorToOutpost::HookRemoved { pid, sent_at_us: 0 });
        }
        self.writers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&record.outpost);
        if let Some(child) = record.child {
            self.retire_child(
                child,
                format!("outpost {} for {pid}", record.outpost),
                why.to_owned(),
            );
        }
        Some((record.outpost, record.unready))
    }

    /// Ends `child` cleanly on a thread of its own ([`retire`]), which
    /// reports how it ended ([`OwnerEvent::Retired`]).
    fn retire_child(&mut self, child: Child, label: String, why: String) {
        let limit = self.shutdown_limit;
        let own_tx = self.own_tx.clone();
        let spawned = thread::Builder::new()
            .name("verbatim-retire".to_owned())
            .spawn(move || {
                // A panic while ending the child leaves it to its job
                // handle, closed as the thread unwinds: it was killed, which
                // the report says as it drops.
                let mut report = Retirement {
                    own_tx,
                    ending: Ending::KilledOnFailure,
                };
                report.ending = retire(child, &label, &why, limit);
            });
        match spawned {
            Ok(_) => self.retiring += 1,
            // The child went with the closure; dropping it closed its job
            // handle, which killed it.
            Err(error) => {
                tracing::error!(%error, "no thread to end a child cleanly; it was killed");
                self.summary.count(Ending::KilledOnFailure);
            }
        }
    }

    /// Verbatim is exiting: no child is started from now on, and every
    /// child is shut down. Children still being launched are shut down as
    /// their launch reports back.
    fn shut_down_all(&mut self, reply: Sender<ShutdownSummary>) {
        tracing::info!("Verbatim is exiting: shutting down every outpost and the focus listener");
        self.shutting_down = Some(reply);
        // Those still launching are ended as their launch reports; those
        // waiting for a failure's backoff have nothing to wait for.
        let launched: Vec<Pid> = self
            .records
            .iter()
            .filter(|(_, record)| record.child.is_some() || record.backoff.due().is_some())
            .map(|(pid, _)| *pid)
            .collect();
        for pid in launched {
            self.end_quietly(pid, "Verbatim is exiting");
        }
        // Spares still launching are ended as their launch reports.
        let spares: Vec<OutpostId> = self
            .spare_children
            .iter()
            .filter(|(_, spare)| spare.child.is_some())
            .map(|(outpost, _)| *outpost)
            .collect();
        for outpost in spares {
            if let Some(child) = self
                .spare_children
                .remove(&outpost)
                .and_then(|spare| spare.child)
            {
                self.forget_writer(outpost);
                self.retire_child(
                    child,
                    format!("spare outpost {outpost}"),
                    "Verbatim is exiting".to_owned(),
                );
            }
        }
        if self
            .listener
            .as_ref()
            .is_some_and(|listener| listener.child.is_some())
        {
            self.end_listener("Verbatim is exiting");
        }
    }

    /// Once Verbatim is exiting, every child has been ended and has
    /// finished ending, and every pipe has finished, sends the summary and
    /// says the owner is done. Until then the relay keeps running, since a
    /// child's ending needs its shutdown message written.
    fn finish_shutdown(&mut self) -> bool {
        if self.shutting_down.is_none()
            || !self.records.is_empty()
            || !self.spare_children.is_empty()
            || self.listener.is_some()
            || self.retiring > 0
            || !self.relay.is_idle()
        {
            return false;
        }
        let summary = self.summary;
        tracing::info!(
            clean = summary.clean,
            exited = summary.exited,
            killed = summary.killed,
            killed_on_failure = summary.killed_on_failure,
            "every outpost and the focus listener has ended"
        );
        if let Some(reply) = self.shutting_down.take() {
            let _ = reply.send(summary);
        }
        true
    }

    /// Tells every outpost, ready or starting, that the listener whose UIA
    /// focus markers it may be waiting on has ended, so it waits on none of
    /// them any more ([`DeliveredFact::UiaFocusReadsEnded`]). Sent behind
    /// everything relayed from that listener: its pipe has closed, or it is
    /// ended now, and a read its reader still relays after this lands where
    /// no marker waits for it, and is handled where it arrives.
    fn focus_reads_ended(&mut self) {
        for record in self.records.values_mut() {
            let held = HeldFact {
                trace_id: TraceId::mint(),
                observed_at_ms: crate::outpost::now_ms(),
                timing: EventTiming::default(),
                fact: DeliveredFact::UiaFocusReadsEnded,
                win_event: None,
                received: crate::supervisor::writer::Received::now(),
            };
            match record.child.as_ref().filter(|_| record.ready) {
                Some(child) => deliver(&child.writer, held),
                None => record.held.hold(held),
            }
        }
    }

    /// Ends the focus listener, shutting its process down cleanly, for
    /// `why`.
    fn end_listener(&mut self, why: &str) {
        if let Some(listener) = self.listener.take()
            && let Some(child) = listener.child
        {
            self.retire_child(
                child,
                format!("focus listener {}", listener.outpost),
                why.to_owned(),
            );
        }
    }

    /// Pings every running child and ends any that is wedged. Whether an
    /// application with many abandoned workers is hung is checked on a
    /// thread of its own, since it walks the application's windows, and
    /// the outpost is judged on its answer ([`OwnerEvent::HangChecked`]);
    /// until then only its missed pongs count against it.
    fn heartbeat(&mut self) {
        self.relay.commands().forget_finished();
        if self.shutting_down.is_some() {
            return;
        }
        let now = Instant::now();
        if let Some(progress) = self.outbox.sink.progress()
            && let Some(stall) = self.reducer.note(progress, now)
        {
            self.outbox.sink.stalled(stall);
        }
        self.ping_seq += 1;
        let ping = SupervisorToOutpost::Ping {
            seq: self.ping_seq,
            sent_at_us: 0,
        };
        let mut wedged = Vec::new();
        let mut exited = Vec::new();
        let mut hang_checks = Vec::new();
        for (&pid, record) in &self.records {
            let Some(child) = &record.child else { continue };
            // An application whose outpost could not tell it exited.
            if self.has_exited(pid) {
                exited.push(pid);
                continue;
            }
            let check_hang = record.abandoned >= ABANDONED_WORKER_LIMIT;
            if check_hang && !record.hang_check {
                hang_checks.push((pid, record.outpost));
            }
            match wedge_decision(
                record.last_pong_at,
                now,
                PING_INTERVAL,
                MISSED_PONG_THRESHOLD,
                record.abandoned,
                ABANDONED_WORKER_LIMIT,
                // Abandoned workers are judged when the check answers.
                check_hang,
            ) {
                Some(reason) => wedged.push((pid, reason)),
                None => {
                    let _ = child.writer.push(Outgoing::Ping(ping.clone()));
                }
            }
        }
        for (pid, outpost) in hang_checks {
            self.check_hang(pid, outpost);
        }
        for pid in exited {
            self.target_exited(pid, "target exited, seen by the supervisor");
        }
        for (pid, reason) in wedged {
            tracing::warn!(%pid, %reason, "ending a stuck outpost");
            self.outpost_failed(pid, &format!("ended as stuck: {reason}"), EndReason::Killed);
        }
        self.spare_heartbeat(now, &ping);

        match &self.listener {
            None => self.start_listener(true),
            Some(listener) => {
                if let Some(child) = &listener.child {
                    let decision = wedge_decision(
                        listener.last_pong_at,
                        now,
                        PING_INTERVAL,
                        MISSED_PONG_THRESHOLD,
                        0,
                        ABANDONED_WORKER_LIMIT,
                        false,
                    );
                    if decision == Some(WedgeReason::MissedHeartbeats) {
                        tracing::warn!("focus listener stopped answering; replacing it");
                        self.end_listener("it stopped answering pings; it is replaced");
                        self.focus_reads_ended();
                        self.listener_gone();
                        self.start_listener(true);
                    } else {
                        let _ = child.writer.push(Outgoing::Ping(ping));
                    }
                }
            }
        }
    }

    /// Pings every spare waiting and ends any that has missed its pings, as
    /// a spare's failure.
    fn spare_heartbeat(&mut self, now: Instant, ping: &SupervisorToOutpost) {
        let mut wedged = Vec::new();
        for (&outpost, spare) in &self.spare_children {
            let Some(child) = &spare.child else { continue };
            let decision = wedge_decision(
                spare.last_pong_at,
                now,
                PING_INTERVAL,
                MISSED_PONG_THRESHOLD,
                0,
                ABANDONED_WORKER_LIMIT,
                false,
            );
            if decision.is_some() {
                wedged.push(outpost);
            } else {
                let _ = child.writer.push(Outgoing::Ping(ping.clone()));
            }
        }
        for outpost in wedged {
            tracing::warn!(%outpost, "ending a spare outpost that stopped answering pings");
            let mut up = Duration::ZERO;
            if let Some(spare) = self.spare_children.remove(&outpost) {
                up = spare.started_at.elapsed();
                if let Some(child) = spare.child {
                    self.forget_writer(outpost);
                    self.retire_child(
                        child,
                        format!("spare outpost {outpost}"),
                        "it stopped answering pings".to_owned(),
                    );
                }
            }
            if self.spares.remove(outpost) {
                self.spare_failed(&format!("spare {outpost} stopped answering pings"), up);
            }
        }
    }

    /// Checks on a thread of its own whether `pid`'s application, whose
    /// outpost `outpost` has many abandoned workers, has a visible window
    /// the system reports hung ([`process::application_is_hung`], which
    /// walks its windows), and reports [`OwnerEvent::HangChecked`].
    fn check_hang(&mut self, pid: Pid, outpost: OutpostId) {
        let own_tx = self.own_tx.clone();
        let spawned = thread::Builder::new()
            .name("verbatim-hang-check".to_owned())
            .spawn(move || {
                let hung = process::application_is_hung(pid);
                let _ = own_tx.send(OwnerEvent::HangChecked { outpost, hung });
            });
        match spawned {
            Ok(_) => {
                if let Some(record) = self.records.get_mut(&pid) {
                    record.hang_check = true;
                }
            }
            // Tried again on the next heartbeat.
            Err(error) => {
                tracing::error!(%error, %pid, "no thread to check whether an application is hung");
            }
        }
    }

    /// Judges `outpost` by its abandoned workers now that the check of its
    /// application has answered, as the heartbeat did when it made the
    /// check itself.
    fn on_hang_checked(&mut self, outpost: OutpostId, hung: bool) {
        let Some(pid) = self.pid_of(outpost) else {
            return;
        };
        let Some(record) = self.records.get_mut(&pid) else {
            return;
        };
        record.hang_check = false;
        if self.shutting_down.is_some() || record.child.is_none() {
            return;
        }
        if let Some(reason) = wedge_decision(
            record.last_pong_at,
            Instant::now(),
            PING_INTERVAL,
            MISSED_PONG_THRESHOLD,
            record.abandoned,
            ABANDONED_WORKER_LIMIT,
            hung,
        ) {
            tracing::warn!(%pid, %reason, "ending a stuck outpost");
            self.outpost_failed(pid, &format!("ended as stuck: {reason}"), EndReason::Killed);
        }
    }

    /// Retires every outpost whose application has not held attention for
    /// two minutes and in which Core holds no nodes. Core's own outpost is
    /// never retired: its next menu would otherwise spawn it cold.
    fn sweep(&mut self) {
        if self.shutting_down.is_some() {
            return;
        }
        let now = Instant::now();
        let retiring: Vec<Pid> = self
            .records
            .iter()
            .filter(|(pid, record)| {
                **pid != self.own_pid
                    && record.child.is_some()
                    && retirement_decision(
                        record.last_attention_at,
                        now,
                        IDLE_RETIREMENT,
                        self.attention == Some(**pid),
                        self.holding.contains(&record.outpost),
                    )
            })
            .map(|(pid, _)| *pid)
            .collect();
        for pid in retiring {
            self.end(pid, EndReason::Retired);
            self.release_target(pid);
        }
        // Applications whose failed outposts' replacements wait for their
        // backoff, and which have since exited.
        let gone: Vec<Pid> = self
            .records
            .iter()
            .filter(|(pid, record)| record.backoff.due().is_some() && self.has_exited(**pid))
            .map(|(pid, _)| *pid)
            .collect();
        for pid in gone {
            self.target_exited(pid, "target exited while its outpost's replacement waited");
        }
        // Applications whose records have ended, by retirement, and which
        // have since exited: their locks are closed.
        let exited: Vec<Pid> = self
            .apps
            .iter()
            .filter(|(pid, app)| !self.records.contains_key(pid) && app.target.has_exited())
            .map(|(pid, _)| *pid)
            .collect();
        for pid in exited {
            self.forget_app(pid);
        }
    }
}

/// Reports how a retirement's child ended when it is dropped: at the end
/// of the retirement, or as its thread unwinds from a panic, with the
/// child killed.
struct Retirement {
    own_tx: Mailbox<OwnerEvent>,
    ending: Ending,
}

impl Drop for Retirement {
    fn drop(&mut self) {
        let _ = self.own_tx.send(OwnerEvent::Retired(self.ending));
    }
}

/// Asks an outpost for the idle barrier's round `round`, behind everything
/// queued for it before. One that has ended answers by its end.
fn ask_idle(writer: &WriterHandle, round: u64) {
    let query = SupervisorToOutpost::IdleQuery { round };
    if let Err(error) = writer.push(Outgoing::IdleQuery(query)) {
        tracing::debug!(%error, "the idle query could not reach an outpost");
    }
}

/// Sends a routed fact to an outpost: written at once when its pipe is
/// free, queued otherwise.
pub(super) fn deliver(writer: &WriterHandle, held: HeldFact) {
    if let Err(error) = writer.push(Outgoing::delivered(held)) {
        tracing::warn!(%error, "a routed fact could not be queued");
    }
}

/// The launch helper's work: launch the process and make its writer on the
/// relay's port; for an outpost, register the writer. Returns the launch,
/// with the pipe the relay will read.
fn launch_child(
    exe_path: &std::path::Path,
    (options, ignored, hooks_in): (crate::OutpostOptions, &str, &str),
    (outpost, role, lock): (OutpostId, Role, Option<(OwnedHandle, u64)>),
    (writers, commands): (&Writers, &CommandPipes),
) -> io::Result<Launch> {
    let (launched, pipes) = process::launch(exe_path, role, options, (ignored, hooks_in), lock)?;
    tracing::info!(%outpost, process_id = %launched.process_id, ?role, "launched");
    // A failure from here drops the process's job, which kills it.
    let writer = commands.open(outpost, pipes.to_child)?;
    let source = match role {
        Role::Listener => Source::Listener,
        Role::Outpost(pid) => Source::Outpost(pid),
        Role::Spare(_) => Source::Spare,
    };
    if let Role::Outpost(_) | Role::Spare(_) = role {
        writers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(outpost, writer.clone());
    }
    Ok(Launch {
        child: Child {
            process: launched,
            writer,
        },
        from_child: pipes.from_child,
        source,
    })
}
