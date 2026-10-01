//! The lifecycle owner: one thread that makes every lifecycle decision and
//! owns the per-application records and the listener record (outpost
//! redesign, "The supervisor"). Everything else only reports to it.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, select, tick};
use verbatim_model::{OutpostId, Pid, TraceId};

use crate::protocol::{
    DeliveredFact, ListenerFact, OutpostToSupervisor, SupervisorToOutpost, read_message,
};

use super::policy::{
    CrashHistory, HeldFact, HeldFacts, WedgeReason, retirement_decision, wedge_decision,
};
use super::process::{self, Launched, Role};
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
        fact: ListenerFact,
    },
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
}

/// A running child: its process, held so ending the record kills it, and
/// its writer.
pub(super) struct Child {
    process: Launched,
    writer: WriterHandle,
}

/// What the owner thread starts with.
pub(super) struct Setup {
    pub(super) exe_path: PathBuf,
    pub(super) events_tx: Sender<OutpostMessage>,
    pub(super) writers: Writers,
    pub(super) own_tx: Sender<OwnerEvent>,
    pub(super) events: Receiver<OwnerEvent>,
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
    events_tx: Sender<OutpostMessage>,
    writers: Writers,
    own_tx: Sender<OwnerEvent>,
    /// Numbers every launch, outposts and listener alike; never reused.
    generation: u64,
    records: HashMap<Pid, Record>,
    listener: Option<ListenerRecord>,
    crashes: HashMap<Pid, CrashHistory>,
    attention: Option<Pid>,
    holding: BTreeSet<OutpostId>,
    ping_seq: u64,
    own_pid: Pid,
}

