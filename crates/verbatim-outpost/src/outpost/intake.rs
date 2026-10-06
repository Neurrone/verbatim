//! The outpost's intake queue (outpost redesign, "Inside an outpost"): one
//! queue for the application, holding both events and Core's queries, with
//! NVDA's limiter rules (`orderedWinEventLimiter.py` and the UIA event
//! limiter in NVDA's source).
//!
//! - One waiting entry per object and event kind: a newer one replaces it and
//!   takes its place at the back.
//! - A batch is everything that accumulated while the worker handled the
//!   previous batch.
//! - Per batch, the newest 4 focus events and the newest 10 other events per
//!   application UI thread are kept; the focused object's events are always
//!   kept, and so are queries and housekeeping entries. A newer list of the
//!   nodes Core holds replaces a waiting one.
//! - Events from a window the system reports as hung are dropped before any
//!   read.
//! - Within a batch only the newest foreground change and the newest focus
//!   from each backend are handled (the worker's arbitration then drops the
//!   one whose backend does not own the window, as NVDA's separate MSAA and
//!   UIA limiters do), and the newest menu opening from each backend is
//!   handled last.
//!
//! Intake callbacks only push and return: they never call into the
//! application and never wait on the worker.

use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex, PoisonError};

use verbatim_ia2::WinEventKind;
use verbatim_model::{Notification, TraceId};
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::core::AgileReference;

use crate::protocol::{DeliveredFact, Query, UiaSnapshotFact};

/// How many focus events a batch keeps.
const FOCUS_EVENTS_PER_BATCH: usize = 4;

/// How many other events a batch keeps per application UI thread.
const EVENTS_PER_THREAD: usize = 10;

/// What a UIA event reports.
#[derive(Clone, Debug)]
pub(super) enum UiaKind {
    /// A property changed: name, value, or a state-bearing property.
    Property(i32),
    /// An element was selected within its container.
    Selection,
    /// An application-initiated notification.
    Notification(Notification),
}

/// A UIA event as its callback captured it: cached properties only, plus an
/// agile reference to the element for anything the worker must ask it.
pub(super) struct UiaEvent {
    pub(super) kind: UiaKind,
    pub(super) parts: UiaSnapshotFact,
    /// The element's cached window handle, 0 when it is not a window itself.
    pub(super) hwnd: isize,
    pub(super) element: Option<AgileReference<IUIAutomationElement>>,
}

/// One unit of work for the worker.
pub(super) enum Item {
    /// An MSAA `WinEvent` from this outpost's own hooks.
    Msaa {
        kind: WinEventKind,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
    },
    /// A UIA event from this outpost's own subscriptions.
    Uia(UiaEvent),
    /// A focus fact routed from the listener.
    Fact(DeliveredFact),
    /// A query from Core.
    Query { request_id: u64, query: Query },
    /// The nodes Core still holds, and the position of the last message it
    /// has handled: release the rest.
    NodesHeld { nodes: Vec<u64>, acknowledged: u64 },
    /// A follow-up finding the live element of a focus reported from its
    /// event alone, for the focus-following property subscription.
    ResolveFocus {
        runtime_id: Vec<i32>,
        attempt: u32,
        /// A focus held back because another element of the application
        /// had the keyboard focus when it was read: reported once the
        /// follow-up finds its element focused after all.
        held: Option<Box<HeldFocus>>,
    },
}

/// A UIA focus fact the worker held back as possibly stale
/// ([`Item::ResolveFocus`]), with what reporting it needs.
pub(super) struct HeldFocus {
    pub(super) windows: (isize, isize),
    pub(super) fact: crate::protocol::UiaSnapshotFact,
    pub(super) observed_at_ms: u64,
}

/// An item with its trace and observation time.
pub(super) struct Entry {
    pub(super) item: Item,
    pub(super) trace: TraceId,
    pub(super) observed_at_ms: u64,
    /// When it was raised, observed, and relayed, for the latency log.
    pub(super) timing: crate::protocol::EventTiming,
}

/// The object and kind an entry concerns, for the one-per-object rule and for
/// recognizing the focused object.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Key {
    Msaa(u8, isize, i32, i32),
    Uia(u8, i32, Vec<i32>),
    Foreground(isize),
    MsaaFocus(isize, i32, i32),
    UiaFocus(Vec<i32>),
    MenuPopup(isize, i32, i32),
    UiaMenuOpened(Vec<i32>),
    NodesHeld,
}

