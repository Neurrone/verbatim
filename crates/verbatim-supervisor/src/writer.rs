//! Each child's writer: a [`Sender`] on its command pipe, so nothing that
//! makes a decision ever waits on a pipe (`docs/design/communication.md`,
//! section 4.3). The thread
//! that queues a command writes it itself when the pipe is free and
//! nothing waits; while a write is in flight, commands wait in a bounded
//! queue, written in order by the relay once that write's completion
//! reaches its port ([`super::relay`]). A writer is of one child's
//! commands ([`ToChild`]): an outpost's, [`CoreToOutpost`], or the
//! listener's, [`CoreToListener`].
//!
//! When the queue is full, each kind of command has its own policy:
//!
//! - a ping always gets through, past the bound, since a missed ping would
//!   get a healthy outpost killed, and so does the shutdown message, the
//!   last command a child is sent;
//! - a command that must not be lost always gets through, past the bound,
//!   and keeps its place behind everything queued before it: a spare's
//!   `Assign`, and every command to the listener, an application's exit
//!   and Core's requests and cancels among them;
//! - the idle barrier's query (`docs/design/communication.md`, section 6.7)
//!   always gets through, past the bound, behind everything queued before
//!   it;
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

use verbatim_model::protocol::{CoreToListener, CoreToOutpost};
use verbatim_process::overlapped::{Completion, Port};
use verbatim_process::transport::{Backlog, SendError, Sender};

use verbatim_process::clock::now_us;
use verbatim_process::held_slot::HeldSlot;
use verbatim_process::message::{Message, put_message};

/// How many commands may wait for one child before the overload policy
/// applies.
pub(super) const WRITER_CAPACITY: usize = 64;

/// The commands of one kind of child, as its writer's overload policy
/// needs them.
pub(crate) trait ToChild: Message + Clone + std::fmt::Debug + Send + 'static {
    /// A ping.
    fn ping() -> Self;

    /// The shutdown message.
    fn shutdown() -> Self;

    /// Stamps a ping, or a command to the listener, with `now`, read just
    /// before its write begins.
    fn stamp_sent(&mut self, now: impl FnOnce() -> u64);
}

impl ToChild for CoreToOutpost {
    fn ping() -> Self {
        CoreToOutpost::Ping { sent_at_us: 0 }
    }

    fn shutdown() -> Self {
        CoreToOutpost::Shutdown
    }

    fn stamp_sent(&mut self, now: impl FnOnce() -> u64) {
        CoreToOutpost::stamp_sent(self, now);
    }
}

impl ToChild for CoreToListener {
    fn ping() -> Self {
        CoreToListener::Ping { sent_at_us: 0 }
    }

    fn shutdown() -> Self {
        CoreToListener::Shutdown
    }

    fn stamp_sent(&mut self, now: impl FnOnce() -> u64) {
        CoreToListener::stamp_sent(self, now);
    }
}

/// One command waiting to be written, with what the overload policy needs
/// to know about it.
#[derive(Debug)]
pub(super) enum Outgoing<M: ToChild> {
    /// The shutdown message.
    Shutdown,
    /// A command the bound never refuses: a liveness ping, the idle
    /// barrier's query, a spare's `Assign`, or any command to the
    /// listener.
    Unbounded(M),
    /// Anything else: a request, its cancel, or what Core holds.
    Other(M),
}

impl<M: ToChild> Outgoing<M> {
    /// The command to write now, a ping with the moment its write begins
    /// ([`ToChild::stamp_sent`]).
    pub(super) fn relayed(self) -> M {
        self.relayed_with(now_us)
    }

    /// [`relayed`](Self::relayed), with `now` the clock a command to the
    /// listener is stamped from.
    fn relayed_with(self, now: impl FnOnce() -> u64) -> M {
        match self {
            Outgoing::Shutdown => M::shutdown(),
            Outgoing::Other(command) => command,
            Outgoing::Unbounded(mut command) => {
                command.stamp_sent(now);
                command
            }
        }
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
#[derive(Debug)]
pub(super) struct Queue<M: ToChild> {
    items: VecDeque<Outgoing<M>>,
    /// The latest held list not yet sent, an outpost's held nodes or the
    /// listener's held view ([`HeldSlot`]).
    held: HeldSlot<M>,
}

impl<M: ToChild> Default for Queue<M> {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
            held: HeldSlot::default(),
        }
    }
}

