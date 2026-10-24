//! The Core-side supervisor (architecture section 1, decisions D9 and D13;
//! outpost redesign, "The supervisor").
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
//! - The supervisor therefore runs on one thread, the relay, plus a helper
//!   that parses large frames for the app ([`Frames`]).
//! - It keeps two spare outposts started and waiting, with no application
//!   (`docs/design/focus-pipeline.md`, section 19), from the moment Core's
//!   own outpost is ready: a start for an application takes the oldest,
//!   sends it `Assign`, and tells the app the incarnation `Started`; a
//!   spare given never returns, and its replacement starts at once.
//! - Every outpost failure takes one path, whatever its cause: a crash, a
//!   hook that fails, missed pings, piled-up abandoned workers, or a spare
//!   that dies waiting. The outpost is ended; an application it served is
//!   given the next spare, its held facts with it; and nothing is given up,
//!   repeated failures being spaced by an exponential backoff
//!   ([`policy::Backoff`]).
//!
//! The app hears of each outpost incarnation through [`OutpostMessage`]:
//! `Started` before any of its messages, then its messages, then `Ended`.
//! When an outpost exits on its own, `Ended` follows everything it wrote; when
//! the owner kills or retires it, `Ended` is sent at once, and the relay
//! drops anything the child wrote afterwards. One application's
//! incarnations come in the order the supervisor ended and started them:
//! an outpost's `Ended` before its replacement's `Started`, even while
//! [`Frames`] holds that `Ended` behind a large frame.

mod frames;
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
use verbatim_model::{OutpostId, Pid, TraceId};
use verbatim_process::port_queue::Progress;

use crate::protocol::{OutpostToSupervisor, SupervisorToOutpost};

pub use frames::{Frame, Frames, LARGE_FRAME_BYTES, Parsed};
pub use ignored::{IGNORE_PIDS_ENV, IgnoredProcesses, parse_pids};
pub use retire::ShutdownSummary;
pub use writer::QueueError;

pub(crate) use process::image_stem;

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

/// Why an outpost incarnation ended.
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

/// Where the supervisor delivers its [`OutpostMessage`]s. The relay, the
/// launch helpers and the helper parsing large frames deliver through the
/// same sink, so a sink that puts each message in one queue gives the app
/// every outpost's messages in the order they reached the supervisor. The
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
    /// app took it, has gone, or refused it for now ([`Undelivered`]):
    /// once the app has gone, the helper parsing large frames stops, and a
    /// result it refused is delivered again.
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
/// ([`crate::protocol::OutpostToSupervisor::ForegroundWindow`]).
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

/// A message from the supervisor to the app.
#[derive(Debug)]
pub enum OutpostMessage {
    /// An outpost incarnation started. Sent before any of its messages.
    Started {
        /// The new incarnation.
        outpost: OutpostId,
        /// The application it watches.
        target_pid: Pid,
    },
    /// A message an outpost sent, tagged with its target pid and the outpost
    /// incarnation whose pipe it arrived on.
    Event {
        /// The application the outpost watches.
        pid: Pid,
        /// The incarnation whose pipe the message arrived on.
        outpost: OutpostId,
        /// How many messages carrying node ids the outpost has sent up to and
        /// including this one: what the app acknowledges in
        /// [`Supervisor::leave_nodes_held`] once it has handled the message.
        position: u64,
        /// The message, boxed so every channel send is small, not sized to
        /// the largest reply.
        message: Box<OutpostToSupervisor>,
    },
    /// A spare outpost said it is ready, and `waiting` spares are ready and
    /// waiting now (`docs/design/focus-pipeline.md`, section 19). Verbatim
    /// is ready for the end-to-end harness once [`SPARES`] are, so every
    /// scenario's applications take a spare.
    SpareReady {
        /// The spare's incarnation: the id it has once given.
        spare: OutpostId,
        /// The spare's own process id.
        outpost_pid: Pid,
        /// How many spares are ready and waiting.
        waiting: usize,
    },
    /// The focus listener is ready. When it is a replacement for one that
    /// ended, facts were lost in the gap, so the app asks the foreground
    /// application for its current focus.
    ListenerReady {
        /// Whether this listener replaced one that ended.
        replacement: bool,
    },
    /// A menu closed, menu mode ended, or the Alt+Tab switcher closed,
    /// somewhere on the desktop, at `ended_at_ms`: unless a focus observed
    /// since has been applied, the app asks the foreground application for
    /// its focused control (NVDA's fake focus).
    MenuOrSwitchEnded {
        /// When it ended, in milliseconds since the Unix epoch.
        ended_at_ms: u64,
    },
    /// A window came to the system's foreground, as the focus listener
    /// learned it: the app gives it to Core, which decides the keyboard
    /// hook's view of the foreground (architecture section 5).
    ForegroundWindow(ForegroundReport),
    /// An outpost incarnation ended. Its node ids are dead from now on.
    Ended {
        /// The incarnation that ended.
        outpost: OutpostId,
        /// The application it watched.
        target_pid: Pid,
        /// Why it ended.
        reason: EndReason,
    },
    /// The incarnation `outpost` given to `target_pid` failed before its
    /// `Ready` (`docs/design/focus-pipeline.md`, section 19.5), and the
    /// application's held facts, and the idle query held for it, go to the
    /// incarnation `next`: the next spare, or an outpost launched for the
    /// application. Sent when `next` is given, at once or when the
    /// failure's backoff ends, before `outpost`'s `Ended`, which follows
    /// when the app heard it started, and before `next`'s `Started`, which
    /// follows at once or once its launch reports. In an
    /// idle round that asked `outpost`, `next` answers in its place; and
    /// nothing the application reported is lost, so the app asks it for
    /// nothing again.
    Reincarnated {
        /// The incarnation that failed.
        outpost: OutpostId,
        /// The incarnation that takes its place.
        next: OutpostId,
        /// The application.
        target_pid: Pid,
    },
    /// An outpost was wanted for `target_pid`, and none was started: the
    /// application's process has exited, or cannot be opened to be held,
    /// or is one Verbatim ignores ([`IgnoredProcesses`]). Nothing should
    /// wait for one. An application whose outposts fail is never given up
    /// and never reported so: each failure's replacement only waits for
    /// its backoff.
    NotWatched {
        /// The application.
        target_pid: Pid,
    },
    /// A frame from an outpost's pipe, read whole by the relay and not yet
    /// parsed: the app passes it through [`Frames`], which turns it into
    /// the [`OutpostMessage::Event`] it carries.
    Frame(Frame),
    /// A large frame parsed by the helper of [`Frames`], ordered among the
    /// app's inputs where the parse finished: the app passes it through
    /// [`Frames`] too.
    Parsed(Parsed),
    /// The idle barrier's round `round` (`docs/design/communication.md`,
    /// section 6.7; [`Supervisor::query_idle`]): the listener has answered,
    /// or has gone, and every outpost in `outposts` is asked now, each
    /// behind every fact relayed to it before, one still starting once it
    /// is ready. Sent before any of them is asked, so it reaches the app
    /// ahead of every answer. Each answers
    /// [`OutpostToSupervisor::IdleAnswer`] as a message of its own, or
    /// ends ([`OutpostMessage::Ended`]), which counts as its answer, unless
    /// it was a spare that failed before its `Ready`: then the incarnation
    /// [`OutpostMessage::Reincarnated`] names answers in its place.
    IdleQueried {
        /// The round.
        round: u64,
        /// The outpost incarnations asked.
        outposts: Vec<OutpostId>,
    },
}