impl Key {
    /// The object part alone, to compare with the focused object whatever
    /// the event kind.
    fn object(&self) -> Option<Object> {
        match self {
            Key::Msaa(_, hwnd, object, child)
            | Key::MsaaFocus(hwnd, object, child)
            | Key::MenuPopup(hwnd, object, child) => Some(Object::Msaa(*hwnd, *object, *child)),
            Key::Uia(_, _, runtime_id)
            | Key::UiaFocus(runtime_id)
            | Key::UiaMenuOpened(runtime_id) => Some(Object::Uia(runtime_id.clone())),
            Key::Foreground(_) | Key::NodesHeld => None,
        }
    }
}

/// An accessible object, by its event address or runtime id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Object {
    Msaa(isize, i32, i32),
    Uia(Vec<i32>),
}

/// How the batch limits treat an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Category {
    /// A focus, foreground, or menu event.
    Focus,
    /// Any other event, limited per UI thread.
    Other,
    /// Always kept: queries and housekeeping.
    Exempt,
}

/// An entry with what the limiter needs to know about it, computed when it
/// is pushed.
struct Waiting {
    key: Option<Key>,
    category: Category,
    hwnd: isize,
    thread: u32,
    entry: Entry,
}

/// The order in which the worker handles a planned batch.
pub(super) enum Planned {
    /// Handle the entry.
    Run(Entry),
    /// Handle a menu opening, after the batch's focus events.
    Menu(Entry),
    /// Focus events from one backend, newest first: the worker handles each
    /// in turn until one is reported, as NVDA falls back to an older focus
    /// event when the newest cannot be processed.
    Focus(Vec<Entry>),
}

impl Planned {
    /// The entries, in the order the worker may handle them.
    pub(super) fn entries(&self) -> Vec<&Entry> {
        match self {
            Planned::Run(entry) | Planned::Menu(entry) => vec![entry],
            Planned::Focus(entries) => entries.iter().collect(),
        }
    }
}

/// How many of a batch's focus events from one backend are kept to fall
/// back on.
const FOCUS_CANDIDATES: usize = 3;

#[derive(Default)]
struct State {
    waiting: VecDeque<Waiting>,
    batch: VecDeque<Planned>,
    /// Numbers the batches, so the worker can tell whether a focus was
    /// reported within the batch a menu opening belongs to.
    batch_number: u64,
    focused: Option<Object>,
    closed: bool,
}

/// The queue the worker draws from.
#[derive(Default)]
pub(super) struct Intake {
    state: Mutex<State>,
    ready: Condvar,
}

impl Intake {
    /// Adds an entry. `thread_of` and the window are read here, with local
    /// calls, so the limiter never needs them later.
    pub(super) fn push(&self, entry: Entry) {
        let (key, category, hwnd) = classify(&entry.item);
        let thread = super::window::window_thread(hwnd);
        let mut state = self.lock();
        if let Some(key) = &key {
            state
                .waiting
                .retain(|waiting| waiting.key.as_ref() != Some(key));
        }
        state.waiting.push_back(Waiting {
            key,
            category,
            hwnd,
            thread,
            entry,
        });
        drop(state);
        self.ready.notify_one();
    }

