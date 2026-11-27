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
//! - Sending never waits: nothing that sends a message, the workers and the
//!   reducer's thread above all, can be held up by a pipe Core is slow to
//!   read.
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
//! - A message is encoded before it is numbered, so one larger than a frame
//!   may take has no side effect: its nodes are withdrawn, and an answer is
//!   replaced by its request's answer ended, so every request keeps its one
//!   answer; any other such message is dropped, logged.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::convert::Infallible;
use std::io;

use verbatim_model::protocol::{OutpostToCore, Urgent};
use verbatim_model::request::{Answer, Outcome};
use verbatim_process::transport::{Backlog, SendError, Sender};

use super::store::Recorded;
use verbatim_process::clock::now_us;
use verbatim_process::message::put_message;
use verbatim_reader_rules::Numbered;

/// Something a message may carry that the outpost keeps until Core has
/// seen it and does not hold it: a node, a text anchor, or a caret tell's
/// element, by number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Carried {
    /// A node.
    Node(u64),
    /// A text anchor.
    Anchor(u64),
    /// The element a caret tell names by its token, for Core's caret read
    /// to adopt.
    Told(u64),
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

/// The waiting messages, and the numbering of those that carry node ids:
/// the backlog of the outpost's [`Sender`]. Every message sent passes
/// through it, written at once or after waiting, so the numbers are given
/// in the order the messages are written. Pure, so the order and the
/// numbering are tested on their own.
pub(crate) struct Pending {
    urgent: VecDeque<Urgent>,
    ordinary: VecDeque<Waiting>,
    /// The numbering of the messages that carry node ids, as Core counts
    /// them, and where each node, anchor and caret tell's element was
    /// carried.
    numbered: Numbered<Carried>,
}

impl Pending {
    fn new() -> Self {
        Self {
            urgent: VecDeque::new(),
            ordinary: VecDeque::new(),
            numbered: Numbered::default(),
        }
    }

    /// Queues `message`, which may report each of `touched`, at the back.
    fn push_message(&mut self, message: OutpostToCore, touched: Vec<Carried>) {
        for &carried in &touched {
            self.numbered.queued(carried);
        }
        self.ordinary
            .push_back(Waiting::Message(Box::new(message), touched));
    }

