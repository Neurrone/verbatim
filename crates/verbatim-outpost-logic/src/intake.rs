//! What the outpost still hears, queued for its worker
//! (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in this
//! build"): its `Follow` subscription's UI Automation events, a console
//! host's console events, a window Core named destroyed, and Core's
//! lists of held nodes and idle queries, handled in the order they
//! arrived. The event queue is Core's: the intake keeps only the
//! limiter on what the outpost hears, as NVDA's UIA event limiter keeps
//! one event per element and kind.
//!
//! - One waiting entry per object and event kind: a newer one replaces it
//!   and takes its place at the back. A console's events merge per window
//!   and kind, since each names only the cells it changed and the whole
//!   is read. A newer list of the nodes Core holds replaces a waiting one.
//! - A batch is everything that accumulated while the worker handled the
//!   previous batch, up to and including the first idle query, handed out
//!   in the order it arrived.
//!
//! The intake is the reducer's ([`super`]): the mailbox's producers only
//! append, and the reducer pushes each entry it drains into the intake, in
//! mailbox order.

use std::collections::VecDeque;

use super::entry::{Asker, Entry, Item, UiaKind, WinKind};

/// The object and kind an entry concerns, for the one-per-object rule.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    /// A console's event, by its kind and window.
    Console(WinKind, isize),
    /// A UIA event, by its kind number, its property for a property change,
    /// and its element's runtime id.
    Uia(u8, i32, Vec<i32>),
    /// The list of the nodes Core holds.
    NodesHeld,
}

/// An entry with its key, computed when it is pushed.
#[cfg_attr(test, derive(Clone, Debug))]
struct Waiting {
    key: Option<Key>,
    entry: Entry,
}

/// The intake: what waits, and the batch being handed out. One owner, the
/// reducer, so it takes no lock.
#[cfg_attr(test, derive(Clone, Debug))]
#[derive(Default)]
pub struct Intake {
    waiting: VecDeque<Waiting>,
    batch: VecDeque<Entry>,
    closed: bool,
}

/// What one push did besides queuing its entry: the objects parked for
/// entries it removed, which a worker drops.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pushed {
    pub unparked: Vec<u64>,
}

impl Intake {
    /// Adds an entry. An entry with a key replaces the one waiting with the
    /// same key, unless an idle query waits between them
    /// (`docs/design/communication.md`, section 6.7): the older entry then
    /// keeps its place ahead of the query. Returns the parked objects of
    /// the entries a newer one replaced.
    pub fn push(&mut self, entry: Entry) -> Pushed {
        let mut pushed = Pushed::default();
        if self.closed {
            pushed.unparked = entry.item.parked();
            return pushed;
        }
        let key = key_of(&entry.item);
        if let Some(key) = &key
            && let Some(gone) = remove_keyed(&mut self.waiting, key)
        {
            pushed.unparked.extend(gone.entry.item.parked());
        }
        self.waiting.push_back(Waiting { key, entry });
        pushed
    }

    /// The next entry to handle, planning a new batch from everything
    /// waiting up to the first idle query when the current one is done.
    /// `None` when nothing is ready.
    pub fn next(&mut self) -> Option<Entry> {
        if let Some(entry) = self.batch.pop_front() {
            return Some(entry);
        }
        // An idle query ends its batch, so nothing that arrived after it is
        // handed out with what arrived before.
        let ready = self
            .waiting
            .iter()
            .position(|waiting| matches!(waiting.entry.item, Item::Barrier { .. }))
            .map_or(self.waiting.len(), |at| at.saturating_add(1));
        if ready > 0 {
            self.batch = self
                .waiting
                .drain(..ready)
                .map(|waiting| waiting.entry)
                .collect();
            return self.batch.pop_front();
        }
        None
    }

    /// Closes the queue for the outpost's shutdown: everything waiting and
    /// the batch in progress are dropped, nothing pushed from now on is
    /// kept, and [`next`](Self::next) answers `None`. The parked objects of
    /// what was dropped are returned, for a worker to drop.
    pub fn close(&mut self) -> Vec<u64> {
        self.closed = true;
        let dropped = self
            .waiting
            .drain(..)
            .map(|waiting| waiting.entry)
            .chain(self.batch.drain(..));
        dropped.flat_map(|entry| entry.item.parked()).collect()
    }

    /// The entries waiting, then those planned and not yet handed out.
    fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.waiting
            .iter()
            .map(|waiting| &waiting.entry)
            .chain(self.batch.iter())
    }

    /// The tests' settles waiting, planned or not, by number.
    pub fn settles(&self) -> std::collections::BTreeSet<u64> {
        self.entries()
            .filter_map(|entry| match entry.item {
                Item::Barrier {
                    asker: Asker::Settle(id),
                    ..
                } => Some(id),
                _ => None,
            })
            .collect()
    }
}

