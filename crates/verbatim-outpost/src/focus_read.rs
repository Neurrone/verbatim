//! The focus listener's judging and reading of each UIA focus event, from
//! the event's own element, on a thread for the element's application
//! (`docs/design/focus-pipeline.md`, section 12; decision D13 as amended
//! on 2026-10-11).
//!
//! A UIA element cannot be handed from the process that received its event
//! to another (experiment E-marshal), so the listener, which receives every
//! focus event, is the one process that can judge a focus by its sender, as
//! NVDA does, and read it. Its callback only queues the sender for the
//! thread of the sender's application and returns; the thread, one per
//! application, created when the application's first focus event arrives,
//! does NVDA's flusher-thread work for each of its application's events in
//! arrival order ([`FocusReader::read`]):
//!
//! - NVDA's focus handler's checks, in its order, before anything of the
//!   sender is read (`FocusReader::judge_before_reading`;
//!   `docs/design/focus-pipeline.md`, section 18): a sender whose cached
//!   window the system reports hung is dropped; one whose nearest window's
//!   class is on NVDA's MSAA class list, or that has none, is not native
//!   and is dropped, the window's verdict otherwise being its outpost's
//!   (`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated
//!   fact", the verdicts); a
//!   sender UI Automation counts as the element of the focus Core holds
//!   (`CompareElements`), while that element still has the keyboard
//!   focus, read live, is a repeat and is dropped, but only once Core has
//!   judged every focus the listener read ([`HeldElements`]; decision D14,
//!   "Where the rules run"); one whose nearest window fails NVDA's
//!   acceptance test is dropped; the console window's own element, which
//!   NVDA's `consoleUIAWindow` refuses, and a focus in Core's hidden frame
//!   are dropped; and a sender without the keyboard focus, read with a
//!   plain property read, is dropped, as NVDA's `shouldAllowUIAFocusEvent`
//!   drops it. A shell window that only stages a switch is read and goes to
//!   Core like any focus, and Core's rules decide what is said of it. A
//!   menu opening is judged by NVDA's automation event handler's checks
//!   instead, the same but for the repeat, and with a sender UI Automation
//!   counts as the element of the focus Core holds judged as that focus,
//!   with its window and without the native check;
//! - then the classic read, every call on an element the listener holds,
//!   with no connection to any window and no remote operation
//!   (`docs/design/listener-without-connections.md`, sections 2 and 3):
//!   when the sender has the runtime id of the focus the thread read
//!   before, whether that earlier element still has the keyboard focus,
//!   the evidence that the runtime id still names it; then the sender's
//!   own value, states, and details afresh, its selected child for a list
//!   or tab control, and its ancestors, one step up at a time, to the
//!   first one the thread knew from the focus before, whose ancestors are
//!   reused from that read, or to the desktop's root, or to the process
//!   boundary; and for a menu item, its legacy MSAA state, where its
//!   checked state may be. The sender's keyboard focus and nearest window,
//!   read just before, are not asked again.
//!
//! The result, or the drop, goes to the application's outpost through
//! verbatim.exe like every fact; the outpost was sent a marker the moment
//! the event arrived, and holds the application's later events until the
//! result or the drop takes the marker's place (section 12.4).
//!
//! Time limits: a read is several calls, each on an element the listener
//! holds, which UIA's transaction timeout, set once for the process, ends
//! (`verbatim_uia::TRANSACTION_TIMEOUT`); a call that fails ends the read.
//! NVDA's checks have the time budget [`READ_BUDGET`] from the read's
//! start, and the walk after them has its bound, measured from the event's
//! observation (`docs/design/focus-pipeline-rework.md`, the Terms):
//! [`MOVED_ON_GRACE`] when the focus's top-level window is not that of the
//! previous focus the listener observed, another application's focus
//! before it always counting as elsewhere, and [`READ_BUDGET`] otherwise.
//! Each is asked before each of the read's calls after the first
//! ([`Budget`]): once the bound has passed, the walk makes no more calls,
//! and the focus is reported with what it has read, as the event's cache
//! gives what it has not, and the windows it has not reached as plain
//! windows ([`read_at_bound`]). A read whose call is still under way at
//! its bound, once it has passed every check, is published then by the
//! watchdog from the progress its thread shares ([`ReadProgress`]), the
//! same read through the same function, and its thread's late result is
//! discarded; the thread is not abandoned for this, and reads its
//! application's next event when its call returns. A read still in its
//! checks at the bound is left to the rules below. The watchdog abandons a
//! thread whose read runs past [`READ_DEADLINE`], the budget and the one
//! call that can be under way when the budget passes, and, as NVDA's
//! watchdog cancels a slow call once the user has
//! moved on (`docs/nvda/main-loop-and-watchdog.md`, "The watchdog:
//! detection"), a thread whose read of a focus has taken
//! [`MOVED_ON_GRACE`] while the
//! window in front belongs to the read's application on another UI thread
//! than the window being read ([`WatchedRead`]). A read is judged so only
//! once its window is known, from the event's cached window handle or from
//! the sender's nearest window once the read has found it, and a selection
//! or a notification is never judged so. It is judged on each foreground
//! event the listener's hook delivers once the grace has ended
//! ([`Readers::foreground_changed`]), and when the grace ends if a
//! foreground event has come since the read's event arrived, or if the
//! window in front was already another thread's when the read's window
//! became known: one process can own windows on several UI threads, as
//! File Explorer's folder windows, the taskbar and the Alt+Tab switcher
//! are all `explorer.exe`'s, and the application's focus on another thread
//! must not wait behind a slow read of the first. Either way its event is
//! dropped, a new thread takes the application's later events, and the
//! abandoned thread, once UIA gives up, says nothing more, accepts no
//! focus, and ends. An application whose abandoned threads reach
//! [`ABANDONED_LIMIT`] gets no new thread until one returns: its events
//! wait, a newer event of an element replacing its waiting one, and the
//! first abandoned thread whose call returns reads them, unless
//! [`READ_DEADLINE`] has passed since the application reached the limit,
//! when the watchdog drops every event waiting and each later one is
//! dropped as it arrives, until a thread returns (one bound per
//! application, which [`READ_DEADLINE`] states); an event waiting so is a
//! pending focus for its own application's menu openings only
//! ([`Readers::focus_pending`]).
//!
//! A thread keeps its focus before, NVDA's duplicate evidence, for as long
//! as its application has an outpost: it ends, once it has read every event
//! waiting, when the listener learns that the application's outpost has
//! ended ([`Readers::outpost_ended`]), and at the listener's shutdown
//! (`docs/design/focus-pipeline.md`, section 16.6). An event that arrives
//! after that, for an application still running, starts a new thread.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_LegacyIAccessibleStatePropertyId,
};
use windows::core::AgileReference;

use verbatim_model::{NodeDetails, Pid, State, TraceId};
use verbatim_uia::map::{CachedUiaParts, add_legacy_checked_state, wants_legacy_checked_state};
use verbatim_uia::{
    Ancestor, AncestorGates, Cached, ElementExt as _, Fill, FocusInput, ListenerEvent, Node,
    NodeGates, Shape, Uia,
};
use verbatim_uia_rops::{Ancestry, FocusAncestry, FocusQuery, Progress, focus_ancestry_classic};

use crate::ForegroundReader;
use crate::arbitration::{
    ClassVerdict, Clock, ObservationClock, WindowClasses, class_verdict, window_class_name,
};
use crate::outpost::acceptance::{EventName, RequestedEvents, SystemWindows, accepts_event};
use crate::outpost::alarm::Alarm;
use crate::outpost::text_reads::CONSOLE_WINDOW_CLASS;
use crate::outpost::window::{
    MOVED_ON_GRACE, front_is_another_thread_of_its_application, top_level_of,
    window_belongs_to_hidden_frame, window_is_hung,
};
use crate::protocol::{
    AncestorsEnd, DeliveredFact, EventTiming, GapAnswer, UiaAncestorFact, UiaFocusRead,
    UiaSnapshotFact, now_us,
};
use verbatim_core::focus_rules::{RepeatEvidence, repeats_focus};

/// The most ancestors read for a focus, as many as the outpost walks.
const MAX_ANCESTORS: u32 = 64;

/// How long NVDA's checks of one focus event may take from the read's
/// start, and the bound of the walk after them, from the event's
/// observation, for a focus in the top-level window of the focus before
/// it, before the read makes no more calls into the application: asked
/// before each call after the first, each of which UIA ends after its
/// transaction timeout (`docs/design/listener-without-connections.md`,
/// section 5). A read of an application that answers takes milliseconds
/// (section 10, step 6), so only a slow application meets it, and its
/// focus is then reported with what was read by then, the windows not
/// reached as plain windows, rather than held (section 9, risk 4).
const READ_BUDGET: Duration = Duration::from_secs(2);

/// How long a reading thread may take over one focus event before the
/// watchdog abandons it, and the bound on an application at
/// [`ABANDONED_LIMIT`], 8 seconds: the [`READ_BUDGET`] of 2 seconds,
/// UIA's transaction timeout of 5 seconds for the call that can be under
/// way when the budget passes, and a second to spare. The listener makes
/// no probe of its own, so no probe's wait is added
/// (`docs/design/focus-pipeline-rework.md`, the verdicts). It bounds
/// how long the application's later events wait behind the read's marker,
/// whatever the read's calls do.
///
/// The bound at the limit is one per application, not one per event: once
/// its second abandoned thread is still in its call, its events wait for a
/// thread until this long after that moment; then every event waiting is
/// dropped ([`WAITED_TOO_LONG`]), and each later one is dropped as it
/// arrives, until an abandoned thread's call returns, which ends the limit
/// and the bound with it. So the application's outpost, whose later events
/// wait behind each marker, runs at most this long behind the hung reads,
/// once, however many events the application raises meanwhile.
///
/// It does
/// not bound the call under way: the transaction timeout bounds each
/// provider call, not a UIA operation (`docs/uia-bugs.md`, "A timeout
/// bounds each provider call, not an operation"), so one operation that
/// makes several provider calls, as the nearest-window lookup's
/// `NormalizeElementBuildCache` does, can run past the deadline with each
/// of its provider calls answered within the timeout, and its read is then
/// abandoned though the application still answers. Only a read whose last
/// call is one provider call, or one remote operation, a single
/// transaction, is sure to end within it.
pub(crate) const READ_DEADLINE: Duration = READ_BUDGET
    .saturating_add(verbatim_uia::TRANSACTION_TIMEOUT)
    .saturating_add(Duration::from_secs(1));

/// One focus read's time budget, on the reader's clock, asked before each
/// of the read's calls after the first: [`READ_BUDGET`] from the read's
/// start for NVDA's checks, and, once the read has passed them, the walk's
/// bound, measured from the event's observation
/// ([`bound_walk`](Self::bound_walk)).
///
/// It also times each of the read's calls, by the system's clock whatever
/// the reader's: a call that takes longer than [`SLOW_CALL`] is logged at
/// info with its name and how long it took, so the slowest calls behind
/// [`READ_DEADLINE`] can be seen (Dickson, 2026-10-20). Each call costs one
/// clock reading, at its start, which is also the end of the call before
/// it; the last ends when the budget is dropped, at the read's end.
struct Budget {
    clock: Clock,
    /// When the read began, on the reader's clock.
    start: Instant,
    /// When the time is up.
    end: Cell<Instant>,
    /// The call under way, by name, and when it began, by the system's
    /// clock.
    call: Cell<Option<(&'static str, Instant)>>,
}

/// How long one call of a focus read may take before it is logged at info.
const SLOW_CALL: Duration = Duration::from_secs(1);

impl Budget {
    /// A budget from now, by `clock`.
    fn new(clock: Clock) -> Self {
        let start = clock();
        Self {
            clock,
            start,
            end: Cell::new(start + READ_BUDGET),
            call: Cell::new(None),
        }
    }

    /// The read has passed NVDA's checks: from now on its time is up at
    /// `bound` after the read's start, the walk's bound less the time the
    /// event waited before its read began. Answers how long is left, which
    /// is none once that is past.
    fn bound_walk(&self, bound: Duration) -> Duration {
        let end = self.start + bound;
        self.end.set(end);
        end.saturating_duration_since((self.clock)())
    }

    /// The call named `name` begins, and the one before it, if any, has
    /// ended.
    fn call(&self, name: &'static str) {
        let now = Instant::now();
        self.call_ended(now);
        self.call.set(Some((name, now)));
    }

    /// The call under way, if any, ended at `now`: logged if it was slow.
    fn call_ended(&self, now: Instant) {
        if let Some((call, began)) = self.call.take() {
            let elapsed = now.saturating_duration_since(began);
            if elapsed > SLOW_CALL {
                tracing::info!(
                    call,
                    elapsed_ms = elapsed.as_millis(),
                    "a call of a UIA focus read took longer than a second"
                );
            }
        }
    }

    /// Whether the time is up.
    fn out_of_time(&self) -> bool {
        (self.clock)() >= self.end.get()
    }
}

impl Drop for Budget {
    /// The read has ended, and with it its last call.
    fn drop(&mut self) {
        if self.call.get().is_some() {
            self.call_ended(Instant::now());
        }
    }
}

/// How many of an application's reading threads may be abandoned and still
/// in their calls before it gets no new thread: its focus events then wait
/// for the first of those threads whose call returns, within the bound
/// [`READ_DEADLINE`] states.
const ABANDONED_LIMIT: usize = 2;

/// Why an event is dropped whose read the watchdog abandoned at the
/// deadline.
const NOT_IN_TIME: &str = "its read did not return in time";

/// Why an event is dropped whose read the watchdog abandoned because the
/// user moved on from its window, as an outpost's watchdog says it.
const MOVED_ON: &str = "the user moved on from a slow window";

/// Why an event is dropped that waited for a thread while its application's
/// abandoned threads were at [`ABANDONED_LIMIT`] until the bound
/// [`READ_DEADLINE`] states, or arrived after that bound.
const WAITED_TOO_LONG: &str = "it waited too long behind its application's abandoned reads";

/// Why a focus is dropped whose event said it lacked the keyboard focus
/// when its read's time was up, before the live read of its focus.
const UNFOCUSED_OUT_OF_TIME: &str =
    "its event says it did not have the keyboard focus, and its read's time was up";

/// Why a focus is dropped whose read's time was up before its sender's
/// nearest window was looked up.
const OUT_OF_TIME_BEFORE_LOOKUP: &str = "its read ran out of time before its window was looked up";

/// What a read does once its window becomes known
/// ([`WatchedRead::learn`]), or as it starts with its window known from
/// the event's cache ([`WatchedRead::known`]): judges, as the watchdog
/// would, whether the window in front is already another thread's, and
/// wakes the watchdog if it is.
type OnLearn = Box<dyn Fn(&WatchedRead) + Send + Sync>;

/// What a read does once it has passed NVDA's checks and its bound is
/// known ([`WatchedRead::arm`]): has the watchdog's alarm go off by that
/// bound, by the system's clock, which wakes no thread.
type OnArm = Box<dyn Fn(Instant) + Send + Sync>;

/// No one has claimed the outcome of a read: its reading thread sends it.
const OPEN: u8 = 0;

/// The watchdog has abandoned the read.
const ABANDONED: u8 = 1;

/// The watchdog has published the read at its bound from its progress
/// ([`ReadProgress`]): the reading thread's result is discarded when it
/// returns.
const PUBLISHED: u8 = 2;

/// One read as the reading threads' watchdog watches it, shared by the read
/// and the watchdog. Each read has its own, so an abandoned read that finds
/// its window later changes nothing the watchdog judges.
///
/// The window it is of, for the judgement of whether the user has moved on
/// from it ([`MOVED_ON_GRACE`]): the
/// sender's own window from the event's cache as the read starts, for a
/// focus ([`Reading::window`]), and the sender's nearest window once the
/// read of a focus whose sender is not a window has found it
/// ([`learn`](Self::learn)); 0 while neither is known, and always for a
/// selection or a notification, when the user cannot be judged to have
/// moved on.
///
/// Whether the window in front was another thread of the application than
/// the read's window when that became known ([`judge_front`](Self::judge_front)),
/// which has the watchdog check the read when its grace ends, as a
/// foreground event since its event does.
///
/// Its claim, which says who sends its outcome: its reading thread, unless
/// the watchdog has abandoned it ([`abandon`](Self::abandon)) or published
/// it at its bound ([`publish`](Self::publish)). A read that returns after
/// either keeps no focus for Core's judgement ([`HeldElements`]), and its
/// result is discarded. Only the watchdog claims a read, under the
/// readers' lock.
///
/// And, once the read has passed every check NVDA makes before it queues a
/// focus, its progress ([`ReadProgress`]): what it has read so far and its
/// bound, from which the watchdog publishes it should a call of its walk
/// still be under way at the bound (`docs/design/focus-pipeline-rework.md`,
/// "A window's name, and the one fallback").
#[derive(Clone, Default)]
pub(crate) struct WatchedRead(Arc<Watched>);

#[derive(Default)]
struct Watched {
    window: AtomicIsize,
    /// The top-level window of the read's window, once the read has found
    /// it, 0 before.
    top_level: AtomicIsize,
    /// Who sends the read's outcome: [`OPEN`], [`ABANDONED`] or
    /// [`PUBLISHED`].
    claim: AtomicU8,
    /// Whether the window in front has been judged against the read's
    /// window since that became known.
    front_judged: AtomicBool,
    /// Whether it was then another thread's of the read's application.
    front_elsewhere: AtomicBool,
    /// Whether the focus event the listener received before this one was
    /// another application's ([`Readers::push`]), so that this focus's
    /// top-level window is not that of the previous focus the listener
    /// observed, whatever its reading thread read before.
    after_elsewhere: bool,
    on_learn: Option<OnLearn>,
    on_arm: Option<OnArm>,
    progress: Mutex<Option<ReadProgress>>,
}

impl WatchedRead {
    /// A read of window `hwnd`, 0 for one not yet known, which does
    /// `on_learn` once it learns its window, or as it starts with it known,
    /// and `on_arm` once its bound is known; `after_elsewhere` says whether
    /// the focus event before it was another application's.
    fn new(
        hwnd: isize,
        (on_learn, on_arm): (Option<OnLearn>, Option<OnArm>),
        after_elsewhere: bool,
    ) -> Self {
        Self(Arc::new(Watched {
            window: AtomicIsize::new(hwnd),
            after_elsewhere,
            on_learn,
            on_arm,
            ..Watched::default()
        }))
    }

    /// The read has found that it is of window `hwnd`.
    pub(crate) fn learn(&self, hwnd: isize) {
        self.0.window.store(hwnd, Ordering::SeqCst);
        if let Some(on_learn) = &self.0.on_learn {
            on_learn(self);
        }
    }

    /// The read starts with its window known from the event's cache: does
    /// what learning it does, if it is.
    fn known(&self) {
        if self.window() != 0
            && let Some(on_learn) = &self.0.on_learn
        {
            on_learn(self);
        }
    }

    /// The window the read is of, 0 for none known yet.
    fn window(&self) -> isize {
        self.0.window.load(Ordering::SeqCst)
    }

    /// The read has found its window's top-level window, `top`.
    fn place(&self, top: isize) {
        self.0.top_level.store(top, Ordering::SeqCst);
    }

    /// The top-level window of the read's window, 0 while not found.
    fn top_level(&self) -> isize {
        self.0.top_level.load(Ordering::SeqCst)
    }

    /// Whether the focus event before this one was another application's.
    fn after_elsewhere(&self) -> bool {
        self.0.after_elsewhere
    }