impl<M: ToChild> Queue<M> {
    /// Adds `item` under the overload policy for `capacity`.
    pub(super) fn push(&mut self, item: Outgoing<M>, capacity: usize) -> Result<(), QueueError> {
        match item {
            Outgoing::Shutdown | Outgoing::Unbounded(_) => {}
            Outgoing::Other(..) => {
                if self.items.len() >= capacity {
                    return Err(QueueError::Full);
                }
            }
        }
        self.items.push_back(item);
        Ok(())
    }

    fn pop(&mut self) -> Option<Outgoing<M>> {
        let item = self.items.pop_front()?;
        self.held.popped();
        Some(item)
    }

    /// Leaves `held` in the slot, replacing a list not yet sent, behind
    /// every command waiting now.
    fn leave(&mut self, held: M) {
        self.held.leave(held, self.items.len());
    }
}

impl<M: ToChild> Backlog for Queue<M> {
    type Item = Outgoing<M>;
    type Refused = QueueError;

    fn push(&mut self, item: Outgoing<M>) -> Result<(), QueueError> {
        Queue::push(self, item, WRITER_CAPACITY)
    }

    fn pop(&mut self) -> Option<Outgoing<M>> {
        Queue::pop(self)
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Appends `item`'s frame, then the held nodes' list in the slot, if
    /// any and nothing queued before it was left still waits, which the
    /// item's write then carries; the shutdown message carries none, since
    /// the outpost is ending.
    fn encode(&mut self, item: Outgoing<M>, buffer: &mut Vec<u8>) -> io::Result<()> {
        let carried = self.held.carried(matches!(item, Outgoing::Shutdown));
        put_message(buffer, &item.relayed())?;
        if let Some(held) = carried {
            let before = buffer.len();
            if let Err(error) = put_message(buffer, &held) {
                buffer.truncate(before);
                tracing::warn!(target: verbatim_model::DEFECT_TARGET, %error, "the held nodes could not be encoded; they are dropped");
            }
        }
        Ok(())
    }
}

/// The shared end of one child's writer: the relay and the reducer thread
/// send through it.
pub(crate) struct WriterHandle<M: ToChild> {
    sender: Sender<Queue<M>>,
}

impl<M: ToChild> Clone for WriterHandle<M> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<M: ToChild> WriterHandle<M> {
    /// The writer for a child's command pipe, an overlapped end, whose
    /// writes that pend complete on `port`, the relay's, under `key`.
    pub(super) fn on_port(to_child: OwnedHandle, port: Arc<Port>, key: usize) -> io::Result<Self> {
        Ok(Self {
            sender: Sender::on_port(to_child, Queue::default(), port, key)?,
        })
    }

    /// Writes `item` now if the pipe is free, or queues it, without
    /// waiting.
    pub(super) fn push(&self, item: Outgoing<M>) -> Result<(), QueueError> {
        self.sender.send(item).map_err(|error| match error {
            SendError::Refused(refused) => refused,
            SendError::Closed => QueueError::Closed,
        })
    }

    /// Leaves `held`, a held nodes' list, in the slot, replacing a list not
    /// yet sent: it writes nothing, and the next command written once every
    /// command waiting now has been written carries it.
    pub(super) fn leave_held(&self, held: M) {
        self.sender.with_backlog(|queue| queue.leave(held));
    }

    /// Writes `held`, a held nodes' list, behind every command waiting now,
    /// in place of a list left in the slot, which it supersedes. When the
    /// queue is full it is left in the slot instead, for the next command
    /// written to carry.
    pub(super) fn send_held(&self, held: M) {
        self.sender.with_backlog(|queue| queue.held.superseded());
        if let Err(QueueError::Full) = self.push(Outgoing::Other(held.clone())) {
            self.leave_held(held);
        }
    }

    /// Closes the writer: nothing more is accepted, and the pipe is closed
    /// once everything queued is written.
    pub(super) fn close(&self) {
        self.sender.close();
    }
}

/// A writer as the relay finishes its writes that pended, whichever kind of
/// child it writes to.
pub(super) trait Finishing: Send + Sync {
    /// Takes `completion`, of a write of this writer's that pended, from
    /// the relay: writes what waited behind it.
    fn complete(&self, completion: &Completion);

