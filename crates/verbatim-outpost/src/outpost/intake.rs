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
//! - A focus change is never kept waiting behind reads of other objects
//!   ([`overtaken`]): within a batch that holds a foreground change or a
//!   focus, the events of objects that are neither the focus nor the
//!   object the change moves to are handled after it, menus included, in
//!   their own order; and a focus change that arrives while the worker is
//!   in the middle of a batch takes such events of that batch that have not
//!   started back into the queue, where the next batch puts them after it.
//!   A state change on one of the focus's ancestors, which NVDA would judge
//!   against the focus it was observed under, is not overtaken: observed
//!   before a batch's focus change, it is handled before it, whether it
//!   came first in the batch, in an earlier batch, or was pushed after the
//!   change though observed before it ([`State::judged`]).
//! - A batch that holds a foreground change is held until that change's
//!   window is the foreground window, for 250 ms at most, as NVDA holds
//!   back event handling after a foreground event; Core's queries and its
//!   list of held nodes are handed out ahead of it meanwhile, so the worker
//!   never waits for the foreground.
//!
//! Intake callbacks only push and return: they never call into the
//! application and never wait on the worker.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use verbatim_ia2::{CHILDID_SELF, WinEventKind};
use verbatim_model::{Notification, TraceId};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextRange};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};
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
    /// A text focus's caret or selection changed (`Text_TextSelectionChanged`).
    TextSelection,
    /// A text focus's text changed (`Text_TextChanged`).
    TextChanged,
    /// The active position in a text focus's text changed, to the start of
    /// this range when the event carried one.
    ActiveTextPosition(Option<ActiveRange>),
}

/// The range an active text position change carried.
#[derive(Clone)]
pub(super) struct ActiveRange(pub(super) AgileReference<IUIAutomationTextRange>);

