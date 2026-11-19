//! Where an arriving entry waits: the merge, MSAA runs, NVDA's limiter on
//! the waiting MSAA entries and the menu slot
//! (`docs/design/readers-tell-core-asks-build1.md`, "The event queue" and
//! Table 1; `docs/parity/backends.md`, "MSAA winevent flood control", "MSAA
//! menu events in one flush").
//!
//! NVDA applies its limiter once per pump, to the events gathered since
//! the last; Core has no pump and applies it each time an MSAA entry joins,
//! counting only the waiting MSAA entries that arrived within one pump
//! window of the newest ([`PUMP_WINDOW_US`]), by the arrival times their
//! tells carry. Only waiting entries are merged, joined or counted; the
//! asked front entry never is.

use std::collections::{BTreeMap, VecDeque};

use verbatim_model::WindowHandle;
use verbatim_model::event_kind::EventKind;

use serde::{Deserialize, Serialize};

use super::Logic;

/// How many focus and foreground candidates wait at most, the newest kept.
pub(crate) const FOCUS_CAP: usize = 4;
/// How many other MSAA entries of one raising thread wait at most, the
/// newest kept.
pub(crate) const THREAD_CAP: usize = 10;
/// How far apart, in microseconds, two MSAA entries' arrivals may lie and
/// still be counted by the caps together: NVDA's core pump runs at most
/// 10 ms after a pump is requested (`PUMP_MAX_DELAY`, `core.py` 55), and its
/// limiter is flushed once per pump, so entries further apart reach NVDA in
/// different pumps and are never capped against each other.
pub(crate) const PUMP_WINDOW_US: u64 = 10_000;

/// An MSAA tell's key: its kind, its object's ids and the thread that
/// raised it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct MsaaKey {
    /// The kind.
    pub(crate) kind: EventKind,
    /// The window.
    pub(crate) window: WindowHandle,
    /// The object id.
    pub(crate) object: i32,
    /// The child id.
    pub(crate) child: i32,
    /// The raising thread.
    pub(crate) thread: u32,
}

impl MsaaKey {
    /// Whether `other` names the same object from the same thread.
    fn same_object(&self, other: &Self) -> bool {
        (self.window, self.object, self.child, self.thread)
            == (other.window, other.object, other.child, other.thread)
    }
}

/// The part an MSAA tell plays in the limiter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MsaaClass {
    /// A focus: a run candidate.
    Focus,
    /// A foreground tell: a run candidate, also counted by its thread.
    Foreground,
    /// A menu start or end, popup start or end, or switch end: the menu
    /// slot.
    Menu,
    /// A show, which removes a waiting hide of its object.
    Show,
    /// A hide, which removes a waiting show of its object.
    Hide,
    /// Any other MSAA tell, counted by its thread.
    Other,
}

/// How an entry waits, as the logic places it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// An MSAA tell.
    Msaa {
        /// Its key.
        key: MsaaKey,
        /// Its part.
        class: MsaaClass,
        /// Whether it passes acceptance on its tell's facts: an accepted
        /// MSAA tell other than a run candidate or a menu tell ends a run.
        accepted: bool,
        /// When the reader received its tell, in microseconds on the run's
        /// shared clock: the caps count it only with entries that arrived
        /// within [`PUMP_WINDOW_US`] of the newest.
        at_us: u64,
    },
    /// Anything else: a UI Automation tell, a key, a command, an entry
    /// Core makes.
    Other {
        /// Whether a waiting entry the logic finds the same
        /// ([`Logic::merges`]) is replaced by it.
        merges: bool,
        /// Whether it is a focus-kind entry, past which no merge reaches.
        focus_kind: bool,
        /// Whether it ends a waiting run.
        ends_run: bool,
    },
}

/// A waiting item of the queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Item<E> {
    /// One entry.
    One(E),
    /// A run of MSAA focus and foreground candidates, oldest first.
    Run(Vec<E>),
    /// The menu slot's entry, and whether it was already moved behind the
    /// runs waiting behind it.
    Menu {
        /// The entry.
        entry: E,
        /// Whether it was moved once.
        moved: bool,
    },
}

