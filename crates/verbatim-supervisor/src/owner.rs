//! The process owner: one thread that launches and ends every child,
//! watches each one's liveness by pings, sees pipes close and processes
//! exit, and reports these facts to Core, carrying out Core's start, end
//! and shutdown effects (`docs/design/readers-tell-core-asks-build1.md`,
//! "The outpost generation record"). Core's reducer owns the outpost
//! generation record and judges every generation: when an outpost is
//! started, replaced or ended, its backoff, its liveness, and which frames
//! are current. The owner keeps only what carrying out those effects
//! needs: each application's running process, the spares, the listener's
//! record, and the processes it holds.
//!
//! It is also the relay (`docs/design/communication.md`, sections 4.7 and
//! 6.2): it waits only on the relay's completion port ([`super::relay`]),
//! reads the listener's frames and hands each tell and answer to the app
//! in the pass that read it; reads every outpost's frames, handling
//! pongs, `Ready`, `TargetExited` and the pipe's end itself and handing
//! every other frame to the app whole, in the pass that read it; finishes
//! every child's writes that pended; and takes everything other threads
//! report from its mailbox when the port says there is something. Its
//! heartbeat runs when the wait's deadline passes. It never blocks:
//! launches and retirements run on threads of their own and report back.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use verbatim_model::{OutpostId, Pid};
use verbatim_process::overlapped::{Completion, Completions, Port};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};
use verbatim_process::transport::{Parts, Reading};

use verbatim_model::protocol::{
    CoreToListener, CoreToOutpost, ListenerToCore, OutpostToCore, hop_us,
};
use verbatim_process::clock::now_us;
use verbatim_process::message::{Peeked, decode_message, peek};

use super::Executables;
use super::frames::Frame;
use super::policy::{Liveness, Spares, WedgeLimits, WedgeReason};
use super::process::{self, Launched, Role};
use super::relay::{COMPLETIONS_PER_WAIT, CommandPipes, Inbox, Mailbox, Relay, Source, Taken};
use super::retire::{Ending, ShutdownSummary, retire};
use super::writer::{Outgoing, ToChild, WriterHandle};
use super::{EndReason, EventSink, OutpostMessage, SPARES, Sealed, Timings, Undelivered, Writers};
use verbatim_core::supervisor::Received;
use verbatim_process::arguments::ListenerArguments;
use verbatim_process::held::{IgnoredProcesses, NotHeld, Target};

/// The relay thread's priority: it reads each of the listener's tells and
/// hands it on, reads each outpost's frame and hands it on, finishes
/// writes that pended, and handles each child's reports, for microseconds
/// per wake (`docs/design/communication.md`, section 6.1).
const RELAY_PRIORITY: ThreadPriority = ThreadPriority::Highest;

/// A fact reported to the owner, or an effect it is asked to carry out.
pub(super) enum OwnerEvent {
    /// From the app, Core's effect: start an outpost for this application.
    StartOutpost(Pid),
    /// From a test: read the time the spares' backoff is judged by from
    /// this clock from now on ([`Owner::backoff_clock`]).
    BackoffClock(BackoffClock),
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
    /// From the app: run the idle barrier's round through the children.
    IdleRound(u64),
    /// From the app: a command for the listener, a request or cancel of
    /// Core's.
    ToListener(CoreToListener),
    /// From the app, Core's effect: the listener's configuration, kept for
    /// every later launch and written to the listener running or held
    /// while one launches.
    ConfigureListener {
        /// Whether background progress bars are reported.
        report_background_progress_bars: bool,
    },
    /// From the app: Core's held view for the listener
    /// ([`CoreToListener::ElementsHeld`]), written now when `now`, else
    /// left for the next command written to it.
    ListenerHeld {
        /// The view.
        view: CoreToListener,
        /// Whether it is written now.
        now: bool,
    },
    /// From the app, Core's effect: end this outpost generation.
    EndOutpost(OutpostId),
    /// From the app: Verbatim is exiting. Every child is shut down, and the
    /// summary of every ending in Verbatim's life is sent back once all
    /// have ended; the owner then ends.
    Shutdown(Sender<ShutdownSummary>),
}

/// A running child: its process, held so ending the record kills it, and
/// its writer, of the commands its kind of child takes.
pub(super) struct Child<M: ToChild> {
    pub(super) process: Launched,
    pub(super) writer: WriterHandle<M>,
}

/// An outpost's running process, given or spare.
type OutpostChild = Child<CoreToOutpost>;

/// A launched child of either kind.
pub(super) enum LaunchedChild {
    /// The focus listener.
    Listener(Child<CoreToListener>),
    /// An outpost, for an application or a spare.
    Outpost(OutpostChild),
}

/// A launched child, with its pipe to Core, which the relay reads.
pub(super) struct Launch {
    child: LaunchedChild,
    from_child: OwnedHandle,
    source: Source,
}

/// What the owner thread starts with.
pub(super) struct Setup {
    pub(super) executables: Executables,
    pub(super) options: verbatim_process::arguments::OutpostOptions,
    pub(super) shutdown_limit: Duration,
    /// The periods and limits it runs by.
    pub(super) timings: Timings,
    pub(super) events_tx: EventSink,
    pub(super) writers: Writers,
    pub(super) own_tx: Mailbox<OwnerEvent>,
    pub(super) events: Inbox<OwnerEvent>,
    /// The relay, whose port `own_tx` wakes.
    pub(super) relay: Relay,
    /// The processes ignored entirely.
    pub(super) ignored: std::sync::Arc<IgnoredProcesses>,
    /// Whether background progress bars are reported, from the settings
    /// loaded before the supervisor was made, until Core's first
    /// configuration command.
    pub(super) report_background_progress_bars: bool,
}

/// A child's process from its launch: still being launched, or running.
/// The listener and the spares waiting live this pair of states.
#[derive(Debug)]
pub(super) enum Run<C> {
    /// The launch helper is still at work.
    Launching,
    /// Its launch has reported.
    Running(C),
}

impl<C> Run<C> {
    /// The running child.
    pub(super) fn running(&self) -> Option<&C> {
        match self {
            Run::Running(child) => Some(child),
            Run::Launching => None,
        }
    }

    /// The running child, taken.
    pub(super) fn into_running(self) -> Option<C> {
        match self {
            Run::Running(child) => Some(child),
            Run::Launching => None,
        }
    }
}

