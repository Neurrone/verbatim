//! The Core-side supervisor (architecture section 1, decision D9).
//!
//! The supervisor spawns outposts, holds their job handles so the kernel kills
//! them if Core dies, forwards their messages to a channel the app consumes,
//! and respawns any that exit unexpectedly. Each outpost is spawned suspended,
//! placed in a job object carrying kill-on-job-close plus a per-process memory
//! cap, then resumed — so it is inside the job before it runs a single
//! instruction. Communication is over two anonymous pipes whose child ends are
//! inherited by handle value; no named endpoint exists.
//!
//! State is a map keyed by target [`Pid`], N-ready by construction: one
//! outpost process per application (D9), spawned when that application first
//! gains focus and kept alive when it loses foreground again — never respawned
//! or rebound to a different application.
//!
//! Focus detection lives in a separate process (decision D13): the focus
//! listener holds the desktop-global UIA focus registration and global MSAA
//! hooks and forwards each captured focus fact to the supervisor, which routes
//! it to the target's own outpost ([`SupervisorShared::route_fact`]) —
//! spawning that outpost if needed and queueing facts newest-wins across the
//! spawn. The listener lives in a dedicated slot, supervised by the same job,
//! heartbeat, and respawn machinery but never in the per-pid map, so the idle
//! sweep structurally never touches it. What remains of the announce poll is a
//! fallback: [`Supervisor::note_foreground`] spawns-or-announces for the
//! startup target and to re-announce the current foreground across a listener
//! respawn gap.
//!
//! Idle outposts are retired on a timer so memory use stays bounded (risk R2):
//! an outpost whose application has not held foreground for
//! [`IDLE_RETIREMENT`] is sent `Shutdown` and reaped, swept both on every
//! foreground change and from a coarse background timer, and the current
//! foreground's outpost is never a candidate. Retirement removes the entry
//! from the map *before* sending `Shutdown`, which is what makes the ordinary
//! respawn-on-death path (below) correctly do nothing for a deliberately
//! retired outpost: by the time its pipe reaches end of stream, there is no
//! matching map entry left to respawn.
//!
//! Respawn-on-death is per-pid and generation-checked (an outpost that exited
//! after already being replaced or retired is not respawned), and additionally
//! checks that the watched application's process is itself still alive before
//! respawning — an outpost whose application has already exited is retired
//! instead, not resurrected to watch a pid that no longer exists.
//!
//! Respawn-on-death only covers an outpost that *exits*. Recovery ladder rung
//! 3 also covers one that is alive but wedged: stopped answering entirely, or
//! quietly piling up parked threads from abandoned COM calls (rung 2's
//! bounded garbage — architecture section 1). A dedicated heartbeat thread
//! (started in [`Supervisor::new`] alongside the idle sweep) pings every live
//! outpost on [`PING_INTERVAL`] and, from each pong, learns the outpost's
//! current parked-thread count. [`wedge_decision`] is the pure policy —
//! given the last pong time, now, and the last reported parked count, decide
//! whether to kill and respawn, and why — kept separate from the I/O that
//! gathers those inputs, the same split [`idle_decision`] uses. A kill is
//! generation-checked exactly like a natural respawn, so it cannot race a
//! retirement or a respawn that already replaced the entry, and it works by
//! removing the outpost's map entry and dropping its job handle: the
//! kill-on-job-close limit set up at spawn turns that drop into an immediate
//! kernel-level kill, so no message has to reach an outpost that is by
//! definition not reliably answering messages. The old process's late pong
//! or end-of-stream is then generation-mismatched against the freshly
//! spawned replacement and ignored.

use std::ffi::c_void;
use std::fs::File;
use std::io::{self, BufReader};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
use windows::Win32::Foundation::{HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_ALWAYS,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, OpenProcess,
    PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOW,
};
use windows::core::{PCWSTR, PWSTR};

use verbatim_model::{Pid, TraceId};

use crate::protocol::{
    DeliveredFact, ListenerFact, OutpostToSupervisor, SupervisorToOutpost, read_message,
    write_message,
};

/// A 200 MB per-outpost memory cap; a leaking outpost is killed by the kernel
/// and respawned by the supervisor.
const OUTPOST_MEMORY_CAP: usize = 200 * 1024 * 1024;

/// How long an outpost's application must have last held foreground before
/// it is a candidate for idle retirement (risk R2's memory-use mitigation).
const IDLE_RETIREMENT: Duration = Duration::from_mins(2);

/// How often the background sweep thread checks for idle outposts, for
/// applications that stay backgrounded long enough that no foreground
/// change ever triggers a sweep on its own.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// How often the heartbeat thread pings every live outpost (recovery ladder
/// rung 3's wedged-but-alive detection). A few seconds keeps a wedge visible
/// on a human timescale — a screen reader user experiencing a stuck
/// application notices within tens of seconds, not minutes — without adding
/// meaningful overhead: a handful of tiny JSON messages per interval, even
/// with several outposts running at once.
const PING_INTERVAL: Duration = Duration::from_secs(3);

/// How many consecutive [`PING_INTERVAL`]s may pass with no pong before an
/// outpost is declared wedged. More than one interval tolerates a single
/// slow tick (VM scheduling variance has already forced one latency-budget
/// loosening in this codebase); three intervals means a wedge is declared
/// only after roughly [`PING_INTERVAL`] times three with no sign of life,
/// long enough that a merely busy outpost has had ample chance to answer.
const MISSED_PONG_THRESHOLD: u32 = 3;

/// The parked-thread count (recovery ladder rung 2's bounded garbage; see
/// this module's doc and architecture section 1) at or above which an
/// outpost is killed and respawned even though it is still answering pings.
/// Each parked thread is roughly a megabyte of stack and a handle, so 8 is
/// already several megabytes of garbage — and, since parking only happens
/// when a call is abandoned past its deadline, also evidence the target
/// application is repeatedly hanging calls rather than having hit one
/// isolated slow call.
const PARKED_THREAD_KILL_THRESHOLD: usize = 8;

/// A message from the supervisor to the app: either something an outpost
/// sent over its pipe, tagged with the target application it watches, or a
/// lifecycle notice the supervisor itself generates.
#[derive(Debug)]
pub enum OutpostMessage {
    /// A message an outpost sent, tagged with its target pid. Boxed because
    /// `OutpostToSupervisor` grew with M3's tree, ancestor-chain, and
    /// navigation replies while `Retired` stays a bare pid; boxing keeps
    /// every channel send small instead of sized to the largest reply.
    Event(Pid, Box<OutpostToSupervisor>),
    /// The supervisor retired an outpost (idle timeout) or gave up
    /// respawning one whose watched application has itself exited; Core
    /// should drop it from any status mirror.
    Retired(Pid),
    /// The focus listener reported a foreground change to this pid (decision
    /// D13). Sent before the corresponding focus fact is delivered, so the
    /// reducer's stale-event gate has the new foreground recorded by the time
    /// the fact's own event arrives (channel ordering makes this race-free).
    /// The app stores it and notes the pid as targeted, exactly what the old
    /// in-Core foreground trigger did — but without spawning or announcing,
    /// which the supervisor now drives from the fact itself.
    ForegroundChanged(Pid),
}

/// Spawns, tracks, retires, and respawns outpost processes.
pub struct Supervisor {
    shared: Arc<SupervisorShared>,
}

