//! The outpost's messages to Core: written to Core's pipe by the thread
//! that sends each one, through the transport's [`Sender`], and held in one
//! queue, its backlog, only while a write is in flight, which the outpost's
//! I/O thread then finishes (`books/dev/src/reference/crates/verbatim-outpost.md`, "Messages
//! to Core"; `docs/design/communication.md`, section 4.3).
//!
//! - Pongs, `Ready`, `TargetExited` and `SpareReady` go ahead of everything else that
//!   waits, behind only the write in flight, so heartbeat delivery never
//!   waits behind event traffic and a busy outpost is never mistaken for a
//!   dead one.
//! - Sending never waits: nothing that sends a message, the worker and the
//!   watchdog above all, can be held up by a pipe Core is slow to read.
//! - Ordinary messages, tells, answers to Core's requests, the idle
//!   barrier's answer and faults, are never merged and keep their order:
//!   each answer answers its own request, and each tell is an entry of
//!   Core's event queue, whose limiter is Core's. What waits is bounded by
//!   Core's requests and by the intake's own limits.
//! - The messages that carry node ids are numbered as Core counts them, as
//!   each is written, and the nodes and text anchors each names, which the
//!   object store listed in its [`Recorded`], are recorded at its number
//!   then; one a message still waiting carries has no number yet. So the
//!   store releases only nodes and anchors Core has seen
//!   ([`super::store::Store::release`]).

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::io;

use verbatim_model::protocol::{OutpostToCore, Urgent};
use verbatim_process::transport::{Backlog, SendError, Sender};

use super::store::Recorded;
use verbatim_process::clock::now_us;
use verbatim_process::message::put_message;

/// Something a message may carry that the outpost keeps until Core has
/// seen it and does not hold it: a node, or a text anchor, by number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Carried {
    /// A node.
    Node(u64),
    /// A text anchor.
    Anchor(u64),
}

/// What is sent to Core: one item of the outbound queue.
pub(crate) enum Outgoing {
    /// A message that goes ahead of everything waiting: a pong, `Ready`,
    /// `TargetExited` or `SpareReady`, none of which carries a node.
    Urgent(Urgent),
    /// An ordinary message, with the nodes and anchors it names.
    Message(OutpostToCore, Vec<Carried>),
    /// Answered once every message sent before it is written, every write
    /// before it having completed.
    Flushed(std::sync::mpsc::Sender<()>),
}

/// One entry in the ordinary queue.
enum Waiting {
    /// A message to write, with the nodes it may report.
    Message(Box<OutpostToCore>, Vec<Carried>),
    /// [`Outgoing::Flushed`].
    Flushed(std::sync::mpsc::Sender<()>),
}

/// When a node was reported: the number of the last message written that
/// may have reported it, and how many messages that may report it still
/// wait to be written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Report {
    position: u64,
    waiting: u32,
}

/// The waiting messages, and the numbering of those that carry node ids:
/// the backlog of the outpost's [`Sender`]. Every message sent passes
/// through it, written at once or after waiting, so the numbers are given
/// in the order the messages are written. Pure, so the order and the
/// numbering are tested on their own.
pub(crate) struct Pending {
    urgent: VecDeque<Urgent>,
    ordinary: VecDeque<Waiting>,
    /// How many messages that carry node ids have been written: the
    /// position of the last one, as Core counts them.
    position: u64,
    /// When each node and anchor was reported.
    reported: BTreeMap<Carried, Report>,
}

impl Pending {
    fn new() -> Self {
        Self {
            urgent: VecDeque::new(),
            ordinary: VecDeque::new(),
            position: 0,
            reported: BTreeMap::new(),
        }
    }

    /// Queues `message`, which may report each of `touched`, at the back.
    fn push_message(&mut self, message: OutpostToCore, touched: Vec<Carried>) {
        for &number in &touched {
            self.reported.entry(number).or_default().waiting += 1;
        }
        self.ordinary
            .push_back(Waiting::Message(Box::new(message), touched));
    }

    /// Numbers `message`, being written now, if it carries node ids, and
    /// records each of `touched` as reported at its number.
    fn number(&mut self, message: &OutpostToCore, touched: &[Carried]) {
        if message.carries_nodes() {
            self.position += 1;
        }
        for number in touched {
            if let Some(report) = self.reported.get_mut(number) {
                report.waiting = report.waiting.saturating_sub(1);
                report.position = self.position;
            }
        }
    }

    /// When `carried` was carried: the position of the last message
    /// written that carried it, and how many messages that carry it still
    /// wait; `None` when no message carried it.
    pub(super) fn report(&self, carried: Carried) -> Option<(u64, u32)> {
        self.reported
            .get(&carried)
            .map(|report| (report.position, report.waiting))
    }