/// The MSAA place of `entry`, `None` for an entry of another kind.
fn msaa<L: Logic>(facts: &L::Facts, entry: &L::Entry) -> Option<(MsaaKey, MsaaClass, bool)> {
    match L::place(facts, entry) {
        Place::Msaa {
            key,
            class,
            accepted,
            ..
        } => Some((key, class, accepted)),
        Place::Other { .. } => None,
    }
}

/// When the reader received `entry`'s tell, `None` for an entry of
/// another kind.
fn arrived<L: Logic>(facts: &L::Facts, entry: &L::Entry) -> Option<u64> {
    match L::place(facts, entry) {
        Place::Msaa { at_us, .. } => Some(at_us),
        Place::Other { .. } => None,
    }
}

/// Whether `item` is a focus-kind item, past which no merge reaches.
fn focus_kind<L: Logic>(facts: &L::Facts, item: &Item<L::Entry>) -> bool {
    match item {
        Item::Run(_) => true,
        Item::Menu { .. } => false,
        Item::One(entry) => matches!(
            L::place(facts, entry),
            Place::Other {
                focus_kind: true,
                ..
            }
        ),
    }
}

/// Whether `item` ends a run waiting before it.
fn ends_run<L: Logic>(facts: &L::Facts, item: &Item<L::Entry>) -> bool {
    match item {
        Item::Run(_) | Item::Menu { .. } => false,
        Item::One(entry) => match L::place(facts, entry) {
            Place::Msaa { accepted, .. } => accepted,
            Place::Other { ends_run, .. } => ends_run,
        },
    }
}

/// Places `entry` at the back of `waiting`.
pub(crate) fn arrive<L: Logic>(
    facts: &L::Facts,
    waiting: &mut VecDeque<Item<L::Entry>>,
    entry: L::Entry,
) {
    match L::place(facts, &entry) {
        Place::Other { merges, .. } => {
            let mut entry = entry;
            if merges
                && let Some(removed) =
                    merge_away::<L>(facts, waiting, |waiting| L::merges(facts, waiting, &entry))
            {
                L::merged(&removed, &mut entry);
            }
            waiting.push_back(Item::One(entry));
        }
        Place::Msaa { key, class, .. } => {
            match class {
                MsaaClass::Focus | MsaaClass::Foreground => {
                    join_run::<L>(facts, waiting, key, class, entry);
                }
                MsaaClass::Menu => {
                    waiting.retain(|item| !matches!(item, Item::Menu { .. }));
                    waiting.push_back(Item::Menu {
                        entry,
                        moved: false,
                    });
                }
                MsaaClass::Show | MsaaClass::Hide | MsaaClass::Other => {
                    if class != MsaaClass::Other {
                        let opposite = if class == MsaaClass::Show {
                            MsaaClass::Hide
                        } else {
                            MsaaClass::Show
                        };
                        waiting.retain(|item| match item {
                            Item::One(waiting) => {
                                !msaa::<L>(facts, waiting).is_some_and(|(other, other_class, _)| {
                                    other_class == opposite && other.same_object(&key)
                                })
                            }
                            Item::Run(_) | Item::Menu { .. } => true,
                        });
                    }
                    let mut entry = entry;
                    if let Some(removed) = merge_away::<L>(facts, waiting, |waiting| {
                        msaa::<L>(facts, waiting).is_some_and(|(other, _, _)| other == key)
                    }) {
                        L::merged(&removed, &mut entry);
                    }
                    waiting.push_back(Item::One(entry));
                }
            }
            limit::<L>(facts, waiting);
        }
    }
}

