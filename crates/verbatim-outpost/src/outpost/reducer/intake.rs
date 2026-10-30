//! The outpost's intake queue (architecture section 1, "Outposts"): one
//! queue for the application, holding both events and Core's queries,
//! handled in the order they arrived, with NVDA's limiter rules for MSAA
//! events (`orderedWinEventLimiter.py`, and `pumpAll` in NVDA's
//! `IAccessibleHandler`; `docs/design/focus-pipeline.md` section 5).
//!
//! - One waiting entry per object and event kind, and, for an MSAA focus,
//!   foreground change or other MSAA event but a menu opening, per thread
//!   that raised it, as NVDA's limiter keys its focus and other events
//!   (`orderedWinEventLimiter.py` 63 to 75 and 79), so two threads' events
//!   of one address both wait: a newer one replaces it and takes its place
//!   at the back. A menu opening waits per address alone, and a batch
//!   handles only its newest, whatever its address or thread, as NVDA's
//!   limiter keeps only its most recent menu event (76 to 78): per
//!   application, for popup starts, where NVDA's one slot also holds menu
//!   starts and ends and covers every application (`docs/parity/backends.md`,
//!   the limiter's entry). The console host's events, which NVDA hooks
//!   apart, merge per window and kind whatever the thread. UIA events are
//!   limited by this alone, as NVDA's UIA event limiter keeps one event
//!   per element and kind and counts nothing.
//! - A batch is everything that accumulated while the worker handled the
//!   previous batch, planned in the order it arrived: nothing moves ahead
//!   of what came before it. There are two exceptions: the focused
//!   terminal's caret event, planned after a text change of the terminal
//!   that waits behind it with only events between, since the change's
//!   read reads the caret too (`caret_after_update`); and the old focus's
//!   own UIA events behind a UIA focus marker whose read is a focus inside
//!   the old focus, described below.
//! - One entry may be kept apart, handed out only when nothing else is
//!   ready: a held terminal's noted caret ([`Item::NotedCaret`]).
//! - The console updates of one window may be absorbed: dropped as they are
//!   pushed, without waking the worker, while the worker has marked that
//!   window's console terminal as held with its text changed and no caret
//!   key's watch open ([`Intake::absorb_console_updates`]). The worker does
//!   nothing for such an update, and the read that ends the hold covers
//!   every update observed before it, as NVDA's console handler skips an
//!   update while one is already pending.
//! - The system caret's events (`WinKind::Caret`) are dropped as they
//!   are pushed, without waking the worker, while the worker has marked
//!   the focus as having caret events of its own
//!   ([`Intake::drop_system_caret`]): a UIA focus that has raised a UIA
//!   caret event, or a focused console terminal, whose console caret event
//!   tells of the same caret.
//! - Per batch, of the MSAA events, the newest 4 focus events and the
//!   newest 10 other events per thread that raised them are kept; a
//!   foreground change counts among both and is kept by either, as NVDA's
//!   limiter keeps it; the focused object's other events are always kept,
//!   though they count. Every MSAA event carries the thread that raised it,
//!   from the outpost's own hook or from the listener's, which counted by
//!   it the same. UIA events, menu openings, queries and housekeeping
//!   entries are not counted. A newer list of the nodes Core holds replaces
//!   a waiting one. A foreground change removes a focus event waiting on
//!   its window's client object from the same thread when the focus was
//!   not raised after the change, as NVDA's limiter does
//!   (`orderedWinEventLimiter.py` 65 to 67). A run of focus events and
//!   foreground changes can hold the 4 newest of them and, besides,
//!   every older foreground change of another window, or of the same
//!   window from another thread, that its thread's limit keeps.
//! - UIA events from a window the system reports as hung are dropped
//!   before any read; an MSAA event from one is counted and kept, and the
//!   worker drops it before it reads anything, as NVDA's limiter counts it
//!   and `winEventToNVDAEvent` drops it.
//! - Each consecutive run of MSAA focus events and foreground changes is
//!   tried newest first, until one is reported, at the place of the run's
//!   newest, as NVDA's `pumpAll` collects a run of focus and foreground
//!   events and tries it newest first when the next accepted event that is
//!   neither comes. Only an MSAA event NVDA's pump receives ends a run,
//!   menu openings among them; entries its pump never receives neither
//!   join a run nor end it, and are handled at their own places, before
//!   the run when they arrived before its newest: UIA events and focus
//!   reads, Core's queries and list of held nodes, the outpost's settle
//!   request, an object destroyed, and
//!   the console host's events and a text control's selection change, which
//!   NVDA hooks apart or not at all (`ends_a_run`). An MSAA event the
//!   worker's acceptance test will drop still ends a run, where NVDA's pump
//!   filters it out first, since that test is made against the foreground
//!   window when the event is handled. A foreground change with no focus
//!   event beside it is a run of its own. A run whose events
//!   all carry the time Windows raised them is tried in that order, which
//!   is the order NVDA's one hook receives them in, since an outpost can
//!   receive the listener's copy of an event after its own hook's copy of
//!   a later one (`in_raised_order`); events of one tick of the clock keep
//!   the order they arrived in. Every foreground change of the
//!   batch is tried in its run, as NVDA tries each; of two changes of the
//!   same window from the same thread, the later to arrive replaces the
//!   one waiting, as NVDA's limiter keeps one foreground event per window,
//!   object and thread. Every UIA focus event is handled in turn at its
//!   own place, as NVDA's UIA handler queues each one as it arrives, on a
//!   path apart from the MSAA pump: one
//!   that arrived between two events of an MSAA run is handled before the
//!   run (the worker's arbitration then drops a focus whose backend does
//!   not own the window, as NVDA's separate MSAA and UIA paths do, so it
//!   is never spoken beside an MSAA focus of the same window). The newest
//!   menu opening
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
//! - When a UIA focus's read takes its marker's place and the focus last
//!   reported ([`Intake::note_focus`]) is among the read's ancestors, the
//!   old focus's own UIA state changes that the outpost's subscriptions
//!   queued after the marker, up to the next marker or idle query, are
//!   planned ahead of the read, in their order (`old_focus_events_first`).
//!   The listener sends the marker the moment the focus event arrives,
//!   while the outpost's own subscriptions receive the old focus's events
//!   on a route of their own, so a state change the provider raised before
//!   moving the focus into a child, as a combo box opening into its list
//!   raises its expand state's change before its item's focus, can be
//!   queued a few milliseconds behind the marker; handled after the
//!   focus, it would be a change of an ancestor the focus report already
//!   read, and go unsaid. The intake cannot tell a state change raised
//!   before the focus from one raised after it, and moves both, so either
//!   is spoken, as NVDA speaks a state change of the focus or of any of
//!   its ancestors in either order (`docs/design/focus-pipeline.md`,
//!   section 12.4). A state change is a change of any property the worker
//!   reports as the element's states ([`super::entry::is_state_property`]), such
//!   as the expand and toggle states. The old focus's name and value
//!   changes keep their places, since NVDA speaks those only for the
//!   focus, so one raised after the focus goes unsaid in both. Events of
//!   other elements, the listener's facts (already in the listener's
//!   order), and every event when the new focus is not inside the old keep
//!   their places, so a change raised as an element loses the focus is
//!   still handled after the new focus. The check compares runtime ids
//!   already in hand and makes no call. Its reference is the focus last
//!   reported, which lags a focus read taken in but not yet handled, so
//!   a state change of that read's element queued behind the next focus
//!   inside it is not moved: a stated limit.
//! - The gap record ([`super::gap_record`]) is kept here: each UIA
//!   focus's outcome, a read placed by its own window's top-level window
//!   or a drop by the answer it carries, is applied to it as the outcome
//!   resolves its marker; a drop for no window that answers an open record
//!   takes its marker's place as the record's read ([`Item::GapRead`]).
//!   Programs' judgements of copies and foreground changes are applied
//!   with their commits, and the bound's entry ([`Item::GapBound`]),
//!   pushed when the reducer's timer for it fires, waits behind every
//!   marker queued before it, so a UIA focus already being read decides
//!   first (`docs/design/focus-pipeline-rework.md`, "Who owns each
//!   duplicated fact", the gap's evidence).
//! - A batch that holds a foreground change is handed out at once, never
//!   held for its window to become the foreground window: Windows raises
//!   the foreground event once the change is made, so a foreground event
//!   whose window is not in front when its batch is handled is stale or
//!   raised early by the application itself, and the worker drops it, as
//!   NVDA's `processForegroundWinEvent` does (measured live on
//!   2026-10-09; `docs/crates/verbatim-outpost.md`, "Foreground changes and
//!   focus-now").
//!
//! The intake is the reducer's ([`super`]): the mailbox's producers only
//! append, and the reducer pushes each entry it drains into the intake, in
//! mailbox order.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use verbatim_model::protocol::{DeliveredFact, GapAnswer, WinEventStamp};
use verbatim_model::{NotificationKind, NotificationProcessing};

use super::entry::{Entry, Item, UiaKind, WinKind, is_state_property};
use super::gap_record::{Accepted, CopyDropped, GapRead, GapRecord, Outcome};

/// The object id of a window's client area (`OBJID_CLIENT`), which the
/// hook gives an event on a window object, as NVDA's event callback does.
pub(in crate::outpost) const OBJID_CLIENT: i32 = -4;

/// How many MSAA focus events a batch keeps (NVDA's limiter's
/// `maxFocusItems`).
const FOCUS_EVENTS_PER_BATCH: usize = 4;

/// How many other MSAA events a batch keeps per thread that raised them
/// (NVDA's `MAX_WINEVENTS_PER_THREAD`).
const EVENTS_PER_THREAD: usize = 10;

/// What the gap record's timer is to do after a change to the record: be
/// set for a record opened, cancelled for one answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::outpost) enum TimerAction {
    /// Nothing.
    Keep,
    /// Set it for this due time, on the observation clock.
    Set(u64),
    /// Cancel it.
    Cancel,
}

/// The object and kind an entry concerns, for the one-per-object rule and for
/// recognizing the focused object.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(in crate::outpost) enum Key {
    /// An MSAA event other than a focus or foreground change, by its kind,
    /// address and the thread that raised it, as NVDA's limiter keys it;
    /// the console host's events, which NVDA hooks apart, by kind and
    /// window alone, with thread 0.
    Msaa(u8, isize, i32, i32, u32),
    Uia(u8, i32, Vec<i32>),
    /// A window's foreground change, by the window and the thread that
    /// raised it, as NVDA's limiter keys it.
    Foreground(isize, u32),
    /// An MSAA focus, by its address and the thread that raised it, as
    /// NVDA's limiter keys it.
    MsaaFocus(isize, i32, i32, u32),
    UiaFocus(Vec<i32>),
    MenuPopup(isize, i32, i32),
    UiaNotification(
        Vec<i32>,
        NotificationKind,
        NotificationProcessing,
        Option<String>,
    ),
    NodesHeld,
}

