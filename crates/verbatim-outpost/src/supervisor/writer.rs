//! Each child's writer: a [`Sender`] on its command pipe, so nothing that
//! makes a decision ever waits on a pipe (outpost redesign, "The
//! supervisor"; `docs/design/communication.md`, section 4.3). The thread
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
//! - a list of the nodes Core holds replaces any older list still waiting,
//!   and otherwise gets through past the bound, since only the newest list
//!   matters and an outpost that never hears one keeps every node;
//! - a routed fact replaces any older fact still waiting for the same object
//!   and kind; if there is none, the oldest waiting fact is dropped to make
//!   room, so the newest facts survive;
//! - a message of the MSAA hand-over between the listener and an outpost
//!   (`docs/design/focus-pipeline.md`, section 12.4) always gets through,
//!   past the bound, and keeps its place behind everything queued before
//!   it: the listener's "forwarding ended" must reach the outpost after
//!   every fact the listener sent before it, and is never lost; so does
//!   every part of a UIA focus's read (its marker, its read or drop, and
//!   the end of a listener's reads), since an outpost waits on a marker
//!   until its read or drop arrives;
//! - anything else, a query above all, fails at once, so its asker gets an
//!   outcome straight away instead of waiting behind a stuck outpost.

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
    /// The nodes Core holds.
    NodesHeld(SupervisorToOutpost),
    /// A routed fact, keyed by the object and kind it concerns (`None` for
    /// a notification, which nothing replaces), with when verbatim.exe read
    /// it from the listener.
    Fact(Option<FactKey>, SupervisorToOutpost, Received),
    /// A message of the MSAA hand-over: `ForwardingEnded` to an outpost,
    /// `HookInstalled` or `HookRemoved` to the listener; and `FocusQueued`
    /// to the listener, which its duplicate rule depends on.
    HandOver(SupervisorToOutpost),
    /// A part of a UIA focus's read routed to an outpost, which waits on
    /// its marker, so it gets through as a hand-over message does, with
    /// when verbatim.exe read it from the listener.
    FocusRead(SupervisorToOutpost, Received),
    /// Anything else: a query, an activation, a configuration command, with
    /// when it was queued.
    Other(SupervisorToOutpost, Received),
}

impl Outgoing {
    /// A routed fact for an outpost: a part of a UIA focus's read, which an
    /// outpost waits on in order, gets through as a hand-over message does
    /// and is never merged; any other fact is keyed by the object and kind
    /// it concerns.
    pub(super) fn delivered(held: super::policy::HeldFact) -> Self {
        let key = held.fact.key();
        let focus_read = held.fact.is_focus_read();
        let command = SupervisorToOutpost::DeliverFact {
            trace_id: held.trace_id,
            observed_at_ms: held.observed_at_ms,
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
            | Outgoing::NodesHeld(command)
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
            Outgoing::NodesHeld(command) => command,
        }
    }
}

/// When verbatim.exe took in something it relays to an outpost: a
/// listener's fact as its read completed, or a request as it was queued
/// (`docs/design/relay.md`, section 4.5). Read on two clocks: the monotonic
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

/// The waiting commands. Pure, so the overload policy is tested on its own.
#[derive(Debug, Default)]
pub(super) struct Queue {
    items: VecDeque<Outgoing>,
}

impl Queue {
    /// Adds `item` under the overload policy for `capacity`.
    pub(super) fn push(&mut self, item: Outgoing, capacity: usize) -> Result<(), QueueError> {
        match item {
            Outgoing::Ping(_)
            | Outgoing::Shutdown
            | Outgoing::HandOver(_)
            | Outgoing::FocusRead(..) => {}
            Outgoing::NodesHeld(_) => {
                self.items
                    .retain(|waiting| !matches!(waiting, Outgoing::NodesHeld(_)));
            }
            Outgoing::Fact(ref key, ..) => {
                let same = key.as_ref().and_then(|key| {
                    self.items.iter().position(
                        |waiting| matches!(waiting, Outgoing::Fact(Some(other), ..) if other == key),
                    )
                });
                if let Some(index) = same {
                    self.items.remove(index);
                } else if self.items.len() >= capacity
                    && let Some(oldest) = self
                        .items
                        .iter()
                        .position(|waiting| matches!(waiting, Outgoing::Fact(..)))
                {
                    self.items.remove(oldest);
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
        self.items.pop_front()
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

    fn encode(item: Outgoing, buffer: &mut Vec<u8>) -> io::Result<()> {
        put_message(buffer, &item.relayed())
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
                query: crate::protocol::Query::DumpTree,
                timing: EventTiming::default(),
            },
            Received::now(),
        )
    }

    fn fact(hwnd: isize) -> Outgoing {
        Outgoing::Fact(
            Some(FactKey::Foreground(hwnd)),
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

    #[test]
    fn a_held_list_replaces_a_waiting_one_and_gets_through_a_full_queue() {
        let held = |acknowledged| {
            Outgoing::NodesHeld(SupervisorToOutpost::NodesHeld {
                nodes: Vec::new(),
                anchors: Vec::new(),
                acknowledged,
            })
        };
        let mut queue = Queue::default();
        queue.push(held(1), 1).expect("room for one");
        queue.push(held(2), 1).expect("replaces the waiting list");
        queue.push(query(), 3).expect("room for a query");
        queue.push(query(), 3).expect("room for another");
        queue
            .push(held(3), 2)
            .expect("replaces it again and gets through the full queue");
        let acknowledged: Vec<u64> = std::iter::from_fn(|| queue.pop())
            .filter_map(|item| match item.command() {
                SupervisorToOutpost::NodesHeld { acknowledged, .. } => Some(*acknowledged),
                _ => None,
            })
            .collect();
        assert_eq!(acknowledged, vec![3]);
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
            observed_at_ms: 0,
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