    /// Forgets what was recorded of each node and anchor `gone` answers,
    /// which the store has released.
    pub(super) fn forget(&mut self, gone: impl Fn(Carried) -> bool) {
        self.reported.retain(|&carried, _| !gone(carried));
    }

    /// How many messages that carry node ids have been written: the
    /// position of the last one, as Core counts them.
    pub(super) fn position(&self) -> u64 {
        self.position
    }
}

impl Backlog for Pending {
    type Item = Outgoing;
    type Refused = Infallible;

    fn push(&mut self, item: Outgoing) -> Result<(), Infallible> {
        match item {
            Outgoing::Urgent(message) => self.urgent.push_back(message),
            Outgoing::Message(message, touched) => self.push_message(message, touched),
            Outgoing::Flushed(done) => self.ordinary.push_back(Waiting::Flushed(done)),
        }
        Ok(())
    }

    /// The next item: an urgent message, or the ordinary queue's first.
    fn pop(&mut self) -> Option<Outgoing> {
        if let Some(message) = self.urgent.pop_front() {
            return Some(Outgoing::Urgent(message));
        }
        Some(match self.ordinary.pop_front()? {
            Waiting::Message(message, touched) => Outgoing::Message(*message, touched),
            Waiting::Flushed(done) => Outgoing::Flushed(done),
        })
    }

    fn is_empty(&self) -> bool {
        self.urgent.is_empty() && self.ordinary.is_empty()
    }

    fn encode(&mut self, item: Outgoing, buffer: &mut Vec<u8>) -> io::Result<()> {
        let mut message = match item {
            Outgoing::Urgent(message) => message.into(),
            Outgoing::Message(message, touched) => {
                self.number(&message, &touched);
                message
            }
            Outgoing::Flushed(done) => {
                // Every write before it has completed.
                let _ = done.send(());
                return Ok(());
            }
        };
        // The write's stamp, for the latency ledger's split of the outbound
        // queue's time from the pipe's, taken as the write begins, whichever
        // thread writes.
        message.written(now_us());
        put_message(buffer, &message)
    }
}

/// A send's outcome: once the outpost is shutting down, or Core's pipe has
/// gone, what is sent is dropped.
fn sent(sent: &Result<(), SendError<Infallible>>) {
    if sent.is_err() {
        tracing::debug!("a message to Core is dropped: the pipe is closed");
    }
}

/// The sending side: every thread of the outpost that sends Core a
/// message sends it through this.
#[derive(Clone)]
pub(crate) struct Outbound {
    sender: Sender<Pending>,
}

impl Outbound {
    /// The backlog a new outbound queue starts with.
    pub(crate) fn backlog() -> Pending {
        Pending::new()
    }

    /// Sends through `sender`, whose backlog came from
    /// [`Outbound::backlog`].
    pub(crate) fn new(sender: Sender<Pending>) -> Self {
        Self { sender }
    }

    /// The sender, for the thread that finishes its writes.
    pub(crate) fn sender(&self) -> &Sender<Pending> {
        &self.sender
    }

    /// Sends a message that must not wait behind others: a pong, `Ready`,
    /// `TargetExited` or `SpareReady`, none of which carries a node, so
    /// none skips the numbering of those that do.
    pub(crate) fn urgent(&self, message: Urgent) {
        sent(&self.sender.send(Outgoing::Urgent(message)));
    }

    /// Sends a test's message, which names no node, without waiting.
    #[cfg(test)]
    pub(crate) fn send(&self, message: OutpostToCore) {
        sent(&self.sender.send(Outgoing::Message(message, Vec::new())));
    }

    /// Sends an ordinary message without waiting, recording the nodes and
    /// anchors it names as carried by it when it is written.
    pub(crate) fn publish(&self, recorded: Recorded) {
        let (message, carried) = recorded.into_parts();
        sent(&self.sender.send(Outgoing::Message(message, carried)));
    }

    /// Runs `read` on the queue's record of what was carried, under the
    /// backlog's lock, so no message is queued or numbered meanwhile: the
    /// store's release, which takes its own lock inside
    /// ([`super::store::Store::release`]).
    pub(super) fn with_pending<R>(&self, read: impl FnOnce(&mut Pending) -> R) -> R {
        self.sender.with_backlog(read)
    }

    /// Takes nothing more, writes what waits, and then closes the pipe, for
    /// the outpost's shutdown; whatever is sent afterwards is dropped. The
    /// thread that finishes the sender's writes sees it through.
    pub(crate) fn close(&self) {
        self.sender.close();
    }