impl Key {
    /// The object part alone, to compare with the focused object whatever
    /// the event kind.
    fn object(&self) -> Option<Object> {
        match self {
            Key::Msaa(_, hwnd, object, child, _)
            | Key::MsaaFocus(hwnd, object, child, _)
            | Key::MenuPopup(hwnd, object, child) => Some(Object::Msaa(*hwnd, *object, *child)),
            Key::Uia(_, _, runtime_id)
            | Key::UiaFocus(runtime_id)
            | Key::UiaNotification(runtime_id, ..) => Some(Object::Uia(runtime_id.clone())),
            Key::Foreground(..) | Key::NodesHeld => None,
        }
    }
}

/// An accessible object, by its event address or runtime id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(in crate::outpost) enum Object {
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
    /// The thread an MSAA entry is counted by ([`raiser_of`]), 0 for one
    /// not counted.
    raiser: u32,
    /// For a UIA focus's marker still waiting for its read, the marker:
    /// nothing at or after it is planned.
    marker: Option<u64>,
    entry: Entry,
}

/// The order in which the worker handles a planned batch.
pub(in crate::outpost) enum Planned {
    /// Handle the entry.
    Run(Entry),
    /// Handle a menu opening, after the batch's focus events.
    Menu(Entry),
    /// Focus events from one backend, with a foreground change among MSAA
    /// ones, newest first: the worker handles each in turn until one is
    /// reported ([`try_newest_first`]), as NVDA falls back to an older focus
    /// or foreground event when the newest cannot be processed.
    Focus(Vec<Entry>),
}

/// How a program is to handle one entry of a [`Planned`] item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::outpost) enum Handling {
    /// On its own.
    Run,
    /// As a menu opening, after the batch's focus events.
    Menu,
    /// As one entry of a focus group, which is reported or not, of this
    /// rank in it, 0 for the newest.
    Tried(u32),
}

impl Planned {
    /// The entries, in the order the worker may handle them.
    pub(in crate::outpost) fn entries(&self) -> Vec<&Entry> {
        match self {
            Planned::Run(entry) | Planned::Menu(entry) => vec![entry],
            Planned::Focus(entries) => entries.iter().collect(),
        }
    }
}

/// The intake: what waits to be planned, the batch being handed out, and
/// what the planning reads of the focus. One owner, the reducer, so it
/// takes no lock.
#[derive(Default)]
pub(in crate::outpost) struct Intake {
    waiting: VecDeque<Waiting>,
    batch: VecDeque<Planned>,
    /// Numbers the batches, so a program can tell whether a focus was
    /// reported within the batch a menu opening belongs to.
    batch_number: u64,
    marks: Marks,
    /// A noted caret ([`Item::NotedCaret`]), handed out only when nothing
    /// else is ready ([`Intake::when_idle`]).
    idle: Option<Entry>,
    closed: bool,
    /// When a wake was asked for ([`Intake::wake_at`]), on the reducer's
    /// clock: [`Item::Wake`] is handed out then if nothing else comes
    /// first.
    wake: Option<Duration>,
    /// The gap record and the answers it looks back on.
    gap: GapRecord,
}

/// What the intake, and the programs it hands entries to, read of the
/// focus last reported: set in one go by the commit that makes a focus
/// followed ([`Intake::note_focus`]), and changed by later commits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::outpost) struct Marks {
    /// The object last reported as the focus.
    pub(in crate::outpost) focused: Option<Object>,
    /// When `focused` is a terminal read through UIA, its window (0 when
    /// not known): its caret event is planned after its text changes
    /// waiting with it ([`caret_after_update`]).
    pub(in crate::outpost) terminal: Option<isize>,
    /// Whether `focused` was reached by redirecting a control's own focus
    /// event to its focused child, whose own event is still to come.
    pub(in crate::outpost) focus_redirected: bool,
    /// The console window whose updates are dropped as they are pushed
    /// ([`Intake::absorb_console_updates`]).
    pub(in crate::outpost) absorbed: Option<isize>,
    /// Whether the system caret's events are dropped as they are pushed:
    /// the focus last reported has caret events of its own
    /// ([`Intake::drop_system_caret`]).
    pub(in crate::outpost) system_caret_unused: bool,
}

impl Marks {
    /// Whether `item` is dropped as it is pushed, waking nobody: a console
    /// update while its window's updates are absorbed, for which a program
    /// would do nothing and which the read that ends the hold covers
    /// ([`Intake::absorb_console_updates`]), and the system caret's event
    /// while the focus's own caret events tell of the same moves
    /// ([`Intake::drop_system_caret`]). Judged by the mailbox as an entry
    /// is pushed, on its copy of these marks.
    pub(in crate::outpost) fn drops_on_arrival(&self, item: &Item) -> bool {
        match item {
            Item::Msaa {
                kind: WinKind::ConsoleUpdate,
                hwnd,
                ..
            } => self.absorbed == Some(*hwnd),
            Item::Msaa {
                kind: WinKind::Caret,
                ..
            } => self.system_caret_unused,
            _ => false,
        }
    }
}

/// What one push did besides queuing its entry: what the gap record's
/// timer is to do, and the objects parked for entries it removed, which a
/// worker drops.
#[derive(Debug, Default, PartialEq, Eq)]
pub(in crate::outpost) struct Pushed {
    pub(in crate::outpost) timer: Option<TimerAction>,
    pub(in crate::outpost) unparked: Vec<u64>,
}

impl Intake {
    /// Resolves the markers `entry` resolves without a read, a drop or the
    /// end of the listener's reads; whether it was one. A drop whose answer
    /// had the gap record read at once is not one: it has become that read
    /// ([`Item::GapRead`]), which takes the marker's place.
    fn resolves_without_read(&mut self, entry: &Entry) -> bool {
        match &entry.item {
            Item::Fact(DeliveredFact::UiaFocusDropped { marker, .. }, _) => {
                self.waiting
                    .retain(|waiting| waiting.marker != Some(*marker));
                true
            }
            Item::Fact(DeliveredFact::UiaFocusReadsEnded, _) => {
                self.waiting.retain(|waiting| waiting.marker.is_none());
                true
            }
            _ => false,
        }
    }

    /// Applies `entry`, a UIA focus's outcome, to the gap record, a read's
    /// answer being `read_answer`, and returns it with what the record's
    /// timer is to do: a drop for no window that answered the open record
    /// becomes the record's read ([`Item::GapRead`]), which takes the
    /// drop's marker's place, stamped with the record's stamp.
    fn apply_gap_outcome(
        &mut self,
        entry: Entry,
        read_answer: Option<GapAnswer>,
    ) -> (Entry, TimerAction, Option<u64>) {
        let answer = match &entry.item {
            Item::Fact(DeliveredFact::UiaFocusDropped { answer, .. }, _) => *answer,
            _ => match read_answer {
                Some(answer) => answer,
                None => return (entry, TimerAction::Keep, None),
            },
        };
        match self.gap.outcome(answer, entry.observed_at_us) {
            Outcome::Unchanged => (entry, TimerAction::Keep, None),
            Outcome::Answered => (entry, TimerAction::Cancel, None),
            Outcome::ReadNow(read) => match entry.item {
                Item::Fact(DeliveredFact::UiaFocusDropped { marker, .. }, _) => (
                    Entry {
                        item: Item::GapRead {
                            window: read.window,
                        },
                        observed_at_us: read.stamp_us,
                        ..entry
                    },
                    TimerAction::Cancel,
                    Some(marker),
                ),
                item => (Entry { item, ..entry }, TimerAction::Cancel, None),
            },
        }
    }

    /// Adds an entry. A UIA focus's read takes its marker's place, and a
    /// drop, or the end of the listener's reads, removes the markers it
    /// resolves. An entry with a key replaces the one waiting with the same
    /// key, unless an idle query waits between them
    /// (`docs/design/communication.md`, section 6.7): the older entry then
    /// keeps its place ahead of the query. A foreground change also
    /// removes a focus waiting on its window's client object from the same
    /// thread, unless the focus was raised after the change
    /// ([`focus_raised_first`]), as NVDA's limiter removes it when it adds
    /// the window's foreground event (`orderedWinEventLimiter.py` 65 to
    /// 67); the hook gives an event on a window object as its client
    /// object, as NVDA's callback does.
    ///
    /// A UIA focus's outcome is applied to the gap record as it resolves
    /// its marker: a focus's read placed by `read_answer`, the answer the
    /// mailbox found with one local call as the read was pushed, and a
    /// drop by the answer it carries. Returns what the record's timer is to
    /// do then, and the parked objects of the entries a newer one replaced.
    pub(in crate::outpost) fn push(
        &mut self,
        entry: Entry,
        read_answer: Option<GapAnswer>,
    ) -> Pushed {
        let mut pushed = Pushed::default();
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
        let raiser = raiser_of(&entry.item);
        if self.closed {
            pushed.unparked = entry.item.parked();
            return pushed;
        }
        let (entry, action, gap_read_resolves) = self.apply_gap_outcome(entry, read_answer);
        if action != TimerAction::Keep {
            pushed.timer = Some(action);
        }
        if self.resolves_without_read(&entry) {
            return pushed;
        }
        let marker = match &entry.item {
            Item::Fact(DeliveredFact::UiaFocusMarker { marker }, _) => Some(*marker),
            _ => None,
        };
        let resolves = match &entry.item {
            Item::Fact(DeliveredFact::UiaFocusRead { marker, .. }, _) => Some(*marker),
            _ => resolved_by_read.or(gap_read_resolves),
        };
        let mut place = resolves.and_then(|resolves| {
            self.waiting
                .iter()
                .position(|waiting| waiting.marker == Some(resolves))
        });
        if let Some(key) = &key {
            // Never across an idle query: the entry replaced would move
            // behind it.
            let at = place.unwrap_or(self.waiting.len());
            if let Some((removed, gone)) = remove_keyed(&mut self.waiting, key, at) {
                pushed.unparked.extend(gone.entry.item.parked());
                if let Some(place) = place.as_mut()
                    && removed < *place
                {
                    *place -= 1;
                }
            }
            if let (Key::Foreground(window, raiser), Item::Fact(_, stamp)) = (key, &entry.item) {
                remove_client_focus(&mut self.waiting, *window, *raiser, *stamp);
            }
        }
        // A focus moving into a child of the old focus: the old focus's own
        // UIA events queued behind the marker are planned ahead of the read.
        if let (Some(at), Item::Fact(DeliveredFact::UiaFocusRead { read, .. }, _)) =
            (place.as_mut(), &entry.item)
            && let Some(Object::Uia(old)) = &self.marks.focused
            && read
                .ancestors
                .iter()
                .any(|ancestor| ancestor.snapshot.runtime_id == *old)
        {
            let old = old.clone();
            *at = old_focus_events_first(&mut self.waiting, *at, &old);
        }
        let waiting = Waiting {
            key,
            category,
            hwnd,
            raiser,
            marker,
            entry,
        };
        match place {
            Some(place) => self.waiting[place] = waiting,
            None => self.waiting.push_back(waiting),
        }
        pushed
    }

