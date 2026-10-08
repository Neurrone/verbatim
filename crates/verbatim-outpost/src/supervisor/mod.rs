//! The Core-side supervisor (architecture section 1, decisions D9 and D13;
//! outpost redesign, "The supervisor").
//!
//! The supervisor starts outposts and the focus listener, routes the
//! listener's facts to the right outpost, and ends children that crash, hang,
//! or are no longer needed. Its parts:
//!
//! - One lifecycle owner thread ([`owner`]) makes every lifecycle decision and
//!   owns the per-application records and the listener record. Reader threads,
//!   the heartbeat and sweep timers, launch helpers, and the app only report
//!   facts to it over one channel; nothing else changes a record, and there is
//!   no shared map lock.
//! - Launching a process blocks, so a helper thread does it and reports back.
//!   The owner records the application as starting before the launch, so a
//!   second fact meanwhile is held, not a second launch.
//! - Each child has a writer thread with a bounded queue ([`writer`]), so the
//!   owner never writes to a pipe while deciding, and the reducer thread can
//!   queue a query without waiting.
//! - Each child has a reader thread that stamps every node id with the
//!   child's outpost id, forwards the child's messages to the app, and
//!   reports `Ready`, pongs, and the pipe's end to the owner.
//! - Core's thread count is therefore two per child, plus the owner.
//!
//! The app hears of each outpost incarnation through [`OutpostMessage`]:
//! `Started` before any of its messages, then its messages, then `Ended`.
//! When an outpost exits on its own, `Ended` follows everything it wrote; when
//! the owner kills or retires it, `Ended` is sent at once and anything the
//! child wrote afterwards follows it, which the app drops.

mod owner;
mod policy;
mod process;
mod retire;
mod writer;

use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Sender, unbounded};
use verbatim_model::{OutpostId, Pid};

use crate::protocol::{OutpostToSupervisor, SupervisorToOutpost};

pub use retire::ShutdownSummary;
pub use writer::QueueError;

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

/// How many crashes within [`CRASH_WINDOW`] stop respawning until the next
/// foreground change to the application.
const CRASH_LIMIT: usize = 3;

/// The window [`CRASH_LIMIT`] counts crashes in.
const CRASH_WINDOW: Duration = Duration::from_mins(1);

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
        /// [`Supervisor::send_nodes_held`] once it has handled the message.
        position: u64,
        /// The message, boxed so every channel send is small, not sized to
        /// the largest reply.
        message: Box<OutpostToSupervisor>,
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
    /// An outpost incarnation ended. Its node ids are dead from now on.
    Ended {
        /// The incarnation that ended.
        outpost: OutpostId,
        /// The application it watched.
        target_pid: Pid,
        /// Why it ended.
        reason: EndReason,
    },
    /// An outpost was wanted for `target_pid`, asked for or to replace one
    /// that crashed, and none was started: the application's process has
    /// exited, or cannot be opened to be held, or its outposts crashed
    /// repeatedly, which stops respawning until the next foreground change
    /// to it. Nothing should wait for one.
    NotWatched {
        /// The application.
        target_pid: Pid,
    },
}

/// The live writers, by outpost id, so the reducer thread can queue a
/// command for an outpost without going through the owner. The owner adds a
/// writer when an outpost starts and removes it when the outpost ends; the
/// lock is held only to look one up, never across any I/O.
type Writers = Arc<Mutex<std::collections::HashMap<OutpostId, writer::WriterHandle>>>;

/// Starts, routes to, and ends outposts and the focus listener.
pub struct Supervisor {
    owner: Sender<owner::OwnerEvent>,
    writers: Writers,
}

impl Supervisor {
    /// Starts the supervisor: its lifecycle owner thread and the focus
    /// listener. Outpost messages and lifecycle notices go to `events_tx`.
    /// The outpost executable is resolved next to the current executable.
    /// Every outpost is launched with `options`.
    ///
    /// # Errors
    ///
    /// Returns an error if the current executable path cannot be determined
    /// or the owner thread cannot be started.
    pub fn new(
        events_tx: Sender<OutpostMessage>,
        options: crate::OutpostOptions,
    ) -> io::Result<Self> {
        let exe_path = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("current exe has no parent directory"))?
            .join("verbatim-outpost.exe");
        Self::with_executable(events_tx, options, exe_path, retire::SHUTDOWN_LIMIT)
    }

    /// [`Supervisor::new`], launching every child from `exe_path` and giving
    /// each `shutdown_limit` to exit after the shutdown message before it is
    /// killed. For tests that launch a stand-in for the outpost binary;
    /// Verbatim uses [`Supervisor::new`].
    ///
    /// # Errors
    ///
    /// Returns an error if the owner thread cannot be started.
    pub fn with_executable(
        events_tx: Sender<OutpostMessage>,
        options: crate::OutpostOptions,
        exe_path: std::path::PathBuf,
        shutdown_limit: Duration,
    ) -> io::Result<Self> {
        // The launch's log directory was prepared by the app at startup,
        // before any child (the synthesizer host first) began writing to it.
        let writers: Writers = Arc::default();
        let (owner_tx, owner_rx) = unbounded();
        owner::start(owner::Setup {
            exe_path,
            options,
            shutdown_limit,
            events_tx,
            writers: Arc::clone(&writers),
            own_tx: owner_tx.clone(),
            events: owner_rx,
        })?;
        Ok(Self {
            owner: owner_tx,
            writers,
        })
    }

    /// Starts an outpost for `target_pid` if none exists, without asking it
    /// to report anything. Used once at startup for Core's own process, so
    /// its outpost is warm before the first gesture: a cold spawn races a
    /// real keypress sent right after Verbatim's menu opens. An application
    /// whose process has exited, or whose outposts crashed repeatedly, gets
    /// none, and the app hears [`OutpostMessage::NotWatched`].
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
        writer.push(writer::Outgoing::Other(command))
    }

    /// Tells one outpost incarnation which of its nodes and text anchors Core
    /// still holds, and the position of the last of its messages the app has
    /// handled. A newer list replaces one still waiting to be written, and
    /// gets through even when the queue is full. Nothing happens if the
    /// incarnation has ended.
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
            let _ = writer.push(writer::Outgoing::NodesHeld(
                SupervisorToOutpost::NodesHeld {
                    nodes,
                    anchors,
                    acknowledged,
                },
            ));
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
    /// state holds nodes. Attention decides respawn after a crash; both
    /// decide retirement.
    pub fn note_views(&self, attention: Option<Pid>, holding: BTreeSet<OutpostId>) {
        let _ = self
            .owner
            .send(owner::OwnerEvent::Views { attention, holding });
    }
}
