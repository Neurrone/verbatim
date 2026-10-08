//! The outpost's messages to Core: one queue, and one thread that writes it
//! to Core's pipe (`docs/crates/verbatim-outpost.md`, "Messages to Core").
//!
//! - Pongs, `Ready`, and `TargetExited` go ahead of everything else, so
//!   heartbeat delivery never waits behind event traffic and a busy outpost
//!   is never mistaken for a dead one.
//! - Queuing never waits: nothing that queues a message, the worker and the
//!   watchdog above all, can be held up by a pipe Core is slow to read.
//! - While messages wait, they are merged by object and kind, as the intake
//!   and the supervisor merge what reaches the outpost and as NVDA's
//!   limiters do: an event replaces a waiting event of the same kind for
//!   the same node and goes to the back, and a terminal's new output is
//!   combined with its output still waiting, under the flood policy's
//!   limits ([`crate::terminal::combine`]). Answers to Core's requests,
//!   notifications, and faults are never merged, and keep their order.
//!   What waits is therefore bounded by the nodes and kinds there are, by
//!   Core's requests, and by the intake's own limits.
//! - The queue numbers the messages that carry node ids as Core counts
//!   them, in the order they are written, and records which nodes each may
//!   have reported, so the worker releases only nodes Core has seen
//!   ([`Outbound::release`]).

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::mem::Discriminant;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use verbatim_model::{NodeId, NormalizedEvent, PropertyChange};

use crate::protocol::{OutpostToSupervisor, write_message};

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
        | NormalizedEvent::ProgressChanged { node }
        | NormalizedEvent::ControlledSelection { node, .. }
        | NormalizedEvent::Alert { node } => (node.id, Detail::None),
        NormalizedEvent::ValueChanged { node_id, .. }
        | NormalizedEvent::CaretMoved { node_id, .. }
        | NormalizedEvent::NoText { node_id }
        | NormalizedEvent::TextChanged { node_id }
        | NormalizedEvent::TerminalOutput { node_id, .. }
        | NormalizedEvent::ActiveTextPositionChanged { node_id, .. } => (*node_id, Detail::None),
        // A notification, and any kind of event added later, is never
        // merged.
        _ => return None,
    };
    Some(Key { node, kind, detail })
}

/// One entry in the ordinary queue.
enum Waiting {
    /// A message to write, with the key it is merged under.
    Message(Option<Key>, Box<OutpostToSupervisor>),
    /// Answered once every message queued before it is written. Nothing
    /// queued after it is merged into a message before it.
    Flushed(std::sync::mpsc::Sender<()>),
    /// Ends the writer once every message queued before it is written,
    /// closing the pipe: the outpost is shutting down.
    Close,
}

/// What the writer takes next.
enum Next {
    Message(Box<OutpostToSupervisor>),
    Flushed(std::sync::mpsc::Sender<()>),
    Close,
}

/// The waiting messages and the numbering of those that carry node ids.
/// Pure, so the merging and the numbering are tested on their own.
#[derive(Default)]
struct Pending {
    urgent: VecDeque<OutpostToSupervisor>,
    ordinary: VecDeque<Waiting>,
    /// Whether the close has been queued: nothing more is accepted.
    closed: bool,
    /// How many messages that carry node ids have been queued and not
    /// merged away: the position of the last one, as Core will count it.
    position: u64,
    /// The position of the last message that may have reported each node,
    /// by node number.
    reported: HashMap<u64, u64>,
}

impl Pending {
    /// Queues `message`, recording each of `touched` as reported at the
    /// position it then has. A message merged away gives up its position:
    /// the messages after it come one place earlier than their recorded
    /// positions say, which only keeps their nodes a little longer.
    fn push(
        &mut self,
        message: OutpostToSupervisor,
        touched: impl IntoIterator<Item = u64>,
        terminal_lines: usize,
    ) {
        if self.closed {
            return;
        }
        let key = key(&message);
        let mut message = message;
        if let Some(key) = &key
            && let Some(index) = self.waiting_with(key)
            && let Some(Waiting::Message(_, older)) = self.ordinary.remove(index)
        {
            if older.carries_nodes() {
                self.position -= 1;
            }
            message = merged(*older, message, terminal_lines);
        }
        if message.carries_nodes() {
            self.position += 1;
        }
        for number in touched {
            self.reported.insert(number, self.position);
        }
        self.ordinary
            .push_back(Waiting::Message(key, Box::new(message)));
    }