    /// The gap record's bound has been reached at `now_us`, on the
    /// observation clock ([`GapRecord::bound_reached`]): its entry waits
    /// behind every marker queued before it, so a UIA focus already being
    /// read decides first.
    pub(in crate::outpost) fn push_gap_bound(&mut self, entry: Entry) {
        let _ = self.push(entry, None);
    }

    /// A program dropped a copy in `window`, of top-level window
    /// `top_level`, observed at `at_us`, because it judges the window UIA
    /// ([`GapRecord::copy_dropped`]); with what the record's timer is to do.
    pub(in crate::outpost) fn gap_copy_dropped(
        &mut self,
        window: isize,
        top_level: isize,
        at_us: u64,
    ) -> (CopyDropped, TimerAction) {
        let was_open = self.gap.is_open();
        let dropped = self.gap.copy_dropped(window, top_level, at_us);
        let action = match dropped {
            CopyDropped::Opened { due_us } => TimerAction::Set(due_us),
            _ if was_open && !self.gap.is_open() => TimerAction::Cancel,
            _ => TimerAction::Keep,
        };
        (dropped, action)
    }

    /// A program accepted an event of `window`, of top-level window
    /// `top_level`, observed at `at_us`: a foreground change, which
    /// `restamps`, or a copy it handles through MSAA
    /// ([`GapRecord::accepted`]); with what the record's timer is to do.
    pub(in crate::outpost) fn gap_accepted(
        &mut self,
        window: isize,
        top_level: isize,
        at_us: u64,
        restamps: bool,
    ) -> TimerAction {
        match self.gap.accepted(window, top_level, at_us, restamps) {
            Accepted::Answered => TimerAction::Cancel,
            Accepted::Restamped | Accepted::Unchanged => TimerAction::Keep,
        }
    }

    /// The bound's entry is handled at `now_us`
    /// ([`GapRecord::bound_reached`]): the record's read, if its bound has
    /// passed, or the due time to set the timer to again for a record
    /// whose bound has not.
    pub(in crate::outpost) fn gap_bound_reached(
        &mut self,
        now_us: u64,
    ) -> Result<Option<GapRead>, u64> {
        self.gap.bound_reached(now_us)
    }

    /// `hwnd` was destroyed: its answer as a top-level window goes.
    pub(in crate::outpost) fn gap_window_destroyed(&mut self, hwnd: isize) {
        self.gap.window_destroyed(hwnd);
    }

    /// Whether the gap record is open, which the idle answer waits for.
    pub(in crate::outpost) fn gap_open(&self) -> bool {
        self.gap.is_open()
    }

    /// The next entry to handle and the number of its batch, planning a new
    /// batch from everything waiting when the current one is done, with
    /// `window_is_hung` telling a hung window, a local read. With nothing
    /// else ready it gives the entry [`when_idle`](Self::when_idle) kept,
    /// if any, and then [`Item::Wake`] once the time
    /// [`wake_at`](Self::wake_at) named has come by `now`. `None` when
    /// nothing is ready.
    pub(in crate::outpost) fn next(
        &mut self,
        now: Duration,
        window_is_hung: impl Fn(isize) -> bool,
    ) -> Option<(Planned, u64)> {
        if let Some(planned) = self.batch.pop_front() {
            return Some((planned, self.batch_number));
        }
        // Everything before the first marker still waiting for its read;
        // with a marker first, the reducer waits for the read. An idle
        // query ends its batch, so nothing that arrived after it is planned
        // with what arrived before.
        let ready = self
            .waiting
            .iter()
            .position(|waiting| {
                waiting.marker.is_some() || matches!(waiting.entry.item, Item::IdleQuery { .. })
            })
            .map_or(self.waiting.len(), |at| {
                if self.waiting[at].marker.is_some() {
                    at
                } else {
                    at + 1
                }
            });
        if ready > 0 {
            let mut waiting: Vec<Waiting> = self.waiting.drain(..ready).collect();
            caret_after_update(
                &mut waiting,
                self.marks.focused.as_ref(),
                self.marks.terminal,
            );
            let batch = plan(waiting, self.marks.focused.as_ref(), window_is_hung);
            self.batch = batch.into();
            self.batch_number += 1;
            if let Some(planned) = self.batch.pop_front() {
                return Some((planned, self.batch_number));
            }
        }
        if self.closed {
            return None;
        }
        if let Some(entry) = self.idle.take() {
            return Some((Planned::Run(entry), self.batch_number));
        }
        if self.wake.is_some_and(|wake| now >= wake) {
            self.wake = None;
            return Some((
                Planned::Run(Entry::own(Item::Wake, verbatim_model::TraceId::mint(), 0)),
                self.batch_number,
            ));
        }
        None
    }

    /// When the wake [`wake_at`](Self::wake_at) asked for is due, if one
    /// is.
    pub(in crate::outpost) fn wake(&self) -> Option<Duration> {
        self.wake
    }

    /// Closes the queue for the outpost's shutdown: everything waiting and
    /// the batch in progress are dropped, nothing pushed from now on is
    /// kept, and [`next`](Self::next) answers `None`. The parked objects of
    /// what was dropped are returned, for a worker to drop.
    pub(in crate::outpost) fn close(&mut self) -> Vec<u64> {
        self.closed = true;
        self.wake = None;
        let mut dropped: Vec<Entry> = self
            .waiting
            .drain(..)
            .map(|waiting| waiting.entry)
            .collect();
        for planned in self.batch.drain(..) {
            match planned {
                Planned::Run(entry) | Planned::Menu(entry) => dropped.push(entry),
                Planned::Focus(entries) => dropped.extend(entries),
            }
        }
        dropped.extend(self.idle.take());
        dropped
            .iter()
            .flat_map(|entry| entry.item.parked())
            .collect()
    }

    /// Asks [`next`](Self::next) for an [`Item::Wake`] at `at` if nothing
    /// else is to be done then, or for none, in place of any time asked for
    /// before.
    pub(in crate::outpost) fn wake_at(&mut self, at: Option<Duration>) {
        self.wake = at;
    }

    /// Withdraws query `request_id` if it has not started. Returns whether it
    /// was found.
    pub(in crate::outpost) fn cancel(&mut self, request_id: u64) -> bool {
        let is_it = |entry: &Entry| matches!(entry.item, Item::Query { request_id: id, .. } if id == request_id);
        let before = self.waiting.len() + self.batch.len();
        self.waiting.retain(|waiting| !is_it(&waiting.entry));
        self.batch
            .retain(|planned| !planned.entries().into_iter().any(is_it));
        before != self.waiting.len() + self.batch.len()
    }

    /// Whether [`next`](Self::next) has an entry to hand out without
    /// waiting: one planned, or one waiting ahead of every marker.
    #[cfg(test)]
    fn ready_now(&self) -> bool {
        !self.batch.is_empty()
            || self
                .waiting
                .front()
                .is_some_and(|waiting| waiting.marker.is_none())
    }

    /// Whether anything is waiting to be handled, planned or not, a UIA
    /// focus's marker waiting for its read included, and the entry kept for
    /// when nothing else is ready left out ([`take_idle`](Self::take_idle)).
    pub(in crate::outpost) fn busy(&self) -> bool {
        !self.waiting.is_empty() || !self.batch.is_empty()
    }

    /// Puts `entries` ahead of everything waiting, for the idle barrier's
    /// query the reducer has reached with a noted caret owed: the query
    /// ends its batch and nothing after it has been handled, so the query
    /// waits for that caret and for nothing that arrived after it.
    pub(in crate::outpost) fn push_ahead(&mut self, entries: impl IntoIterator<Item = Entry>) {
        if self.closed {
            return;
        }
        for (offset, entry) in entries.into_iter().enumerate() {
            let (key, category, hwnd) = classify(&entry.item);
            let raiser = raiser_of(&entry.item);
            self.waiting.insert(
                offset,
                Waiting {
                    key,
                    category,
                    hwnd,
                    raiser,
                    marker: None,
                    entry,
                },
            );
        }
    }

    /// Keeps `entry` to be handed out once nothing else is ready, in place
    /// of one kept before: a held terminal's noted caret is read then
    /// ([`Item::NotedCaret`]), when the burst of events that kept the
    /// outpost busy has ended, and not on every caret event of the burst.
    pub(in crate::outpost) fn when_idle(&mut self, entry: Entry) {
        self.idle = Some(entry);
    }

    /// Takes the entry [`when_idle`](Self::when_idle) kept, if any.
    pub(in crate::outpost) fn take_idle(&mut self) -> Option<Entry> {
        self.idle.take()
    }

    /// Drops the console updates of `window` as they are pushed, from now
    /// until the next call, or takes them in again with `None`. A program
    /// marks the window while its focused console terminal's reading is
    /// held with its text changed and no caret key's watch is open, when it
    /// would do nothing for an update; it clears the mark before it handles
    /// anything that changes that, so an update observed after the read
    /// that ends the hold begins is never dropped. A new focus clears it
    /// too ([`note_focus`](Self::note_focus)).
    pub(in crate::outpost) fn absorb_console_updates(&mut self, window: Option<isize>) {
        self.marks.absorbed = window;
    }

    /// Records the object last reported as the focus, in one go, as the
    /// commit that makes it followed does: its MSAA events other than focus
    /// events are always kept; a terminal read through UIA, in window
    /// `terminal` (0 when not known), has its caret event planned after its
    /// text changes; with `drop_system_caret`, the system caret's events
    /// are dropped, as for a console host's terminal, whose console caret
    /// event is its caret event; and `redirected` says it was reached by
    /// redirecting a control's own focus event to its focused child.
    pub(in crate::outpost) fn note_focus(
        &mut self,
        object: Option<Object>,
        terminal: Option<isize>,
        drop_system_caret: bool,
        redirected: bool,
    ) {
        self.marks = Marks {
            focused: object,
            terminal,
            focus_redirected: redirected,
            absorbed: None,
            system_caret_unused: drop_system_caret,
        };
    }

    /// The number of the batch being handed out.
    pub(in crate::outpost) fn batch_number(&self) -> u64 {
        self.batch_number
    }

    /// The caret events waiting, planned or not, observed at or after
    /// `since_ms`, in whole milliseconds, by what names their nodes, none
    /// with no `since_ms`; and every edit control's value change waiting.
    pub(in crate::outpost) fn waiting_carets(
        &self,
        since_ms: Option<u64>,
    ) -> Vec<super::WaitingCaret> {
        self.waiting
            .iter()
            .map(|waiting| &waiting.entry)
            .chain(self.batch.iter().flat_map(Planned::entries))
            .filter_map(|entry| {
                let waiting = super::waiting_caret(&entry.item)?;
                let caret = !matches!(waiting, super::WaitingCaret::ValueChange(_));
                let counted = !caret
                    || since_ms.is_some_and(|since_ms| entry.observed_at_us / 1_000 >= since_ms);
                counted.then_some(waiting)
            })
            .collect()
    }

