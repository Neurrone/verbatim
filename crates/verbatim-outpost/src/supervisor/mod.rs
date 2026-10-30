//! The Core-side supervisor (architecture section 1, decisions D9 and D13).
//!
//! The supervisor starts outposts and the focus listener, routes the
//! listener's facts to the right outpost, and ends children that crash, hang,
//! or are no longer needed. Its parts:
//!
//! - One lifecycle owner thread ([`owner`]) makes every lifecycle decision and
//!   owns the per-application records and the listener record. Launch
//!   helpers, retirements, and the app only report facts to it through one
//!   mailbox; nothing else changes a record, and there is no shared map
//!   lock. Its heartbeat and sweep run when its wait's deadline passes.
//! - The owner thread is also the relay ([`relay`];
//!   `docs/design/communication.md`, section 6.2): it waits only on an I/O
//!   completion port, reads the listener's pipe and routes each fact in the
//!   pass that read it, writing it to its outpost there and then, so a fact
//!   crosses verbatim.exe with one thread's wake-up; it reads every
//!   outpost's pipe, handles pongs, `Ready`, `TargetExited` and the pipe's
//!   end itself, and delivers every other frame whole and unparsed
//!   ([`OutpostMessage::Frame`]), which the app parses through [`Frames`];
//!   and it finishes every child's writes that pended. It never blocks:
//!   launching a process,
//!   ending one, and checking whether an application is hung run on threads
//!   of their own and report back. The owner records the application as
//!   starting before the launch, so a second fact meanwhile is held, not a
//!   second launch.
//! - Each child has a writer with a bounded queue ([`writer`]): whichever
//!   thread sends a command writes it at once when no write is in flight,
//!   so the reducer thread sends a query without waiting and without a
//!   hand-off, and a write the pipe cannot take at once is finished by the
//!   relay.
//! - The relay watches the reducer too: at each heartbeat it reads how many
//!   of the app's inputs wait and how many the reducer has taken, and
//!   reports a reducer that has taken nothing for three ping intervals
//!   while inputs waited ([`EventSink::watched`]). Since the relay reads
//!   the pongs itself, a stalled reducer never makes an outpost look
//!   wedged (section 6.8).
//! - The supervisor therefore runs on one thread, the relay; the app parses
//!   every frame on its reducer thread ([`Frames`]).
//! - It keeps two spare outposts started and waiting, with no application
//!   (`docs/design/focus-pipeline.md`, section 19), from the moment Core's
//!   own outpost is ready: a start for an application takes the oldest,
//!   sends it `Assign`, and tells the app the generation `Started`; a
//!   spare given never returns, and its replacement starts at once.
//! - Every outpost failure takes one path, whatever its cause: a crash, a
//!   hook that fails, missed pings, piled-up abandoned workers, or a spare
//!   that dies waiting. The outpost is ended; an application it served is
//!   given the next spare, its held facts with it; and nothing is given up,
//!   repeated failures being spaced by an exponential backoff
//!   ([`policy::Backoff`]).
//!
//! The app hears of each outpost generation through [`OutpostMessage`]:
//! `Started` before any of its messages, then its messages, then `Ended`.
//! When an outpost exits on its own, `Ended` follows everything it wrote; when
//! the owner kills or retires it, `Ended` is sent at once, and the relay
//! drops anything the child wrote afterwards. One application's
//! generations come in the order the supervisor ended and started them:
//! an outpost's `Ended` before its replacement's `Started`.

mod frames;
mod generation;
mod ignored;
mod owner;
mod policy;
mod process;
mod relay;
mod retire;
mod writer;

use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::Sender;
use verbatim_model::{OutpostId, Pid};
use verbatim_process::port_queue::Progress;

use verbatim_model::protocol::{CoreToOutpost, OutpostToCore};

pub use frames::{Frame, Frames};
pub use ignored::{IGNORE_PIDS_ENV, IgnoredProcesses, parse_pids};
pub use retire::ShutdownSummary;
pub use writer::QueueError;

pub(crate) use process::Target;

/// How often every child is pinged.
const PING_INTERVAL: Duration = Duration::from_secs(3);

