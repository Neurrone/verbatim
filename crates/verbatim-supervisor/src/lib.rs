//! The Core-side supervisor (architecture section 1, decisions D9 and D13):
//! the process owner, reduced to process facts and their effects
//! (`docs/design/readers-tell-core-asks-build1.md`, "The outpost
//! generation record"). Core's reducer owns the outpost generation record
//! and judges every generation; the supervisor launches children, watches
//! their liveness by pings, sees pipes close and processes exit, and
//! reports these to Core, carrying out Core's start, end and shutdown
//! effects. Its parts:
//!
//! - One owner thread ([`owner`]) launches and ends every child and keeps
//!   what carrying out Core's effects needs: each application's running
//!   process, the spares, the listener's record and the processes it
//!   holds. Launch helpers, retirements, and the app only report to it
//!   through one mailbox; there is no shared map lock. Its heartbeat runs
//!   when its wait's deadline passes.
//! - The owner thread is also the relay ([`relay`];
//!   `docs/design/communication.md`, section 6.2): it waits only on an I/O
//!   completion port, reads the listener's pipe and hands each tell and
//!   answer to the app in the pass that read it, dropping every frame of a
//!   listener that was replaced; it reads every outpost's pipe, handles
//!   pongs, `Ready`, `TargetExited` and the pipe's end itself, and delivers
//!   every other frame whole and unparsed ([`OutpostMessage::Frame`]),
//!   which the app parses through [`Frames`]; and it finishes every
//!   child's writes that pended. It never blocks: launching a process and
//!   ending one run on threads of their own and report back.
//! - Each child has a writer with a bounded queue ([`writer`]): whichever
//!   thread sends a command writes it at once when no write is in flight,
//!   so the reducer thread sends a request without waiting and without a
//!   hand-off, and a write the pipe cannot take at once is finished by the
//!   relay.
//! - It keeps two spare outposts started and waiting, with no application
//!   (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"), from the moment Core's
//!   own outpost is ready: a start for an application takes the oldest,
//!   sends it `Assign`, and tells the app the generation `Started`; a
//!   spare given never returns, and its replacement starts at once. A
//!   spare that fails while it waits is replaced after the spares'
//!   backoff ([`policy::Backoff`]).
//!
//! The app hears of each outpost generation through [`OutpostMessage`]:
//! `Started` before any of its messages, then its messages, then `Ended`.
//! When an outpost exits on its own, `Ended` follows everything it wrote; when
//! the owner ends it, `Ended` is sent at once, and the relay drops
//! anything the child wrote afterwards. One application's generations
//! come in the order the supervisor ended and started them: an outpost's
//! `Ended` before its replacement's `Started`.

mod frames;
mod owner;
mod process;
mod relay;
mod retire;
mod writer;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::Sender;
use verbatim_model::{OutpostId, Pid};
use verbatim_process::port_queue::Poster;

use verbatim_model::protocol::{CoreToListener, CoreToOutpost, ListenerToCore, OutpostToCore};

pub use frames::{Frame, Frames};
pub use retire::ShutdownSummary;
pub use verbatim_core::supervisor::EndReason;
use verbatim_core::supervisor::policy;
use verbatim_process::held::IgnoredProcesses;
pub use writer::QueueError;

/// How many spare outposts the supervisor keeps waiting
/// (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts").
pub const SPARES: usize = 2;

/// The supervisor's periods and limits. Verbatim runs with
/// [`Timings::default`]; the
/// tests' supervisor ([`Supervisor::with_executables`]) runs with the
/// default too, since a stand-in judged by a shorter ping interval is
/// ended as stuck when its start is slow. No setting in the settings file
/// reaches them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timings {
    /// How often every child is pinged: 3 seconds.
    pub ping_interval: Duration,
    /// How many ping intervals may pass without a pong before a child is
    /// ended: three intervals, nine seconds.
    pub missed_pongs: u32,
    /// How long the replacement of a spare's second failure in a row
    /// waits: a second. Each failure after it waits twice as long as the
    /// one before, up to [`backoff_limit`](Self::backoff_limit)
    /// (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"). The first failure
    /// is replaced at once. An application's outposts are spaced by Core's
    /// own backoff.
    pub backoff_first: Duration,
    /// The longest a failed spare's replacement waits: 30 seconds.
    pub backoff_limit: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(3),
            missed_pongs: 3,
            backoff_first: Duration::from_secs(1),
            backoff_limit: Duration::from_secs(30),
        }
    }
}