    /// Whether the window in front was another thread of the read's
    /// application than the read's window when that became known, as
    /// `moved_on` says: asked once, by the watchdog or by the read's
    /// [`learn`](Self::learn), whichever comes first once the window is
    /// known, and `false` while it is not.
    fn judge_front(&self, moved_on: impl FnOnce(isize) -> bool) -> bool {
        let window = self.window();
        if window != 0 && !self.0.front_judged.swap(true, Ordering::SeqCst) && moved_on(window) {
            self.0.front_elsewhere.store(true, Ordering::SeqCst);
        }
        self.0.front_elsewhere.load(Ordering::SeqCst)
    }

    /// The watchdog has abandoned the read: answers whether its outcome was
    /// still unsent, so that its drop is to be sent, rather than published
    /// at its bound already.
    fn abandon(&self) -> bool {
        self.0.claim.swap(ABANDONED, Ordering::SeqCst) == OPEN
    }

    /// Whether the watchdog has abandoned the read or published it, so its
    /// reading thread's result is discarded.
    fn claimed(&self) -> bool {
        self.0.claim.load(Ordering::SeqCst) != OPEN
    }

    /// Whether the watchdog has published the read at its bound and not
    /// abandoned it since.
    fn published(&self) -> bool {
        self.0.claim.load(Ordering::SeqCst) == PUBLISHED
    }

    /// The read has passed NVDA's checks with `progress`, which says its
    /// bound, and the watchdog is told when that is.
    fn arm(&self, progress: ReadProgress) {
        let bound_at = progress.bound_at;
        *self.lock_progress() = Some(progress);
        if let Some(on_arm) = &self.0.on_arm {
            on_arm(bound_at);
        }
    }

    /// Updates the read's progress, if it is armed.
    fn advance(&self, update: impl FnOnce(&mut ReadProgress)) {
        if let Some(progress) = self.lock_progress().as_mut() {
            update(progress);
        }
    }

    /// Takes the read's progress, ending it: the watchdog has nothing more
    /// to publish from.
    fn take_progress(&self) -> Option<ReadProgress> {
        self.lock_progress().take()
    }

    /// Puts `progress` back, its walk ended, for a last call
    /// ([`ReadProgress::finished`]).
    fn restore_progress(&self, progress: ReadProgress) {
        *self.lock_progress() = Some(progress);
    }

    /// The read's bound, by the system's clock, while it is armed and no
    /// one has claimed it.
    fn bound_at(&self) -> Option<Instant> {
        if self.claimed() {
            return None;
        }
        self.lock_progress()
            .as_ref()
            .map(|progress| progress.bound_at)
    }

    /// Claims the read for the watchdog's publication at its bound and
    /// answers it from its progress ([`read_at_bound`]), with the top-level
    /// window it was placed in; `None`, claiming nothing, when the read is
    /// not open or has no progress. Called under the readers' lock; the
    /// local calls that complete the chain are made on the watchdog's
    /// thread.
    fn publish(&self) -> Option<UiaFocusRead> {
        if self.claimed() {
            return None;
        }
        let read = read_at_bound(self.lock_progress().as_ref()?);
        self.0
            .claim
            .compare_exchange(OPEN, PUBLISHED, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| read)
    }

    fn lock_progress(&self) -> MutexGuard<'_, Option<ReadProgress>> {
        self.0
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// A UIA focus read's progress, which its reading thread shares with the
/// watchdog once the read has passed every check NVDA makes before it
/// queues a focus (`docs/design/focus-pipeline-rework.md`, "A window's
/// name, and the one fallback"): the event's cached facts, the facts read
/// so far, the ancestors read and the nearest window. Plain data, so the
/// watchdog's thread can make the read of it ([`read_at_bound`]).
pub(crate) struct ReadProgress {
    /// The read's bound, by the system's clock: [`MOVED_ON_GRACE`] after the
    /// event's observation when the focus's top-level window is not that of
    /// the previous focus the listener observed, and [`READ_BUDGET`] after
    /// it otherwise.
    bound_at: Instant,
    /// The bound's length from the event's observation, which the outpost
    /// continuing the chain keeps ([`UiaFocusRead::bound_us`]).
    bound: Duration,
    /// The sender's own window from the event's cache, 0 for none.
    hwnd: isize,
    /// The sender's nearest window.
    nearest: isize,
    /// The focus as the event's cache has it, its class name included.
    event: UiaSnapshotFact,
    /// Its value, states and details read afresh, and whether its value is
    /// its first selected item's name, once read.
    fresh: Option<(CachedUiaParts, bool)>,
    /// Its first selected child, once read.
    selected_child: Option<UiaSnapshotFact>,
    /// The ancestors read, nearest first.
    ancestors: Vec<UiaAncestorFact>,
    /// The automation id of the first ancestor read, the raw-view parent.
    parent_automation_id: Option<String>,
    /// Whether the element the event's runtime id named before still has
    /// the keyboard focus, once read.
    same_as_last: Option<bool>,
    /// The read as it stands once its walk has ended, while a last call is
    /// under way (a menu item's legacy MSAA state).
    finished: Option<UiaFocusRead>,
}

impl ReadProgress {
    /// Takes in what the classic walk has just read.
    fn take_in(&mut self, read: Progress<'_>) {
        match read {
            Progress::PreviousFocused(focused) => self.same_as_last = Some(focused),
            Progress::Element(fresh) => {
                let read = fresh.read();
                self.fresh = Some((
                    read.node.parts(fresh.runtime_id()),
                    read.node.value_from_selection(),
                ));
            }
            Progress::SelectedChild(child) => {
                self.selected_child = Some(fact_of(child.read().node.parts(child.runtime_id())));
            }
            Progress::Ancestor(ancestor) => {
                if self.ancestors.is_empty() {
                    self.parent_automation_id = ancestor.read().automation_id;
                }
                self.ancestors.push(ancestor_fact(ancestor));
            }
        }
    }
}

/// The focus a read's progress reads as: its name and role from the
/// event's cache, its value, states and details as read afresh when they
/// were, its class from the event's cache and its parent's automation id;
/// and its selected child, or, for a focus whose value is its first
/// selected item, that item's name as its value and no selected child.
fn focus_of(progress: &ReadProgress) -> (UiaSnapshotFact, Option<UiaSnapshotFact>, bool) {
    let event = &progress.event;
    let (role, value, states, details, value_from_selection) = match &progress.fresh {
        Some((live, from_selection)) => (
            live.role,
            live.value.clone(),
            live.states,
            live.details.clone(),
            *from_selection,
        ),
        None => (
            event.role,
            event.value.clone(),
            event.states,
            event.details.clone(),
            false,
        ),
    };
    let mut snapshot = UiaSnapshotFact {
        runtime_id: event.runtime_id.clone(),
        role,
        name: event.name.clone(),
        value,
        states: states.with(State::Focused),
        details: NodeDetails {
            class_name: event.details.class_name.clone(),
            parent_automation_id: progress.parent_automation_id.clone(),
            ..details
        },
    };
    let mut selected_child = progress.selected_child.clone();
    if value_from_selection {
        snapshot.value = selected_child.take().and_then(|child| child.name);
    }
    (snapshot, selected_child, value_from_selection)
}

/// The read a focus read's progress publishes at its bound, whoever
/// publishes it: the watchdog, for a read whose call is still under way at
/// the bound, or the reading thread, for a walk that reached the bound
/// between its calls. The one function both use, so that both publish the
/// same read. The read as it stood once its walk ended, if it has; else
/// the focus as read so far ([`focus_of`]) with the ancestors read, and
/// the chain completed by plain windows from the outermost ancestor's
/// nearest window, included, or from its parent window when that ancestor
/// is the window's own element ([`verbatim_uia::plain_windows_start`],
/// [`AncestorsEnd::PlainFrom`]), which the outpost reads as plain windows,
/// named by their text. Local calls only.
fn read_at_bound(progress: &ReadProgress) -> UiaFocusRead {
    if let Some(read) = &progress.finished {
        return read.clone();
    }
    let (snapshot, selected_child, value_from_selection) = focus_of(progress);
    let plain_from = verbatim_uia::plain_windows_start(
        progress.nearest,
        std::iter::once(progress.hwnd)
            .chain(progress.ancestors.iter().map(|ancestor| ancestor.hwnd)),
    );
    UiaFocusRead {
        hwnd: progress.hwnd,
        snapshot,
        ancestors: progress.ancestors.clone(),
        ancestors_end: AncestorsEnd::PlainFrom(plain_from),
        selected_child,
        value_from_selection,
        window: Some(progress.nearest),
        same_as_last: progress.same_as_last,
        bound_us: duration_us(progress.bound),
    }
}

/// `duration` in microseconds.
fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// The kind of UIA event a reading thread takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEvent {
    /// A focus change (`HandleFocusChangedEvent`).
    Focus,
    /// A menu opened (`MenuOpened`), which NVDA maps to a focus on the menu
    /// and handles in its automation event handler
    /// (`IUIAutomationEventHandler_HandleAutomationEvent`,
    /// `UIAHandler/__init__.py` 757 to 868).
    MenuOpened,
    /// An element selected, judged by its sender's nearest window
    /// ([`FocusReader::check_fact`]).
    Selection,
    /// A notification, judged by its sender's nearest window
    /// ([`FocusReader::check_fact`]).
    Notification,
}

/// The reason a focus or selection is dropped when its sender has no
/// window above it, which in a new File Explorer window is a sender gone
/// before its read (`docs/design/listener-without-connections.md`, risk 13).
pub const NO_WINDOW: &str = "it has no window above it, so NVDA makes no object for it";

/// The reason a focus is dropped when the lookup of its sender's nearest
/// window did not answer within UIA's transaction timeout
/// (`verbatim_uia::nearest_window_lookup`): NVDA's lookup answers no window
/// and NVDA drops the focus, and nothing more is read into an application
/// that has just failed to answer (`docs/design/focus-pipeline-rework.md`,
/// the audit's difference 14).
pub const NEAREST_TIMED_OUT: &str = "its nearest window's lookup timed out, so NVDA drops it";

/// The reason a notification is dropped when its sender has no window
/// above it.
const NOTIFICATION_NO_WINDOW: &str = "its sender has no window above it";

/// The reason a focus is dropped whose element no longer has the keyboard
/// focus when it is read, as NVDA drops it.
pub const NOT_FOCUSED: &str = "its element no longer has the keyboard focus";

/// The reason an event is dropped that a newer event of the same kind and
/// element replaced while it waited, as NVDA's UIA limiter keeps one.
const REPLACED: &str = "a newer focus event of its element replaced it";

/// The reason a focus is dropped whose own window the system reports hung.
const HUNG: &str = "its window is hung";

/// The reason a menu opening is dropped whose comparison with the current
/// focus failed.
const COMPARISON_FAILED: &str = "its comparison with the current focus failed";

/// The reason a focus is dropped that repeats the focus Core holds.
const REPEAT: &str = "a repeat of the last focus, which still has the keyboard focus";

/// The reason a focus is dropped that fails NVDA's acceptance test.
const NOT_RELATED: &str = "its window is not related to the foreground window";

/// The reason a focus in Core's hidden frame is dropped, as its outpost
/// would drop it once read.
const HIDDEN_FRAME: &str = "Core's hidden frame";

/// The reason the console window's own element's focus is dropped.
const CONSOLE_OWN: &str = "the console window's own focus";

/// The reason a focus is skipped whose nearest window's class is on
/// NVDA's MSAA class list, by the class verdict the outposts' arbitrators
/// use ([`class_verdict`]), so the outpost judges that window MSAA too.
pub const MSAA_CLASS: &str =
    "its window's class is on NVDA's MSAA list, so its MSAA events speak for it";

/// The drops of a focus that are NVDA's checks' verdict on its event, as
/// its read would have been, which answer its application's gap record
/// ([`gap_answer`]): the class-list skip among them, since the outpost
/// judges that window MSAA by the same function.
const VERDICTS: [&str; 7] = [
    MSAA_CLASS,
    HUNG,
    COMPARISON_FAILED,
    REPEAT,
    NOT_RELATED,
    CONSOLE_OWN,
    HIDDEN_FRAME,
];

/// The drops of a focus because the listener gave up on its application,
/// which did not answer in time, which answer the application's gap
/// record wherever its copy is ([`gap_answer`]): a read into that
/// application by the outpost would call into one that has just failed to
/// answer, so the wait ends without one (Dickson, 2026-10-21). The drop
/// whose event said it lacked the keyboard focus when the read's time was
/// up is among them, a judgement call of its own (Dickson, 2026-10-21):
/// its time ran out as the others' did; so is a nearest window's lookup
/// that timed out (`docs/design/focus-pipeline-rework.md`, the audit's
/// difference 14).
const GAVE_UP: [&str; 5] = [
    NOT_IN_TIME,
    WAITED_TOO_LONG,
    OUT_OF_TIME_BEFORE_LOOKUP,
    UNFOCUSED_OUT_OF_TIME,
    NEAREST_TIMED_OUT,
];

/// What became of one focus event.
#[derive(Debug)]
pub enum Judged {
    /// The focus, read.
    Read(Box<UiaFocusRead>),
    /// Not reported, for the reason given.
    Dropped(&'static str),
    /// A selection or notification that passed its checks, to be passed on
    /// as it now stands ([`FocusReader::check_fact`]).
    Passed(Box<DeliveredFact>),
}

impl Judged {
    /// The fact that resolves marker `marker`, sent for an event of kind
    /// `kind`, with this outcome; a drop carries its answer to the
    /// application's gap record ([`gap_answer`]), placed in the top-level
    /// window `placed` when its read found one.
    #[must_use]
    pub fn into_fact(self, marker: u64, kind: FocusEvent, placed: Option<isize>) -> DeliveredFact {
        let answer = gap_answer(kind, &self, placed);
        match self {
            Judged::Read(read) => DeliveredFact::UiaFocusRead {
                marker,
                read,
                focus: kind == FocusEvent::Focus,
            },
            Judged::Dropped(_) => DeliveredFact::UiaFocusDropped {
                marker,
                focus: kind == FocusEvent::Focus,
                answer,
            },
            Judged::Passed(fact) => DeliveredFact::UiaMarked { marker, fact },
        }
    }

    /// Why it was not reported, for one that was dropped.
    #[must_use]
    pub fn dropped_reason(&self) -> Option<&'static str> {
        match self {
            Judged::Dropped(reason) => Some(reason),
            Judged::Read(_) | Judged::Passed(_) => None,
        }
    }
}

/// Whether a focus whose nearest window has the classes `classes` is
/// skipped ([`MSAA_CLASS`]): the class verdict every outpost's arbitrator
/// gives calls it MSAA ([`class_verdict`]), unless a backend override
/// forcing UIA holds for its application (`forced_uia`).
fn skipped_by_class(classes: &WindowClasses, forced_uia: bool) -> bool {
    !forced_uia && class_verdict(classes) == ClassVerdict::NonUia
}

/// What the outcome `judged` of an event of kind `kind`, placed in the
/// top-level window `placed` when its read found one, says of its
/// application's gap record (`docs/design/focus-pipeline-rework.md`, "Who
/// owns each duplicated fact", the gap's evidence), which the drop carries
/// to the outpost, whose intake keeps the record. Only a focus's outcome
/// answers it. A read and a verdict of NVDA's checks ([`VERDICTS`]), the
/// class-list skip among them, answer a copy in their top-level window, or
/// in any window when the read found none. The
/// listener giving up on the application ([`GAVE_UP`]) answers a copy in
/// any window. A drop for no window above the sender ([`NO_WINDOW`])
/// answers a copy in any window too, which its outpost is to read at once
/// in its place: the focus came and its sender, gone, cannot be read, and
/// the event for where the focus
/// went may never come (`docs/design/listener-without-connections.md`,
/// risk 13). Any other drop leaves the record open: one no longer focused
/// ([`NOT_FOCUSED`]) or replaced by a newer event of its element comes in
/// fast navigation, where it says nothing of the copy's focus (Dickson,
/// 2026-10-21), and so do the rest, which could not establish the focus.
#[must_use]
pub fn gap_answer(kind: FocusEvent, judged: &Judged, placed: Option<isize>) -> GapAnswer {
    if kind != FocusEvent::Focus {
        return GapAnswer::LeavesOpen;
    }
    match judged {
        Judged::Read(_) => GapAnswer::Answers(placed),
        Judged::Dropped(reason) if VERDICTS.contains(reason) => GapAnswer::Answers(placed),
        Judged::Dropped(reason) if GAVE_UP.contains(reason) => GapAnswer::Answers(None),
        Judged::Dropped(reason) if *reason == NO_WINDOW => GapAnswer::ReadsNow,
        Judged::Dropped(_) | Judged::Passed(_) => GapAnswer::LeavesOpen,
    }
}

/// The focus a reading thread read last, for NVDA's duplicate evidence
/// and for the ancestor read to stop where it meets this focus's.
struct LastFocus {
    runtime_id: Vec<i32>,
    element: IUIAutomationElement,
    /// Its raw-view ancestors, nearest first, or `None` when its read left
    /// them unknown: the next read then reuses nothing of this focus.
    ancestors: Option<Vec<UiaAncestorFact>>,
}

/// The elements of the UIA focuses the listener's readers accepted, by
/// how Core has judged them (decision D14, "Where the rules run"): the one
/// behind the focus Core holds, when Core took it from one of these reads,
/// and those Core has not judged yet. Shared by every reading thread, as
/// NVDA keeps one last focus for the whole program
/// (`docs/design/focus-pipeline.md`, section 16.3). A focus event whose
/// sender UI Automation counts as the element of the focus Core holds,
/// while that element still has the keyboard focus, is a repeat and is
/// dropped before it is read ([`FocusReader::read`]), but only while Core
/// has judged every focus the listener has accepted
/// ([`verbatim_core::focus_rules::repeats_focus`]): until then the focus
/// Core holds can yet become another, as NVDA's last queued focus is
/// replaced by each focus it queues. A menu opening is compared with the
/// focus Core holds, as NVDA compares it with its focus object. Each
/// thread's own focus before still serves the ancestor read's stop and the
/// evidence that a runtime id now names another element (`same_as_last`).
///
/// Core tells the listener of every focus event it judges and every change
/// of its focus (`SupervisorToOutpost::CoreFocus`), so an element whose
/// focus Core refused, such as a shell window that only stages a switch,
/// never stands for the focus Core holds. An element is released by a
/// reading thread, in COM's apartment, once Core no longer holds it and has
/// judged it: the notice only marks it.
#[derive(Clone, Default)]
pub struct HeldElements(Arc<Mutex<Vec<Accepted>>>);

/// One focus a reader accepted: the number of the reader, the element, the
/// window it was judged with (NVDA's object's `windowHandle`), the event it
/// came in, and how Core has judged it.
struct Accepted {
    reader: u64,
    element: AgileReference<IUIAutomationElement>,
    window: isize,
    arrival: Arrival,
    judged: Judgement,
}

/// How Core has judged an accepted focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Judgement {
    /// Not yet.
    Waiting,
    /// It is the focus Core holds.
    Held,
    /// Judged, and not the focus Core holds: released by the next reading
    /// thread that takes the lock.
    Done,
}

/// The UIA event a focus came in: its trace, and when the listener
/// received it, on the run's shared clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    /// The trace minted for the event.
    pub trace: TraceId,
    /// When the event was received, in microseconds on the run's shared
    /// clock.
    pub observed_at_us: u64,
}

impl HeldElements {
    fn lock(&self) -> MutexGuard<'_, Vec<Accepted>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The element of the focus Core holds, with the window it was judged
    /// with, when Core took that focus from one of these reads and it can
    /// be had on this thread; and whether Core has judged every focus the
    /// listener accepted.
    fn held(&self) -> (Option<(IUIAutomationElement, isize)>, bool) {
        let entries = self.lock();
        let settled = entries
            .iter()
            .all(|accepted| accepted.judged != Judgement::Waiting);
        let held = entries
            .iter()
            .find(|accepted| accepted.judged == Judgement::Held)
            .and_then(|accepted| {
                accepted
                    .element
                    .resolve()
                    .ok()
                    .map(|element| (element, accepted.window))
            });
        (held, settled)
    }