/// One application's outpost generation as the owner carries it out, from
/// Core's start effect until it ends: Core judges it, and the owner only
/// reports its process's facts.
enum Generation {
    /// Its process is being launched for the application, or a spare was
    /// given whose launch has not reported.
    Launching {
        /// The generation.
        outpost: OutpostId,
        /// Whether it is a given spare, which is sent `Assign` once it
        /// runs.
        spare: bool,
    },
    /// Its process runs; its `Ready` has not come.
    Starting {
        /// The generation.
        outpost: OutpostId,
        /// Its process.
        child: OutpostChild,
        /// Its last fault, such as a given spare's hook failing: logged
        /// with its end.
        fault: Option<String>,
        /// The idle barrier's round, asked once it is ready.
        idle_query: Option<u64>,
    },
    /// It sent its `Ready`.
    Ready {
        /// The generation.
        outpost: OutpostId,
        /// Its process.
        child: OutpostChild,
    },
}

impl Generation {
    /// Its generation.
    fn outpost(&self) -> OutpostId {
        match self {
            Generation::Launching { outpost, .. }
            | Generation::Starting { outpost, .. }
            | Generation::Ready { outpost, .. } => *outpost,
        }
    }

    /// Its process, once its launch has reported.
    fn child(&self) -> Option<&OutpostChild> {
        match self {
            Generation::Launching { .. } => None,
            Generation::Starting { child, .. } | Generation::Ready { child, .. } => Some(child),
        }
    }

    /// Its process, taken, once its launch has reported.
    fn into_child(self) -> Option<OutpostChild> {
        match self {
            Generation::Launching { .. } => None,
            Generation::Starting { child, .. } | Generation::Ready { child, .. } => Some(child),
        }
    }

    /// Whether it sent its `Ready`.
    fn is_ready(&self) -> bool {
        matches!(self, Generation::Ready { .. })
    }
}

/// Where the idle barrier's round is in the relay
/// (`docs/design/communication.md`, section 6.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdleStage {
    /// The listener has been asked and has not answered.
    Listener(u64),
    /// The listener has answered, or has gone, and an outpost is still
    /// being launched: the outposts are asked once every launch has
    /// reported, so the app has heard each one started.
    Launches(u64),
}

