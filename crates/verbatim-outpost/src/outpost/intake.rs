//! The outpost's intake queue (outpost redesign, "Inside an outpost"): one
//! queue for the application, holding both events and Core's queries,
//! handled in the order they arrived, with NVDA's limiter rules for MSAA
//! events (`orderedWinEventLimiter.py`, and `pumpAll` in NVDA's
//! `IAccessibleHandler`; `docs/design/focus-pipeline.md` section 5).
//!
//! - One waiting entry per object and event kind: a newer one replaces it and
//!   takes its place at the back. UIA events are limited by this alone, as
//!   NVDA's UIA event limiter keeps one event per element and kind and
//!   counts nothing.
//! - A batch is everything that accumulated while the worker handled the
//!   previous batch, planned in the order it arrived: nothing moves ahead
//!   of what came before it. The one exception is the focused terminal's
//!   caret event, planned after a text change of the terminal that waits
//!   behind it with only events between, since the change's read reads the
//!   caret too (`caret_after_update`).
//! - One entry may be kept apart, handed out only when nothing else is
//!   ready: a held terminal's noted caret ([`Item::NotedCaret`]).
//! - The console updates of one window may be absorbed: dropped as they are
//!   pushed, without waking the worker, while the worker has marked that
//!   window's console terminal as held with its text changed and no caret
//!   key's watch open ([`Intake::absorb_console_updates`]). The worker does
//!   nothing for such an update, and the read that ends the hold covers
//!   every update observed before it, as NVDA's console handler skips an
//!   update while one is already pending.
//! - Per batch, of the MSAA events, the newest 4 focus events and the
//!   newest 10 other events per thread that raised them are kept; a
//!   foreground change counts among both and is kept by either, as NVDA's
//!   limiter keeps it; the focused object's other events are always kept,
//!   though they count. Every MSAA event carries the thread that raised it,
//!   from the outpost's own hook or from the listener's, which counted by
//!   it the same. UIA events, menu openings, queries and housekeeping
//!   entries are not counted. A newer list of the nodes Core holds replaces
//!   a waiting one.
//! - UIA events from a window the system reports as hung are dropped
//!   before any read; an MSAA event from one is counted and kept, and the
//!   worker drops it before it reads anything, as NVDA's limiter counts it
//!   and `winEventToNVDAEvent` drops it.
//! - Within a batch only the newest foreground change is handled. Each
//!   consecutive run of MSAA focus events is tried newest first, until one
//!   is reported, at the place of the run's newest. NVDA's `pumpAll`
//!   collects a run of focus and foreground events together and tries it
//!   newest first when the next accepted event that is neither comes;
//!   Verbatim's runs differ: any other entry ends one, and a foreground
//!   change is not in one but handled at its own place (roadmap, the
//!   audits' items). Every UIA focus event is handled in turn at its own
//!   place, as NVDA's UIA handler queues each one (the worker's
//!   arbitration then drops a focus whose backend does not own the window,
//!   as NVDA's separate MSAA and UIA paths do). The newest menu opening
//!   from each backend is handled last: NVDA keeps only its newest MSAA
//!   menu event, a menu end included, which Verbatim sends elsewhere, and
//!   tries it after the focus events; NVDA's UIA menu opening is instead
//!   an event of its own in arrival order, which Verbatim does not yet
//!   follow.
//! - A UIA focus waits in the place its marker took
//!   ([`DeliveredFact::UiaFocusMarker`]): the focus listener sends the
//!   marker the moment the event arrives and reads the focus from the
//!   event's own element, and nothing taken in after the marker is planned
//!   until the read takes the marker's place, or a drop removes it
//!   (`docs/design/focus-pipeline.md`, section 12.4, decision 1 of section
//!   15). NVDA's UIA events likewise pass in order through the one thread
//!   that judges them, so an event raised after a focus is never handled
//!   before it. A read whose marker the intake never saw, as when the
//!   outpost started after it, is taken in where it arrives.
//! - A batch that holds a foreground change is handed out at once, never
//!   held for its window to become the foreground window: Windows raises
//!   the foreground event once the change is made, so a foreground event
//!   whose window is not in front when its batch is handled is stale or
//!   raised early by the application itself, and the worker drops it, as
//!   NVDA's `processForegroundWinEvent` does (measured live;
//!   `docs/design/phase6-design.md`, "Foreground events against the foreground
//!   window").
//!
//! Intake callbacks only push and return: they never call into the
//! application and never wait on the worker.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Instant;

use verbatim_ia2::WinEventKind;
use verbatim_model::{Notification, NotificationKind, NotificationProcessing, TraceId};
use windows::Win32::UI::Accessibility::IUIAutomationTextRange;
use windows::core::AgileReference;

use verbatim_uia::{Agile, Cached, Kept, PropertyEvent, TextEvent};

use crate::protocol::{DeliveredFact, Query, UiaSnapshotFact, WinEventStamp};

/// How many MSAA focus events a batch keeps (NVDA's limiter's
/// `maxFocusItems`).
const FOCUS_EVENTS_PER_BATCH: usize = 4;

/// How many other MSAA events a batch keeps per thread that raised them
/// (NVDA's `MAX_WINEVENTS_PER_THREAD`).
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
    /// For a listener's fact on an element that is not a window itself, its
    /// process's keyboard focus window when the listener captured it, else
    /// 0: the window its acceptance is tested with.
    pub(super) focus_window: isize,
    pub(super) element: Option<EventElement>,
}

/// The element one of the outpost's own UIA events came with, as the
/// shape its registration's part filled it as.
pub(super) enum EventElement {
    /// A property change's.
    Property(Agile<PropertyEvent>),
    /// A caret, text, or active text position change's.
    Text(Agile<TextEvent>),
}

impl EventElement {
    /// The element on this thread, read as the `Kept` shape both event
    /// shapes cover, which is all any reader of it reads and what the node
    /// registry keeps; `None` when it cannot be resolved.
    pub(super) fn resolve(&self) -> Option<Cached<Kept>> {
        match self {
            Self::Property(agile) => agile.resolve().ok().map(|event| event.narrow()),
            Self::Text(agile) => agile.resolve().ok().map(|event| event.narrow()),
        }
    }
}