    /// Records `element`, which reader `reader` accepted from the event
    /// `arrival` and judged with window `window` in the read `read`, as a
    /// focus Core has not judged yet, unless the watchdog has abandoned that
    /// read, when Core is told of its drop instead and never judges it, or
    /// published it at its bound, when Core judges the published focus,
    /// which holds no element here. The read's claim is asked under the
    /// lock, and the watchdog claims a read before a newer read can begin. Releases, on this
    /// reading thread, the elements Core has judged and does not hold.
    fn accept(
        &self,
        reader: u64,
        element: &IUIAutomationElement,
        (window, arrival): (isize, Arrival),
        read: &WatchedRead,
    ) {
        match AgileReference::new(element) {
            Ok(element) => {
                let mut entries = self.lock();
                entries.retain(|accepted| accepted.judged != Judgement::Done);
                if read.claimed() {
                    tracing::debug!(
                        "an abandoned read's focus, or one published at its bound, is not kept for Core's judgement"
                    );
                    return;
                }
                entries.push(Accepted {
                    reader,
                    element,
                    window,
                    arrival,
                    judged: Judgement::Waiting,
                });
            }
            Err(error) => {
                tracing::debug!(%error, "an accepted focus could not be kept");
            }
        }
    }

    /// Core judged the focus event traced `judged`, if any, and holds the
    /// focus `held`, the trace of the event it took it from and whether it
    /// is alive (`SupervisorToOutpost::CoreFocus`). A judged read and every
    /// earlier read of the same reader, whose reads reach Core in the order
    /// they were accepted, are judged; the read whose trace the focus Core
    /// holds carries is that focus's element, and no other is.
    pub fn core_focus(&self, judged: Option<TraceId>, held: Option<(TraceId, bool)>) {
        let mut entries = self.lock();
        if let Some(judged) = judged
            && let Some(at) = entries.iter().position(|accepted| {
                accepted.judged == Judgement::Waiting && accepted.arrival.trace == judged
            })
        {
            let reader = entries[at].reader;
            for accepted in &mut entries[..=at] {
                if accepted.reader == reader && accepted.judged == Judgement::Waiting {
                    accepted.judged = Judgement::Done;
                }
            }
        }
        let held = held.filter(|&(_, alive)| alive).map(|(trace, _)| trace);
        for accepted in entries.iter_mut() {
            if Some(accepted.arrival.trace) == held {
                accepted.judged = Judgement::Held;
            } else if accepted.judged == Judgement::Held {
                accepted.judged = Judgement::Done;
            }
        }
    }

    /// The watchdog has abandoned the read of the event traced `trace`,
    /// whose drop Core is told of: a focus that read accepted just before
    /// is one Core never judges. Called under the readers' lock after the
    /// read is marked abandoned, so this covers an accept made before the
    /// mark, and the mark one made after ([`accept`](Self::accept)). The
    /// element is kept for a reading thread to release, in COM's apartment.
    pub(crate) fn abandoned(&self, trace: TraceId) {
        let mut entries = self.lock();
        for accepted in entries.iter_mut() {
            if accepted.arrival.trace == trace && accepted.judged == Judgement::Waiting {
                tracing::debug!(
                    ?trace,
                    "an abandoned read's focus was accepted; Core never judges it"
                );
                accepted.judged = Judgement::Done;
            }
        }
    }

    /// Forgets every focus reader `reader` accepted, and every element Core
    /// has judged and does not hold, releasing them on the thread that ends
    /// with that reader, while it is still in COM's apartment. Core judges
    /// none of the reader's focuses it has not judged yet once its
    /// application's outpost has ended, which ends the reader.
    fn release(&self, reader: u64) {
        self.lock()
            .retain(|accepted| accepted.reader != reader && accepted.judged != Judgement::Done);
    }
}

/// The next [`FocusReader`]'s number, which names the reader that accepted
/// the [`HeldElements`] focus.
static NEXT_READER: AtomicU64 = AtomicU64::new(1);

/// One application's focus reader: a UIA client of its own and the focus
/// it read last. Lives on one reading thread.
pub struct FocusReader {
    uia: Uia,
    last: Option<LastFocus>,
    /// This reader's number among the listener's readers.
    number: u64,
    /// The elements of the focuses the listener's readers accepted, by how
    /// Core has judged them.
    held_elements: HeldElements,
    /// Reads the foreground window for NVDA's acceptance test: the
    /// system's, unless a test supplies its own
    /// ([`set_foreground_reader`](Self::set_foreground_reader)).
    foreground: ForegroundReader,
    /// The time its classic walk's budget is measured by: the system's,
    /// unless a test sets its own ([`set_clock`](Self::set_clock)).
    clock: Clock,
    /// The run's shared clock, which events' observations are stamped by,
    /// for how long an event waited before its read began: the system's,
    /// unless a test sets its own
    /// ([`set_observation_clock`](Self::set_observation_clock)).
    observed: ObservationClock,
    /// The top-level window of the last focus this reader placed, for the
    /// bound of the next focus's walk.
    last_top: Option<isize>,
    /// Whether a backend override forcing UIA is set for this reader's
    /// application, so no focus is skipped by its window's class
    /// ([`SupervisorToOutpost::SetBackendOverride`](crate::protocol::SupervisorToOutpost::SetBackendOverride)).
    forced_uia: Arc<AtomicBool>,
    /// The read under way as the reading threads' watchdog watches it,
    /// told a focus's sender's nearest window once the read finds it, and asked
    /// whether it was abandoned before its focus is accepted
    /// ([`judge_watched`](Self::judge_watched)).
    window: WatchedRead,
    /// Told the name of each call of the classic walk before it is made,
    /// when a test asks ([`observe_calls`](Self::observe_calls)).
    calls: Option<CallObserver>,
    /// Judges whether a window a step of the classic walk reaches can be
    /// read, or is a ghost's or a hung application's: the system's
    /// judgement, unless a test supplies its own
    /// ([`set_window_readability_reader`](Self::set_window_readability_reader)).
    window_readability: crate::WindowReadabilityReader,
}