/// Removes the entry waiting with `key` behind the last idle query: an
/// idle query splits what waits into stretches, each holding at most one
/// entry per key, and the entry replaced would move behind the query.
fn remove_keyed(waiting: &mut VecDeque<Waiting>, key: &Key) -> Option<Waiting> {
    let found = waiting
        .iter()
        .rev()
        .take_while(|waiting| !matches!(waiting.entry.item, Item::Barrier { .. }))
        .position(|waiting| waiting.key.as_ref() == Some(key))
        .and_then(|back| waiting.len().checked_sub(1)?.checked_sub(back))?;
    waiting.remove(found)
}

/// The key of an item, if it merges with a newer one.
fn key_of(item: &Item) -> Option<Key> {
    match item {
        // Each console update names the cells it changed, and each console
        // caret event the cell its cursor is in; only that the window's
        // text changed, or its cursor moved, matters, as both are read, so
        // each kind merges into one per window while they wait.
        Item::Msaa { kind, hwnd, .. } => Some(Key::Console(*kind, *hwnd)),
        Item::Uia(event) => {
            let runtime_id = event.parts.runtime_id.clone();
            Some(match &event.kind {
                UiaKind::Property { id, .. } => Key::Uia(0, *id, runtime_id),
                UiaKind::TextSelection => Key::Uia(2, 0, runtime_id),
                UiaKind::TextChanged => Key::Uia(3, 0, runtime_id),
            })
        }
        // Only the newest list of held nodes matters.
        Item::NodesHeld { .. } => Some(Key::NodesHeld),
        Item::Barrier { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::TraceId;
    use verbatim_model::protocol::UiaSnapshotFact;

    use super::super::entry::UiaEvent;
    use super::*;

    fn snapshot(runtime_id: Vec<i32>) -> UiaSnapshotFact {
        UiaSnapshotFact {
            runtime_id,
            role: verbatim_model::Role::Unknown,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        }
    }

    fn push_item(intake: &mut Intake, item: Item, observed_at_us: u64) {
        let _ = intake.push(Entry::own(
            item,
            TraceId::from_raw(observed_at_us),
            observed_at_us,
        ));
    }

    /// Hands out everything ready, by the observation of each entry.
    fn handed_out(intake: &mut Intake) -> Vec<u64> {
        std::iter::from_fn(|| intake.next())
            .map(|entry| entry.observed_at_us)
            .collect()
    }

    fn text_event(kind: UiaKind, runtime_id: &[i32]) -> Item {
        Item::Uia(UiaEvent {
            kind,
            parts: snapshot(runtime_id.to_vec()),
            hwnd: 0,
            element: None,
        })
    }

    /// A console event of `kind` in window `hwnd`.
    fn console(kind: WinKind, hwnd: isize) -> Item {
        Item::Msaa {
            kind,
            hwnd,
            id_object: 3,
            id_child: 65,
            raiser: 80,
        }
    }

    #[test]
    fn a_windows_console_updates_merge_into_one_while_they_wait() {
        let mut intake = Intake::default();
        push_item(&mut intake, console(WinKind::ConsoleUpdate, 7), 1);
        push_item(&mut intake, console(WinKind::ConsoleUpdate, 8), 2);
        push_item(&mut intake, console(WinKind::ConsoleUpdate, 7), 3);
        assert_eq!(handed_out(&mut intake), [2, 3]);
    }

    #[test]
    fn a_newer_event_for_the_same_element_and_kind_replaces_the_waiting_one() {
        let mut intake = Intake::default();
        push_item(&mut intake, text_event(UiaKind::TextChanged, &[1]), 1);
        push_item(&mut intake, text_event(UiaKind::TextSelection, &[1]), 2);
        push_item(&mut intake, text_event(UiaKind::TextChanged, &[2]), 3);
        push_item(&mut intake, text_event(UiaKind::TextChanged, &[1]), 4);
        assert_eq!(handed_out(&mut intake), [2, 3, 4]);
    }

    fn idle_query(round: u64) -> Item {
        Item::Barrier {
            asker: Asker::Round(round),
        }
    }

    /// An idle query divides what waits: a console update that waited
    /// before it is not replaced by a newer one of the same window behind
    /// it, and is handled ahead of it.
    #[test]
    fn an_entry_waiting_before_an_idle_query_is_never_replaced_from_behind_it() {
        let mut intake = Intake::default();
        push_item(&mut intake, console(WinKind::ConsoleUpdate, 7), 1);
        push_item(&mut intake, idle_query(1), 2);
        push_item(&mut intake, console(WinKind::ConsoleUpdate, 7), 3);
        // Behind the query, a newer update of the same window still
        // replaces the one waiting with it.
        push_item(&mut intake, console(WinKind::ConsoleUpdate, 7), 4);
        assert_eq!(handed_out(&mut intake), [1, 2, 4]);
    }

    #[test]
    fn a_newer_list_of_held_nodes_replaces_the_waiting_one() {
        let mut intake = Intake::default();
        let held = |acknowledged| Item::NodesHeld {
            nodes: Vec::new(),
            anchors: Vec::new(),
            acknowledged,
        };
        push_item(&mut intake, held(1), 1);
        push_item(&mut intake, console(WinKind::ConsoleUpdate, 7), 2);
        push_item(&mut intake, held(2), 3);
        assert_eq!(handed_out(&mut intake), [2, 3]);
    }
}