impl Timings {
    /// The backoff's three times, as [`policy::Backoff::failed`] takes
    /// them.
    fn backoff(&self) -> (Duration, Duration, Duration) {
        (self.backoff_first, self.backoff_limit, STAYED_UP)
    }
}

/// How long a spare must have been up for its failure to
/// count as the first again: a minute, twenty heartbeats, long past the
/// start and first focus in which a failure that repeats shows itself.
const STAYED_UP: Duration = Duration::from_mins(1);

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
        }
    }

    /// A sink that posts each message, wrapped by `wrap`, to a completion
    /// port's queue through `poster`, as the app's reducer takes its
    /// inputs: a post given back while the queue is still taken is
    /// [`Undelivered::Refused`], the port having refused the packet, and
    /// the message is had back with `unwrap`, which is given only an item
    /// `wrap` made; one given back once the queue is gone is
    /// [`Undelivered::Gone`].
    #[must_use]
    pub fn posting<T: Send + 'static>(
        poster: Poster<T>,
        wrap: fn(OutpostMessage) -> T,
        unwrap: fn(T) -> OutpostMessage,
    ) -> Self {
        Self::new(move |message| match poster.send(wrap(message)) {
            Ok(()) => Ok(()),
            Err(item) if poster.is_open() => Err(Undelivered::Refused(unwrap(item))),
            Err(_) => Err(Undelivered::Gone),
        })
    }

    /// Delivers `message`.
    ///
    /// # Errors
    ///
    /// Returns why it was not delivered ([`Undelivered`]).
    pub fn send(&self, message: OutpostMessage) -> Result<(), Undelivered> {
        (self.deliver)(message)
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

/// Arms the relay mailbox's fault point
/// ([`verbatim_faults::Fault::RelayWake`]) to refuse, once, the wake that
/// [`Supervisor::shutdown`]'s message posts, for a fault test: the point's
/// subject, the owner's event, is private to this crate.
#[cfg(feature = "faults")]
pub fn refuse_shutdown_wake() -> verbatim_faults::Guard {
    verbatim_faults::arm(
        verbatim_faults::Fault::RelayWake,
        |event: &owner::OwnerEvent| matches!(event, owner::OwnerEvent::Shutdown(_)),
        1,
    )
}

/// A window that came to the foreground, as the focus listener reported it
/// ([`verbatim_model::protocol::ListenerToCore::Foreground`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForegroundReport {
    /// The window.
    pub window: isize,
    /// The top of its owner chain.
    pub root_owner: isize,
    /// The process its facts go to.
    pub process: u32,
    /// When the listener observed it, in microseconds on the run's shared
    /// clock.
    pub observed_at_us: u64,
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
    /// Asked to start an outpost for `target_pid`
    /// ([`Supervisor::start_outpost`]) while one already runs: the
    /// generation running, and whether it has said it is ready, so Core's
    /// generation record takes it.
    Running {
        /// The generation.
        outpost: OutpostId,
        /// The application it watches.
        target_pid: Pid,
        /// Whether it sent its `Ready`.
        ready: bool,
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
    /// waiting now (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"). Verbatim
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
        /// The listener's generation.
        listener: OutpostId,
        /// Whether this listener replaced one that ended.
        replacement: bool,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// The owner is about to launch the listener of generation `listener`:
    /// sent before its process starts, so Core keeps its first tells
    /// (`docs/design/readers-tell-core-asks-build1.md`, "The outpost
    /// generation record").
    ListenerStarted {
        /// The listener's generation.
        listener: OutpostId,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// The listener of generation `listener` ended: its pipe closed, it was
    /// found wedged, or Verbatim is exiting.
    ListenerEnded {
        /// The listener's generation.
        listener: OutpostId,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// A tell or an answer the listener of generation `listener` sent, for
    /// Core's event queue, which judges whether it is the current one's.
    Listener {
        /// The listener's generation.
        listener: OutpostId,
        /// The message: a [`ListenerToCore::Tell`] or a
        /// [`ListenerToCore::Answer`].
        message: Box<ListenerToCore>,
    },
    /// A child answered a ping: an outpost of `target_pid`, or the
    /// listener when `target_pid` is `None`. Core's generation record keeps
    /// each one's liveness and judges an outpost's abandoned workers.
    Pong {
        /// The generation that answered.
        outpost: OutpostId,
        /// The application its outpost watches; `None` for the listener.
        target_pid: Option<Pid>,
        /// How many of the outpost's workers were abandoned and have not
        /// returned, but for those parked on a window judged hung: the
        /// pong's `parked_count`; 0 for the listener.
        abandoned: usize,
    },
    /// A progress bar's value changed, as the focus listener read it: the
    /// app gives it to Core.
    Progress(verbatim_model::BarChange),
    /// A window came to the system's foreground, as the focus listener
    /// learned it: the app gives it to Core, which decides the keyboard
    /// hook's view of the foreground (architecture section 5), attention
    /// and its foreground record.
    Foreground(ForegroundReport),
    /// An outpost generation ended. Its node ids are dead from now on.
    /// What follows is Core's decision.
    Ended {
        /// The generation that ended.
        outpost: OutpostId,
        /// The application it watched.
        target_pid: Pid,
        /// Why it ended.
        reason: EndReason,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// Core asked for an outpost for `target_pid` and none was started:
    /// its launch failed, or the application's process has exited, cannot
    /// be held, or is one Verbatim ignores ([`IgnoredProcesses`]), which
    /// `application_gone` says (Table 5, column C).
    LaunchFailed {
        /// The application.
        target_pid: Pid,
        /// Whether no outpost can serve it: its process is gone, cannot be
        /// held, or is ignored.
        application_gone: bool,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
    /// The application `target_pid`'s own process exited, as the handle
    /// the supervisor holds says (Table 5, column J). Its outpost's
    /// `Ended`, when one ran, comes first.
    ApplicationExited {
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
    /// behind every command sent to it before, one still starting once it
    /// is ready. Sent before any of them is asked, so it reaches the app
    /// ahead of every answer. Each answers
    /// [`OutpostToCore::IdleAnswer`] as a message of its own, or
    /// ends ([`OutpostMessage::Ended`]), which counts as its answer.
    IdleQueried {
        /// The round.
        round: u64,
        /// The outpost generations asked.
        outposts: Vec<OutpostId>,
        /// Only the supervisor sends it.
        sealed: Sealed,
    },
}

/// The executables the supervisor launches its children from, one for each
/// role: both are staged beside `verbatim.exe`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Executables {
    /// `verbatim-outpost.exe`, which every outpost and spare runs.
    pub outpost: PathBuf,
    /// `verbatim-listener.exe`, which the focus listener runs.
    pub listener: PathBuf,
}

impl Executables {
    /// Both executables, beside the current executable.
    ///
    /// # Errors
    ///
    /// When the current executable's path cannot be determined or has no
    /// parent directory.
    pub fn beside_current() -> io::Result<Self> {
        let dir = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("current exe has no parent directory"))?
            .to_owned();
        Ok(Self {
            outpost: dir.join("verbatim-outpost.exe"),
            listener: dir.join("verbatim-listener.exe"),
        })
    }

    /// The executable `role` runs from.
    fn of(&self, role: process::Role) -> &Path {
        match role {
            process::Role::Outpost(_) | process::Role::Spare(_) => &self.outpost,
            process::Role::Listener => &self.listener,
        }
    }

    /// One executable for both roles: a test's stand-in for both, which
    /// tells the roles apart by the listener's first argument.
    #[must_use]
    pub fn both(exe: PathBuf) -> Self {
        Self {
            outpost: exe.clone(),
            listener: exe,
        }
    }
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
    /// The outpost's and the listener's executables are resolved next to
    /// the current executable ([`Executables::beside_current`]).
    /// Every outpost is launched with `options`. The processes `ignored`
    /// holds are ignored entirely: the listener drops their facts, and no
    /// outpost is ever started for them. Every period and limit is
    /// [`Timings::default`].
    ///
    /// # Errors
    ///
    /// Returns an error if the current executable path cannot be determined
    /// or the relay's completion port or the owner thread cannot be made.
    pub fn new(
        events: impl Into<EventSink>,
        options: verbatim_process::arguments::OutpostOptions,
        ignored: Arc<IgnoredProcesses>,
    ) -> io::Result<Self> {
        let executables = Executables::beside_current()?;
        Self::start(
            events,
            options,
            executables,
            retire::SHUTDOWN_LIMIT,
            Timings::default(),
            ignored,
        )
    }

    /// [`Supervisor::new`], launching each role from its executable in
    /// `executables` and giving
    /// each `shutdown_limit` to exit after the shutdown message before it is
    /// killed, running by [`Timings::default`], and ignoring the processes
    /// `ignored` holds. For tests that launch stand-ins for the outpost and listener
    /// binaries ([`Executables::both`]); Verbatim uses
    /// [`Supervisor::new`].
    ///
    /// # Errors
    ///
    /// Returns an error if the relay's completion port or the owner thread
    /// cannot be made.
    pub fn with_executables(
        events: impl Into<EventSink>,
        options: verbatim_process::arguments::OutpostOptions,
        executables: Executables,
        shutdown_limit: Duration,
        ignored: Arc<IgnoredProcesses>,
    ) -> io::Result<Self> {
        Self::start(
            events,
            options,
            executables,
            shutdown_limit,
            Timings::default(),
            ignored,
        )
    }

    /// The supervisor of `executables`, running by `timings`, as
    /// [`Supervisor::new`] and [`Supervisor::with_executables`] start it.
    fn start(
        events: impl Into<EventSink>,
        options: verbatim_process::arguments::OutpostOptions,
        executables: Executables,
        shutdown_limit: Duration,
        timings: Timings,
        ignored: Arc<IgnoredProcesses>,
    ) -> io::Result<Self> {
        // The launch's log directory was prepared by the app at startup,
        // before any child (the synthesizer host first) began writing to it.
        let writers: Writers = Arc::default();
        let relay = relay::Relay::new()?;
        let (owner_tx, owner_rx) = relay::mailbox(std::sync::Arc::clone(relay.port()));
        let events: EventSink = events.into();
        owner::start(owner::Setup {
            executables,
            options,
            shutdown_limit,
            timings,
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

    /// Carries out Core's effect "start an outpost for `target_pid`",
    /// giving it a spare when one waits: the app hears
    /// [`OutpostMessage::Started`] once its process runs, or
    /// [`OutpostMessage::LaunchFailed`]. One that runs already is reported
    /// again ([`OutpostMessage::Running`]).
    pub fn start_outpost(&self, target_pid: Pid) {
        let _ = self.owner.send(owner::OwnerEvent::StartOutpost(target_pid));
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
        writer.push(writer::Outgoing::Other(command, writer::received_now()))
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
    /// one pipe hop (`books/dev/src/explanation/design/ownership.md`).
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

    /// Reads the time the spares' backoff is set and ended by from
    /// `clock`, in microseconds, in place of the run's shared clock, from
    /// the next failure on: a test that moves it past a backoff decides
    /// when the backoff ends, so what it waits for has happened first, and
    /// wakes the supervisor with any later command to have it act at once.
    pub fn set_backoff_clock(&self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) {
        let _ = self.owner.send(owner::OwnerEvent::BackoffClock(clock));
    }

    /// Sends the focus listener `command`, a request or cancel of Core's
    /// event queue, behind every command sent to it before, never dropped;
    /// held while a listener is launching. With none, nothing is sent:
    /// Core heard the listener end ([`OutpostMessage::ListenerEnded`]) and
    /// answered its requests.
    pub fn send_to_listener(&self, command: CoreToListener) {
        let _ = self.owner.send(owner::OwnerEvent::ToListener(command));
    }

    /// Ends the outpost generation `outpost`, as Core's generation record
    /// asks: it missed its pings, or its application exited before it was
    /// ready. Nothing happens when it has ended already; its
    /// [`OutpostMessage::Ended`] follows otherwise.
    pub fn end_outpost(&self, outpost: OutpostId) {
        let _ = self.owner.send(owner::OwnerEvent::EndOutpost(outpost));
    }

    /// Runs the idle barrier's round `round` through the children
    /// (`docs/design/communication.md`, section 6.7): the listener is asked
    /// first, behind every command sent to it before; once it answers, or
    /// if it has gone, every outpost, ready or starting, is asked behind
    /// every command sent to it before, and the app hears which
    /// ([`OutpostMessage::IdleQueried`]). An outpost still being launched
    /// is waited for first, so the app has heard it started.
    pub fn query_idle(&self, round: u64) {
        let _ = self.owner.send(owner::OwnerEvent::IdleRound(round));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_role_runs_from_its_own_executable() {
        let executables = Executables {
            outpost: PathBuf::from("outpost.exe"),
            listener: PathBuf::from("listener.exe"),
        };
        assert_eq!(
            executables.of(process::Role::Outpost(verbatim_model::Pid(7))),
            Path::new("outpost.exe")
        );
        assert_eq!(
            executables.of(process::Role::Spare(verbatim_model::OutpostId(1))),
            Path::new("outpost.exe")
        );
        assert_eq!(
            executables.of(process::Role::Listener),
            Path::new("listener.exe")
        );
    }
}