    /// Where the waiting message with `key` is, if one is waiting since the
    /// last flush marker.
    fn waiting_with(&self, key: &Key) -> Option<usize> {
        for (index, waiting) in self.ordinary.iter().enumerate().rev() {
            match waiting {
                Waiting::Message(Some(other), _) if other == key => return Some(index),
                Waiting::Message(..) => {}
                Waiting::Flushed(_) | Waiting::Close => return None,
            }
        }
        None
    }

    fn push_marker(&mut self, marker: Waiting) -> Result<(), Waiting> {
        if self.closed {
            return Err(marker);
        }
        self.closed = matches!(marker, Waiting::Close);
        self.ordinary.push_back(marker);
        Ok(())
    }

    fn pop(&mut self) -> Option<Next> {
        if let Some(message) = self.urgent.pop_front() {
            return Some(Next::Message(Box::new(message)));
        }
        Some(match self.ordinary.pop_front()? {
            Waiting::Message(_, message) => Next::Message(message),
            Waiting::Flushed(done) => Next::Flushed(done),
            Waiting::Close => Next::Close,
        })
    }

    /// Takes the nodes to release for a held list acknowledging position
    /// `acknowledged`: those not held that were reported at or before it.
    /// A node reported later, or never reported, is kept, since Core may not
    /// have seen it yet.
    fn take_releasable(&mut self, held: &HashSet<u64>, acknowledged: u64) -> HashSet<u64> {
        let released: HashSet<u64> = self
            .reported
            .iter()
            .filter(|&(number, &position)| position <= acknowledged && !held.contains(number))
            .map(|(&number, _)| number)
            .collect();
        self.reported.retain(|number, _| !released.contains(number));
        released
    }
}

/// `newer` in place of `older`, the waiting message with the same key: a
/// terminal's output combined with what was waiting, anything else
/// replacing it.
fn merged(
    older: OutpostToSupervisor,
    newer: OutpostToSupervisor,
    terminal_lines: usize,
) -> OutpostToSupervisor {
    match (older, newer) {
        (
            OutpostToSupervisor::Event {
                event:
                    NormalizedEvent::TerminalOutput {
                        output: waiting, ..
                    },
                ..
            },
            OutpostToSupervisor::Event {
                trace_id,
                observed_at_ms,
                backend,
                window,
                timing,
                event: NormalizedEvent::TerminalOutput { node_id, output },
            },
        ) => OutpostToSupervisor::Event {
            trace_id,
            observed_at_ms,
            backend,
            window,
            timing,
            event: NormalizedEvent::TerminalOutput {
                node_id,
                output: crate::terminal::combine(waiting, output, terminal_lines),
            },
        },
        (_, newer) => newer,
    }
}

struct Shared {
    pending: Mutex<Pending>,
    ready: Condvar,
    /// How many of a change's newest lines a terminal read takes
    /// ([`crate::protocol::SupervisorToOutpost::TerminalLines`]), which
    /// also bounds combined output.
    terminal_lines: AtomicU16,
}

/// The sending side of the writer.
#[derive(Clone)]
pub(crate) struct Outbound {
    shared: Arc<Shared>,
}