/// One unit of work for the worker.
pub(super) enum Item {
    /// An MSAA `WinEvent` from this outpost's own hooks, with the thread
    /// that raised it, which the batch limits count it by.
    Msaa {
        kind: WinEventKind,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        thread: u32,
    },
    /// A UIA event from this outpost's own subscriptions.
    Uia(UiaEvent),
    /// A fact routed from the listener, or an MSAA focus, foreground
    /// change, menu opening, alert, or tooltip the outpost's own hook took
    /// in as the same fact, with the thread that raised an MSAA event and
    /// when (`None` for a UIA fact).
    Fact(DeliveredFact, Option<WinEventStamp>),
    /// A query from Core.
    Query { request_id: u64, query: Query },
    /// The nodes Core still holds, and the position of the last message it
    /// has handled: release the rest.
    NodesHeld { nodes: Vec<u64>, acknowledged: u64 },
    /// Report the caret of the focus `node_id` names, which has text or
    /// whose role says it may, just after it was reported: as `CaretMoved`,
    /// or as `NoText` when there is no caret to report.
    CaretOf { node_id: verbatim_model::NodeId },
    /// A caret event of the focused terminal `node_id`, noted while its
    /// reading was held and its text had changed unread: its caret is read
    /// once nothing else is waiting ([`Intake::when_idle`]), unless a read
    /// since saw it, so Core's copy of the caret catches up when a burst of
    /// output ends rather than at the next read Core asks for.
    NotedCaret { node_id: verbatim_model::NodeId },
    /// [`Outpost::settle`](super::Outpost::settle): answered once nothing
    /// else is waiting, the focus-following subscriptions have made every
    /// move asked of them, and every message published has been written.
    Settle(std::sync::mpsc::Sender<()>),
    /// The harness's idle barrier, round `round`
    /// (`docs/design/communication.md`, section 6.7): reached once
    /// everything that arrived before it has been handled, and answered
    /// once the follow-ups those entries queued are handled and no deferred
    /// work is pending. `behind_follow_ups` once the worker has put it
    /// directly behind those follow-ups ([`Intake::push_behind_follow_ups`]),
    /// so that reaching it again waits for nothing more.
    IdleQuery { round: u64, behind_follow_ups: bool },
    /// The time [`Intake::wake_at`] named has come with nothing else to do:
    /// a caret key's watch may have reached the bound on its age.
    Wake,
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
    UiaNotification(
        Vec<i32>,
        NotificationKind,
        NotificationProcessing,
        Option<String>,
    ),
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
            | Key::UiaNotification(runtime_id, ..) => Some(Object::Uia(runtime_id.clone())),
            Key::Foreground(_) | Key::NodesHeld | Key::CaretOf(_) => None,
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
    /// An MSAA focus or foreground event, among the newest few per batch.
    Focus,
    /// Any other MSAA event, limited per thread that raised it.
    Other,
    /// Not counted: a UIA event, which NVDA's MSAA limiter never sees,
    /// dropped with its window when that is hung, and an MSAA menu
    /// opening, of which only the newest is handled, as NVDA's limiter
    /// keeps its newest menu event apart from both limits.
    Uncounted,
    /// Always kept: queries and housekeeping.
    Exempt,
}

/// An entry with what the limiter needs to know about it, computed when it
/// is pushed.
struct Waiting {
    key: Option<Key>,
    category: Category,
    hwnd: isize,
    /// The thread an MSAA entry is counted by ([`thread_of`]), 0 for one
    /// not counted.
    thread: u32,
    /// For a UIA focus's marker still waiting for its read, the marker:
    /// nothing at or after it is planned.
    marker: Option<u64>,
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

#[derive(Default)]
struct State {
    waiting: VecDeque<Waiting>,
    batch: VecDeque<Planned>,
    /// Numbers the batches, so the worker can tell whether a focus was
    /// reported within the batch a menu opening belongs to.
    batch_number: u64,
    focused: Option<Object>,
    /// When [`State::focused`] is a terminal read through UIA, its window
    /// (0 when not known): its caret event is planned after its text
    /// changes waiting with it ([`caret_after_update`]).
    terminal: Option<isize>,
    /// A noted caret ([`Item::NotedCaret`]), handed out only when nothing
    /// else is ready ([`Intake::when_idle`]).
    idle: Option<Entry>,
    /// Whether [`State::focused`] was reached by redirecting a control's
    /// own focus event to its focused child, whose own event is still to
    /// come.
    focus_redirected: bool,
    closed: bool,
    /// When the worker asked to be given [`Item::Wake`] if nothing else
    /// comes first.
    wake: Option<Instant>,
    /// The console window whose updates are dropped as they are pushed
    /// ([`Intake::absorb_console_updates`]).
    absorbed: Option<isize>,
}

/// The queue the worker draws from.
#[derive(Default)]
pub(super) struct Intake {
    state: Mutex<State>,
    ready: Condvar,
}

impl State {
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
}

impl Intake {
    /// Adds an entry. The thread it is counted by and the window are read
    /// here, with local calls, so the limiter never needs them later. A UIA
    /// focus's read takes its marker's place, and a drop, or the end of the
    /// listener's reads, removes the markers it resolves. An entry with a
    /// key replaces the one waiting with the same key, unless an idle query
    /// waits between them (`docs/design/communication.md`, section 6.7):
    /// the older entry then keeps its place ahead of the query.
    pub(super) fn push(&self, entry: Entry) {
        // A marked UIA event takes its marker's place as itself.
        let (entry, resolved_by_read) = match entry.item {
            Item::Fact(DeliveredFact::UiaMarked { marker, fact }, stamp) => (
                Entry {
                    item: Item::Fact(*fact, stamp),
                    ..entry
                },
                Some(marker),
            ),
            item => (Entry { item, ..entry }, None),
        };
        let (key, category, hwnd) = classify(&entry.item);
        let thread = thread_of(&entry.item);
        let mut state = self.lock();
        if state.closed {
            // Shutting down: nothing new is taken. The entry is dropped
            // here, outside the lock, with whatever it holds.
            drop(state);
            return;
        }
        if let Item::Msaa {
            kind: WinEventKind::ConsoleUpdate,
            hwnd,
            ..
        } = &entry.item
            && state.absorbed == Some(*hwnd)
        {
            // The worker would do nothing for it, and the read that ends
            // the hold covers it: it is not queued, and wakes nobody.
            return;
        }
        let marker = match &entry.item {
            Item::Fact(DeliveredFact::UiaFocusDropped { marker }, _) => {
                tracing::debug!(marker, "a UIA focus's marker is dropped with its focus");
                state
                    .waiting
                    .retain(|waiting| waiting.marker != Some(*marker));
                drop(state);
                self.ready.notify_one();
                return;
            }
            Item::Fact(DeliveredFact::UiaFocusReadsEnded, _) => {
                tracing::debug!("the listener's reads ended; no marker is waited on");
                state.waiting.retain(|waiting| waiting.marker.is_none());
                drop(state);
                self.ready.notify_one();
                return;
            }
            Item::Fact(DeliveredFact::UiaFocusMarker { marker }, _) => Some(*marker),
            _ => None,
        };
        let resolves = match &entry.item {
            Item::Fact(DeliveredFact::UiaFocusRead { marker, .. }, _) => Some(*marker),
            _ => resolved_by_read,
        };
        let mut place = resolves.and_then(|resolves| {
            state
                .waiting
                .iter()
                .position(|waiting| waiting.marker == Some(resolves))
        });
        if let Some(key) = &key {
            // Never across an idle query: the entry replaced would move
            // behind it.
            let at = place.unwrap_or(state.waiting.len());
            if let Some(removed) = remove_keyed(&mut state.waiting, key, at)
                && let Some(place) = place.as_mut()
                && removed < *place
            {
                *place -= 1;
            }
        }
        // Logged, with what is queued around it, so an entry that waits
        // long in the intake can be told from one that arrived late; once
        // the lock is released, so the worker never waits on the log.
        let logged = tracing::enabled!(tracing::Level::DEBUG).then(|| {
            (
                entry.trace,
                key.clone(),
                state.waiting.len() + 1,
                state.batch.len(),
                state.batch_number,
            )
        });
        let waiting = Waiting {
            key,
            category,
            hwnd,
            thread,
            marker,
            entry,
        };
        match place {
            Some(place) => state.waiting[place] = waiting,
            None => state.waiting.push_back(waiting),
        }
        drop(state);
        self.ready.notify_one();
        if let Some((trace, key, waiting, in_batch, batch)) = logged {
            tracing::debug!(%trace, ?key, waiting, in_batch, batch, "intake pushed");
        }
    }

