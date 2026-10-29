//! The outpost's messages to Core: written to Core's pipe by the thread
//! that sends each one, through the transport's [`Sender`], and held in one
//! queue, its backlog, only while a write is in flight, which the outpost's
//! I/O thread then finishes (`docs/crates/verbatim-outpost.md`, "Messages
//! to Core"; `docs/design/communication.md`, section 4.3).
//!
//! - Pongs, `Ready`, and `TargetExited` go ahead of everything else that
//!   waits, behind only the write in flight, so heartbeat delivery never
//!   waits behind event traffic and a busy outpost is never mistaken for a
//!   dead one.
//! - Sending never waits: nothing that sends a message, the worker and the
//!   watchdog above all, can be held up by a pipe Core is slow to read.
//! - While messages wait, they are merged by object and kind, as the intake
//!   and the supervisor merge what reaches the outpost and as NVDA's
//!   limiters do: an event replaces a waiting event of the same kind for
//!   the same node and goes to the back. A terminal's read is never
//!   merged: where each read starts is where the one before it said to
//!   start, so a read dropped for a newer one would leave the rows between
//!   them unread by Core. Answers to Core's requests, notifications, and
//!   faults are never merged, and keep their order. The
//!   idle barrier's answer is never merged either, and no event is merged
//!   across it, so it never overtakes what was sent before it.
//!   What waits is therefore bounded by the nodes and kinds there are, by
//!   Core's requests, and by the intake's own limits.
//! - The messages that carry node ids are numbered as Core counts them, as
//!   each is written, and the nodes each may have reported are recorded at
//!   its number then; a node a message still waiting may report has no
//!   number yet. So the worker releases only nodes Core has seen
//!   ([`Outbound::release`]).

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::io;
use std::mem::Discriminant;

use verbatim_model::{NodeId, NormalizedEvent, PropertyChange};
use verbatim_process::transport::{Backlog, SendError, Sender};

use crate::protocol::{OutpostToSupervisor, now_us, put_message};

/// What two waiting events must share to be merged: the node and the kind
/// of event (and, for a property change, which property; for a focus,
/// whether it is a foreground change).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    node: NodeId,
    kind: Discriminant<NormalizedEvent>,
    detail: Detail,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Detail {
    None,
    Property(Discriminant<PropertyChange>),
    Foreground(bool),
}

/// The key `message` is merged under, or `None` for one that is never
/// merged: anything but an event, and a notification, each of which says
/// something of its own.
fn key(message: &OutpostToSupervisor) -> Option<Key> {
    let OutpostToSupervisor::Event { event, .. } = message else {
        return None;
    };
    let kind = std::mem::discriminant(event);
    let (node, detail) = match event {
        NormalizedEvent::FocusChanged {
            node, foreground, ..
        } => (node.id, Detail::Foreground(*foreground)),
        NormalizedEvent::PropertyChanged {
            node_id, change, ..
        } => (*node_id, Detail::Property(std::mem::discriminant(change))),
        NormalizedEvent::SelectionChanged { node }
        | NormalizedEvent::ControlledSelection { node, .. }
        | NormalizedEvent::Alert { node } => (node.id, Detail::None),
        NormalizedEvent::ProgressChanged { bar } => (bar.id, Detail::None),
        NormalizedEvent::ValueChanged { node_id, .. }
        | NormalizedEvent::CaretMoved { node_id, .. }
        | NormalizedEvent::TextChanged { node_id }
        | NormalizedEvent::ActiveTextPositionChanged { node_id, .. } => (*node_id, Detail::None),
        // A notification, and any kind of event added later, is never
        // merged.
        _ => return None,
    };
    Some(Key { node, kind, detail })
}

/// What is sent to Core: one item of the outbound queue.
pub(crate) enum Outgoing {
    /// A message that goes ahead of everything waiting: a pong, `Ready`,
    /// or `TargetExited`.
    Urgent(OutpostToSupervisor),
    /// An ordinary message, with the numbers of the nodes issued or looked
    /// up since the last one, which it may report.
    Message(OutpostToSupervisor, Vec<u64>),
    /// Answered once every message sent before it is written, every write
    /// before it having completed. Nothing sent after it is merged into a
    /// message before it.
    Flushed(std::sync::mpsc::Sender<()>),
}