/// Starts the owner thread, which starts the listener at once.
pub(super) fn start(setup: Setup) -> io::Result<()> {
    let Setup {
        exe_path,
        events_tx,
        writers,
        own_tx,
        events,
    } = setup;
    let mut owner = Owner {
        exe_path,
        events_tx,
        writers,
        own_tx,
        generation: 0,
        records: HashMap::new(),
        listener: None,
        crashes: HashMap::new(),
        attention: None,
        holding: BTreeSet::new(),
        ping_seq: 0,
        own_pid: Pid(std::process::id()),
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
            OwnerEvent::EnsureSpawned(pid) => {
                if !self.records.contains_key(&pid) && !self.respawn_stopped(pid) {
                    self.start_outpost(pid, None);
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
            OwnerEvent::Fact {
                trace_id,
                observed_at_ms,
                fact,
            } => self.route_fact(trace_id, observed_at_ms, fact),
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

    /// Records `pid` as starting and hands its launch to a helper thread.
    fn start_outpost(&mut self, pid: Pid, first: Option<HeldFact>) {
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
        let exe_path = self.exe_path.clone();
        let events_tx = self.events_tx.clone();
        let own_tx = self.own_tx.clone();
        let writers = self.writers.clone();
        let spawned = thread::Builder::new()
            .name("verbatim-launch".to_owned())
            .spawn(move || {
                match launch_child(&exe_path, outpost, role, &events_tx, &writers) {
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
                child.writer.close();
                drop(child.process);
            }
            return;
        };
        match result {
            Ok(child) => {
                if let Some(record) = self.records.get_mut(&pid) {
                    record.child = Some(child);
                    record.last_pong_at = Instant::now();
                }
            }
            Err(error) => {
                tracing::error!(%error, %pid, "failed to launch an outpost");
                self.records.remove(&pid);
            }
        }
    }

    /// Releases held facts on `Ready`, in arrival order. A replacement
    /// listener's `Ready` is reported to the app: facts were lost in the gap,
    /// so the app asks for the current focus.
    fn on_ready(&mut self, outpost: OutpostId) {
        if let Some(listener) = self
            .listener
            .as_ref()
            .filter(|listener| listener.outpost == outpost)
        {
            tracing::info!(%outpost, "focus listener ready");
            if listener.replacement {
                let _ = self.events_tx.send(OutpostMessage::ListenerReplaced);
            }
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
    fn route_fact(&mut self, trace_id: TraceId, observed_at_ms: u64, fact: ListenerFact) {
        let pid = fact.pid();
        if matches!(fact.fact, DeliveredFact::Foreground { .. })
            && let Some(history) = self.crashes.get_mut(&pid)
        {
            history.reset();
        }
        let held = HeldFact {
            trace_id,
            observed_at_ms,
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
                    self.start_outpost(pid, Some(held));
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
            tracing::warn!(%outpost, "focus listener exited; replacing it");
            self.end_listener();
            self.start_listener(true);
            return;
        }
        if let Some(pid) = self.pid_of(outpost) {
            self.end(pid, EndReason::Exited);
            self.after_crash(pid);
        }
        // Otherwise the owner already ended it, and the app already heard.
    }

    /// Ends `pid`'s outpost: removes its record and writer, closes its job
    /// handle, which kills it if it is still running, and tells the app.
    fn end(&mut self, pid: Pid, reason: EndReason) {
        let Some(record) = self.records.remove(&pid) else {
            return;
        };
        self.writers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&record.outpost);
        if let Some(child) = record.child {
            child.writer.close();
            drop(child.process);
        }
        tracing::info!(outpost = %record.outpost, %pid, %reason, "outpost ended");
        let _ = self.events_tx.send(OutpostMessage::Ended {
            outpost: record.outpost,
            target_pid: pid,
            reason,
        });
    }

    /// After a crash or a kill: replace the outpost at once only if its
    /// application holds attention, and stop replacing it after repeated
    /// crashes until the next foreground change to it. An application that
    /// has itself exited is left alone.
    fn after_crash(&mut self, pid: Pid) {
        if !process::process_is_alive(pid) {
            self.crashes.remove(&pid);
            return;
        }
        let stopped =
            self.crashes
                .entry(pid)
                .or_default()
                .record(Instant::now(), CRASH_LIMIT, CRASH_WINDOW);
        if stopped {
            tracing::error!(%pid, "outpost crashed repeatedly; not replacing it until the next foreground change to its application");
            return;
        }
        if self.attention == Some(pid) {
            self.start_outpost(pid, None);
        }
    }

    fn end_listener(&mut self) {
        if let Some(listener) = self.listener.take()
            && let Some(child) = listener.child
        {
            child.writer.close();
            drop(child.process);
        }
    }

    /// Pings every running child and ends any that is wedged.
    fn heartbeat(&mut self) {
        let now = Instant::now();
        self.ping_seq += 1;
        let ping = SupervisorToOutpost::Ping { seq: self.ping_seq };
        let mut wedged = Vec::new();
        for (&pid, record) in &self.records {
            let Some(child) = &record.child else { continue };
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
        for (pid, reason) in wedged {
            tracing::warn!(%pid, %reason, "ending a wedged outpost");
            self.end(pid, EndReason::Killed);
            self.after_crash(pid);
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
                        self.end_listener();
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
        }
    }
}

/// Queues a routed fact for an outpost.
fn deliver(writer: &WriterHandle, held: HeldFact) {
    let key = held.fact.key();
    let command = SupervisorToOutpost::DeliverFact {
        trace_id: held.trace_id,
        observed_at_ms: held.observed_at_ms,
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
    outpost: OutpostId,
    role: Role,
    events_tx: &Sender<OutpostMessage>,
    writers: &Writers,
) -> io::Result<(Child, File)> {
    let (launched, pipes) = process::launch(exe_path, role)?;
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

/// An outpost's reader: stamps every node id with the outpost's id, forwards
/// each message to the app, reports `Ready` and pongs to the owner, and
/// reports the pipe's end once everything has been forwarded.
fn read_outpost(
    outpost: OutpostId,
    pid: Pid,
    from_child: File,
    events_tx: &Sender<OutpostMessage>,
    own_tx: &Sender<OwnerEvent>,
) {
    let mut reader = BufReader::new(from_child);
    while let Ok(Some(mut message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
        // Node ids name the incarnation whose pipe they arrived on, never
        // whatever the message body claims.
        message.assign_outpost(outpost);
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
            _ => {}
        }
        if events_tx
            .send(OutpostMessage::Event(pid, outpost, Box::new(message)))
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
                fact,
            } => OwnerEvent::Fact {
                trace_id,
                observed_at_ms,
                fact,
            },
            OutpostToSupervisor::Pong { .. } => OwnerEvent::Pong {
                outpost,
                abandoned: 0,
            },
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