/// Told the name of each call a focus read's classic walk is about to make
/// ([`FocusReader::observe_calls`]).
pub type CallObserver = Arc<dyn Fn(&'static str) + Send + Sync>;

impl Drop for FocusReader {
    fn drop(&mut self) {
        self.held_elements.release(self.number);
    }
}

impl FocusReader {
    /// A reader with a UIA client of its own, made on the calling thread,
    /// which joins COM's multithreaded apartment. Every read is the classic
    /// walk, which makes no remote operation
    /// (`docs/design/listener-without-connections.md`, section 2).
    /// `held_elements` holds the elements of the focuses the listener's
    /// readers accepted, by how Core has judged them, which this one shares.
    ///
    /// # Errors
    ///
    /// The COM error when the client cannot be made.
    pub fn new(held_elements: HeldElements) -> windows::core::Result<Self> {
        Ok(Self {
            uia: Uia::new()?,
            last: None,
            number: NEXT_READER.fetch_add(1, Ordering::Relaxed),
            held_elements,
            foreground: Arc::new(crate::outpost::window::foreground_window_handle),
            clock: Arc::new(Instant::now),
            observed: Arc::new(now_us),
            last_top: None,
            forced_uia: Arc::default(),
            window: WatchedRead::default(),
            calls: None,
            window_readability: Arc::new(crate::window_unreadable),
        })
    }

    /// Tells `observer` the name of each call its reads' classic walk is
    /// about to make, before it is made (`FocusQuery::before_call`): a
    /// test that changes the application between two of a walk's calls,
    /// as between its boundary check and its step, so that the order is
    /// forced.
    pub fn observe_calls(&mut self, observer: CallObserver) {
        self.calls = Some(observer);
    }

    /// Judges with `reader` from now on, in place of the system, whether a
    /// window a step of its reads' classic walk reaches cannot be read, as
    /// an outpost does (`Outpost::set_window_readability_reader`): a test
    /// that cannot wait for Windows to judge an application hung says
    /// which window is.
    pub fn set_window_readability_reader(&mut self, reader: crate::WindowReadabilityReader) {
        self.window_readability = reader;
    }

    /// Reads the foreground window with `reader` from now on, in place of
    /// the system's, for NVDA's acceptance test: a test whose application
    /// runs on a desktop where no window can take the foreground says which
    /// window is the foreground, as it does to an outpost
    /// (`Outpost::set_foreground_reader`).
    pub fn set_foreground_reader(&mut self, reader: ForegroundReader) {
        self.foreground = reader;
    }

    /// Reads the time for the time budget of its classic walk from `clock`
    /// from now on, in place of the system's, as an outpost does
    /// (`Outpost::set_clock`): a test that counts a read's calls decides
    /// when the walk's time is up, however long its calls take.
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    /// Reads now on the run's shared clock from `clock` from now on, in
    /// place of the system's, as an outpost does
    /// (`Outpost::set_observation_clock`): a test that stamps its events'
    /// observations by its own clock has how long each waited before its
    /// read measured by the same clock.
    pub fn set_observation_clock(&mut self, clock: ObservationClock) {
        self.observed = clock;
    }

    /// The reader's clock ([`set_clock`](Self::set_clock)), which its
    /// classic walk measures its time budget by.
    fn clock(&self) -> Clock {
        Arc::clone(&self.clock)
    }

    /// Whether a backend override forcing UIA is set for its application,
    /// shared with whoever sets it ([`forces_uia`](Self::forces_uia)).
    pub(crate) fn share_forced_uia(&mut self, forced: Arc<AtomicBool>) {
        self.forced_uia = forced;
    }

    /// Sets whether a backend override forcing UIA holds for its
    /// application: while one does, no focus is skipped by its window's
    /// class, since the outpost then counts it.
    pub fn forces_uia(&self, forced: bool) {
        self.forced_uia.store(forced, Ordering::Relaxed);
    }

    /// [`read`](Self::read), or for a selection or a notification, whose
    /// captured `fact` is given, [`check_fact`](Self::check_fact), in the
    /// read `watched`, which the reading threads' watchdog watches
    /// ([`WatchedRead`]): a focus's read tells it the sender's nearest
    /// window once it has found it, and a read the watchdog has abandoned
    /// accepts no focus.
    pub(crate) fn judge_watched(
        &mut self,
        sender: &Cached<ListenerEvent>,
        (kind, fact): (FocusEvent, Option<DeliveredFact>),
        (arrival, watched): (Arrival, WatchedRead),
    ) -> Judged {
        self.window = watched;
        let judged = match fact {
            Some(fact) => self.check_fact(sender, fact),
            None => self.read(sender, kind, arrival),
        };
        self.window = WatchedRead::default();
        judged
    }

    /// Judges and reads the UIA focus event, or menu opening (`kind`),
    /// whose sender is `sender`, as the registration delivers it, and which
    /// came as `arrival`.
    ///
    /// # Panics
    ///
    /// Never: the read's progress, armed once its checks pass, is taken
    /// only by the read, once its walk has ended.
    #[expect(
        clippy::too_many_lines,
        reason = "the read's checks, query and outcome, read top to bottom as they run"
    )]
    pub fn read(
        &mut self,
        sender: &Cached<ListenerEvent>,
        kind: FocusEvent,
        arrival: Arrival,
    ) -> Judged {
        let fields = sender.read();
        let element = sender.live().element();
        let hwnd = fields.role.hwnd;
        let mut event = fields.parts(sender.runtime_id());
        // The sender's own UIA class, from the event's cache, as NVDA's app
        // modules read it (`cachedClassName`), for Core's rules for an
        // application's elements (decision D6): no call.
        event.details.class_name.clone_from(&fields.role.class_name);
        // The read's time budget, from here, for its checks; its walk's is
        // its bound, measured from the event's observation, so the time
        // the event waited before its read began counts against it.
        let budget = Budget::new(self.clock());
        let waited =
            Duration::from_micros((self.observed)().saturating_sub(arrival.observed_at_us));
        let out_of_time = || budget.out_of_time();
        let judged = self.judge_before_reading(sender, (&fields, &event.runtime_id), kind, &budget);
        // The focus's top-level window, once its window is known, placed in
        // the order this application's focuses arrive: the walk's bound is
        // the short one when it is not the top-level window of the focus
        // before, as the listener observed them (the Terms of
        // `docs/design/focus-pipeline-rework.md`), and another
        // application's focus before it is always elsewhere. A local call.
        let known = match judged {
            Ok(window) => window,
            Err(_) if hwnd != 0 => hwnd,
            Err(_) => self.window.window(),
        };
        let top = Some(known)
            .filter(|&window| window != 0)
            .map(top_level_of)
            .filter(|&top| top != 0);
        let moved = self.window.after_elsewhere() || top.is_none() || top != self.last_top;
        if let Some(top) = top {
            self.last_top = Some(top);
            self.window.place(top);
        }
        let nearest = match judged {
            Ok(nearest) => nearest,
            Err(reason) => return Judged::Dropped(reason),
        };
        let previous = self
            .last
            .as_ref()
            .filter(|last| last.runtime_id == event.runtime_id)
            .map(|last| last.element.clone());
        // The focus before and its ancestors, nearest first: where the
        // ancestor read stops ([`FocusQuery::known`]). A focus before whose
        // ancestors are unknown is left out, itself included, since the
        // chain reused from where the read meets it would end there with
        // nothing above it: the read then runs as a first read does.
        let known: Vec<Vec<i32>> = self
            .last
            .as_ref()
            .and_then(|last| {
                let ancestors = last.ancestors.as_ref()?;
                Some(
                    std::iter::once(last.runtime_id.clone())
                        .chain(
                            ancestors
                                .iter()
                                .map(|ancestor| ancestor.snapshot.runtime_id.clone()),
                        )
                        .collect(),
                )
            })
            .unwrap_or_default();
        // The read has passed every check NVDA makes before it queues the
        // focus: from here its progress is shared with the watchdog, which
        // publishes it at the walk's bound should a call still be under
        // way then (`docs/design/focus-pipeline-rework.md`, "A window's
        // name, and the one fallback").
        let bound = if moved { MOVED_ON_GRACE } else { READ_BUDGET };
        let left = budget.bound_walk(bound.saturating_sub(waited));
        self.window.arm(ReadProgress {
            bound_at: Instant::now() + left,
            bound,
            hwnd,
            nearest,
            event: fact_of(event),
            fresh: None,
            selected_child: None,
            ancestors: Vec::new(),
            parent_automation_id: None,
            same_as_last: None,
            finished: None,
        });
        // Every detail: the listener reads for every application, whatever
        // theme decides which details are spoken. A request that cannot be
        // built is a read that failed.
        // The ancestors are read without their place or their cell, as
        // every read of many elements is, and the focus with them
        // (`verbatim_uia::Place`, `verbatim_uia::Cell`).
        let input = sender.narrow::<FocusInput>();
        let ancestors = AncestorGates {
            node: NodeGates {
                place: None,
                cell: None,
                ..Node::all_on()
            },
            ..Ancestor::all_on()
        };
        let read = self
            .uia
            .request::<Ancestor>(ancestors)
            .and_then(|request| Ok((request, self.uia.request::<Ancestor>(Ancestor::all_on())?)))
            .map_err(verbatim_uia_rops::Error::Uia)
            .and_then(|(request, element_request)| {
                let fill = Fill::new(&request);
                let element_fill = Fill::new(&element_request);
                // The sender's keyboard focus was read plainly just now, as
                // NVDA's last check, and its nearest window found for NVDA's
                // native check, so the read asks neither again
                // (`docs/design/listener-without-connections.md`, section 3,
                // step 8).
                let query = FocusQuery {
                    element: &input,
                    known: &known,
                    previous: previous.as_ref(),
                    depth_limit: MAX_ANCESTORS,
                    fill: &fill,
                    element_fill: &element_fill,
                    out_of_time: Some(&out_of_time),
                    require_focus: false,
                    refresh: true,
                    nearest_window: Some(nearest),
                    before_call: Some(&|call| {
                        budget.call(call);
                        if let Some(observer) = &self.calls {
                            observer(call);
                        }
                    }),
                    // Read from the element's fresh copy.
                    value_available: None,
                    // A ghost's or a hung application's window that a step
                    // reaches ends the walk, by the system's judgement: local
                    // calls.
                    plain: Some(&|window| (self.window_readability)(window).is_some()),
                    // Each fact the walk reads is shared with the watchdog
                    // as it is read.
                    progress: Some(&|read| self.window.advance(|progress| progress.take_in(read))),
                };
                focus_ancestry_classic(&self.uia, &query)
            });
        // The walk has ended: the watchdog has nothing more to publish from,
        // and what the read reports is made from what it shared.
        let progress = self
            .window
            .take_progress()
            .expect("the read's progress, armed above and taken only here");
        match read {
            Ok(FocusAncestry::NotFocused) => Judged::Dropped(NOT_FOCUSED),
            Ok(FocusAncestry::Focused(ancestry)) => {
                self.focused(element, progress, &ancestry, (arrival, &budget))
            }
            Err(error) => {
                // NVDA lets a focus event through when its sender's keyboard
                // focus cannot be read (`shouldAllowUIAFocusEvent` answers
                // true on a COM error): reported as the event's cache gives
                // it, whatever that cache says of its keyboard focus. Step 6
                // has already judged that: a live read that answered true,
                // or failed, lets the focus through in NVDA too, and a cache
                // that said no has already dropped it out of time. Its chain
                // is the ancestors read before the failure, completed by
                // plain windows from the window the outermost of them, or
                // the focus, is in, as at the walk's bound
                // (`docs/design/focus-pipeline-rework.md`, "A window's name,
                // and the one fallback").
                tracing::debug!(%error, "a UIA focus could not be read; reported from its event");
                self.last = Some(LastFocus {
                    runtime_id: progress.event.runtime_id.clone(),
                    element: element.clone(),
                    ancestors: None,
                });
                self.held_elements
                    .accept(self.number, element, (nearest, arrival), &self.window);
                // Reported as the focus, as `focused` reports one, whatever
                // the event's cache says of its keyboard focus.
                // An outermost element, the focus or an ancestor, that is
                // a window's own is completed from its parent window, so
                // its own window is not in the chain twice.
                let plain_from = verbatim_uia::plain_windows_start(
                    nearest,
                    std::iter::once(hwnd)
                        .chain(progress.ancestors.iter().map(|ancestor| ancestor.hwnd)),
                );
                let mut snapshot = progress.event;
                snapshot.states = snapshot.states.with(State::Focused);
                Judged::Read(Box::new(UiaFocusRead {
                    hwnd,
                    snapshot,
                    ancestors: progress.ancestors,
                    ancestors_end: AncestorsEnd::PlainFrom(plain_from),
                    selected_child: None,
                    value_from_selection: false,
                    window: Some(nearest),
                    same_as_last: None,
                    bound_us: duration_us(bound),
                }))
            }
        }
    }

    /// What NVDA checks of a selection's or a notification's sender,
    /// `element`, before it handles one, here where a cross-process call
    /// may be made: for a sender that is not a window, its nearest window,
    /// one call (`getNearestWindowHandle`). A selection whose sender has
    /// none is dropped, as NVDA's object for it could not be made, and one
    /// found is the window the outpost tests the selection's window and
    /// acceptance with, as NVDA tests them with the nearest window
    /// (`HandleAutomationEvent`, `UIAHandler/__init__.py` 819 to 855). A
    /// notification whose sender has none is dropped, as the app module
    /// default drops it (`shouldProcessUIANotificationEvent`,
    /// `appModuleHandler.py` 842 to 845), unless it is the shell's window
    /// snap results, which File Explorer's app module lets through
    /// (`appModules/explorer.py` 519 to 535). A sender that is a window
    /// takes no call. The check is never judged by the moved-on rule: NVDA
    /// makes it on UIA's callback thread, whose calls its watchdog never
    /// cancels, so a slow check is passed on late rather than dropped.
    pub fn check_fact(&mut self, sender: &Cached<ListenerEvent>, fact: DeliveredFact) -> Judged {
        let element = sender.live().element();
        match fact {
            DeliveredFact::UiaSelection {
                hwnd,
                focus_window: _,
                snapshot,
            } => {
                let nearest = if hwnd == 0 {
                    match verbatim_uia::nearest_window_handle(element) {
                        Some(nearest) => nearest,
                        None => {
                            return Judged::Dropped(NO_WINDOW);
                        }
                    }
                } else {
                    0
                };
                Judged::Passed(Box::new(DeliveredFact::UiaSelection {
                    hwnd,
                    focus_window: nearest,
                    snapshot,
                }))
            }
            DeliveredFact::UiaNotification {
                hwnd,
                snapshot,
                notification,
            } => {
                let snap_results = notification.activity_id.as_deref()
                    == Some(verbatim_core::SNAP_RESULTS_ACTIVITY);
                if hwnd == 0
                    && !snap_results
                    && verbatim_uia::nearest_window_handle(element).is_none()
                {
                    return Judged::Dropped(NOTIFICATION_NO_WINDOW);
                }
                Judged::Passed(Box::new(DeliveredFact::UiaNotification {
                    hwnd,
                    snapshot,
                    notification,
                }))
            }
            other => Judged::Passed(Box::new(other)),
        }
    }

    /// The read of a focus whose element had the keyboard focus, from what
    /// the read shared as it read it, `progress`, and how its walk ended,
    /// `ancestry`: its name and role from the event's cache, as NVDA reads
    /// them from the event's element, and its value, states, and details
    /// afresh ([`focus_of`]); its ancestors as read, followed by those of
    /// the focus before above the one the read stopped at. A walk that
    /// reached its bound between its calls is read as the watchdog
    /// publishes one at the bound ([`read_at_bound`]). `arrival` is the
    /// event it came in.
    fn focused(
        &mut self,
        element: &IUIAutomationElement,
        mut progress: ReadProgress,
        ancestry: &Ancestry,
        (arrival, budget): (Arrival, &Budget),
    ) -> Judged {
        let path = "classic";
        let (hwnd, nearest) = (progress.hwnd, progress.nearest);
        // A walk whose step to a parent failed or found none below the root
        // goes on from the parent window of its outermost element's nearest
        // window, as NVDA's failed parent read falls back to the window
        // hierarchy rather than ending the chain; one cut at the depth limit
        // is completed by plain windows from that nearest window, or from
        // its parent window when the outermost element is its own, and one
        // cut by its bound is read as the watchdog publishes one at the
        // bound. The listener connects to neither: it names the window, and
        // the outpost goes on from it.
        // Local calls, made only for a walk that did not reach the top.
        let outermost = || {
            verbatim_uia::outermost_nearest_window(
                nearest,
                std::iter::once(hwnd)
                    .chain(progress.ancestors.iter().map(|ancestor| ancestor.hwnd)),
            )
        };
        // A walk that stopped at a step into a window read as a plain
        // window, a ghost's or a hung application's, goes on from that
        // window, which the outpost reads as a plain window, and on by the
        // window hierarchy, as NVDA goes on from its plain window.
        let ancestors_end = if let Some(plain) = ancestry.plain_window {
            AncestorsEnd::ContinueFrom(plain)
        } else if ancestry.failed_hop.is_some() {
            AncestorsEnd::ContinueFrom(verbatim_uia::parent_window(outermost()))
        } else if ancestry.depth_limited {
            AncestorsEnd::PlainFrom(verbatim_uia::plain_windows_start(
                nearest,
                std::iter::once(hwnd)
                    .chain(progress.ancestors.iter().map(|ancestor| ancestor.hwnd)),
            ))
        } else {
            AncestorsEnd::Top
        };
        tracing::debug!(
            path,
            read = progress.ancestors.len(),
            met_known = ?ancestry.met_known,
            depth_limited = ancestry.depth_limited,
            out_of_time = ancestry.out_of_time,
            failed_hop = ?ancestry.failed_hop,
            window = ?ancestry.window,
            "UIA focus ancestors read"
        );
        let mut read = if ancestry.out_of_time && ancestry.plain_window.is_none() {
            read_at_bound(&progress)
        } else {
            let (snapshot, selected_child, value_from_selection) = focus_of(&progress);
            let tail = match (ancestry.met_known, &self.last) {
                (
                    Some(met),
                    Some(LastFocus {
                        ancestors: Some(before),
                        ..
                    }),
                ) => reused_ancestors(before, met),
                _ => Vec::new(),
            };
            let ancestors = std::mem::take(&mut progress.ancestors)
                .into_iter()
                .chain(tail)
                .collect::<Vec<_>>();
            UiaFocusRead {
                hwnd,
                snapshot,
                ancestors,
                ancestors_end,
                selected_child,
                value_from_selection,
                // The classic walk's steps go through the client side,
                // whose window proxy gives a window's element its handle
                // and title, the console window's included (16.4), and it
                // walks to the desktop's root or the process boundary
                // itself; the window is the nearest window found for the
                // acceptance test when the walk named none.
                window: ancestry.window.or(Some(nearest)),
                same_as_last: ancestry.previous_focused,
                bound_us: duration_us(progress.bound),
            }
        };
        log_ancestor_facts(path, &read.ancestors);
        if wants_legacy_checked_state(read.snapshot.role, read.snapshot.states)
            && !budget.out_of_time()
        {
            // A menu item without the Toggle pattern: its checked state may
            // be only in its legacy MSAA state, which the classic walk
            // leaves to this one live call, made while the read's time
            // lasts. Until it returns, the watchdog publishes the read as
            // it stands should the bound pass.
            progress.finished = Some(read.clone());
            self.window.restore_progress(progress);
            budget.call("the legacy MSAA state");
            let legacy = element
                .current_i32_ignoring_default(UIA_LegacyIAccessibleStatePropertyId)
                .ok()
                .flatten();
            self.window.take_progress();
            add_legacy_checked_state(&mut read.snapshot.states, legacy);
        }
        // Only a chain that reaches the top is where a later read may stop
        // and reuse the rest, since the outpost goes on from a cut one.
        self.last = Some(LastFocus {
            runtime_id: read.snapshot.runtime_id.clone(),
            element: element.clone(),
            ancestors: (read.ancestors_end == AncestorsEnd::Top).then(|| read.ancestors.clone()),
        });
        self.held_elements
            .accept(self.number, element, (nearest, arrival), &self.window);
        Judged::Read(Box::new(read))
    }

    /// What a focus event goes through before anything of its sender,
    /// `element`, whose own window is `hwnd` and whose runtime id is
    /// `runtime_id`, is read: a port of NVDA's focus handler,
    /// `IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent`
    /// (`UIAHandler/__init__.py` 874 to 958), step by step in its order
    /// (`docs/design/focus-pipeline.md`, section 18). Answers the window
    /// the focus is judged with, the sender's nearest window unless a menu
    /// opening is redirected, or why the event is dropped.
    ///
    /// A menu opening (`kind`) goes through NVDA's automation event
    /// handler instead, `IUIAutomationEventHandler_HandleAutomationEvent`
    /// (757 to 868), which has no duplicate test (step 3) and, before the
    /// native check (step 2), compares the sender with the current focus:
    /// a sender UI Automation counts as the current focus is redirected to
    /// it, judged with the focus's window and its keyboard focus read from
    /// the focus's element, without the native check. Its drop while a
    /// focus is pending is made as it arrives (`Readers::focus_pending`).
    /// As NVDA queues it as a focus on the menu, its read is kept for Core's
    /// judgement as a focus's is.
    ///
    /// The read's time, `budget`, is asked before each call after the
    /// first. Once it is up, a focus whose window's verdict is known is
    /// still spoken (Dickson, 2026-10-20): every check of NVDA's focus
    /// handler and of `shouldAllowUIAFocusEvent`
    /// (`NVDAObjects/UIA/__init__.py`) that needs no call into the
    /// application is made, and one that needs a call is skipped and lets
    /// focus through, as NVDA lets it through when such a read fails
    /// (`docs/design/listener-without-connections.md`, section 5). Made:
    /// the hung window (step 1), from the event's cached window; the native
    /// check (step 2), its window's class alone, with no call
    /// ([`native_window`](Self::native_window)); the acceptance test and
    /// the refusals (steps 4 and 5), local calls; and the sender's keyboard
    /// focus (step 6) from the event's cached `HasKeyboardFocus` in place
    /// of NVDA's live read, a sender whose event says it did not have the
    /// focus being dropped. Skipped, a deliberate divergence from NVDA: the
    /// repeat check (step 3), whose comparison the runtime ids could answer
    /// but which drops a repeat only while the element of the focus Core
    /// holds still has the keyboard focus, read live from that element,
    /// which no cache holds; a repeat out of time is spoken. A menu opening's comparison is its
    /// read's first call, made within the budget.
    fn judge_before_reading(
        &self,
        held: &Cached<ListenerEvent>,
        (sender, runtime_id): (&ListenerEvent, &[i32]),
        kind: FocusEvent,
        budget: &Budget,
    ) -> Result<isize, &'static str> {
        let element = held.live().element();
        let hwnd = sender.role.hwnd;
        let out_of_time = || budget.out_of_time();
        // 1. A sender whose own window, from the event's cache, the system
        // reports hung (`_shouldSkipEventForHungWindow`, `utils.py` 438):
        // its cached handle only, so a windowless sender is not tested
        // here, and nothing is read live from a hung application.
        if hwnd != 0 && window_is_hung(hwnd) {
            return Err(HUNG);
        }
        // The focus Core holds, as far as the listener holds its element:
        // Core took it from a UIA focus this listener read (decision D14,
        // "Where the rules run"; section 16.3). An MSAA focus, a foreground
        // change, or a focus Core asked for holds no element of the
        // listener's, and NVDA's last focus or focus object is then no UIA
        // object either, so nothing is compared with it. While a focus the
        // listener read waits for Core's judgement, Core's focus can yet
        // become it, so nothing is a repeat ([`HeldElements`]).
        let (current, settled) = self.held_elements.held();
        let compare = || {
            let same = current
                .as_ref()
                .filter(|_| !out_of_time())
                .map(|(focus, _)| {
                    budget.call("the comparison with the current focus");
                    self.uia.compare_elements(element, focus)
                });
            if tracing::enabled!(tracing::Level::DEBUG) {
                Self::log_sender(sender, runtime_id, same.as_ref());
            }
            same
        };
        // The window the focus is judged with, and the element whose
        // keyboard focus decides it (step 6): NVDA's object's.
        let (window, subject) = match kind {
            FocusEvent::MenuOpened => {
                // NVDA's automation event handler (`UIAHandler/__init__.py`
                // 806 to 824) compares the sender with its focus object's
                // element (`compareElements` with `api.getFocusObject()`,
                // when that is a UIA object) before anything else, and a
                // sender that is the focus is the focus object: the event
                // goes to it, judged with its window, and the native check
                // is not made. NVDA catches no error from the comparison, so
                // one that fails drops the event.
                let same = compare();
                match (&same, current.as_ref()) {
                    (Some(Err(_)), _) => {
                        return Err(COMPARISON_FAILED);
                    }
                    (Some(Ok(true)), Some((focus, window))) => (*window, focus),
                    _ => (
                        self.native_window(held, sender, budget, same.is_some())?,
                        element,
                    ),
                }
            }
            // Only focuses and menu openings are read; a selection or a
            // notification is checked ([`Self::check_fact`]).
            FocusEvent::Focus | FocusEvent::Selection | FocusEvent::Notification => {
                let nearest = self.native_window(held, sender, budget, false)?;
                // 3. The repeat rule (`focus_rules::repeats_focus`; NVDA's
                // duplicate rule, `UIAHandler/__init__.py` 902 to 923;
                // section 16.3): Core has judged every focus the listener
                // read, the sender is the element of the focus Core holds,
                // as UIA compares them, and that element still has the
                // keyboard focus, read live. A comparison that fails is no
                // duplicate, as NVDA lets the event through on a COM error,
                // and so is a keyboard focus read that fails: the earlier
                // element is gone. Two calls, the second only for an element
                // UIA equates, and neither while a focus waits for Core's
                // judgement. Out of time, neither is made, and the focus is
                // no repeat.
                let same = if settled {
                    compare()
                } else {
                    if tracing::enabled!(tracing::Level::DEBUG) {
                        Self::log_sender(sender, runtime_id, None);
                    }
                    None
                };
                let held = current
                    .as_ref()
                    .filter(|_| matches!(same, Some(Ok(true))) && kind == FocusEvent::Focus);
                let still_focused = held.is_some_and(|(last, _)| {
                    !out_of_time() && {
                        budget.call("the last focus's keyboard focus");
                        last.has_keyboard_focus().unwrap_or(false)
                    }
                });
                if repeats_focus(RepeatEvidence {
                    settled,
                    same_element: held.is_some(),
                    still_focused,
                }) {
                    return Err(REPEAT);
                }
                (nearest, element)
            }
        };
        // 4. NVDA's acceptance test against the same window
        // (`shouldAcceptEvent`, `UIAHandler/__init__.py` 924 to 930, and
        // for a menu opening 825 to 837; sections 16.2 and 16.3). A focus
        // that fails is dropped and never reaches Core, so the same
        // element's focus once its window is in front, as a console's text
        // area's comes, is no repeat.
        // Local calls, timed apart from the call before them.
        budget.call("the acceptance test and the window checks");
        let windows = SystemWindows {
            foreground: (self.foreground)(),
        };
        if !accepts_event(
            EventName::GainFocus,
            window,
            &windows,
            &RequestedEvents::default(),
        ) {
            return Err(NOT_RELATED);
        }
        // 5. The refusals of NVDA's object for the sender, before its
        // keyboard focus is read (`shouldAllowUIAFocusEvent` overridden):
        // the console host's window element, which reports the keyboard
        // focus whenever its text area has it, and whose focus events, which
        // come around the text area's, are never the focus
        // (`consoleUIAWindow`, `NVDAObjects/UIA/winConsoleUIA.py` 358). It
        // is the one element whose own window is the console window; the
        // text area has none.
        if hwnd != 0 && window_class_name(hwnd) == CONSOLE_WINDOW_CLASS {
            return Err(CONSOLE_OWN);
        }
        // A focus in Core's hidden frame, which its outpost drops whatever
        // it is: dropped before it is read, so it never waits for a
        // judgement of Core's that cannot come ([`HeldElements`]). A local
        // read of the window's property.
        if window_belongs_to_hidden_frame(if hwnd != 0 { hwnd } else { window }) {
            return Err(HIDDEN_FRAME);
        }
        // 6. NVDA's last check before it queues the focus,
        // `shouldAllowUIAFocusEvent` (`UIAHandler/__init__.py` 948,
        // `NVDAObjects/UIA/__init__.py` 1632): the sender's keyboard focus,
        // read live with a plain property read (`CurrentHasKeyboardFocus`
        // on the event's element, client side, or on the focus's for a
        // menu opening redirected to it), a failed read letting the event
        // through. It is this plain read, here, and not one inside a remote
        // operation, because Windows Terminal's second identity of its
        // terminal answers the two differently, and only the plain read
        // drops it as NVDA does (`docs/design/focus-pipeline.md`, section
        // 16.8); the read does not make it again, since a focus that moves
        // on while it is read is followed by the focus event of where it
        // went (`docs/design/listener-without-connections.md`, section 3,
        // step 8).
        //
        // Out of time, the live read is not made, and the sender's keyboard
        // focus is taken from the event's cache instead, as it stood when
        // the event was raised: a sender that did not have it then is
        // dropped. A live read that fails, by UIA's transaction timeout or
        // otherwise, lets the focus through, as NVDA's `except COMError:
        // return True` does (Dickson, 2026-10-21). A menu opening redirected
        // to the current focus is judged by the focus's element, whose
        // keyboard focus no cache of this event holds, so it is let through.
        let lacked_by_its_event =
            || std::ptr::eq(subject, element) && sender.lacked_keyboard_focus();
        if out_of_time() {
            if lacked_by_its_event() {
                return Err(UNFOCUSED_OUT_OF_TIME);
            }
        } else {
            budget.call("the keyboard focus");
            if let Ok(false) = subject.has_keyboard_focus() {
                return Err(NOT_FOCUSED);
            }
        }
        Ok(window)
    }

    /// Step 2: whether the sender, `held`, whose cache reads as `sender`, is
    /// native UIA (`isNativeUIAElement`, `UIAHandler/__init__.py` 1515 to
    /// 1575): its nearest window, its own else the nearest ancestor's
    /// (`getNearestWindowHandle`, one call), and that window's class. A
    /// sender with no window anywhere above it, or whose walk to one fails,
    /// is not native, and NVDA's object for it could not be made without a
    /// window ("no windowHandle", `NVDAObjects/UIA/__init__.py` 1598 to
    /// 1606). Windows Terminal's second identity of its terminal can be
    /// such an element, its island not yet joined to its window (section
    /// 16.8). A sender whose nearest window's class is on NVDA's MSAA class
    /// list, and not on its UIA list or under a shell window, is skipped
    /// ([`MSAA_CLASS`]), by the class verdict every outpost's arbitrator
    /// gives ([`class_verdict`]): a function of the window's class names,
    /// local calls only, so the two processes agree on it, unless a backend
    /// override forcing UIA holds for the application, which then counts
    /// that focus. The rest of `isUIAWindow`, the probe and the post-probe
    /// checks, is the outpost's, whose verdict alone decides which
    /// backend's focus counts in each window
    /// (`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated
    /// fact", the verdicts): the listener reads every other focus that
    /// passes NVDA's checks, and the outpost drops a read in a window it
    /// judges not UIA. NVDA's two exceptions, a `NetUI` container in a Word
    /// document and Internet Explorer 9's downloads list, are Office's and
    /// Internet Explorer's, and are not ported. Answers the nearest window.
    ///
    /// The read's time, `budget`, is asked before the nearest window's
    /// lookup when `called` says a call was made before this check. The
    /// class verdict needs no call, so it is made whatever the time.
    fn native_window(
        &self,
        held: &Cached<ListenerEvent>,
        sender: &ListenerEvent,
        budget: &Budget,
        called: bool,
    ) -> Result<isize, &'static str> {
        let out_of_time = || budget.out_of_time();
        let element = held.live().element();
        let hwnd = sender.role.hwnd;
        let nearest = if hwnd == 0 {
            if called && out_of_time() {
                return Err(OUT_OF_TIME_BEFORE_LOOKUP);
            }
            budget.call("the nearest window");
            let started = Instant::now();
            let nearest = verbatim_uia::nearest_window_lookup(element);
            let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            tracing::debug!(
                elapsed_us,
                window = ?nearest,
                "UIA focus sender's nearest window looked up"
            );
            let nearest = nearest.map_err(|_| NEAREST_TIMED_OUT)?.ok_or(NO_WINDOW)?;
            self.window.learn(nearest);
            nearest
        } else {
            hwnd
        };
        if skipped_by_class(
            &WindowClasses::of(nearest),
            self.forced_uia.load(Ordering::Relaxed),
        ) {
            return Err(MSAA_CLASS);
        }
        Ok(nearest)
    }

    /// Logs, at debug, what section 16.3 of `docs/design/focus-pipeline.md`
    /// asks to see of each read before the duplicate rule is built on
    /// `CompareElements`: the sender's runtime id, class name, framework,
    /// and own native window handle, `hwnd`, and whether UI Automation
    /// counts it as the same element as the focus Core holds, when the
    /// listener holds that focus's element (`same_as_current`, the
    /// duplicate rule's comparison, and a menu opening's with the focus). All but
    /// the comparison come from the event's cache, as NVDA's debug strings
    /// do; nothing is read live for the log.
    fn log_sender(
        sender: &ListenerEvent,
        runtime_id: &[i32],
        same_as_current: Option<&windows::core::Result<bool>>,
    ) {
        // From the event's cache, as NVDA's debug strings read it.
        let class_name = &sender.role.class_name;
        let framework = &sender.framework_id;
        tracing::debug!(
            ?runtime_id,
            ?class_name,
            ?framework,
            hwnd = sender.role.hwnd,
            ?same_as_current,
            "UIA focus sender"
        );
    }
}

