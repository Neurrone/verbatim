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
mod writer;

use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Sender, unbounded};
use verbatim_model::{OutpostId, Pid};

use crate::protocol::{OutpostToSupervisor, SupervisorToOutpost};

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
    /// The owner killed it for not answering pings or for piling up abandoned
    /// workers.
    Killed,
    /// The owner retired it: its application had not held attention for two
    /// minutes and Core held none of its nodes.
    Retired,
}

impl std::fmt::Display for EndReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EndReason::Exited => "exited",
            EndReason::Killed => "killed",
            EndReason::Retired => "retired",
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
    /// The focus listener was replaced and its replacement is ready. Facts
    /// were lost in the gap, so the app asks the foreground application for
    /// its current focus.
    ListenerReplaced,
    /// A menu closed, menu mode ended, or the Alt+Tab switcher closed,
    /// somewhere on the desktop, and no focus event followed within the
    /// listener's grace: the app asks the foreground application for its
    /// focused control (NVDA's fake focus).
    MenuOrSwitchEnded,
    /// An outpost incarnation ended. Its node ids are dead from now on.
    Ended {
        /// The incarnation that ended.
        outpost: OutpostId,
        /// The application it watched.
        target_pid: Pid,
        /// Why it ended.
        reason: EndReason,
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
    ///
    /// # Errors
    ///
    /// Returns an error if the current executable path cannot be determined
    /// or the owner thread cannot be started.
    pub fn new(events_tx: Sender<OutpostMessage>) -> io::Result<Self> {
        let exe_path = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("current exe has no parent directory"))?
            .join("verbatim-outpost.exe");
        if let Some(exe_dir) = exe_path.parent() {
            process::prepare_launch_logs(exe_dir);
        }
        let writers: Writers = Arc::default();
        let (owner_tx, owner_rx) = unbounded();
        owner::start(owner::Setup {
            exe_path,
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
    /// real keypress sent right after Verbatim's menu opens.
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

    /// Tells one outpost incarnation which of its nodes Core still holds, and
    /// the position of the last of its messages the app has handled. A newer
    /// list replaces one still waiting to be written, and gets through even
    /// when the queue is full. Nothing happens if the incarnation has ended.
    pub fn send_nodes_held(&self, outpost: OutpostId, nodes: Vec<u64>, acknowledged: u64) {
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
                    acknowledged,
                },
            ));
        }
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
