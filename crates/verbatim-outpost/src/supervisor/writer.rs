//! Each child's writer: a [`Sender`] on its command pipe, so nothing that
//! makes a decision ever waits on a pipe (`docs/design/communication.md`,
//! section 4.3). The thread
//! that queues a command writes it itself when the pipe is free and
//! nothing waits; while a write is in flight, commands wait in a bounded
//! queue, written in order by the relay once that write's completion
//! reaches its port ([`super::relay`]).
//!
//! When the queue is full, each kind of command has its own policy:
//!
//! - a ping always gets through, past the bound, since a missed ping would
//!   get a healthy outpost killed, and so does the shutdown message, the
//!   last command a child is sent;
//! - a routed fact replaces any older fact still waiting for the same object
//!   and kind, and for a `WinEvent`'s fact other than a menu opening from
//!   the same thread (`DeliveredFact::key`); if there is none, the oldest waiting fact is dropped to make
//!   room, so the newest facts survive;
//! - a message of the MSAA hand-over between the listener and an outpost
//!   (`docs/design/focus-pipeline.md`, section 12.4) always gets through,
//!   past the bound, and keeps its place behind everything queued before
//!   it: the listener's "forwarding ended" must reach the outpost after
//!   every fact the listener sent before it, and is never lost; so does
//!   every part of a UIA focus's read (its marker, its read or drop, and
//!   the end of a listener's reads), since an outpost waits on a marker
//!   until its read or drop arrives;
//! - the idle barrier's query (`docs/design/communication.md`, section 6.7)
//!   always gets through, past the bound, behind everything queued before
//!   it, and no fact queued after it replaces one queued before it, which
//!   would move that fact behind the query;
//! - anything else, a query above all, fails at once, so its asker gets an
//!   outcome straight away instead of waiting behind a stuck outpost.
//!
//! The list of the nodes Core holds in an outpost is not a command of its
//! own (`docs/design/communication.md`, step 11): the reducer leaves the
//! latest list in the writer's slot ([`WriterHandle::leave_held`]), a newer
//! one replacing a list not yet sent, and whichever write to the outpost
//! comes next, a request from the reducer, a ping from the relay, or any
//! other command but the shutdown message, takes it and carries it in the
//! same write, after its own frame, so the command is handled first and the
//! list never costs a write of its own. While commands queued before the
//! list was left still wait, the list waits behind the last of them, so a
//! request the reducer issued before the list is always handled before it.
//! The relay pings every outpost every three seconds, so a list waits at
//! most that long once nothing is queued ahead of it.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::Arc;
use std::time::Instant;

use verbatim_process::overlapped::{Completion, Port};
use verbatim_process::transport::{Backlog, SendError, Sender};

use crate::protocol::{EventTiming, FactKey, SupervisorToOutpost, now_us, put_message};

/// How many commands may wait for one child before the overload policy
/// applies.
pub(super) const WRITER_CAPACITY: usize = 64;

/// One command waiting to be written, with what the overload policy needs
/// to know about it.
#[derive(Debug)]
pub(super) enum Outgoing {
    /// A liveness ping.
    Ping(SupervisorToOutpost),
    /// The shutdown message, [`SupervisorToOutpost::Shutdown`].
    Shutdown,
    /// A routed fact, keyed by the object and kind it concerns and, for an
    /// MSAA event other than a menu opening, the thread that raised it
    /// (`None` for a notification,
    /// which nothing replaces), with when verbatim.exe read it from the
    /// listener.
    Fact(Option<FactKey>, SupervisorToOutpost, Received),
    /// A message of the MSAA hand-over: `ForwardingEnded` to an outpost,
    /// `HookInstalled` or `HookRemoved` to the listener; and `CoreFocus`
    /// to the listener and the outposts concerned, which their judgments
    /// before a read depend on.
    HandOver(SupervisorToOutpost),
    /// A part of a UIA focus's read routed to an outpost, which waits on
    /// its marker, so it gets through as a hand-over message does, with
    /// when verbatim.exe read it from the listener.
    FocusRead(SupervisorToOutpost, Received),
    /// The idle barrier's query for a round,
    /// [`SupervisorToOutpost::IdleQuery`].
    IdleQuery(SupervisorToOutpost),
    /// Anything else: a query, an activation, a configuration command, with
    /// when it was queued.
    Other(SupervisorToOutpost, Received),
}