/// One entry in the ordinary queue.
enum Waiting {
    /// A message to write, with the key it is merged under and the nodes it
    /// may report.
    Message(Option<Key>, Box<OutpostToSupervisor>, Vec<u64>),
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
/// in the order the messages are written. Pure, so the merging and the
/// numbering are tested on their own.
pub(crate) struct Pending {
    urgent: VecDeque<OutpostToSupervisor>,
    ordinary: VecDeque<Waiting>,
    /// How many messages that carry node ids have been written: the
    /// position of the last one, as Core counts them.
    position: u64,
    /// When each node, by number, was reported.
    reported: HashMap<u64, Report>,
}

impl Pending {
    fn new() -> Self {
        Self {
            urgent: VecDeque::new(),
            ordinary: VecDeque::new(),
            position: 0,
            reported: HashMap::new(),
        }
    }

    /// Queues `message`, which may report each of `touched`, merged with a
    /// waiting message of the same key.
    fn push_message(&mut self, message: OutpostToSupervisor, mut touched: Vec<u64>) {
        for &number in &touched {
            self.reported.entry(number).or_default().waiting += 1;
        }
        let key = key(&message);
        let mut message = message;
        if let Some(key) = &key
            && let Some(index) = self.waiting_with(key, &message)
            && let Some(Waiting::Message(_, older, older_touched)) = self.ordinary.remove(index)
        {
            message = merged(*older, message);
            touched.extend(older_touched);
        }
        self.ordinary
            .push_back(Waiting::Message(key, Box::new(message), touched));
    }

    /// Where the waiting message with `key` is, if one is waiting since the
    /// last flush marker and the last answer to the idle barrier, and, for
    /// a caret report that carries selection changes, since the last reply:
    /// a message merged across an answer would move what it reports behind
    /// it, and a change moved behind a reply would reach Core after the
    /// reply to a key pressed before Core saw it, whose own selection
    /// changes, compared with the selection Core knew, already include it.
    /// Nor is a caret report that carries selection changes merged with
    /// `newer` when either says its changes in general terms
    /// ([`verbatim_model::CaretReply::content_changed`]): each such report's
    /// words are worked out from its own selection.
    fn waiting_with(&self, key: &Key, newer: &OutpostToSupervisor) -> Option<usize> {
        let mut past_reply = false;
        for (index, waiting) in self.ordinary.iter().enumerate().rev() {
            match waiting {
                Waiting::Message(Some(other), message, _) if other == key => {
                    let kept = carries_selection_changes(message)
                        && (past_reply || content_changed(message) || content_changed(newer));
                    return (!kept).then_some(index);
                }
                Waiting::Message(_, message, _) if message.is_idle_answer() => return None,
                Waiting::Message(_, message, _) => {
                    past_reply |= matches!(**message, OutpostToSupervisor::Reply { .. });
                }
                Waiting::Flushed(_) => return None,
            }
        }
        None
    }

    /// Numbers `message`, being written now, if it carries node ids, and
    /// records each of `touched` as reported at its number.
    fn number(&mut self, message: &OutpostToSupervisor, touched: &[u64]) {
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

    /// Takes the nodes to release for a held list acknowledging position
    /// `acknowledged`: those not held that were reported at or before it,
    /// and that no message still waiting may report. A node reported
    /// later, or never reported, is kept, since Core may not have seen it
    /// yet.
    fn take_releasable(&mut self, held: &HashSet<u64>, acknowledged: u64) -> HashSet<u64> {
        let released: HashSet<u64> = self
            .reported
            .iter()
            .filter(|&(number, report)| {
                report.waiting == 0 && report.position <= acknowledged && !held.contains(number)
            })
            .map(|(&number, _)| number)
            .collect();
        self.reported.retain(|number, _| !released.contains(number));
        released
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
            Waiting::Message(_, message, touched) => Outgoing::Message(*message, touched),
            Waiting::Flushed(done) => Outgoing::Flushed(done),
        })
    }

    fn is_empty(&self) -> bool {
        self.urgent.is_empty() && self.ordinary.is_empty()
    }