    /// The next entry to handle and the number of its batch, planning a new
    /// batch from everything waiting when the current one is done. With
    /// nothing else ready it gives the entry [`when_idle`](Self::when_idle)
    /// kept, if any. Blocks while there is nothing to do, and gives
    /// [`Item::Wake`] when the time
    /// [`wake_at`](Self::wake_at) named comes first; `None` once the queue
    /// is closed.
    pub(super) fn next(&self) -> Option<(Planned, u64)> {
        let mut state = self.lock();
        loop {
            if let Some(planned) = state.batch.pop_front() {
                return Some((planned, state.batch_number));
            }
            // Everything before the first marker still waiting for its
            // read; with a marker first, the worker waits for the read. An
            // idle query ends its batch, so nothing that arrived after it
            // is planned with what arrived before.
            let ready = state
                .waiting
                .iter()
                .position(|waiting| {
                    waiting.marker.is_some() || matches!(waiting.entry.item, Item::IdleQuery { .. })
                })
                .map_or(state.waiting.len(), |at| {
                    if state.waiting[at].marker.is_some() {
                        at
                    } else {
                        at + 1
                    }
                });
            if ready > 0 {
                let mut waiting: Vec<Waiting> = state.waiting.drain(..ready).collect();
                let taken = waiting.len();
                let focused = state.focused.clone();
                caret_after_update(&mut waiting, focused.as_ref(), state.terminal);
                let batch = plan(waiting, focused.as_ref(), super::window::window_is_hung);
                state.batch = batch.into();
                state.batch_number += 1;
                tracing::debug!(
                    batch = state.batch_number,
                    taken,
                    planned = state.batch.len(),
                    "intake planned a batch"
                );
                continue;
            }
            if state.closed {
                return None;
            }
            if let Some(entry) = state.idle.take() {
                return Some((Planned::Run(entry), state.batch_number));
            }
            let now = Instant::now();
            if let Some(entry) = state.wake_due(now) {
                return Some((Planned::Run(entry), state.batch_number));
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

    /// Closes the queue for the outpost's shutdown: everything waiting and
    /// the batch in progress are dropped, nothing pushed from now on is kept, and every
    /// [`next`](Self::next) answers `None` once the entry in hand is done.
    /// The entries are returned for the caller to drop on a thread in COM's
    /// multithreaded apartment, since an entry can hold a UIA element.
    pub(super) fn close(&self) -> Vec<Entry> {
        let mut state = self.lock();
        state.closed = true;
        state.wake = None;
        let mut dropped: Vec<Entry> = state
            .waiting
            .drain(..)
            .map(|waiting| waiting.entry)
            .collect();
        for planned in state.batch.drain(..) {
            match planned {
                Planned::Run(entry) | Planned::Menu(entry) => dropped.push(entry),
                Planned::Focus(entries) => dropped.extend(entries),
            }
        }
        dropped.extend(state.idle.take());
        drop(state);
        self.ready.notify_all();
        dropped
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

    /// Whether [`next`](Self::next) has an entry to hand out without
    /// waiting: one planned, or one waiting ahead of every marker.
    #[cfg(test)]
    fn ready_now(&self) -> bool {
        let state = self.lock();
        !state.batch.is_empty()
            || state
                .waiting
                .front()
                .is_some_and(|waiting| waiting.marker.is_none())
    }

    /// Whether anything is waiting to be handled, planned or not, a UIA
    /// focus's marker waiting for its read included, and the entry kept for
    /// when nothing else is ready left out ([`take_idle`](Self::take_idle)).
    pub(super) fn busy(&self) -> bool {
        let state = self.lock();
        !state.waiting.is_empty() || !state.batch.is_empty()
    }

    /// Whether an entry for which `wanted` holds waits to be handled,
    /// planned or not: what has been observed and not yet handled. The
    /// entry kept for when nothing else is ready is left out.
    pub(super) fn any_waiting(&self, wanted: impl Fn(&Entry) -> bool) -> bool {
        let state = self.lock();
        state.waiting.iter().any(|waiting| wanted(&waiting.entry))
            || state
                .batch
                .iter()
                .any(|planned| planned.entries().into_iter().any(&wanted))
    }

    /// Puts `entries` directly behind the newest caret follow-up waiting
    /// ([`Item::CaretOf`]), or ahead of everything waiting when none is,
    /// for the idle barrier's query the worker has reached: every follow-up
    /// waiting then was queued by an entry that arrived before the query,
    /// since the query ends its batch and nothing after it has been
    /// handled, so the query waits for those and for nothing that arrived
    /// after it.
    pub(super) fn push_behind_follow_ups(&self, entries: impl IntoIterator<Item = Entry>) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        let at = state
            .waiting
            .iter()
            .rposition(|waiting| matches!(waiting.entry.item, Item::CaretOf { .. }))
            .map_or(0, |caret| caret + 1);
        for (offset, entry) in entries.into_iter().enumerate() {
            let (key, category, hwnd) = classify(&entry.item);
            let thread = thread_of(&entry.item);
            state.waiting.insert(
                at + offset,
                Waiting {
                    key,
                    category,
                    hwnd,
                    thread,
                    marker: None,
                    entry,
                },
            );
        }
        drop(state);
        self.ready.notify_one();
    }

    /// Keeps `entry` to be handed out once nothing else is ready, in place
    /// of one kept before: the worker reads a held terminal's noted caret
    /// then ([`Item::NotedCaret`]), when the burst of events that kept it
    /// busy has ended, and not on every caret event of the burst.
    pub(super) fn when_idle(&self, entry: Entry) {
        let replaced = self.lock().idle.replace(entry);
        drop(replaced);
    }

    /// Drops the console updates of `window` as they are pushed, from now
    /// until the next call, or takes them in again with `None`. The worker
    /// marks the window while its focused console terminal's reading is
    /// held with its text changed and no caret key's watch is open, when it
    /// would do nothing for an update; it clears the mark before it handles
    /// anything that changes that, so an update observed after the read
    /// that ends the hold begins is never dropped. A new focus clears it
    /// too ([`set_focused`](Self::set_focused)).
    pub(super) fn absorb_console_updates(&self, window: Option<isize>) {
        self.lock().absorbed = window;
    }

    /// Takes the entry [`when_idle`](Self::when_idle) kept, if any.
    pub(super) fn take_idle(&self) -> Option<Entry> {
        self.lock().idle.take()
    }

    /// Records the object the worker last reported as the focus: its MSAA
    /// events other than focus events are always kept.
    pub(super) fn set_focused(&self, object: Option<Object>) {
        let mut state = self.lock();
        state.focused = object;
        state.terminal = None;
        state.focus_redirected = false;
        state.absorbed = None;
    }

    /// Records that the focus last reported ([`set_focused`](Self::set_focused))
    /// is a terminal read through UIA, in window `window` (0 when not
    /// known), whose caret event is planned after its text changes.
    pub(super) fn set_focus_is_terminal(&self, window: isize) {
        self.lock().terminal = Some(window);
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

/// Removes the entry waiting with `key` among those around index `at` that
/// no idle query divides from it, and returns its index. An idle query
/// splits what waits into stretches, each holding at most one entry per
/// key, so the first found is the only one.
fn remove_keyed(waiting: &mut VecDeque<Waiting>, key: &Key, at: usize) -> Option<usize> {
    let is_query = |waiting: &Waiting| matches!(waiting.entry.item, Item::IdleQuery { .. });
    let matches = |waiting: &Waiting| waiting.key.as_ref() == Some(key);
    let found = waiting
        .range(..at)
        .rev()
        .take_while(|waiting| !is_query(waiting))
        .position(matches)
        .map(|back| at - 1 - back)
        .or_else(|| {
            waiting
                .range(at..)
                .take_while(|waiting| !is_query(waiting))
                .position(matches)
                .map(|ahead| at + ahead)
        })?;
    waiting.remove(found);
    Some(found)
}

/// The window an item concerns, 0 for none.
pub(super) fn window_of(item: &Item) -> isize {
    classify(item).2
}

/// The key, category, and window of an item.
fn classify(item: &Item) -> (Option<Key>, Category, isize) {
    match item {
        Item::Msaa {
            kind,
            hwnd,
            id_object,
            id_child,
            ..
        } => match kind {
            WinEventKind::Destroy => (None, Category::Exempt, *hwnd),
            // A foreground window raised hidden is reported once it is shown,
            // so its show is never dropped; one per window waits.
            WinEventKind::WindowShown => (
                Some(Key::Msaa(*kind as u8, *hwnd, 0, 0)),
                Category::Exempt,
                *hwnd,
            ),
            // Each console update names the cells it changed, and each
            // console caret event the cell its cursor is in; only that the
            // window's text changed, or its cursor moved, matters, as both
            // are read, so each kind merges into one per window while they
            // wait.
            WinEventKind::ConsoleUpdate | WinEventKind::ConsoleCaret => (
                Some(Key::Msaa(*kind as u8, *hwnd, 0, 0)),
                Category::Other,
                *hwnd,
            ),
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
                UiaKind::Notification(notification) => {
                    Some(notification_key(&event.parts.runtime_id, notification))
                }
            };
            (key, Category::Uncounted, event.hwnd)
        }
        Item::Fact(fact, _) => classify_fact(fact),
        // Only the newest list of held nodes matters.
        Item::NodesHeld { .. } => (Some(Key::NodesHeld), Category::Exempt, 0),
        // Only the newest caret report for a node matters, and it is never
        // limited: the focus's caret.
        Item::CaretOf { node_id } => (Some(Key::CaretOf(node_id.number())), Category::Exempt, 0),
        Item::Query { .. }
        | Item::NotedCaret { .. }
        | Item::Settle(_)
        | Item::IdleQuery { .. }
        | Item::Wake => (None, Category::Exempt, 0),
    }
}

/// Moves the focused terminal's caret event (its console's caret event, or
/// its UIA caret event in Windows Terminal), when it waits ahead of a text
/// change of the same terminal (its console update, or its UIA text change),
/// to just after the newest such change, as long as only events lie between
/// them. The text change's read reads the caret too, so the caret event is
/// then covered by it and reads nothing (`Context::caret_read_since`), where
/// handled first it read the caret once more for nothing; a batch with a
/// query, a fact, or anything else between the two keeps its order. Windows
/// Terminal raises its caret event after its text change, but the intake
/// moves a newer change that replaces a waiting one to the back, behind the
/// caret event; the console host raises the two in either order.
fn caret_after_update(
    waiting: &mut Vec<Waiting>,
    focused: Option<&Object>,
    terminal: Option<isize>,
) {
    let (Some(Object::Uia(runtime_id)), Some(window)) = (focused, terminal) else {
        return;
    };
    let uia_caret = Key::Uia(2, 0, runtime_id.clone());
    let console_caret =
        (window != 0).then_some(Key::Msaa(WinEventKind::ConsoleCaret as u8, window, 0, 0));
    let Some(at) = waiting.iter().position(|waiting| {
        let key = waiting.key.as_ref();
        key == Some(&uia_caret) || (key.is_some() && key == console_caret.as_ref())
    }) else {
        return;
    };
    let text_changed = Key::Uia(3, 0, runtime_id.clone());
    let console_update =
        (window != 0).then_some(Key::Msaa(WinEventKind::ConsoleUpdate as u8, window, 0, 0));
    let mut newest_change = None;
    for (index, later) in waiting.iter().enumerate().skip(at + 1) {
        if !matches!(later.entry.item, Item::Msaa { .. } | Item::Uia(_)) {
            break;
        }
        let key = later.key.as_ref();
        if key == Some(&text_changed) || (key.is_some() && key == console_update.as_ref()) {
            newest_change = Some(index);
        }
    }
    if let Some(change) = newest_change {
        let caret = waiting.remove(at);
        waiting.insert(change, caret);
    }
}

/// A notification's key: its element, kind, processing, and activity id,
/// not its text, as NVDA's UIA limiter keys one (`eventRecord.h`), so a
/// newer notification of the same activity replaces one still waiting.
fn notification_key(runtime_id: &[i32], notification: &Notification) -> Key {
    Key::UiaNotification(
        runtime_id.to_vec(),
        notification.kind,
        notification.processing,
        notification.activity_id.clone(),
    )
}

/// The key, category, and window of a fact, from the listener or the
/// outpost's own hook.
fn classify_fact(fact: &DeliveredFact) -> (Option<Key>, Category, isize) {
    match fact {
        // Unwrapped as it is pushed (`Intake::push`); as itself otherwise.
        DeliveredFact::UiaMarked { fact, .. } => classify_fact(fact),
        // Selections and notifications routed from the listener are
        // UIA events, never counted.
        DeliveredFact::UiaSelection { hwnd, snapshot, .. } => (
            Some(Key::Uia(1, 0, snapshot.runtime_id.clone())),
            Category::Uncounted,
            *hwnd,
        ),
        DeliveredFact::UiaNotification {
            hwnd,
            snapshot,
            notification,
        } => (
            Some(notification_key(&snapshot.runtime_id, notification)),
            Category::Uncounted,
            *hwnd,
        ),
        // Alerts and shows are ordinary MSAA events, limited per thread.
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
            (Some(key), Category::Other, *hwnd)
        }
        DeliveredFact::Foreground { hwnd } => {
            (Some(Key::Foreground(*hwnd)), Category::Focus, *hwnd)
        }
        DeliveredFact::MsaaFocus {
            hwnd,
            id_object,
            id_child,
        } => (
            Some(Key::MsaaFocus(*hwnd, *id_object, *id_child)),
            Category::Focus,
            *hwnd,
        ),
        DeliveredFact::UiaFocusRead { read, .. } => (
            Some(Key::UiaFocus(read.snapshot.runtime_id.clone())),
            Category::Uncounted,
            if read.hwnd == 0 {
                read.focus_window
            } else {
                read.hwnd
            },
        ),
        // A marker is never planned: its read takes its place, or a drop
        // removes it. A drop and the end of the listener's reads are never
        // queued.
        DeliveredFact::UiaFocusMarker { .. }
        | DeliveredFact::UiaFocusDropped { .. }
        | DeliveredFact::UiaFocusReadsEnded => (None, Category::Exempt, 0),
        DeliveredFact::MenuPopup {
            hwnd,
            id_object,
            id_child,
        } => (
            Some(Key::MenuPopup(*hwnd, *id_object, *id_child)),
            Category::Uncounted,
            *hwnd,
        ),
    }
}

/// The thread the batch limits count an entry by: for an MSAA event, from
/// the outpost's own hook or the listener's, the thread that raised it, as
/// NVDA's limiter counts by the event's own thread (for a console window,
/// the console host's thread, not the window's); 0 for anything else, and
/// for an MSAA fact with no thread, which only a test hands over.
fn thread_of(item: &Item) -> u32 {
    match item {
        Item::Msaa { thread, .. } => *thread,
        Item::Fact(_, Some(stamp)) => stamp.thread,
        _ => 0,
    }
}

/// What the batch limits keep of everything that was waiting, oldest
/// first, as NVDA's limiters flush it: UIA events from hung windows are
/// dropped, as NVDA's UIA handlers drop them as they arrive, and so are
/// all but the newest MSAA focus events and the newest other MSAA events of
/// each thread, those of the focused object excepted. An MSAA event from a
/// hung window is counted and kept here, as NVDA's limiter counts it, and
/// dropped by the worker when it is handled, as NVDA drops it when it
/// makes its object (`winEventToNVDAEvent`).
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
        let msaa = matches!(
            item.key,
            Some(Key::Msaa(..) | Key::MsaaFocus(..) | Key::Foreground(_) | Key::MenuPopup(..))
        );
        if !msaa
            && item.category != Category::Exempt
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
            Category::Exempt | Category::Uncounted => true,
            Category::Focus => {
                focus_kept += 1;
                let among_focus = focus_kept <= FOCUS_EVENTS_PER_BATCH;
                if matches!(item.key, Some(Key::Foreground(_))) {
                    // A foreground change is also one of its thread's other
                    // events, as NVDA's limiter caches a foreground event
                    // with both its focus events and its other events
                    // (`orderedWinEventLimiter.py`, `addEvent`), so newer
                    // focus events do not crowd it out.
                    let count = per_thread.entry(item.thread).or_default();
                    *count += 1;
                    among_focus || *count <= EVENTS_PER_THREAD
                } else {
                    among_focus
                }
            }
            Category::Other => {
                // The focused object's events count, and are kept beyond
                // the limit, as NVDA's `alwaysAllowedObjects`.
                let count = per_thread.entry(item.thread).or_default();
                *count += 1;
                *count <= EVENTS_PER_THREAD || of_focus
            }
        };
    }