struct SupervisorShared {
    exe_path: PathBuf,
    events_tx: Sender<OutpostMessage>,
    generation: AtomicU64,
    outposts: Mutex<HashMap<Pid, Running>>,
    /// The focus listener's dedicated slot (decision D13): one permanent,
    /// stateless process, supervised like any outpost (job, heartbeat,
    /// respawn) but never in the per-pid map, so the idle sweep structurally
    /// never touches it and no listener entry pollutes the status mirror.
    listener: Mutex<Option<ListenerRunning>>,
    current_foreground: Mutex<Option<Pid>>,
    /// Source of the `seq` echoed in each `Ping`/`Pong`; only monotonicity
    /// (for log correlation) matters, not per-outpost uniqueness.
    ping_seq: AtomicU64,
}

/// One live outpost process, the parent end of its command pipe, and enough
/// bookkeeping to decide respawn, idle-retirement, and wedge-kill policy.
struct Running {
    generation: u64,
    /// Held so the kernel kills the outpost when this handle closes — the
    /// mechanism both natural job cleanup and a deliberate wedge kill
    /// (dropping this early) rely on.
    _job: OwnedHandle,
    /// The outpost process handle; closing it does not kill the process (the
    /// job does), it just releases our reference.
    _process: OwnedHandle,
    /// The outpost's own process id, for wedge-kill log lines.
    outpost_pid: Pid,
    to_outpost: File,
    last_foreground_at: Instant,
    /// When the most recent pong from this outpost was recorded. Set to the
    /// spawn time initially, so a freshly spawned outpost is not judged
    /// wedged before it has had a chance to answer even one ping.
    last_pong_at: Instant,
    /// The parked-thread count from the most recent pong, or 0 before the
    /// first one arrives.
    last_parked_count: usize,
    /// Whether this outpost has sent its `Ready` yet. A focus fact routed to
    /// an outpost still spawning is not written straight to its pipe — it is
    /// held in [`pending`](Self::pending) and flushed when `Ready` arrives, so
    /// the fact is never lost to the spawn gap (decision D13).
    ready: bool,
    /// Focus facts routed to this outpost before it sent `Ready`, one slot per
    /// category, newest-wins (see [`PendingFacts`]).
    pending: PendingFacts,
}

/// The focus listener's live process, the parent end of its command pipe, and
/// the bookkeeping to decide respawn and wedge-kill policy (decision D13). It
/// carries no `last_foreground_at` (the listener is never idle-retired) and no
/// parked count (it never parks a thread — its hard rule is no cross-process
/// call, so it never blocks on one).
struct ListenerRunning {
    generation: u64,
    /// Held so the kernel kills the listener when this handle closes, the same
    /// mechanism the per-app outposts use.
    _job: OwnedHandle,
    /// The listener process handle; closing it just releases our reference.
    _process: OwnedHandle,
    /// The listener's own process id, for wedge-kill log lines.
    outpost_pid: Pid,
    to_listener: File,
    /// When the most recent pong was recorded, spawn time initially.
    last_pong_at: Instant,
}

impl Supervisor {
    /// Creates a supervisor that forwards outpost messages to `events_tx` and
    /// starts its background idle-retirement sweep thread. The outpost
    /// executable is resolved next to the current executable.
    ///
    /// # Errors
    ///
    /// Returns an error if the current executable path cannot be determined.
    pub fn new(events_tx: Sender<OutpostMessage>) -> io::Result<Self> {
        let exe_path = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("current exe has no parent directory"))?
            .join("verbatim-outpost.exe");
        let shared = Arc::new(SupervisorShared {
            exe_path,
            events_tx,
            generation: AtomicU64::new(0),
            outposts: Mutex::new(HashMap::new()),
            listener: Mutex::new(None),
            current_foreground: Mutex::new(None),
            ping_seq: AtomicU64::new(0),
        });
        // The focus listener exists from startup (decision D13). A failed
        // initial spawn is logged and left for the heartbeat tick to retry on
        // its interval, rather than failing supervisor construction.
        shared.ensure_listener();
        spawn_sweep_thread(&shared);
        spawn_heartbeat_thread(&shared);
        Ok(Self { shared })
    }

    /// Reports a foreground change to `target_pid` by the announce-poll path:
    /// spawns an outpost if none exists for this pid yet, otherwise sends the
    /// existing one an `AnnounceFocus` — never respawning or rebinding it.
    /// Outposts stay alive when their application loses foreground.
    ///
    /// Under decision D13 this is a fallback, not the mechanism of record.
    /// Ordinary foreground changes now flow through the focus listener as
    /// facts ([`SupervisorShared::route_fact`]); this poll remains for the
    /// supervisor's own startup target (`verbatim-app`'s
    /// `target_current_foreground`) and to re-announce the current foreground
    /// after a listener respawn. It writes `AnnounceFocus` on both branches —
    /// `spawn` itself no longer does, so a fact-routing spawn stays silent and
    /// lets the fact do the announcing.
    ///
    /// # Errors
    ///
    /// Returns an error if spawning or sending fails.
    pub fn note_foreground(&self, target_pid: Pid) -> io::Result<()> {
        *self
            .shared
            .current_foreground
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(target_pid);
        let result = self.shared.announce_foreground(target_pid);
        self.shared.sweep_idle();
        result
    }

    /// Spawns an outpost for `target_pid` if none exists yet, without
    /// touching foreground tracking — a warm-up call, not a foreground
    /// report. Does nothing if an outpost already watches this pid.
    ///
    /// Exists for one specific case: Core's own process. Verbatim reading
    /// its own GUI is a first-class scenario (M1's defining test), and a
    /// cold outpost spawn (process creation, `WinEvent` hook install, UIA
    /// registration) measurably races a real down-arrow keypress sent
    /// immediately after the popup menu takes foreground — confirmed live
    /// against the VM: without this, the menu's own top-level-window
    /// announcement lands, but the `WinEvent` for the arrow-key-selected menu
    /// item can fire before the hook is installed and is lost for good,
    /// since (unlike the initial `AnnounceFocus`) ordinary live navigation
    /// events are not retried. Calling this once at startup for Core's own
    /// pid gives its outpost the whole time between startup and the first
    /// gesture to become warm, which in practice is always enough.
    ///
    /// # Errors
    ///
    /// Returns an error if spawning fails.
    pub fn ensure_spawned(&self, target_pid: Pid) -> io::Result<()> {
        let mut outposts = self
            .shared
            .outposts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if outposts.contains_key(&target_pid) {
            return Ok(());
        }
        let running = self.shared.spawn(target_pid)?;
        outposts.insert(target_pid, running);
        Ok(())
    }

    /// Sends a command to a specific outpost.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no outpost for `target_pid` or the write
    /// fails.
    pub fn send_to(&self, target_pid: Pid, command: &SupervisorToOutpost) -> io::Result<()> {
        let mut outposts = self
            .shared
            .outposts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let running = outposts
            .get_mut(&target_pid)
            .ok_or_else(|| io::Error::other(format!("no outpost is watching pid {target_pid}")))?;
        write_message(&mut running.to_outpost, command)
    }
}