    /// Whether the writer has closed and written everything, or failed:
    /// no completion of its will come again.
    fn is_finished(&self) -> bool;
}

impl<M: ToChild> Finishing for WriterHandle<M>
where
    Sender<Queue<M>>: Send + Sync,
{
    fn complete(&self, completion: &Completion) {
        self.sender.complete(completion);
    }

    fn is_finished(&self) -> bool {
        self.sender.is_finished()
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    impl Outgoing<CoreToOutpost> {
        fn command(&self) -> CoreToOutpost {
            match self {
                Outgoing::Shutdown => CoreToOutpost::Shutdown,
                Outgoing::Unbounded(command) | Outgoing::Other(command) => command.clone(),
            }
        }
    }

    fn ping() -> Outgoing<CoreToOutpost> {
        Outgoing::Unbounded(CoreToOutpost::Ping { sent_at_us: 0 })
    }

    /// A request numbered `id`.
    fn request(id: u64) -> Outgoing<CoreToOutpost> {
        Outgoing::Other(CoreToOutpost::Cancel(verbatim_model::request::Cancel {
            number: verbatim_model::request::RequestNumber(id),
            reason: verbatim_model::request::CancelReason::Withdrawn,
        }))
    }

    fn query() -> Outgoing<CoreToOutpost> {
        request(1)
    }

    /// Each command waiting, named, in the order it is written.
    fn order(queue: &mut Queue<CoreToOutpost>) -> Vec<String> {
        std::iter::from_fn(|| queue.pop())
            .map(|item| match item.command() {
                CoreToOutpost::Cancel(cancel) => format!("query {}", cancel.number.0),
                CoreToOutpost::Assign { target_pid } => format!("assign {target_pid}"),
                CoreToOutpost::IdleQuery { round } => format!("idle {round}"),
                other => format!("{other:?}"),
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

    /// A held nodes' list acknowledging `acknowledged`.
    fn held_list(acknowledged: u64) -> CoreToOutpost {
        CoreToOutpost::NodesHeld {
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
        fn new() -> (WriterHandle<CoreToOutpost>, Self) {
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
            verbatim_process::message::read_message::<_, CoreToOutpost>(&mut data).expect("a frame")
        {
            names.push(match command {
                CoreToOutpost::Ping { .. } => "ping".to_owned(),
                CoreToOutpost::Cancel(cancel) => format!("query {}", cancel.number.0),
                CoreToOutpost::NodesHeld { acknowledged, .. } => {
                    format!("held {acknowledged}")
                }
                other => format!("{other:?}"),
            });
        }
        names
    }

    /// A list left while requests wait goes after the last of them, never
    /// ahead of one the reducer issued before it.
    #[test]
    fn held_nodes_go_after_every_command_queued_before_they_were_left() {
        let writes = |queue: &mut Queue<CoreToOutpost>| {
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
        assert_eq!(child.one_write(), ["ping", "held 4"]);
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
        assert_eq!(child.one_write(), ["ping", "held 9"]);
    }

    /// A spare's `Assign` gets through a full queue, behind the requests
    /// before it.
    #[test]
    fn a_hand_over_message_gets_through_a_full_queue_behind_the_requests_before_it() {
        let assign = || {
            Outgoing::Unbounded(CoreToOutpost::Assign {
                target_pid: verbatim_model::Pid(99),
            })
        };
        let mut queue = Queue::default();
        queue.push(request(1), 2).expect("queued");
        queue.push(request(2), 2).expect("queued");
        queue
            .push(assign(), 2)
            .expect("gets through the full queue");
        assert_eq!(order(&mut queue), ["query 1", "query 2", "assign 99"]);
    }

    #[test]
    fn a_command_to_the_listener_is_stamped_as_its_write_begins() {
        let written = Outgoing::Unbounded(CoreToListener::ping()).relayed_with(|| 5_000);
        assert_eq!(
            verbatim_model::protocol::hop_us(written.sent_at_us(), 5_400),
            Some(400)
        );
        let ping = ping().relayed_with(|| 5_000);
        assert_eq!(
            verbatim_model::protocol::hop_us(ping.sent_at_us(), 5_400),
            Some(400)
        );
    }
}