impl Outgoing {
    /// A routed fact for an outpost: a part of a UIA focus's read, which an
    /// outpost waits on in order, gets through as a hand-over message does
    /// and is never merged; any other fact is keyed by the object and kind
    /// it concerns and, for an MSAA event other than a menu opening, the
    /// thread that raised it.
    pub(super) fn delivered(held: super::policy::HeldFact) -> Self {
        let key = held.fact.key(held.win_event);
        let focus_read = held.fact.is_focus_read();
        let command = SupervisorToOutpost::DeliverFact {
            trace_id: held.trace_id,
            timing: held.timing,
            fact: held.fact,
            win_event: held.win_event,
        };
        if focus_read {
            Outgoing::FocusRead(command, held.received)
        } else {
            Outgoing::Fact(key, command, held.received)
        }
    }

    #[cfg(test)]
    fn command(&self) -> &SupervisorToOutpost {
        match self {
            Outgoing::Shutdown => &SupervisorToOutpost::Shutdown,
            Outgoing::Ping(command)
            | Outgoing::IdleQuery(command)
            | Outgoing::Fact(_, command, _)
            | Outgoing::HandOver(command)
            | Outgoing::FocusRead(command, _)
            | Outgoing::Other(command, _) => command,
        }
    }

    /// The command to write now, a fact or a request stamped with
    /// verbatim.exe's relay time up to this moment ([`Received::stamp`]),
    /// and a command to the listener with the moment its write begins
    /// ([`SupervisorToOutpost::stamp_sent`]).
    pub(super) fn relayed(self) -> SupervisorToOutpost {
        self.relayed_with(now_us)
    }

    /// [`relayed`](Self::relayed), with `now` the clock a command to the
    /// listener is stamped from.
    fn relayed_with(self, now: impl FnOnce() -> u64) -> SupervisorToOutpost {
        match self {
            Outgoing::Shutdown => SupervisorToOutpost::Shutdown,
            Outgoing::Fact(_, mut command, received)
            | Outgoing::FocusRead(mut command, received)
            | Outgoing::Other(mut command, received) => {
                match &mut command {
                    SupervisorToOutpost::DeliverFact { timing, .. }
                    | SupervisorToOutpost::Query { timing, .. } => received.stamp(timing),
                    _ => {}
                }
                command
            }
            Outgoing::Ping(mut command) | Outgoing::HandOver(mut command) => {
                command.stamp_sent(now);
                command
            }
            Outgoing::IdleQuery(command) => command,
        }
    }
}

/// When verbatim.exe took in something it relays to an outpost: a
/// listener's fact as its read completed, or a request as it was queued
/// (`docs/design/communication.md`, section 10.1). Read on two clocks: the monotonic
/// one, for verbatim.exe's own duration, and the wall clock the other
/// processes' stamps share, for the spans across processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Received {
    at: Instant,
    at_us: u64,
}

impl Received {
    /// Now.
    pub(super) fn now() -> Self {
        Self {
            at: Instant::now(),
            at_us: now_us(),
        }
    }

    /// When, on the clock the other processes' stamps share.
    pub(super) fn at_us(self) -> u64 {
        self.at_us
    }