    fn encode(&mut self, item: Outgoing, buffer: &mut Vec<u8>) -> io::Result<()> {
        let mut message = match item {
            Outgoing::Urgent(message) => message,
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
        let written = put_message(buffer, &message);
        // A terminal's read, written, gives its buffers back for the next.
        if let Some(snapshot) = terminal_read(message) {
            crate::terminal::recycle(snapshot);
        }
        written
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
    /// or `TargetExited`.
    pub(crate) fn urgent(&self, message: OutpostToSupervisor) {
        sent(&self.sender.send(Outgoing::Urgent(message)));
    }

    /// Sends an ordinary message that reports no node, without waiting.
    pub(crate) fn send(&self, message: OutpostToSupervisor) {
        self.publish(message, Vec::new());
    }

    /// Sends an ordinary message without waiting, recording each of
    /// `touched`, the nodes issued or looked up since the last message, as
    /// reported by it.
    pub(crate) fn publish(&self, message: OutpostToSupervisor, touched: Vec<u64>) {
        sent(&self.sender.send(Outgoing::Message(message, touched)));
    }

    /// Takes the nodes to release for Core's held list `held`, which
    /// acknowledges position `acknowledged`, and runs `forget` on them
    /// before any message sent afterwards can record them again. `None`
    /// when there is nothing to release. Returns the position reached too,
    /// for the log.
    pub(crate) fn release<R>(
        &self,
        held: &HashSet<u64>,
        acknowledged: u64,
        forget: impl FnOnce(&HashSet<u64>) -> R,
    ) -> (u64, Option<(HashSet<u64>, R)>) {
        self.sender.with_backlog(|pending| {
            let released = pending.take_releasable(held, acknowledged);
            if released.is_empty() {
                return (pending.position, None);
            }
            let forgotten = forget(&released);
            (pending.position, Some((released, forgotten)))
        })
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

/// Whether `message` is a caret report that carries selection changes.
fn carries_selection_changes(message: &OutpostToSupervisor) -> bool {
    matches!(
        message,
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { selection_changes, .. },
            ..
        } if !selection_changes.is_empty()
    )
}

/// The terminal's read `message` carries, as an event or an answer.
fn terminal_read(message: OutpostToSupervisor) -> Option<verbatim_model::TerminalSnapshot> {
    match message {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::TerminalSnapshot { snapshot, .. },
            ..
        }
        | OutpostToSupervisor::Reply {
            outcome:
                crate::protocol::QueryOutcome::Done(crate::protocol::QueryResult::Text(
                    verbatim_model::TextReply::TerminalSnapshot(snapshot),
                )),
            ..
        } => Some(*snapshot),
        _ => None,
    }
}

/// Whether `message` is a caret report whose selection changes are said in
/// general terms.
fn content_changed(message: &OutpostToSupervisor) -> bool {
    matches!(
        message,
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved {
                content_changed: true,
                ..
            },
            ..
        }
    )
}

/// `newer` in place of `older`, the waiting message with the same key: a
/// caret report
/// carrying the waiting one's selection changes before its own, each
/// change being from the report before, and anything else replacing it.
/// Two caret reports merged are never both in general terms, nor is the
/// waiting one when it carries changes ([`Pending::waiting_with`]), so the
/// newer's general terms are kept.
fn merged(older: OutpostToSupervisor, newer: OutpostToSupervisor) -> OutpostToSupervisor {
    match (older, newer) {
        (
            OutpostToSupervisor::Event {
                event:
                    NormalizedEvent::CaretMoved {
                        selection_changes: mut waiting,
                        ..
                    },
                ..
            },
            OutpostToSupervisor::Event {
                trace_id,
                observed_at_us,
                backend,
                window,
                application,
                timing,
                event:
                    NormalizedEvent::CaretMoved {
                        node_id,
                        caret,
                        selection_changes,
                        content_changed,
                        new_selection,
                    },
            },
        ) => {
            waiting.extend(selection_changes);
            OutpostToSupervisor::Event {
                trace_id,
                observed_at_us,
                backend,
                window,
                application,
                timing,
                event: NormalizedEvent::CaretMoved {
                    node_id,
                    caret,
                    selection_changes: waiting,
                    content_changed,
                    new_selection,
                },
            }
        }
        (_, newer) => newer,
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::BufReader;
    use std::sync::mpsc;
    use std::time::Duration;

    use verbatim_model::{Backend, TerminalColourRun, TerminalSnapshot, TerminalText, TraceId};
    use verbatim_process::overlapped::{End, Flow, pipe};

    use super::*;
    use crate::protocol::{EventTiming, QueryOutcome, read_message};

    fn node(number: u64) -> NodeId {
        NodeId::new(number)
    }

    fn event(event: NormalizedEvent) -> OutpostToSupervisor {
        OutpostToSupervisor::Event {
            trace_id: TraceId::mint(),
            observed_at_us: 0,
            backend: Backend::Uia,
            window: None,
            application: None,
            timing: EventTiming::default(),
            event,
        }
    }

    fn text_changed(number: u64) -> OutpostToSupervisor {
        event(NormalizedEvent::TextChanged {
            node_id: node(number),
        })
    }

    fn value_changed(number: u64) -> OutpostToSupervisor {
        event(NormalizedEvent::ValueChanged {
            node_id: node(number),
            value: None,
        })
    }

    fn reply(request_id: u64) -> OutpostToSupervisor {
        OutpostToSupervisor::Reply {
            trace_id: TraceId::mint(),
            request_id,
            outcome: QueryOutcome::Abandoned,
            timing: EventTiming::default(),
        }
    }

    /// A read of a terminal whose text is `text`, with colours when
    /// `coloured`.
    fn terminal(text: &str, coloured: bool) -> OutpostToSupervisor {
        event(NormalizedEvent::TerminalSnapshot {
            node_id: node(9),
            snapshot: Box::new(TerminalSnapshot {
                text: TerminalText::unpadded(text),
                screen: TerminalText::unpadded(text),
                from_start: true,
                shrank: false,
                at_document_start: false,
                alternate: false,
                first_row: TerminalText::default(),
                caret_rows_to_end: 1,
                caret_line: TerminalText::default(),
                caret_column: 0,
                above: 0,
                started_ms: 0,
                ended_ms: 0,
                waited: false,
                read: 0,
                colours: coloured.then(|| {
                    vec![vec![TerminalColourRun {
                        column: 0,
                        length: 3,
                        foreground: 7,
                        background: 0,
                    }]]
                }),
                caret: None,
            }),
        })
    }

    /// The text of a terminal's read written.
    fn text_of(message: &OutpostToSupervisor) -> &str {
        match message {
            OutpostToSupervisor::Event {
                event: NormalizedEvent::TerminalSnapshot { snapshot, .. },
                ..
            } => &snapshot.text.text,
            other => panic!("not a terminal's read: {other:?}"),
        }
    }

    /// A terminal's read larger than a pipe's 4 KiB buffer, so the write
    /// that carries it pends until the test reads.
    fn large() -> OutpostToSupervisor {
        terminal(&"x".repeat(20_000), false)
    }

    fn pending() -> Pending {
        Pending::new()
    }

    /// Queues `message`, which may report `touched`.
    fn push(pending: &mut Pending, message: OutpostToSupervisor, touched: &[u64]) {
        assert!(
            pending
                .push(Outgoing::Message(message, touched.to_vec()))
                .is_ok()
        );
    }

    /// Writes what waits, as the sender does: each item taken and encoded
    /// in turn. Returns the messages, as Core reads them.
    fn write(pending: &mut Pending) -> Vec<OutpostToSupervisor> {
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

    fn summary(message: &OutpostToSupervisor) -> String {
        match message {
            OutpostToSupervisor::Event { event, .. } => format!("{event:?}"),
            OutpostToSupervisor::Reply { request_id, .. } => format!("reply {request_id}"),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn a_waiting_event_is_replaced_by_one_of_the_same_kind_for_the_same_node_at_the_back() {
        let mut pending = pending();
        push(&mut pending, text_changed(1), &[]);
        push(&mut pending, value_changed(1), &[]);
        push(&mut pending, text_changed(2), &[]);
        push(&mut pending, reply(7), &[]);
        push(&mut pending, text_changed(1), &[]);
        let order: Vec<String> = write(&mut pending).iter().map(summary).collect();
        assert_eq!(
            order,
            vec![
                summary(&value_changed(1)),
                summary(&text_changed(2)),
                "reply 7".to_owned(),
                summary(&text_changed(1)),
            ]
        );
    }

    #[test]
    fn answers_and_notifications_are_never_merged() {
        let notification = || {
            event(NormalizedEvent::Notification {
                node_id: node(1),
                notification: verbatim_model::Notification {
                    kind: verbatim_model::NotificationKind::Other,
                    processing: verbatim_model::NotificationProcessing::All,
                    display_string: Some("saved".to_owned()),
                    activity_id: None,
                },
            })
        };
        let mut pending = pending();
        push(&mut pending, reply(1), &[]);
        push(&mut pending, notification(), &[]);
        push(&mut pending, reply(1), &[]);
        push(&mut pending, notification(), &[]);
        assert_eq!(write(&mut pending).len(), 4);
    }

    #[test]
    fn a_terminals_reads_are_never_merged() {
        let mut pending = pending();
        push(&mut pending, terminal("one", false), &[]);
        push(&mut pending, terminal("two", true), &[]);
        push(&mut pending, terminal("three", false), &[]);
        let written = write(&mut pending);
        let texts: Vec<&str> = written.iter().map(text_of).collect();
        assert_eq!(texts, ["one", "two", "three"]);
    }

    /// The whole document of a full history, 9,001 rows of 120 columns
    /// each holding text to its last column, goes to Core as one message.
    #[test]
    fn a_whole_documents_read_of_a_full_history_is_written_whole() {
        let row = "y".repeat(120);
        let text = vec![row.as_str(); 9_001].join(
            "
",
        );
        let mut pending = pending();
        push(&mut pending, terminal(&text, false), &[]);
        let written = write(&mut pending);
        assert_eq!(written.len(), 1);
        assert_eq!(text_of(&written[0]), text);
    }

    fn caret_moved(offset: u32, changes: &[(bool, &str)]) -> OutpostToSupervisor {
        event(NormalizedEvent::CaretMoved {
            node_id: node(1),
            caret: verbatim_model::CaretReport {
                line: verbatim_model::TextChunk {
                    unit: verbatim_model::TextUnit::Line,
                    text: "alpha beta\n".to_owned(),
                    start: verbatim_model::TextAnchor(1),
                    offset,
                    languages: Vec::new(),
                    first: true,
                    last: true,
                    truncated: false,
                    formats: Vec::new(),
                },
                selection: None,
                selection_line: None,
            },
            selection_changes: changes
                .iter()
                .map(|&(selected, text)| verbatim_model::SelectionChange {
                    selected,
                    text: text.to_owned(),
                    characters: 5,
                })
                .collect(),
            content_changed: false,
            new_selection: None,
        })
    }

    #[test]
    fn a_waiting_caret_reports_selection_changes_come_before_the_newer_ones() {
        let mut pending = pending();
        push(&mut pending, caret_moved(5, &[(true, "alpha")]), &[]);
        push(
            &mut pending,
            caret_moved(10, &[(false, "alpha"), (true, "beta ")]),
            &[],
        );
        let written: Vec<String> = write(&mut pending).iter().map(summary).collect();
        assert_eq!(
            written,
            vec![summary(&caret_moved(
                10,
                &[(true, "alpha"), (false, "alpha"), (true, "beta ")]
            ))]
        );
    }

    /// A waiting caret report that carries selection changes is not merged
    /// across a reply: the merged report would be written after the reply,
    /// and its change, which Core had not seen when the key the reply
    /// answers was pressed, would be spoken a second time.
    #[test]
    fn a_caret_report_with_selection_changes_is_not_moved_past_a_reply() {
        let mut pending = pending();
        push(&mut pending, caret_moved(5, &[(true, "alpha")]), &[]);
        push(&mut pending, reply(7), &[]);
        push(&mut pending, caret_moved(10, &[]), &[]);
        let order: Vec<String> = write(&mut pending).iter().map(summary).collect();
        assert_eq!(
            order,
            vec![
                summary(&caret_moved(5, &[(true, "alpha")])),
                "reply 7".to_owned(),
                summary(&caret_moved(10, &[])),
            ]
        );
    }

    #[test]
    fn nothing_is_merged_across_a_flush() {
        let mut pending = pending();
        push(&mut pending, text_changed(1), &[]);
        let (done, _) = mpsc::channel();
        assert!(pending.push(Outgoing::Flushed(done)).is_ok());
        push(&mut pending, text_changed(1), &[]);
        assert_eq!(write(&mut pending).len(), 2);
    }

    #[test]
    fn urgent_messages_go_first() {
        let mut pending = pending();
        push(&mut pending, text_changed(1), &[]);
        assert!(
            pending
                .push(Outgoing::Urgent(OutpostToSupervisor::TargetExited))
                .is_ok()
        );
        let written = write(&mut pending);
        assert_eq!(written.first(), Some(&OutpostToSupervisor::TargetExited));
    }

    #[test]
    fn positions_are_counted_in_the_order_messages_are_written() {
        let mut pending = pending();
        push(&mut pending, text_changed(1), &[1]);
        push(&mut pending, text_changed(2), &[2]);
        // Replaces the first, and goes to the back: Core reads node 2's
        // message first, and this one second.
        push(&mut pending, text_changed(1), &[3]);
        // A message without node ids takes no position.
        push(&mut pending, reply(4), &[4]);
        assert_eq!(write(&mut pending).len(), 3);
        assert_eq!(pending.position, 2, "Core counts two messages");
        assert_eq!(
            pending.take_releasable(&HashSet::new(), 1),
            HashSet::from([2]),
            "only the nodes of the first message written"
        );
        assert_eq!(
            pending.take_releasable(&HashSet::from([3]), 2),
            HashSet::from([1, 4]),
            "the second's, and the reply's, but not a node Core holds"
        );
        assert_eq!(
            pending.take_releasable(&HashSet::new(), 2),
            HashSet::from([3])
        );
    }

    #[test]
    fn a_node_a_waiting_message_may_report_is_kept_though_core_saw_it_earlier() {
        let mut pending = pending();
        push(&mut pending, text_changed(1), &[1]);
        assert_eq!(write(&mut pending).len(), 1);
        push(&mut pending, text_changed(2), &[1]);
        assert!(
            pending.take_releasable(&HashSet::new(), 1).is_empty(),
            "the second message, not yet written, may report node 1 again"
        );
        assert_eq!(write(&mut pending).len(), 1);
        assert!(pending.take_releasable(&HashSet::new(), 1).is_empty());
        assert_eq!(
            pending.take_releasable(&HashSet::new(), 2),
            HashSet::from([1])
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

    fn read(reader: &mut BufReader<File>) -> OutpostToSupervisor {
        read_message(reader)
            .expect("read")
            .expect("a message, not the end")
    }

    #[test]
    fn messages_sent_behind_a_write_in_flight_are_merged_and_written_in_order_after_it() {
        let (outbound, mut reader) = outbound();
        outbound.send(large());
        outbound.send(text_changed(1));
        outbound.send(value_changed(1));
        outbound.send(text_changed(2));
        outbound.send(reply(7));
        outbound.send(text_changed(1));
        outbound.urgent(OutpostToSupervisor::Pong {
            seq: 3,
            parked_count: 0,
            written_at_us: 0,
        });
        assert_eq!(summary(&read(&mut reader)), summary(&large()));
        assert!(
            matches!(read(&mut reader), OutpostToSupervisor::Pong { seq: 3, .. }),
            "the pong goes ahead of what waited"
        );
        let rest: Vec<String> = (0..4).map(|_| summary(&read(&mut reader))).collect();
        assert_eq!(
            rest,
            vec![
                summary(&value_changed(1)),
                summary(&text_changed(2)),
                "reply 7".to_owned(),
                summary(&text_changed(1)),
            ]
        );
        outbound.close();
        assert_eq!(
            read_message::<_, OutpostToSupervisor>(&mut reader).expect("read"),
            None
        );
    }

    /// The idle barrier's answer, sent behind a write in flight, goes after
    /// every message waiting before it, a pong sent after it going ahead of
    /// both, and a newer event of a node whose event waits before it is not
    /// merged into that one across it, so what that event reports is never
    /// moved behind the answer.
    #[test]
    fn the_idle_answer_waits_behind_every_message_before_it_and_nothing_is_merged_across_it() {
        let (outbound, mut reader) = outbound();
        outbound.send(large());
        outbound.send(text_changed(1));
        outbound.send(reply(7));
        outbound.send(OutpostToSupervisor::IdleAnswer { round: 4 });
        outbound.send(text_changed(1));
        outbound.urgent(OutpostToSupervisor::Pong {
            seq: 3,
            parked_count: 0,
            written_at_us: 0,
        });
        assert_eq!(summary(&read(&mut reader)), summary(&large()));
        assert!(
            matches!(read(&mut reader), OutpostToSupervisor::Pong { seq: 3, .. }),
            "the pong goes ahead of what waited"
        );
        let rest: Vec<String> = (0..4).map(|_| summary(&read(&mut reader))).collect();
        assert_eq!(
            rest,
            vec![
                summary(&text_changed(1)),
                "reply 7".to_owned(),
                summary(&OutpostToSupervisor::IdleAnswer { round: 4 }),
                summary(&text_changed(1)),
            ]
        );
        outbound.close();
        assert_eq!(
            read_message::<_, OutpostToSupervisor>(&mut reader).expect("read"),
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
        assert_eq!(
            done.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "the large message's write is still in flight"
        );
        assert_eq!(summary(&read(&mut reader)), summary(&large()));
        assert_eq!(done.recv_timeout(Duration::from_secs(10)), Ok(()));
    }
}