impl SupervisorShared {
    /// Spawns an outpost watching `target_pid` for its whole life (decision
    /// D9: fixed at spawn, never retargeted) and starts its reader thread.
    fn spawn(self: &Arc<Self>, target_pid: Pid) -> io::Result<Running> {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let pipes = Pipes::create()?;
        let job = create_job()?;

        let command_line = format!(
            "\"{}\" --pipe-in {} --pipe-out {} --target-pid {}",
            self.exe_path.display(),
            pipes.child_in.0 as usize,
            pipes.child_out.0 as usize,
            target_pid.0,
        );
        // Redirect the outpost's stderr to a per-pid log file so its `tracing`
        // output is readable (Task: outpost observability). Best-effort: a
        // failed log open leaves the outpost unredirected, never unspawned.
        let log_handle = child_log_handle(&self.exe_path, &format!("outpost-{}", target_pid.0));
        let spawn_result = spawn_suspended(&command_line, &job, log_handle);
        if let Some(handle) = log_handle {
            // The child inherited its own copy; drop ours whether or not the
            // spawn succeeded.
            close_handle(handle);
        }
        let process = spawn_result?;

        // The child has inherited its pipe ends; close ours to them so EOF is
        // observed correctly when the outpost exits.
        pipes.close_child_ends();

        // Resume now that the process is in the job.
        // SAFETY: `process.thread` is the suspended primary thread handle.
        unsafe {
            ResumeThread(process.thread);
        }
        close_handle(process.thread);

        let to_outpost = pipes.parent_out;
        // No `AnnounceFocus` is written here (decision D13): focus now arrives
        // as a listener fact, and a fact-routing spawn wants the fact to do
        // the announcing, not a poll. The two callers that still want the poll
        // — the startup target and a listener-respawn recovery — write
        // `AnnounceFocus` themselves through `announce_foreground`.

        // SAFETY: `process.process` is a valid process handle we now own.
        let process_owned = unsafe { OwnedHandle::from_raw_handle(process.process.0 as RawHandle) };
        let outpost_pid = Pid(process.pid);

        let reader_shared = Arc::clone(self);
        let from_outpost = pipes.parent_in;
        thread::Builder::new()
            .name("verbatim-outpost-reader".to_owned())
            .spawn(move || reader_loop(&reader_shared, generation, target_pid, from_outpost))
            .map_err(io::Error::other)?;

        let now = Instant::now();
        Ok(Running {
            generation,
            _job: job,
            _process: process_owned,
            outpost_pid,
            to_outpost,
            last_foreground_at: now,
            last_pong_at: now,
            last_parked_count: 0,
            ready: false,
            pending: PendingFacts::default(),
        })
    }

    /// The announce-poll spawn-or-announce behind [`Supervisor::note_foreground`]:
    /// if an outpost already watches `target_pid`, refresh its foreground time
    /// and send it an `AnnounceFocus`; otherwise spawn one and send the same.
    /// The poll is a decision-D13 fallback (startup target, listener-respawn
    /// recovery), so unlike [`Self::route_fact`] this always writes
    /// `AnnounceFocus`.
    fn announce_foreground(self: &Arc<Self>, target_pid: Pid) -> io::Result<()> {
        let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(running) = outposts.get_mut(&target_pid) {
            running.last_foreground_at = Instant::now();
        } else {
            let running = self.spawn(target_pid)?;
            outposts.insert(target_pid, running);
        }
        let running = outposts
            .get_mut(&target_pid)
            .expect("just inserted or already present");
        write_message(
            &mut running.to_outpost,
            &SupervisorToOutpost::AnnounceFocus {
                trace_id: TraceId::mint(),
            },
        )
    }