/// The focus listener.
struct ListenerRecord {
    outpost: OutpostId,
    child: Run<Child<CoreToListener>>,
    /// Its pongs.
    liveness: Liveness,
    /// Commands for it, held while its launch is still at work, in order,
    /// and written once it has a writer.
    pending: Vec<CoreToListener>,
    /// The processes whose exit it has been told and has not answered, each
    /// handle held until [`ListenerToCore::ExitHandled`], so no new process
    /// can have the id until the listener has moved its record.
    exits_told: BTreeMap<Pid, Target>,
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

/// A clock in microseconds, the run's shared clock's units.
pub(super) type BackoffClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The owner thread's state.
struct Owner {
    /// The time the spares' backoff is set and ended by: the run's shared
    /// clock, or a test's ([`crate::Supervisor::set_backoff_clock`]).
    backoff_clock: BackoffClock,
    /// The executables each role is launched from.
    executables: Executables,
    /// What every outpost is launched with.
    options: verbatim_process::arguments::OutpostOptions,
    /// The periods and limits it runs by.
    timings: Timings,
    outbox: Outbox,
    writers: Writers,
    own_tx: Mailbox<OwnerEvent>,
    /// The port this thread waits on, and the pipes it serves.
    relay: Relay,
    /// The relay's port, which a reading of the listener's pipe is given.
    port: Arc<Port>,
    /// Numbers every launch, outposts and listener alike; never reused.
    generation: u64,
    /// Each application's outpost generation, from Core's start effect
    /// until it ends.
    records: BTreeMap<Pid, Generation>,
    listener: Option<ListenerRecord>,
    /// The applications held, each until its process exits: the process,
    /// held open so the pid it is known by names that process and no other
    /// ([`Target`]), from the first tell that names it or Core's start of
    /// its outpost, so a process id is never reused while Core, an outpost
    /// or a reading thread knows it.
    apps: BTreeMap<Pid, Target>,
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
    /// the listener, told of them, drops their events.
    ignored: std::sync::Arc<IgnoredProcesses>,
    /// Whether background progress bars are reported, as Core's last
    /// configuration command said, or the settings loaded at startup
    /// before Core's first: every listener generation is launched with it,
    /// since the owner relaunches the listener without Core. Only Core's
    /// commands set it, in Core's order, so a launch and a later change
    /// cannot cross: a command that arrives while the listener launches is
    /// held in its `pending` and written after the launch.
    report_background_progress_bars: bool,
    /// The idle barrier's round under way in the relay, until the
    /// outposts are asked.
    idle: Option<IdleStage>,
    /// The spare outposts kept waiting, and the replacements owed
    /// (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts").
    spares: Spares,
    /// Every spare's process, by outpost id, from its launch until it is
    /// given or ends.
    spare_children: BTreeMap<OutpostId, SpareChild>,
}

/// A spare outpost's process while it waits.
struct SpareChild {
    child: Run<OutpostChild>,
    /// Its pongs.
    liveness: Liveness,
    /// The last fault it reported: logged with its failure.
    fault: Option<String>,
    /// When it was launched, on the run's shared clock: how long it had
    /// been up when it fails.
    started_us: u64,
}

/// Starts the owner thread, which starts the listener at once.
pub(super) fn start(setup: Setup) -> io::Result<()> {
    let Setup {
        executables,
        options,
        shutdown_limit,
        timings,
        events_tx,
        writers,
        own_tx,
        events,
        relay,
        ignored,
        report_background_progress_bars,
    } = setup;
    let mut owner = Owner {
        backoff_clock: Arc::new(now_us),
        executables,
        options,
        shutdown_limit,
        timings,
        retiring: 0,
        summary: ShutdownSummary::default(),
        shutting_down: None,
        outbox: Outbox::new(events_tx),
        writers,
        own_tx,
        port: Arc::clone(relay.port()),
        relay,
        generation: 0,
        records: BTreeMap::new(),
        listener: None,
        apps: BTreeMap::new(),
        own_pid: Pid(std::process::id()),
        ignored,
        report_background_progress_bars,
        idle: None,
        spares: Spares::default(),
        spare_children: BTreeMap::new(),
    };
    thread::Builder::new()
        .name("verbatim-relay".to_owned())
        .spawn(move || {
            prioritize_role("relay", RELAY_PRIORITY);
            owner.start_listener();
            owner.run(&events);
        })?;
    Ok(())
}

/// How long from `since_us` until now, on the run's shared clock.
fn up_since(since_us: u64) -> Duration {
    Duration::from_micros(now_us().saturating_sub(since_us))
}
/// The next time a periodic duty with `interval` is due after `due`, which
/// has just been done at `now`: a missed time is skipped, never made up.
fn next_due(due: Instant, now: Instant, interval: Duration) -> Instant {
    let next = due + interval;
    if next > now { next } else { now + interval }
}
impl Owner {
    /// The limits a child is judged stuck by.
    fn wedge_limits(&self) -> WedgeLimits {
        WedgeLimits {
            ping_interval: self.timings.ping_interval,
            missed_pongs: self.timings.missed_pongs,
        }
    }
    /// The relay's loop: waits on the port until the heartbeat is due or
    /// the spares' backoff ends, and handles what the wait took, in the
    /// port's order. Returns once Verbatim's exit has ended every child
    /// and every pipe has finished.
    fn run(&mut self, events: &Inbox<OwnerEvent>) {
        let start = Instant::now();
        let ping_interval = self.timings.ping_interval;
        let mut heartbeat_due = start + ping_interval;
        let mut completions = Completions::with_capacity(COMPLETIONS_PER_WAIT);
        loop {
            if self.finish_shutdown() {
                return;
            }
            let mut timeout = heartbeat_due.saturating_duration_since(Instant::now());
            if let Some(due_us) = self.spares.due() {
                timeout = timeout.min(Duration::from_micros(
                    due_us.saturating_sub((self.backoff_clock)()),
                ));
            }
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
            if self
                .spares
                .due()
                .is_some_and(|due| due <= (self.backoff_clock)())
            {
                self.replenish_spares();
            }
            let now = Instant::now();
            if now >= heartbeat_due {
                // What was sent while the port refused the mailbox's wake,
                // and what the app's port refused.
                for event in events.take() {
                    self.handle(event);
                }
                self.outbox.retry();
                self.heartbeat();
                heartbeat_due = next_due(heartbeat_due, now, ping_interval);
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
        let reading = read.pipe.take(completion, &port, |body| match source {
            Source::Listener => {
                // When verbatim.exe took the message in, before it is
                // parsed (`docs/design/communication.md`, section 10.1).
                let received = Received { at_us: now_us() };
                self.listener_message(outpost, received, decode_message(body)?);
                Ok(())
            }
            Source::Outpost(pid) => self.outpost_frame(outpost, pid, body),
            Source::Spare => self.spare_frame(outpost, body),
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
                tracing::warn!(target: verbatim_model::DEFECT_TARGET, %outpost, %error, "an outpost's message could not be read");
                self.on_pipe_closed(outpost);
            }
        }
    }
    /// A frame read from the pipe of `outpost`, watching `pid`, dropped
    /// once the owner has ended that outpost: a pong and `TargetExited`
    /// are handled here, `Ready` is handled here and handed on, and every
    /// other frame is handed on to the app whole, unparsed, with when it
    /// was read (`docs/design/communication.md`, section 6.2). Only the
    /// frame's kind is read, from its first bytes; a pong, a few dozen
    /// bytes, is parsed for its counts of parked workers. A frame handed
    /// on is copied out of the reader's buffer it was lent from.
    fn outpost_frame(&mut self, outpost: OutpostId, pid: Pid, body: Parts<'_>) -> io::Result<()> {
        if self
            .records
            .get(&pid)
            .is_none_or(|generation| generation.outpost() != outpost)
        {
            // The owner has ended this generation, and told the app; its
            // pipe is read until it ends, and the rest is dropped here.
            return Ok(());
        }
        let read_at_us = now_us();
        #[deny(clippy::wildcard_enum_match_arm)]
        match peek(body.body) {
            Peeked::Pong => {
                let OutpostToCore::Pong {
                    parked_count,
                    written_at_us,
                    ..
                } = decode_message(body)?
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
            Peeked::TargetExited => self.application_exited(pid, "its outpost saw it exit"),
            Peeked::Ready => {
                self.hand_on(Frame::new(pid, outpost, body.into_owned()));
                self.on_ready(outpost);
            }
            // Written by a spare before it read its `Assign`, and read
            // since: it was given already.
            Peeked::SpareReady => {}
            Peeked::Fault => {
                // A given spare's fault before its `Ready`, such as its
                // hook failing, is kept for the log of its end.
                if let Some(Generation::Starting { .. }) = self.records.get(&pid)
                    && let OutpostToCore::Fault { detail } = decode_message(body)?
                    && let Some(Generation::Starting { fault, .. }) = self.records.get_mut(&pid)
                {
                    *fault = Some(detail);
                }
                self.hand_on(Frame::new(pid, outpost, body.into_owned()));
            }
            Peeked::Other => self.hand_on(Frame::new(pid, outpost, body.into_owned())),
        }
        Ok(())
    }

    /// A frame read from the pipe of the spare `outpost`, before it was
    /// given: its `SpareReady`, its pongs, and its faults. Dropped once the
    /// spare has ended.
    fn spare_frame(&mut self, outpost: OutpostId, body: Parts<'_>) -> io::Result<()> {
        let Some(spare) = self.spare_children.get_mut(&outpost) else {
            return Ok(());
        };
        #[deny(clippy::wildcard_enum_match_arm)]
        match peek(body.body) {
            Peeked::Pong => spare.liveness.ponged(now_us()),
            Peeked::SpareReady => {
                let OutpostToCore::SpareReady { outpost_pid } = decode_message(body)? else {
                    return Err(io::Error::other("a spare's ready that is not one"));
                };
                if let Some(waiting) = self.spares.ready(outpost) {
                    tracing::info!(%outpost, %outpost_pid, waiting, "spare outpost ready");
                    self.outbox.send(OutpostMessage::SpareReady {
                        spare: outpost,
                        outpost_pid,
                        waiting,
                        sealed: Sealed::new(),
                    });
                }
            }
            Peeked::Fault => {
                if let OutpostToCore::Fault { detail } = decode_message(body)? {
                    tracing::warn!(%outpost, detail, "spare outpost fault");
                    spare.fault = Some(detail);
                }
            }
            kind @ (Peeked::Ready | Peeked::TargetExited | Peeked::Other) => {
                tracing::debug!(%outpost, ?kind, "a spare outpost's message dropped");
            }
        }
        Ok(())
    }
    /// Hands an outpost's frame on to the app. One the app no longer takes
    /// is dropped: it is exiting.
    fn hand_on(&mut self, frame: Frame) {
        self.outbox.send(OutpostMessage::Frame(frame));
    }
    /// A message read from the listener `outpost`'s pipe at `received`:
    /// tells and answers are handed to the app for Core, which judges
    /// them; the window in front, pongs and `Ready` are handled as the
    /// owner's events; faults are logged. Every frame of a listener other
    /// than the current one is dropped, so nothing of a listener replaced
    /// reaches Core. A tell's process is held from the first tell that
    /// names it, so its id names that process until the listener has
    /// handled its exit.
    fn listener_message(
        &mut self,
        outpost: OutpostId,
        received: Received,
        message: ListenerToCore,
    ) {
        if self
            .listener
            .as_ref()
            .is_none_or(|listener| listener.outpost != outpost)
        {
            tracing::debug!(%outpost, "a frame of a listener that was replaced is dropped");
            return;
        }
        #[deny(clippy::wildcard_enum_match_arm)]
        match message {
            ListenerToCore::Pong { written_at_us, .. } => {
                if let Some(us) = hop_us(written_at_us, received.at_us) {
                    tracing::trace!(target: "verbatim::stage", stage = "listener's pipe to verbatim.exe", us, "pong");
                }
                self.handle(OwnerEvent::Pong {
                    outpost,
                    abandoned: 0,
                });
            }
            ListenerToCore::Progress { change, timing } => {
                tracing::trace!(
                    target: "verbatim::stage",
                    handling_us = hop_us(timing.observed_at_us, timing.listener_handled_at_us),
                    queue_us = hop_us(timing.listener_handled_at_us, timing.listener_written_at_us),
                    pipe_us = hop_us(timing.listener_written_at_us, received.at_us),
                    ?change,
                    "a progress bar's change: listener handling, listener queue, listener's pipe to verbatim.exe"
                );
                self.outbox.send(OutpostMessage::Listener {
                    listener: outpost,
                    message: Box::new(ListenerToCore::Progress { change, timing }),
                });
            }
            ListenerToCore::Ready { .. } => self.on_ready(outpost),
            // Core's event queue judges them.
            ListenerToCore::Tell(tell) => {
                self.hold_told(tell.process);
                self.outbox.send(OutpostMessage::Listener {
                    listener: outpost,
                    message: Box::new(ListenerToCore::Tell(tell)),
                });
            }
            message @ (ListenerToCore::Answer(_)
            | ListenerToCore::Destroyed { .. }
            | ListenerToCore::Foreground { .. }) => {
                self.outbox.send(OutpostMessage::Listener {
                    listener: outpost,
                    message: Box::new(message),
                });
            }
            ListenerToCore::IdleAnswer { round } => self.listener_idle(outpost, round),
            ListenerToCore::ExitHandled { pid } => self.exit_handled(outpost, pid),
            ListenerToCore::Fault { detail } => {
                tracing::warn!(detail, "focus listener fault");
            }
        }
    }

    fn handle(&mut self, event: OwnerEvent) {
        match event {
            OwnerEvent::Shutdown(reply) => self.shut_down_all(reply),
            OwnerEvent::BackoffClock(clock) => self.backoff_clock = clock,
            OwnerEvent::StartOutpost(pid) => {
                tracing::info!(%pid, "the owner is asked to start an outpost");
                self.start_outpost(pid);
            }
            OwnerEvent::Pong { outpost, abandoned } => self.ponged(outpost, abandoned),
            OwnerEvent::Unreadable(outpost) => self.on_pipe_closed(outpost),
            OwnerEvent::Launched { outpost, result } => self.on_launched(outpost, result),
            OwnerEvent::Retired(ending) => {
                self.retiring = self.retiring.saturating_sub(1);
                self.summary.count(ending);
            }
            OwnerEvent::IdleRound(round) => self.idle_round(round),
            OwnerEvent::ToListener(command) => self.tell_listener(command),
            OwnerEvent::ConfigureListener {
                report_background_progress_bars,
            } => {
                self.report_background_progress_bars = report_background_progress_bars;
                self.tell_listener(CoreToListener::Configuration {
                    report_background_progress_bars,
                });
            }
            OwnerEvent::ListenerHeld { view, now } => self.listener_held(view, now),
            OwnerEvent::EndOutpost(outpost) => {
                if let Some(pid) = self.pid_of(outpost) {
                    self.end(pid, EndReason::Killed, "Core ended it");
                }
            }
        }
    }

    /// `outpost` answered a ping, reporting `abandoned` workers: an
    /// application's current generation, whose liveness and abandoned
    /// workers Core judges, or the listener.
    fn ponged(&mut self, outpost: OutpostId, abandoned: usize) {
        if let Some(pid) = self.pid_of(outpost) {
            self.outbox.send(OutpostMessage::Pong {
                outpost,
                target_pid: pid,
                abandoned,
            });
        } else if let Some(listener) = self
            .listener
            .as_mut()
            .filter(|listener| listener.outpost == outpost)
        {
            listener.liveness.ponged(now_us());
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
        let query = CoreToListener::IdleQuery { round };
        let reached = if let Some(child) = listener.child.running() {
            let pushed = child.writer.push(Outgoing::Unbounded(query));
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
    /// one pass, behind every command sent to it before, and tells the app
    /// which first ([`OutpostMessage::IdleQueried`]). One still starting is
    /// asked once it is ready. While an outpost is still being launched,
    /// the app may not have heard it started, so the outposts are asked
    /// once every launch has reported.
    fn query_outposts(&mut self, round: u64) {
        if self.shutting_down.is_none()
            && self
                .records
                .values()
                .any(|generation| matches!(generation, Generation::Launching { .. }))
        {
            self.idle = Some(IdleStage::Launches(round));
            return;
        }
        self.idle = None;
        let outposts = self
            .records
            .values()
            .filter(|generation| generation.child().is_some())
            .map(Generation::outpost)
            .collect();
        self.outbox.send(OutpostMessage::IdleQueried {
            round,
            outposts,
            sealed: Sealed::new(),
        });
        for generation in self.records.values_mut() {
            match generation {
                Generation::Ready { child, .. } => ask_idle(&child.writer, round),
                Generation::Starting { idle_query, .. } => *idle_query = Some(round),
                Generation::Launching { .. } => {}
            }
        }
    }

    /// The application `outpost` serves, if it is its current generation.
    fn pid_of(&self, outpost: OutpostId) -> Option<Pid> {
        self.records
            .iter()
            .find(|(_, generation)| generation.outpost() == outpost)
            .map(|(pid, _)| *pid)
    }

    fn next_outpost_id(&mut self) -> OutpostId {
        self.generation += 1;
        OutpostId(self.generation)
    }
    /// Carries out Core's effect "start an outpost for `pid`": gives it
    /// the oldest spare waiting, ready or not, whose replacement starts at
    /// once, or, with none, launches one for it. An application whose
    /// process has exited, or cannot be held, or that Verbatim ignores,
    /// gets none, and Core hears its launch failed. Core asks for none
    /// while a generation of the application lives, so one that does is a
    /// logic bug, logged and left.
    fn start_outpost(&mut self, pid: Pid) {
        if self.shutting_down.is_some() {
            return;
        }
        if let Some(generation) = self.records.get(&pid) {
            tracing::error!(target: verbatim_model::DEFECT_TARGET, %pid, outpost = %generation.outpost(), "Core asked to start an outpost for an application whose generation lives, a logic bug");
            return;
        }
        if self.ignored.contains(pid) {
            tracing::debug!(%pid, "no outpost is started for a process Verbatim ignores");
            self.launch_failed(pid, true);
            return;
        }
        if let Err(not_held) = self.hold_target(pid) {
            tracing::info!(%pid, %not_held, "no outpost is started for an application that is not running");
            self.launch_failed(pid, true);
            return;
        }
        if let Some(spare) = self.spares.take() {
            let child = self
                .spare_children
                .remove(&spare)
                .and_then(|spare| spare.child.into_running());
            tracing::info!(outpost = %spare, %pid, "a spare outpost is given to the application");
            match child {
                Some(child) => self.running(pid, spare, child, true),
                None => {
                    self.records.insert(
                        pid,
                        Generation::Launching {
                            outpost: spare,
                            spare: true,
                        },
                    );
                }
            }
            self.replenish_spares();
        } else {
            let outpost = self.next_outpost_id();
            self.records.insert(
                pid,
                Generation::Launching {
                    outpost,
                    spare: false,
                },
            );
            self.launch(outpost, Role::Outpost(pid));
        }
    }

    /// `pid`'s generation `outpost` runs `child`: a given spare is sent its
    /// `Assign`, ahead of everything the app will send it, and its pipe is
    /// read as the application's from now on; the app hears it started
    /// (Table 5, column B).
    fn running(&mut self, pid: Pid, outpost: OutpostId, child: OutpostChild, spare: bool) {
        if spare {
            if let Err(error) = child
                .writer
                .push(Outgoing::Unbounded(CoreToOutpost::Assign {
                    target_pid: pid,
                }))
            {
                // Its pipe's end follows, and is its end.
                tracing::warn!(%error, %outpost, %pid, "a spare could not be sent its application");
            }
            self.relay.set_source(outpost, Source::Outpost(pid));
        }
        self.records.insert(
            pid,
            Generation::Starting {
                outpost,
                child,
                fault: None,
                idle_query: None,
            },
        );
        self.outbox.send(OutpostMessage::Started {
            outpost,
            target_pid: pid,
            sealed: Sealed::new(),
        });
    }

    /// No outpost was started for `pid`: Core hears its launch failed,
    /// and whether because the application is gone.
    fn launch_failed(&mut self, pid: Pid, application_gone: bool) {
        self.outbox.send(OutpostMessage::LaunchFailed {
            target_pid: pid,
            application_gone,
            sealed: Sealed::new(),
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
        let now = now_us();
        for _ in 0..self.spares.wanted(SPARES, now) {
            let spare = self.next_outpost_id();
            self.spares.add(spare);
            self.spare_children.insert(
                spare,
                SpareChild {
                    child: Run::Launching,
                    liveness: Liveness::new(now),
                    fault: None,
                    started_us: now,
                },
            );
            self.launch(spare, Role::Spare(spare));
        }
    }
    /// Logs the failure, for `why`, of a waiting spare that had been up for
    /// `up` as an error, the evidence to find its cause from, and replaces
    /// it, at once or after its backoff: spares are always replaced.
    fn spare_failed(&mut self, why: &str, up: Duration) {
        let backoff = self.timings.backoff();
        let wait = self.spares.failed(up, (self.backoff_clock)(), backoff);
        tracing::error!(
            why,
            backoff_ms = wait.as_millis(),
            "a spare outpost failed; it is replaced"
        );
        self.replenish_spares();
    }
    /// Removes `outpost`'s writer from those the app can reach.
    fn forget_writer(&self, outpost: OutpostId) {
        self.writers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&outpost);
    }
    /// Holds `pid`'s process, unless it is held already: the pid then names
    /// that process for as long as it is held. Fails when the process has
    /// exited, the one held included, or cannot be opened.
    fn hold_target(&mut self, pid: Pid) -> Result<(), NotHeld> {
        match self.apps.get(&pid) {
            Some(target) if target.has_exited() => Err(NotHeld::Exited),
            Some(_) => Ok(()),
            None => {
                let target = Target::open(pid)?;
                self.apps.insert(pid, target);
                Ok(())
            }
        }
    }

    /// Holds the process a tell names, from the first tell that names it,
    /// so its id names that process until the listener has handled its
    /// exit (`docs/design/readers-tell-core-asks-build1.md`, "The listener's
    /// record"). A process held already costs a lookup and nothing more.
    fn hold_told(&mut self, pid: Pid) {
        if self.apps.contains_key(&pid) || self.ignored.contains(pid) {
            return;
        }
        match Target::open(pid) {
            Ok(target) => {
                self.apps.insert(pid, target);
            }
            Err(not_held) => {
                tracing::debug!(%pid, %not_held, "a process a tell names cannot be held");
            }
        }
    }

    /// Whether `pid`'s process has exited, or is not held.
    fn has_exited(&self, pid: Pid) -> bool {
        self.apps.get(&pid).is_none_or(Target::has_exited)
    }
    /// Tells the running listener that `pid`'s process, `target`, has
    /// exited, keeping `target` open until it answers. With no listener
    /// running, or one still launching, `target` closes at once and nothing
    /// is sent, since a new listener has no record of the process.
    fn tell_exit(&mut self, pid: Pid, target: Target) {
        let Some(listener) = self.listener.as_mut() else {
            return;
        };
        let Some(child) = listener.child.running() else {
            return;
        };
        match child
            .writer
            .push(Outgoing::Unbounded(CoreToListener::ApplicationExited {
                pid,
            })) {
            Ok(()) => {
                listener.exits_told.insert(pid, target);
            }
            Err(error) => {
                tracing::warn!(%error, "an application's exit could not be told to the listener");
            }
        }
    }
    /// The listener `outpost` has moved `pid`'s record: its handle closes.
    fn exit_handled(&mut self, outpost: OutpostId, pid: Pid) {
        if let Some(listener) = self
            .listener
            .as_mut()
            .filter(|listener| listener.outpost == outpost)
        {
            drop(listener.exits_told.remove(&pid));
        }
    }
    /// `pid`'s application has exited, for `why`: its outpost, if one
    /// runs, is ended, the app hearing its end; Core hears the exit
    /// (Table 5, column J); and the listener is told, its handle held
    /// until it answers.
    fn application_exited(&mut self, pid: Pid, why: &str) {
        if self.records.contains_key(&pid) {
            self.end(pid, EndReason::TargetExited, why);
        }
        if let Some(target) = self.apps.remove(&pid) {
            tracing::debug!(%pid, why, "an application's process exited");
            self.outbox.send(OutpostMessage::ApplicationExited {
                target_pid: pid,
                sealed: Sealed::new(),
            });
            self.tell_exit(pid, target);
        }
    }

    /// Records a new listener as starting and hands its launch to a helper,
    /// telling the app first ([`OutpostMessage::ListenerStarted`]), so Core
    /// keeps its first tells.
    fn start_listener(&mut self) {
        let outpost = self.next_outpost_id();
        self.listener = Some(ListenerRecord {
            outpost,
            child: Run::Launching,
            liveness: Liveness::new(now_us()),
            pending: Vec::new(),
            exits_told: BTreeMap::new(),
        });
        self.outbox.send(OutpostMessage::ListenerStarted {
            listener: outpost,
            sealed: Sealed::new(),
        });
        self.launch(outpost, Role::Listener);
    }

    /// Sends the listener `command`, never dropped, or holds it, in order,
    /// while the listener's launch is at work. With no listener it is not
    /// needed: Core answers what it asked of a listener that ended.
    fn tell_listener(&mut self, command: CoreToListener) {
        let Some(listener) = self.listener.as_mut() else {
            return;
        };
        match listener.child.running() {
            Some(child) => {
                if let Err(error) = child.writer.push(Outgoing::Unbounded(command)) {
                    tracing::warn!(%error, "a command could not be told to the listener");
                }
            }
            None => listener.pending.push(command),
        }
    }

    /// Writes Core's held view `view` to the listener now when `now`, in
    /// place of a view left and not yet written, or leaves it in its
    /// writer's slot for the next command written to it. While the
    /// listener launches, a view to write now is held with its commands, in
    /// order, and one to leave is dropped: it releases nothing. The
    /// listener drops a view naming another generation.
    fn listener_held(&mut self, view: CoreToListener, now: bool) {
        let Some(listener) = self.listener.as_mut() else {
            return;
        };
        match (listener.child.running(), now) {
            (Some(child), true) => child.writer.send_held(view),
            (Some(child), false) => child.writer.leave_held(view),
            (None, true) => listener.pending.push(view),
            (None, false) => {}
        }
    }

    /// Launches a child on a helper thread. The helper makes the child's
    /// writer on the relay's port and, for an outpost, registers it; then
    /// it reports [`OwnerEvent::Launched`]. The relay sends `Started` to the
    /// app as it takes the launch, through the same [`Outbox`] as the end
    /// of the generation before, and only then reads the child's pipe. So
    /// the app hears of a generation after the end of the one before and
    /// before any of its messages, a request can reach the outpost as soon
    /// as the app has heard, and the owner always learns of the launch
    /// before the child's `Ready` or the end of its pipe.
    fn launch(&self, outpost: OutpostId, role: Role) {
        let listener = match role {
            Role::Listener => ListenerArguments {
                generation: outpost.0,
                ignored: self.ignored.pids(),
                report_background_progress_bars: self.report_background_progress_bars,
            },
            Role::Outpost(_) | Role::Spare(_) => ListenerArguments::default(),
        };
        let exe_path = self.executables.of(role).to_owned();
        let options = self.options;
        let own_tx = self.own_tx.clone();
        let writers = self.writers.clone();
        let commands = self.relay.commands().clone();
        let spawned = thread::Builder::new()
            .name("verbatim-launch".to_owned())
            .spawn(move || {
                let result = launch_child(
                    &exe_path,
                    (options, listener),
                    (outpost, role),
                    (&writers, &commands),
                );
                // Where a test holds a listener's launch, so a command it
                // sends meanwhile is held in the listener's pending
                // commands.
                #[cfg(any(test, feature = "faults"))]
                if matches!(role, Role::Listener) {
                    verbatim_faults::hold(
                        verbatim_faults::Fault::ListenerLaunchReported,
                        &outpost.0,
                    );
                }
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

    fn launched(&mut self, outpost: OutpostId, result: io::Result<LaunchedChild>) {
        let result = match result {
            Ok(LaunchedChild::Listener(child)) => {
                self.listener_launched(outpost, Ok(child));
                return;
            }
            Ok(LaunchedChild::Outpost(child)) => Ok(child),
            Err(error) => Err(error),
        };
        if self
            .listener
            .as_ref()
            .is_some_and(|listener| listener.outpost == outpost)
        {
            if let Err(error) = result {
                self.listener_launched(outpost, Err(error));
            }
            return;
        }
        self.outpost_launched(outpost, result);
    }
    /// The listener `outpost`'s launch reported.
    fn listener_launched(&mut self, outpost: OutpostId, result: io::Result<Child<CoreToListener>>) {
        if let Some(listener) = self
            .listener
            .as_mut()
            .filter(|listener| listener.outpost == outpost)
        {
            match result {
                Ok(child) => {
                    let pending = std::mem::take(&mut listener.pending);
                    listener.child = Run::Running(child);
                    listener.liveness.ponged(now_us());
                    let Some(child) = self
                        .listener
                        .as_ref()
                        .and_then(|listener| listener.child.running())
                    else {
                        return;
                    };
                    for command in pending {
                        if let Err(error) = child.writer.push(Outgoing::Unbounded(command)) {
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
        } else if let Ok(child) = result {
            self.retire_child(
                child,
                format!("focus listener {outpost}"),
                "launched after it was no longer wanted".to_owned(),
            );
        }
    }
    /// The outpost `outpost`'s launch reported, a spare's or an
    /// application's. One that failed is reported to Core as a failed
    /// launch, which Core judges.
    fn outpost_launched(&mut self, outpost: OutpostId, result: io::Result<OutpostChild>) {
        if self.spare_children.contains_key(&outpost) {
            self.spare_launched(outpost, result);
            return;
        }
        let Some(pid) = self.pid_of(outpost) else {
            // A child nobody wants any more: its generation was ended while
            // it launched, but the child must still die.
            if let Ok(child) = result {
                tracing::warn!(%outpost, "launched a child that is no longer wanted");
                self.forget_writer(outpost);
                self.retire_child(
                    child,
                    format!("outpost {outpost}"),
                    "launched after it was no longer wanted".to_owned(),
                );
            }
            return;
        };
        let Some(Generation::Launching { spare, .. }) = self.records.remove(&pid) else {
            if let Ok(child) = result {
                tracing::error!(target: verbatim_model::DEFECT_TARGET, %outpost, %pid, "launched a child for a generation that is not launching, a logic bug");
                self.forget_writer(outpost);
                self.retire_child(
                    child,
                    format!("outpost {outpost}"),
                    "launched for a generation that was not launching".to_owned(),
                );
            }
            return;
        };
        match result {
            Ok(child) => {
                self.running(pid, outpost, child, spare);
                if self.shutting_down.is_some() {
                    self.end_quietly(pid, "Verbatim is exiting");
                }
            }
            Err(error) => {
                tracing::error!(%error, %pid, %outpost, "failed to launch an outpost");
                if pid == self.own_pid {
                    self.keep_spares();
                }
                if self.shutting_down.is_none() {
                    self.launch_failed(pid, self.has_exited(pid));
                }
            }
        }
    }

    /// The launch of the spare `outpost` reported: it waits from now on,
    /// unless Verbatim is exiting, when it is ended; a failed launch is a
    /// spare's failure.
    fn spare_launched(&mut self, outpost: OutpostId, result: io::Result<OutpostChild>) {
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
                    spare.child = Run::Running(child);
                    spare.liveness.ponged(now_us());
                }
            }
            Err(error) => {
                tracing::error!(%error, %outpost, "failed to launch a spare outpost");
                let up = self
                    .spare_children
                    .remove(&outpost)
                    .map_or(Duration::ZERO, |spare| up_since(spare.started_us));
                if self.spares.remove(outpost) {
                    self.spare_failed(&format!("spare {outpost}'s launch failed: {error}"), up);
                }
            }
        }
    }
    /// A child said `Ready`. The listener's is reported to the app. An
    /// outpost's frame has been handed on already; it is marked ready, asked the idle round held for it,
    /// and, for Verbatim's own, spares are kept from now on.
    fn on_ready(&mut self, outpost: OutpostId) {
        if self
            .listener
            .as_ref()
            .is_some_and(|listener| listener.outpost == outpost)
        {
            tracing::info!(%outpost, "focus listener ready");
            self.outbox.send(OutpostMessage::ListenerReady {
                listener: outpost,
                sealed: Sealed::new(),
            });
            return;
        }
        let Some(pid) = self.pid_of(outpost) else {
            return;
        };
        let Some(Generation::Starting {
            outpost,
            child,
            idle_query,
            ..
        }) = self.records.remove(&pid)
        else {
            tracing::debug!(%outpost, %pid, "a second Ready is dropped");
            return;
        };
        tracing::info!(%outpost, %pid, "outpost ready");
        if let Some(round) = idle_query {
            ask_idle(&child.writer, round);
        }
        self.records
            .insert(pid, Generation::Ready { outpost, child });
        if pid == self.own_pid {
            self.keep_spares();
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
            self.listener_gone();
            self.start_listener();
            return;
        }
        if let Some(spare) = self.spare_children.remove(&outpost) {
            let listed = self.spares.remove(outpost);
            let up = up_since(spare.started_us);
            self.forget_writer(outpost);
            if let Some(child) = spare.child.into_running() {
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
            // Its application's exit, when that is why, is told as such.
            if self.has_exited(pid) {
                self.application_exited(pid, "its pipe closed after its application exited");
            } else {
                self.end(pid, EndReason::Exited, "its pipe closed");
            }
        }
        // Otherwise the owner already ended it, and the app already heard.
    }

    /// Ends `pid`'s generation, for `reason`, saying `why` in the log: its
    /// writer goes, its process is shut down cleanly and killed only if it
    /// does not exit in time ([`retire`], on a thread of its own), and the
    /// app hears its end at once ([`OutpostMessage::Ended`]). Core judges
    /// what follows. Verbatim's own outpost ending before its `Ready`
    /// starts the spares.
    fn end(&mut self, pid: Pid, reason: EndReason, why: &str) {
        let Some(generation) = self.records.remove(&pid) else {
            return;
        };
        let outpost = generation.outpost();
        let ready = generation.is_ready();
        let why = match &generation {
            Generation::Starting {
                fault: Some(fault), ..
            } => format!("{why}, after its fault: {fault}"),
            Generation::Launching { .. }
            | Generation::Starting { .. }
            | Generation::Ready { .. } => why.to_owned(),
        };
        // A failure before `Ready`, such as a hook that fails, is an error,
        // whose sound a user hears, as a spare's failure is; an end after
        // it is a warning.
        if ready || reason != EndReason::Exited {
            tracing::warn!(%outpost, %pid, %reason, why, "an outpost ended");
        } else {
            tracing::error!(%outpost, %pid, %reason, why, "an outpost ended before it was ready");
        }
        self.finish_removal(pid, outpost, generation, &why);
        self.outbox.send(OutpostMessage::Ended {
            outpost,
            target_pid: pid,
            reason,
            sealed: Sealed::new(),
        });
    }

    /// Ends `pid`'s generation as [`end`](Self::end) does, without telling
    /// the app: Verbatim is exiting.
    fn end_quietly(&mut self, pid: Pid, why: &str) {
        if let Some(generation) = self.records.remove(&pid) {
            let outpost = generation.outpost();
            self.finish_removal(pid, outpost, generation, why);
        }
    }

    /// Ends what a removed generation left: its writer goes and its
    /// process is retired, for `why`.
    fn finish_removal(&mut self, pid: Pid, outpost: OutpostId, generation: Generation, why: &str) {
        if pid == self.own_pid && !generation.is_ready() {
            self.keep_spares();
        }
        self.forget_writer(outpost);
        if let Some(child) = generation.into_child() {
            self.retire_child(
                child,
                format!("outpost {outpost} for {pid}"),
                why.to_owned(),
            );
        }
    }

    /// Ends `child` cleanly on a thread of its own ([`retire`]), which
    /// reports how it ended ([`OwnerEvent::Retired`]).
    fn retire_child<M: ToChild>(&mut self, child: Child<M>, label: String, why: String) {
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
        // Those still launching are ended as their launch reports.
        let launched: Vec<Pid> = self
            .records
            .iter()
            .filter(|(_, generation)| generation.child().is_some())
            .map(|(pid, _)| *pid)
            .collect();
        for pid in launched {
            self.end_quietly(pid, "Verbatim is exiting");
        }
        // Spares still launching are ended as their launch reports.
        let spares: Vec<OutpostId> = self
            .spare_children
            .iter()
            .filter(|(_, spare)| spare.child.running().is_some())
            .map(|(outpost, _)| *outpost)
            .collect();
        for outpost in spares {
            if let Some(child) = self
                .spare_children
                .remove(&outpost)
                .and_then(|spare| spare.child.into_running())
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
            .is_some_and(|listener| listener.child.running().is_some())
        {
            self.end_listener("Verbatim is exiting");
        }
    }

    /// Once Verbatim is exiting, every child has been ended and has
    /// finished ending, and every pipe has finished, sends the summary and
    /// says the owner is done. Until then the relay keeps running, since a
    /// child's ending needs its shutdown message written. A generation
    /// still launching ends as its launch reports.
    fn finish_shutdown(&mut self) -> bool {
        if self.shutting_down.is_none()
            || self
                .records
                .values()
                .any(|generation| matches!(generation, Generation::Launching { .. }))
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

    /// Ends the focus listener, shutting its process down cleanly, for
    /// `why`.
    fn end_listener(&mut self, why: &str) {
        let Some(listener) = self.listener.take() else {
            return;
        };
        // Core answers what it asked of the listener; at exit nothing waits.
        if self.shutting_down.is_none() {
            self.outbox.send(OutpostMessage::ListenerEnded {
                listener: listener.outpost,
                sealed: Sealed::new(),
            });
        }
        if let Some(child) = listener.child.into_running() {
            self.retire_child(
                child,
                format!("focus listener {}", listener.outpost),
                why.to_owned(),
            );
        }
    }
    /// Pings every running child, ends the listener when it has stopped
    /// answering, and finds the applications held that have exited. Core
    /// judges an outpost's missed pings and abandoned workers
    /// (`docs/design/readers-tell-core-asks-build1.md`, "The outpost
    /// generation record").
    fn heartbeat(&mut self) {
        self.relay.commands().forget_finished();
        if self.shutting_down.is_some() {
            return;
        }
        let ping = CoreToOutpost::ping();
        let at_us = now_us();
        let limits = self.wedge_limits();
        let exited: Vec<Pid> = self
            .apps
            .iter()
            .filter(|(_, target)| target.has_exited())
            .map(|(pid, _)| *pid)
            .collect();
        for pid in exited {
            self.application_exited(pid, "seen by the supervisor");
        }
        for generation in self.records.values() {
            let (Generation::Starting { child, .. } | Generation::Ready { child, .. }) = generation
            else {
                continue;
            };
            let _ = child.writer.push(Outgoing::Unbounded(ping.clone()));
        }
        self.spare_heartbeat(at_us, &ping);

        match &self.listener {
            None => self.start_listener(),
            Some(listener) => {
                if let Some(child) = listener.child.running() {
                    let decision = listener.liveness.wedged(at_us, limits);
                    if decision == Some(WedgeReason::MissedHeartbeats) {
                        tracing::warn!("focus listener stopped answering; replacing it");
                        self.end_listener("it stopped answering pings; it is replaced");
                        self.listener_gone();
                        self.start_listener();
                    } else {
                        let _ = child
                            .writer
                            .push(Outgoing::Unbounded(CoreToListener::ping()));
                    }
                }
            }
        }
    }

    /// Pings every spare waiting and ends any that has missed its pings, as
    /// a spare's failure, at `now_us` on the run's shared clock.
    fn spare_heartbeat(&mut self, now_us: u64, ping: &CoreToOutpost) {
        let limits = self.wedge_limits();
        let mut wedged = Vec::new();
        for (&outpost, spare) in &self.spare_children {
            let Some(child) = spare.child.running() else {
                continue;
            };
            if spare.liveness.wedged(now_us, limits).is_some() {
                wedged.push(outpost);
            } else {
                let _ = child.writer.push(Outgoing::Unbounded(ping.clone()));
            }
        }
        for outpost in wedged {
            tracing::warn!(%outpost, "ending a spare outpost that stopped answering pings");
            let mut up = Duration::ZERO;
            if let Some(spare) = self.spare_children.remove(&outpost) {
                up = up_since(spare.started_us);
                if let Some(child) = spare.child.into_running() {
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
fn ask_idle(writer: &WriterHandle<CoreToOutpost>, round: u64) {
    let query = CoreToOutpost::IdleQuery { round };
    if let Err(error) = writer.push(Outgoing::Unbounded(query)) {
        tracing::debug!(%error, "the idle query could not reach an outpost");
    }
}
/// The launch helper's work: launch the process and make its writer on the
/// relay's port; for an outpost, register the writer. Returns the launch,
/// with the pipe the relay will read.
fn launch_child(
    exe_path: &std::path::Path,
    (options, listener): (
        verbatim_process::arguments::OutpostOptions,
        ListenerArguments,
    ),
    (outpost, role): (OutpostId, Role),
    (writers, commands): (&Writers, &CommandPipes),
) -> io::Result<Launch> {
    tracing::info!(%outpost, ?role, "launching");
    let (launched, pipes) = process::launch(exe_path, role, options, listener)?;
    tracing::info!(%outpost, process_id = %launched.process_id, ?role, "launched");
    // A failure from here drops the process's job, which kills it.
    let (child, source) = match role {
        Role::Listener => (
            LaunchedChild::Listener(Child {
                process: launched,
                writer: commands.open(outpost, pipes.to_child)?,
            }),
            Source::Listener,
        ),
        Role::Outpost(_) | Role::Spare(_) => {
            let writer = commands.open(outpost, pipes.to_child)?;
            writers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(outpost, writer.clone());
            let source = match role {
                Role::Outpost(pid) => Source::Outpost(pid),
                Role::Spare(_) | Role::Listener => Source::Spare,
            };
            (
                LaunchedChild::Outpost(Child {
                    process: launched,
                    writer,
                }),
                source,
            )
        }
    };
    Ok(Launch {
        child,
        from_child: pipes.from_child,
        source,
    })
}
