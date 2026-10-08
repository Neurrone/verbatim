//! The lifecycle owner: one thread that makes every lifecycle decision and
//! owns the per-application records and the listener record (outpost
//! redesign, "The supervisor"). Everything else only reports to it.

#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select, tick};
use verbatim_model::{OutpostId, Pid, TraceId};

use crate::protocol::{
    DeliveredFact, EventTiming, ListenerFact, OutpostToSupervisor, SupervisorToOutpost,
    read_message,
};

use super::ignored::IgnoredProcesses;
use super::policy::{
    CrashHistory, HeldFact, HeldFacts, WedgeReason, retirement_decision, wedge_decision,
};
use super::process::{self, Launched, Role, Target};
use super::retire::{Ending, ShutdownSummary, retire};
use super::writer::{self, Outgoing, WriterHandle};
use super::{
    ABANDONED_WORKER_LIMIT, CRASH_LIMIT, CRASH_WINDOW, EndReason, IDLE_RETIREMENT,
    MISSED_PONG_THRESHOLD, OutpostMessage, PING_INTERVAL, SWEEP_INTERVAL, Writers,
};

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
    /// From the listener's reader: a focus fact to route.
    Fact {
        trace_id: TraceId,
        observed_at_ms: u64,
        timing: EventTiming,
        fact: ListenerFact,
    },
    /// From the listener's reader: a menu or the Alt+Tab switcher closed.
    MenuOrSwitchEnded { ended_at_ms: u64 },
    /// From a reader: the child sent `Ready`.
    Ready(OutpostId),
    /// From a reader: the child answered a ping.
    Pong {
        outpost: OutpostId,
        abandoned: usize,
    },
    /// From a reader: the child's pipe closed, after everything it wrote was
    /// forwarded.
    PipeClosed(OutpostId),
    /// From a launch helper: the launch finished.
    Launched {
        outpost: OutpostId,
        result: io::Result<Child>,
    },
    /// From an outpost's reader: its target application exited.
    TargetExited(OutpostId),
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

/// What the owner thread starts with.
pub(super) struct Setup {
    pub(super) exe_path: PathBuf,
    pub(super) options: crate::OutpostOptions,
    pub(super) shutdown_limit: Duration,
    pub(super) events_tx: Sender<OutpostMessage>,
    pub(super) writers: Writers,
    pub(super) own_tx: Sender<OwnerEvent>,
    pub(super) events: Receiver<OwnerEvent>,
    /// The processes ignored entirely.
    pub(super) ignored: std::sync::Arc<IgnoredProcesses>,
}

/// One application's outpost: starting until its launch reports back, then
/// running.
struct Record {
    outpost: OutpostId,
    /// `None` while the launch helper is still at work.
    child: Option<Child>,
    /// Whether the outpost has sent `Ready`.
    ready: bool,
    /// Facts that arrived before `Ready`, released in order on `Ready`.
    held: HeldFacts,
    last_pong_at: Instant,
    abandoned: usize,
    /// When the application last held attention.
    last_attention_at: Instant,
}

/// The focus listener.
struct ListenerRecord {
    outpost: OutpostId,
    child: Option<Child>,
    last_pong_at: Instant,
    /// Whether this listener replaces an earlier one: facts were lost in the
    /// gap, so the attention application is asked to report again on `Ready`.
    replacement: bool,
}