    /// Routes one focus fact from the listener to the target's own outpost
    /// (decision D13). For a foreground fact, this first records the new
    /// foreground (both the supervisor's own tracking, for the idle sweep and
    /// listener-respawn recovery, and the app's, via
    /// [`OutpostMessage::ForegroundChanged`]) *before* delivering, so the
    /// reducer's stale-event gate has the new foreground by the time the
    /// fact's own event arrives. For every fact it ensures the target outpost
    /// exists — spawning it if needed, which merely delays this one
    /// announcement by the spawn latency rather than losing it — and then
    /// delivers the fact, or, if the outpost has not sent `Ready` yet, queues
    /// it newest-wins per category to be flushed on `Ready`.
    fn route_fact(self: &Arc<Self>, trace_id: TraceId, observed_at_ms: u64, fact: ListenerFact) {
        let target_pid = fact.pid();
        let is_foreground = matches!(fact, ListenerFact::Foreground { .. });
        if is_foreground {
            *self
                .current_foreground
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(target_pid);
            {
                let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(running) = outposts.get_mut(&target_pid) {
                    running.last_foreground_at = Instant::now();
                }
            }
            // Sent before the deliver below: the app updates its foreground
            // atomic here, and channel ordering guarantees it lands before the
            // outpost's own emitted event can (the outpost has not even been
            // handed the fact yet).
            let _ = self
                .events_tx
                .send(OutpostMessage::ForegroundChanged(target_pid));
        }

        let delivered = fact.into_delivered();
        let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
        let running = match outposts.entry(target_pid) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => match self.spawn(target_pid) {
                Ok(running) => entry.insert(running),
                Err(error) => {
                    tracing::warn!(%error, %target_pid, "failed to spawn outpost for a focus fact");
                    return;
                }
            },
        };
        if running.ready {
            let _ = write_message(
                &mut running.to_outpost,
                &SupervisorToOutpost::DeliverFact {
                    trace_id,
                    observed_at_ms,
                    fact: delivered,
                },
            );
        } else {
            running.pending.store(trace_id, observed_at_ms, delivered);
        }
    }

    /// Flushes any facts queued for `target_pid` while it was still spawning,
    /// when its `Ready` is intercepted (decision D13) — but only if it is
    /// still this `generation`'s entry, the same discipline
    /// [`Self::record_pong`] follows. The facts flush in the fixed order
    /// foreground, focus, menu-popup ([`PendingFacts::drain`]).
    fn on_ready(&self, target_pid: Pid, generation: u64) {
        let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(running) = outposts.get_mut(&target_pid) else {
            return;
        };
        if running.generation != generation {
            return;
        }
        running.ready = true;
        for pending in running.pending.drain() {
            let _ = write_message(
                &mut running.to_outpost,
                &SupervisorToOutpost::DeliverFact {
                    trace_id: pending.trace_id,
                    observed_at_ms: pending.observed_at_ms,
                    fact: pending.fact,
                },
            );
        }
    }

    /// Respawns the outpost that exited, if it is still this generation's
    /// current entry for `target_pid` and its watched application process is
    /// itself still alive. If the entry is gone (already retired or
    /// replaced), does nothing. If the entry is still there but the
    /// application has exited, removes it and reports it retired rather
    /// than resurrecting an outpost for a pid that no longer exists.
    fn respawn_if_alive(self: &Arc<Self>, target_pid: Pid, generation: u64) {
        let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(running) = outposts.get(&target_pid) else {
            return; // Already retired or replaced; nothing to do.
        };
        if running.generation != generation {
            return; // Already replaced; nothing to do.
        }
        if !process_is_alive(target_pid.0) {
            outposts.remove(&target_pid);
            drop(outposts);
            let _ = self.events_tx.send(OutpostMessage::Retired(target_pid));
            return;
        }
        outposts.remove(&target_pid);
        drop(outposts);
        match self.spawn(target_pid) {
            Ok(running) => {
                self.outposts
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(target_pid, running);
            }
            Err(error) => {
                tracing::error!(%error, %target_pid, "failed to respawn outpost");
                let _ = self.events_tx.send(OutpostMessage::Retired(target_pid));
            }
        }
    }

    /// Records a pong from the outpost currently watching `target_pid`,
    /// updating its last-pong time and last-reported parked-thread count —
    /// but only if it is still this `generation`'s entry, so a pong from an
    /// outpost that has since been killed-and-respawned or naturally
    /// respawned (a late message from the old process) is silently ignored,
    /// the same generation discipline [`Self::respawn_if_alive`] follows.
    fn record_pong(&self, target_pid: Pid, generation: u64, parked_count: usize) {
        let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(running) = outposts.get_mut(&target_pid)
            && running.generation == generation
        {
            running.last_pong_at = Instant::now();
            running.last_parked_count = parked_count;
        }
    }

    /// Pings every live outpost and kills-and-respawns any [`wedge_decision`]
    /// judges wedged, from its last recorded pong time and parked-thread
    /// count. Called from the heartbeat thread on every [`PING_INTERVAL`].
    /// Pinging and deciding happen under one lock acquisition (so the
    /// decision is made from a consistent snapshot), but the kill-and-respawn
    /// itself happens afterward, outside the lock, matching
    /// [`Self::sweep_idle`]'s two-pass shape.
    fn heartbeat_tick(self: &Arc<Self>) {
        let now = Instant::now();
        let seq = self.ping_seq.fetch_add(1, Ordering::Relaxed);
        let mut wedged = Vec::new();
        {
            let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
            for (&pid, running) in outposts.iter_mut() {
                match wedge_decision(
                    running.last_pong_at,
                    now,
                    PING_INTERVAL,
                    MISSED_PONG_THRESHOLD,
                    running.last_parked_count,
                    PARKED_THREAD_KILL_THRESHOLD,
                ) {
                    Some(reason) => wedged.push((pid, running.generation, reason)),
                    None => {
                        let _ = write_message(
                            &mut running.to_outpost,
                            &SupervisorToOutpost::Ping { seq },
                        );
                    }
                }
            }
        }
        for (pid, generation, reason) in wedged {
            self.kill_and_respawn(pid, generation, reason);
        }

        // The listener is supervised on the same interval and by the same
        // wedge policy (decision D13), from its dedicated slot rather than the
        // per-pid map. A missing slot (a failed initial or respawn spawn) is
        // retried here.
        self.ensure_listener();
        let wedged_listener = {
            let mut slot = self.listener.lock().unwrap_or_else(PoisonError::into_inner);
            slot.as_mut().and_then(|running| {
                if let Some(reason) = wedge_decision(
                    running.last_pong_at,
                    now,
                    PING_INTERVAL,
                    MISSED_PONG_THRESHOLD,
                    0, // The listener never parks a thread.
                    PARKED_THREAD_KILL_THRESHOLD,
                ) {
                    Some((running.generation, reason))
                } else {
                    let _ =
                        write_message(&mut running.to_listener, &SupervisorToOutpost::Ping { seq });
                    None
                }
            })
        };
        if let Some((generation, reason)) = wedged_listener {
            self.kill_and_respawn_listener(generation, reason);
        }
    }

    /// Kills and respawns the outpost that [`Self::heartbeat_tick`] judged
    /// wedged, if it is still this generation's current entry for
    /// `target_pid` — the same generation check [`Self::respawn_if_alive`]
    /// uses, so a kill decision computed from a slightly stale snapshot
    /// cannot race a retirement or a respawn that already replaced this
    /// entry. Removing the map entry drops its job handle, and the
    /// kill-on-job-close limit set up at spawn turns that drop into an
    /// immediate kernel-level kill — no message needs to reach an outpost
    /// that is, by definition, not reliably answering messages. Logs at warn
    /// level with the target pid, the outpost's own pid, and the reason, for
    /// a flight-recorder-plus-stderr investigation to grep for. Mirrors
    /// [`Self::respawn_if_alive`]'s alive-application check: an outpost
    /// whose application has itself exited is retired, not resurrected.
    fn kill_and_respawn(self: &Arc<Self>, target_pid: Pid, generation: u64, reason: WedgeReason) {
        let killed = {
            let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
            match outposts.get(&target_pid) {
                Some(running) if running.generation == generation => outposts.remove(&target_pid),
                _ => None, // Already retired or replaced; nothing to do.
            }
        };
        let Some(running) = killed else {
            return;
        };
        tracing::warn!(
            %target_pid,
            outpost_pid = %running.outpost_pid,
            reason = %reason,
            "killing wedged outpost"
        );
        // Dropping `running` here closes its job handle; kill-on-job-close
        // kills the outpost process immediately.
        drop(running);

        if !process_is_alive(target_pid.0) {
            let _ = self.events_tx.send(OutpostMessage::Retired(target_pid));
            return;
        }
        match self.spawn(target_pid) {
            Ok(running) => {
                self.outposts
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(target_pid, running);
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    %target_pid,
                    "failed to respawn outpost after killing a wedged one"
                );
                let _ = self.events_tx.send(OutpostMessage::Retired(target_pid));
            }
        }
    }

    /// Spawns the focus listener (decision D13) with the same job-object
    /// machinery as a per-app outpost — suspended, placed in a
    /// kill-on-close-plus-memory-cap job, resumed — but with the `--listener`
    /// command line (no target pid) and its own reader loop. No `AnnounceFocus`
    /// is written: the listener has no target to announce.
    fn spawn_listener(self: &Arc<Self>) -> io::Result<ListenerRunning> {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let pipes = Pipes::create()?;
        let job = create_job()?;

        let command_line = format!(
            "\"{}\" --listener --pipe-in {} --pipe-out {}",
            self.exe_path.display(),
            pipes.child_in.0 as usize,
            pipes.child_out.0 as usize,
        );
        // Redirect the listener's stderr to `logs/listener.log`, same as a
        // per-app outpost (Task: outpost observability). Best-effort.
        let log_handle = child_log_handle(&self.exe_path, "listener");
        let spawn_result = spawn_suspended(&command_line, &job, log_handle);
        if let Some(handle) = log_handle {
            close_handle(handle);
        }
        let process = spawn_result?;
        pipes.close_child_ends();

        // SAFETY: `process.thread` is the suspended primary thread handle.
        unsafe {
            ResumeThread(process.thread);
        }
        close_handle(process.thread);

        // SAFETY: `process.process` is a valid process handle we now own.
        let process_owned = unsafe { OwnedHandle::from_raw_handle(process.process.0 as RawHandle) };
        let outpost_pid = Pid(process.pid);

        let reader_shared = Arc::clone(self);
        let from_listener = pipes.parent_in;
        thread::Builder::new()
            .name("verbatim-listener-reader".to_owned())
            .spawn(move || listener_reader_loop(&reader_shared, generation, from_listener))
            .map_err(io::Error::other)?;

        Ok(ListenerRunning {
            generation,
            _job: job,
            _process: process_owned,
            outpost_pid,
            to_listener: pipes.parent_out,
            last_pong_at: Instant::now(),
        })
    }

    /// Spawns the listener into its slot if the slot is empty. Called at
    /// startup and from each heartbeat tick, so a failed initial spawn is
    /// retried on the heartbeat interval rather than leaving the desktop with
    /// no focus detection. A spawn failure is logged and left for the next
    /// tick.
    fn ensure_listener(self: &Arc<Self>) {
        let mut slot = self.listener.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return;
        }
        match self.spawn_listener() {
            Ok(running) => *slot = Some(running),
            Err(error) => tracing::error!(%error, "failed to spawn focus listener"),
        }
    }

    /// Records a pong from the listener, updating its last-pong time — but only
    /// if it is still this `generation`'s listener, the same generation
    /// discipline [`Self::record_pong`] follows. The listener never parks a
    /// thread, so its parked count is ignored.
    fn record_listener_pong(&self, generation: u64) {
        let mut slot = self.listener.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(running) = slot.as_mut()
            && running.generation == generation
        {
            running.last_pong_at = Instant::now();
        }
    }

    /// Respawns the listener after its pipe reached end of stream, if it is
    /// still this `generation`'s listener, then re-announces the current
    /// foreground once — the poll fallback covering the respawn gap, during
    /// which no facts flowed (decision D13). The generation check makes a late
    /// end-of-stream from a listener already killed-and-respawned a no-op.
    fn respawn_listener_if_current(self: &Arc<Self>, generation: u64) {
        {
            let mut slot = self.listener.lock().unwrap_or_else(PoisonError::into_inner);
            match slot.as_ref() {
                Some(running) if running.generation == generation => {}
                _ => return, // Already replaced; nothing to do.
            }
            match self.spawn_listener() {
                Ok(running) => *slot = Some(running),
                Err(error) => {
                    tracing::error!(%error, "failed to respawn focus listener");
                    *slot = None; // Left empty for the heartbeat tick to retry.
                }
            }
        }
        self.announce_current_foreground();
    }

    /// Kills and respawns the listener the heartbeat judged wedged, if it is
    /// still this `generation`'s listener — the same generation check and
    /// job-handle-drop kill [`Self::kill_and_respawn`] uses. Follows the
    /// respawn with one synthetic announce for the current foreground to cover
    /// the gap, exactly as an end-of-stream respawn does.
    fn kill_and_respawn_listener(self: &Arc<Self>, generation: u64, reason: WedgeReason) {
        {
            let mut slot = self.listener.lock().unwrap_or_else(PoisonError::into_inner);
            let killed = match slot.as_ref() {
                Some(running) if running.generation == generation => slot.take(),
                _ => None,
            };
            let Some(running) = killed else {
                return;
            };
            tracing::warn!(
                outpost_pid = %running.outpost_pid,
                reason = %reason,
                "killing wedged focus listener"
            );
            // Dropping `running` closes its job handle; kill-on-job-close kills
            // the listener process immediately.
            drop(running);
            match self.spawn_listener() {
                Ok(running) => *slot = Some(running),
                Err(error) => {
                    tracing::error!(%error, "failed to respawn focus listener after killing a wedged one");
                    *slot = None;
                }
            }
        }
        self.announce_current_foreground();
    }

    /// Re-announces the current foreground application once through the
    /// announce-poll path, covering a listener respawn gap (decision D13).
    /// Does nothing if no foreground is known yet.
    fn announce_current_foreground(self: &Arc<Self>) {
        let current = *self
            .current_foreground
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(pid) = current
            && let Err(error) = self.announce_foreground(pid)
        {
            tracing::warn!(%error, %pid, "failed to re-announce foreground after listener respawn");
        }
    }

    /// Sends `Shutdown` to and removes every outpost whose application has
    /// not held foreground for [`IDLE_RETIREMENT`], skipping the current
    /// foreground's outpost and Core's own. The map entry is removed
    /// *before* `Shutdown` is written, so the reader thread's subsequent
    /// end-of-stream finds no matching generation and does not respawn (see
    /// this module's doc).
    ///
    /// Core's own outpost is exempt for the same reason
    /// [`Supervisor::ensure_spawned`] pre-warms it at startup: retiring it
    /// after two idle minutes would make the next Verbatim menu open spawn
    /// it cold, recreating the lost-keystroke race the warm-up exists to
    /// prevent — and "open the Verbatim menu after a while working in other
    /// applications" is the common case, not the edge. One permanently warm
    /// outpost watching our own process is a fixed, known cost.
    fn sweep_idle(&self) {
        let current = *self
            .current_foreground
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let own_pid = Pid(std::process::id());
        let now = Instant::now();
        let mut retiring = Vec::new();
        {
            let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
            let idle_pids: Vec<Pid> = outposts
                .iter()
                .filter(|&(&pid, running)| {
                    Some(pid) != current
                        && pid != own_pid
                        && idle_decision(running.last_foreground_at, now, IDLE_RETIREMENT)
                })
                .map(|(&pid, _)| pid)
                .collect();
            for pid in idle_pids {
                if let Some(running) = outposts.remove(&pid) {
                    retiring.push((pid, running));
                }
            }
        }
        for (pid, mut running) in retiring {
            let _ = write_message(&mut running.to_outpost, &SupervisorToOutpost::Shutdown);
            let _ = self.events_tx.send(OutpostMessage::Retired(pid));
            tracing::info!(%pid, "retired idle outpost");
        }
    }
}