/// Logs, at debug, each of a focus's ancestor facts, nearest first, with
/// the read that produced them (`docs/design/focus-pipeline.md`, section
/// 16.4: where a console window element is lost).
fn log_ancestor_facts(path: &str, facts: &[UiaAncestorFact]) {
    for fact in facts {
        log_ancestor_fact(path, fact);
    }
}

/// Logs one ancestor fact at debug, with the read that produced it.
fn log_ancestor_fact(path: &str, fact: &UiaAncestorFact) {
    tracing::debug!(
        path,
        runtime_id = ?fact.snapshot.runtime_id,
        role = ?fact.snapshot.role,
        name = ?fact.snapshot.name,
        hwnd = fact.hwnd,
        content = fact.content,
        "UIA focus ancestor fact"
    );
}

/// The ancestors of the focus before that lie above the one the new
/// focus's ancestor read stopped at, `met`, an index into the read's known
/// runtime ids: the focus before itself, then its ancestors, nearest
/// first. The stop is the last ancestor read, so what is reused begins
/// just above it.
fn reused_ancestors(before: &[UiaAncestorFact], met: usize) -> Vec<UiaAncestorFact> {
    before.get(met..).map(<[_]>::to_vec).unwrap_or_default()
}

/// The fact made of an element's cached parts.
fn fact_of(parts: CachedUiaParts) -> UiaSnapshotFact {
    UiaSnapshotFact {
        runtime_id: parts.runtime_id,
        role: parts.role,
        name: parts.name,
        value: parts.value,
        states: parts.states,
        details: parts.details,
    }
}

/// The fact made of an ancestor the read returned with its cache filled.
fn ancestor_fact(element: &Cached<Ancestor>) -> UiaAncestorFact {
    let read = element.read();
    UiaAncestorFact {
        hwnd: read.node.role.hwnd,
        content: read.is_control_and_content(),
        snapshot: fact_of(read.parts(element.runtime_id())),
    }
}

/// What the listener knows of a focus event when its callback queues it:
/// the marker it sent, and the trace and times its read or drop carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Meta {
    /// The marker sent for the event.
    pub(crate) marker: u64,
    /// The event's kind, which its read or drop tells the outpost.
    pub(crate) kind: FocusEvent,
    /// The trace minted when the event arrived.
    pub(crate) trace: TraceId,
    /// When it arrived, its `observed_at_us` the observation by which its
    /// fact is ordered, and the latency log's stamps.
    pub(crate) timing: EventTiming,
}

/// The work [`Readers`] runs on its threads: the listener's reads of UIA
/// focus events, or a test's stand-in.
pub(crate) trait Reading: Send + Sync + 'static {
    /// What the callback queues for one event besides its [`Meta`].
    type Job: Send + 'static;
    /// What a thread keeps between its reads, made on the thread.
    type Reader;
    /// Makes a thread's reader for `pid`'s events, on that thread; `None`
    /// when it cannot be made.
    fn reader(&self, pid: Pid) -> Option<Self::Reader>;
    /// Judges and reads one event, which `meta` describes, in the read
    /// `watched`, which it tells the window it is of once the read finds it
    /// and asks whether the read was abandoned before accepting a focus.
    fn read(
        &self,
        reader: Option<&mut Self::Reader>,
        meta: Meta,
        job: Self::Job,
        watched: &WatchedRead,
    ) -> Judged;
    /// The window `job`'s event is of as far as the event says, for the
    /// watchdog's judgement of whether the user has moved on from it
    /// ([`WatchedRead`]): `Some(0)` for an event judged so whose window
    /// its read is still to learn, and `None`, the default, for an event
    /// that is never judged so.
    fn window(&self, _job: &Self::Job) -> Option<isize> {
        None
    }
    /// The watchdog has abandoned the read of the event `meta` describes:
    /// called under the readers' lock once the read is marked abandoned
    /// and before its drop is sent. Nothing, the default.
    fn abandoned(&self, _meta: &Meta) {}
    /// Whether the user has moved on from `window`, which a slow read is
    /// of, to a window of the same application on another UI thread
    /// ([`front_is_another_thread_of_its_application`], local calls only,
    /// the default).
    fn moved_on(&self, window: isize) -> bool {
        front_is_another_thread_of_its_application(window)
    }
    /// The kind of event `job` is, for the test of whether a focus is
    /// pending ([`Readers::focus_pending`]).
    fn kind(&self, job: &Self::Job) -> FocusEvent;
    /// The kind of event `job` is and the element it is of, by runtime id,
    /// for NVDA's UIA limiter rule: a newer event of one kind of one
    /// element replaces one still waiting ([`Readers::push`]). `None`, the
    /// default, for an event that is never replaced.
    fn key<'a>(&self, _job: &'a Self::Job) -> Option<(FocusEvent, &'a [i32])> {
        None
    }
    /// Releases a thread's reader as the thread ends, on that thread.
    fn finish(&self, reader: Option<Self::Reader>);
    /// Sends what became of an event of `pid`'s, placed, for its
    /// application's gap record, in the top-level window `placed` when its
    /// read found one ([`gap_answer`]). Called under the readers' lock, so
    /// what one application's events became is sent in the order they
    /// arrived.
    fn outcome(&self, pid: Pid, meta: Meta, judged: Judged, placed: Option<isize>);
    /// `window`'s top-level window, 0 for none: a local call
    /// ([`top_level_of`], the default), made outside the readers' lock.
    fn top_level(&self, window: isize) -> isize {
        top_level_of(window)
    }
}

/// One event waiting for its read.
struct Queued<J> {
    /// Its place in the order every application's events were queued in
    /// ([`ReadersState::queued`]).
    place: u64,
    /// How many foreground events the listener's hook had delivered when
    /// it was queued ([`ReadersState::foregrounds`]).
    foregrounds: u64,
    /// Whether the focus event the listener received before it was another
    /// application's ([`ReadersState::last_focus`]).
    after_elsewhere: bool,
    meta: Meta,
    job: J,
}

/// The event the thread in charge of an application is reading.
struct Running {
    /// When the thread began it.
    started: Instant,
    meta: Meta,
    /// Its kind ([`Reading::kind`]).
    kind: FocusEvent,
    /// The read as the watchdog watches it.
    watched: WatchedRead,
    /// How many foreground events the listener's hook had delivered when
    /// it was queued.
    foregrounds: u64,
}

/// One application's reading.
struct App<J> {
    /// Its events waiting for a read, oldest first.
    queue: VecDeque<Queued<J>>,
    /// The thread in charge of its events, by generation, if one is.
    live: Option<u64>,
    /// The event the thread in charge is reading, if one is.
    running: Option<Running>,
    /// The place of the event being read in the order events were queued,
    /// while one is.
    running_place: u64,
    /// Its threads abandoned to reads that have not returned yet.
    abandoned: usize,
    /// When its abandoned threads reached [`ABANDONED_LIMIT`], while they
    /// are at it: its events wait for a thread until [`READ_DEADLINE`]
    /// after this, and are dropped from then on.
    limit_reached: Option<Instant>,
    /// Whether its outpost has ended, so the thread in charge ends once no
    /// event is waiting.
    ending: bool,
    /// Wakes the thread in charge when an event is queued.
    wake: Arc<Condvar>,
}

impl<J> App<J> {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            live: None,
            running: None,
            running_place: 0,
            abandoned: 0,
            limit_reached: None,
            ending: false,
            wake: Arc::new(Condvar::new()),
        }
    }
}

struct ReadersState<J> {
    apps: HashMap<Pid, App<J>>,
    next_generation: u64,
    threads: Vec<JoinHandle<()>>,
    stopping: bool,
    /// How many events have been queued, of every application: an event's
    /// place in that order is the count once it is queued.
    queued: u64,
    /// What runs once every event queued before it has its outcome sent,
    /// each with the count of events queued when it was added
    /// ([`Readers::when_idle`]), oldest first.
    idle_waiters: VecDeque<(u64, IdleWaiter)>,
    /// How many foreground events the listener's hook has delivered
    /// ([`Readers::foreground_changed`]).
    foregrounds: u64,
    /// Whether the watchdog waits with a read that a foreground event
    /// would have it judge, or check when its grace ends, so that the
    /// event wakes it ([`Readers::foreground_changed`]).
    wake_on_foreground: bool,
    /// The application of the last focus event or menu opening queued, for
    /// whether the next one's top-level window can be that of the focus
    /// before it.
    last_focus: Option<Pid>,
    /// When the watchdog's alarm is set to go off, while it is set
    /// ([`alarm_by`]).
    alarm_due: Option<Instant>,
}

/// What [`Readers::when_idle`] runs.
pub(crate) type IdleWaiter = Box<dyn FnOnce() + Send>;

impl<J> ReadersState<J> {
    /// Runs each waiter for which every event queued before it has its
    /// outcome sent, no application having one of those waiting or being
    /// read. Events queued after a waiter are not waited for, so a stream
    /// of them cannot hold it back. Called under the readers' lock, as
    /// outcomes are sent.
    fn wake_idle_waiters(&mut self) {
        if self.idle_waiters.is_empty() {
            return;
        }
        // An application's oldest event in progress is the one being read,
        // or, with none, the first waiting: each queue keeps the order its
        // events were queued in.
        let oldest = self
            .apps
            .values()
            .filter_map(|app| {
                // A read published at its bound has its outcome sent.
                if app
                    .running
                    .as_ref()
                    .is_some_and(|running| !running.watched.published())
                {
                    Some(app.running_place)
                } else {
                    app.queue.front().map(|queued| queued.place)
                }
            })
            .min();
        while let Some((before, _)) = self.idle_waiters.front()
            && oldest.is_none_or(|oldest| *before < oldest)
        {
            if let Some((_, waiter)) = self.idle_waiters.pop_front() {
                waiter();
            }
        }
    }
}

struct Shared<R: Reading> {
    reading: R,
    deadline: Duration,
    /// How long a read is left alone before the user's moving on abandons
    /// it ([`MOVED_ON_GRACE`](crate::outpost::window::MOVED_ON_GRACE)).
    grace: Duration,
    state: Mutex<ReadersState<R::Job>>,
    /// The watchdog's alarm: its timer set, under the readers' lock, to the
    /// soonest time a read may need judging, which wakes no thread, and
    /// cancelled once no read is under way; its event set when the
    /// watchdog must look now (`docs/design/wake-ups.md`, item 3).
    alarm: Alarm,
    /// The failed cached reads the reading threads have made
    /// (`verbatim_uia::calls`).
    failed_reads: AtomicU32,
}

impl<R: Reading> Shared<R> {
    fn lock(&self) -> MutexGuard<'_, ReadersState<R::Job>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The reading threads, one per application, and their watchdog.
pub(crate) struct Readers<R: Reading> {
    shared: Arc<Shared<R>>,
    watchdog: Mutex<Option<JoinHandle<()>>>,
}

impl<R: Reading> Readers<R> {
    /// Readers doing `reading`, each read abandoned past `deadline`, or
    /// past `grace` once the user has moved on from its window
    /// ([`Reading::moved_on`]).
    ///
    /// # Panics
    ///
    /// Panics if the watchdog's thread cannot be spawned, which means the
    /// process is out of OS thread resources.
    pub(crate) fn new(reading: R, deadline: Duration, grace: Duration) -> Self {
        let shared = Arc::new(Shared {
            reading,
            deadline,
            grace,
            state: Mutex::new(ReadersState {
                apps: HashMap::new(),
                next_generation: 0,
                threads: Vec::new(),
                stopping: false,
                queued: 0,
                idle_waiters: VecDeque::new(),
                foregrounds: 0,
                wake_on_foreground: false,
                last_focus: None,
                alarm_due: None,
            }),
            alarm: Alarm::default(),
            failed_reads: AtomicU32::new(0),
        });
        let watched = Arc::clone(&shared);
        let watchdog = thread::Builder::new()
            .name("verbatim-listener-read-watchdog".to_owned())
            .spawn(move || watchdog(&watched))
            .expect("spawn the reading threads' watchdog");
        Self {
            shared,
            watchdog: Mutex::new(Some(watchdog)),
        }
    }

    /// Queues `job`, a focus event of `pid`'s whose marker `meta` names,
    /// for `pid`'s reading thread, starting one if it has none, unless
    /// [`ABANDONED_LIMIT`] of its threads are abandoned and still in their
    /// calls: the event then waits for the first of them to return, or for
    /// the watchdog to drop it at the application's bound, and once that
    /// bound has passed it is dropped at once, behind every event still
    /// waiting ([`READ_DEADLINE`]). Never waits on a read.
    pub(crate) fn push(&self, pid: Pid, meta: Meta, job: R::Job) {
        let shared = &self.shared;
        let mut state = shared.lock();
        if state.stopping {
            shared.reading.outcome(
                pid,
                meta,
                Judged::Dropped("the listener is shutting down"),
                None,
            );
            return;
        }
        let app = state.apps.entry(pid).or_insert_with(App::new);
        // An event after its outpost ended: the application runs on, and
        // its thread with it.
        app.ending = false;
        // NVDA's UIA limiter keeps one waiting focus event per element: a
        // newer one erases the older and takes the back of the queue
        // (`rateLimitedEventHandler.cpp`), so an element that raises its
        // focus twice before the first is handled is read once. The one
        // replaced is reported dropped, which resolves its marker.
        if let Some(key) = shared.reading.key(&job) {
            let mut kept = VecDeque::with_capacity(app.queue.len());
            for waiting in app.queue.drain(..) {
                if shared.reading.key(&waiting.job) == Some(key) {
                    // Its drop leaves its application's gap record open
                    // ([`gap_answer`]).
                    shared
                        .reading
                        .outcome(pid, waiting.meta, Judged::Dropped(REPLACED), None);
                } else {
                    kept.push_back(waiting);
                }
            }
            app.queue = kept;
        }
        state.queued += 1;
        let after_elsewhere = match shared.reading.kind(&job) {
            FocusEvent::Focus | FocusEvent::MenuOpened => {
                state.last_focus.replace(pid) != Some(pid)
            }
            FocusEvent::Selection | FocusEvent::Notification => false,
        };
        let queued = Queued {
            place: state.queued,
            foregrounds: state.foregrounds,
            after_elsewhere,
            meta,
            job,
        };
        let Some(app) = state.apps.get_mut(&pid) else {
            return;
        };
        app.queue.push_back(queued);
        if app.live.is_some() {
            app.wake.notify_one();
        } else if app
            .limit_reached
            .is_some_and(|reached| Instant::now() >= reached + shared.deadline)
        {
            // Past the application's bound: dropped at once, behind the
            // events the watchdog has not yet dropped, in order.
            drop_waiting(shared, &mut state, pid, Dropping::AsPushed);
        } else if app.abandoned >= ABANDONED_LIMIT {
            tracing::warn!(
                %pid,
                waiting = app.queue.len(),
                "a UIA focus waits: its application's reads have not returned"
            );
            // The watchdog bounds its wait.
            shared.alarm.wake();
        } else {
            start(shared, &mut state, pid);
        }
    }

    /// Whether a focus event is waiting for its read or being read, for a
    /// menu opening of `pid`'s: NVDA drops a menu opening that arrives
    /// while a focus is pending (`isPendingEvents("gainFocus")`,
    /// `UIAHandler/__init__.py` 772 to 777), and here a focus still to be
    /// judged stands for one NVDA has queued and not yet run. A focus of an
    /// application whose abandoned threads are at [`ABANDONED_LIMIT`],
    /// waiting with no thread to read it, counts for that application's own
    /// menu openings only, so a hung application cannot have every other
    /// application's menu openings dropped while it waits.
    pub(crate) fn focus_pending(&self, pid: Option<Pid>) -> bool {
        let shared = &self.shared;
        let state = shared.lock();
        state.apps.iter().any(|(owner, app)| {
            app.running.as_ref().is_some_and(|running| {
                running.kind == FocusEvent::Focus && !running.watched.published()
            }) || (app.live.is_some() || Some(*owner) == pid)
                && app
                    .queue
                    .iter()
                    .any(|queued| shared.reading.kind(&queued.job) == FocusEvent::Focus)
        })
    }

    /// Ends `pid`'s reading thread, the listener having learned that its
    /// outpost has ended: the thread reads the events already waiting, then
    /// releases its reader and ends, unless another event comes first.
    /// Never waits on a read.
    pub(crate) fn outpost_ended(&self, pid: Pid) {
        let mut state = self.shared.lock();
        let Some(app) = state.apps.get_mut(&pid) else {
            return;
        };
        if app.live.is_none() && app.abandoned == 0 && app.queue.is_empty() {
            state.apps.remove(&pid);
            return;
        }
        app.ending = true;
        app.wake.notify_one();
    }

    /// Runs `waiter` once no event queued before this call is waiting for
    /// its read or being read, at once if none is, under the readers'
    /// lock, behind the outcome of every one of them: the idle barrier's
    /// answer goes behind them (`docs/design/communication.md`, section
    /// 6.7). Events queued after this call are not waited for. Never waits
    /// on a read. Once the readers are stopping, `waiter` is dropped unrun.
    pub(crate) fn when_idle(&self, waiter: IdleWaiter) {
        let mut state = self.shared.lock();
        if state.stopping {
            return;
        }
        let queued = state.queued;
        state.idle_waiters.push_back((queued, waiter));
        state.wake_idle_waiters();
    }

    /// A foreground event reached the listener's hook: counted, so the
    /// watchdog knows which reads' events it came after, and, while a read
    /// is under way that the watchdog has not yet looked at or set to be
    /// checked when its grace ends, or one past its grace, wakes the
    /// watchdog to judge
    /// again whether the user has moved on from it, or to check it when its
    /// grace ends. Otherwise it wakes no thread. Never waits on a read.
    pub(crate) fn foreground_changed(&self) {
        let mut state = self.shared.lock();
        state.foregrounds += 1;
        if state.wake_on_foreground && state.apps.values().any(|app| app.running.is_some()) {
            self.shared.alarm.wake();
        }
    }

    /// The work the reading threads do.
    pub(crate) fn reading(&self) -> &R {
        &self.shared.reading
    }

    /// The failed cached reads the reading threads have made so far.
    pub(crate) fn failed_reads(&self) -> u32 {
        self.shared.failed_reads.load(Ordering::Relaxed)
    }