/// The owner thread's state.
struct Owner {
    exe_path: PathBuf,
    /// What every outpost is launched with.
    options: crate::OutpostOptions,
    events_tx: Sender<OutpostMessage>,
    writers: Writers,
    own_tx: Sender<OwnerEvent>,
    /// Numbers every launch, outposts and listener alike; never reused.
    generation: u64,
    records: HashMap<Pid, Record>,
    listener: Option<ListenerRecord>,
    crashes: HashMap<Pid, CrashHistory>,
    /// The target applications' processes, held open while an application
    /// has a record or a crash history, so the pid each is known by names
    /// that process and no other ([`Target`]).
    targets: HashMap<Pid, Target>,
    attention: Option<Pid>,
    holding: BTreeSet<OutpostId>,
    ping_seq: u64,
    own_pid: Pid,
    /// How long an ending child has to exit after the shutdown message.
    shutdown_limit: Duration,
    /// The children being ended, each on a thread of its own
    /// ([`retire`]).
    retiring: Vec<JoinHandle<Ending>>,
    /// How the children that have finished ending ended.
    summary: ShutdownSummary,
    /// Set once Verbatim is exiting: where the summary goes once every
    /// child has ended.
    shutting_down: Option<Sender<ShutdownSummary>>,
    /// The processes ignored entirely: no outpost is started for them, and
    /// the listener, told of them, drops their facts.
    ignored: std::sync::Arc<IgnoredProcesses>,
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
        ignored,
    } = setup;
    let mut owner = Owner {
        exe_path,
        options,
        shutdown_limit,
        retiring: Vec::new(),
        summary: ShutdownSummary::default(),
        shutting_down: None,
        events_tx,
        writers,
        own_tx,
        generation: 0,
        records: HashMap::new(),
        listener: None,
        crashes: HashMap::new(),
        targets: HashMap::new(),
        attention: None,
        holding: BTreeSet::new(),
        ping_seq: 0,
        own_pid: Pid(std::process::id()),
        ignored,
    };
    thread::Builder::new()
        .name("verbatim-supervisor".to_owned())
        .spawn(move || {
            owner.start_listener(false);
            let heartbeat = tick(PING_INTERVAL);
            let sweep = tick(SWEEP_INTERVAL);
            loop {
                select! {
                    recv(events) -> event => {
                        let Ok(event) = event else { return };
                        owner.handle(event);
                        if owner.finish_shutdown() {
                            return;
                        }
                    }
                    recv(heartbeat) -> _ => owner.heartbeat(),
                    recv(sweep) -> _ => owner.sweep(),
                }
            }
        })?;
    Ok(())
}