    waiting
        .into_iter()
        .zip(keep)
        .filter_map(|(item, keep)| {
            if !keep && item.category == Category::Focus {
                tracing::debug!(
                    trace = %item.entry.trace,
                    key = ?item.key,
                    "focus event dropped by the batch limits"
                );
            }
            keep.then_some(item)
        })
        .collect()
}

/// Plans one batch from everything that was waiting, in the order it
/// arrived: drops events from hung windows, applies the batch limits, keeps
/// only the newest foreground change, tries each consecutive run of MSAA
/// focus events newest first at the place of its newest, handles each UIA
/// focus event at its own place, and moves the newest menu opening from
/// each backend to the end, ahead of an idle query that ends the batch.
///
/// NVDA's `pumpAll` walks the flushed events in order, collecting focus and
/// foreground events, and when another event comes processes those
/// collected newest first, stopping at the first valid one, before that
/// event. A foreground change here stays where it was, handled on its own,
/// and does not end a run.
fn plan(
    waiting: Vec<Waiting>,
    focused: Option<&Object>,
    window_is_hung: impl Fn(isize) -> bool,
) -> Vec<Planned> {
    let kept = admit(waiting, focused, window_is_hung);
    let newest = |wanted: fn(&Key) -> bool| {
        kept.iter()
            .rposition(|item| item.key.as_ref().is_some_and(wanted))
    };
    let foreground = newest(|key| matches!(key, Key::Foreground(_)));
    let msaa_menu = newest(|key| matches!(key, Key::MenuPopup(..)));
    // Each consecutive run of MSAA focus events, oldest first, by where its
    // newest is: any entry other than an MSAA focus or a foreground change
    // ends a run.
    let mut runs: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut run: Vec<usize> = Vec::new();
    for (index, item) in kept.iter().enumerate() {
        match item.key {
            Some(Key::MsaaFocus(..)) => run.push(index),
            Some(Key::Foreground(_)) => {}
            _ => {
                if let Some(&last) = run.last() {
                    runs.insert(last, std::mem::take(&mut run));
                }
            }
        }
    }
    if let Some(&last) = run.last() {
        runs.insert(last, run);
    }

    let (keys, mut entries): (Vec<Option<Key>>, Vec<Option<Entry>>) = kept
        .into_iter()
        .map(|item| (item.key, Some(item.entry)))
        .unzip();
    let mut planned = Vec::with_capacity(entries.len());
    let mut deferred = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        if matches!(key, Some(Key::MsaaFocus(..))) {
            // An older event of a run waits for its newest, where the run is
            // tried, newest first.
            if let Some(run) = runs.get(&index) {
                let group = run
                    .iter()
                    .rev()
                    .filter_map(|&at| entries[at].take())
                    .collect();
                planned.push(Planned::Focus(group));
            }
            continue;
        }
        let Some(entry) = entries[index].take() else {
            continue;
        };
        match key {
            Some(Key::Foreground(_)) if Some(index) != foreground => {}
            Some(Key::UiaFocus(_)) => planned.push(Planned::Focus(vec![entry])),
            Some(Key::MenuPopup(..)) => {
                // The newest only, as NVDA keeps its newest menu event.
                if Some(index) == msaa_menu {
                    deferred.push(entry);
                }
            }
            _ => planned.push(Planned::Run(entry)),
        }
    }
    // An idle query ends its batch ([`Intake::next`]): the menu opening
    // that arrived before it is handled before it.
    let query = matches!(
        planned.last(),
        Some(Planned::Run(Entry {
            item: Item::IdleQuery { .. },
            ..
        }))
    )
    .then(|| planned.pop())
    .flatten();
    planned.extend(deferred.into_iter().map(Planned::Menu));
    planned.extend(query);
    planned
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An MSAA event on child `child` of window `hwnd`, raised by `thread`
    /// and observed at `child` milliseconds.
    fn msaa_from(kind: WinEventKind, hwnd: isize, child: i32, thread: u32) -> Waiting {
        let item = Item::Msaa {
            kind,
            hwnd,
            id_object: -4,
            id_child: child,
            thread,
        };
        let (key, category, hwnd) = classify(&item);
        Waiting {
            key,
            category,
            hwnd,
            thread: thread_of(&item),
            marker: None,
            entry: Entry {
                item,
                trace: TraceId::mint(),
                observed_at_ms: u64::try_from(child).unwrap_or(0),
                timing: crate::protocol::EventTiming::default(),
            },
        }
    }