    /// How long ago, in microseconds.
    pub(super) fn elapsed_us(self) -> u64 {
        u64::try_from(self.at.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    /// Fills in `timing`'s relay stamps: received then, relayed now. The
    /// duration is measured on the monotonic clock alone, so it does not
    /// depend on clocks agreeing across processes, and the time sent is
    /// derived from it.
    pub(super) fn stamp(self, timing: &mut EventTiming) {
        let relay_us = self.elapsed_us();
        timing.core_received_at_us = self.at_us;
        timing.core_relay_us = relay_us;
        timing.core_sent_at_us = self.at_us.saturating_add(relay_us);
    }
}

/// Why a command was not queued.
#[derive(Debug, PartialEq, Eq)]
pub enum QueueError {
    /// The queue is full and the command is not one that replaces or gets
    /// through.
    Full,
    /// The child has ended and its writer is closed.
    Closed,
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            QueueError::Full => "the outpost's command queue is full",
            QueueError::Closed => "the outpost has ended",
        })
    }
}

/// The waiting commands, and the slot for the held nodes' list. Pure, so
/// the overload policy is tested on its own.
#[derive(Debug, Default)]
pub(super) struct Queue {
    items: VecDeque<Outgoing>,
    /// The latest [`SupervisorToOutpost::NodesHeld`] not yet sent, which
    /// the next command written carries after its own frame, once every
    /// command queued before it was left has been written.
    held: Option<SupervisorToOutpost>,
    /// How many of the commands waiting were queued before `held` was
    /// left: the list goes with the last of them, or, with none, with the
    /// next command written.
    held_behind: usize,
}

impl Queue {
    /// Adds `item` under the overload policy for `capacity`.
    pub(super) fn push(&mut self, item: Outgoing, capacity: usize) -> Result<(), QueueError> {
        match item {
            Outgoing::Ping(_)
            | Outgoing::Shutdown
            | Outgoing::HandOver(_)
            | Outgoing::FocusRead(..)
            | Outgoing::IdleQuery(_) => {}
            Outgoing::Fact(ref key, ..) => {
                // Never one queued before the idle barrier's query.
                let after_query = self
                    .items
                    .iter()
                    .rposition(|waiting| matches!(waiting, Outgoing::IdleQuery(_)))
                    .map_or(0, |query| query + 1);
                let same = key.as_ref().and_then(|key| {
                    self.items
                        .iter()
                        .skip(after_query)
                        .position(
                            |waiting| matches!(waiting, Outgoing::Fact(Some(other), ..) if other == key),
                        )
                        .map(|index| index + after_query)
                });
                if let Some(index) = same {
                    self.remove(index);
                } else if self.items.len() >= capacity
                    && let Some(oldest) = self
                        .items
                        .iter()
                        .position(|waiting| matches!(waiting, Outgoing::Fact(..)))
                {
                    self.remove(oldest);
                }
                if self.items.len() >= capacity {
                    return Err(QueueError::Full);
                }
            }
            Outgoing::Other(..) => {
                if self.items.len() >= capacity {
                    return Err(QueueError::Full);
                }
            }
        }
        self.items.push_back(item);
        Ok(())
    }

    fn pop(&mut self) -> Option<Outgoing> {
        let item = self.items.pop_front()?;
        self.held_behind = self.held_behind.saturating_sub(1);
        Some(item)
    }

    /// Removes the command waiting at `index`, a fact merged or dropped.
    fn remove(&mut self, index: usize) {
        self.items.remove(index);
        if index < self.held_behind {
            self.held_behind -= 1;
        }
    }

    /// Leaves `held` in the slot, replacing a list not yet sent, behind
    /// every command waiting now.
    fn leave(&mut self, held: SupervisorToOutpost) {
        self.held = Some(held);
        self.held_behind = self.items.len();
    }
}

impl Backlog for Queue {
    type Item = Outgoing;
    type Refused = QueueError;

    fn push(&mut self, item: Outgoing) -> Result<(), QueueError> {
        Queue::push(self, item, WRITER_CAPACITY)
    }