impl Outbound {
    /// Starts the writer thread over `pipe`.
    ///
    /// # Panics
    ///
    /// Panics if the thread cannot be spawned, which means the process is
    /// out of OS thread resources.
    pub(crate) fn start(pipe: Box<dyn Write + Send>) -> (Self, JoinHandle<()>) {
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            ready: Condvar::new(),
            terminal_lines: AtomicU16::new(verbatim_model::DEFAULT_TERMINAL_LINES),
        });
        let writer = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("verbatim-outbound".to_owned())
            .spawn(move || write_loop(pipe, &writer))
            .expect("spawn the outbound writer");
        (Self { shared }, join)
    }

    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.shared
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// How many of a change's newest lines a terminal read takes.
    pub(crate) fn terminal_lines(&self) -> u16 {
        self.shared.terminal_lines.load(Ordering::Relaxed)
    }

    /// Sets how many of a change's newest lines a terminal read takes.
    pub(crate) fn set_terminal_lines(&self, lines: u16) {
        self.shared.terminal_lines.store(lines, Ordering::Relaxed);
    }

    /// Queues a message that must not wait behind others: a pong, `Ready`,
    /// or `TargetExited`.
    pub(crate) fn urgent(&self, message: OutpostToSupervisor) {
        let mut pending = self.lock();
        if !pending.closed {
            pending.urgent.push_back(message);
        }
        drop(pending);
        self.shared.ready.notify_one();
    }

    /// Queues an ordinary message that reports no node, without waiting.
    pub(crate) fn send(&self, message: OutpostToSupervisor) {
        self.publish(message, []);
    }

    /// Queues an ordinary message without waiting, recording each of
    /// `touched`, the nodes issued or looked up since the last message, as
    /// reported at the position the message takes.
    pub(crate) fn publish(
        &self,
        message: OutpostToSupervisor,
        touched: impl IntoIterator<Item = u64>,
    ) {
        let lines = usize::from(self.terminal_lines());
        self.lock().push(message, touched, lines);
        self.shared.ready.notify_one();
    }

    /// Takes the nodes to release for Core's held list `held`, which
    /// acknowledges position `acknowledged`, and runs `forget` on them
    /// before any message queued afterwards can record them again. `None`
    /// when there is nothing to release. Returns the position reached too,
    /// for the log.
    pub(crate) fn release<R>(
        &self,
        held: &HashSet<u64>,
        acknowledged: u64,
        forget: impl FnOnce(&HashSet<u64>) -> R,
    ) -> (u64, Option<(HashSet<u64>, R)>) {
        let mut pending = self.lock();
        let released = pending.take_releasable(held, acknowledged);
        if released.is_empty() {
            return (pending.position, None);
        }
        let forgotten = forget(&released);
        (pending.position, Some((released, forgotten)))
    }

    /// Has the writer write every message queued before this call and
    /// then end, closing the pipe, for the outpost's shutdown. Whatever is
    /// sent afterwards is dropped.
    pub(crate) fn close(&self) {
        let _ = self.lock().push_marker(Waiting::Close);
        self.shared.ready.notify_one();
    }

    /// Waits until every ordinary message queued before this call has been
    /// written to the pipe, or the writer has stopped.
    pub(crate) fn flush(&self) {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let queued = self.lock().push_marker(Waiting::Flushed(done_tx)).is_ok();
        self.shared.ready.notify_one();
        if queued {
            // An error means the writer stopped, with nothing left to write.
            let _ = done_rx.recv();
        }
    }
}