    /// An MSAA event raised by a thread numbered as its window is.
    fn msaa(kind: WinEventKind, hwnd: isize, child: i32) -> Waiting {
        msaa_from(kind, hwnd, child, u32::try_from(hwnd).unwrap_or(0))
    }

    fn fact(fact: DeliveredFact, observed_at_ms: u64) -> Waiting {
        let item = Item::Fact(fact, None);
        let (key, category, hwnd) = classify(&item);
        Waiting {
            key,
            category,
            hwnd,
            thread: 1,
            marker: None,
            entry: Entry {
                item,
                trace: TraceId::mint(),
                observed_at_ms,
                timing: crate::protocol::EventTiming::default(),
            },
        }
    }

    /// An MSAA focus on child `child` of window 9.
    fn msaa_focus(child: i32, observed_at_ms: u64) -> Waiting {
        fact(
            DeliveredFact::MsaaFocus {
                hwnd: 9,
                id_object: -4,
                id_child: child,
            },
            observed_at_ms,
        )
    }

    /// A UIA element's cached parts, `runtime_id` its whole identity.
    fn snapshot(runtime_id: Vec<i32>) -> UiaSnapshotFact {
        UiaSnapshotFact {
            runtime_id,
            role: verbatim_model::Role::ListItem,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        }
    }