    /// Stops every reading thread for the listener's shutdown: nothing is
    /// queued from now on, what is waiting is not read, and every thread is
    /// waited for, each abandoned one returning once UIA gives up on its
    /// call. Returns how many threads were waited for.
    pub(crate) fn stop(&self) -> usize {
        let threads = {
            let mut state = self.shared.lock();
            state.stopping = true;
            for app in state.apps.values() {
                app.wake.notify_all();
            }
            std::mem::take(&mut state.threads)
        };
        self.shared.alarm.wake();
        let watchdog = self
            .watchdog
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(watchdog) = watchdog {
            let _ = watchdog.join();
        }
        let count = threads.len();
        for thread in threads {
            let _ = thread.join();
        }
        count
    }
}

/// Starts a reading thread in charge of `pid`'s events; with no thread to
/// be had, its waiting events are dropped.
fn start<R: Reading>(shared: &Arc<Shared<R>>, state: &mut ReadersState<R::Job>, pid: Pid) {
    state.next_generation += 1;
    let generation = state.next_generation;
    let Some(app) = state.apps.get_mut(&pid) else {
        return;
    };
    app.live = Some(generation);
    let reading = Arc::clone(shared);
    let spawned = thread::Builder::new()
        .name("verbatim-listener-read".to_owned())
        .spawn(move || run(&reading, pid, generation));
    match spawned {
        Ok(thread) => {
            state.threads.retain(|thread| !thread.is_finished());
            state.threads.push(thread);
        }
        Err(error) => {
            tracing::error!(%pid, %error, "no thread to read UIA focus events; they are dropped");
            app.live = None;
            let waiting: Vec<Queued<R::Job>> = app.queue.drain(..).collect();
            for queued in waiting {
                send_outcome(
                    shared,
                    (pid, queued.meta),
                    Judged::Dropped("no thread to read it"),
                    None,
                );
            }
            state.wake_idle_waiters();
        }
    }
}

/// A reading thread: reads `pid`'s events in order while it is the thread
/// in charge of them, then releases its reader and ends.
fn run<R: Reading>(shared: &Arc<Shared<R>>, pid: Pid, generation: u64) {
    let mut generation = generation;
    let mut reader = shared.reading.reader(pid);
    while let Some((mut meta, job, window)) = next_job(shared, pid, generation) {
        // A read whose window the event's cache gives has the window in
        // front judged against it now, on this thread, as one that learns
        // its window later does.
        window.known();
        let judged = shared.reading.read(reader.as_mut(), meta, job, &window);
        // The top-level window of the window the read found, which places
        // a focus's drop for its application's gap record, kept in its
        // outpost: found by the read, or a local call, made before the
        // readers' lock is taken.
        let placed = (meta.kind == FocusEvent::Focus && window.window() != 0)
            .then(|| match window.top_level() {
                0 => shared.reading.top_level(window.window()),
                top => top,
            })
            .filter(|&top| top != 0);
        // The reading thread's end, for the latency ledger's split of the
        // listener's handling from its queue and its write.
        meta.timing.listener_handled_at_us = now_us();
        shared
            .failed_reads
            .fetch_add(verbatim_uia::calls::take_failed_reads(), Ordering::Relaxed);
        let mut guard = shared.lock();
        let state = &mut *guard;
        let Some(app) = state.apps.get_mut(&pid) else {
            break;
        };
        if app.live == Some(generation) {
            app.running = None;
            if window.published() {
                // The watchdog published the read at its bound: its late
                // result is discarded, and the thread goes on to the
                // application's next event.
                tracing::info!(
                    %pid,
                    "a UIA focus read published at its bound returned; its result is discarded"
                );
            } else {
                send_outcome(shared, (pid, meta), judged, placed);
            }
            settle_alarm(shared, state);
            state.wake_idle_waiters();
            continue;
        }
        // Abandoned while it read: the watchdog said the event was dropped,
        // and another thread took the application's later events. With no
        // thread in charge and events waiting, as when the application's
        // abandoned threads reached the limit, this one takes them. Below
        // the limit, the application's bound ends.
        app.abandoned = app.abandoned.saturating_sub(1);
        if app.abandoned < ABANDONED_LIMIT {
            app.limit_reached = None;
        }
        tracing::info!(%pid, "an abandoned UIA focus read returned; its result is discarded");
        if app.live.is_some() || app.queue.is_empty() || state.stopping {
            break;
        }
        state.next_generation += 1;
        generation = state.next_generation;
        app.live = Some(generation);
    }
    shared.reading.finish(reader);
}

/// The next event for the thread `generation` to read for `pid`, waiting
/// for one; `None` when the thread is no longer in charge, the readers are
/// stopping, or the application's outpost has ended and no event is
/// waiting ([`Readers::outpost_ended`]).
fn next_job<R: Reading>(
    shared: &Arc<Shared<R>>,
    pid: Pid,
    generation: u64,
) -> Option<(Meta, R::Job, WatchedRead)> {
    let mut state = shared.lock();
    loop {
        if state.stopping {
            return None;
        }
        let app = state.apps.get_mut(&pid)?;
        if app.live != Some(generation) {
            return None;
        }
        if let Some(Queued {
            place,
            foregrounds,
            after_elsewhere,
            meta,
            job,
        }) = app.queue.pop_front()
        {
            let kind = shared.reading.kind(&job);
            // Only a judged read has its window judged, as it starts or once
            // it learns it, so only it is given what to do then, and only
            // it can reach its bound: a selection or a notification is
            // never judged, so it is given nothing.
            let window = shared.reading.window(&job);
            let watched = WatchedRead::new(
                window.unwrap_or(0),
                if window.is_some() {
                    (Some(on_learn(shared)), Some(on_arm(shared)))
                } else {
                    (None, None)
                },
                after_elsewhere,
            );
            let started = Instant::now();
            app.running = Some(Running {
                started,
                meta,
                kind,
                watched: watched.clone(),
                foregrounds,
            });
            app.running_place = place;
            // The read is checked at its deadline, or at the end of its
            // grace if a foreground event came while it waited, unless
            // the alarm goes off sooner; and until the watchdog has looked
            // at it, a foreground event wakes the watchdog. No thread is
            // woken.
            let check = if foregrounds == state.foregrounds {
                started + shared.deadline
            } else {
                started + shared.grace
            };
            state.wake_on_foreground = true;
            alarm_by(shared, &mut state, check);
            return Some((meta, job, watched));
        }
        if app.ending {
            app.live = None;
            if app.abandoned == 0 {
                state.apps.remove(&pid);
            }
            return None;
        }
        let wake = Arc::clone(&app.wake);
        state = wake.wait(state).unwrap_or_else(PoisonError::into_inner);
    }
}

/// What a read does once it learns its window ([`WatchedRead::learn`]), or
/// as it starts with its window known ([`WatchedRead::known`]):
/// judges whether the window in front is already another thread of its
/// application, with the local calls the watchdog's judgement makes
/// ([`Reading::moved_on`]), and only if it is, wakes the watchdog to check
/// the read when its grace ends. A read whose window is the front's own
/// thread costs no wake-up. Never waits on a read.
fn on_learn<R: Reading>(shared: &Arc<Shared<R>>) -> OnLearn {
    let shared = Arc::downgrade(shared);
    Box::new(move |read: &WatchedRead| {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        if !read.claimed() && read.judge_front(|window| shared.reading.moved_on(window)) {
            let _state = shared.lock();
            shared.alarm.wake();
        }
    })
}

/// What a read does once it has passed NVDA's checks and its bound is
/// known ([`WatchedRead::arm`]): has the watchdog's alarm go off by the
/// bound, a system call that wakes no thread, so the watchdog publishes the
/// read at its bound should a call of its walk still be under way then.
fn on_arm<R: Reading>(shared: &Arc<Shared<R>>) -> OnArm {
    let shared = Arc::downgrade(shared);
    Box::new(move |bound_at: Instant| {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let mut state = shared.lock();
        alarm_by(&shared, &mut state, bound_at);
    })
}

/// Has the watchdog's alarm go off by `at`, setting its timer only when it
/// is not already set to go off sooner. Called under the readers' lock.
fn alarm_by<R: Reading>(shared: &Shared<R>, state: &mut ReadersState<R::Job>, at: Instant) {
    if state.alarm_due.is_none_or(|due| at < due) {
        shared.alarm.set(at);
        state.alarm_due = Some(at);
    }
}

/// Cancels the watchdog's alarm once no read is under way and no
/// application's events wait at the abandoned limit, so a read that
/// returned in time costs the watchdog no wake-up. Called under the
/// readers' lock.
fn settle_alarm<R: Reading>(shared: &Shared<R>, state: &mut ReadersState<R::Job>) {
    if state.alarm_due.is_some()
        && state
            .apps
            .values()
            .all(|app| app.running.is_none() && app.limit_reached.is_none())
    {
        shared.alarm.cancel();
        state.alarm_due = None;
    }
}

/// Why the watchdog abandons a read at `now`, if it does: the read, of
/// `window` (0 for none known), has passed its `deadline`, or has passed
/// the end of its grace, `grace_end`, and `moved_on` says the user has
/// left `window` for another thread of its application.
fn abandon_reason(
    now: Instant,
    (grace_end, deadline): (Instant, Instant),
    window: isize,
    moved_on: impl FnOnce(isize) -> bool,
) -> Option<&'static str> {
    if now >= deadline {
        Some(NOT_IN_TIME)
    } else if window != 0 && now >= grace_end && moved_on(window) {
        Some(MOVED_ON)
    } else {
        None
    }
}

/// The watchdog: abandons a thread whose read has run past the deadline,
/// or past its grace once the user has moved on from its window, reports
/// its event dropped, and gives the application's later events to a new
/// thread, unless too many of its threads are abandoned already; publishes
/// at its bound a read that has passed NVDA's checks and whose walk is
/// still in a call then ([`publish_at_bound`]); and drops an event that has
/// waited the deadline since it was queued while its application had no
/// thread to read it.
///
/// Whether the user has moved on is judged once the read's window is
/// known, and at the end of its grace only once a foreground event has
/// come since the read's event was queued, or the window in front was
/// already another thread's when the read's window became known
/// ([`WatchedRead::judge_front`]): the evidence that the user may have
/// left its window. The watchdog sleeps on its alarm ([`Shared::alarm`]),
/// whose timer a read sets as it starts, to its deadline, or to the end of
/// its grace with that evidence, and again to its bound once it has passed
/// NVDA's checks ([`on_arm`]), each only when the timer is not set sooner,
/// and which a read that returns with no other under way cancels
/// ([`settle_alarm`]): so a read that returns before its bound costs the
/// watchdog no wake-up. It wakes when the timer goes off; when a read
/// learns its window, or starts with it known, and the window in front is
/// another thread's ([`on_learn`]); while an event waits with no thread,
/// at the end of its wait; and, while a read is under way that is past its
/// grace, or that it has not yet looked at or set to be checked when its
/// grace ends, on each foreground event ([`Readers::foreground_changed`]).
fn watchdog<R: Reading>(shared: &Arc<Shared<R>>) {
    loop {
        let mut state = shared.lock();
        if state.stopping {
            return;
        }
        let now = Instant::now();
        let foregrounds = state.foregrounds;
        let mut due: Option<Instant> = None;
        let mut wake_on_foreground = false;
        let mut abandoned = Vec::new();
        let mut published = Vec::new();
        let mut waited = Vec::new();
        for (pid, app) in &state.apps {
            let Some(running) = &app.running else {
                // The events waiting with no thread to read them, as the
                // application's abandoned threads are at the limit, are
                // dropped at its bound.
                if app.live.is_none()
                    && !app.queue.is_empty()
                    && let Some(reached) = app.limit_reached
                {
                    let end = reached + shared.deadline;
                    if now >= end {
                        waited.push(*pid);
                    } else {
                        due = Some(due.map_or(end, |due| due.min(end)));
                    }
                }
                continue;
            };
            let grace_end = running.started + shared.grace;
            let deadline = running.started + shared.deadline;
            let front_elsewhere = running
                .watched
                .judge_front(|window| shared.reading.moved_on(window));
            let armed = running.foregrounds != foregrounds || front_elsewhere;
            let window = running.watched.window();
            let reason = abandon_reason(now, (grace_end, deadline), window, |window| {
                armed && shared.reading.moved_on(window)
            });
            if let Some(reason) = reason {
                abandoned.push((*pid, window, reason));
                continue;
            }
            // A read past NVDA's checks whose walk is still under way at its
            // bound is published from its progress.
            let bound = running.watched.bound_at();
            if bound.is_some_and(|bound| now >= bound) {
                published.push(*pid);
                continue;
            }
            let next = if now < grace_end && armed {
                grace_end
            } else {
                wake_on_foreground = true;
                deadline
            };
            let next = bound.map_or(next, |bound| bound.min(next));
            due = Some(due.map_or(next, |due| due.min(next)));
        }
        if !abandoned.is_empty() || !published.is_empty() || !waited.is_empty() {
            for (pid, window, reason) in abandoned {
                abandon(shared, &mut state, pid, window, reason);
            }
            for pid in published {
                publish_at_bound(shared, &mut state, pid);
            }
            for pid in waited {
                drop_waiting(shared, &mut state, pid, Dropping::Waited);
            }
            continue;
        }
        state.wake_on_foreground = wake_on_foreground;
        match due {
            Some(due) => shared.alarm.set(due),
            None => shared.alarm.cancel(),
        }
        state.alarm_due = due;
        // The alarm keeps a time set, or an event, from now until the wait
        // takes it, so nothing set once the lock is let go is lost.
        drop(state);
        shared.alarm.wait();
    }
}

/// Publishes at its bound the read of `pid`'s event, which has passed
/// NVDA's checks and whose walk is still in a call, from what the read has
/// shared ([`WatchedRead::publish`]), completed by plain windows from its
/// outermost ancestor's nearest window: the read is claimed, so its reading
/// thread's late result is discarded when it returns, but the thread is not
/// abandoned for this, and goes on to its application's next event when its
/// call returns; it is abandoned only at the deadline, or once the user has
/// moved on, as any read is, and then counts against [`ABANDONED_LIMIT`]
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback").
fn publish_at_bound<R: Reading>(
    shared: &Arc<Shared<R>>,
    state: &mut ReadersState<R::Job>,
    pid: Pid,
) {
    let Some(running) = state.apps.get(&pid).and_then(|app| app.running.as_ref()) else {
        return;
    };
    let Some(read) = running.watched.publish() else {
        return;
    };
    let (meta, top) = (running.meta, running.watched.top_level());
    tracing::info!(
        %pid,
        ancestors = read.ancestors.len(),
        "a UIA focus read's call was still under way at its bound; the focus is published from what it read"
    );
    send_outcome(
        shared,
        (pid, meta),
        Judged::Read(Box::new(read)),
        Some(top).filter(|&top| top != 0),
    );
    state.wake_idle_waiters();
}

/// Who drops the waiting events ([`drop_waiting`]), for its log line.
#[derive(Clone, Copy)]
enum Dropping {
    /// The watchdog, for events that waited past the bound.
    Waited,
    /// [`Readers::push`], for the event it pushed, which did not wait, and
    /// any older one the watchdog has not yet dropped.
    AsPushed,
}

/// Drops every event of `pid`'s waiting with no thread to read it, its
/// application's abandoned threads at [`ABANDONED_LIMIT`] past its bound
/// ([`READ_DEADLINE`]), in the order they arrived: each drop resolves its
/// marker, so the application's later events in its outpost are not held
/// behind them.
fn drop_waiting<R: Reading>(
    shared: &Arc<Shared<R>>,
    state: &mut ReadersState<R::Job>,
    pid: Pid,
    dropping: Dropping,
) {
    let Some(app) = state.apps.get_mut(&pid) else {
        return;
    };
    if app.live.is_some() || app.queue.is_empty() {
        return;
    }
    let (abandoned, dropped) = (app.abandoned, app.queue.len());
    match dropping {
        Dropping::Waited => tracing::warn!(
            %pid,
            abandoned,
            dropped,
            "every waiting UIA event of the application, of whatever kind, waited past its bound behind its abandoned reads; they are dropped"
        ),
        Dropping::AsPushed => tracing::warn!(
            %pid,
            abandoned,
            dropped,
            "every waiting UIA event of the application, of whatever kind, is dropped as one arrives: it is past its bound behind its abandoned reads"
        ),
    }
    let waiting: Vec<Queued<R::Job>> = app.queue.drain(..).collect();
    for queued in waiting {
        send_outcome(
            shared,
            (pid, queued.meta),
            Judged::Dropped(WAITED_TOO_LONG),
            None,
        );
    }
    state.wake_idle_waiters();
}

/// Abandons the thread reading `pid`'s event, of `window`, for `reason`:
/// it has run past the deadline, or the user has moved on from its window.
/// The read is marked abandoned before anything else, so that it accepts
/// no focus once a newer read can begin ([`HeldElements::accept`]), and its
/// drop is sent unless it was published at its bound. The
/// application's waiting events go to a new thread, or, once
/// [`ABANDONED_LIMIT`] of its threads are abandoned, wait for the first of
/// them to return.
fn abandon<R: Reading>(
    shared: &Arc<Shared<R>>,
    state: &mut ReadersState<R::Job>,
    pid: Pid,
    window: isize,
    reason: &'static str,
) {
    let Some(app) = state.apps.get_mut(&pid) else {
        return;
    };
    let Some(running) = app.running.take() else {
        return;
    };
    // A read the watchdog published at its bound has had its outcome sent,
    // and Core judges that one.
    let unsent = running.watched.abandon();
    if unsent {
        shared.reading.abandoned(&running.meta);
    }
    app.live = None;
    app.abandoned += 1;
    let abandoned = app.abandoned;
    if abandoned >= ABANDONED_LIMIT && app.limit_reached.is_none() {
        app.limit_reached = Some(Instant::now());
    }
    if reason == MOVED_ON {
        tracing::warn!(
            %pid,
            abandoned,
            window,
            "a UIA focus read was slow and the user moved on to another thread of its application; its thread is abandoned"
        );
    } else {
        tracing::warn!(%pid, abandoned, "a UIA focus read did not return in time; its thread is abandoned");
    }
    let waiting = !app.queue.is_empty();
    if unsent {
        send_outcome(shared, (pid, running.meta), Judged::Dropped(reason), None);
    }
    let Some(app) = state.apps.get_mut(&pid) else {
        return;
    };
    if waiting {
        if abandoned >= ABANDONED_LIMIT {
            tracing::warn!(
                %pid,
                waiting = app.queue.len(),
                "UIA focuses wait: its application's reads have not returned"
            );
        } else {
            start(shared, state, pid);
        }
    }
    state.wake_idle_waiters();
}

/// Sends the outcome `judged` of `pid`'s event `meta`, under the readers'
/// lock: `placed` is the top-level window the read found, when it found
/// one, which places a drop's answer to the application's gap record
/// ([`gap_answer`]).
fn send_outcome<R: Reading>(
    shared: &Shared<R>,
    (pid, meta): (Pid, Meta),
    judged: Judged,
    placed: Option<isize>,
) {
    shared.reading.outcome(pid, meta, judged, placed);
}

/// Sets up each reading thread's [`FocusReader`] as it is made
/// ([`ReadingThreads::new`]).
pub type SetUpReader = Box<dyn Fn(&mut FocusReader) + Send + Sync>;

/// The listener's reading threads and their watchdog, run in the calling
/// process on the UIA focus events it hands them, for a test that watches
/// what the watchdog does with a real application's read: each event is
/// judged and read by a [`FocusReader`] on a thread for its application,
/// as the listener's are, and what became of it is sent, with the event's
/// marker, to the receiver [`new`](Self::new) returns. The deadline and the
/// grace are the listener's. Stopped, every thread waited for, when
/// dropped.
pub struct ReadingThreads {
    readers: Readers<ThreadReading>,
    next_marker: AtomicU64,
}

/// The reading threads' work in [`ReadingThreads`].
struct ThreadReading {
    held_elements: HeldElements,
    set_up: SetUpReader,
    outcomes: Mutex<std::sync::mpsc::Sender<(u64, Judged)>>,
}