/// How many ping intervals may pass without a pong before a child is ended:
/// three intervals, nine seconds.
const MISSED_PONG_THRESHOLD: u32 = 3;

/// How many abandoned workers end an outpost, unless its application's
/// windows are reported hung.
const ABANDONED_WORKER_LIMIT: usize = 8;

/// How long an application must go without attention before its outpost is
/// retired (risk R2's memory-use mitigation).
const IDLE_RETIREMENT: Duration = Duration::from_mins(2);

/// How often idle outposts are looked for.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// How many spare outposts the supervisor keeps waiting
/// (`docs/design/focus-pipeline.md`, section 19).
pub const SPARES: usize = 2;

/// How long the replacement of an outpost's second failure in a row
/// waits; each failure after it waits twice as long as the one before, up
/// to [`BACKOFF_LIMIT`] (`docs/design/focus-pipeline.md`, section 19.5).
/// The first failure is replaced at once.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);

/// The longest a failed outpost's replacement waits.
const BACKOFF_LIMIT: Duration = Duration::from_secs(30);

/// How long an outpost, or a spare, must have been up for its failure to
/// count as the first again: a minute, twenty heartbeats, long past the
/// start and first focus in which a failure that repeats shows itself.
const STAYED_UP: Duration = Duration::from_mins(1);

/// The backoff's three times, as [`policy::Backoff::failed`] takes them.
const BACKOFF: (Duration, Duration, Duration) = (BACKOFF_FIRST, BACKOFF_LIMIT, STAYED_UP);

/// Why an outpost generation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// Its pipe closed: it exited or crashed.
    Exited,
    /// The owner ended it for not answering pings or for piling up abandoned
    /// workers: it was asked to shut down and killed if it did not exit in
    /// time.
    Killed,
    /// The owner retired it: its application had not held attention for two
    /// minutes and Core held none of its nodes.
    Retired,
    /// Its application exited, so the owner shut it down.
    TargetExited,
}

impl std::fmt::Display for EndReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EndReason::Exited => "exited",
            EndReason::Killed => "killed",
            EndReason::Retired => "retired",
            EndReason::TargetExited => "target exited",
        })
    }
}

/// Where the supervisor delivers its [`OutpostMessage`]s. Only the relay
/// delivers through it, so a sink that puts each message in one queue gives
/// the app every outpost's messages in the order they reached the
/// supervisor. The
/// app's sink posts them to the reducer thread's port, among its keys and
/// commands (`docs/design/communication.md`, section 4.5); a channel of
/// [`OutpostMessage`]s converts into a sink, for tests.
#[derive(Clone)]
pub struct EventSink {
    deliver: Arc<dyn Fn(OutpostMessage) -> Result<(), Undelivered> + Send + Sync>,
    watch: Option<Arc<ReducerWatch>>,
}

/// How the relay watches the thread taking the sink's messages: what it
/// reads at each heartbeat, and where it reports a stall.
struct ReducerWatch {
    progress: Box<dyn Fn() -> Progress + Send + Sync>,
    stalled: Box<dyn Fn(ReducerStall) + Send + Sync>,
}

/// A stall of the thread taking the sink's messages, the reducer: it has
/// taken none of its inputs for at least three ping intervals while some
/// waited (`docs/design/communication.md`, section 6.8). It is a fault of
/// its own, and nothing is ended for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReducerStall {
    /// How long it has taken nothing while inputs waited, as the relay's
    /// heartbeats measured it.
    pub stalled_for: Duration,
    /// How many of its inputs waited at the last heartbeat.
    pub waiting: u64,
}

/// Why an [`EventSink`] did not deliver a message.
#[derive(Debug)]
pub enum Undelivered {
    /// The app has gone: nothing it is sent is taken any more.
    Gone,
    /// The app is there, but the message could not be put in line now,
    /// such as a completion port refusing the packet, which only a system
    /// out of memory does; it is given back, and a later attempt may
    /// succeed.
    Refused(OutpostMessage),
}

impl EventSink {
    /// A sink that hands each message to `deliver`, which says whether the
    /// app took it, has gone, or refused it for now ([`Undelivered`]).
    #[must_use]
    pub fn new(
        deliver: impl Fn(OutpostMessage) -> Result<(), Undelivered> + Send + Sync + 'static,
    ) -> Self {
        Self {
            deliver: Arc::new(deliver),
            watch: None,
        }
    }