/// The pure idle-retirement decision, factored out for unit testing: whether
/// an outpost last foregrounded at `last_foreground_at` counts as idle at
/// `now`, against threshold `idle_after`.
fn idle_decision(last_foreground_at: Instant, now: Instant, idle_after: Duration) -> bool {
    now.saturating_duration_since(last_foreground_at) >= idle_after
}

/// Why the supervisor killed and respawned an otherwise-alive outpost
/// (recovery ladder rung 3: wedged-but-alive detection, complementing the
/// crash-triggered respawn [`SupervisorShared::respawn_if_alive`] already
/// handles).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WedgeReason {
    /// No pong was received for [`MISSED_PONG_THRESHOLD`] consecutive ping
    /// intervals: the outpost has stopped answering, whatever the cause.
    MissedHeartbeats,
    /// The most recent pong reported a parked-thread count at or above
    /// [`PARKED_THREAD_KILL_THRESHOLD`].
    ParkedThreads,
}

impl std::fmt::Display for WedgeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            WedgeReason::MissedHeartbeats => "missed heartbeats",
            WedgeReason::ParkedThreads => "parked threads",
        })
    }
}

/// The pure wedge-kill decision, factored out for unit testing exactly like
/// [`idle_decision`]: given the last time a pong was recorded, now, and the
/// most recently reported parked-thread count, decide whether the outpost
/// should be killed and respawned, and why. Parked threads are checked
/// first: a wedged-and-leaking outpost that also happens to have just missed
/// its first pong is still more usefully reported as "parked threads" (the
/// more specific, more actionable diagnosis) than "missed heartbeats".
fn wedge_decision(
    last_pong_at: Instant,
    now: Instant,
    ping_interval: Duration,
    missed_pong_threshold: u32,
    parked_count: usize,
    parked_kill_threshold: usize,
) -> Option<WedgeReason> {
    if parked_count >= parked_kill_threshold {
        return Some(WedgeReason::ParkedThreads);
    }
    if now.saturating_duration_since(last_pong_at) >= ping_interval * missed_pong_threshold {
        return Some(WedgeReason::MissedHeartbeats);
    }
    None
}