    /// What the planning reads of the focus, which programs are given a
    /// copy of and the mailbox judges its drops by.
    #[cfg(test)]
    pub(in crate::outpost) fn marks(&self) -> &Marks {
        &self.marks
    }
}

/// Removes the entry waiting with `key` among those around index `at` that
/// no idle query divides from it, and returns its index. An idle query
/// splits what waits into stretches, each holding at most one entry per
/// key, so the first found is the only one.
fn remove_keyed(waiting: &mut VecDeque<Waiting>, key: &Key, at: usize) -> Option<(usize, Waiting)> {
    remove_waiting(waiting, at, |waiting| waiting.key.as_ref() == Some(key))
}

/// Moves each of the old focus `old`'s own UIA state changes that waits
/// behind the focus marker at `place`, before the next marker or idle
/// query, to just before that marker, in their order; the marker's new
/// place. Called when the marker's read is a focus inside `old`, so a state
/// change the provider raised before moving the focus into the old focus's
/// child, but which the outpost's own subscription queued after the
/// listener's marker, is handled before the focus. A state change raised
/// after the focus is moved too, since the intake cannot tell the two
/// apart, and is spoken either way, as NVDA speaks a state change of the
/// focus or of any of its ancestors. A name or value change keeps its
/// place, as NVDA speaks one only for the focus itself, so one raised after
/// the focus goes unsaid. Other events of the old focus, events of other
/// elements and the listener's facts keep their places.
fn old_focus_events_first(waiting: &mut VecDeque<Waiting>, place: usize, old: &[i32]) -> usize {
    let end = waiting
        .iter()
        .skip(place + 1)
        .position(|waiting| {
            waiting.marker.is_some() || matches!(waiting.entry.item, Item::IdleQuery { .. })
        })
        .map_or(waiting.len(), |at| place + 1 + at);
    let mut place = place;
    for at in place + 1..end {
        // The outpost's own events only: the listener's selections and
        // notifications share their keys, but arrive in the listener's
        // order already.
        let of_old = matches!(
            &waiting[at].entry.item,
            Item::Uia(event)
                if matches!(event.kind, UiaKind::Property(id) if is_state_property(id))
                    && event.parts.runtime_id.as_slice() == old
        );
        if of_old && let Some(moved) = waiting.remove(at) {
            // Removing it and inserting it ahead leaves every later index
            // where it was.
            waiting.insert(place, moved);
            place += 1;
        }
    }
    place
}

/// Removes the focus event waiting on window `window`'s client object
/// from thread `raiser`, among those no idle query divides from the back,
/// for a foreground change of the window raised as `change` says
/// ([`Intake::push`]), unless the focus was raised after the change
/// ([`focus_raised_first`]).
fn remove_client_focus(
    waiting: &mut VecDeque<Waiting>,
    window: isize,
    raiser: u32,
    change: Option<WinEventStamp>,
) {
    let client = Some(Key::MsaaFocus(window, OBJID_CLIENT, 0, raiser));
    let at = waiting.len();
    let _ = remove_waiting(waiting, at, |waiting| {
        waiting.key == client && focus_raised_first(stamp_of(waiting), change, true)
    });
}

/// Whether a focus on a window's client object, raised as `focus` says,
/// was raised before the window's foreground change raised as `change`
/// says, so that NVDA's limiter, adding events in the order they were
/// raised, removes it as it adds the change. `focus_arrived_first` says
/// which reached the intake first, which in one tick of the clock counts
/// as raised first: the hook's copies arrive in the order raised, and the
/// listener's copies are handled as they come, before the hook's held
/// copies are released ([`super::hold`]). Without both times, which only a
/// test hands over, the one that arrived first counts as raised first.
fn focus_raised_first(
    focus: Option<WinEventStamp>,
    change: Option<WinEventStamp>,
    focus_arrived_first: bool,
) -> bool {
    let (Some(focus), Some(change)) = (focus, change) else {
        return focus_arrived_first;
    };
    if focus.time != change.time {
        return focus.raised_before(change.time);
    }
    focus_arrived_first
}

/// The time a waiting `WinEvent` fact was raised, if it carries it.
fn stamp_of(waiting: &Waiting) -> Option<WinEventStamp> {
    match &waiting.entry.item {
        Item::Fact(_, stamp) => *stamp,
        _ => None,
    }
}

/// Removes the first entry for which `matches` holds among those around
/// index `at` that no idle query divides from it, nearest before `at`
/// first, and returns its index with the entry removed.
fn remove_waiting(
    waiting: &mut VecDeque<Waiting>,
    at: usize,
    matches: impl Fn(&Waiting) -> bool,
) -> Option<(usize, Waiting)> {
    let is_query = |waiting: &Waiting| matches!(waiting.entry.item, Item::IdleQuery { .. });
    let found = waiting
        .range(..at)
        .rev()
        .take_while(|waiting| !is_query(waiting))
        .position(&matches)
        .map(|back| at - 1 - back)
        .or_else(|| {
            waiting
                .range(at..)
                .take_while(|waiting| !is_query(waiting))
                .position(&matches)
                .map(|ahead| at + ahead)
        })?;
    waiting.remove(found).map(|removed| (found, removed))
}

/// The window an item concerns, 0 for none.
pub(in crate::outpost) fn window_of(item: &Item) -> isize {
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
            raiser,
        } => match kind {
            WinKind::Destroy => (None, Category::Exempt, *hwnd),
            // Each console update names the cells it changed, and each
            // console caret event the cell its cursor is in; only that the
            // window's text changed, or its cursor moved, matters, as both
            // are read, so each kind merges into one per window while they
            // wait.
            WinKind::ConsoleUpdate | WinKind::ConsoleCaret => (
                Some(Key::Msaa(*kind as u8, *hwnd, 0, 0, 0)),
                Category::Other,
                *hwnd,
            ),
            _ => (
                Some(Key::Msaa(
                    *kind as u8,
                    *hwnd,
                    *id_object,
                    *id_child,
                    *raiser,
                )),
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
        Item::Fact(fact, _) => classify_fact(fact, raiser_of(item)),
        // Only the newest list of held nodes matters.
        Item::NodesHeld { .. } => (Some(Key::NodesHeld), Category::Exempt, 0),
        Item::Query { .. }
        | Item::NotedCaret { .. }
        | Item::Settle(_)
        | Item::IdleQuery { .. }
        | Item::Wake
        | Item::GapBound
        | Item::GapRead { .. } => (None, Category::Exempt, 0),
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
        (window != 0).then_some(Key::Msaa(WinKind::ConsoleCaret as u8, window, 0, 0, 0));
    let Some(at) = waiting.iter().position(|waiting| {
        let key = waiting.key.as_ref();
        key == Some(&uia_caret) || (key.is_some() && key == console_caret.as_ref())
    }) else {
        return;
    };
    let text_changed = Key::Uia(3, 0, runtime_id.clone());
    let console_update =
        (window != 0).then_some(Key::Msaa(WinKind::ConsoleUpdate as u8, window, 0, 0, 0));
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
fn notification_key(runtime_id: &[i32], notification: &verbatim_model::Notification) -> Key {
    Key::UiaNotification(
        runtime_id.to_vec(),
        notification.kind,
        notification.processing,
        notification.activity_id.clone(),
    )
}

/// The key, category, and window of a fact, from the listener or the
/// outpost's own hook, raised by `thread` ([`thread_of`]).
fn classify_fact(fact: &DeliveredFact, raiser: u32) -> (Option<Key>, Category, isize) {
    match fact {
        // Unwrapped as it is pushed (`Intake::push`); as itself otherwise.
        DeliveredFact::UiaMarked { fact, .. } => classify_fact(fact, raiser),
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
                WinKind::Show
            } else {
                WinKind::Alert
            };
            let key = Key::Msaa(kind as u8, *hwnd, *id_object, *id_child, raiser);
            (Some(key), Category::Other, *hwnd)
        }
        DeliveredFact::Foreground { hwnd } => {
            (Some(Key::Foreground(*hwnd, raiser)), Category::Focus, *hwnd)
        }
        DeliveredFact::MsaaFocus {
            hwnd,
            id_object,
            id_child,
        } => (
            Some(Key::MsaaFocus(*hwnd, *id_object, *id_child, raiser)),
            Category::Focus,
            *hwnd,
        ),
        // Its own window, 0 for an element that is not a window, so a
        // windowless focus is never tested as hung, as NVDA's hung-window
        // test passes a UIA event whose sender's cached window is 0
        // (`docs/parity/focus-events.md`).
        DeliveredFact::UiaFocusRead { read, .. } => (
            Some(Key::UiaFocus(read.snapshot.runtime_id.clone())),
            Category::Uncounted,
            read.hwnd,
        ),
        // A marker is never planned: its read takes its place, or a drop
        // removes it. A drop, which becomes the gap record's read when it
        // takes the marker's place, and the end of the listener's reads are
        // never queued.
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
fn raiser_of(item: &Item) -> u32 {
    match item {
        Item::Msaa { raiser, .. } => *raiser,
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
            Some(Key::Msaa(..) | Key::MsaaFocus(..) | Key::Foreground(..) | Key::MenuPopup(..))
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
                if matches!(item.key, Some(Key::Foreground(..))) {
                    // A foreground change is also one of its thread's other
                    // events, as NVDA's limiter caches a foreground event
                    // with both its focus events and its other events
                    // (`orderedWinEventLimiter.py`, `addEvent`), so newer
                    // focus events do not crowd it out.
                    let count = per_thread.entry(item.raiser).or_default();
                    *count += 1;
                    among_focus || *count <= EVENTS_PER_THREAD
                } else {
                    among_focus
                }
            }
            Category::Other => {
                // The focused object's events count, and are kept beyond
                // the limit, as NVDA's `alwaysAllowedObjects`.
                let count = per_thread.entry(item.raiser).or_default();
                *count += 1;
                *count <= EVENTS_PER_THREAD || of_focus
            }
        };
    }

    waiting
        .into_iter()
        .zip(keep)
        .filter_map(|(item, keep)| keep.then_some(item))
        .collect()
}

