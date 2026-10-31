//! Merging waiting messages (`docs/design/ownership.md`): a message type that may be merged while it
//! waits to be written declares how, once, by [`Merge`], and every queue
//! that merges it places a newer message with [`merge_into`].
//!
//! The one implementation is [`OutpostToCore`]'s, for the outpost's
//! outbound queue: an event replaces a waiting event of the same node and
//! kind (and, for a property change, property; for a focus, whether it is
//! a foreground change), and goes to the back, as NVDA's limiter gives a
//! newer event of one key the newest place. A caret report carries the
//! older report's selection changes before its own. Nothing merges across
//! the idle barrier's answer, and a caret report with selection changes
//! does not merge across a reply, nor with a report that says its changes
//! in general terms.

use std::mem::Discriminant;

use super::OutpostToCore;
use crate::{NodeId, NormalizedEvent, PropertyChange};

/// What a waiting message is to a newer one merged across it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Barrier {
    /// Any message may be merged across it.
    None,
    /// Only a message whose kind refuses to cross a reply is not merged
    /// across it ([`Merge::may_merge`]).
    Reply,
    /// Nothing is merged across it.
    All,
}

/// How a message type merges while it waits.
pub trait Merge: Sized {
    /// What two messages must share to be merged.
    type Key: Eq;

    /// The key this message is merged under, or `None` for a message that
    /// is never merged.
    fn merge_key(&self) -> Option<Self::Key>;

    /// What this message is to a newer one merged across it.
    fn barrier(&self) -> Barrier;

    /// Whether `newer` may be merged into `older`, the waiting message with
    /// its key, across waiting messages whose strongest barrier is
    /// `crossed`, never [`Barrier::All`].
    fn may_merge(older: &Self, newer: &Self, crossed: Barrier) -> bool;

    /// `newer` merged into `older`: what is written in their place, at the
    /// newer one's.
    fn merged(older: Self, newer: Self) -> Self;
}