    fn pop(&mut self) -> Option<Outgoing> {
        Queue::pop(self)
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Appends `item`'s frame, then the held nodes' list in the slot, if
    /// any and nothing queued before it was left still waits, which the
    /// item's write then carries; the shutdown message carries none, since
    /// the outpost is ending.
    fn encode(&mut self, item: Outgoing, buffer: &mut Vec<u8>) -> io::Result<()> {
        let carries = !matches!(item, Outgoing::Shutdown) && self.held_behind == 0;
        put_message(buffer, &item.relayed())?;
        if carries && let Some(held) = self.held.take() {
            let before = buffer.len();
            if let Err(error) = put_message(buffer, &held) {
                buffer.truncate(before);
                tracing::warn!(%error, "the held nodes could not be encoded; they are dropped");
            }
        }
        Ok(())
    }
}

/// The shared end of one child's writer: the relay and the reducer thread
/// send through it.
#[derive(Clone)]
pub(crate) struct WriterHandle {
    sender: Sender<Queue>,
}

impl WriterHandle {
    /// The writer for a child's command pipe, an overlapped end, whose
    /// writes that pend complete on `port`, the relay's, under `key`.
    pub(super) fn on_port(to_child: OwnedHandle, port: Arc<Port>, key: usize) -> io::Result<Self> {
        Ok(Self {
            sender: Sender::on_port(to_child, Queue::default(), port, key)?,
        })
    }

    /// Takes `completion`, of a write of this writer's that pended, from
    /// the relay: writes what waited behind it.
    pub(super) fn complete(&self, completion: &Completion) {
        self.sender.complete(completion);
    }

    /// Whether the writer has closed and written everything, or failed:
    /// no completion of its will come again.
    pub(super) fn is_finished(&self) -> bool {
        self.sender.is_finished()
    }

    /// Writes `item` now if the pipe is free, or queues it, without
    /// waiting.
    pub(super) fn push(&self, item: Outgoing) -> Result<(), QueueError> {
        self.sender.send(item).map_err(|error| match error {
            SendError::Refused(refused) => refused,
            SendError::Closed => QueueError::Closed,
        })
    }

    /// Leaves `held`, a [`SupervisorToOutpost::NodesHeld`], in the slot,
    /// replacing a list not yet sent: it writes nothing, and the next
    /// command written once every command waiting now has been written
    /// carries it.
    pub(super) fn leave_held(&self, held: SupervisorToOutpost) {
        self.sender.with_backlog(|queue| queue.leave(held));
    }