    /// The numbering of the messages written, and where each kept thing
    /// was carried.
    pub(super) fn numbered(&mut self) -> &mut Numbered<Carried> {
        &mut self.numbered
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

    /// Encodes `item` into `buffer`. An ordinary message is encoded before
    /// it is numbered, so one that cannot be (larger than a frame may take)
    /// has no side effect: the nodes it named are withdrawn, never recorded
    /// as seen, and an answer is replaced by its request's answer ended, so
    /// the request still has its one answer, at once ([`too_large`]).
    fn encode(&mut self, item: Outgoing, buffer: &mut Vec<u8>) -> io::Result<()> {
        let mut message: OutpostToCore = match item {
            Outgoing::Urgent(message) => message.into(),
            Outgoing::Message(message, touched) => {
                return self.encode_ordinary(message, &touched, buffer);
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

impl Pending {
    /// Encodes the ordinary `message`, which may name each of `touched`,
    /// and only once it is encoded numbers it; or, when it cannot be,
    /// encodes what stands for it ([`too_large`]).
    fn encode_ordinary(
        &mut self,
        message: OutpostToCore,
        touched: &[Carried],
        buffer: &mut Vec<u8>,
    ) -> io::Result<()> {
        let message = match put_message(buffer, &message) {
            Ok(()) => message,
            Err(error) => {
                for &carried in touched {
                    self.numbered.withdrawn(carried);
                }
                let Some(mut ended) = too_large(message, &error) else {
                    return Ok(());
                };
                if put_message(buffer, &OutpostToCore::Answer(ended.clone())).is_err() {
                    // Too many probes to fit even so: they are dropped, so
                    // the answer, now only fixed fields and a short detail,
                    // always fits. Core has no verdict from them and
                    // probes those windows again when it next reads them.
                    tracing::warn!(request = %ended.number, probes = ended.probes.len(), "the ended answer's probes are dropped to fit it in a frame");
                    ended.probes.clear();
                    put_message(buffer, &OutpostToCore::Answer(ended))?;
                }
                // An ended answer carries no node: it is not numbered.
                self.numbered.written(false, []);
                return Ok(());
            }
        };
        self.numbered
            .written(message.carries_nodes(), touched.iter().copied());
        if let OutpostToCore::Answer(answer) = &message {
            tracing::debug!(request = %answer.number, "answer written to Core's pipe");
        }
        Ok(())
    }
}

/// What stands for `message`, which could not be encoded for `error` (it is
/// larger than a frame may take, as a terminal's whole history can be): an
/// answer is replaced by its request's answer ended, the outcome a reader
/// gives a request it ends, with why for the log, so the request is
/// answered at once rather than waiting to its deadline; any other message
/// is dropped, logged, since nothing waits for it. NVDA has no frame limit;
/// a review read that fails there raises and is logged, saying nothing
/// (`nvda/source/scriptHandler.py` 294-295), as Core's review command
/// whose history read ends says nothing.
fn too_large(message: OutpostToCore, error: &io::Error) -> Option<Answer> {
    let OutpostToCore::Answer(answer) = message else {
        tracing::error!(%error, "a message to Core cannot be sent and is dropped");
        return None;
    };
    tracing::error!(request = %answer.number, %error, "an answer to Core cannot be sent; its request is answered ended");
    tracing::debug!(request = %answer.number, "the oversized answer is replaced by its request's answer ended");
    // The probes go with it, as their verdicts are Core's to record; the
    // reader's detail, which can grow with the read, does not.
    Some(Answer {
        number: answer.number,
        outcome: Outcome::Ended,
        probes: answer.probes,
        detail: format!("the answer cannot be sent: {error}"),
        began_us: answer.began_us,
        ended_us: answer.ended_us,
    })
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
            outcome: Outcome::Result(Reply::Steps(vec![
                verbatim_model::batch::StepOutcome::Done(verbatim_model::batch::Done::Follow),
            ])),
            probes: Vec::new(),
            detail: String::new(),
            began_us: 0,
            ended_us: 0,
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
            began_us: 0,
            ended_us: 0,
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

    /// An answer too large for a frame is judged before it is numbered:
    /// its request is answered ended instead, at once, with why for the
    /// log, and the nodes it named are withdrawn, never recorded as seen.
    /// A message behind it is written and numbered as usual.
    #[test]
    fn an_answer_too_large_for_a_frame_answers_its_request_ended() {
        let too_large = Answer {
            number: RequestNumber(7),
            outcome: Outcome::Result(Reply::Steps(Vec::new())),
            probes: Vec::new(),
            detail: "z".repeat(verbatim_process::transport::MAX_FRAME_BYTES),
            began_us: 10,
            ended_us: 20,
        };
        // The transport's own refusal of the answer, which the detail gives.
        let refused = put_message(&mut Vec::new(), &OutpostToCore::Answer(too_large.clone()))
            .expect_err("larger than a frame may take");
        let mut pending = pending();
        push(&mut pending, OutpostToCore::Answer(too_large), &[1]);
        push(&mut pending, answer(8), &[2]);
        let written = write(&mut pending);
        let [OutpostToCore::Answer(ended), next] = written.as_slice() else {
            panic!("two messages are written: {written:?}");
        };
        assert_eq!(
            (ended.number, &ended.outcome, ended.began_us, ended.ended_us),
            (RequestNumber(7), &Outcome::Ended, 10, 20)
        );
        assert_eq!(
            ended.detail,
            format!("the answer cannot be sent: {refused}")
        );
        assert_eq!(next, &answer(8));
        let numbered = pending.numbered();
        assert_eq!(numbered.position(), 1, "only the answer behind it counts");
        let report = |number| {
            numbered
                .report(Carried::Node(number))
                .map(|report| (report.position, report.waiting))
        };
        assert_eq!(report(1), Some((0, 0)), "withdrawn, never seen");
        assert_eq!(report(2), Some((1, 0)));
    }

    /// An answer whose probes alone pass a frame's limit is still answered
    /// ended: the probes are dropped so the ended answer fits, and the
    /// request keeps its one answer.
    #[test]
    fn an_ended_answer_too_large_for_its_probes_is_sent_without_them() {
        let probe = verbatim_model::request::Probe {
            window: verbatim_model::WindowHandle(0x0001_0203),
            thread: 4_096,
            provider: true,
            began_us: 1_000_000_000,
        };
        let probes = vec![probe; verbatim_process::transport::MAX_FRAME_BYTES / 40];
        let mut pending = pending();
        push(
            &mut pending,
            OutpostToCore::Answer(Answer {
                number: RequestNumber(9),
                outcome: Outcome::Result(Reply::Steps(Vec::new())),
                probes,
                detail: String::new(),
                began_us: 0,
                ended_us: 0,
            }),
            &[],
        );
        let written = write(&mut pending);
        let [OutpostToCore::Answer(ended)] = written.as_slice() else {
            panic!("one message is written");
        };
        assert_eq!(
            (ended.number, &ended.outcome, ended.probes.len()),
            (RequestNumber(9), &Outcome::Ended, 0)
        );
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
        let numbered = pending.numbered();
        assert_eq!(numbered.position(), 3, "Core counts three messages");
        let reports: Vec<Option<(u64, u32)>> = [1, 2, 3, 4, 5]
            .map(|number| {
                numbered
                    .report(Carried::Node(number))
                    .map(|report| (report.position, report.waiting))
            })
            .to_vec();
        assert_eq!(
            reports,
            vec![Some((3, 0)), Some((2, 0)), Some((3, 0)), Some((3, 0)), None]
        );
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
        outbound.urgent(Urgent::Pong { parked_count: 0 });
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
        outbound.urgent(Urgent::Pong { parked_count: 0 });
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