impl Reading for ThreadReading {
    /// The event's element and its own window handle from the event's
    /// cache.
    type Job = (verbatim_uia::Agile<ListenerEvent>, isize);
    type Reader = FocusReader;

    fn reader(&self, _: Pid) -> Option<FocusReader> {
        let mut reader = FocusReader::new(self.held_elements.clone()).ok()?;
        (self.set_up)(&mut reader);
        Some(reader)
    }

    fn read(
        &self,
        reader: Option<&mut FocusReader>,
        meta: Meta,
        (element, _): Self::Job,
        watched: &WatchedRead,
    ) -> Judged {
        let Some(reader) = reader else {
            return Judged::Dropped("its reading thread has no UIA client");
        };
        match element.resolve() {
            Ok(sender) => reader.judge_watched(
                &sender,
                (FocusEvent::Focus, None),
                (
                    Arrival {
                        trace: meta.trace,
                        observed_at_us: meta.timing.observed_at_us,
                    },
                    watched.clone(),
                ),
            ),
            Err(_) => Judged::Dropped("its element could not be resolved"),
        }
    }

    fn window(&self, (_, hwnd): &Self::Job) -> Option<isize> {
        Some(*hwnd)
    }

    fn abandoned(&self, meta: &Meta) {
        self.held_elements.abandoned(meta.trace);
    }

    fn kind(&self, _: &Self::Job) -> FocusEvent {
        FocusEvent::Focus
    }

    fn finish(&self, reader: Option<FocusReader>) {
        let joined = reader.is_some();
        drop(reader);
        verbatim_uia::release_thread_state();
        if joined {
            verbatim_uia::leave_mta();
        }
    }

    fn outcome(&self, _: Pid, meta: Meta, judged: Judged, _: Option<isize>) {
        // The test may have finished with the outcomes.
        let _ = self
            .outcomes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send((meta.marker, judged));
    }
}

impl ReadingThreads {
    /// Reading threads whose readers `set_up` sets up as each is made, as a
    /// test sets up its own reader's clocks and foreground window, and the
    /// receiver of what became of each event, with its marker.
    #[must_use]
    pub fn new(set_up: SetUpReader) -> (Self, std::sync::mpsc::Receiver<(u64, Judged)>) {
        let (outcomes, received) = std::sync::mpsc::channel();
        let readers = Readers::new(
            ThreadReading {
                held_elements: HeldElements::default(),
                set_up,
                outcomes: Mutex::new(outcomes),
            },
            READ_DEADLINE,
            MOVED_ON_GRACE,
        );
        (
            Self {
                readers,
                next_marker: AtomicU64::new(1),
            },
            received,
        )
    }

    /// Hands the reading thread of `pid`'s events the UIA focus event whose
    /// element is `sender`, observed at `observed_at_us` on the run's
    /// shared clock, and answers its marker.
    ///
    /// # Errors
    ///
    /// The COM error when the element cannot be kept for its read.
    pub fn push(
        &self,
        pid: Pid,
        sender: &Cached<ListenerEvent>,
        observed_at_us: u64,
    ) -> windows::core::Result<u64> {
        let marker = self.next_marker.fetch_add(1, Ordering::Relaxed);
        let meta = Meta {
            marker,
            kind: FocusEvent::Focus,
            trace: TraceId::mint(),
            timing: crate::protocol::EventTiming {
                observed_at_us,
                ..crate::protocol::EventTiming::default()
            },
        };
        self.readers
            .push(pid, meta, (sender.agile()?, sender.read().role.hwnd));
        Ok(marker)
    }
}

impl Drop for ReadingThreads {
    fn drop(&mut self) {
        self.readers.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use crate::outpost::window::MOVED_ON_GRACE;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread::ThreadId;

    use super::*;

    /// The window every stand-in event's sender is, unless a test says
    /// its senders are not windows.
    const WINDOW: isize = 0x5150;

    /// The nearest window a stand-in read finds, when a test has it find
    /// one.
    const NEAREST: isize = 0x7170;

    /// A stand-in reading: each job is held until its gate opens, if it
    /// has one, and every outcome is sent to the test with the thread that
    /// read the event and the reason, as is each thread that ends, and each
    /// read as it returns. Every event's sender is window `hwnd`, 0 for a
    /// sender that is not a window; each read finds the sender's nearest
    /// window to be `nearest` before it is held, 0 for none. Whether the
    /// user has moved on from a window is what `moved_on` says, each
    /// judgement sent to the test. A read the watchdog abandons is told to
    /// `held_elements`, as the listener's reading tells its own. Each read
    /// is dropped as "read", unless `reads` says it is read; each fact sent
    /// is sent to the test; and a window is its own top-level window.
    struct Script {
        outcomes: Mutex<Sender<(Pid, u64, &'static str)>>,
        read_on: Mutex<Sender<(u64, ThreadId)>>,
        returned: Mutex<Sender<(u64, WatchedRead)>>,
        finished: Mutex<Sender<ThreadId>>,
        moved_on: AtomicBool,
        /// Whether its events are notifications rather than focuses.
        notifications: AtomicBool,
        asked: Mutex<Sender<isize>>,
        hwnd: isize,
        nearest: isize,
        held_elements: HeldElements,
        /// Whether its reads read their focus, `None`, or drop it, and for
        /// what reason.
        reads: Mutex<Option<&'static str>>,
        facts: Mutex<Sender<DeliveredFact>>,
    }

    /// A read of a focus, for a stand-in read that reads its focus.
    fn read_focus() -> Judged {
        Judged::Read(Box::new(UiaFocusRead {
            hwnd: 0,
            snapshot: UiaSnapshotFact {
                runtime_id: vec![42],
                role: verbatim_model::Role::Button,
                name: None,
                value: None,
                states: verbatim_model::StateSet::new(),
                details: NodeDetails::default(),
            },
            ancestors: Vec::new(),
            ancestors_end: crate::protocol::AncestorsEnd::Top,
            selected_child: None,
            value_from_selection: false,
            window: None,
            same_as_last: None,
            bound_us: 2_000_000,
        }))
    }

    impl Reading for Script {
        /// The event's marker, its gate, and the element it is of, if
        /// any.
        type Job = (u64, Option<Receiver<()>>, Vec<i32>);
        type Reader = ThreadId;

        fn reader(&self, _: Pid) -> Option<ThreadId> {
            Some(thread::current().id())
        }

        fn kind(&self, _: &Self::Job) -> FocusEvent {
            if self.notifications.load(Ordering::SeqCst) {
                FocusEvent::Notification
            } else {
                FocusEvent::Focus
            }
        }

        fn key<'a>(&self, (_, _, element): &'a Self::Job) -> Option<(FocusEvent, &'a [i32])> {
            (!element.is_empty()).then_some((FocusEvent::Focus, element.as_slice()))
        }

        fn read(
            &self,
            reader: Option<&mut ThreadId>,
            _: Meta,
            (marker, gate, _): Self::Job,
            watched: &WatchedRead,
        ) -> Judged {
            if self.nearest != 0 {
                watched.learn(self.nearest);
            }
            if let Some(thread) = reader {
                let _ = self
                    .read_on
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send((marker, *thread));
            }
            if let Some(gate) = gate {
                // The test opens the gate, or drops it.
                let _ = gate.recv();
            }
            let _ = self
                .returned
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send((marker, watched.clone()));
            match *self.reads.lock().unwrap_or_else(PoisonError::into_inner) {
                Some(reason) => Judged::Dropped(reason),
                None => read_focus(),
            }
        }

        fn top_level(&self, window: isize) -> isize {
            window
        }

        fn window(&self, _: &Self::Job) -> Option<isize> {
            Some(self.hwnd)
        }

        fn abandoned(&self, meta: &Meta) {
            self.held_elements.abandoned(meta.trace);
        }

        fn moved_on(&self, window: isize) -> bool {
            // Read before the judgement is sent, so a test that changes the
            // answer once it has seen a judgement changes only later ones.
            let moved_on = self.moved_on.load(Ordering::SeqCst);
            let _ = self
                .asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(window);
            moved_on
        }

        fn finish(&self, reader: Option<ThreadId>) {
            if let Some(thread) = reader {
                let _ = self
                    .finished
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send(thread);
            }
        }

        fn outcome(&self, pid: Pid, meta: Meta, judged: Judged, placed: Option<isize>) {
            let reason = match judged {
                Judged::Dropped(reason) => reason,
                Judged::Read(_) => "it was read",
                ref other @ Judged::Passed(_) => {
                    panic!("the stand-in neither drops nor reads: {other:?}")
                }
            };
            let _ = self
                .facts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(judged.into_fact(meta.marker, meta.kind, placed));
            let _ = self
                .outcomes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send((pid, meta.marker, reason));
        }
    }

    struct Fixture {
        readers: Readers<Script>,
        outcomes: Receiver<(Pid, u64, &'static str)>,
        read_on: Receiver<(u64, ThreadId)>,
        returned: Receiver<(u64, WatchedRead)>,
        finished: Receiver<ThreadId>,
        asked: Receiver<isize>,
        facts: Receiver<DeliveredFact>,
    }

    impl Fixture {
        /// Has the stand-in's reads read their focus, `None`, or drop it
        /// for `reason`.
        fn set_reads(&self, reads: Option<&'static str>) {
            *self
                .readers
                .shared
                .reading
                .reads
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = reads;
        }

        /// Says, from now on, whether the user has moved on.
        fn set_moved_on(&self, moved_on: bool) {
            self.readers
                .shared
                .reading
                .moved_on
                .store(moved_on, Ordering::SeqCst);
        }
    }

    /// Readers abandoning a read past `deadline`, with the grace the
    /// listener uses, and a user who never moves on.
    fn fixture(deadline: Duration) -> Fixture {
        fixture_with_grace(deadline, MOVED_ON_GRACE)
    }

    /// Readers abandoning a read past `deadline`, or past `grace` once the
    /// user has moved on, whose events' senders are windows that their
    /// reads find no other window for.
    fn fixture_with_grace(deadline: Duration, grace: Duration) -> Fixture {
        fixture_of(deadline, grace, (WINDOW, 0))
    }

    /// Readers abandoning a read past `deadline`, or past `grace` once the
    /// user has moved on, whose events' senders are window `hwnd`, 0 for
    /// senders that are not windows, and whose reads find the sender's
    /// nearest window to be `nearest`, 0 for none.
    fn fixture_of(deadline: Duration, grace: Duration, (hwnd, nearest): (isize, isize)) -> Fixture {
        let (outcomes_tx, outcomes) = mpsc::channel();
        let (read_tx, read_on) = mpsc::channel();
        let (returned_tx, returned) = mpsc::channel();
        let (finished_tx, finished) = mpsc::channel();
        let (asked_tx, asked) = mpsc::channel();
        let (facts_tx, facts) = mpsc::channel();
        Fixture {
            readers: Readers::new(
                Script {
                    outcomes: Mutex::new(outcomes_tx),
                    read_on: Mutex::new(read_tx),
                    returned: Mutex::new(returned_tx),
                    finished: Mutex::new(finished_tx),
                    moved_on: AtomicBool::new(false),
                    notifications: AtomicBool::new(false),
                    asked: Mutex::new(asked_tx),
                    hwnd,
                    nearest,
                    held_elements: HeldElements::default(),
                    reads: Mutex::new(Some("read")),
                    facts: Mutex::new(facts_tx),
                },
                deadline,
                grace,
            ),
            outcomes,
            read_on,
            returned,
            finished,
            asked,
            facts,
        }
    }

    fn meta(marker: u64) -> Meta {
        Meta {
            marker,
            kind: FocusEvent::Focus,
            trace: TraceId::mint(),
            timing: EventTiming::default(),
        }
    }

    const WAIT: Duration = Duration::from_secs(10);

    /// What each outcome of a focus event says of its application's gap
    /// record: a read or a verdict of NVDA's checks, the class-list skip
    /// among them, answers a copy in the top-level window its read found;
    /// the listener giving up on the application, a timed-out
    /// nearest-window lookup among them, answers a copy in any window, and
    /// a drop for no window answers it to be read at once; and every other
    /// drop of a focus leaves the record open, as does any outcome of a
    /// selection, a notification, or a menu opening. Every reason the
    /// listener drops a focus for is listed.
    #[test]
    fn a_focus_events_drop_answers_its_gap_record_or_leaves_it_open() {
        let placed = Some(70);
        let dropped = |reason| gap_answer(FocusEvent::Focus, &Judged::Dropped(reason), placed);
        assert_eq!(
            gap_answer(FocusEvent::Focus, &read_focus(), placed),
            GapAnswer::Answers(placed)
        );
        for verdict in [
            "its window's class is on NVDA's MSAA list, so its MSAA events speak for it",
            "its window is hung",
            "its comparison with the current focus failed",
            "a repeat of the last focus, which still has the keyboard focus",
            "its window is not related to the foreground window",
            "the console window's own focus",
            "Core's hidden frame",
        ] {
            assert_eq!(dropped(verdict), GapAnswer::Answers(placed), "{verdict}");
        }
        assert_eq!(
            dropped("it has no window above it, so NVDA makes no object for it"),
            GapAnswer::ReadsNow
        );
        for anywhere in [
            "its nearest window's lookup timed out, so NVDA drops it",
            "its read did not return in time",
            "it waited too long behind its application's abandoned reads",
            "its read ran out of time before its window was looked up",
            "its event says it did not have the keyboard focus, and its read's time was up",
        ] {
            assert_eq!(dropped(anywhere), GapAnswer::Answers(None), "{anywhere}");
        }
        for open in [
            "its element no longer has the keyboard focus",
            "the user moved on from a slow window",
            "a newer focus event of its element replaced it",
            "no thread to read it",
            "the listener is shutting down",
            "its element could not be kept for its read",
            "its reading thread has no UIA client",
            "its element could not be resolved",
        ] {
            assert_eq!(dropped(open), GapAnswer::LeavesOpen, "{open}");
        }
        for kind in [
            FocusEvent::Selection,
            FocusEvent::Notification,
            FocusEvent::MenuOpened,
        ] {
            assert_eq!(
                gap_answer(kind, &read_focus(), placed),
                GapAnswer::LeavesOpen
            );
        }
    }

    /// A focus whose nearest window's class is on NVDA's MSAA class list,
    /// and not under a shell window, is skipped with no call, by the class
    /// verdict the outposts' arbitrators give, and its drop answers the
    /// record in its top-level window; one on no list, or under a shell
    /// window, is read, and so is any focus while a backend override
    /// forcing UIA holds for its application.
    #[test]
    fn a_focus_whose_windows_class_is_on_the_msaa_list_is_skipped_and_answers_the_record() {
        let edit = WindowClasses::new("Edit", "Notepad");
        assert!(skipped_by_class(&edit, false));
        assert!(!skipped_by_class(&edit, true), "the override forcing UIA");
        assert!(!skipped_by_class(
            &WindowClasses::new("Windows.UI.Input.InputSite.WindowClass", "Notepad"),
            false
        ));
        assert!(!skipped_by_class(
            &WindowClasses::new("Edit", "Shell_TrayWnd"),
            false
        ));
        assert_eq!(
            Judged::Dropped(MSAA_CLASS).into_fact(9, FocusEvent::Focus, Some(70)),
            DeliveredFact::UiaFocusDropped {
                marker: 9,
                focus: true,
                answer: GapAnswer::Answers(Some(70)),
            }
        );
    }

    /// Each drop the reading threads send carries its answer to its
    /// application's gap record and the top-level window it is placed in:
    /// that of the sender's own window from the event's cache, or of the
    /// nearest window its read found for a sender with none, or none when
    /// it found neither, which answers a copy in any window.
    #[test]
    fn a_drop_carries_its_answer_and_its_top_level_window() {
        let cases = [
            ((WINDOW, 0), Some(WINDOW)),
            ((0, NEAREST), Some(NEAREST)),
            ((0, 0), None),
        ];
        for ((hwnd, nearest), placed) in cases {
            let fixture = fixture_of(WAIT, MOVED_ON_GRACE, (hwnd, nearest));
            fixture.set_reads(Some(HUNG));
            fixture.readers.push(Pid(1), meta(1), (1, None, Vec::new()));
            assert_eq!(
                fixture.facts.recv_timeout(WAIT),
                Ok(DeliveredFact::UiaFocusDropped {
                    marker: 1,
                    focus: true,
                    answer: GapAnswer::Answers(placed),
                }),
                "a sender of window {hwnd} whose nearest window is {nearest}"
            );
        }
    }

    #[test]
    fn an_applications_events_are_read_in_the_order_they_arrived_on_one_thread() {
        let fixture = fixture(Duration::from_secs(60));
        for marker in 1..=3 {
            fixture
                .readers
                .push(Pid(5), meta(marker), (marker, None, Vec::new()));
        }
        let outcomes: Vec<(Pid, u64, &str)> = (0..3)
            .map(|_| fixture.outcomes.recv_timeout(WAIT).expect("an outcome"))
            .collect();
        assert_eq!(
            outcomes,
            [
                (Pid(5), 1, "read"),
                (Pid(5), 2, "read"),
                (Pid(5), 3, "read")
            ]
        );
        let threads: Vec<ThreadId> = (0..3)
            .map(|_| fixture.read_on.recv_timeout(WAIT).expect("a read").1)
            .collect();
        assert!(threads.iter().all(|thread| *thread == threads[0]));
        assert_eq!(fixture.readers.stop(), 1);
    }

    /// A notification's check being made is no pending focus: a menu
    /// opening that arrives meanwhile, of its application or another, is
    /// kept, as NVDA's `isPendingEvents("gainFocus")` counts no
    /// notification.
    #[test]
    fn a_running_notification_check_is_no_pending_focus() {
        let fixture = fixture(Duration::from_secs(60));
        fixture
            .readers
            .shared
            .reading
            .notifications
            .store(true, Ordering::SeqCst);
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        assert_eq!(
            (
                fixture.readers.focus_pending(Some(Pid(5))),
                fixture.readers.focus_pending(Some(Pid(6)))
            ),
            (false, false)
        );
        open.send(()).expect("the read waits");
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "read")
        );
        assert_eq!(fixture.readers.stop(), 1);
    }