/// A focus fact routed to an outpost, held until the outpost is ready to
/// receive it (decision D13).
struct PendingFact {
    trace_id: TraceId,
    observed_at_ms: u64,
    fact: DeliveredFact,
}

/// The per-outpost queue of focus facts that arrived while the outpost was
/// still spawning (decision D13). One slot per category, each newest-wins:
/// three focus changes during one spawn deliver one announcement, the current
/// one. Kept as a pure type with its own unit tests, the same split
/// [`idle_decision`] and [`wedge_decision`] use, so the queueing policy is
/// testable without a live outpost.
///
/// The MSAA and UIA focus facts get *separate* slots, not one shared focus
/// slot. Both backends can report the same focus, and the app outpost resolves
/// a real per-window verdict for each fact independently; which fact is the one
/// that actually announces depends on the window's backend — a UIA window's
/// UIA fact, an MSAA window's MSAA fact. That only works if both facts survive
/// the spawn: a single shared slot let the later-arriving fact overwrite the
/// earlier, so the survivor could be the fact the verdict drops, announcing
/// nothing (found live for a UIA search box that fires no MSAA focus event at
/// all — its UIA fact was overwritten by a foreground-driven MSAA fact, which
/// then dropped against the UIA verdict). Keeping both lets whichever backend
/// owns the window announce, in either arrival order.
#[derive(Default)]
struct PendingFacts {
    foreground: Option<PendingFact>,
    msaa_focus: Option<PendingFact>,
    uia_focus: Option<PendingFact>,
    menu_popup: Option<PendingFact>,
}

impl PendingFacts {
    /// Stores `fact` in its category's slot, overwriting any older fact there
    /// (newest-wins). The MSAA and UIA focus facts have distinct slots (see the
    /// type's doc), so a spawn that saw both delivers both.
    fn store(&mut self, trace_id: TraceId, observed_at_ms: u64, fact: DeliveredFact) {
        let pending = PendingFact {
            trace_id,
            observed_at_ms,
            fact,
        };
        let slot = match pending.fact {
            DeliveredFact::Foreground { .. } => &mut self.foreground,
            DeliveredFact::MsaaFocus { .. } => &mut self.msaa_focus,
            DeliveredFact::UiaFocus { .. } => &mut self.uia_focus,
            DeliveredFact::MenuPopup { .. } => &mut self.menu_popup,
        };
        *slot = Some(pending);
    }

    /// Takes the queued facts in the fixed flush order foreground, MSAA focus,
    /// UIA focus, menu-popup, leaving every slot empty. The order is a stable
    /// default, not a correctness requirement: each focus fact resolves its own
    /// real per-window verdict on its per-fact thread, so exactly one backend
    /// announces regardless of which flushes first (the outpost no longer
    /// depends on a provisional MSAA-first ordering the way it did before facts
    /// resolved real verdicts).
    fn drain(&mut self) -> Vec<PendingFact> {
        [
            self.foreground.take(),
            self.msaa_focus.take(),
            self.uia_focus.take(),
            self.menu_popup.take(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// Spawns the coarse background thread that sweeps idle outposts even when
/// no foreground change happens to trigger one — a long-backgrounded
/// application's outpost still needs to be reaped eventually. Lives for the
/// process's whole life, like the supervisor itself.
fn spawn_sweep_thread(shared: &Arc<SupervisorShared>) {
    let shared = Arc::clone(shared);
    let _ = thread::Builder::new()
        .name("verbatim-outpost-sweep".to_owned())
        .spawn(move || {
            loop {
                thread::sleep(SWEEP_INTERVAL);
                shared.sweep_idle();
            }
        });
}

/// Spawns the dedicated heartbeat thread that pings every live outpost on
/// [`PING_INTERVAL`] and kills-and-respawns any [`wedge_decision`] judges
/// wedged (recovery ladder rung 3). A separate thread rather than folding
/// into the idle sweep because the two run on genuinely different
/// timescales: idle retirement checks every [`SWEEP_INTERVAL`] (tens of
/// seconds is plenty for a memory-use mitigation), while a wedge needs to be
/// caught within a handful of [`PING_INTERVAL`]s to matter to a screen
/// reader user waiting on a response. Lives for the process's whole life,
/// like the supervisor itself and the sweep thread.
fn spawn_heartbeat_thread(shared: &Arc<SupervisorShared>) {
    let shared = Arc::clone(shared);
    let _ = thread::Builder::new()
        .name("verbatim-outpost-heartbeat".to_owned())
        .spawn(move || {
            loop {
                thread::sleep(PING_INTERVAL);
                shared.heartbeat_tick();
            }
        });
}

/// Whether `pid` names a process that is still running. Used before
/// respawning an outpost whose process just exited, so a dead application's
/// outpost is retired instead of resurrected to watch a pid that no longer
/// exists. Pid reuse is a known, accepted imprecision here, the same trade
/// every Win32 API taking a bare pid makes.
fn process_is_alive(pid: u32) -> bool {
    // SAFETY: OpenProcess with a query-only access right fails safely on an
    // invalid or inaccessible pid; the handle is closed before returning.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut exit_code = 0u32;
        let alive = GetExitCodeProcess(handle, &raw mut exit_code).is_ok()
            && exit_code == STILL_ACTIVE.0.cast_unsigned();
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        alive
    }
}

/// Forwards outpost messages until the pipe closes, then requests a
/// respawn-if-alive decision. Two message kinds are intercepted:
///
/// - A `Pong` is consumed here, never forwarded: it feeds
///   [`SupervisorShared::record_pong`] (the heartbeat bookkeeping
///   [`heartbeat_tick`](SupervisorShared::heartbeat_tick) reads), and carries
///   nothing the app needs.
/// - A `Ready` is intercepted to flush any focus facts queued while the
///   outpost was spawning ([`SupervisorShared::on_ready`], decision D13), but
///   is *also* still forwarded to the app, which updates its status mirror
///   from it.
fn reader_loop(
    shared: &Arc<SupervisorShared>,
    generation: u64,
    target_pid: Pid,
    from_outpost: File,
) {
    let mut reader = BufReader::new(from_outpost);
    while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
        if let OutpostToSupervisor::Pong { parked_count, .. } = message {
            shared.record_pong(target_pid, generation, parked_count);
            continue;
        }
        if matches!(message, OutpostToSupervisor::Ready { .. }) {
            shared.on_ready(target_pid, generation);
            // Fall through: the app still wants the Ready for its status mirror.
        }
        if shared
            .events_tx
            .send(OutpostMessage::Event(target_pid, Box::new(message)))
            .is_err()
        {
            return; // The app dropped the receiver; stop without respawning.
        }
    }
    // Reached on end of stream or a pipe error: the outpost has exited.
    shared.respawn_if_alive(target_pid, generation);
}

/// Forwards listener messages until its pipe closes, then respawns the
/// listener (decision D13). Unlike [`reader_loop`], a `FocusFact` is routed to
/// the target's own outpost and never forwarded to the app; a `Pong` feeds the
/// listener's own heartbeat bookkeeping; `Ready` and `Fault` are logged. The
/// listener sends nothing else.
fn listener_reader_loop(shared: &Arc<SupervisorShared>, generation: u64, from_listener: File) {
    let mut reader = BufReader::new(from_listener);
    while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
        match message {
            OutpostToSupervisor::FocusFact {
                trace_id,
                observed_at_ms,
                fact,
            } => shared.route_fact(trace_id, observed_at_ms, fact),
            OutpostToSupervisor::Pong { .. } => shared.record_listener_pong(generation),
            OutpostToSupervisor::Ready { outpost_pid, .. } => {
                tracing::info!(%outpost_pid, "focus listener ready");
            }
            OutpostToSupervisor::Fault { detail } => {
                tracing::warn!(detail, "focus listener fault");
            }
            _ => {} // The listener sends nothing else.
        }
    }
    // Reached on end of stream or a pipe error: the listener has exited.
    shared.respawn_listener_if_current(generation);
}

/// The four pipe handles: parent and child ends of two anonymous pipes.
struct Pipes {
    /// Parent end: supervisor reads outpost messages.
    parent_in: File,
    /// Child end: outpost writes messages (its `--pipe-out`).
    child_out: HANDLE,
    /// Parent end: supervisor writes commands.
    parent_out: File,
    /// Child end: outpost reads commands (its `--pipe-in`).
    child_in: HANDLE,
}

impl Pipes {
    /// Creates both pipes with inheritable child ends and non-inheritable
    /// parent ends.
    fn create() -> io::Result<Self> {
        // Command pipe: supervisor writes (parent_out), outpost reads (child_in).
        let (child_in, parent_out) = anonymous_pipe(PipeInherit::Read)?;
        // Event pipe: outpost writes (child_out), supervisor reads (parent_in).
        let (child_out, parent_in) = anonymous_pipe(PipeInherit::Write)?;
        // SAFETY: the parent ends are valid pipe handles we own.
        let parent_out = unsafe { File::from_raw_handle(parent_out.0 as RawHandle) };
        let parent_in = unsafe { File::from_raw_handle(parent_in.0 as RawHandle) };
        Ok(Self {
            parent_in,
            child_out,
            parent_out,
            child_in,
        })
    }