/// Where `newer` goes among `waiting`, the messages waiting since the last
/// flush marker, oldest first, each with its place in its queue: the place
/// of the waiting message it is merged into, which it then replaces at the
/// back of the queue ([`Merge::merged`]), or `None` when it is simply
/// queued at the back. The newest waiting message with its key decides,
/// across those after it; nothing is merged across [`Barrier::All`].
pub fn merge_into<'a, M: Merge + 'a>(
    waiting: impl DoubleEndedIterator<Item = (usize, &'a M)>,
    newer: &M,
) -> Option<usize> {
    let key = newer.merge_key()?;
    let mut crossed = Barrier::None;
    for (place, older) in waiting.rev() {
        if older.merge_key().as_ref() == Some(&key) {
            return M::may_merge(older, newer, crossed).then_some(place);
        }
        match older.barrier() {
            Barrier::All => return None,
            barrier @ (Barrier::None | Barrier::Reply) => crossed = crossed.max(barrier),
        }
    }
    None
}

/// What two of an outpost's events must share to be merged: the node and
/// the kind of event (and, for a property change, which property; for a
/// focus, whether it is a foreground change).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventKey {
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

impl Merge for OutpostToCore {
    type Key = EventKey;

    /// An event's node and kind; `None` for anything but an event, for a
    /// notification, which says something of its own, and for a terminal's
    /// read, since where each read starts is where the one before it said
    /// to start.
    fn merge_key(&self) -> Option<EventKey> {
        let OutpostToCore::Event { event, .. } = self else {
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
            | NormalizedEvent::ActiveTextPositionChanged { node_id, .. } => {
                (*node_id, Detail::None)
            }
            NormalizedEvent::Notification { .. } | NormalizedEvent::TerminalSnapshot { .. } => {
                return None;
            }
        };
        Some(EventKey { node, kind, detail })
    }

    /// The idle barrier's answer is merged across by nothing, so it never
    /// overtakes what was sent before it; a reply only bars a caret report
    /// with selection changes (Dickson, 2026-10-30).
    fn barrier(&self) -> Barrier {
        match self {
            OutpostToCore::IdleAnswer { .. } => Barrier::All,
            OutpostToCore::Reply { .. } => Barrier::Reply,
            OutpostToCore::Ready { .. }
            | OutpostToCore::SpareReady { .. }
            | OutpostToCore::Event { .. }
            | OutpostToCore::Pong { .. }
            | OutpostToCore::Fault { .. }
            | OutpostToCore::TargetExited => Barrier::None,
        }
    }

    /// A caret report that carries selection changes is not merged across
    /// a reply: a change moved behind a reply would reach Core after the
    /// reply to a key pressed before Core saw it, whose own selection
    /// changes, compared with the selection Core knew, already include it.
    /// Nor is it merged with a newer report when either says its changes
    /// in general terms ([`crate::CaretReply::content_changed`]): each such
    /// report's words are worked out from its own selection.
    fn may_merge(older: &Self, newer: &Self, crossed: Barrier) -> bool {
        !(carries_selection_changes(older)
            && (crossed >= Barrier::Reply || content_changed(older) || content_changed(newer)))
    }

    /// `newer` in place of `older`: a caret report carrying the older one's
    /// selection changes before its own, each change being from the report
    /// before, and anything else replacing it. Two caret reports merged
    /// are never both in general terms, nor is the older when it carries
    /// changes ([`Merge::may_merge`]), so the newer's general terms are
    /// kept.
    fn merged(older: Self, newer: Self) -> Self {
        match (older, newer) {
            (
                OutpostToCore::Event {
                    event:
                        NormalizedEvent::CaretMoved {
                            selection_changes: mut waiting,
                            ..
                        },
                    ..
                },
                OutpostToCore::Event {
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
                OutpostToCore::Event {
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
}

/// Whether `message` is a caret report that carries selection changes.
fn carries_selection_changes(message: &OutpostToCore) -> bool {
    matches!(
        message,
        OutpostToCore::Event {
            event: NormalizedEvent::CaretMoved { selection_changes, .. },
            ..
        } if !selection_changes.is_empty()
    )
}

/// Whether `message` is a caret report whose selection changes are said in
/// general terms.
fn content_changed(message: &OutpostToCore) -> bool {
    matches!(
        message,
        OutpostToCore::Event {
            event: NormalizedEvent::CaretMoved {
                content_changed: true,
                ..
            },
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message of a test's own: a key, whether it refuses to cross a
    /// reply, and its barrier.
    #[derive(Debug, PartialEq)]
    struct Message {
        key: Option<u8>,
        refuses_replies: bool,
        barrier: Barrier,
    }

    impl Merge for Message {
        type Key = u8;

        fn merge_key(&self) -> Option<u8> {
            self.key
        }

        fn barrier(&self) -> Barrier {
            self.barrier
        }

        fn may_merge(older: &Self, _newer: &Self, crossed: Barrier) -> bool {
            !(older.refuses_replies && crossed >= Barrier::Reply)
        }

        fn merged(_older: Self, newer: Self) -> Self {
            newer
        }
    }

    fn keyed(key: u8, refuses_replies: bool) -> Message {
        Message {
            key: Some(key),
            refuses_replies,
            barrier: Barrier::None,
        }
    }

    fn barrier(barrier: Barrier) -> Message {
        Message {
            key: None,
            refuses_replies: false,
            barrier,
        }
    }

    /// Where `newer` goes among `waiting`, each at its own place.
    fn place(waiting: &[Message], newer: &Message) -> Option<usize> {
        merge_into(waiting.iter().enumerate(), newer)
    }

    #[test]
    fn nothing_merges_across_a_barrier_to_all() {
        for refuses_replies in [false, true] {
            let waiting = [keyed(1, refuses_replies), barrier(Barrier::All)];
            assert_eq!(place(&waiting, &keyed(1, refuses_replies)), None);
        }
    }

    #[test]
    fn a_key_that_refuses_replies_does_not_merge_across_one_and_any_other_does() {
        let refusing = [keyed(1, true), barrier(Barrier::Reply)];
        assert_eq!(place(&refusing, &keyed(1, true)), None);
        let other = [keyed(1, false), barrier(Barrier::Reply)];
        assert_eq!(place(&other, &keyed(1, false)), Some(0));
        let unbarred = [keyed(1, true), barrier(Barrier::None)];
        assert_eq!(place(&unbarred, &keyed(1, true)), Some(0));
    }

    #[test]
    fn the_newest_waiting_message_of_the_key_decides() {
        let waiting = [
            keyed(1, false),
            keyed(2, false),
            keyed(1, true),
            barrier(Barrier::Reply),
        ];
        assert_eq!(place(&waiting, &keyed(1, false)), None);
        assert_eq!(place(&waiting, &keyed(2, false)), Some(1));
        assert_eq!(place(&waiting, &barrier(Barrier::None)), None);
    }
}