fn write_loop(mut pipe: Box<dyn Write + Send>, shared: &Shared) {
    loop {
        let next = {
            let mut pending = shared
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(next) = pending.pop() {
                    break next;
                }
                pending = shared
                    .ready
                    .wait(pending)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        // Written with no lock held.
        match next {
            Next::Message(message) => {
                for message in within_limit(*message) {
                    if let Err(error) = write_message(&mut pipe, &message) {
                        tracing::warn!(%error, "a message to Core could not be written");
                        return;
                    }
                }
            }
            Next::Flushed(done) => {
                let _ = done.send(());
            }
            Next::Close => {
                // Urgent messages queued before the close go too.
                let urgent = std::mem::take(
                    &mut shared
                        .pending
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .urgent,
                );
                for message in &urgent {
                    if write_message(&mut pipe, message).is_err() {
                        return;
                    }
                }
                let _ = pipe.flush();
                return;
            }
        }
    }
}

/// `message` as messages that each stay within the limit on outpost
/// messages: a terminal's output larger than
/// [`crate::terminal::MESSAGE_TEXT_BUDGET`] is
/// split ([`crate::terminal::split`]) into outputs sent one after
/// another, an answer to Core's request carrying the first; anything else
/// as it is.
fn within_limit(message: OutpostToSupervisor) -> Vec<OutpostToSupervisor> {
    match message {
        OutpostToSupervisor::Event {
            trace_id,
            observed_at_ms,
            backend,
            window,
            timing,
            event: NormalizedEvent::TerminalOutput { node_id, output },
        } => crate::terminal::split(output, crate::terminal::MESSAGE_TEXT_BUDGET)
            .into_iter()
            .map(|output| OutpostToSupervisor::Event {
                trace_id,
                observed_at_ms,
                backend,
                window,
                timing,
                event: NormalizedEvent::TerminalOutput { node_id, output },
            })
            .collect(),
        other => vec![other],
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{Backend, TerminalOutput, TraceId};

    use super::*;
    use crate::protocol::{EventTiming, QueryOutcome};

    fn node(number: u64) -> NodeId {
        NodeId::new(number)
    }

    fn event(event: NormalizedEvent) -> OutpostToSupervisor {
        OutpostToSupervisor::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: 0,
            backend: Backend::Uia,
            window: None,
            timing: EventTiming::default(),
            event,
        }
    }

    fn text_changed(number: u64) -> OutpostToSupervisor {
        event(NormalizedEvent::TextChanged {
            node_id: node(number),
        })
    }

    fn no_text(number: u64) -> OutpostToSupervisor {
        event(NormalizedEvent::NoText {
            node_id: node(number),
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

    fn terminal(lines: &[&str]) -> OutpostToSupervisor {
        event(NormalizedEvent::TerminalOutput {
            node_id: node(9),
            output: TerminalOutput {
                lines: lines.iter().map(|&line| line.to_owned()).collect(),
                ..TerminalOutput::default()
            },
        })
    }

    fn drain(pending: &mut Pending) -> Vec<OutpostToSupervisor> {
        std::iter::from_fn(|| pending.pop())
            .filter_map(|next| match next {
                Next::Message(message) => Some(*message),
                Next::Flushed(_) | Next::Close => None,
            })
            .collect()
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
        let mut pending = Pending::default();
        pending.push(text_changed(1), [], 30);
        pending.push(no_text(1), [], 30);
        pending.push(text_changed(2), [], 30);
        pending.push(reply(7), [], 30);
        pending.push(text_changed(1), [], 30);
        let order: Vec<String> = drain(&mut pending).iter().map(summary).collect();
        assert_eq!(
            order,
            vec![
                summary(&no_text(1)),
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
        let mut pending = Pending::default();
        pending.push(reply(1), [], 30);
        pending.push(notification(), [], 30);
        pending.push(reply(1), [], 30);
        pending.push(notification(), [], 30);
        assert_eq!(drain(&mut pending).len(), 4);
    }

    #[test]
    fn a_terminals_waiting_output_is_combined_with_its_newer_output() {
        let mut pending = Pending::default();
        pending.push(terminal(&["one", "two"]), [], 3);
        pending.push(terminal(&["three", "four"]), [], 3);
        let written = drain(&mut pending);
        let [
            OutpostToSupervisor::Event {
                event: NormalizedEvent::TerminalOutput { output, .. },
                ..
            },
        ] = written.as_slice()
        else {
            panic!("one combined output, not {written:?}");
        };
        assert_eq!(
            output,
            &TerminalOutput {
                above: Vec::new(),
                changed: None,
                head: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
                skipped: None,
                lines: vec!["four".to_owned()],
            }
        );
    }

    #[test]
    fn nothing_is_merged_across_a_flush() {
        let mut pending = Pending::default();
        pending.push(text_changed(1), [], 30);
        let (done, _) = std::sync::mpsc::channel();
        assert!(pending.push_marker(Waiting::Flushed(done)).is_ok());
        pending.push(text_changed(1), [], 30);
        assert_eq!(drain(&mut pending).len(), 2);
    }

    #[test]
    fn urgent_messages_go_first() {
        let mut pending = Pending::default();
        pending.push(text_changed(1), [], 30);
        pending.urgent.push_back(OutpostToSupervisor::TargetExited);
        let written = drain(&mut pending);
        assert_eq!(written.first(), Some(&OutpostToSupervisor::TargetExited));
    }

    #[test]
    fn a_held_list_releases_only_nodes_core_has_seen_and_does_not_hold() {
        let mut pending = Pending::default();
        pending.push(text_changed(1), [1, 2], 30);
        pending.push(text_changed(3), [3], 30);
        // A message without node ids takes no position.
        pending.push(reply(1), [4], 30);

        let held = HashSet::from([2]);
        assert_eq!(pending.take_releasable(&held, 1), HashSet::from([1]));
        assert_eq!(
            pending.take_releasable(&HashSet::new(), 2),
            HashSet::from([2, 3, 4])
        );
        pending.push(text_changed(5), [5], 30);
        assert!(
            pending.take_releasable(&HashSet::new(), 2).is_empty(),
            "a node reported after the acknowledged position is kept"
        );
    }

    #[test]
    fn a_message_merged_away_gives_up_its_position() {
        let mut pending = Pending::default();
        pending.push(text_changed(1), [1], 30);
        pending.push(text_changed(2), [2], 30);
        pending.push(text_changed(1), [3], 30);
        assert_eq!(pending.position, 2, "Core will count two messages");
        assert_eq!(
            pending.take_releasable(&HashSet::new(), 1),
            HashSet::from([1]),
            "node 2, whose message is now written first, is kept until the second"
        );
        assert_eq!(
            pending.take_releasable(&HashSet::new(), 2),
            HashSet::from([2, 3])
        );
    }
}