    #[test]
    fn a_newer_event_of_an_element_replaces_its_waiting_one() {
        let fixture = fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), vec![42, 1]));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        // Event 1 is being read, so it stays; 2 waits and is replaced by 4,
        // another element's 3 stays.
        fixture
            .readers
            .push(Pid(5), meta(2), (2, None, vec![42, 2]));
        fixture
            .readers
            .push(Pid(5), meta(3), (3, None, vec![42, 3]));
        fixture
            .readers
            .push(Pid(5), meta(4), (4, None, vec![42, 2]));
        open.send(()).expect("the gate opens");
        let outcomes: Vec<(Pid, u64, &str)> = (0..4)
            .map(|_| fixture.outcomes.recv_timeout(WAIT).expect("an outcome"))
            .collect();
        assert_eq!(
            outcomes,
            [
                (Pid(5), 2, "a newer focus event of its element replaced it"),
                (Pid(5), 1, "read"),
                (Pid(5), 3, "read"),
                (Pid(5), 4, "read"),
            ]
        );
        assert_eq!(fixture.readers.stop(), 1);
    }

    /// The idle barrier's wait on the reading threads: run at once when no
    /// event waits or is read, and otherwise only once the outcome of every
    /// event queued before it has been sent, a held read's included.
    #[test]
    fn the_idle_waiter_runs_after_the_outcome_of_every_event_queued_before_it() {
        let fixture = fixture(Duration::from_secs(60));
        let idle = |round: u64| -> IdleWaiter {
            let outcomes = fixture
                .readers
                .shared
                .reading
                .outcomes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            Box::new(move || {
                let _ = outcomes.send((Pid(0), round, "idle"));
            })
        };
        fixture.readers.when_idle(idle(1));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(0), 1, "idle"),
            "nothing is read, so the waiter runs at once"
        );
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.readers.push(Pid(5), meta(2), (2, None, Vec::new()));
        fixture.readers.when_idle(idle(2));
        assert_eq!(
            fixture.outcomes.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "event 1 is being read and event 2 waits"
        );
        open.send(()).expect("the held read waits");
        let outcomes: Vec<(Pid, u64, &str)> = (0..3)
            .map(|_| fixture.outcomes.recv_timeout(WAIT).expect("an outcome"))
            .collect();
        assert_eq!(
            outcomes,
            [
                (Pid(5), 1, "read"),
                (Pid(5), 2, "read"),
                (Pid(0), 2, "idle")
            ]
        );
        assert_eq!(fixture.readers.stop(), 1);
    }

    /// The idle barrier's waiter does not wait for events queued after it:
    /// it runs once the event being read when it came has its outcome,
    /// ahead of the read of an event queued behind it, so a stream of
    /// events cannot hold it back.
    #[test]
    fn the_idle_waiter_does_not_wait_for_events_queued_after_it() {
        let fixture = fixture(Duration::from_secs(60));
        let outcomes = fixture
            .readers
            .shared
            .reading
            .outcomes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.readers.when_idle(Box::new(move || {
            let _ = outcomes.send((Pid(0), 1, "idle"));
        }));
        fixture.readers.push(Pid(5), meta(2), (2, None, Vec::new()));
        open.send(()).expect("the held read waits");
        let outcomes: Vec<(Pid, u64, &str)> = (0..3)
            .map(|_| fixture.outcomes.recv_timeout(WAIT).expect("an outcome"))
            .collect();
        assert_eq!(
            outcomes,
            [
                (Pid(5), 1, "read"),
                (Pid(0), 1, "idle"),
                (Pid(5), 2, "read")
            ]
        );
        assert_eq!(fixture.readers.stop(), 1);
    }

    #[test]
    fn an_application_whose_read_is_held_does_not_hold_another() {
        let fixture = fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.readers.push(Pid(6), meta(2), (2, None, Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(6), 2, "read"),
            "the other application's event, while the first is held"
        );
        open.send(()).expect("the held read waits");
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "read")
        );
        assert_eq!(fixture.readers.stop(), 2);
    }

    #[test]
    fn a_read_past_its_deadline_is_dropped_and_a_new_thread_reads_on() {
        let fixture = fixture(Duration::from_millis(50));
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, held_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        fixture.readers.push(Pid(5), meta(2), (2, None, Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "its read did not return in time")
        );
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, "read")
        );
        let (_, read_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        assert_ne!(read_on, held_on, "a new thread read the next event");
        // The abandoned thread returns and says nothing more.
        open.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 2);
        assert_eq!(fixture.outcomes.try_recv().ok(), None);
    }

    /// NVDA's watchdog rule: a read held past its grace is abandoned once
    /// the user has moved on to another thread of its application, judged
    /// again on a foreground change after the grace's own check, which a
    /// foreground change since the read's event called for, found the user
    /// had not; its event is dropped for that, and a new thread reads the
    /// application's next event. The read's start judged the window in
    /// front against the read's window first.
    #[test]
    fn a_held_read_is_abandoned_once_the_user_moves_on_past_its_grace() {
        let fixture = fixture_with_grace(Duration::from_secs(60), Duration::from_millis(50));
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, held_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        assert_eq!(
            fixture
                .asked
                .recv_timeout(WAIT)
                .expect("the start's judgement"),
            WINDOW,
            "the read's start judges the window in front against the read's window"
        );
        fixture.readers.push(Pid(5), meta(2), (2, None, Vec::new()));
        fixture.readers.foreground_changed();
        assert_eq!(
            fixture.asked.recv_timeout(WAIT).expect("the grace's check"),
            WINDOW,
            "the grace's end asks about the read's window"
        );
        fixture.set_moved_on(true);
        fixture.readers.foreground_changed();
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "the user moved on from a slow window")
        );
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, "read")
        );
        let (_, read_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        assert_ne!(read_on, held_on, "a new thread read the next event");
        // The abandoned thread returns and says nothing more.
        open.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 2);
        assert_eq!(fixture.outcomes.try_recv().ok(), None);
    }

    /// Within its grace a held read is left alone though the user has
    /// moved on, its start found so, and a foreground change says so:
    /// nothing is asked after the start's judgement, and the read's own
    /// outcome is sent once it returns.
    #[test]
    fn a_held_read_within_its_grace_is_left_alone_though_the_user_moved_on() {
        let fixture = fixture_with_grace(Duration::from_secs(60), Duration::from_secs(60));
        fixture.set_moved_on(true);
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        assert_eq!(
            fixture
                .asked
                .recv_timeout(WAIT)
                .expect("the start's judgement"),
            WINDOW
        );
        fixture.readers.foreground_changed();
        open.send(()).expect("the held read waits");
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "read")
        );
        assert_eq!(fixture.readers.stop(), 1);
        assert_eq!(
            fixture.asked.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "nothing more asked within the grace"
        );
    }

    /// Past its grace a held read is left alone while the user has not
    /// moved on, both when the grace ends, after a foreground change since
    /// the read's event, and on a later foreground change, so only the
    /// deadline can abandon it
    /// (`a_read_past_its_deadline_is_dropped_and_a_new_thread_reads_on`):
    /// the read's own outcome is sent once it returns.
    #[test]
    fn a_held_read_past_its_grace_is_left_alone_while_the_user_has_not_moved_on() {
        let fixture = fixture_with_grace(Duration::from_secs(60), Duration::from_millis(50));
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, read_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        assert_eq!(
            fixture
                .asked
                .recv_timeout(WAIT)
                .expect("the start's judgement"),
            WINDOW
        );
        fixture.readers.foreground_changed();
        assert_eq!(
            fixture.asked.recv_timeout(WAIT).expect("the grace's check"),
            WINDOW
        );
        fixture.readers.foreground_changed();
        assert_eq!(
            fixture
                .asked
                .recv_timeout(WAIT)
                .expect("the foreground change's check"),
            WINDOW
        );
        open.send(()).expect("the held read waits");
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "read")
        );
        assert_eq!(fixture.readers.stop(), 1);
        assert_eq!(
            fixture.finished.recv_timeout(WAIT).expect("a thread ended"),
            read_on,
            "the thread that read it, never abandoned"
        );
    }

    /// A read held past its grace whose sender is not a window, before the
    /// read has found the sender's nearest window, is not judged, though
    /// the user has moved on and a foreground change says so: only its
    /// deadline abandons it, and nothing is asked.
    #[test]
    fn a_windowless_sender_s_read_is_not_judged_before_its_window_is_known() {
        let fixture = fixture_of(
            Duration::from_millis(300),
            Duration::from_millis(50),
            (0, 0),
        );
        fixture.set_moved_on(true);
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.readers.foreground_changed();
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "its read did not return in time")
        );
        open.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 1);
        assert_eq!(fixture.asked.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    /// Once a read whose sender is not a window has found the sender's
    /// nearest window, the read is judged by that window: the window in
    /// front, already another thread's when it was found, has the read
    /// checked when its grace ends with no foreground event, and abandoned.
    /// That window is asked about twice, when it is found and when the
    /// grace ends, and nothing else is asked.
    #[test]
    fn a_read_that_found_its_sender_s_nearest_window_is_judged_by_it() {
        let fixture = fixture_of(
            Duration::from_secs(60),
            Duration::from_millis(50),
            (0, NEAREST),
        );
        fixture.set_moved_on(true);
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "the user moved on from a slow window")
        );
        open.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 1);
        let asked: Vec<isize> = fixture.asked.try_iter().collect();
        assert_eq!(asked, [NEAREST, NEAREST]);
    }

    /// An event queued after a foreground event, behind a read abandoned as
    /// the user moved on, is checked at its own grace end with no further
    /// foreground event, since the window in front was already another
    /// thread's when its read began, and is abandoned as moved on too.
    #[test]
    fn a_read_begun_with_the_front_on_another_thread_is_judged_at_its_grace_end() {
        let fixture = fixture_with_grace(Duration::from_secs(60), Duration::from_millis(50));
        let (open_first, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        assert_eq!(
            fixture
                .asked
                .recv_timeout(WAIT)
                .expect("the start's judgement"),
            WINDOW
        );
        fixture.set_moved_on(true);
        fixture.readers.foreground_changed();
        let (open_second, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "the user moved on from a slow window")
        );
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 2);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, "the user moved on from a slow window")
        );
        open_first.send(()).expect("the first held read waits");
        open_second.send(()).expect("the second held read waits");
        assert_eq!(fixture.readers.stop(), 2);
        assert_eq!(fixture.outcomes.try_recv().ok(), None);
        // The first read's grace check, then the second's start judgement
        // and grace check.
        let asked: Vec<isize> = fixture.asked.try_iter().collect();
        assert_eq!(asked, [WINDOW, WINDOW, WINDOW]);
    }

    /// With [`ABANDONED_LIMIT`] of an application's threads abandoned and
    /// still in their calls, its next events are not dropped: they wait,
    /// pending focuses for the application's own menu openings and not for
    /// another application's, and the first abandoned thread whose call
    /// returns before the application's bound reads them in the order they
    /// arrived.
    #[test]
    fn an_event_at_the_abandoned_limit_waits_for_a_thread_that_returns() {
        let fixture = fixture_with_grace(Duration::from_secs(60), Duration::from_millis(50));
        fixture.set_moved_on(true);
        let (open_first, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, first) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "the user moved on from a slow window")
        );
        let (open_second, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 2);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, "the user moved on from a slow window")
        );
        fixture
            .readers
            .push(Pid(5), meta(3), (3, None, vec![42, 3]));
        fixture
            .readers
            .push(Pid(5), meta(4), (4, None, vec![42, 4]));
        assert_eq!(
            fixture.outcomes.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "events 3 and 4 wait"
        );
        assert_eq!(
            (
                fixture.readers.focus_pending(Some(Pid(5))),
                fixture.readers.focus_pending(Some(Pid(6)))
            ),
            (true, false),
            "a focus waiting with no thread to read it is pending for its own application only"
        );
        fixture.set_moved_on(false);
        open_first.send(()).expect("the first held read waits");
        let outcomes: Vec<(Pid, u64, &str)> = (0..2)
            .map(|_| fixture.outcomes.recv_timeout(WAIT).expect("an outcome"))
            .collect();
        assert_eq!(outcomes, [(Pid(5), 3, "read"), (Pid(5), 4, "read")]);
        let read_on: Vec<(u64, ThreadId)> = (0..2)
            .map(|_| fixture.read_on.recv_timeout(WAIT).expect("a read"))
            .collect();
        assert_eq!(
            read_on,
            [(3, first), (4, first)],
            "the first abandoned thread, returned, read them"
        );
        open_second.send(()).expect("the second held read waits");
        assert_eq!(fixture.readers.stop(), 2);
        assert_eq!(fixture.outcomes.try_recv().ok(), None);
    }

    /// The bound at [`ABANDONED_LIMIT`] is the application's: events 3 and
    /// 4, waiting while none of its abandoned threads returns, are dropped
    /// in order, which resolves their markers, and event 5, pushed after
    /// the bound has passed, is dropped at once, as it is pushed, which is
    /// what shows the bound is the application's rather than each event's
    /// own deadline after its push. None of them is read.
    #[test]
    fn events_waiting_at_the_abandoned_limit_are_dropped_at_the_applications_bound() {
        let fixture = fixture(Duration::from_millis(50));
        let (open_first, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "its read did not return in time")
        );
        let (open_second, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, "its read did not return in time")
        );
        fixture.readers.push(Pid(5), meta(3), (3, None, Vec::new()));
        fixture.readers.push(Pid(5), meta(4), (4, None, Vec::new()));
        let waited = "it waited too long behind its application's abandoned reads";
        let outcomes: Vec<(Pid, u64, &str)> = (0..2)
            .map(|_| fixture.outcomes.recv_timeout(WAIT).expect("an outcome"))
            .collect();
        assert_eq!(outcomes, [(Pid(5), 3, waited), (Pid(5), 4, waited)]);
        fixture.readers.push(Pid(5), meta(5), (5, None, Vec::new()));
        assert_eq!(
            fixture.outcomes.try_recv(),
            Ok((Pid(5), 5, waited)),
            "dropped as it was pushed"
        );
        open_first.send(()).expect("the first held read waits");
        open_second.send(()).expect("the second held read waits");
        assert_eq!(fixture.readers.stop(), 2);
        assert_eq!(fixture.outcomes.try_recv().ok(), None);
        let read: Vec<u64> = fixture
            .read_on
            .try_iter()
            .map(|(marker, _)| marker)
            .collect();
        assert_eq!(read, [1, 2], "events 3, 4 and 5 were never read");
    }

    /// The window of the element held for the focus Core holds, and
    /// whether Core has judged every focus accepted.
    fn held_window(held_elements: &HeldElements) -> (Option<isize>, bool) {
        let (held, settled) = held_elements.held();
        (held.map(|(_, window)| window), settled)
    }

    /// The element compared with is the one behind the focus Core holds: a
    /// focus accepted waits for Core's judgement, and nothing is a repeat
    /// meanwhile; one Core took is held; one Core refused, as it refuses a
    /// shell window that only stages a switch, is not, and the focus Core
    /// still holds stays; and once Core holds a focus no read of the
    /// listener's gave it, none is held.
    #[test]
    fn the_element_compared_with_is_the_one_behind_the_focus_core_holds() {
        const STAGING: isize = 0x8180;
        let uia = Uia::new().expect("a UIA client");
        let element = uia.root_element().expect("the desktop's root element");
        let held_elements = HeldElements::default();
        let arrival = |trace| Arrival {
            trace,
            observed_at_us: 0,
        };
        let (taken, refused) = (TraceId::mint(), TraceId::mint());
        held_elements.accept(
            1,
            &element,
            (WINDOW, arrival(taken)),
            &WatchedRead::default(),
        );
        assert_eq!(held_window(&held_elements), (None, false));
        held_elements.core_focus(Some(taken), Some((taken, true)));
        assert_eq!(held_window(&held_elements), (Some(WINDOW), true));
        held_elements.accept(
            1,
            &element,
            (STAGING, arrival(refused)),
            &WatchedRead::default(),
        );
        assert_eq!(held_window(&held_elements), (Some(WINDOW), false));
        held_elements.core_focus(Some(refused), Some((taken, true)));
        assert_eq!(held_window(&held_elements), (Some(WINDOW), true));
        held_elements.core_focus(None, Some((TraceId::mint(), true)));
        assert_eq!(held_window(&held_elements), (None, true));
    }

    /// A focus a read accepted just before the watchdog abandoned it waits
    /// for no judgement once the watchdog has, since Core was told the event
    /// was dropped; and a read that returns after it was abandoned keeps no
    /// focus, so only the newer focus accepted while it was held is Core's
    /// to judge.
    #[test]
    fn a_read_returning_after_it_was_abandoned_accepts_no_focus() {
        const NEWER_WINDOW: isize = 0x8180;
        let uia = Uia::new().expect("a UIA client");
        let element = uia.root_element().expect("the desktop's root element");
        let fixture = fixture_with_grace(Duration::from_secs(60), Duration::from_millis(50));
        let held_elements = fixture.readers.shared.reading.held_elements.clone();
        let first = meta(1);
        let arrival = Arrival {
            trace: first.trace,
            observed_at_us: 0,
        };
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), first, (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        assert_eq!(
            fixture
                .asked
                .recv_timeout(WAIT)
                .expect("the start's judgement"),
            WINDOW
        );
        // The read accepts its focus while it is not yet abandoned.
        held_elements.accept(1, &element, (WINDOW, arrival), &WatchedRead::default());
        assert_eq!(held_window(&held_elements), (None, false));
        fixture.set_moved_on(true);
        fixture.readers.foreground_changed();
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "the user moved on from a slow window")
        );
        assert_eq!(
            held_window(&held_elements),
            (None, true),
            "Core never judges the abandoned read's focus"
        );
        open.send(()).expect("the held read waits");
        let (marker, abandoned) = fixture
            .returned
            .recv_timeout(WAIT)
            .expect("the read returns");
        assert_eq!(marker, 1);
        let newer = Arrival {
            trace: TraceId::mint(),
            observed_at_us: 0,
        };
        held_elements.accept(
            2,
            &element,
            (NEWER_WINDOW, newer),
            &WatchedRead::new(NEWER_WINDOW, (None, None), false),
        );
        held_elements.accept(1, &element, (WINDOW, arrival), &abandoned);
        held_elements.core_focus(Some(newer.trace), Some((newer.trace, true)));
        assert_eq!(
            held_window(&held_elements),
            (Some(NEWER_WINDOW), true),
            "the returning read's focus was never kept"
        );
        assert_eq!(fixture.readers.stop(), 1);
    }

    /// A read with no foreground event since its event, whose start found
    /// the window in front on its window's own thread, is not judged when
    /// its grace ends though the user has moved on since: only its
    /// deadline abandons it, and nothing is asked after its start.
    #[test]
    fn a_read_with_no_foreground_event_since_its_event_is_not_judged_at_its_grace_end() {
        let fixture = fixture_with_grace(Duration::from_millis(300), Duration::from_millis(50));
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        assert_eq!(
            fixture
                .asked
                .recv_timeout(WAIT)
                .expect("the start's judgement"),
            WINDOW
        );
        fixture.set_moved_on(true);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "its read did not return in time")
        );
        assert_eq!(fixture.asked.try_recv(), Err(mpsc::TryRecvError::Empty));
        open.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 1);
    }

    #[test]
    fn a_thread_ends_when_its_applications_outpost_ends() {
        let fixture = fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture
            .readers
            .push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, first) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        // The outpost ends while the thread reads: the read is finished
        // and sent, then the thread ends.
        fixture.readers.outpost_ended(Pid(5));
        open.send(()).expect("the held read waits");
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "read")
        );
        assert_eq!(
            fixture.finished.recv_timeout(WAIT).expect("a thread ended"),
            first
        );
        // The application runs on: its next event starts a new thread.
        fixture.readers.push(Pid(5), meta(2), (2, None, Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, "read")
        );
        let (_, second) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        assert_ne!(second, first, "a new thread read the next event");
        let _ = fixture.readers.stop();
        assert_eq!(fixture.finished.try_recv().ok(), Some(second));
    }

    #[test]
    fn the_reads_reused_from_the_focus_before_begin_above_the_stop() {
        let fact = |id: i32| UiaAncestorFact {
            snapshot: UiaSnapshotFact {
                runtime_id: vec![42, id],
                role: verbatim_model::Role::Group,
                name: None,
                value: None,
                states: verbatim_model::StateSet::new(),
                details: verbatim_model::NodeDetails::default(),
            },
            hwnd: 0,
            content: true,
        };
        let before = [fact(1), fact(2), fact(3)];
        let ids = |reused: Vec<UiaAncestorFact>| -> Vec<Vec<i32>> {
            reused
                .into_iter()
                .map(|ancestor| ancestor.snapshot.runtime_id)
                .collect()
        };
        // Stopped at the focus before itself: all its ancestors are reused.
        assert_eq!(
            ids(reused_ancestors(&before, 0)),
            [vec![42, 1], vec![42, 2], vec![42, 3]]
        );
        // Stopped at its second ancestor: those above it.
        assert_eq!(ids(reused_ancestors(&before, 2)), [vec![42, 3]]);
        assert_eq!(ids(reused_ancestors(&before, 3)), Vec::<Vec<i32>>::new());
        assert_eq!(ids(reused_ancestors(&before, 9)), Vec::<Vec<i32>>::new());
    }
}