    /// Waits until every ordinary message sent before this call has been
    /// written to the pipe, its write completed, or the pipe has closed.
    /// Never called on the thread that finishes the sender's writes.
    pub(crate) fn flush(&self) {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        if self.sender.send(Outgoing::Flushed(done_tx)).is_ok() {
            // An error means the queue was dropped, the pipe having failed,
            // with nothing left to write.
            let _ = done_rx.recv();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::BufReader;
    use std::sync::mpsc;
    use std::time::Duration;

    use verbatim_model::request::{Answer, Outcome, Reply, RequestNumber};
    use verbatim_process::overlapped::{End, Flow, pipe};

    use super::*;
    use verbatim_process::message::read_message;

    /// The answer to Core's request `number`, its result carrying nodes.
    fn answer(number: u64) -> OutpostToCore {
        OutpostToCore::Answer(Answer {
            number: RequestNumber(number),
            outcome: Outcome::Result(Reply::Followed),
            probes: Vec::new(),
            detail: String::new(),
        })
    }

    /// The answer to Core's request `number`, ended, with `detail` for the
    /// log: it carries no node.
    fn ended(number: u64, detail: &str) -> OutpostToCore {
        OutpostToCore::Answer(Answer {
            number: RequestNumber(number),
            outcome: Outcome::Ended,
            probes: Vec::new(),
            detail: detail.to_owned(),
        })
    }

    /// An answer larger than a pipe's 4 KiB buffer, so the write that
    /// carries it pends until the test reads.
    fn large() -> OutpostToCore {
        ended(99, &"x".repeat(20_000))
    }

    fn pending() -> Pending {
        Pending::new()
    }

    /// Queues `message`, which may report the nodes `touched`.
    fn push(pending: &mut Pending, message: OutpostToCore, touched: &[u64]) {
        let touched = touched
            .iter()
            .map(|&number| Carried::Node(number))
            .collect();
        assert!(pending.push(Outgoing::Message(message, touched)).is_ok());
    }

    /// Writes what waits, as the sender does: each item taken and encoded
    /// in turn. Returns the messages, as Core reads them.
    fn write(pending: &mut Pending) -> Vec<OutpostToCore> {
        let mut written = Vec::new();
        while let Some(item) = pending.pop() {
            let mut buffer = Vec::new();
            pending.encode(item, &mut buffer).expect("encoded");
            let mut bytes = buffer.as_slice();
            while let Some(message) = read_message(&mut bytes).expect("a frame") {
                written.push(message);
            }
        }
        written
    }

    /// Two answers to one request number and an idle answer between them
    /// are each written, in the order sent: nothing merges.
    #[test]
    fn ordinary_messages_are_written_whole_in_the_order_sent() {
        let mut pending = pending();
        push(&mut pending, answer(1), &[]);
        push(&mut pending, ended(2, "gone"), &[]);
        push(&mut pending, OutpostToCore::IdleAnswer { round: 3 }, &[]);
        push(&mut pending, answer(1), &[]);
        assert_eq!(
            write(&mut pending),
            [
                answer(1),
                ended(2, "gone"),
                OutpostToCore::IdleAnswer { round: 3 },
                answer(1),
            ]
        );
    }

    /// A message the size of a terminal's whole document of a full history,
    /// 9,001 rows of 120 columns each holding text to its last column, goes
    /// to Core as one message.
    #[test]
    fn a_message_the_size_of_a_full_historys_read_is_written_whole() {
        let row = "y".repeat(120);
        let text = vec![row.as_str(); 9_001].join("\n");
        let mut pending = pending();
        push(&mut pending, ended(1, &text), &[]);
        assert_eq!(write(&mut pending), [ended(1, &text)]);
    }

    #[test]
    fn urgent_messages_go_first() {
        let mut pending = pending();
        push(&mut pending, answer(1), &[]);
        assert!(pending.push(Outgoing::Urgent(Urgent::TargetExited)).is_ok());
        let written = write(&mut pending);
        assert_eq!(written.first(), Some(&OutpostToCore::TargetExited));
    }

    #[test]
    fn positions_are_counted_in_the_order_messages_are_written() {
        let mut pending = pending();
        push(&mut pending, answer(1), &[1]);
        push(&mut pending, answer(2), &[2]);
        push(&mut pending, answer(3), &[1, 3]);
        // A message without node ids takes no position.
        push(&mut pending, ended(4, ""), &[4]);
        assert_eq!(write(&mut pending).len(), 4);
        assert_eq!(pending.position(), 3, "Core counts three messages");
        let reports: Vec<Option<(u64, u32)>> = [1, 2, 3, 4, 5]
            .map(|number| pending.report(Carried::Node(number)))
            .to_vec();
        assert_eq!(
            reports,
            vec![Some((3, 0)), Some((2, 0)), Some((3, 0)), Some((3, 0)), None]
        );
    }

    #[test]
    fn a_node_a_waiting_message_carries_is_counted_as_waiting_until_written() {
        let mut pending = pending();
        push(&mut pending, answer(1), &[1]);
        assert_eq!(write(&mut pending).len(), 1);
        push(&mut pending, answer(2), &[1]);
        assert_eq!(pending.report(Carried::Node(1)), Some((1, 1)));
        assert_eq!(write(&mut pending).len(), 1);
        assert_eq!(pending.report(Carried::Node(1)), Some((2, 0)));
        pending.forget(|carried| carried == Carried::Node(1));
        assert_eq!(pending.report(Carried::Node(1)), None);
    }

    /// An outbound queue writing to a pipe with a 4 KiB buffer, its writes
    /// that pend finished by a thread of the sender's own, and the pipe's
    /// other end, which the test reads.
    fn outbound() -> (Outbound, BufReader<File>) {
        let (ours, theirs) = pipe(Flow::Outbound, 4096, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(ours, Outbound::backlog(), "outbound-test", || {}).expect("a sender");
        (Outbound::new(sender), BufReader::new(File::from(theirs)))
    }

    fn read(reader: &mut BufReader<File>) -> OutpostToCore {
        read_message(reader)
            .expect("read")
            .expect("a message, not the end")
    }

    #[test]
    fn messages_sent_behind_a_write_in_flight_are_written_in_order_after_it() {
        let (outbound, mut reader) = outbound();
        outbound.send(large());
        outbound.send(answer(1));
        outbound.send(ended(2, ""));
        outbound.send(answer(1));
        outbound.urgent(Urgent::Pong {
            parked_count: 0,
            parked_on_hung: 0,
        });
        assert_eq!(read(&mut reader), large());
        assert!(
            matches!(read(&mut reader), OutpostToCore::Pong { .. }),
            "the pong goes ahead of what waited"
        );
        let rest: Vec<OutpostToCore> = (0..3).map(|_| read(&mut reader)).collect();
        assert_eq!(rest, [answer(1), ended(2, ""), answer(1)]);
        outbound.close();
        assert_eq!(
            read_message::<_, OutpostToCore>(&mut reader).expect("read"),
            None
        );
    }

    /// The idle barrier's answer, sent behind a write in flight, goes after
    /// every message waiting before it, a pong sent after it going ahead of
    /// both.
    #[test]
    fn the_idle_answer_waits_behind_every_message_before_it() {
        let (outbound, mut reader) = outbound();
        outbound.send(large());
        outbound.send(answer(1));
        outbound.send(OutpostToCore::IdleAnswer { round: 4 });
        outbound.send(answer(2));
        outbound.urgent(Urgent::Pong {
            parked_count: 0,
            parked_on_hung: 0,
        });
        assert_eq!(read(&mut reader), large());
        assert!(
            matches!(read(&mut reader), OutpostToCore::Pong { .. }),
            "the pong goes ahead of what waited"
        );
        let rest: Vec<OutpostToCore> = (0..3).map(|_| read(&mut reader)).collect();
        assert_eq!(
            rest,
            [answer(1), OutpostToCore::IdleAnswer { round: 4 }, answer(2),]
        );
        outbound.close();
        assert_eq!(
            read_message::<_, OutpostToCore>(&mut reader).expect("read"),
            None
        );
    }

    #[test]
    fn a_flush_is_answered_only_once_the_writes_before_it_have_completed() {
        let (outbound, mut reader) = outbound();
        outbound.send(large());
        // Sent as `flush` sends it, without waiting for the answer here.
        let (done_tx, done) = mpsc::channel();
        assert!(outbound.sender().send(Outgoing::Flushed(done_tx)).is_ok());
        // The evidence that the flush has been taken and waits: it is in the
        // backlog, behind the large message's write, which stays in flight
        // until the test reads it.
        assert!(
            outbound.with_pending(|pending| {
                pending.ordinary.len() == 1
                    && matches!(pending.ordinary.front(), Some(Waiting::Flushed(_)))
            }),
            "the flush waits in the backlog"
        );
        assert_eq!(
            done.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "the large message's write is still in flight"
        );
        assert_eq!(read(&mut reader), large());
        assert_eq!(done.recv_timeout(Duration::from_secs(10)), Ok(()));
    }
}