    /// Closes the writer: nothing more is accepted, and the pipe is closed
    /// once everything queued is written.
    pub(super) fn close(&self) {
        self.sender.close();
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::TraceId;

    use super::*;
    use crate::protocol::{DeliveredFact, UiaSnapshotFact};
    use crate::supervisor::policy::HeldFact;

    fn ping() -> Outgoing {
        Outgoing::Ping(SupervisorToOutpost::Ping {
            seq: 1,
            sent_at_us: 0,
        })
    }

    fn query() -> Outgoing {
        Outgoing::Other(
            SupervisorToOutpost::Query {
                trace_id: TraceId::mint(),
                request_id: 1,
                query: crate::protocol::Query::DumpTree { locations: false },
                timing: EventTiming::default(),
            },
            Received::now(),
        )
    }

    fn fact(hwnd: isize) -> Outgoing {
        Outgoing::Fact(
            Some(FactKey::Foreground(hwnd, 1)),
            SupervisorToOutpost::Ping {
                seq: hwnd.cast_unsigned() as u64,
                sent_at_us: 0,
            },
            Received::now(),
        )
    }

    fn seqs(queue: &mut Queue) -> Vec<u64> {
        std::iter::from_fn(|| queue.pop())
            .map(|item| match item.command() {
                SupervisorToOutpost::Ping { seq, .. } => *seq,
                _ => 0,
            })
            .collect()
    }

    #[test]
    fn a_full_queue_fails_a_query_at_once_but_lets_a_ping_through() {
        let mut queue = Queue::default();
        queue.push(query(), 1).expect("room for one");
        assert_eq!(queue.push(query(), 1), Err(QueueError::Full));
        assert_eq!(queue.push(ping(), 1), Ok(()));
    }

    #[test]
    fn a_fact_replaces_a_waiting_fact_for_the_same_object_and_moves_to_the_back() {
        let mut queue = Queue::default();
        queue.push(fact(1), 8).expect("queued");
        queue.push(fact(2), 8).expect("queued");
        queue.push(fact(1), 8).expect("queued");
        assert_eq!(seqs(&mut queue), vec![2, 1]);
    }

    #[test]
    fn a_new_fact_into_a_full_queue_drops_the_oldest_fact() {
        let mut queue = Queue::default();
        queue.push(fact(1), 2).expect("queued");
        queue.push(fact(2), 2).expect("queued");
        queue.push(fact(3), 2).expect("the oldest fact makes room");
        assert_eq!(seqs(&mut queue), vec![2, 3]);
    }

    /// A held nodes' list acknowledging `acknowledged`.
    fn held_list(acknowledged: u64) -> SupervisorToOutpost {
        SupervisorToOutpost::NodesHeld {
            nodes: vec![acknowledged],
            anchors: Vec::new(),
            acknowledged,
        }
    }

    /// A writer on a pipe of its own, and the child's end, read with one
    /// overlapped read at a time, so each read takes what one write
    /// brought, and only that.
    struct Child {
        port: Port,
        pipe: verbatim_process::overlapped::PortFile,
        reading: verbatim_process::overlapped::PipeIo,
    }

    impl Child {
        fn new() -> (WriterHandle, Self) {
            use verbatim_process::overlapped::{End, Flow, PipeIo, pipe};
            let (ours, theirs) = pipe(Flow::Inbound, 4096, End::Overlapped).expect("a pipe");
            let writer = WriterHandle::on_port(theirs, Arc::new(Port::new().expect("a port")), 2)
                .expect("a writer");
            let port = Port::new().expect("a port");
            let pipe = port.associate(ours, 5).expect("associated");
            let mut child = Self {
                port,
                pipe,
                reading: PipeIo::new(4096),
            };
            child.arm();
            (writer, child)
        }

        /// Starts the next read, before anything is written.
        fn arm(&mut self) {
            assert!(matches!(
                self.reading.read(&self.pipe),
                verbatim_process::overlapped::Started::Pending
            ));
        }

        /// Whether nothing has been written: every write is begun on the
        /// thread that sends, and a write completes the read in flight
        /// before it returns.
        fn nothing_written(&self) -> bool {
            let mut completions = verbatim_process::overlapped::Completions::with_capacity(4);
            self.port
                .wait(&mut completions, Some(std::time::Duration::ZERO))
                .expect("waited")
                == 0
        }

        /// What the next write brought, each frame named, and a read armed
        /// for the one after.
        fn one_write(&mut self) -> Vec<String> {
            let mut completions = verbatim_process::overlapped::Completions::with_capacity(4);
            assert_eq!(
                self.port
                    .wait(&mut completions, Some(std::time::Duration::from_secs(10)))
                    .expect("waited"),
                1
            );
            let completion = completions.iter().next().expect("one completion");
            self.reading
                .complete(&self.pipe, &completion)
                .expect("read");
            let names = names(self.reading.data());
            self.arm();
            names
        }
    }

    /// Each frame in `data`, named.
    fn names(mut data: &[u8]) -> Vec<String> {
        let mut names = Vec::new();
        while let Some(command) =
            crate::protocol::read_message::<_, SupervisorToOutpost>(&mut data).expect("a frame")
        {
            names.push(match command {
                SupervisorToOutpost::Ping { seq, .. } => format!("ping {seq}"),
                SupervisorToOutpost::Query { request_id, .. } => {
                    format!("query {request_id}")
                }
                SupervisorToOutpost::NodesHeld { acknowledged, .. } => {
                    format!("held {acknowledged}")
                }
                other => format!("{other:?}"),
            });
        }
        names
    }

    /// A list left while requests wait goes after the last of them, never
    /// ahead of one the reducer issued before it; a fact queued before it
    /// that a later one replaces no longer counts as ahead of it.
    #[test]
    fn held_nodes_go_after_every_command_queued_before_they_were_left() {
        let request = |id| {
            Outgoing::Other(
                SupervisorToOutpost::Query {
                    trace_id: TraceId::mint(),
                    request_id: id,
                    query: crate::protocol::Query::DumpTree { locations: false },
                    timing: EventTiming::default(),
                },
                Received::now(),
            )
        };
        let writes = |queue: &mut Queue| {
            let mut writes = Vec::new();
            while let Some(item) = queue.pop() {
                let mut buffer = Vec::new();
                Backlog::encode(queue, item, &mut buffer).expect("encoded");
                writes.push(names(&buffer));
            }
            writes
        };
        let mut queue = Queue::default();
        queue.push(request(1), WRITER_CAPACITY).expect("queued");
        queue.push(request(2), WRITER_CAPACITY).expect("queued");
        queue.leave(held_list(7));
        queue.push(request(3), WRITER_CAPACITY).expect("queued");
        assert_eq!(
            writes(&mut queue),
            [vec!["query 1"], vec!["query 2", "held 7"], vec!["query 3"]]
        );
        queue.push(fact(10), WRITER_CAPACITY).expect("queued");
        queue.push(request(4), WRITER_CAPACITY).expect("queued");
        queue.leave(held_list(8));
        queue.push(fact(10), WRITER_CAPACITY).expect("replaces");
        assert_eq!(
            writes(&mut queue),
            [vec!["query 4", "held 8"], vec!["ping 10"]]
        );
    }

    /// A list left in the slot is not written; the next request carries it
    /// in its own write, after it; and the slot is then empty, so the write
    /// after that carries nothing.
    #[test]
    fn held_nodes_go_after_the_next_request_in_its_write_and_never_in_one_of_their_own() {
        let (writer, mut child) = Child::new();
        writer.leave_held(held_list(7));
        assert!(child.nothing_written(), "the list is not written alone");
        writer.push(query()).expect("written");
        assert_eq!(child.one_write(), ["query 1", "held 7"]);
        writer.push(query()).expect("written");
        assert_eq!(child.one_write(), ["query 1"], "the list went once");
    }

    /// With no request, the relay's next ping carries the list, after it,
    /// in the same write.
    #[test]
    fn held_nodes_go_with_the_next_ping_when_no_request_comes() {
        let (writer, mut child) = Child::new();
        writer.leave_held(held_list(4));
        assert!(child.nothing_written(), "the list is not written alone");
        writer.push(ping()).expect("written");
        assert_eq!(child.one_write(), ["ping 1", "held 4"]);
    }

    /// A newer list replaces one not yet sent, and only the newer one
    /// goes.
    #[test]
    fn a_newer_held_list_replaces_an_unsent_one() {
        let (writer, mut child) = Child::new();
        writer.leave_held(held_list(4));
        writer.leave_held(held_list(9));
        assert!(child.nothing_written(), "neither list is written alone");
        writer.push(ping()).expect("written");
        assert_eq!(child.one_write(), ["ping 1", "held 9"]);
    }

    /// A selection of `runtime_id` named `name`, as routed.
    fn selection(runtime_id: i32, name: &str) -> Outgoing {
        Outgoing::delivered(held(DeliveredFact::UiaSelection {
            hwnd: 0,
            focus_window: 0,
            snapshot: UiaSnapshotFact {
                runtime_id: vec![runtime_id],
                role: verbatim_model::Role::ListItem,
                name: Some(name.to_owned()),
                value: None,
                states: verbatim_model::StateSet::new(),
                details: verbatim_model::NodeDetails::default(),
            },
        }))
    }

    fn held(fact: DeliveredFact) -> HeldFact {
        HeldFact {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            fact,
            win_event: None,
            received: Received::now(),
        }
    }

    /// What a routed fact in the queue is, for the order asserted.
    fn routed(queue: &mut Queue) -> Vec<String> {
        std::iter::from_fn(|| queue.pop())
            .map(|item| match item.command() {
                SupervisorToOutpost::DeliverFact {
                    fact: DeliveredFact::UiaFocusMarker { marker },
                    ..
                } => format!("marker {marker}"),
                SupervisorToOutpost::DeliverFact {
                    fact: DeliveredFact::UiaSelection { snapshot, .. },
                    ..
                } => format!(
                    "selection {:?} {}",
                    snapshot.runtime_id,
                    snapshot.name.as_deref().unwrap_or_default()
                ),
                other => format!("{other:?}"),
            })
            .collect()
    }

    /// A UIA focus's marker is keyed apart from every fact, so a fact for
    /// the same element never merges with it, a full queue never drops it
    /// as its oldest fact, and a newer fact replacing an older one moves to
    /// the back without moving the marker behind it.
    #[test]
    fn a_focus_marker_is_never_merged_dropped_or_moved_behind_later_facts() {
        let mut queue = Queue::default();
        let marker = Outgoing::delivered(held(DeliveredFact::UiaFocusMarker { marker: 7 }));
        queue.push(marker, 2).expect("queued");
        queue.push(selection(1, "first"), 2).expect("queued");
        queue
            .push(selection(2, "other"), 2)
            .expect("the oldest fact makes room");
        queue
            .push(selection(2, "newer"), 2)
            .expect("replaces the waiting one");
        assert_eq!(routed(&mut queue), ["marker 7", "selection [2] newer"]);
    }

    /// A foreground change and an MSAA focus are keyed by the thread that
    /// raised them, as NVDA's limiter keys them: the change or focus of one
    /// window from another thread waits beside the one waiting, and a newer
    /// one from the same thread replaces it and moves to the back.
    #[test]
    fn a_foreground_change_or_msaa_focus_replaces_only_the_one_its_thread_raised() {
        let raised = |fact, thread, time| {
            Outgoing::delivered(HeldFact {
                win_event: Some(crate::protocol::WinEventStamp { thread, time }),
                ..held(fact)
            })
        };
        let foreground = || DeliveredFact::Foreground { hwnd: 8 };
        let focus = || DeliveredFact::MsaaFocus {
            hwnd: 8,
            id_object: -4,
            id_child: 0,
        };
        let mut queue = Queue::default();
        for (fact, thread, time) in [
            (foreground(), 7, 100),
            (focus(), 7, 101),
            (foreground(), 9, 110),
            (focus(), 9, 111),
            (foreground(), 7, 120),
            (focus(), 7, 121),
        ] {
            queue.push(raised(fact, thread, time), 8).expect("queued");
        }
        let order: Vec<String> = std::iter::from_fn(|| queue.pop())
            .map(|item| match item.command() {
                SupervisorToOutpost::DeliverFact {
                    fact,
                    win_event: Some(stamp),
                    ..
                } => format!("{fact:?} thread {} at {}", stamp.thread, stamp.time),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            order,
            [
                "Foreground { hwnd: 8 } thread 9 at 110",
                "MsaaFocus { hwnd: 8, id_object: -4, id_child: 0 } thread 9 at 111",
                "Foreground { hwnd: 8 } thread 7 at 120",
                "MsaaFocus { hwnd: 8, id_object: -4, id_child: 0 } thread 7 at 121",
            ]
        );
    }

    /// An alert and a tooltip shown are keyed by the thread that raised
    /// them too, as NVDA's limiter keys those `WinEvent`s: one of an
    /// address from another thread waits beside the one waiting, and a
    /// newer one from the same thread replaces it and moves to the back. A
    /// menu opening is keyed by its address alone, as NVDA's limiter keeps
    /// one menu event whatever its thread: one from another thread
    /// replaces the one waiting and moves to the back.
    #[test]
    fn alerts_and_tooltips_are_replaced_per_thread_and_menu_openings_per_address() {
        let raised = |fact, thread, time| {
            Outgoing::delivered(HeldFact {
                win_event: Some(crate::protocol::WinEventStamp { thread, time }),
                ..held(fact)
            })
        };
        let menu = || DeliveredFact::MenuPopup {
            hwnd: 8,
            id_object: -4,
            id_child: 0,
        };
        let alert = || DeliveredFact::Alert {
            hwnd: 8,
            id_object: -4,
            id_child: 0,
        };
        let shown = || DeliveredFact::Show {
            hwnd: 8,
            id_object: -4,
            id_child: 0,
        };
        let mut queue = Queue::default();
        for (fact, thread, time) in [
            (menu(), 7, 100),
            (alert(), 7, 101),
            (shown(), 7, 102),
            (menu(), 9, 110),
            (alert(), 9, 111),
            (shown(), 9, 112),
            (alert(), 7, 121),
            (shown(), 7, 122),
        ] {
            queue.push(raised(fact, thread, time), 16).expect("queued");
        }
        let order: Vec<String> = std::iter::from_fn(|| queue.pop())
            .map(|item| match item.command() {
                SupervisorToOutpost::DeliverFact {
                    fact,
                    win_event: Some(stamp),
                    ..
                } => format!("{fact:?} thread {} at {}", stamp.thread, stamp.time),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            order,
            [
                "MenuPopup { hwnd: 8, id_object: -4, id_child: 0 } thread 9 at 110",
                "Alert { hwnd: 8, id_object: -4, id_child: 0 } thread 9 at 111",
                "Show { hwnd: 8, id_object: -4, id_child: 0 } thread 9 at 112",
                "Alert { hwnd: 8, id_object: -4, id_child: 0 } thread 7 at 121",
                "Show { hwnd: 8, id_object: -4, id_child: 0 } thread 7 at 122",
            ]
        );
    }

    #[test]
    fn a_hand_over_message_gets_through_a_full_queue_behind_the_facts_before_it() {
        let ended = || Outgoing::HandOver(SupervisorToOutpost::ForwardingEnded { through: 99 });
        let mut queue = Queue::default();
        queue.push(fact(1), 2).expect("queued");
        queue.push(fact(2), 2).expect("queued");
        queue.push(ended(), 2).expect("gets through the full queue");
        let order: Vec<String> = std::iter::from_fn(|| queue.pop())
            .map(|item| match item.command() {
                SupervisorToOutpost::Ping { seq, .. } => seq.to_string(),
                SupervisorToOutpost::ForwardingEnded { .. } => "ended".to_owned(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(order, ["1", "2", "ended"]);
    }

    /// The idle barrier's query gets through a full queue behind
    /// everything queued before it, and a fact queued after it replaces no
    /// fact queued before it, which would move that fact's object behind
    /// the query.
    #[test]
    fn the_idle_query_gets_through_behind_the_facts_before_it_and_nothing_merges_across_it() {
        let idle = || Outgoing::IdleQuery(SupervisorToOutpost::IdleQuery { round: 3 });
        let mut queue = Queue::default();
        queue.push(fact(1), 2).expect("queued");
        queue.push(fact(2), 2).expect("queued");
        queue.push(idle(), 2).expect("gets through the full queue");
        queue
            .push(fact(1), 8)
            .expect("queued behind it, the waiting fact 1 kept");
        let order: Vec<String> = std::iter::from_fn(|| queue.pop())
            .map(|item| match item.command() {
                SupervisorToOutpost::Ping { seq, .. } => seq.to_string(),
                SupervisorToOutpost::IdleQuery { round } => format!("idle {round}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(order, ["1", "2", "idle 3", "1"]);
    }

    #[test]
    fn a_command_to_the_listener_is_stamped_as_its_write_begins() {
        let commands = [
            ping(),
            Outgoing::HandOver(SupervisorToOutpost::HookRemoved {
                pid: verbatim_model::Pid(5),
                sent_at_us: 0,
            }),
        ];
        let hops: Vec<Option<u64>> = commands
            .into_iter()
            .map(|command| {
                let written = command.relayed_with(|| 5_000);
                crate::protocol::hop_us(written.sent_at_us(), 5_400)
            })
            .collect();
        assert_eq!(hops, [Some(400), Some(400)]);
    }
}