/// Removes the newest waiting single entry that `same` finds, searching
/// from the back and stopping at a focus-kind item, which is itself
/// reached: a focus tell merges a waiting one of its element, but nothing
/// merges past one. The entry removed, if any.
fn merge_away<L: Logic>(
    facts: &L::Facts,
    waiting: &mut VecDeque<Item<L::Entry>>,
    same: impl Fn(&L::Entry) -> bool,
) -> Option<L::Entry> {
    let mut found = None;
    for (place, item) in waiting.iter().enumerate().rev() {
        if let Item::One(entry) = item
            && same(entry)
        {
            found = Some(place);
            break;
        }
        if focus_kind::<L>(facts, item) {
            break;
        }
    }
    match found.and_then(|place| waiting.remove(place)) {
        Some(Item::One(entry)) => Some(entry),
        Some(Item::Run(_) | Item::Menu { .. }) | None => None,
    }
}

/// Joins a focus or foreground candidate to the run waiting with no
/// accepted MSAA tell after it, at the newer place, or starts a run. A
/// foreground tell first removes every waiting focus of its object from
/// its thread.
fn join_run<L: Logic>(
    facts: &L::Facts,
    waiting: &mut VecDeque<Item<L::Entry>>,
    key: MsaaKey,
    class: MsaaClass,
    entry: L::Entry,
) {
    let mut entry = entry;
    for item in waiting.iter_mut() {
        if let Item::Run(candidates) = item {
            let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(candidates)
                .into_iter()
                .partition(|candidate| {
                    msaa::<L>(facts, candidate).is_some_and(|(other, other_class, _)| {
                        other == key
                            || (class == MsaaClass::Foreground
                                && other_class == MsaaClass::Focus
                                && other.same_object(&key))
                    })
                });
            *candidates = kept;
            for removed in &removed {
                L::merged(removed, &mut entry);
            }
        }
    }
    waiting.retain(|item| !matches!(item, Item::Run(candidates) if candidates.is_empty()));
    let mut run = None;
    for (place, item) in waiting.iter().enumerate().rev() {
        if matches!(item, Item::Run(_)) {
            run = Some(place);
            break;
        }
        if ends_run::<L>(facts, item) {
            break;
        }
    }
    match run.and_then(|place| waiting.get_mut(place)) {
        Some(Item::Run(candidates)) => candidates.push(entry),
        Some(Item::One(_) | Item::Menu { .. }) | None => waiting.push_back(Item::Run(vec![entry])),
    }
}

/// An MSAA entry its thread's cap counts: its item's place, its place in
/// a run when it is a run's candidate, its key and its part.
type Counted = (usize, Option<usize>, MsaaKey, MsaaClass);