impl Owner {
    fn handle(&mut self, event: OwnerEvent) {
        match event {
            OwnerEvent::Shutdown(reply) => self.shut_down_all(reply),
            OwnerEvent::EnsureSpawned(pid) => {
                if self.shutting_down.is_none() && !self.records.contains_key(&pid) {
                    if self.respawn_stopped(pid) {
                        tracing::info!(%pid, "no outpost is started: respawning is stopped after repeated crashes");
                        self.not_watched(pid);
                    } else {
                        let _ = self.start_outpost(pid, None);
                    }
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
            } => self.route_fact(trace_id, observed_at_ms, timing, fact),
            OwnerEvent::TargetExited(outpost) => {
                if let Some(pid) = self.pid_of(outpost) {
                    self.target_exited(pid, "target exited");
                }
            }
            OwnerEvent::MenuOrSwitchEnded { ended_at_ms } => {
                let _ = self
                    .events_tx
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
            OwnerEvent::PipeClosed(outpost) => self.on_pipe_closed(outpost),
            OwnerEvent::Launched { outpost, result } => self.on_launched(outpost, result),
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

    /// Records `pid` as starting and hands its launch to a helper thread,
    /// once its process is held ([`hold_target`](Self::hold_target)), and
    /// says whether it did. An application whose process has exited, or
    /// cannot be held, gets no outpost, and the app is told
    /// ([`OutpostMessage::NotWatched`]), so nothing waits for one.
    fn start_outpost(&mut self, pid: Pid, first: Option<HeldFact>) -> bool {
        if self.ignored.contains(pid) {
            tracing::debug!(%pid, "no outpost is started for a process Verbatim ignores");
            let _ = self
                .events_tx
                .send(OutpostMessage::NotWatched { target_pid: pid });
            return false;
        }
        if let Err(not_held) = self.hold_target(pid) {
            tracing::info!(%pid, %not_held, "no outpost is started for an application that is not running");
            self.release_target(pid);
            self.not_watched(pid);
            return false;
        }
        let outpost = self.next_outpost_id();
        let now = Instant::now();
        let mut held = HeldFacts::default();
        if let Some(fact) = first {
            held.hold(fact);
        }
        self.records.insert(
            pid,
            Record {
                outpost,
                child: None,
                ready: false,
                held,
                last_pong_at: now,
                abandoned: 0,
                last_attention_at: now,
            },
        );
        self.launch(outpost, Role::Outpost(pid));
        true
    }

    /// Tells the app that no outpost will watch `pid` for now
    /// ([`OutpostMessage::NotWatched`]), so nothing waits for one.
    fn not_watched(&self, pid: Pid) {
        let _ = self
            .events_tx
            .send(OutpostMessage::NotWatched { target_pid: pid });
    }

    /// Holds `pid`'s process, unless it is held already: the pid then names
    /// that process for as long as it is held. Fails when the process has
    /// exited, the one held included, or cannot be opened. A process that
    /// has exited is let go, with its crash history.
    fn hold_target(&mut self, pid: Pid) -> Result<(), process::NotHeld> {
        match self.targets.get(&pid) {
            Some(target) if target.has_exited() => {
                self.targets.remove(&pid);
                self.crashes.remove(&pid);
                Err(process::NotHeld::Exited)
            }
            Some(_) => Ok(()),
            None => {
                self.targets.insert(pid, Target::open(pid)?);
                Ok(())
            }
        }
    }

    /// Whether `pid`'s process has exited, or is not held.
    fn has_exited(&self, pid: Pid) -> bool {
        self.targets.get(&pid).is_none_or(Target::has_exited)
    }

    /// Lets `pid`'s process go once nothing is kept for it: no record and
    /// no crash history.
    fn release_target(&mut self, pid: Pid) {
        if !self.records.contains_key(&pid) && !self.crashes.contains_key(&pid) {
            self.targets.remove(&pid);
        }
    }

    /// `pid`'s application has exited: its outpost is ended, never replaced,
    /// and everything kept for it is forgotten.
    fn target_exited(&mut self, pid: Pid, why: &str) {
        self.crashes.remove(&pid);
        self.end_because(pid, EndReason::TargetExited, why);
        self.release_target(pid);
    }

    /// Records a new listener as starting and hands its launch to a helper.
    fn start_listener(&mut self, replacement: bool) {
        let outpost = self.next_outpost_id();
        self.listener = Some(ListenerRecord {
            outpost,
            child: None,
            last_pong_at: Instant::now(),
            replacement,
        });
        self.launch(outpost, Role::Listener);
    }

    /// Launches a child on a helper thread. The helper starts the child's
    /// writer and, for an outpost, registers it and sends `Started` to the
    /// app; then it reports [`OwnerEvent::Launched`]; only then does it start
    /// the reader. So the app hears of an incarnation before any of its
    /// messages, a query can reach the outpost as soon as the app has heard,
    /// and the owner always learns of the launch before the child's `Ready`
    /// or the end of its pipe.
    fn launch(&self, outpost: OutpostId, role: Role) {
        let ignored = self.ignored.list();
        let exe_path = self.exe_path.clone();
        let options = self.options;
        let events_tx = self.events_tx.clone();
        let own_tx = self.own_tx.clone();
        let writers = self.writers.clone();
        let spawned = thread::Builder::new()
            .name("verbatim-launch".to_owned())
            .spawn(move || {
                match launch_child(
                    &exe_path,
                    (options, &ignored),
                    outpost,
                    role,
                    &events_tx,
                    &writers,
                ) {
                    Ok((child, from_child)) => {
                        let _ = own_tx.send(OwnerEvent::Launched {
                            outpost,
                            result: Ok(child),
                        });
                        if let Err(error) =
                            start_reader(outpost, role, from_child, &events_tx, &own_tx)
                        {
                            // No reader, no messages: end it as though its
                            // pipe had closed.
                            tracing::error!(%error, %outpost, "failed to start a reader");
                            let _ = own_tx.send(OwnerEvent::PipeClosed(outpost));
                        }
                    }
                    Err(error) => {
                        let _ = own_tx.send(OwnerEvent::Launched {
                            outpost,
                            result: Err(error),
                        });
                    }
                }
            });
        if let Err(error) = spawned {
            let _ = self.own_tx.send(OwnerEvent::Launched {
                outpost,
                result: Err(error),
            });
        }
    }

    fn on_launched(&mut self, outpost: OutpostId, result: io::Result<Child>) {
        if let Some(listener) = self
            .listener
            .as_mut()
            .filter(|listener| listener.outpost == outpost)
        {
            match result {
                Ok(child) => {
                    listener.child = Some(child);
                    listener.last_pong_at = Instant::now();
                    if self.shutting_down.is_some() {
                        self.end_listener("Verbatim is exiting");
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "failed to launch the focus listener; retrying on the next heartbeat");
                    self.listener = None;
                }
            }
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
                if let Some(record) = self.records.get_mut(&pid) {
                    record.child = Some(child);
                    record.last_pong_at = Instant::now();
                }
                if self.shutting_down.is_some() {
                    self.end_quietly(pid, "Verbatim is exiting");
                }
            }
            Err(error) => {
                tracing::error!(%error, %pid, "failed to launch an outpost");
                self.records.remove(&pid);
                self.release_target(pid);
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
            let _ = self.events_tx.send(OutpostMessage::ListenerReady {
                replacement: listener.replacement,
            });
            return;
        }
        let Some(record) = self.record_of(outpost) else {
            return;
        };
        record.ready = true;
        let Some(writer) = record.child.as_ref().map(|child| child.writer.clone()) else {
            return;
        };
        for held in record.held.take() {
            deliver(&writer, held);
        }
    }

    /// Whether crashes have stopped respawning for `pid` until its next
    /// foreground change.
    fn respawn_stopped(&self, pid: Pid) -> bool {
        self.crashes.get(&pid).is_some_and(CrashHistory::stopped)
    }

    /// Routes a listener fact to its application's outpost: delivered at
    /// once if the outpost is ready, held if it is starting, and starting one
    /// if there is none — unless crashes have stopped respawning for that
    /// application. A foreground change earns a stopped application another
    /// try.
    fn route_fact(
        &mut self,
        trace_id: TraceId,
        observed_at_ms: u64,
        timing: EventTiming,
        fact: ListenerFact,
    ) {
        let pid = fact.pid();
        if self.ignored.contains(pid) {
            // The listener drops these already; nothing is routed either way.
            return;
        }
        if matches!(fact.fact, DeliveredFact::Foreground { .. })
            && let Some(history) = self.crashes.get_mut(&pid)
        {
            history.reset();
        }
        let held = HeldFact {
            trace_id,
            observed_at_ms,
            timing,
            fact: fact.into_delivered(),
        };
        match self.records.get_mut(&pid) {
            Some(record) => match record.child.as_ref().filter(|_| record.ready) {
                Some(child) => deliver(&child.writer, held),
                None => record.held.hold(held),
            },
            None => {
                if !held.fact.may_start_outpost() {
                    // A selection in an application with no outpost is not
                    // worth starting one for.
                } else if self.respawn_stopped(pid) {
                    tracing::debug!(%pid, "fact dropped: respawning is stopped after repeated crashes");
                } else {
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
            self.start_listener(true);
            return;
        }
        if let Some(pid) = self.pid_of(outpost) {
            if self.shutting_down.is_some() {
                self.end_quietly(pid, "its pipe closed while Verbatim is exiting");
                return;
            }
            self.ended_unexpectedly(pid, EndReason::Exited, &EndReason::Exited.to_string());
        }
        // Otherwise the owner already ended it, and the app already heard.
    }

    /// Ends `pid`'s outpost, which crashed or was ended as wedged, for
    /// `reason` and `why`, and replaces it if [`after_crash`](Self::after_crash)
    /// says so; but if its application has exited, the outpost ended
    /// because its target did, and is not replaced.
    fn ended_unexpectedly(&mut self, pid: Pid, reason: EndReason, why: &str) {
        if self.has_exited(pid) {
            self.target_exited(pid, why);
            return;
        }
        self.end_because(pid, reason, why);
        self.after_crash(pid);
    }

    /// Ends `pid`'s outpost: removes its record and writer, shuts the
    /// process down cleanly, killing it only if it does not exit in time
    /// ([`retire`], on a thread of its own), and tells the app at once.
    fn end(&mut self, pid: Pid, reason: EndReason) {
        self.end_because(pid, reason, &reason.to_string());
    }

    /// [`end`](Self::end), saying `why` in the log of the process's end.
    fn end_because(&mut self, pid: Pid, reason: EndReason, why: &str) {
        let Some(outpost) = self.take_record(pid, why) else {
            return;
        };
        tracing::info!(%outpost, %pid, %reason, "outpost ended");
        let _ = self.events_tx.send(OutpostMessage::Ended {
            outpost,
            target_pid: pid,
            reason,
        });
    }

    /// Ends `pid`'s outpost as [`end`](Self::end) does, without telling the
    /// app: Verbatim is exiting.
    fn end_quietly(&mut self, pid: Pid, why: &str) {
        let _ = self.take_record(pid, why);
    }

    /// Removes `pid`'s record and writer and retires its process, if it
    /// has one yet, for `why`. Returns the record's outpost id.
    fn take_record(&mut self, pid: Pid, why: &str) -> Option<OutpostId> {
        let record = self.records.remove(&pid)?;
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
        Some(record.outpost)
    }

    /// Ends `child` cleanly on a thread of its own ([`retire`]).
    fn retire_child(&mut self, child: Child, label: String, why: String) {
        let limit = self.shutdown_limit;
        let spawned = thread::Builder::new()
            .name("verbatim-retire".to_owned())
            .spawn(move || retire(child, &label, &why, limit));
        match spawned {
            Ok(handle) => self.retiring.push(handle),
            // The child went with the closure; dropping it closed its job
            // handle, which killed it.
            Err(error) => {
                tracing::error!(%error, "no thread to end a child cleanly; it was killed");
                self.summary.count(Ending::Killed);
            }
        }
    }

    /// Counts the endings that have finished, keeping those still waiting
    /// for their child; with `wait`, waits for every one.
    fn collect_endings(&mut self, wait: bool) {
        let (finished, waiting): (Vec<_>, Vec<_>) = self
            .retiring
            .drain(..)
            .partition(|handle| wait || handle.is_finished());
        self.retiring = waiting;
        for handle in finished {
            // A panic while ending a child leaves it to its job handle,
            // which was closed as the thread unwound: it was killed.
            let ending = handle.join().unwrap_or(Ending::Killed);
            self.summary.count(ending);
        }
    }

    /// Verbatim is exiting: no child is started from now on, and every
    /// child is shut down. Children still being launched are shut down as
    /// their launch reports back.
    fn shut_down_all(&mut self, reply: Sender<ShutdownSummary>) {
        tracing::info!("Verbatim is exiting: shutting down every outpost and the focus listener");
        self.shutting_down = Some(reply);
        let launched: Vec<Pid> = self
            .records
            .iter()
            .filter(|(_, record)| record.child.is_some())
            .map(|(pid, _)| *pid)
            .collect();
        for pid in launched {
            self.end_quietly(pid, "Verbatim is exiting");
        }
        if self
            .listener
            .as_ref()
            .is_some_and(|listener| listener.child.is_some())
        {
            self.end_listener("Verbatim is exiting");
        }
    }

    /// Once Verbatim is exiting and every child has been ended, waits for
    /// every ending to finish, sends the summary, and says the owner is
    /// done.
    fn finish_shutdown(&mut self) -> bool {
        if self.shutting_down.is_none() || !self.records.is_empty() || self.listener.is_some() {
            return false;
        }
        self.collect_endings(true);
        let summary = self.summary;
        tracing::info!(
            clean = summary.clean,
            exited = summary.exited,
            killed = summary.killed,
            "every outpost and the focus listener has ended"
        );
        if let Some(reply) = self.shutting_down.take() {
            let _ = reply.send(summary);
        }
        true
    }

    /// After a crash or a kill of an outpost whose application is still
    /// running: replace the outpost at once only if its application holds
    /// attention, and stop replacing it after repeated crashes until the
    /// next foreground change to it. The application's process stays held
    /// with its crash history.
    fn after_crash(&mut self, pid: Pid) {
        let stopped =
            self.crashes
                .entry(pid)
                .or_default()
                .record(Instant::now(), CRASH_LIMIT, CRASH_WINDOW);
        if stopped {
            tracing::error!(%pid, "outpost crashed repeatedly; not replacing it until the next foreground change to its application");
            // The app, told of the crash, may be waiting for a replacement
            // to ask for the focus.
            self.not_watched(pid);
            return;
        }
        if self.attention == Some(pid) {
            let _ = self.start_outpost(pid, None);
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

    /// Pings every running child and ends any that is wedged.
    fn heartbeat(&mut self) {
        self.collect_endings(false);
        if self.shutting_down.is_some() {
            return;
        }
        let now = Instant::now();
        self.ping_seq += 1;
        let ping = SupervisorToOutpost::Ping { seq: self.ping_seq };
        let mut wedged = Vec::new();
        let mut exited = Vec::new();
        for (&pid, record) in &self.records {
            let Some(child) = &record.child else { continue };
            // An application whose outpost could not tell it exited.
            if self.has_exited(pid) {
                exited.push(pid);
                continue;
            }
            let hung =
                record.abandoned >= ABANDONED_WORKER_LIMIT && process::application_is_hung(pid);
            match wedge_decision(
                record.last_pong_at,
                now,
                PING_INTERVAL,
                MISSED_PONG_THRESHOLD,
                record.abandoned,
                ABANDONED_WORKER_LIMIT,
                hung,
            ) {
                Some(reason) => wedged.push((pid, reason)),
                None => {
                    let _ = child.writer.push(Outgoing::Ping(ping.clone()));
                }
            }
        }
        for pid in exited {
            self.target_exited(pid, "target exited, seen by the supervisor");
        }
        for (pid, reason) in wedged {
            tracing::warn!(%pid, %reason, "ending a wedged outpost");
            self.ended_unexpectedly(
                pid,
                EndReason::Killed,
                &format!("ended as wedged: {reason}"),
            );
        }

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
                        self.start_listener(true);
                    } else {
                        let _ = child.writer.push(Outgoing::Ping(ping));
                    }
                }
            }
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
        // Crash histories of applications that have since exited.
        let gone: Vec<Pid> = self
            .crashes
            .keys()
            .copied()
            .filter(|pid| !self.records.contains_key(pid) && self.has_exited(*pid))
            .collect();
        for pid in gone {
            self.crashes.remove(&pid);
            self.release_target(pid);
        }
    }
}

/// Queues a routed fact for an outpost.
fn deliver(writer: &WriterHandle, held: HeldFact) {
    let key = held.fact.key();
    let command = SupervisorToOutpost::DeliverFact {
        trace_id: held.trace_id,
        observed_at_ms: held.observed_at_ms,
        timing: held.timing,
        fact: held.fact,
    };
    if let Err(error) = writer.push(Outgoing::Fact(key, command)) {
        tracing::warn!(%error, "a routed fact could not be queued");
    }
}

/// The launch helper's first half: launch the process and start its writer;
/// for an outpost, register the writer and tell the app it started. Returns
/// the child and the pipe its reader will read.
fn launch_child(
    exe_path: &std::path::Path,
    (options, ignored): (crate::OutpostOptions, &str),
    outpost: OutpostId,
    role: Role,
    events_tx: &Sender<OutpostMessage>,
    writers: &Writers,
) -> io::Result<(Child, File)> {
    let (launched, pipes) = process::launch(exe_path, role, options, ignored)?;
    tracing::info!(%outpost, process_id = %launched.process_id, ?role, "launched");
    let writer_name = match role {
        Role::Outpost(_) => "verbatim-outpost-writer",
        Role::Listener => "verbatim-listener-writer",
    };
    let writer = writer::start(pipes.to_child, writer_name)?;
    if let Role::Outpost(pid) = role {
        writers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(outpost, writer.clone());
        let _ = events_tx.send(OutpostMessage::Started {
            outpost,
            target_pid: pid,
        });
    }
    Ok((
        Child {
            process: launched,
            writer,
        },
        pipes.from_child,
    ))
}

/// The launch helper's second half: start the child's reader.
fn start_reader(
    outpost: OutpostId,
    role: Role,
    from_child: File,
    events_tx: &Sender<OutpostMessage>,
    own_tx: &Sender<OwnerEvent>,
) -> io::Result<()> {
    let events_tx = events_tx.clone();
    let own_tx = own_tx.clone();
    let name = match role {
        Role::Outpost(_) => "verbatim-outpost-reader",
        Role::Listener => "verbatim-listener-reader",
    };
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || match role {
            Role::Outpost(pid) => read_outpost(outpost, pid, from_child, &events_tx, &own_tx),
            Role::Listener => read_listener(outpost, from_child, &own_tx),
        })?;
    Ok(())
}

/// An outpost's reader: stamps every node id with the outpost's id, numbers
/// the messages that carry node ids as the outpost does, forwards each
/// message to the app with the position reached, reports `Ready` and pongs to
/// the owner, and reports the pipe's end once everything has been forwarded.
fn read_outpost(
    outpost: OutpostId,
    pid: Pid,
    from_child: File,
    events_tx: &Sender<OutpostMessage>,
    own_tx: &Sender<OwnerEvent>,
) {
    let mut reader = BufReader::new(from_child);
    let mut position = 0u64;
    loop {
        let mut message = match read_message::<_, OutpostToSupervisor>(&mut reader) {
            Ok(Some(message)) => message,
            Ok(None) => break,
            // A message too large or not a message: the outpost is treated
            // as failed, as when its pipe closes.
            Err(error) => {
                tracing::warn!(%outpost, %error, "an outpost's message could not be read");
                break;
            }
        };
        // Node ids name the incarnation whose pipe they arrived on, never
        // whatever the message body claims.
        message.assign_outpost(outpost);
        if message.carries_nodes() {
            position += 1;
        }
        match message {
            OutpostToSupervisor::Pong { parked_count, .. } => {
                let _ = own_tx.send(OwnerEvent::Pong {
                    outpost,
                    abandoned: parked_count,
                });
                continue;
            }
            OutpostToSupervisor::Ready { .. } => {
                let _ = own_tx.send(OwnerEvent::Ready(outpost));
            }
            OutpostToSupervisor::TargetExited => {
                let _ = own_tx.send(OwnerEvent::TargetExited(outpost));
                continue;
            }
            _ => {}
        }
        if events_tx
            .send(OutpostMessage::Event {
                pid,
                outpost,
                position,
                message: Box::new(message),
            })
            .is_err()
        {
            return; // The app is gone.
        }
    }
    let _ = own_tx.send(OwnerEvent::PipeClosed(outpost));
}

/// The listener's reader: focus facts, pongs, and `Ready` go to the owner;
/// faults are logged.
fn read_listener(outpost: OutpostId, from_child: File, own_tx: &Sender<OwnerEvent>) {
    let mut reader = BufReader::new(from_child);
    while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
        let event = match message {
            OutpostToSupervisor::FocusFact {
                trace_id,
                observed_at_ms,
                timing,
                fact,
            } => OwnerEvent::Fact {
                trace_id,
                observed_at_ms,
                timing,
                fact,
            },
            OutpostToSupervisor::Pong { .. } => OwnerEvent::Pong {
                outpost,
                abandoned: 0,
            },
            OutpostToSupervisor::MenuOrSwitchEnded { ended_at_ms } => {
                OwnerEvent::MenuOrSwitchEnded { ended_at_ms }
            }
            OutpostToSupervisor::Ready { .. } => OwnerEvent::Ready(outpost),
            OutpostToSupervisor::Fault { detail } => {
                tracing::warn!(detail, "focus listener fault");
                continue;
            }
            _ => continue, // The listener sends nothing else.
        };
        if own_tx.send(event).is_err() {
            return;
        }
    }
    let _ = own_tx.send(OwnerEvent::PipeClosed(outpost));
}