    /// This sink, with the thread taking its messages watched: at each
    /// heartbeat the relay reads `progress`, how many of that thread's
    /// inputs have been posted and how many taken, and once it has taken
    /// none for three ping intervals while some waited, reports the stall
    /// to `stalled`, once until it takes something again.
    #[must_use]
    pub fn watched(
        self,
        progress: impl Fn() -> Progress + Send + Sync + 'static,
        stalled: impl Fn(ReducerStall) + Send + Sync + 'static,
    ) -> Self {
        Self {
            deliver: self.deliver,
            watch: Some(Arc::new(ReducerWatch {
                progress: Box::new(progress),
                stalled: Box::new(stalled),
            })),
        }
    }

    /// Delivers `message`.
    ///
    /// # Errors
    ///
    /// Returns why it was not delivered ([`Undelivered`]).
    pub fn send(&self, message: OutpostMessage) -> Result<(), Undelivered> {
        (self.deliver)(message)
    }

    /// The watched thread's progress, if it is watched.
    fn progress(&self) -> Option<Progress> {
        self.watch.as_ref().map(|watch| (watch.progress)())
    }

    /// Reports a stall of the watched thread.
    fn stalled(&self, stall: ReducerStall) {
        if let Some(watch) = &self.watch {
            (watch.stalled)(stall);
        }
    }
}

impl From<Sender<OutpostMessage>> for EventSink {
    fn from(events_tx: Sender<OutpostMessage>) -> Self {
        Self::new(move |message| events_tx.send(message).map_err(|_| Undelivered::Gone))
    }
}

impl std::fmt::Debug for EventSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EventSink")
    }
}

/// The window in the foreground as the focus listener reported it
/// ([`verbatim_model::protocol::ListenerToCore::ForegroundWindow`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForegroundReport {
    /// The window.
    pub window: isize,
    /// The thread that raised its event, the console host's own for a
    /// console window; 0 for a console window whose thread no event has
    /// named yet.
    pub thread: u32,
    /// Whether it is a console host window.
    pub console: bool,
    /// The integrity level of the process that owns it, or `None` when it
    /// cannot be read.
    pub integrity: Option<u32>,
}

/// The seal on the lifecycle notices of [`OutpostMessage`]: only the
/// supervisor's owner thread makes one, so no other code can announce a
/// generation started or ended, or an idle round asked, and the app takes
/// every such notice as the supervisor's own. A test of the app makes one
/// with [`Sealed::for_tests`], behind the `lifecycle-for-tests` feature.
#[derive(Debug, PartialEq, Eq)]
pub struct Sealed(());

impl Sealed {
    /// The seal, for the supervisor's own notices.
    fn new() -> Self {
        Self(())
    }

    /// The seal, for a test that hands the app notices of its own.
    #[cfg(feature = "lifecycle-for-tests")]
    #[must_use]
    pub fn for_tests() -> Self {
        Self(())
    }
}