impl std::fmt::Debug for ActiveRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ActiveRange")
    }
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
    /// Report the caret of the focus `node_id` names, which has text or
    /// whose role says it may, just after it was reported: as `CaretMoved`,
    /// or as `NoText` when there is no caret to report.
    CaretOf { node_id: verbatim_model::NodeId },
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
    /// [`Outpost::settle`](super::Outpost::settle): answered once nothing
    /// else is waiting, the focus-following subscriptions have made every
    /// move asked of them, and every message published has been written.
    Settle(std::sync::mpsc::Sender<()>),
    /// The time [`Intake::wake_at`] named has come with nothing else to do:
    /// a caret key's watch may have reached the bound on its age.
    Wake,
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
    CaretOf(u64),
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
            Key::Foreground(_) | Key::NodesHeld | Key::CaretOf(_) => None,
        }
    }

    /// Whether this is a focus change: a foreground change or a focus, the
    /// entries that overtake reads of other objects ([`overtaken`]).
    fn is_focus_change(&self) -> bool {
        matches!(
            self,
            Key::Foreground(_) | Key::MsaaFocus(..) | Key::UiaFocus(_)
        )
    }

    /// The objects a focus change moves to: the focus's own, or a
    /// foreground window's window and client objects, which the foreground
    /// report reads.
    fn moves_to(&self) -> Vec<Object> {
        match self {
            Key::Foreground(hwnd) => vec![
                Object::Msaa(*hwnd, OBJID_WINDOW.0, CHILDID_SELF),
                Object::Msaa(*hwnd, OBJID_CLIENT.0, CHILDID_SELF),
            ],
            Key::MsaaFocus(..) | Key::UiaFocus(_) => self.object().into_iter().collect(),
            _ => Vec::new(),
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
    /// An event an earlier batch admitted and a focus change took back into
    /// the queue before it was handled ([`overtaken`]): the limits, which
    /// it has already passed, keep it.
    admitted: bool,
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

/// The longest a batch holding a foreground change is held, for that
/// change's window to become the foreground window. Measured live on
/// 2026-10-02 over 245 such events (msinfo32, Notepad, and Verbatim's own
/// windows): 5 to 100 ms, median 44 ms. A window that has not arrived by
/// then was refused the foreground, and the batch is handed out anyway.
pub(super) const FOREGROUND_WAIT: Duration = Duration::from_millis(250);

/// How often a held batch's window is checked, with a local call, while
/// nothing else comes: no event says when a window has become the
/// foreground window, since Windows raises the foreground event before.
const FOREGROUND_POLL: Duration = Duration::from_millis(10);

/// What became of a batch's foreground change while the batch was held,
/// told with the batch's first entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Foreground {
    /// Its window became the foreground window at this time, in
    /// milliseconds since the Unix epoch.
    Confirmed(u64),
    /// It had not within [`FOREGROUND_WAIT`].
    NotConfirmed,
}

/// A planned batch held until its foreground change's window is the
/// foreground window.
struct Hold {
    hwnd: isize,
    until: Instant,
}

#[derive(Default)]
struct State {
    waiting: VecDeque<Waiting>,
    batch: VecDeque<Planned>,
    /// Numbers the batches, so the worker can tell whether a focus was
    /// reported within the batch a menu opening belongs to.
    batch_number: u64,
    focused: Option<Object>,
    /// The focus's ancestors, by their own addresses, whose state changes
    /// are judged against the focus and may be spoken. NVDA handles events
    /// in the order they came, so such a change observed before a focus
    /// change is spoken, or not, by the focus it was observed under, and is
    /// then cut off by the focus change's own cancelling, or not: a change
    /// observed before the focus change is handled before it, so Core,
    /// whose cancelling is NVDA's, gives NVDA's outcome however the events
    /// were batched.
    judged: Vec<Object>,
    /// Whether [`State::focused`] was reached by redirecting a control's
    /// own focus event to its focused child, whose own event is still to
    /// come.
    focus_redirected: bool,
    closed: bool,
    /// When the worker asked to be given [`Item::Wake`] if nothing else
    /// comes first.
    wake: Option<Instant>,
    /// The planned batch held for its foreground change, if it is.
    hold: Option<Hold>,
}

/// The queue the worker draws from.
#[derive(Default)]
pub(super) struct Intake {
    state: Mutex<State>,
    ready: Condvar,
}

impl State {
    /// Takes the oldest entry that does not wait for a held batch's
    /// foreground change: a query from Core, or the list of nodes Core
    /// holds, from the held batch first and then from what is waiting
    /// behind it. Events, focus changes above all, are judged against the
    /// foreground and wait; a query answers Core about the application as
    /// it is.
    fn take_unheld(&mut self) -> Option<Entry> {
        let unheld = |item: &Item| matches!(item, Item::Query { .. } | Item::NodesHeld { .. });
        if let Some(index) = self
            .batch
            .iter()
            .position(|planned| matches!(planned, Planned::Run(entry) if unheld(&entry.item)))
        {
            return match self.batch.remove(index) {
                Some(Planned::Run(entry)) => Some(entry),
                _ => None,
            };
        }
        let index = self
            .waiting
            .iter()
            .position(|waiting| unheld(&waiting.entry.item))?;
        self.waiting.remove(index).map(|waiting| waiting.entry)
    }

    /// The wake [`Intake::wake_at`] asked for, when its time has come.
    fn wake_due(&mut self, now: Instant) -> Option<Entry> {
        if self.wake.is_none_or(|wake| now < wake) {
            return None;
        }
        self.wake = None;
        Some(Entry {
            item: Item::Wake,
            trace: TraceId::mint(),
            observed_at_ms: super::now_ms(),
            timing: crate::protocol::EventTiming::default(),
        })
    }

    /// Takes the events of the batch in progress that have not started and
    /// that a focus change overtakes back into the queue, ahead of what is
    /// waiting, in their order, so the next batch puts them after the
    /// focus change ([`overtaken`]). One that a newer entry for the same
    /// object and kind already replaces is dropped.
    fn take_back_overtaken(&mut self) {
        let focused = self.focused.clone();
        let judged = self.judged.clone();
        let mut kept = VecDeque::with_capacity(self.batch.len());
        let mut taken = Vec::new();
        for planned in self.batch.drain(..) {
            let Planned::Run(entry) = planned else {
                kept.push_back(planned);
                continue;
            };
            let (key, category, hwnd) = classify(&entry.item);
            if overtaken(key.as_ref(), category, (focused.as_ref(), &judged), &[]) {
                taken.push(Waiting {
                    key,
                    category,
                    hwnd,
                    thread: super::window::window_thread(hwnd),
                    admitted: true,
                    entry,
                });
            } else {
                kept.push_back(Planned::Run(entry));
            }
        }
        self.batch = kept;
        for waiting in taken.into_iter().rev() {
            let replaced = waiting.key.as_ref().is_some_and(|key| {
                self.waiting
                    .iter()
                    .any(|newer| newer.key.as_ref() == Some(key))
            });
            if !replaced {
                self.waiting.push_front(waiting);
            }
        }
    }
}

/// Whether an entry is an event a focus change overtakes: an event, not a
/// notification or an alert, of an object that is neither the focus
/// (`focused`) nor one the change moves to (`moving_to`), and not a state
/// change on one of the focus's ancestors that is judged against it
/// (`judged`, [`State::judged`]).
///
/// NVDA queues a UIA focus event the moment its UIA thread receives it,
/// and reads an MSAA event of another object with one call (the object
/// from the event), judging what it says against the focus when it runs: a
/// state change speaks only for the focus. The outpost reads each event's
/// whole snapshot, up to ten calls, and an application building a window
/// answers each slowly: File Explorer's first focus in a new window waited
/// 2.6 seconds behind such reads (`docs/performance.md`). So a focus change
/// goes first. The focus's own events, and those of the object focus moves
/// to, keep their place: they can change what the focus says. So do
/// notifications and alerts, which are spoken whatever the focus.
fn overtaken(
    key: Option<&Key>,
    category: Category,
    (focused, judged): (Option<&Object>, &[Object]),
    moving_to: &[Object],
) -> bool {
    if key.is_some_and(|key| is_judged(key, judged)) {
        return false;
    }
    let event = match key {
        Some(Key::Msaa(kind, ..)) => *kind != WinEventKind::Alert as u8,
        Some(Key::Uia(..)) => true,
        _ => false,
    };
    category == Category::Other
        && event
        && key
            .and_then(Key::object)
            .is_some_and(|object| Some(&object) != focused && !moving_to.contains(&object))
}

/// Whether `key` is a state change on one of the focus's ancestors judged
/// against the focus ([`State::judged`]).
fn is_judged(key: &Key, judged: &[Object]) -> bool {
    matches!(key, Key::Msaa(kind, ..) if *kind == WinEventKind::StateChange as u8)
        && key.object().is_some_and(|object| judged.contains(&object))
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
        if key.as_ref().is_some_and(Key::is_focus_change) {
            state.take_back_overtaken();
        }
        state.waiting.push_back(Waiting {
            key,
            category,
            hwnd,
            thread,
            admitted: false,
            entry,
        });
        drop(state);
        self.ready.notify_one();
    }

    /// The next entry to handle and the number of its batch, planning a new
    /// batch from everything waiting when the current one is done. Blocks
    /// while there is nothing to do, and gives [`Item::Wake`] when the time
    /// [`wake_at`](Self::wake_at) named comes first; `None` once the queue
    /// is closed.
    ///
    /// A batch that holds a foreground change is held until that change's
    /// window is the foreground window, or for [`FOREGROUND_WAIT`] at most,
    /// as NVDA holds back event handling after a foreground event until the
    /// foreground window matches (`_shouldGetEvents`, issue 3831; see
    /// [`foreground_of`]); what became of it comes with the batch's first
    /// entry. While it is held, queries and the list of nodes Core holds
    /// are handed out ahead of it, so they never wait for the foreground.
    pub(super) fn next(&self) -> Option<(Planned, u64, Option<Foreground>)> {
        self.next_with(&super::window::window_is_foreground, FOREGROUND_WAIT)
    }

    /// [`next`](Self::next), with the foreground read through
    /// `is_foreground` and batches held for `foreground_wait` at most.
    fn next_with(
        &self,
        is_foreground: &dyn Fn(isize) -> bool,
        foreground_wait: Duration,
    ) -> Option<(Planned, u64, Option<Foreground>)> {
        let mut state = self.lock();
        let mut foreground = None;
        loop {
            if let Some(hold) = &state.hold {
                let (hwnd, until) = (hold.hwnd, hold.until);
                let now = Instant::now();
                if is_foreground(hwnd) {
                    state.hold = None;
                    foreground = Some(Foreground::Confirmed(super::now_ms()));
                } else if now >= until {
                    tracing::debug!(
                        hwnd,
                        "the foreground window did not become the event's window"
                    );
                    state.hold = None;
                    foreground = Some(Foreground::NotConfirmed);
                } else if let Some(entry) = state.take_unheld().or_else(|| state.wake_due(now)) {
                    return Some((Planned::Run(entry), state.batch_number, None));
                } else {
                    let poll = FOREGROUND_POLL.min(until - now);
                    state = self
                        .ready
                        .wait_timeout(state, poll)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                    continue;
                }
            }
            if let Some(planned) = state.batch.pop_front() {
                return Some((planned, state.batch_number, foreground));
            }
            if !state.waiting.is_empty() {
                let waiting: Vec<Waiting> = state.waiting.drain(..).collect();
                let focused = state.focused.clone();
                let judged = state.judged.clone();
                let batch = plan(
                    waiting,
                    (focused.as_ref(), &judged),
                    super::window::window_is_hung,
                );
                state.hold = foreground_of(&batch).map(|hwnd| Hold {
                    hwnd,
                    until: Instant::now() + foreground_wait,
                });
                state.batch = batch.into();
                state.batch_number += 1;
                continue;
            }
            if state.closed {
                return None;
            }
            let now = Instant::now();
            if let Some(entry) = state.wake_due(now) {
                return Some((Planned::Run(entry), state.batch_number, None));
            }
            state = match state.wake {
                Some(wake) => {
                    self.ready
                        .wait_timeout(state, wake - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self
                    .ready
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
    }

    /// Asks [`next`](Self::next) for an [`Item::Wake`] at `at` if nothing
    /// else is to be done then, or for none. The worker waits for nothing:
    /// the queue's own wait for work ends at that time as well.
    pub(super) fn wake_at(&self, at: Option<Instant>) {
        self.lock().wake = at;
        self.ready.notify_one();
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

    /// Whether an MSAA focus in window `hwnd` is waiting for a later batch:
    /// one that arrived while an older focus there was being read.
    pub(super) fn msaa_focus_waiting(&self, hwnd: isize) -> bool {
        self.lock().waiting.iter().any(
            |waiting| matches!(waiting.key, Some(Key::MsaaFocus(window, _, _)) if window == hwnd),
        )
    }

    /// Whether anything is waiting to be handled, planned or not.
    pub(super) fn busy(&self) -> bool {
        let state = self.lock();
        !state.waiting.is_empty() || !state.batch.is_empty()
    }

    /// Records the object the worker last reported as the focus: its events
    /// are always kept.
    pub(super) fn set_focused(&self, object: Option<Object>) {
        let mut state = self.lock();
        state.focused = object;
        state.focus_redirected = false;
    }

    /// Records the focus's ancestors whose state changes are judged against
    /// it ([`State::judged`]), by their own addresses.
    pub(super) fn set_judged(&self, ancestors: Vec<Object>) {
        self.lock().judged = ancestors;
    }

    /// Records that the focus last reported was reached by redirecting a
    /// control's own focus event to its focused child.
    pub(super) fn set_focus_redirected(&self) {
        self.lock().focus_redirected = true;
    }

    /// Whether a focus event on `object` is the redirected-to child's own,
    /// arriving after its control's: `object` is the focus, reached by a
    /// redirect, and no other focus came between. Answers once.
    pub(super) fn take_redirected_focus(&self, object: &Object) -> bool {
        let mut state = self.lock();
        let repeated = state.focus_redirected && state.focused.as_ref() == Some(object);
        if repeated {
            state.focus_redirected = false;
        }
        repeated
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
                UiaKind::TextSelection => Some(Key::Uia(2, 0, event.parts.runtime_id.clone())),
                UiaKind::TextChanged => Some(Key::Uia(3, 0, event.parts.runtime_id.clone())),
                // Only the newest position matters, as NVDA's limiter keeps
                // one event per element and kind.
                UiaKind::ActiveTextPosition(_) => {
                    Some(Key::Uia(4, 0, event.parts.runtime_id.clone()))
                }
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
                }
                | DeliveredFact::Show {
                    hwnd,
                    id_object,
                    id_child,
                } => {
                    let kind = if matches!(fact, DeliveredFact::Show { .. }) {
                        WinEventKind::Show
                    } else {
                        WinEventKind::Alert
                    };
                    let key = Key::Msaa(kind as u8, *hwnd, *id_object, *id_child);
                    return (Some(key), Category::Other, *hwnd);
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
        // Only the newest caret report for a node matters, and it is never
        // limited: the focus's caret.
        Item::CaretOf { node_id } => (Some(Key::CaretOf(node_id.number())), Category::Exempt, 0),
        Item::Query { .. } | Item::ResolveFocus { .. } | Item::Settle(_) | Item::Wake => {
            (None, Category::Exempt, 0)
        }
    }
}

/// What the batch limits keep of everything that was waiting, oldest
/// first: events from hung windows are dropped, and so are all but the
/// newest focus events and the newest other events of each UI thread.
fn admit(
    waiting: Vec<Waiting>,
    focused: Option<&Object>,
    window_is_hung: impl Fn(isize) -> bool,
) -> Vec<Waiting> {
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
            _ if item.admitted => true,
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

    waiting
        .into_iter()
        .zip(keep)
        .filter_map(|(item, keep)| keep.then_some(item))
        .collect()
}

/// Plans one batch from everything that was waiting, oldest first: drops
/// events from hung windows, applies the batch limits, keeps only the newest
/// foreground change and the newest focus from each backend, moves the
/// newest menu opening after them, and, when the batch changes the focus,
/// the events it overtakes after all of those ([`overtaken`]). A state
/// change on an ancestor judged against the focus (`judged`) that was
/// observed before the batch's first focus change, or in the same
/// millisecond, goes before that change even when it was pushed after it.
fn plan(
    waiting: Vec<Waiting>,
    (focused, judged): (Option<&Object>, &[Object]),
    window_is_hung: impl Fn(isize) -> bool,
) -> Vec<Planned> {
    let kept = admit(waiting, focused, window_is_hung);
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
    // What the batch's focus changes move to, when it holds any: the
    // events of other objects are handled after them.
    let changes: Vec<&Key> = kept
        .iter()
        .filter_map(|item| item.key.as_ref())
        .filter(|key| key.is_focus_change())
        .collect();
    let moving_to: Vec<Object> = changes.iter().flat_map(|key| key.moves_to()).collect();
    let focus_changes = !changes.is_empty();
    // When the batch's first focus change was observed, where known (a
    // fact relayed without its time has 0).
    let change_observed = kept
        .iter()
        .filter(|item| item.key.as_ref().is_some_and(Key::is_focus_change))
        .map(|item| item.entry.observed_at_ms)
        .filter(|&observed| observed != 0)
        .min();

    let mut planned = Vec::with_capacity(kept.len());
    let mut deferred = Vec::new();
    let mut after_focus = Vec::new();
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
        let observed_before = change_observed
            .is_some_and(|change| item.entry.observed_at_ms <= change)
            && item.key.as_ref().is_some_and(|key| is_judged(key, judged));
        if focus_changes && observed_before {
            planned.push(Planned::Run(item.entry));
            continue;
        }
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
            _ if focus_changes
                && overtaken(
                    item.key.as_ref(),
                    item.category,
                    (focused, judged),
                    &moving_to,
                ) =>
            {
                after_focus.push(item.entry);
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
    planned.extend(after_focus.into_iter().map(Planned::Run));
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
            admitted: false,
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
            admitted: false,
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
            admitted: false,
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
        let planned = plan(waiting, (None, &[]), never_hung);
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
        let planned = plan(waiting, (Some(&focused), &[]), never_hung);
        // The focused object's event and the query survive the limit and
        // do not count toward it: the newest ten other events are kept, and
        // only the oldest, observed at 2, is dropped.
        assert_eq!(
            observed(&planned),
            vec![1, 0, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
        );
    }

    #[test]
    fn events_from_a_hung_window_are_dropped_but_queries_are_not() {
        let waiting = vec![msaa(WinEventKind::ValueChange, 9, 1), query(1)];
        let planned = plan(waiting, (None, &[]), |hwnd| hwnd == 9);
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
        let planned = plan(waiting, (None, &[]), never_hung);
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

    fn push_fact(intake: &Intake, fact: DeliveredFact) {
        intake.push(Entry {
            item: Item::Fact(fact),
            trace: TraceId::mint(),
            observed_at_ms: 0,
            timing: crate::protocol::EventTiming::default(),
        });
    }

    fn push_query(intake: &Intake, request_id: u64) {
        intake.push(Entry {
            item: Item::Query {
                request_id,
                query: Query::FocusNow,
            },
            trace: TraceId::mint(),
            observed_at_ms: 0,
            timing: crate::protocol::EventTiming::default(),
        });
    }

    /// The query an entry carries, if it is one.
    fn request_of(planned: &Planned) -> Option<u64> {
        match planned {
            Planned::Run(Entry {
                item: Item::Query { request_id, .. },
                ..
            }) => Some(*request_id),
            _ => None,
        }
    }

    #[test]
    fn a_batch_holding_a_foreground_change_is_held_until_its_window_is_in_front() {
        // msinfo32 raises its focus event just before its foreground event,
        // and neither is handed out before the window is in front.
        let intake = Intake::default();
        push_fact(
            &intake,
            DeliveredFact::MsaaFocus {
                hwnd: 78,
                id_object: -4,
                id_child: 1,
            },
        );
        push_fact(&intake, DeliveredFact::Foreground { hwnd: 77 });
        let in_front = |hwnd| hwnd == 77;
        let first = intake
            .next_with(&in_front, FOREGROUND_WAIT)
            .expect("an entry");
        assert!(
            matches!(first.2, Some(Foreground::Confirmed(_))),
            "{:?}",
            first.2
        );
        let second = intake
            .next_with(&in_front, FOREGROUND_WAIT)
            .expect("an entry");
        assert_eq!(second.2, None, "only the batch's first entry carries it");
        assert_eq!(first.1, second.1, "one batch");

        push_fact(
            &intake,
            DeliveredFact::MsaaFocus {
                hwnd: 78,
                id_object: -4,
                id_child: 2,
            },
        );
        assert_eq!(
            intake
                .next_with(&in_front, FOREGROUND_WAIT)
                .expect("an entry")
                .2,
            None
        );
    }

    #[test]
    fn queries_go_ahead_of_a_batch_held_for_its_foreground() {
        // The window is not in front yet: the batch's events wait, and its
        // query, and one that arrives meanwhile, are handed out at once.
        let intake = Intake::default();
        push_fact(&intake, DeliveredFact::Foreground { hwnd: 77 });
        push_query(&intake, 1);
        let not_yet = |_| false;
        let long = Duration::from_secs(60);
        let first = intake.next_with(&not_yet, long).expect("an entry");
        assert_eq!((request_of(&first.0), first.2), (Some(1), None));
        push_query(&intake, 2);
        let second = intake.next_with(&not_yet, long).expect("an entry");
        assert_eq!((request_of(&second.0), second.2), (Some(2), None));
        assert!(intake.busy(), "the foreground change still waits");
        let third = intake
            .next_with(&|hwnd| hwnd == 77, long)
            .expect("an entry");
        assert!(matches!(
            third.0,
            Planned::Run(Entry {
                item: Item::Fact(DeliveredFact::Foreground { hwnd: 77 }),
                ..
            })
        ));
        assert!(matches!(third.2, Some(Foreground::Confirmed(_))));
    }

    #[test]
    fn a_batch_whose_window_never_comes_in_front_is_handed_out_after_the_wait() {
        let intake = Intake::default();
        push_fact(&intake, DeliveredFact::Foreground { hwnd: 77 });
        let entry = intake
            .next_with(&|_| false, Duration::ZERO)
            .expect("an entry");
        assert_eq!(entry.2, Some(Foreground::NotConfirmed));
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
        // The focus's own event keeps its place; the other is overtaken.
        let focused = Object::Msaa(3, -4, 30);
        let planned = plan(waiting, (Some(&focused), &[]), never_hung);
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

    #[test]
    fn an_msaa_focus_waiting_in_a_window_is_found_until_its_batch_is_planned() {
        let intake = Intake::default();
        let focus = DeliveredFact::MsaaFocus {
            hwnd: 9,
            id_object: -4,
            id_child: 1,
        };
        intake.push(fact(focus, 1).entry);
        assert!(intake.msaa_focus_waiting(9));
        assert!(!intake.msaa_focus_waiting(8));
        let _ = intake.next();
        assert!(!intake.msaa_focus_waiting(9));
    }

    #[test]
    fn a_focus_change_goes_before_the_events_of_other_objects_queued_ahead_of_it() {
        let waiting = vec![
            msaa(WinEventKind::NameChange, 7, 1),
            msaa(WinEventKind::ValueChange, 8, 2),
            msaa(WinEventKind::Selection, 9, 3),
            fact(
                DeliveredFact::MsaaFocus {
                    hwnd: 9,
                    id_object: -4,
                    id_child: 3,
                },
                4,
            ),
            msaa(WinEventKind::NameChange, 7, 5),
        ];
        let focused = Object::Msaa(8, -4, 2);
        let planned = plan(waiting, (Some(&focused), &[]), never_hung);
        assert_eq!(
            observed(&planned),
            vec![2, 3, 4, 1, 5],
            "the focus's event and the event of the object focus moves to keep their place; the other objects' events follow the focus, in their order"
        );
    }

    #[test]
    fn an_ancestors_state_change_observed_before_a_focus_change_goes_before_it() {
        // The focus's ancestor at (7, -4, 1) is judged; (7, -4, 2) is not.
        let judged = [Object::Msaa(7, -4, 1)];
        let focus = |observed_at_ms| {
            fact(
                DeliveredFact::MsaaFocus {
                    hwnd: 9,
                    id_object: -4,
                    id_child: 9,
                },
                observed_at_ms,
            )
        };
        let in_order = vec![
            msaa(WinEventKind::StateChange, 7, 1),
            msaa(WinEventKind::StateChange, 7, 2),
            focus(3),
        ];
        let planned = plan(in_order, (None, &judged), never_hung);
        assert_eq!(
            observed(&planned),
            vec![1, 3, 2],
            "the ancestor's change keeps its place ahead of the focus; the other object's follows it"
        );
        // Pushed after the focus change, though observed before it: the
        // listener relays a focus from another process.
        let pushed_late = vec![
            msaa(WinEventKind::StateChange, 7, 2),
            focus(5),
            msaa(WinEventKind::StateChange, 7, 1),
        ];
        let planned = plan(pushed_late, (None, &judged), never_hung);
        assert_eq!(observed(&planned), vec![1, 5, 2]);
        // Observed after the focus change, it is judged against the new
        // focus, after it.
        let observed_after = vec![focus(0), msaa(WinEventKind::StateChange, 7, 1)];
        let planned = plan(observed_after, (None, &judged), never_hung);
        assert_eq!(observed(&planned), vec![0, 1]);
    }

    #[test]
    fn a_focus_change_leaves_an_ancestors_unstarted_state_change_in_the_batch() {
        let intake = Intake::default();
        intake.set_judged(vec![Object::Msaa(0, -4, 2)]);
        let push = |item: Item, observed_at_ms: u64| {
            intake.push(Entry {
                item,
                trace: TraceId::mint(),
                observed_at_ms,
                timing: crate::protocol::EventTiming::default(),
            });
        };
        let state_change = |child: i32| Item::Msaa {
            kind: WinEventKind::StateChange,
            hwnd: 0,
            id_object: -4,
            id_child: child,
        };
        push(state_change(1), 1);
        push(state_change(2), 2);
        push(state_change(3), 3);
        let mut order = Vec::new();
        let (first, _, _) = intake.next().expect("an entry");
        order.extend(first.entries().iter().map(|entry| entry.observed_at_ms));
        push(
            Item::Fact(DeliveredFact::MsaaFocus {
                hwnd: 0,
                id_object: -4,
                id_child: 4,
            }),
            4,
        );
        while intake.busy() {
            let (planned, _, _) = intake.next().expect("an entry");
            order.extend(planned.entries().iter().map(|entry| entry.observed_at_ms));
        }
        assert_eq!(
            order,
            vec![1, 2, 4, 3],
            "the ancestor's change stays ahead of the focus; the other object's waits for it"
        );
    }

    #[test]
    fn a_focus_change_takes_back_the_unstarted_events_of_the_batch_in_progress() {
        let intake = Intake::default();
        let push = |item: Item, observed_at_ms: u64| {
            intake.push(Entry {
                item,
                trace: TraceId::mint(),
                observed_at_ms,
                timing: crate::protocol::EventTiming::default(),
            });
        };
        let name_change = |child: i32| Item::Msaa {
            kind: WinEventKind::NameChange,
            hwnd: 0,
            id_object: -4,
            id_child: child,
        };
        push(name_change(1), 1);
        push(name_change(2), 2);
        push(name_change(3), 3);
        let mut order = Vec::new();
        let (first, batch, _) = intake.next().expect("an entry");
        order.extend(first.entries().iter().map(|entry| entry.observed_at_ms));
        push(
            Item::Fact(DeliveredFact::MsaaFocus {
                hwnd: 0,
                id_object: -4,
                id_child: 4,
            }),
            4,
        );
        let mut batches = Vec::new();
        while intake.busy() {
            let (planned, number, _) = intake.next().expect("an entry");
            order.extend(planned.entries().iter().map(|entry| entry.observed_at_ms));
            batches.push(number);
        }
        assert_eq!(
            order,
            vec![1, 4, 2, 3],
            "the events not yet started wait for the focus"
        );
        assert_eq!(batches, vec![batch + 1; 3], "in the next batch");
    }
}