    /// The next entry to handle and the number of its batch, planning a new
    /// batch from everything waiting when the current one is done. With the
    /// first entry of a batch that holds a foreground change comes that
    /// change's window, for the worker to wait on before handling the batch
    /// (see [`foreground_of`]). Blocks while there is nothing to do; `None`
    /// once the queue is closed.
    pub(super) fn next(&self) -> Option<(Planned, u64, Option<isize>)> {
        let mut state = self.lock();
        let mut foreground = None;
        loop {
            if let Some(planned) = state.batch.pop_front() {
                return Some((planned, state.batch_number, foreground));
            }
            if !state.waiting.is_empty() {
                let waiting: Vec<Waiting> = state.waiting.drain(..).collect();
                let focused = state.focused.clone();
                let batch = plan(waiting, focused.as_ref(), super::window::window_is_hung);
                foreground = foreground_of(&batch);
                state.batch = batch.into();
                state.batch_number += 1;
                continue;
            }
            if state.closed {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Withdraws query `request_id` if it has not started. Returns whether it
    /// was found.
    pub(super) fn cancel(&self, request_id: u64) -> bool {
        let is_it = |entry: &Entry| matches!(entry.item, Item::Query { request_id: id, .. } if id == request_id);
        let mut state = self.lock();
        let before = state.waiting.len() + state.batch.len();
        state.waiting.retain(|waiting| !is_it(&waiting.entry));
        state
            .batch
            .retain(|planned| !planned.entries().into_iter().any(is_it));
        before != state.waiting.len() + state.batch.len()
    }

    /// Records the object the worker last reported as the focus: its events
    /// are always kept.
    pub(super) fn set_focused(&self, object: Option<Object>) {
        self.lock().focused = object;
    }

    /// The object the worker last reported as the focus.
    pub(super) fn focused(&self) -> Option<Object> {
        self.lock().focused.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The key, category, and window of an item.
/// The window an item concerns, 0 for none.
pub(super) fn window_of(item: &Item) -> isize {
    classify(item).2
}

fn classify(item: &Item) -> (Option<Key>, Category, isize) {
    match item {
        Item::Msaa {
            kind,
            hwnd,
            id_object,
            id_child,
        } => match kind {
            WinEventKind::Destroy => (None, Category::Exempt, *hwnd),
            _ => (
                Some(Key::Msaa(*kind as u8, *hwnd, *id_object, *id_child)),
                Category::Other,
                *hwnd,
            ),
        },
        Item::Uia(event) => {
            let key = match &event.kind {
                UiaKind::Property(id) => Some(Key::Uia(0, *id, event.parts.runtime_id.clone())),
                UiaKind::Selection => Some(Key::Uia(1, 0, event.parts.runtime_id.clone())),
                // Each notification carries its own text, so none replaces
                // another.
                UiaKind::Notification(_) => None,
            };
            (key, Category::Other, event.hwnd)
        }
        Item::Fact(fact) => {
            let (key, hwnd) = match fact {
                // Selections, notifications, and alerts routed from the
                // listener are ordinary events, limited per UI thread.
                DeliveredFact::UiaSelection { hwnd, snapshot } => {
                    return (
                        Some(Key::Uia(1, 0, snapshot.runtime_id.clone())),
                        Category::Other,
                        *hwnd,
                    );
                }
                DeliveredFact::UiaNotification { hwnd, .. } => {
                    return (None, Category::Other, *hwnd);
                }
                DeliveredFact::Alert {
                    hwnd,
                    id_object,
                    id_child,
                } => {
                    return (
                        Some(Key::Msaa(
                            WinEventKind::Alert as u8,
                            *hwnd,
                            *id_object,
                            *id_child,
                        )),
                        Category::Other,
                        *hwnd,
                    );
                }
                DeliveredFact::UiaMenuOpened { hwnd, snapshot } => {
                    (Key::UiaMenuOpened(snapshot.runtime_id.clone()), *hwnd)
                }
                DeliveredFact::Foreground { hwnd } => (Key::Foreground(*hwnd), *hwnd),
                DeliveredFact::MsaaFocus {
                    hwnd,
                    id_object,
                    id_child,
                } => (Key::MsaaFocus(*hwnd, *id_object, *id_child), *hwnd),
                DeliveredFact::UiaFocus {
                    hwnd,
                    focus_window,
                    snapshot,
                } => (
                    Key::UiaFocus(snapshot.runtime_id.clone()),
                    if *hwnd == 0 { *focus_window } else { *hwnd },
                ),
                DeliveredFact::MenuPopup {
                    hwnd,
                    id_object,
                    id_child,
                } => (Key::MenuPopup(*hwnd, *id_object, *id_child), *hwnd),
            };
            (Some(key), Category::Focus, hwnd)
        }
        // Only the newest list of held nodes matters.
        Item::NodesHeld { .. } => (Some(Key::NodesHeld), Category::Exempt, 0),
        Item::Query { .. } | Item::ResolveFocus { .. } => (None, Category::Exempt, 0),
    }
}

/// Plans one batch from everything that was waiting, oldest first: drops
/// events from hung windows, applies the batch limits, keeps only the newest
/// foreground change and the newest focus from each backend, and moves the
/// newest menu opening to the end.
fn plan(
    waiting: Vec<Waiting>,
    focused: Option<&Object>,
    window_is_hung: impl Fn(isize) -> bool,
) -> Vec<Planned> {
    let mut hung: HashMap<isize, bool> = HashMap::new();
    let mut keep = vec![false; waiting.len()];
    let mut focus_kept = 0;
    let mut per_thread: HashMap<u32, usize> = HashMap::new();
    for (index, item) in waiting.iter().enumerate().rev() {
        if item.category != Category::Exempt
            && item.hwnd != 0
            && *hung
                .entry(item.hwnd)
                .or_insert_with(|| window_is_hung(item.hwnd))
        {
            continue;
        }
        let of_focus =
            focused.is_some() && item.key.as_ref().and_then(Key::object).as_ref() == focused;
        keep[index] = match item.category {
            Category::Exempt => true,
            _ if of_focus => true,
            Category::Focus => {
                focus_kept += 1;
                focus_kept <= FOCUS_EVENTS_PER_BATCH
            }
            Category::Other => {
                let count = per_thread.entry(item.thread).or_default();
                *count += 1;
                *count <= EVENTS_PER_THREAD
            }
        };
    }

    let kept: Vec<Waiting> = waiting
        .into_iter()
        .zip(keep)
        .filter_map(|(item, keep)| keep.then_some(item))
        .collect();
    let newest = |wanted: fn(&Key) -> bool| {
        kept.iter()
            .rposition(|item| item.key.as_ref().is_some_and(wanted))
    };
    let foreground = newest(|key| matches!(key, Key::Foreground(_)));
    let msaa_focus = newest(|key| matches!(key, Key::MsaaFocus(..)));
    let uia_focus = newest(|key| matches!(key, Key::UiaFocus(_)));
    // The newest few focus events from each backend, to fall back on.
    let candidates = |wanted: fn(&Key) -> bool| -> Vec<usize> {
        kept.iter()
            .enumerate()
            .rev()
            .filter(|(_, item)| item.key.as_ref().is_some_and(wanted))
            .map(|(index, _)| index)
            .take(FOCUS_CANDIDATES)
            .collect()
    };
    let msaa_candidates = candidates(|key| matches!(key, Key::MsaaFocus(..)));
    let uia_candidates = candidates(|key| matches!(key, Key::UiaFocus(_)));
    let msaa_menu = newest(|key| matches!(key, Key::MenuPopup(..)));
    let uia_menu = newest(|key| matches!(key, Key::UiaMenuOpened(_)));

    let mut planned = Vec::with_capacity(kept.len());
    let mut deferred = Vec::new();
    // Focus candidates are gathered newest first and placed where the
    // newest was.
    let mut slots: Vec<Option<Entry>> = kept.iter().map(|_| None).collect();
    let mut rest = Vec::with_capacity(kept.len());
    for (index, item) in kept.into_iter().enumerate() {
        if msaa_candidates.contains(&index) || uia_candidates.contains(&index) {
            slots[index] = Some(item.entry);
        } else {
            rest.push((index, item));
        }
    }
    let mut take_group = |indices: &[usize]| -> Vec<Entry> {
        indices
            .iter()
            .filter_map(|&index| slots[index].take())
            .collect()
    };
    let msaa_group = take_group(&msaa_candidates);
    let uia_group = take_group(&uia_candidates);
    let mut groups = vec![(msaa_focus, msaa_group), (uia_focus, uia_group)];
    for (index, item) in rest {
        for (newest, group) in &mut groups {
            if newest.is_some_and(|newest| newest < index) && !group.is_empty() {
                planned.push(Planned::Focus(std::mem::take(group)));
            }
        }
        match &item.key {
            Some(Key::Foreground(_)) if Some(index) != foreground => {}
            // Older than the candidates kept to fall back on.
            Some(Key::MsaaFocus(..) | Key::UiaFocus(_)) => {}
            Some(Key::MenuPopup(..) | Key::UiaMenuOpened(_)) => {
                // The newest from each backend, as for focus: the worker's
                // arbitration drops the one whose backend does not own the
                // window.
                if Some(index) == msaa_menu || Some(index) == uia_menu {
                    deferred.push(item.entry);
                }
            }
            _ => planned.push(Planned::Run(item.entry)),
        }
    }
    for (_, group) in groups {
        if !group.is_empty() {
            planned.push(Planned::Focus(group));
        }
    }
    planned.extend(deferred.into_iter().map(Planned::Menu));
    planned
}

/// The window of a batch's newest foreground change, if it has one. Windows
/// can raise a window's foreground event a little before the window is the
/// foreground window (NVDA issue 3831), and an application's focus event
/// can come before its foreground event; NVDA holds back all event handling
/// until the foreground window matches, so the batch's focus is judged
/// against the real foreground.
fn foreground_of(batch: &[Planned]) -> Option<isize> {
    batch.iter().rev().find_map(|planned| match planned {
        Planned::Run(Entry {
            item: Item::Fact(DeliveredFact::Foreground { hwnd }),
            ..
        }) => Some(*hwnd),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msaa(kind: WinEventKind, hwnd: isize, child: i32) -> Waiting {
        let item = Item::Msaa {
            kind,
            hwnd,
            id_object: -4,
            id_child: child,
        };
        let (key, category, hwnd) = classify(&item);
        Waiting {
            key,
            category,
            hwnd,
            thread: u32::try_from(hwnd).unwrap_or(0),
            entry: Entry {
                item,
                trace: TraceId::mint(),
                observed_at_ms: u64::try_from(child).unwrap_or(0),
                timing: crate::protocol::EventTiming::default(),
            },
        }
    }

    fn fact(fact: DeliveredFact, observed_at_ms: u64) -> Waiting {
        let item = Item::Fact(fact);
        let (key, category, hwnd) = classify(&item);
        Waiting {
            key,
            category,
            hwnd,
            thread: 1,
            entry: Entry {
                item,
                trace: TraceId::mint(),
                observed_at_ms,
                timing: crate::protocol::EventTiming::default(),
            },
        }
    }

    fn query(request_id: u64) -> Waiting {
        let item = Item::Query {
            request_id,
            query: Query::FocusNow,
        };
        let (key, category, hwnd) = classify(&item);
        Waiting {
            key,
            category,
            hwnd,
            thread: 0,
            entry: Entry {
                item,
                trace: TraceId::mint(),
                observed_at_ms: 0,
                timing: crate::protocol::EventTiming::default(),
            },
        }
    }

    fn observed(planned: &[Planned]) -> Vec<u64> {
        planned
            .iter()
            .flat_map(|planned| {
                planned
                    .entries()
                    .into_iter()
                    .map(|entry| entry.observed_at_ms)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn never_hung(_: isize) -> bool {
        false
    }

    #[test]
    fn a_batch_keeps_the_newest_ten_events_per_thread() {
        let mut waiting: Vec<Waiting> = (1..=12)
            .map(|child| msaa(WinEventKind::ValueChange, 7, child))
            .collect();
        waiting.push(msaa(WinEventKind::ValueChange, 8, 50));
        let planned = plan(waiting, None, never_hung);
        assert_eq!(
            observed(&planned),
            vec![3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 50]
        );
    }

    #[test]
    fn the_focused_objects_events_and_queries_are_always_kept() {
        let mut waiting = vec![msaa(WinEventKind::NameChange, 7, 1), query(1)];
        waiting.extend((2..=12).map(|child| msaa(WinEventKind::ValueChange, 7, child)));
        let focused = Object::Msaa(7, -4, 1);
        let planned = plan(waiting, Some(&focused), never_hung);
        let kept = observed(&planned);
        assert_eq!(kept[0], 1, "the focused object's event survives the limit");
        assert_eq!(kept[1], 0, "the query survives");
        assert!(!kept.contains(&2), "the oldest other event is dropped");
    }

    #[test]
    fn events_from_a_hung_window_are_dropped_but_queries_are_not() {
        let waiting = vec![msaa(WinEventKind::ValueChange, 9, 1), query(1)];
        let planned = plan(waiting, None, |hwnd| hwnd == 9);
        assert_eq!(observed(&planned), vec![0]);
    }

    #[test]
    fn only_the_newest_focus_from_each_backend_is_handled_and_a_menu_opening_goes_last() {
        let waiting = vec![
            fact(
                DeliveredFact::MenuPopup {
                    hwnd: 3,
                    id_object: -4,
                    id_child: 0,
                },
                1,
            ),
            fact(DeliveredFact::Foreground { hwnd: 2 }, 2),
            fact(
                DeliveredFact::MsaaFocus {
                    hwnd: 3,
                    id_object: -4,
                    id_child: 2,
                },
                4,
            ),
        ];
        let mut waiting = waiting;
        waiting.push(fact(
            DeliveredFact::UiaFocus {
                hwnd: 3,
                focus_window: 0,
                snapshot: UiaSnapshotFact {
                    runtime_id: vec![42],
                    role: verbatim_model::Role::ListItem,
                    name: None,
                    value: None,
                    states: verbatim_model::StateSet::new(),
                    details: verbatim_model::NodeDetails::default(),
                },
            },
            5,
        ));
        let planned = plan(waiting, None, never_hung);
        assert_eq!(
            observed(&planned),
            vec![2, 4, 5, 1],
            "the newest MSAA and UIA focus both survive, so the backend that owns the window reports"
        );
        assert!(matches!(planned.last(), Some(Planned::Menu(_))));
    }

    #[test]
    fn a_newer_event_for_the_same_object_replaces_the_waiting_one() {
        let intake = Intake::default();
        let push = |child: i32, observed_at_ms: u64| {
            intake.push(Entry {
                item: Item::Msaa {
                    kind: WinEventKind::ValueChange,
                    hwnd: 0,
                    id_object: -4,
                    id_child: child,
                },
                trace: TraceId::mint(),
                observed_at_ms,
                timing: crate::protocol::EventTiming::default(),
            });
        };
        push(1, 10);
        push(2, 20);
        push(1, 30);
        let mut order = Vec::new();
        for _ in 0..2 {
            if let Some((Planned::Run(entry), _, _)) = intake.next() {
                order.push(entry.observed_at_ms);
            }
        }
        assert_eq!(order, vec![20, 30]);
    }

    #[test]
    fn a_newer_list_of_held_nodes_replaces_the_waiting_one_and_is_never_limited() {
        let intake = Intake::default();
        for acknowledged in [1, 2] {
            intake.push(Entry {
                item: Item::NodesHeld {
                    nodes: Vec::new(),
                    acknowledged,
                },
                trace: TraceId::mint(),
                observed_at_ms: 0,
                timing: crate::protocol::EventTiming::default(),
            });
        }
        let Some((Planned::Run(entry), _, _)) = intake.next() else {
            panic!("the list is planned");
        };
        assert!(matches!(
            entry.item,
            Item::NodesHeld {
                acknowledged: 2,
                ..
            }
        ));
        let (_, category, _) = classify(&entry.item);
        assert_eq!(category, Category::Exempt);
    }

    #[test]
    fn a_batch_holding_a_foreground_change_names_its_window_with_its_first_entry() {
        // msinfo32 raises its focus event just before its foreground event,
        // and the worker must wait for the window before handling either.
        let intake = Intake::default();
        let push = |fact| {
            intake.push(Entry {
                item: Item::Fact(fact),
                trace: TraceId::mint(),
                observed_at_ms: 0,
                timing: crate::protocol::EventTiming::default(),
            });
        };
        push(DeliveredFact::MsaaFocus {
            hwnd: 78,
            id_object: -4,
            id_child: 1,
        });
        push(DeliveredFact::Foreground { hwnd: 77 });
        let first = intake.next().expect("an entry");
        assert_eq!(first.2, Some(77));
        let second = intake.next().expect("an entry");
        assert_eq!(second.2, None, "only the batch's first entry carries it");
        assert_eq!(first.1, second.1, "one batch");

        push(DeliveredFact::MsaaFocus {
            hwnd: 78,
            id_object: -4,
            id_child: 2,
        });
        assert_eq!(intake.next().expect("an entry").2, None);
    }

    #[test]
    fn a_query_that_has_not_started_can_be_cancelled() {
        let intake = Intake::default();
        intake.push(Entry {
            item: Item::Query {
                request_id: 5,
                query: Query::DumpTree,
            },
            trace: TraceId::mint(),
            observed_at_ms: 0,
            timing: crate::protocol::EventTiming::default(),
        });
        assert!(intake.cancel(5));
        assert!(!intake.cancel(5), "already withdrawn");
    }

    #[test]
    fn the_newest_focus_events_are_kept_newest_first_to_fall_back_on() {
        let focus = |child: i32, at: u64| {
            fact(
                DeliveredFact::MsaaFocus {
                    hwnd: 9,
                    id_object: -4,
                    id_child: child,
                },
                at,
            )
        };
        let waiting = vec![
            focus(1, 1),
            focus(2, 2),
            msaa(WinEventKind::NameChange, 3, 30),
            focus(3, 4),
            focus(4, 5),
            msaa(WinEventKind::NameChange, 3, 60),
        ];
        let planned = plan(waiting, None, never_hung);
        let focus_groups: Vec<Vec<u64>> = planned
            .iter()
            .filter_map(|planned| match planned {
                Planned::Focus(entries) => {
                    Some(entries.iter().map(|entry| entry.observed_at_ms).collect())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            focus_groups,
            vec![vec![5, 4, 2]],
            "the three newest, newest first; the oldest is dropped"
        );
        assert_eq!(
            observed(&planned),
            vec![30, 5, 4, 2, 60],
            "the group takes the newest focus's place"
        );
    }
}