/// A message from the supervisor to the app. Its lifecycle notices carry a
/// [`Sealed`], so only the supervisor makes them; a pattern that
/// destructures one ends in `..`.
#[derive(Debug)]
pub enum OutpostMessage {
    /// An outpost generation started. Sent before any of its messages.
    Started {
        /// The new generation.
        outpost: OutpostId,
        /// The application it watches.
        target_pid: Pid,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// A message an outpost sent, tagged with its target pid and the outpost
    /// generation whose pipe it arrived on.
    Event {
        /// The application the outpost watches.
        pid: Pid,
        /// The generation whose pipe the message arrived on.
        outpost: OutpostId,
        /// How many messages carrying node ids the outpost has sent up to and
        /// including this one: what the app acknowledges in
        /// [`Supervisor::leave_nodes_held`] once it has handled the message.
        position: u64,
        /// The message, boxed so every channel send is small, not sized to
        /// the largest reply.
        message: Box<OutpostToCore>,
    },
    /// A spare outpost said it is ready, and `waiting` spares are ready and
    /// waiting now (`docs/design/focus-pipeline.md`, section 19). Verbatim
    /// is ready for the end-to-end harness once [`SPARES`] are, so every
    /// scenario's applications take a spare.
    SpareReady {
        /// The spare's generation: the id it has once given.
        spare: OutpostId,
        /// The spare's own process id.
        outpost_pid: Pid,
        /// How many spares are ready and waiting.
        waiting: usize,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// The focus listener is ready. When it is a replacement for one that
    /// ended, facts were lost in the gap, so the app asks the foreground
    /// application for its current focus.
    ListenerReady {
        /// Whether this listener replaced one that ended.
        replacement: bool,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// A menu closed, menu mode ended, or the Alt+Tab switcher closed,
    /// somewhere on the desktop, at `ended_at_us`: unless a focus observed
    /// since has been applied, the app asks the foreground application for
    /// its focused control (NVDA's fake focus).
    MenuOrSwitchEnded {
        /// When it ended, in microseconds since the Unix epoch on the run's
        /// shared clock.
        ended_at_us: u64,
    },
    /// A window came to the system's foreground, as the focus listener
    /// learned it: the app gives it to Core, which decides the keyboard
    /// hook's view of the foreground (architecture section 5).
    ForegroundWindow(ForegroundReport),
    /// An outpost generation ended. Its node ids are dead from now on.
    Ended {
        /// The generation that ended.
        outpost: OutpostId,
        /// The application it watched.
        target_pid: Pid,
        /// Why it ended.
        reason: EndReason,
        /// Whether a next generation follows for the application, at once
        /// or after a failure's backoff: the supervisor's decision, made
        /// as it ended this one, so nothing else infers it. When none
        /// follows, [`OutpostMessage::NotWatched`] comes next.
        follows: bool,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// The generation `outpost` given to `target_pid` failed before its
    /// `Ready` (`docs/design/focus-pipeline.md`, section 19.5), and the
    /// application's held facts, and the idle query held for it, go to the
    /// generation `next`: the next spare, or an outpost launched for the
    /// application. Sent when `next` is given, at once or when the
    /// failure's backoff ends, before `outpost`'s `Ended`, which follows
    /// when the app heard it started, and before `next`'s `Started`, which
    /// follows at once or once its launch reports. In an
    /// idle round that asked `outpost`, `next` answers in its place; and
    /// nothing the application reported is lost, so the app asks it for
    /// nothing again.
    Regenerated {
        /// The generation that failed.
        outpost: OutpostId,
        /// The generation that takes its place.
        next: OutpostId,
        /// The application.
        target_pid: Pid,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// No outpost watches `target_pid` now, nor will one until something
    /// asks for one again: one was wanted and none was started, its
    /// process having exited, or being one that cannot be opened to be
    /// held, or one Verbatim ignores ([`IgnoredProcesses`]); or its record
    /// was removed, after the `Ended` of every generation the app heard
    /// started, as its application exited or its outpost retired, a record
    /// waiting for a failure's backoff among them. Nothing should wait for
    /// one. An application whose outposts fail is never given up and never
    /// reported so: each failure's replacement only waits for its backoff.
    NotWatched {
        /// The application.
        target_pid: Pid,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// A frame from an outpost's pipe, read whole by the relay and not yet
    /// parsed: the app passes it through [`Frames`], which turns it into
    /// the [`OutpostMessage::Event`] it carries.
    Frame(Frame),
    /// The idle barrier's round `round` (`docs/design/communication.md`,
    /// section 6.7; [`Supervisor::query_idle`]): the listener has answered,
    /// or has gone, and every outpost in `outposts` is asked now, each
    /// behind every fact relayed to it before, one still starting once it
    /// is ready. Sent before any of them is asked, so it reaches the app
    /// ahead of every answer. Each answers
    /// [`OutpostToCore::IdleAnswer`] as a message of its own, or
    /// ends ([`OutpostMessage::Ended`]), which counts as its answer, unless
    /// it was a spare that failed before its `Ready`: then the generation
    /// [`OutpostMessage::Regenerated`] names answers in its place.
    IdleQueried {
        /// The round.
        round: u64,
        /// The outpost generations asked.
        outposts: Vec<OutpostId>,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
}

/// The live writers, by outpost id, so the reducer thread can queue a
/// command for an outpost without going through the owner. The owner adds a
/// writer when an outpost starts and removes it when the outpost ends; the
/// lock is held only to look one up, never across any I/O.
type Writers =
    Arc<Mutex<std::collections::HashMap<OutpostId, writer::WriterHandle<CoreToOutpost>>>>;

/// Starts, routes to, and ends outposts and the focus listener.
pub struct Supervisor {
    owner: relay::Mailbox<owner::OwnerEvent>,
    writers: Writers,
    ignored: Arc<IgnoredProcesses>,
}

impl Supervisor {
    /// Starts the supervisor: its lifecycle owner thread and the focus
    /// listener. Outpost messages and lifecycle notices go to `events`.
    /// The outpost executable is resolved next to the current executable.
    /// Every outpost is launched with `options`. The processes `ignored`
    /// holds are ignored entirely: the listener drops their facts, and no
    /// outpost is ever started for them.
    ///
    /// # Errors
    ///
    /// Returns an error if the current executable path cannot be determined
    /// or the relay's completion port or the owner thread cannot be made.
    pub fn new(
        events: impl Into<EventSink>,
        options: crate::OutpostOptions,
        ignored: Arc<IgnoredProcesses>,
    ) -> io::Result<Self> {
        let exe_path = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("current exe has no parent directory"))?
            .join("verbatim-outpost.exe");
        Self::with_executable(events, options, exe_path, retire::SHUTDOWN_LIMIT, ignored)
    }

    /// [`Supervisor::new`], launching every child from `exe_path` and giving
    /// each `shutdown_limit` to exit after the shutdown message before it is
    /// killed, and ignoring the processes `ignored` holds. For tests that
    /// launch a stand-in for the outpost binary; Verbatim uses
    /// [`Supervisor::new`].
    ///
    /// # Errors
    ///
    /// Returns an error if the relay's completion port or the owner thread
    /// cannot be made.
    pub fn with_executable(
        events: impl Into<EventSink>,
        options: crate::OutpostOptions,
        exe_path: std::path::PathBuf,
        shutdown_limit: Duration,
        ignored: Arc<IgnoredProcesses>,
    ) -> io::Result<Self> {
        // The launch's log directory was prepared by the app at startup,
        // before any child (the synthesizer host first) began writing to it.
        let writers: Writers = Arc::default();
        let relay = relay::Relay::new()?;
        let (owner_tx, owner_rx) = relay::mailbox(std::sync::Arc::clone(relay.port()));
        let events: EventSink = events.into();
        owner::start(owner::Setup {
            exe_path,
            options,
            shutdown_limit,
            events_tx: events,
            writers: Arc::clone(&writers),
            own_tx: owner_tx.clone(),
            events: owner_rx,
            relay,
            ignored: Arc::clone(&ignored),
        })?;
        Ok(Self {
            owner: owner_tx,
            writers,
            ignored,
        })
    }

    /// What the app passes every message this supervisor delivers through,
    /// on the thread that takes them: it parses each outpost's frame into
    /// the message it carries, keeping each outpost's order ([`Frames`]).
    #[must_use]
    pub fn frames(&self) -> Frames {
        Frames::new(self.owner.clone())
    }

    /// Whether `pid` names a process Verbatim ignores entirely
    /// ([`IgnoredProcesses`]): nothing asks it for anything.
    #[must_use]
    pub fn is_ignored(&self, pid: Pid) -> bool {
        self.ignored.contains(pid)
    }

    /// Starts an outpost for `target_pid` if none exists, without asking it
    /// to report anything, giving it a spare when one waits. Used at
    /// startup for Core's own process, before any spare exists, so its
    /// outpost is warm before the first gesture: a cold spawn races a real
    /// keypress sent right after Verbatim's menu opens; the spares are
    /// started once it is ready. Also used for the taskbar's process when
    /// the tray list dialog asks for the shell's items and that process has
    /// no ready outpost. An application whose process has exited
    /// gets none, and the app hears
    /// [`OutpostMessage::NotWatched`].
    pub fn ensure_spawned(&self, target_pid: Pid) {
        let _ = self
            .owner
            .send(owner::OwnerEvent::EnsureSpawned(target_pid));
    }

    /// Queues `command` for one outpost generation without waiting. A node
    /// id names the generation that issued it, so a command for a replaced
    /// outpost's node can never reach its successor.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Closed`] if that generation has ended and
    /// [`QueueError::Full`] if its queue is full; either way the command was
    /// not sent and the caller gives its asker an outcome at once.
    pub fn send_to_outpost(
        &self,
        outpost: OutpostId,
        command: CoreToOutpost,
    ) -> Result<(), QueueError> {
        let writer = self
            .writers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&outpost)
            .cloned()
            .ok_or(QueueError::Closed)?;
        writer.push(writer::Outgoing::Other(command, writer::Received::now()))
    }

    /// Leaves for one outpost generation the list of its nodes and text
    /// anchors Core still holds, with the position of the last of its
    /// messages the app has handled (`docs/design/communication.md`, step
    /// 11). Nothing is written for it: the next command written to that
    /// generation, a request from the app or the relay's ping within
    /// three seconds, carries it after its own frame, in the same write. A
    /// newer list replaces one not yet sent. Nothing happens if the
    /// generation has ended.
    pub fn leave_nodes_held(
        &self,
        outpost: OutpostId,
        nodes: Vec<u64>,
        anchors: Vec<u64>,
        acknowledged: u64,
    ) {
        let writer = self
            .writers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&outpost)
            .cloned();
        if let Some(writer) = writer {
            writer.leave_held(CoreToOutpost::NodesHeld {
                nodes,
                anchors,
                acknowledged,
            });
        }
    }

    /// Writes for one outpost generation the list of its nodes and text
    /// anchors Core still holds, with the position of the last of its
    /// messages the app has handled, behind every command waiting for it
    /// now, in place of a list left and not yet sent
    /// ([`leave_nodes_held`](Self::leave_nodes_held)): the app writes it
    /// after it handled a focus report, or a message that changed the nodes
    /// Core holds there, so the outpost releases what Core let go of within
    /// one pipe hop (`docs/design/ownership-one-judge.md`, "Manageability").
    /// Nothing happens if the generation has ended.
    pub fn send_nodes_held(
        &self,
        outpost: OutpostId,
        nodes: Vec<u64>,
        anchors: Vec<u64>,
        acknowledged: u64,
    ) {
        let writer = self
            .writers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&outpost)
            .cloned();
        if let Some(writer) = writer {
            writer.send_held(CoreToOutpost::NodesHeld {
                nodes,
                anchors,
                acknowledged,
            });
        }
    }

    /// Shuts down every outpost and the focus listener for Verbatim's exit,
    /// and returns once every one has ended: each is sent the shutdown
    /// message and given its time limit to exit, and killed through its job
    /// only if it has not, which is logged with the reason. No child is
    /// started afterwards. Returns how every child that ended during this
    /// Verbatim's life ended, the ones ended now included.
    #[must_use]
    pub fn shutdown(&self) -> ShutdownSummary {
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        if self
            .owner
            .send(owner::OwnerEvent::Shutdown(reply_tx))
            .is_err()
        {
            return ShutdownSummary::default();
        }
        reply_rx.recv().unwrap_or_default()
    }

    /// Tells the owner the views the app derives from the reducer state:
    /// the application holding attention, and the outposts in which the
    /// state holds nodes, which decide retirement.
    pub fn note_views(&self, attention: Option<Pid>, holding: BTreeSet<OutpostId>) {
        let _ = self
            .owner
            .send(owner::OwnerEvent::Views { attention, holding });
    }

    /// Runs the idle barrier's round `round` through the children
    /// (`docs/design/communication.md`, section 6.7): the listener is asked
    /// first, behind every command sent to it before; once it answers, or
    /// if it has gone, every outpost, ready or starting, is asked behind
    /// every fact relayed to it before, and the app hears which
    /// ([`OutpostMessage::IdleQueried`]). An outpost still being launched
    /// is waited for first, so the round covers the facts held for it.
    pub fn query_idle(&self, round: u64) {
        let _ = self.owner.send(owner::OwnerEvent::IdleRound(round));
    }
}