    /// Closes the parent's copies of the child-end handles after the child has
    /// inherited them.
    fn close_child_ends(&self) {
        close_handle(self.child_in);
        close_handle(self.child_out);
    }
}

/// Which end of an anonymous pipe should be inheritable by a child.
#[derive(Clone, Copy)]
enum PipeInherit {
    /// The read end is inheritable (the child reads).
    Read,
    /// The write end is inheritable (the child writes).
    Write,
}

/// Creates one anonymous pipe, returning `(child_end, parent_end)` with the
/// child end marked inheritable and the parent end not.
fn anonymous_pipe(inherit: PipeInherit) -> io::Result<(HANDLE, HANDLE)> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    // SAFETY: both out-handles are written by CreatePipe before use; the
    // security attributes live for the duration of the call.
    unsafe {
        CreatePipe(
            &raw mut read,
            &raw mut write,
            Some(&raw const attributes),
            0,
        )
        .map_err(to_io)?;
    }
    // CreatePipe made both ends inheritable; clear inheritance on the parent's.
    let (child_end, parent_end) = match inherit {
        PipeInherit::Read => (read, write),
        PipeInherit::Write => (write, read),
    };
    // SAFETY: `parent_end` is a valid handle just created.
    unsafe {
        SetHandleInformation(parent_end, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)).map_err(to_io)?;
    }
    Ok((child_end, parent_end))
}

/// Creates a job object with kill-on-close and the per-process memory cap.
fn create_job() -> io::Result<OwnedHandle> {
    // SAFETY: CreateJobObjectW with null attributes and name creates an
    // unnamed job; the returned handle is validated before use.
    let job = unsafe { CreateJobObjectW(None, PWSTR::null()).map_err(to_io)? };
    if job.is_invalid() {
        return Err(io::Error::other(
            "CreateJobObjectW returned an invalid handle",
        ));
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    limits.ProcessMemoryLimit = OUTPOST_MEMORY_CAP;
    // SAFETY: `limits` is a correctly sized JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    // matching the information class.
    unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
        )
        .map_err(to_io)?;
    }
    // SAFETY: `job` is a valid job handle we now own.
    Ok(unsafe { OwnedHandle::from_raw_handle(job.0 as RawHandle) })
}

/// The handles and id a spawned process yields.
struct Spawned {
    process: HANDLE,
    thread: HANDLE,
    pid: u32,
}

/// Creates the outpost process suspended, inheriting handles, and assigns it to
/// `job` before it runs. When `log_handle` is `Some`, the child's standard
/// output and error are redirected to it (its standard input too, harmlessly —
/// the outpost reads its command pipe, never stdin), so the outpost's own
/// `tracing` output lands in a file the harness can read; the handle must be
/// inheritable (see [`open_child_log`]).
fn spawn_suspended(
    command_line: &str,
    job: &OwnedHandle,
    log_handle: Option<HANDLE>,
) -> io::Result<Spawned> {
    let mut command: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut startup = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or(0),
        ..Default::default()
    };
    if let Some(log) = log_handle {
        startup.dwFlags |= STARTF_USESTDHANDLES;
        startup.hStdInput = log;
        startup.hStdOutput = log;
        startup.hStdError = log;
    }
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: `command` is a NUL-terminated writable UTF-16 buffer; startup and
    // info are correctly sized; bInheritHandles is true so the inheritable
    // child pipe ends (and the log handle, when set) pass to the child.
    unsafe {
        CreateProcessW(
            None,
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_SUSPENDED | CREATE_NO_WINDOW,
            None,
            None,
            &raw const startup,
            &raw mut info,
        )
        .map_err(to_io)?;
        // Assign to the job while still suspended, so it is contained before it
        // runs. The job handle is the OwnedHandle's raw value.
        let job_handle = HANDLE(job_raw(job));
        AssignProcessToJobObject(job_handle, info.hProcess).map_err(to_io)?;
    }
    Ok(Spawned {
        process: info.hProcess,
        thread: info.hThread,
        pid: info.dwProcessId,
    })
}

/// Returns the raw `HANDLE` value of an `OwnedHandle` without consuming it.
fn job_raw(job: &OwnedHandle) -> *mut c_void {
    use std::os::windows::io::AsRawHandle;
    job.as_raw_handle().cast()
}

fn close_handle(handle: HANDLE) {
    if handle.is_invalid() || handle == INVALID_HANDLE_VALUE {
        return;
    }
    // SAFETY: `handle` is a valid handle we own and are done with.
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(handle);
    }
}