/// Plans one batch from everything that was waiting, in the order it
/// arrived: drops events from hung windows, applies the batch limits, tries
/// each consecutive run of MSAA focus events and foreground changes newest
/// first at the place of its newest ([`try_newest_first`]), a run ended
/// only by an MSAA event of NVDA's pump ([`ends_a_run`]), handles every
/// other entry, each UIA focus event among them, at its own place, and
/// moves the newest menu opening from each backend to the end, ahead of an
/// idle query that ends the batch.
///
/// NVDA's `pumpAll` walks the flushed events in order, collecting focus and
/// foreground events, and when another accepted event comes processes
/// those collected newest first, stopping at the first valid one, before
/// that event.
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
    let msaa_menu = newest(|key| matches!(key, Key::MenuPopup(..)));
    // Each consecutive run of MSAA focus events and foreground changes,
    // oldest first, by where its newest is: an MSAA event of NVDA's pump
    // ends a run ([`ends_a_run`]), and every other entry keeps its place
    // without ending it.
    let mut runs: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut run: Vec<usize> = Vec::new();
    for (index, item) in kept.iter().enumerate() {
        match &item.key {
            Some(Key::MsaaFocus(..) | Key::Foreground(..)) => run.push(index),
            key if ends_a_run(key.as_ref()) => {
                if let Some(&last) = run.last() {
                    runs.insert(last, std::mem::take(&mut run));
                }
            }
            _ => {}
        }
    }
    if let Some(&last) = run.last() {
        runs.insert(last, run);
    }
    for run in runs.values_mut() {
        in_raised_order(&kept, run);
    }

    let (keys, mut entries): (Vec<Option<Key>>, Vec<Option<Entry>>) = kept
        .into_iter()
        .map(|item| (item.key, Some(item.entry)))
        .unzip();
    let mut planned = Vec::with_capacity(entries.len());
    let mut deferred = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        if matches!(key, Some(Key::MsaaFocus(..) | Key::Foreground(..))) {
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

/// Whether an entry keyed `key` ends a run of MSAA focus events and
/// foreground changes: an MSAA event NVDA's `pumpAll` receives, as only
/// the next accepted event of its pump that is neither a focus nor a
/// foreground event ends its run. Entries NVDA's pump never receives
/// neither join a run nor end it, and are handled at their own places:
/// UIA events and focus reads, which NVDA's UIA handler queues on its own
/// path; Core's queries and list of held nodes, and the outpost's settle
/// request,
/// which have no counterpart; an
/// object destroyed, which NVDA's event callback handles as it arrives;
/// the console host's updates and caret, which NVDA's console support
/// hooks apart; and a text control's selection change, which NVDA does not
/// hook. Whether the worker's acceptance test will drop an MSAA event,
/// which NVDA's pump filters out before its run, is known only against the
/// foreground window when it is handled, so such an event ends the run.
fn ends_a_run(key: Option<&Key>) -> bool {
    match key {
        Some(Key::Msaa(kind, ..)) => ![
            WinKind::ConsoleUpdate as u8,
            WinKind::ConsoleCaret as u8,
            WinKind::TextSelectionChange as u8,
        ]
        .contains(kind),
        Some(Key::MenuPopup(..)) => true,
        _ => false,
    }
}

/// Puts `run`, indexes into `kept` in the order they arrived, in the order
/// Windows raised its events, when every one carries the time it was
/// raised: NVDA's one hook receives a run's events in that order, while an
/// outpost can receive the focus listener's copy of an event after its own
/// hook's copy of a later one. The sort is stable, and a run with an event
/// of unknown time keeps the order it arrived in.
///
/// The clock advances in ticks of about 15.6 milliseconds, so two events
/// raised a few milliseconds apart often carry the same time; those keep
/// the order they arrived in. The hook's copies arrive in the order raised,
/// and while the listener forwards, the hook's copies are held and released
/// behind every listener copy handled ([`super::hold`]), so a listener copy
/// raised in the same tick as a hook copy is ahead of it.
fn in_raised_order(kept: &[Waiting], run: &mut [usize]) {
    let raised = |at: usize| match &kept[at].entry.item {
        Item::Fact(_, Some(stamp)) => Some(stamp.time),
        _ => None,
    };
    let Some(first) = run.first().and_then(|&at| raised(at)) else {
        return;
    };
    if run.iter().any(|&at| raised(at).is_none()) {
        return;
    }
    // Relative to the first, so the clock's wrap after 49.7 days orders
    // a run raised across it.
    run.sort_by_key(|&at| raised(at).map_or(0, |time| time.wrapping_sub(first).cast_signed()));
}

#[cfg(test)]
mod tests {
    use verbatim_model::protocol::{Query, UiaSnapshotFact};
    use verbatim_model::{Notification, TraceId};

    use super::super::entry::UiaEvent;
    use super::*;

    /// An MSAA event on child `child` of window `hwnd`, raised by `thread`
    /// and observed at `child` milliseconds.
    fn msaa_from(kind: WinKind, hwnd: isize, child: i32, thread: u32) -> Waiting {
        let item = Item::Msaa {
            kind,
            hwnd,
            id_object: -4,
            id_child: child,
            raiser: thread,
        };
        let (key, category, hwnd) = classify(&item);
        Waiting {
            key,
            category,
            hwnd,
            raiser: raiser_of(&item),
            marker: None,
            entry: Entry {
                item,
                trace: TraceId::mint(),
                observed_at_us: u64::try_from(child).unwrap_or(0),
                timing: verbatim_model::protocol::EventTiming::default(),
            },
        }
    }

    /// An MSAA event raised by a thread numbered as its window is.
    fn msaa(kind: WinKind, hwnd: isize, child: i32) -> Waiting {
        msaa_from(kind, hwnd, child, u32::try_from(hwnd).unwrap_or(0))
    }

    fn fact(fact: DeliveredFact, observed_at_us: u64) -> Waiting {
        let item = Item::Fact(fact, None);
        let (key, category, hwnd) = classify(&item);
        Waiting {
            key,
            category,
            hwnd,
            raiser: 1,
            marker: None,
            entry: Entry {
                item,
                trace: TraceId::mint(),
                observed_at_us,
                timing: verbatim_model::protocol::EventTiming::default(),
            },
        }
    }

    /// An MSAA focus on child `child` of window 9.
    fn msaa_focus(child: i32, observed_at_us: u64) -> Waiting {
        fact(
            DeliveredFact::MsaaFocus {
                hwnd: 9,
                id_object: -4,
                id_child: child,
            },
            observed_at_us,
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
            read: Box::new(verbatim_model::protocol::UiaFocusRead {
                hwnd,
                snapshot: snapshot(runtime_id),
                ancestors: Vec::new(),
                ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
                selected_child: None,
                value_from_selection: false,
                window: Some(hwnd),
                same_as_last: None,
                bound_us: 2_000_000,
            }),
            focus: true,
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
            raiser: 0,
            marker: None,
            entry: Entry {
                item,
                trace: TraceId::mint(),
                observed_at_us: 0,
                timing: verbatim_model::protocol::EventTiming::default(),
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
                    .map(|entry| entry.observed_at_us)
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
                    Some(entries.iter().map(|entry| entry.observed_at_us).collect())
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
                raiser: 0,
            })
            .0
        };
        // Console updates name the cells they changed; one per window waits.
        assert_eq!(
            key(WinKind::ConsoleUpdate, 3, 65),
            key(WinKind::ConsoleUpdate, 9, 66)
        );
        // Other events keep their objects apart.
        assert_ne!(
            key(WinKind::ValueChange, 3, 0),
            key(WinKind::ValueChange, 9, 0)
        );
    }

    #[test]
    fn a_batch_keeps_the_newest_ten_events_per_thread() {
        let mut waiting: Vec<Waiting> = (1..=12)
            .map(|child| msaa(WinKind::ValueChange, 7, child))
            .collect();
        waiting.push(msaa(WinKind::ValueChange, 8, 50));
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
        let mut waiting = vec![msaa_from(WinKind::ConsoleUpdate, 7, 1, 80)];
        waiting.extend((2..=11).map(|child| msaa_from(WinKind::ValueChange, 7, child, 70)));
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
        assert_eq!(raiser_of(&alert), 80);
        let unstamped = Item::Fact(DeliveredFact::Foreground { hwnd: 7 }, None);
        assert_eq!(raiser_of(&unstamped), 0);
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
            .map(|item| item.entry.observed_at_us)
            .collect();
        // Four newer focus events fill the focus limit; the foreground
        // change is kept as one of its thread's other events, and the
        // oldest focus is dropped.
        assert_eq!(kept, vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn the_focused_objects_events_and_queries_are_always_kept() {
        let mut waiting = vec![msaa(WinKind::NameChange, 7, 1), query(1)];
        waiting.extend((2..=12).map(|child| msaa(WinKind::ValueChange, 7, child)));
        waiting.push(msaa(WinKind::ValueChange, 7, 1));
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
        let mut intake = Intake::default();
        push_item(&mut intake, notification_fact("progress", "10 percent"), 10);
        push_item(&mut intake, notification_fact("other", "saved"), 20);
        push_item(&mut intake, notification_fact("progress", "20 percent"), 30);
        let mut order = Vec::new();
        for _ in 0..2 {
            if let Some((Planned::Run(entry), _)) = intake.next(Duration::ZERO, never_hung) {
                order.push(entry.observed_at_us);
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
            msaa(WinKind::ValueChange, 9, 1),
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
            vec![4, 2, 5, 1],
            "the MSAA focus run, with its foreground change, and the UIA focus are both handled, so the backend that owns the window reports"
        );
        assert!(matches!(planned.last(), Some(Planned::Menu(_))));
    }

    #[test]
    fn every_uia_focus_of_a_batch_is_handled_oldest_first() {
        // Control+Tab in Windows Terminal: the tab the focus leaves, the tab
        // it moves to, and that tab's terminal, in one batch.
        let uia = |runtime_id: i32, observed_at_us: u64| {
            fact(uia_focus(0, 3, vec![42, runtime_id]), observed_at_us)
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
            msaa(WinKind::NameChange, 3, 30),
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

    /// The entries of `planned` the worker handles, in order, through
    /// [`Planned::handle`], when only those observed at a time `reported`
    /// names are reported: a focus group stops at the first reported.
    /// The entries of `planned` in the order they are tried, each focus
    /// group's newest first until one of `reported`, as the reducer tries
    /// them (`super::super::Reducer`).
    fn tried(planned: Vec<Planned>, reported: &[u64]) -> Vec<u64> {
        let mut order = Vec::new();
        for planned in planned {
            match planned {
                Planned::Run(entry) | Planned::Menu(entry) => order.push(entry.observed_at_us),
                Planned::Focus(entries) => {
                    for entry in entries {
                        order.push(entry.observed_at_us);
                        if reported.contains(&entry.observed_at_us) {
                            break;
                        }
                    }
                }
            }
        }
        order
    }

    /// Every foreground change of a batch joins the run it is beside, an
    /// older one among them: a run of two focus events and two foreground
    /// changes of different windows is one group, tried newest first.
    #[test]
    fn an_older_foreground_change_joins_its_run() {
        let waiting = vec![
            msaa_focus(1, 1),
            fact(DeliveredFact::Foreground { hwnd: 9 }, 2),
            msaa_focus(3, 3),
            fact(DeliveredFact::Foreground { hwnd: 8 }, 4),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![4, 3, 2, 1]]);
        assert_eq!(observed(&planned), vec![4, 3, 2, 1, 30]);
    }

    /// A run's events in the order Windows raised them when each carries
    /// the time it was raised: a foreground change raised before a focus
    /// but arriving after it, as the focus listener's copy can arrive after
    /// the outpost's own hook's during a hand-over, is the older, so the
    /// focus is tried first.
    #[test]
    fn a_run_is_tried_in_the_order_its_events_were_raised() {
        let stamped = |mut waiting: Waiting, time: u32| {
            if let Item::Fact(_, stamp) = &mut waiting.entry.item {
                *stamp = Some(WinEventStamp { thread: 1, time });
            }
            waiting
        };
        let waiting = vec![
            stamped(msaa_focus(1, 1), 200),
            stamped(fact(DeliveredFact::Foreground { hwnd: 9 }, 2), 196),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![1, 2]]);
        assert_eq!(observed(&planned), vec![1, 2, 30]);
    }

    /// NVDA's limiter removes a waiting focus event on a window's client
    /// object, from the same thread, when the window's foreground event is
    /// added: the focus is gone, and the foreground change stays.
    #[test]
    fn a_foreground_change_removes_a_waiting_focus_on_its_windows_client_object() {
        let mut intake = Intake::default();
        let stamp = Some(WinEventStamp {
            thread: 7,
            time: 200,
        });
        push_item(
            &mut intake,
            Item::Fact(
                DeliveredFact::MsaaFocus {
                    hwnd: 9,
                    id_object: OBJID_CLIENT,
                    id_child: 0,
                },
                stamp,
            ),
            1,
        );
        push_item(
            &mut intake,
            Item::Fact(DeliveredFact::Foreground { hwnd: 9 }, stamp),
            2,
        );
        assert_eq!(handed_out(&mut intake), vec![2]);
    }

    /// A focus on the window's client object raised a tick after the
    /// window's foreground change but arriving first, as the hook's copy can
    /// before the listener's copy of a change raised earlier: NVDA's
    /// limiter adds the change first, so it removes nothing, and both are
    /// kept, the focus tried first as the newer. A change of the same tick
    /// that arrives first, as the listener's copies do ahead of the hook's
    /// held ones, removes nothing either, since the focus comes after it.
    #[test]
    fn a_foreground_change_keeps_a_focus_raised_after_it() {
        let stamp = |time| Some(WinEventStamp { thread: 7, time });
        let client_focus = || DeliveredFact::MsaaFocus {
            hwnd: 9,
            id_object: OBJID_CLIENT,
            id_child: 0,
        };
        let mut intake = Intake::default();
        push_item(&mut intake, Item::Fact(client_focus(), stamp(200)), 1);
        push_item(
            &mut intake,
            Item::Fact(DeliveredFact::Foreground { hwnd: 9 }, stamp(184)),
            2,
        );
        assert_eq!(handed_out(&mut intake), vec![1, 2], "a tick earlier");
        let mut intake = Intake::default();
        push_item(
            &mut intake,
            Item::Fact(DeliveredFact::Foreground { hwnd: 9 }, stamp(200)),
            2,
        );
        push_item(&mut intake, Item::Fact(client_focus(), stamp(200)), 3);
        assert_eq!(handed_out(&mut intake), vec![3, 2], "the same tick");
    }

    /// Two foreground changes in one run, the newer dropped, as when its
    /// window is not yet in front as it is handled: the older is tried
    /// next, and once reported ends the run's tries.
    #[test]
    fn an_older_foreground_change_is_tried_when_the_newer_is_dropped() {
        let waiting = vec![
            fact(DeliveredFact::Foreground { hwnd: 9 }, 1),
            fact(DeliveredFact::Foreground { hwnd: 8 }, 2),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![2, 1]]);
        assert_eq!(tried(planned, &[1, 30]), vec![2, 1, 30]);
    }

    #[test]
    fn a_foreground_change_newest_in_a_run_is_tried_first_and_ends_the_run_when_reported() {
        let waiting = vec![
            msaa_focus(1, 1),
            fact(DeliveredFact::Foreground { hwnd: 9 }, 2),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![2, 1]]);
        assert_eq!(tried(planned, &[1, 2, 30]), vec![2, 30]);
    }

    #[test]
    fn a_dropped_foreground_change_newest_in_a_run_lets_the_older_focus_be_tried() {
        let waiting = vec![
            msaa_focus(1, 1),
            fact(DeliveredFact::Foreground { hwnd: 9 }, 2),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(tried(planned, &[1, 30]), vec![2, 1, 30]);
    }

    #[test]
    fn a_focus_newest_in_a_run_is_tried_before_the_older_foreground_change() {
        let waiting = vec![
            msaa_focus(1, 1),
            fact(DeliveredFact::Foreground { hwnd: 9 }, 2),
            msaa_focus(3, 3),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![3, 2, 1]]);
        assert_eq!(tried(planned, &[1, 2, 3, 30]), vec![3, 30]);
    }

    #[test]
    fn a_foreground_change_with_no_focus_event_beside_it_is_tried_alone_at_its_place() {
        let waiting = vec![
            msaa_focus(1, 1),
            msaa(WinKind::NameChange, 3, 30),
            fact(DeliveredFact::Foreground { hwnd: 9 }, 2),
            msaa(WinKind::NameChange, 3, 40),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![1], vec![2]]);
        assert_eq!(tried(planned, &[]), vec![1, 30, 2, 40]);
    }

    /// Core's housekeeping and queries, which NVDA's `WinEvent` stream never
    /// holds, neither join nor end a run: they are handled at their places,
    /// before the run, and the run, its foreground change newest, is tried
    /// as one.
    #[test]
    fn core_entries_between_two_focus_facts_neither_join_nor_end_their_run() {
        let held = Item::NodesHeld {
            nodes: Vec::new(),
            anchors: Vec::new(),
            acknowledged: 0,
        };
        let (key, category, hwnd) = classify(&held);
        let held = Waiting {
            key,
            category,
            hwnd,
            raiser: 0,
            marker: None,
            entry: Entry {
                item: held,
                trace: TraceId::mint(),
                observed_at_us: 2,
                timing: verbatim_model::protocol::EventTiming::default(),
            },
        };
        let waiting = vec![
            msaa_focus(1, 1),
            held,
            query(1),
            fact(DeliveredFact::Foreground { hwnd: 9 }, 3),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![3, 1]]);
        assert_eq!(tried(planned, &[3]), vec![2, 0, 3, 30]);
    }

    /// A UIA event, which NVDA's MSAA pump never sees, neither joins nor
    /// ends a run: it is handled at its place, before the run.
    #[test]
    fn a_uia_event_between_two_focus_facts_neither_joins_nor_ends_their_run() {
        let waiting = vec![
            msaa_focus(1, 1),
            fact(
                DeliveredFact::UiaSelection {
                    hwnd: 3,
                    focus_window: 0,
                    snapshot: snapshot(vec![42]),
                },
                2,
            ),
            msaa_focus(3, 3),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![3, 1]]);
        assert_eq!(observed(&planned), vec![2, 3, 1, 30]);
    }

    /// A UIA focus read between two focus facts of a run is handled on its
    /// own at its place, before the run, which it does not end.
    #[test]
    fn a_uia_focus_between_two_focus_facts_is_handled_before_their_run() {
        let waiting = vec![
            msaa_focus(1, 1),
            fact(uia_focus(0, 3, vec![42]), 2),
            msaa_focus(3, 3),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![2], vec![3, 1]]);
        assert_eq!(observed(&planned), vec![2, 3, 1, 30]);
    }

    /// The MSAA events NVDA's pump never receives neither join nor end a
    /// run: an object destroyed, which NVDA's callback handles as it
    /// arrives, the console host's update and caret, which its console
    /// support hooks apart, and a text control's selection change, which it does not
    /// hook.
    #[test]
    fn msaa_events_outside_nvdas_pump_neither_join_nor_end_a_run() {
        let waiting = vec![
            msaa_focus(1, 1),
            msaa(WinKind::Destroy, 3, 20),
            msaa(WinKind::ConsoleUpdate, 4, 21),
            msaa(WinKind::TextSelectionChange, 5, 22),
            msaa(WinKind::ConsoleCaret, 6, 23),
            msaa_focus(3, 3),
            msaa(WinKind::NameChange, 3, 30),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![3, 1]]);
        assert_eq!(observed(&planned), vec![20, 21, 22, 23, 3, 1, 30]);
    }

    /// An MSAA event of NVDA's pump that is neither a focus nor a
    /// foreground change ends a run, a menu opening, which still goes
    /// last, and a caret event among them.
    #[test]
    fn an_msaa_event_of_another_kind_ends_a_run() {
        let waiting = vec![
            msaa_focus(1, 1),
            fact(
                DeliveredFact::MenuPopup {
                    hwnd: 3,
                    id_object: -4,
                    id_child: 0,
                },
                2,
            ),
            msaa_focus(3, 3),
            msaa(WinKind::Caret, 3, 40),
            msaa_focus(5, 5),
        ];
        let planned = plan(waiting, None, never_hung);
        assert_eq!(focus_groups(&planned), vec![vec![1], vec![3], vec![5]]);
        assert_eq!(observed(&planned), vec![1, 3, 40, 5, 2]);
    }

    #[test]
    fn a_focus_change_is_handled_in_the_order_it_arrived() {
        // In one batch, after the events of other objects that came first.
        let waiting = vec![
            msaa(WinKind::NameChange, 7, 1),
            msaa(WinKind::ValueChange, 8, 2),
            msaa_focus(3, 3),
            msaa(WinKind::NameChange, 7, 4),
        ];
        assert_eq!(observed(&plan(waiting, None, never_hung)), vec![1, 2, 3, 4]);
        // Arriving while a batch is in progress, after that batch's events
        // that have not started.
        let mut intake = Intake::default();
        let push = |intake: &mut Intake, item: Item, observed_at_us: u64| {
            intake.push(
                Entry {
                    item,
                    trace: TraceId::mint(),
                    observed_at_us,
                    timing: verbatim_model::protocol::EventTiming::default(),
                },
                None,
            );
        };
        let name_change = |child: i32| Item::Msaa {
            kind: WinKind::NameChange,
            hwnd: 0,
            id_object: -4,
            id_child: child,
            raiser: 0,
        };
        push(&mut intake, name_change(1), 1);
        push(&mut intake, name_change(2), 2);
        push(&mut intake, name_change(3), 3);
        let mut order = Vec::new();
        let (first, _) = intake.next(Duration::ZERO, never_hung).expect("an entry");
        order.extend(first.entries().iter().map(|entry| entry.observed_at_us));
        push(
            &mut intake,
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
            let (planned, _) = intake.next(Duration::ZERO, never_hung).expect("an entry");
            order.extend(planned.entries().iter().map(|entry| entry.observed_at_us));
        }
        assert_eq!(order, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_newer_event_for_the_same_object_replaces_the_waiting_one() {
        let mut intake = Intake::default();
        let push = |intake: &mut Intake, child: i32, observed_at_us: u64| {
            intake.push(
                Entry {
                    item: Item::Msaa {
                        kind: WinKind::ValueChange,
                        hwnd: 0,
                        id_object: -4,
                        id_child: child,
                        raiser: 0,
                    },
                    trace: TraceId::mint(),
                    observed_at_us,
                    timing: verbatim_model::protocol::EventTiming::default(),
                },
                None,
            );
        };
        push(&mut intake, 1, 10);
        push(&mut intake, 2, 20);
        push(&mut intake, 1, 30);
        let mut order = Vec::new();
        for _ in 0..2 {
            if let Some((Planned::Run(entry), _)) = intake.next(Duration::ZERO, never_hung) {
                order.push(entry.observed_at_us);
            }
        }
        assert_eq!(order, vec![20, 30]);
    }

    /// Value changes of one address from threads 7, 8 and 7 again: thread
    /// 7's second replaces its first, and thread 8's, keyed by its own
    /// thread as NVDA's limiter keys it, is neither replaced nor replaces.
    #[test]
    fn an_msaa_event_replaces_only_the_one_waiting_from_its_own_thread() {
        let mut intake = Intake::default();
        let push = |intake: &mut Intake, thread: u32, observed_at_us: u64| {
            intake.push(
                Entry {
                    item: Item::Msaa {
                        kind: WinKind::ValueChange,
                        hwnd: 9,
                        id_object: -4,
                        id_child: 1,
                        raiser: thread,
                    },
                    trace: TraceId::mint(),
                    observed_at_us,
                    timing: verbatim_model::protocol::EventTiming::default(),
                },
                None,
            );
        };
        push(&mut intake, 7, 1);
        push(&mut intake, 8, 2);
        push(&mut intake, 7, 3);
        assert_eq!(handed_out(&mut intake), vec![2, 3]);
    }

    /// Focus events on one address from threads 7, 8 and 7 again: thread
    /// 7's second replaces its first, and thread 8's, keyed by its own
    /// thread as NVDA's limiter keys a focus, is neither replaced nor
    /// replaces, so both threads' focuses are tried in their run, newest
    /// raised first.
    #[test]
    fn an_msaa_focus_replaces_only_the_one_waiting_from_its_own_thread() {
        let mut intake = Intake::default();
        let push = |intake: &mut Intake, thread: u32, observed_at_us: u64| {
            intake.push(
                Entry {
                    item: Item::Fact(
                        DeliveredFact::MsaaFocus {
                            hwnd: 9,
                            id_object: -4,
                            id_child: 1,
                        },
                        Some(WinEventStamp {
                            thread,
                            time: u32::try_from(observed_at_us).unwrap_or(0),
                        }),
                    ),
                    trace: TraceId::mint(),
                    observed_at_us,
                    timing: verbatim_model::protocol::EventTiming::default(),
                },
                None,
            );
        };
        push(&mut intake, 7, 1);
        push(&mut intake, 8, 2);
        push(&mut intake, 7, 3);
        assert_eq!(handed_out(&mut intake), vec![3, 2]);
    }

    #[test]
    fn a_newer_list_of_held_nodes_replaces_the_waiting_one_and_is_never_limited() {
        let mut intake = Intake::default();
        for acknowledged in [1, 2] {
            intake.push(
                Entry {
                    item: Item::NodesHeld {
                        nodes: Vec::new(),
                        anchors: Vec::new(),
                        acknowledged,
                    },
                    trace: TraceId::mint(),
                    observed_at_us: 0,
                    timing: verbatim_model::protocol::EventTiming::default(),
                },
                None,
            );
        }
        let Some((Planned::Run(entry), _)) = intake.next(Duration::ZERO, never_hung) else {
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

    fn push_fact(intake: &mut Intake, fact: DeliveredFact) {
        intake.push(
            Entry {
                item: Item::Fact(fact, None),
                trace: TraceId::mint(),
                observed_at_us: 0,
                timing: verbatim_model::protocol::EventTiming::default(),
            },
            None,
        );
    }

    #[test]
    fn a_batch_holding_a_foreground_change_is_handed_out_at_once() {
        // Window 77 is not the foreground window, nor any window at all:
        // the batch is not held for it, and the worker judges the change.
        let mut intake = Intake::default();
        push_fact(
            &mut intake,
            DeliveredFact::MsaaFocus {
                hwnd: 78,
                id_object: -4,
                id_child: 1,
            },
        );
        push_fact(&mut intake, DeliveredFact::Foreground { hwnd: 77 });
        let (planned, _) = intake.next(Duration::ZERO, never_hung).expect("an entry");
        assert_eq!(
            planned.entries().len(),
            2,
            "the focus and the foreground change, one run"
        );
        assert!(!intake.busy(), "nothing is held");
    }

    #[test]
    fn a_query_that_has_not_started_can_be_cancelled() {
        let mut intake = Intake::default();
        intake.push(
            Entry {
                item: Item::Query {
                    request_id: 5,
                    query: Query::DumpTree { locations: false },
                },
                trace: TraceId::mint(),
                observed_at_us: 0,
                timing: verbatim_model::protocol::EventTiming::default(),
            },
            None,
        );
        assert!(intake.cancel(5));
        assert!(!intake.cancel(5), "already withdrawn");
    }

    fn push_item(intake: &mut Intake, item: Item, observed_at_us: u64) {
        intake.push(
            Entry {
                item,
                trace: TraceId::mint(),
                observed_at_us,
                timing: verbatim_model::protocol::EventTiming::default(),
            },
            None,
        );
    }

    fn name_change(child: i32) -> Item {
        Item::Msaa {
            kind: WinKind::NameChange,
            hwnd: 0,
            id_object: -4,
            id_child: child,
            raiser: 0,
        }
    }

    fn marker(marker: u64) -> Item {
        Item::Fact(DeliveredFact::UiaFocusMarker { marker }, None)
    }

    /// Everything the intake hands out while it has something ready, by
    /// observation time.
    fn handed_out(intake: &mut Intake) -> Vec<u64> {
        let mut order = Vec::new();
        while intake.ready_now() {
            let (planned, _) = intake.next(Duration::ZERO, never_hung).expect("an entry");
            order.extend(planned.entries().iter().map(|entry| entry.observed_at_us));
        }
        order
    }

    #[test]
    fn what_arrives_after_a_focus_marker_waits_for_its_read_and_the_read_takes_its_place() {
        let mut intake = Intake::default();
        push_item(&mut intake, name_change(1), 1);
        push_item(&mut intake, marker(7), 2);
        push_item(&mut intake, name_change(3), 3);
        assert_eq!(
            handed_out(&mut intake),
            [1],
            "the change after the marker waits"
        );
        assert!(intake.busy(), "the marker and the change still wait");
        push_item(
            &mut intake,
            Item::Fact(uia_focus(7, 3, vec![42, 7]), None),
            4,
        );
        assert_eq!(
            handed_out(&mut intake),
            [4, 3],
            "the focus in its marker's place, then the change"
        );
        assert!(!intake.busy());
    }

    #[test]
    fn a_marked_selection_takes_its_markers_place_as_itself() {
        let mut intake = Intake::default();
        push_item(&mut intake, marker(8), 1);
        push_item(&mut intake, name_change(3), 2);
        assert_eq!(
            handed_out(&mut intake),
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
        push_item(&mut intake, Item::Fact(marked, None), 3);
        let (planned, _) = intake.next(Duration::ZERO, never_hung).expect("an entry");
        let entries = planned.entries();
        assert!(
            matches!(&entries[0].item, Item::Fact(fact, None) if *fact == selection),
            "the selection, unwrapped, in its marker's place"
        );
        assert_eq!(handed_out(&mut intake), [2], "then the change");
    }

    #[test]
    fn a_dropped_focus_releases_what_waited_behind_its_marker() {
        let mut intake = Intake::default();
        push_item(&mut intake, marker(7), 1);
        push_item(&mut intake, marker(8), 2);
        push_item(&mut intake, name_change(3), 3);
        assert_eq!(handed_out(&mut intake), Vec::<u64>::new());
        push_item(
            &mut intake,
            Item::Fact(
                DeliveredFact::UiaFocusDropped {
                    marker: 7,
                    focus: true,
                    answer: GapAnswer::LeavesOpen,
                },
                None,
            ),
            4,
        );
        assert_eq!(
            handed_out(&mut intake),
            Vec::<u64>::new(),
            "marker 8 still waits"
        );
        push_item(
            &mut intake,
            Item::Fact(
                DeliveredFact::UiaFocusDropped {
                    marker: 8,
                    focus: true,
                    answer: GapAnswer::LeavesOpen,
                },
                None,
            ),
            5,
        );
        assert_eq!(handed_out(&mut intake), [3]);
        assert!(!intake.busy());
    }

    /// The gap record's bound's entry, pushed while a UIA focus's marker
    /// waits, waits behind it: the read that resolves the marker is handed
    /// out first, so it decides before the bound's entry is handled.
    #[test]
    fn the_bounds_entry_waits_behind_a_marker() {
        let mut intake = Intake::default();
        push_item(&mut intake, marker(7), 1);
        intake.push_gap_bound(Entry::own(Item::GapBound, TraceId::mint(), 0));
        assert_eq!(handed_out(&mut intake), Vec::<u64>::new());
        push_item(
            &mut intake,
            Item::Fact(
                DeliveredFact::UiaFocusDropped {
                    marker: 7,
                    focus: true,
                    answer: GapAnswer::LeavesOpen,
                },
                None,
            ),
            2,
        );
        let (planned, _) = intake
            .next(Duration::ZERO, never_hung)
            .expect("the bound's entry");
        assert!(
            matches!(
                planned.entries()[..],
                [Entry {
                    item: Item::GapBound,
                    ..
                }]
            ),
            "the bound's entry once the marker is resolved"
        );
        assert!(!intake.busy());
    }

    /// A UIA focus's drop answers the gap record as it resolves its marker,
    /// with nothing handed to the worker and the timer cancelled; a drop for
    /// no window that answers the record takes its marker's place as the
    /// record's read, stamped with the record's stamp.
    #[test]
    fn a_drops_answer_is_applied_as_its_marker_resolves() {
        let mut intake = Intake::default();
        let dropped = |marker, answer| {
            Item::Fact(
                DeliveredFact::UiaFocusDropped {
                    marker,
                    focus: true,
                    answer,
                },
                None,
            )
        };
        let (opened, timer) = intake.gap_copy_dropped(0x11, 0x10, 1_000);
        assert_eq!(
            (opened, timer),
            (
                CopyDropped::Opened { due_us: 1_001_000 },
                TimerAction::Set(1_001_000)
            )
        );
        push_item(&mut intake, marker(7), 1_500);
        assert_eq!(
            intake
                .push(
                    Entry {
                        item: dropped(7, GapAnswer::Answers(Some(0x10))),
                        trace: TraceId::mint(),
                        observed_at_us: 1_500,
                        timing: verbatim_model::protocol::EventTiming::default(),
                    },
                    None
                )
                .timer,
            Some(TimerAction::Cancel)
        );
        assert!(!intake.gap_open());
        assert!(!intake.busy(), "nothing for the worker");

        let _ = intake.gap_copy_dropped(0x11, 0x10, 500_000);
        push_item(&mut intake, marker(8), 501_000);
        push_item(&mut intake, name_change(3), 502_000);
        assert_eq!(
            intake
                .push(
                    Entry {
                        item: dropped(8, GapAnswer::ReadsNow),
                        trace: TraceId::mint(),
                        observed_at_us: 501_000,
                        timing: verbatim_model::protocol::EventTiming::default(),
                    },
                    None
                )
                .timer,
            Some(TimerAction::Cancel)
        );
        let (planned, _) = intake
            .next(Duration::ZERO, never_hung)
            .expect("the record's read");
        assert!(
            matches!(
                planned.entries()[..],
                [Entry {
                    item: Item::GapRead { window: 0x11 },
                    observed_at_us: 500_000,
                    ..
                }]
            ),
            "the record's read in the marker's place"
        );
        assert_eq!(handed_out(&mut intake), [502_000]);
    }

    #[test]
    fn the_end_of_the_listeners_reads_releases_every_marker() {
        let mut intake = Intake::default();
        push_item(&mut intake, marker(7), 1);
        push_item(&mut intake, name_change(2), 2);
        push_item(&mut intake, marker(8), 3);
        push_item(&mut intake, name_change(4), 4);
        push_item(
            &mut intake,
            Item::Fact(DeliveredFact::UiaFocusReadsEnded, None),
            5,
        );
        assert_eq!(handed_out(&mut intake), [2, 4]);
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
        let mut intake = Intake::default();
        intake.note_focus(Some(Object::Uia(terminal.to_vec())), Some(7), false, false);
        // The console host's text area, whose caret its console's caret
        // event tells of, and its output its console updates.
        push_item(
            &mut intake,
            Item::Msaa {
                kind: WinKind::ConsoleCaret,
                hwnd: 7,
                id_object: 2,
                id_child: 9,
                raiser: 80,
            },
            1,
        );
        push_item(
            &mut intake,
            Item::Msaa {
                kind: WinKind::ConsoleUpdate,
                hwnd: 7,
                id_object: 3,
                id_child: 65,
                raiser: 80,
            },
            2,
        );
        assert_eq!(handed_out(&mut intake), [2, 1]);
        // Windows Terminal's control, whose output its UIA text change
        // tells of.
        push_item(
            &mut intake,
            text_event(UiaKind::TextSelection, &terminal),
            3,
        );
        push_item(&mut intake, text_event(UiaKind::TextChanged, &terminal), 4);
        assert_eq!(handed_out(&mut intake), [4, 3]);
    }

    /// A console update of window `hwnd`, observed at `observed_at_us`.
    fn push_console_update(intake: &mut Intake, hwnd: isize, observed_at_us: u64) {
        push_item(intake, console_update(hwnd), observed_at_us);
    }

    /// A console update of window `hwnd`.
    fn console_update(hwnd: isize) -> Item {
        Item::Msaa {
            kind: WinKind::ConsoleUpdate,
            hwnd,
            id_object: 3,
            id_child: 65,
            raiser: 80,
        }
    }

    #[test]
    fn a_held_consoles_updates_are_dropped_at_push_until_the_mark_is_cleared() {
        let mut intake = Intake::default();
        intake.absorb_console_updates(Some(7));
        assert!(intake.marks().drops_on_arrival(&console_update(7)));
        // Another window's update, and the held window's caret, still come.
        assert!(!intake.marks().drops_on_arrival(&console_update(8)));
        let caret = Item::Msaa {
            kind: WinKind::ConsoleCaret,
            hwnd: 7,
            id_object: 2,
            id_child: 9,
            raiser: 80,
        };
        assert!(!intake.marks().drops_on_arrival(&caret));
        intake.absorb_console_updates(None);
        assert!(
            !intake.marks().drops_on_arrival(&console_update(7)),
            "the next update comes through"
        );
        intake.absorb_console_updates(Some(7));
        intake.note_focus(None, None, false, false);
        assert!(
            !intake.marks().drops_on_arrival(&console_update(7)),
            "a new focus clears the mark"
        );
    }

    fn idle_query(round: u64) -> Item {
        Item::IdleQuery {
            round,
            behind_noted: false,
        }
    }

    /// An idle query divides what waits: a console update that waited
    /// before it is not replaced by a newer one of the same window behind
    /// it, and is handled ahead of it.
    #[test]
    fn an_entry_waiting_before_an_idle_query_is_never_replaced_from_behind_it() {
        let mut intake = Intake::default();
        push_console_update(&mut intake, 7, 1);
        push_item(&mut intake, idle_query(1), 2);
        push_console_update(&mut intake, 7, 3);
        // Ahead of the query, a newer update of the same window behind
        // the first still replaces the one waiting with it.
        push_console_update(&mut intake, 7, 4);
        assert_eq!(handed_out(&mut intake), [1, 2, 4]);
    }

    /// An idle query ends its batch: the menu opening that arrived before
    /// it is handled before it, and a foreground change that arrived after
    /// it does not replace one that arrived before.
    #[test]
    fn a_batch_ends_at_an_idle_query_with_its_menu_opening_ahead_of_it() {
        let mut intake = Intake::default();
        push_fact(
            &mut intake,
            DeliveredFact::MenuPopup {
                hwnd: 77,
                id_object: -4,
                id_child: 0,
            },
        );
        push_item(
            &mut intake,
            Item::Fact(DeliveredFact::Foreground { hwnd: 77 }, None),
            2,
        );
        push_item(&mut intake, idle_query(1), 3);
        push_item(
            &mut intake,
            Item::Fact(DeliveredFact::Foreground { hwnd: 78 }, None),
            4,
        );
        assert_eq!(handed_out(&mut intake), [2, 0, 3, 4]);
    }

    /// A UIA property change `property` on element `runtime_id`.
    fn property_change(property: i32, runtime_id: &[i32]) -> Item {
        text_event(UiaKind::Property(property), runtime_id)
    }

    /// [`uia_focus`] on `runtime_id`, read with `ancestors` as its
    /// ancestors, nearest first.
    fn uia_focus_inside(marker: u64, runtime_id: Vec<i32>, ancestors: &[&[i32]]) -> Item {
        let mut fact = uia_focus(marker, 3, runtime_id);
        if let DeliveredFact::UiaFocusRead { read, .. } = &mut fact {
            read.ancestors = ancestors
                .iter()
                .map(|runtime_id| verbatim_model::protocol::UiaAncestorFact {
                    snapshot: snapshot(runtime_id.to_vec()),
                    hwnd: 0,
                    content: true,
                })
                .collect();
        }
        Item::Fact(fact, None)
    }

    /// A combo box `[42, 1]` focused opens into its item `[42, 2]`: the
    /// combo box's state changes (its expand and toggle states), queued
    /// after the item's marker, are planned ahead of the item's read, in
    /// their order, while another element's change and the combo box's
    /// own value change keep their places behind it.
    #[test]
    fn the_old_focus_uia_events_behind_a_focus_inside_it_are_planned_first() {
        let mut intake = Intake::default();
        intake.note_focus(Some(Object::Uia(vec![42, 1])), None, false, false);
        push_item(&mut intake, marker(7), 1);
        push_item(&mut intake, property_change(30070, &[42, 1]), 2);
        push_item(&mut intake, property_change(30005, &[42, 5]), 3);
        push_item(&mut intake, property_change(30045, &[42, 1]), 4);
        push_item(&mut intake, property_change(30086, &[42, 1]), 5);
        push_item(
            &mut intake,
            uia_focus_inside(7, vec![42, 2], &[&[42, 3], &[42, 1]]),
            6,
        );
        assert_eq!(handed_out(&mut intake), [2, 5, 6, 3, 4]);
    }

    /// The old focus's state change behind a second focus marker keeps its
    /// place behind that marker when the first marker's read is a focus
    /// inside the old focus: the move looks no further than the next
    /// marker.
    #[test]
    fn the_old_focus_state_change_behind_the_next_marker_keeps_its_place() {
        let mut intake = Intake::default();
        intake.note_focus(Some(Object::Uia(vec![42, 1])), None, false, false);
        push_item(&mut intake, marker(7), 1);
        push_item(&mut intake, property_change(30070, &[42, 1]), 2);
        push_item(&mut intake, marker(8), 3);
        push_item(&mut intake, property_change(30086, &[42, 1]), 4);
        push_item(
            &mut intake,
            uia_focus_inside(7, vec![42, 2], &[&[42, 3], &[42, 1]]),
            5,
        );
        push_item(
            &mut intake,
            uia_focus_inside(8, vec![42, 4], &[&[42, 3]]),
            6,
        );
        assert_eq!(handed_out(&mut intake), [2, 5, 6, 4]);
    }

    /// The listener's selection and notification on the old focus, which
    /// share their keys with the outpost's own events, keep their places
    /// behind a focus inside it, already in the listener's order; the old
    /// focus's own state change behind them is still planned first.
    #[test]
    fn the_listener_facts_on_the_old_focus_keep_their_places_behind_a_focus_inside_it() {
        let mut intake = Intake::default();
        intake.note_focus(Some(Object::Uia(vec![42, 1])), None, false, false);
        push_item(&mut intake, marker(7), 1);
        push_item(
            &mut intake,
            Item::Fact(
                DeliveredFact::UiaSelection {
                    hwnd: 3,
                    focus_window: 0,
                    snapshot: snapshot(vec![42, 1]),
                },
                None,
            ),
            2,
        );
        push_item(
            &mut intake,
            Item::Fact(
                DeliveredFact::UiaNotification {
                    hwnd: 3,
                    snapshot: snapshot(vec![42, 1]),
                    notification: Notification {
                        kind: NotificationKind::ActionCompleted,
                        processing: NotificationProcessing::All,
                        display_string: Some("Opened".to_owned()),
                        activity_id: None,
                    },
                },
                None,
            ),
            3,
        );
        push_item(&mut intake, property_change(30070, &[42, 1]), 4);
        push_item(
            &mut intake,
            uia_focus_inside(7, vec![42, 2], &[&[42, 3], &[42, 1]]),
            5,
        );
        assert_eq!(handed_out(&mut intake), [4, 5, 2, 3]);
    }

    /// A focus that is not inside the old focus keeps its place ahead of
    /// the old focus's later events.
    #[test]
    fn a_focus_outside_the_old_focus_keeps_its_place_before_its_events() {
        let mut intake = Intake::default();
        intake.note_focus(Some(Object::Uia(vec![42, 1])), None, false, false);
        push_item(&mut intake, marker(7), 1);
        push_item(&mut intake, property_change(30070, &[42, 1]), 2);
        push_item(
            &mut intake,
            uia_focus_inside(7, vec![42, 2], &[&[42, 3]]),
            3,
        );
        assert_eq!(handed_out(&mut intake), [3, 2]);
    }

    #[test]
    fn a_focus_read_whose_marker_never_came_is_taken_where_it_arrives() {
        let mut intake = Intake::default();
        push_item(&mut intake, name_change(1), 1);
        push_item(
            &mut intake,
            Item::Fact(uia_focus(9, 3, vec![42, 9]), None),
            2,
        );
        push_item(&mut intake, name_change(3), 3);
        assert_eq!(handed_out(&mut intake), [1, 2, 3]);
    }
}
