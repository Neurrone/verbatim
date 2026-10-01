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
//!   kept, and so are queries and housekeeping entries.
//! - Events from a window the system reports as hung are dropped before any
//!   read.
//! - Within a batch only the newest foreground change and the newest focus
//!   from each backend are handled (the worker's arbitration then drops the
//!   one whose backend does not own the window, as NVDA's separate MSAA and
//!   UIA limiters do), and the newest menu opening is handled last.
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
    /// Read the real focus: a menu closed and no focus event may follow.
    CheckFocus,
    /// A query from Core.
    Query { request_id: u64, query: Query },
}

/// An item with its trace and observation time.
pub(super) struct Entry {
    pub(super) item: Item,
    pub(super) trace: TraceId,
    pub(super) observed_at_ms: u64,
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
    CheckFocus,
}

impl Key {
    /// The object part alone, to compare with the focused object whatever
    /// the event kind.
    fn object(&self) -> Option<Object> {
        match self {
            Key::Msaa(_, hwnd, object, child)
            | Key::MsaaFocus(hwnd, object, child)
            | Key::MenuPopup(hwnd, object, child) => Some(Object::Msaa(*hwnd, *object, *child)),
            Key::Uia(_, _, runtime_id) | Key::UiaFocus(runtime_id) => {
                Some(Object::Uia(runtime_id.clone()))
            }
            Key::Foreground(_) | Key::CheckFocus => None,
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
}

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
    /// batch from everything waiting when the current one is done. Blocks
    /// while there is nothing to do; `None` once the queue is closed.
    pub(super) fn next(&self) -> Option<(Planned, u64)> {
        let mut state = self.lock();
        loop {
            if let Some(planned) = state.batch.pop_front() {
                return Some((planned, state.batch_number));
            }
            if !state.waiting.is_empty() {
                let waiting: Vec<Waiting> = state.waiting.drain(..).collect();
                let focused = state.focused.clone();
                state.batch = plan(waiting, focused.as_ref(), super::window::window_is_hung).into();
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
        state.batch.retain(|planned| match planned {
            Planned::Run(entry) | Planned::Menu(entry) => !is_it(entry),
        });
        before != state.waiting.len() + state.batch.len()
    }

    /// Records the object the worker last reported as the focus: its events
    /// are always kept.
    pub(super) fn set_focused(&self, object: Option<Object>) {
        self.lock().focused = object;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The key, category, and window of an item.
fn classify(item: &Item) -> (Option<Key>, Category, isize) {
    match item {
        Item::Msaa {
            kind,
            hwnd,
            id_object,
            id_child,
        } => match kind {
            WinEventKind::MenuEnd | WinEventKind::Destroy => (None, Category::Exempt, *hwnd),
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
                DeliveredFact::Foreground { hwnd } => (Key::Foreground(*hwnd), *hwnd),
                DeliveredFact::MsaaFocus {
                    hwnd,
                    id_object,
                    id_child,
                } => (Key::MsaaFocus(*hwnd, *id_object, *id_child), *hwnd),
                DeliveredFact::UiaFocus { hwnd, snapshot } => {
                    (Key::UiaFocus(snapshot.runtime_id.clone()), *hwnd)
                }
                DeliveredFact::MenuPopup {
                    hwnd,
                    id_object,
                    id_child,
                } => (Key::MenuPopup(*hwnd, *id_object, *id_child), *hwnd),
            };
            (Some(key), Category::Focus, hwnd)
        }
        Item::CheckFocus => (Some(Key::CheckFocus), Category::Exempt, 0),
        Item::Query { .. } => (None, Category::Exempt, 0),
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
    let menu = newest(|key| matches!(key, Key::MenuPopup(..)));

    let mut planned = Vec::with_capacity(kept.len());
    let mut deferred = None;
    for (index, item) in kept.into_iter().enumerate() {
        match &item.key {
            Some(Key::Foreground(_)) if Some(index) != foreground => {}
            Some(Key::MsaaFocus(..)) if Some(index) != msaa_focus => {}
            Some(Key::UiaFocus(_)) if Some(index) != uia_focus => {}
            Some(Key::MenuPopup(..)) => {
                if Some(index) == menu {
                    deferred = Some(item.entry);
                }
            }
            _ => planned.push(Planned::Run(item.entry)),
        }
    }
    if let Some(entry) = deferred {
        planned.push(Planned::Menu(entry));
    }
    planned
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
            },
        }
    }

    fn observed(planned: &[Planned]) -> Vec<u64> {
        planned
            .iter()
            .map(|planned| match planned {
                Planned::Run(entry) | Planned::Menu(entry) => entry.observed_at_ms,
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
            });
        };
        push(1, 10);
        push(2, 20);
        push(1, 30);
        let mut order = Vec::new();
        for _ in 0..2 {
            if let Some((Planned::Run(entry), _)) = intake.next() {
                order.push(entry.observed_at_ms);
            }
        }
        assert_eq!(order, vec![20, 30]);
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
        });
        assert!(intake.cancel(5));
        assert!(!intake.cancel(5), "already withdrawn");
    }
}