/// Opens (creating if needed) the per-role log file a spawned child's stderr is
/// redirected into, returning an inheritable, append-mode handle — or `None` if
/// the logs directory or file could not be created, since outpost logging is
/// diagnostics and must never fail a spawn. `file_stem` names the file
/// (`outpost-<target pid>` or `listener`) inside a `logs` directory next to the
/// outpost executable, where `cargo xtask vm logs` and the E2E harness can read
/// it. Append mode so a crashed outpost's log and its respawn's both survive in
/// one file. The caller passes the returned handle to [`spawn_suspended`] and
/// closes its own copy afterward (the child keeps its inherited copy).
fn child_log_handle(exe_path: &Path, file_stem: &str) -> Option<HANDLE> {
    let dir = exe_path.parent()?.join("logs");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::warn!(%error, "could not create the outpost logs directory");
        return None;
    }
    let path = dir.join(format!("{file_stem}.log"));
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    // SAFETY: `wide` is a NUL-terminated path alive across the call; the
    // attributes live for the call; append-mode writes are atomic at end of
    // file, so concurrent writers (a respawned outpost) do not interleave.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_APPEND_DATA.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            Some(&raw const attributes),
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match handle {
        Ok(handle) if !handle.is_invalid() => Some(handle),
        Ok(handle) => {
            close_handle(handle);
            None
        }
        Err(error) => {
            tracing::warn!(%error, "could not open an outpost log file");
            None
        }
    }
}

fn to_io(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_decision_is_false_before_the_threshold() {
        let start = Instant::now();
        let now = start + Duration::from_secs(119);
        assert!(!idle_decision(start, now, Duration::from_mins(2)));
    }

    #[test]
    fn idle_decision_is_true_at_and_past_the_threshold() {
        let start = Instant::now();
        assert!(idle_decision(
            start,
            start + Duration::from_mins(2),
            Duration::from_mins(2)
        ));
        assert!(idle_decision(
            start,
            start + Duration::from_secs(200),
            Duration::from_mins(2)
        ));
    }

    #[test]
    fn idle_decision_never_panics_when_now_precedes_last_foreground() {
        // saturating_duration_since guards against a caller-supplied `now`
        // that is somehow earlier than `last_foreground_at`.
        let start = Instant::now();
        let earlier = start.checked_sub(Duration::from_secs(5)).unwrap_or(start);
        assert!(!idle_decision(start, earlier, Duration::from_mins(2)));
    }

    #[test]
    fn wedge_decision_is_none_when_recently_ponged_and_unparked() {
        let start = Instant::now();
        let now = start + Duration::from_secs(1);
        assert_eq!(
            wedge_decision(start, now, Duration::from_secs(3), 3, 0, 8),
            None
        );
    }

    #[test]
    fn wedge_decision_is_none_just_before_the_missed_pong_threshold() {
        let start = Instant::now();
        let now = (start + Duration::from_secs(3) * 3)
            .checked_sub(Duration::from_millis(1))
            .expect("computable");
        assert_eq!(
            wedge_decision(start, now, Duration::from_secs(3), 3, 0, 8),
            None
        );
    }

    #[test]
    fn wedge_decision_is_missed_heartbeats_at_and_past_the_threshold() {
        let start = Instant::now();
        let interval = Duration::from_secs(3);
        assert_eq!(
            wedge_decision(start, start + interval * 3, interval, 3, 0, 8),
            Some(WedgeReason::MissedHeartbeats)
        );
        assert_eq!(
            wedge_decision(start, start + interval * 10, interval, 3, 0, 8),
            Some(WedgeReason::MissedHeartbeats)
        );
    }

    #[test]
    fn wedge_decision_is_none_just_below_the_parked_threshold() {
        let start = Instant::now();
        assert_eq!(
            wedge_decision(start, start, Duration::from_secs(3), 3, 7, 8),
            None
        );
    }

    #[test]
    fn wedge_decision_is_parked_threads_at_and_past_the_threshold() {
        let start = Instant::now();
        assert_eq!(
            wedge_decision(start, start, Duration::from_secs(3), 3, 8, 8),
            Some(WedgeReason::ParkedThreads)
        );
        assert_eq!(
            wedge_decision(start, start, Duration::from_secs(3), 3, 50, 8),
            Some(WedgeReason::ParkedThreads)
        );
    }

    #[test]
    fn wedge_decision_prefers_parked_threads_when_both_conditions_hold() {
        // Parked threads is the more specific, more actionable diagnosis, so
        // it wins when an outpost has both missed its pongs and leaked
        // threads.
        let start = Instant::now();
        let interval = Duration::from_secs(3);
        assert_eq!(
            wedge_decision(start, start + interval * 5, interval, 3, 8, 8),
            Some(WedgeReason::ParkedThreads)
        );
    }

    #[test]
    fn wedge_decision_never_panics_when_now_precedes_last_pong() {
        let start = Instant::now();
        let earlier = start.checked_sub(Duration::from_secs(5)).unwrap_or(start);
        assert_eq!(
            wedge_decision(start, earlier, Duration::from_secs(3), 3, 0, 8),
            None
        );
    }

    fn uia_focus(runtime: i32) -> DeliveredFact {
        DeliveredFact::UiaFocus {
            hwnd: 0,
            snapshot: crate::protocol::UiaSnapshotFact {
                runtime_id: vec![runtime],
                role: verbatim_model::Role::Button,
                name: None,
                value: None,
                states: verbatim_model::StateSet::new(),
                details: verbatim_model::NodeDetails::default(),
            },
        }
    }

    fn msaa_focus(id_child: i32) -> DeliveredFact {
        DeliveredFact::MsaaFocus {
            hwnd: 9,
            id_object: -4,
            id_child,
        }
    }

    #[test]
    fn pending_facts_keep_newest_within_each_backend_and_survive_both() {
        let mut pending = PendingFacts::default();
        // Two UIA focus changes and two MSAA focus changes during one spawn:
        // each backend's slot keeps only its newest, but both backends survive
        // — a shared slot would have lost one, and the app outpost needs both
        // to arbitrate independently.
        pending.store(TraceId::mint(), 1, uia_focus(1));
        pending.store(TraceId::mint(), 2, uia_focus(2));
        pending.store(TraceId::mint(), 3, msaa_focus(10));
        pending.store(TraceId::mint(), 4, msaa_focus(20));
        let drained = pending.drain();
        assert_eq!(drained.len(), 2, "one MSAA and one UIA focus fact survive");
        // Drain order is MSAA focus before UIA focus.
        assert_eq!(drained[0].observed_at_ms, 4);
        assert_eq!(drained[0].fact, msaa_focus(20));
        assert_eq!(drained[1].observed_at_ms, 2);
        assert_eq!(drained[1].fact, uia_focus(2));
    }

    #[test]
    fn pending_facts_drain_in_foreground_msaa_uia_menu_order() {
        let mut pending = PendingFacts::default();
        // Stored out of order; must drain foreground, MSAA focus, UIA focus,
        // then menu-popup.
        pending.store(
            TraceId::mint(),
            40,
            DeliveredFact::MenuPopup {
                hwnd: 3,
                id_object: -3,
                id_child: 0,
            },
        );
        pending.store(TraceId::mint(), 30, uia_focus(2));
        pending.store(TraceId::mint(), 20, msaa_focus(5));
        pending.store(TraceId::mint(), 10, DeliveredFact::Foreground { hwnd: 1 });
        let order: Vec<u64> = pending.drain().iter().map(|p| p.observed_at_ms).collect();
        assert_eq!(order, vec![10, 20, 30, 40]);
    }

    #[test]
    fn pending_facts_drain_leaves_slots_empty() {
        let mut pending = PendingFacts::default();
        pending.store(TraceId::mint(), 1, DeliveredFact::Foreground { hwnd: 1 });
        assert_eq!(pending.drain().len(), 1);
        assert!(pending.drain().is_empty(), "a second drain finds nothing");
    }
}