/// The live writers, by outpost id, so the reducer thread can queue a
/// command for an outpost without going through the owner. The owner adds a
/// writer when an outpost starts and removes it when the outpost ends; the
/// lock is held only to look one up, never across any I/O.
type Writers = Arc<Mutex<std::collections::HashMap<OutpostId, writer::WriterHandle>>>;

/// Starts, routes to, and ends outposts and the focus listener.
pub struct Supervisor {
    owner: relay::Mailbox<owner::OwnerEvent>,
    writers: Writers,
    ignored: Arc<IgnoredProcesses>,
    /// The sink, for the helper of [`Supervisor::frames`].
    events: EventSink,
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
            events_tx: events.clone(),
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
            events,
        })
    }

    /// What the app passes every message this supervisor delivers through,
    /// on the thread that takes them: it parses each outpost's frame into
    /// the message it carries, a large one on a helper thread, keeping each
    /// outpost's order ([`Frames`]).
    #[must_use]
    pub fn frames(&self) -> Frames {
        Frames::new(self.events.clone(), self.owner.clone())
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

    /// Queues `command` for one outpost incarnation without waiting. A node
    /// id names the incarnation that issued it, so a command for a replaced
    /// outpost's node can never reach its successor.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Closed`] if that incarnation has ended and
    /// [`QueueError::Full`] if its queue is full; either way the command was
    /// not sent and the caller gives its asker an outcome at once.
    pub fn send_to_outpost(
        &self,
        outpost: OutpostId,
        command: SupervisorToOutpost,
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

    /// Leaves for one outpost incarnation the list of its nodes and text
    /// anchors Core still holds, with the position of the last of its
    /// messages the app has handled (`docs/design/communication.md`, step
    /// 11). Nothing is written for it: the next command written to that
    /// incarnation, a request from the app or the relay's ping within
    /// three seconds, carries it after its own frame, in the same write. A
    /// newer list replaces one not yet sent. Nothing happens if the
    /// incarnation has ended.
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
            writer.leave_held(SupervisorToOutpost::NodesHeld {
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

    /// Tells the focus listener and `outposts` the focus Core holds,
    /// `focus`, once Core has judged the focus event traced `judged` or
    /// its focus has changed ([`SupervisorToOutpost::CoreFocus`]): the
    /// listener whole, and each outpost only when the focus is its own.
    pub fn core_focus(
        &self,
        judged: Option<TraceId>,
        focus: Option<crate::protocol::HeldFocus>,
        outposts: Vec<OutpostId>,
    ) {
        let _ = self.owner.send(owner::OwnerEvent::CoreFocus {
            judged,
            focus,
            outposts,
        });
    }
}