/// NVDA's caps on the waiting MSAA entries of one pump window: the newest
/// four run candidates kept, what a capped one carries handed to the
/// newest kept in its run ([`Logic::merged`]); the newest ten other MSAA entries of each
/// raising thread kept, foreground tells counted among them, entries on
/// the logic's exempt ids kept but counted. A foreground tell is dropped
/// only when neither cap keeps it, as NVDA keeps a copy in each cache. The
/// menu slot is outside both. Only entries that arrived within
/// [`PUMP_WINDOW_US`] of the newest waiting MSAA entry are counted or
/// dropped: older ones would have left NVDA's limiter in an earlier pump.
fn limit<L: Logic>(facts: &L::Facts, waiting: &mut VecDeque<Item<L::Entry>>) {
    let entries = waiting.iter().flat_map(|item| match item {
        Item::Run(run) => run.iter().collect::<Vec<_>>(),
        Item::One(entry) => vec![entry],
        Item::Menu { .. } => Vec::new(),
    });
    let Some(newest) = entries.filter_map(|entry| arrived::<L>(facts, entry)).max() else {
        return;
    };
    let in_window = |entry: &L::Entry| {
        arrived::<L>(facts, entry)
            .is_some_and(|at_us| at_us.saturating_add(PUMP_WINDOW_US) >= newest)
    };
    // Every MSAA entry waiting in the window, by its place: (item, place
    // in run).
    let mut candidates = Vec::new();
    let mut by_thread: BTreeMap<u32, Vec<Counted>> = BTreeMap::new();
    for (place, item) in waiting.iter().enumerate() {
        match item {
            Item::Run(run) => {
                for (inner, entry) in run.iter().enumerate() {
                    if !in_window(entry) {
                        continue;
                    }
                    if let Some((key, class, _)) = msaa::<L>(facts, entry) {
                        candidates.push((place, inner));
                        if class == MsaaClass::Foreground {
                            by_thread.entry(key.thread).or_default().push((
                                place,
                                Some(inner),
                                key,
                                class,
                            ));
                        }
                    }
                }
            }
            Item::One(entry) => {
                if !in_window(entry) {
                    continue;
                }
                if let Some((key, class, _)) = msaa::<L>(facts, entry) {
                    by_thread
                        .entry(key.thread)
                        .or_default()
                        .push((place, None, key, class));
                }
            }
            Item::Menu { .. } => {}
        }
    }
    let beyond_four: Vec<(usize, usize)> =
        candidates.iter().rev().skip(FOCUS_CAP).copied().collect();
    let mut beyond_ten = Vec::new();
    for entries in by_thread.values() {
        for (place, inner, key, class) in entries.iter().rev().skip(THREAD_CAP) {
            if !L::exempt_from_cap(facts, key) {
                beyond_ten.push((*place, *inner, *class));
            }
        }
    }
    let mut dropped: Vec<(usize, Option<usize>)> = beyond_ten
        .iter()
        .filter(|(place, inner, class)| match (class, inner) {
            (MsaaClass::Foreground, Some(inner)) => beyond_four.contains(&(*place, *inner)),
            _ => true,
        })
        .map(|(place, inner, _)| (*place, *inner))
        .collect();
    for (place, inner) in &beyond_four {
        let foreground_kept_by_its_thread = waiting
            .get(*place)
            .and_then(|item| match item {
                Item::Run(run) => run.get(*inner),
                Item::One(_) | Item::Menu { .. } => None,
            })
            .and_then(|entry| msaa::<L>(facts, entry))
            .is_some_and(|(_, class, _)| class == MsaaClass::Foreground)
            && !beyond_ten.contains(&(*place, Some(*inner), MsaaClass::Foreground));
        if !foreground_kept_by_its_thread {
            dropped.push((*place, Some(*inner)));
        }
    }
    dropped.sort_unstable();
    dropped.dedup();
    for (place, inner) in dropped.into_iter().rev() {
        match inner {
            Some(inner) => {
                if let Some(Item::Run(run)) = waiting.get_mut(place)
                    && inner < run.len()
                {
                    // What a capped candidate carries goes to the newest
                    // kept in its run, as a merge keeps it.
                    let removed = run.remove(inner);
                    if let Some(newest) = run.last_mut() {
                        L::merged(&removed, newest);
                    }
                }
            }
            None => {
                waiting.remove(place);
            }
        }
    }
    waiting.retain(|item| !matches!(item, Item::Run(run) if run.is_empty()));
}

/// Takes the next entry to run from the front of `waiting`: the menu
/// slot's entry is moved once behind the runs waiting behind it; a run
/// becomes the logic's run entry.
pub(crate) fn take<L: Logic>(waiting: &mut VecDeque<Item<L::Entry>>) -> Option<L::Entry> {
    loop {
        match waiting.pop_front()? {
            Item::One(entry) => return Some(entry),
            Item::Run(candidates) => return Some(L::run_entry(candidates)),
            Item::Menu { entry, moved } => {
                let last_run = waiting
                    .iter()
                    .rposition(|item| matches!(item, Item::Run(_)));
                match (moved, last_run) {
                    (false, Some(place)) => {
                        waiting.insert(place.saturating_add(1), Item::Menu { entry, moved: true });
                    }
                    (true, _) | (false, None) => return Some(entry),
                }
            }
        }
    }
}