    /// The focus listener's read of a UIA focus on `runtime_id` in window
    /// `hwnd`, resolving marker `marker`.
    fn uia_focus(marker: u64, hwnd: isize, runtime_id: Vec<i32>) -> DeliveredFact {
        DeliveredFact::UiaFocusRead {
            marker,
            read: Box::new(crate::protocol::UiaFocusRead {
                hwnd,
                focus_window: 0,
                snapshot: snapshot(runtime_id),
                ancestors: Some(Vec::new()),
                selected_child: None,
                window: Some(hwnd),
                same_as_last: None,
            }),
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
            marker: None,
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

    /// The focus groups of `planned`, each newest first, in order.
    fn focus_groups(planned: &[Planned]) -> Vec<Vec<u64>> {
        planned
            .iter()
            .filter_map(|planned| match planned {
                Planned::Focus(entries) => {
                    Some(entries.iter().map(|entry| entry.observed_at_ms).collect())
                }
                _ => None,
            })
            .collect()
    }

    fn never_hung(_: isize) -> bool {
        false
    }

    #[test]
    fn a_windows_console_updates_merge_into_one_while_they_wait() {
        let key = |kind, object, child| {
            classify(&Item::Msaa {
                kind,
                hwnd: 7,
                id_object: object,
                id_child: child,
                thread: 0,
            })
            .0
        };
        // Console updates name the cells they changed; one per window waits.
        assert_eq!(
            key(WinEventKind::ConsoleUpdate, 3, 65),
            key(WinEventKind::ConsoleUpdate, 9, 66)
        );
        // Other events keep their objects apart.
        assert_ne!(
            key(WinEventKind::ValueChange, 3, 0),
            key(WinEventKind::ValueChange, 9, 0)
        );
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
    fn msaa_events_are_counted_by_the_thread_that_raised_them() {
        // A console window's update comes from the console host's thread,
        // 80, not the window's, 70, which raised ten newer events in it.
        let mut waiting = vec![msaa_from(WinEventKind::ConsoleUpdate, 7, 1, 80)];
        waiting.extend((2..=11).map(|child| msaa_from(WinEventKind::ValueChange, 7, child, 70)));
        let planned = plan(waiting, None, never_hung);
        assert_eq!(observed(&planned), (1..=11).collect::<Vec<u64>>());
    }

    #[test]
    fn an_msaa_fact_is_counted_by_the_thread_that_raised_it() {
        // An alert from the listener or the outpost's own hook carries its
        // event's thread, whatever thread its window belongs to.
        let alert = Item::Fact(
            DeliveredFact::Alert {
                hwnd: 7,
                id_object: -4,
                id_child: 0,
            },
            Some(WinEventStamp {
                thread: 80,
                time: 1,
            }),
        );
        assert_eq!(thread_of(&alert), 80);
        let unstamped = Item::Fact(DeliveredFact::Foreground { hwnd: 7 }, None);
        assert_eq!(thread_of(&unstamped), 0);
    }

    #[test]
    fn uia_events_are_not_counted_against_the_limits() {
        let mut waiting: Vec<Waiting> = (1..=11)
            .map(|n| {
                fact(
                    DeliveredFact::UiaSelection {
                        hwnd: 3,
                        focus_window: 0,
                        snapshot: snapshot(vec![42, n]),
                    },
                    u64::try_from(n).unwrap_or(0),
                )
            })
            .collect();
        waiting.extend(
            (12..=16).map(|n| fact(uia_focus(0, 3, vec![42, n]), u64::try_from(n).unwrap_or(0))),
        );
        let planned = plan(waiting, None, never_hung);
        assert_eq!(
            observed(&planned),
            (1..=16).collect::<Vec<u64>>(),
            "eleven selections and five focus events, each handled in order"
        );
    }

    #[test]
    fn a_foreground_change_is_kept_behind_newer_focus_events() {
        let waiting = vec![
            msaa_focus(1, 1),
            fact(DeliveredFact::Foreground { hwnd: 9 }, 2),
            msaa_focus(3, 3),
            msaa_focus(4, 4),
            msaa_focus(5, 5),
            msaa_focus(6, 6),
        ];
        let kept: Vec<u64> = admit(waiting, None, never_hung)
            .iter()
            .map(|item| item.entry.observed_at_ms)
            .collect();
        // Four newer focus events fill the focus limit; the foreground
        // change is kept as one of its thread's other events, and the
        // oldest focus is dropped.
        assert_eq!(kept, vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn the_focused_objects_events_and_queries_are_always_kept() {
        let mut waiting = vec![msaa(WinEventKind::NameChange, 7, 1), query(1)];
        waiting.extend((2..=12).map(|child| msaa(WinEventKind::ValueChange, 7, child)));
        waiting.push(msaa(WinEventKind::ValueChange, 7, 1));
        let focused = Object::Msaa(7, -4, 1);
        let planned = plan(waiting, Some(&focused), never_hung);
        // The query is not counted. The focused object's events are, as
        // NVDA counts them, but kept beyond the limit: its newest is the
        // first of the ten, so of the other events only the nine newest
        // are kept, and its oldest is kept though the limit was reached.
        assert_eq!(
            observed(&planned),
            vec![1, 0, 4, 5, 6, 7, 8, 9, 10, 11, 12, 1]
        );
    }

    /// A notification raised on element 42 with `activity` and `text`.
    fn notification_fact(activity: &str, text: &str) -> Item {
        Item::Fact(
            DeliveredFact::UiaNotification {
                hwnd: 0,
                snapshot: snapshot(vec![42]),
                notification: Notification {
                    kind: NotificationKind::ActionCompleted,
                    processing: NotificationProcessing::All,
                    display_string: Some(text.to_owned()),
                    activity_id: Some(activity.to_owned()),
                },
            },
            None,
        )
    }

    #[test]
    fn a_newer_notification_of_the_same_activity_replaces_one_waiting_whatever_its_text() {
        let intake = Intake::default();
        push_item(&intake, notification_fact("progress", "10 percent"), 10);
        push_item(&intake, notification_fact("other", "saved"), 20);
        push_item(&intake, notification_fact("progress", "20 percent"), 30);
        let mut order = Vec::new();
        for _ in 0..2 {
            if let Some((Planned::Run(entry), _)) = intake.next() {
                order.push(entry.observed_at_ms);
            }
        }
        assert_eq!(order, vec![20, 30]);
    }

    #[test]
    fn a_hung_windows_uia_events_are_dropped_its_msaa_events_kept_for_the_worker() {
        // The MSAA event is counted and kept, for the worker to drop as
        // NVDA drops it when it makes its object; the UIA selection is
        // dropped, as NVDA's UIA handler drops it on arrival; a query is
        // never dropped.
        let waiting = vec![
            msaa(WinEventKind::ValueChange, 9, 1),
            fact(
                DeliveredFact::UiaSelection {
                    hwnd: 9,
                    focus_window: 0,
                    snapshot: snapshot(vec![42]),
                },
                5,
            ),
            query(1),
        ];
        let planned = plan(waiting, None, |hwnd| hwnd == 9);
        assert_eq!(observed(&planned), vec![1, 0]);
    }

    #[test]
    fn a_menu_opening_goes_after_the_focus_events_of_its_batch() {
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
            fact(uia_focus(0, 3, vec![42]), 5),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(
            observed(&planned),
            vec![2, 4, 5, 1],
            "the MSAA and the UIA focus are both handled, so the backend that owns the window reports"
        );
        assert!(matches!(planned.last(), Some(Planned::Menu(_))));
    }

    #[test]
    fn every_uia_focus_of_a_batch_is_handled_oldest_first() {
        // Control+Tab in Windows Terminal: the tab the focus leaves, the tab
        // it moves to, and that tab's terminal, in one batch.
        let uia = |runtime_id: i32, observed_at_ms: u64| {
            fact(uia_focus(0, 3, vec![42, runtime_id]), observed_at_ms)
        };
        let planned = plan(vec![uia(1, 1), uia(2, 2), uia(3, 3)], None, never_hung);
        assert_eq!(observed(&planned), vec![1, 2, 3]);
        assert!(
            planned
                .iter()
                .all(|planned| matches!(planned, Planned::Focus(entries) if entries.len() == 1)),
            "each is handled on its own, not as a fallback for a newer one"
        );
    }

    #[test]
    fn each_consecutive_run_of_msaa_focus_events_is_tried_newest_first_at_its_place() {
        let waiting = vec![
            msaa_focus(1, 1),
            msaa_focus(2, 2),
            msaa(WinEventKind::NameChange, 3, 30),
            msaa_focus(3, 4),
            msaa_focus(4, 5),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(
            focus_groups(&planned),
            vec![vec![2, 1], vec![5, 4]],
            "the event between ends the first run"
        );
        assert_eq!(observed(&planned), vec![2, 1, 30, 5, 4]);
    }

    #[test]
    fn a_run_of_five_msaa_focus_events_keeps_the_newest_four() {
        let waiting = (1..=5).map(|child| msaa_focus(child, u64::try_from(child).unwrap_or(0)));
        let planned = plan(waiting.collect(), None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![5, 4, 3, 2]]);
    }

    #[test]
    fn a_foreground_change_within_a_run_keeps_its_place_and_does_not_end_the_run() {
        let waiting = vec![
            msaa_focus(1, 1),
            fact(DeliveredFact::Foreground { hwnd: 9 }, 2),
            msaa_focus(3, 3),
            msaa(WinEventKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![3, 1]]);
        assert_eq!(observed(&planned), vec![2, 3, 1, 30]);
    }

    #[test]
    fn a_focus_change_is_handled_in_the_order_it_arrived() {
        // In one batch, after the events of other objects that came first.
        let waiting = vec![
            msaa(WinEventKind::NameChange, 7, 1),
            msaa(WinEventKind::ValueChange, 8, 2),
            msaa_focus(3, 3),
            msaa(WinEventKind::NameChange, 7, 4),
        ];
        assert_eq!(observed(&plan(waiting, None, never_hung)), vec![1, 2, 3, 4]);
        // Arriving while a batch is in progress, after that batch's events
        // that have not started.
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
            thread: 0,
        };
        push(name_change(1), 1);
        push(name_change(2), 2);
        push(name_change(3), 3);
        let mut order = Vec::new();
        let (first, _) = intake.next().expect("an entry");
        order.extend(first.entries().iter().map(|entry| entry.observed_at_ms));
        push(
            Item::Fact(
                DeliveredFact::MsaaFocus {
                    hwnd: 0,
                    id_object: -4,
                    id_child: 4,
                },
                None,
            ),
            4,
        );
        while intake.busy() {
            let (planned, _) = intake.next().expect("an entry");
            order.extend(planned.entries().iter().map(|entry| entry.observed_at_ms));
        }
        assert_eq!(order, vec![1, 2, 3, 4]);
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
                    thread: 0,
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
            if let Some((Planned::Run(entry), _)) = intake.next() {
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
        let Some((Planned::Run(entry), _)) = intake.next() else {
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
            item: Item::Fact(fact, None),
            trace: TraceId::mint(),
            observed_at_ms: 0,
            timing: crate::protocol::EventTiming::default(),
        });
    }

    #[test]
    fn a_batch_holding_a_foreground_change_is_handed_out_at_once() {
        // Window 77 is not the foreground window, nor any window at all:
        // the batch is not held for it, and the worker judges the change.
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
        let (_, first) = intake.next().expect("an entry");
        let (_, second) = intake.next().expect("an entry");
        assert_eq!(first, second, "one batch");
        assert!(!intake.busy(), "nothing is held");
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

    fn push_item(intake: &Intake, item: Item, observed_at_ms: u64) {
        intake.push(Entry {
            item,
            trace: TraceId::mint(),
            observed_at_ms,
            timing: crate::protocol::EventTiming::default(),
        });
    }

    fn name_change(child: i32) -> Item {
        Item::Msaa {
            kind: WinEventKind::NameChange,
            hwnd: 0,
            id_object: -4,
            id_child: child,
            thread: 0,
        }
    }

    fn marker(marker: u64) -> Item {
        Item::Fact(DeliveredFact::UiaFocusMarker { marker }, None)
    }

    /// Everything the intake hands out while it has something ready, by
    /// observation time.
    fn handed_out(intake: &Intake) -> Vec<u64> {
        let mut order = Vec::new();
        while intake.ready_now() {
            let (planned, _) = intake.next().expect("an entry");
            order.extend(planned.entries().iter().map(|entry| entry.observed_at_ms));
        }
        order
    }

    #[test]
    fn what_arrives_after_a_focus_marker_waits_for_its_read_and_the_read_takes_its_place() {
        let intake = Intake::default();
        push_item(&intake, name_change(1), 1);
        push_item(&intake, marker(7), 2);
        push_item(&intake, name_change(3), 3);
        assert_eq!(
            handed_out(&intake),
            [1],
            "the change after the marker waits"
        );
        assert!(intake.busy(), "the marker and the change still wait");
        push_item(&intake, Item::Fact(uia_focus(7, 3, vec![42, 7]), None), 4);
        assert_eq!(
            handed_out(&intake),
            [4, 3],
            "the focus in its marker's place, then the change"
        );
        assert!(!intake.busy());
    }

    #[test]
    fn a_marked_selection_takes_its_markers_place_as_itself() {
        let intake = Intake::default();
        push_item(&intake, marker(8), 1);
        push_item(&intake, name_change(3), 2);
        assert_eq!(
            handed_out(&intake),
            Vec::<u64>::new(),
            "all wait on the marker"
        );
        let selection = DeliveredFact::UiaSelection {
            hwnd: 0,
            focus_window: 9,
            snapshot: snapshot(vec![42, 8]),
        };
        let marked = DeliveredFact::UiaMarked {
            marker: 8,
            fact: Box::new(selection.clone()),
        };
        push_item(&intake, Item::Fact(marked, None), 3);
        let (planned, _) = intake.next().expect("an entry");
        let entries = planned.entries();
        assert!(
            matches!(&entries[0].item, Item::Fact(fact, None) if *fact == selection),
            "the selection, unwrapped, in its marker's place"
        );
        assert_eq!(handed_out(&intake), [2], "then the change");
    }

    #[test]
    fn a_dropped_focus_releases_what_waited_behind_its_marker() {
        let intake = Intake::default();
        push_item(&intake, marker(7), 1);
        push_item(&intake, marker(8), 2);
        push_item(&intake, name_change(3), 3);
        assert_eq!(handed_out(&intake), Vec::<u64>::new());
        push_item(
            &intake,
            Item::Fact(DeliveredFact::UiaFocusDropped { marker: 7 }, None),
            4,
        );
        assert_eq!(
            handed_out(&intake),
            Vec::<u64>::new(),
            "marker 8 still waits"
        );
        push_item(
            &intake,
            Item::Fact(DeliveredFact::UiaFocusDropped { marker: 8 }, None),
            5,
        );
        assert_eq!(handed_out(&intake), [3]);
        assert!(!intake.busy());
    }

    #[test]
    fn the_end_of_the_listeners_reads_releases_every_marker() {
        let intake = Intake::default();
        push_item(&intake, marker(7), 1);
        push_item(&intake, name_change(2), 2);
        push_item(&intake, marker(8), 3);
        push_item(&intake, name_change(4), 4);
        push_item(
            &intake,
            Item::Fact(DeliveredFact::UiaFocusReadsEnded, None),
            5,
        );
        assert_eq!(handed_out(&intake), [2, 4]);
        assert!(!intake.busy());
    }

    /// A UIA text event of kind `kind` on element `runtime_id`.
    fn text_event(kind: UiaKind, runtime_id: &[i32]) -> Item {
        Item::Uia(UiaEvent {
            kind,
            parts: snapshot(runtime_id.to_vec()),
            hwnd: 0,
            focus_window: 0,
            element: None,
        })
    }

    #[test]
    fn a_focused_terminals_caret_event_is_planned_after_its_text_change() {
        let terminal = [42, 7];
        let intake = Intake::default();
        intake.set_focused(Some(Object::Uia(terminal.to_vec())));
        intake.set_focus_is_terminal(7);
        // The console host's text area, whose caret its console's caret
        // event tells of, and its output its console updates.
        push_item(
            &intake,
            Item::Msaa {
                kind: WinEventKind::ConsoleCaret,
                hwnd: 7,
                id_object: 2,
                id_child: 9,
                thread: 80,
            },
            1,
        );
        push_item(
            &intake,
            Item::Msaa {
                kind: WinEventKind::ConsoleUpdate,
                hwnd: 7,
                id_object: 3,
                id_child: 65,
                thread: 80,
            },
            2,
        );
        assert_eq!(handed_out(&intake), [2, 1]);
        // Windows Terminal's control, whose output its UIA text change
        // tells of.
        push_item(&intake, text_event(UiaKind::TextSelection, &terminal), 3);
        push_item(&intake, text_event(UiaKind::TextChanged, &terminal), 4);
        assert_eq!(handed_out(&intake), [4, 3]);
    }

    /// A console update of window `hwnd`, observed at `observed_at_ms`.
    fn push_console_update(intake: &Intake, hwnd: isize, observed_at_ms: u64) {
        push_item(
            intake,
            Item::Msaa {
                kind: WinEventKind::ConsoleUpdate,
                hwnd,
                id_object: 3,
                id_child: 65,
                thread: 80,
            },
            observed_at_ms,
        );
    }

    #[test]
    fn a_held_consoles_updates_wake_nobody_until_the_mark_is_cleared() {
        let intake = Intake::default();
        intake.absorb_console_updates(Some(7));
        push_console_update(&intake, 7, 1);
        assert!(!intake.busy(), "the held console's update is not queued");
        // Another window's update, and the held window's caret, still come.
        push_console_update(&intake, 8, 2);
        push_item(
            &intake,
            Item::Msaa {
                kind: WinEventKind::ConsoleCaret,
                hwnd: 7,
                id_object: 2,
                id_child: 9,
                thread: 80,
            },
            3,
        );
        assert_eq!(handed_out(&intake), [2, 3]);
        intake.absorb_console_updates(None);
        push_console_update(&intake, 7, 4);
        assert_eq!(handed_out(&intake), [4], "the next update comes through");
        intake.absorb_console_updates(Some(7));
        intake.set_focused(None);
        push_console_update(&intake, 7, 5);
        assert_eq!(handed_out(&intake), [5], "a new focus clears the mark");
    }

    fn idle_query(round: u64) -> Item {
        Item::IdleQuery {
            round,
            behind_follow_ups: false,
        }
    }

    /// An idle query divides what waits: a console update that waited
    /// before it is not replaced by a newer one of the same window behind
    /// it, and is handled ahead of it.
    #[test]
    fn an_entry_waiting_before_an_idle_query_is_never_replaced_from_behind_it() {
        let intake = Intake::default();
        push_console_update(&intake, 7, 1);
        push_item(&intake, idle_query(1), 2);
        push_console_update(&intake, 7, 3);
        // Ahead of the query, a newer update of the same window behind
        // the first still replaces the one waiting with it.
        push_console_update(&intake, 7, 4);
        assert_eq!(handed_out(&intake), [1, 2, 4]);
    }

    /// An idle query ends its batch: the menu opening that arrived before
    /// it is handled before it, and a foreground change that arrived after
    /// it does not replace one that arrived before.
    #[test]
    fn a_batch_ends_at_an_idle_query_with_its_menu_opening_ahead_of_it() {
        let intake = Intake::default();
        push_fact(
            &intake,
            DeliveredFact::MenuPopup {
                hwnd: 77,
                id_object: -4,
                id_child: 0,
            },
        );
        push_item(
            &intake,
            Item::Fact(DeliveredFact::Foreground { hwnd: 77 }, None),
            2,
        );
        push_item(&intake, idle_query(1), 3);
        push_item(
            &intake,
            Item::Fact(DeliveredFact::Foreground { hwnd: 78 }, None),
            4,
        );
        assert_eq!(handed_out(&intake), [2, 0, 3, 4]);
    }

    #[test]
    fn a_focus_read_whose_marker_never_came_is_taken_where_it_arrives() {
        let intake = Intake::default();
        push_item(&intake, name_change(1), 1);
        push_item(&intake, Item::Fact(uia_focus(9, 3, vec![42, 9]), None), 2);
        push_item(&intake, name_change(3), 3);
        assert_eq!(handed_out(&intake), [1, 2, 3]);
    }
}
