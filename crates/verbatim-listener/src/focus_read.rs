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
//!   fact", the verdicts); one whose nearest window fails NVDA's
//!   acceptance test is dropped; the console window's own element, which
//!   NVDA's `consoleUIAWindow` refuses, and a focus in Core's hidden frame
//!   are dropped; and a sender without the keyboard focus, read with a
//!   plain property read, is dropped, as NVDA's `shouldAllowUIAFocusEvent`
//!   drops it. A shell window that only stages a switch is read and goes to
//!   Core like any focus, and Core's rules decide what is said of it. A
//!   menu opening is judged by NVDA's automation event handler's checks
//!   instead, with a sender UI Automation counts as the thread's own last
//!   read, while that read is current, judged as that focus, with its
//!   window and without the native check. NVDA's duplicate test is not
//!   made: a focus that passes the other checks is read in full, and Core,
//!   which holds the focus, judges a repeat
//!   (`docs/design/ownership.md`);
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
//! its bound, once it has passed every check, is published then from the
//! progress its thread shares ([`ReadProgress`]), as the schedule asks,
//! the same read through the same function, and its thread's late result
//! is discarded; the thread is not abandoned for this, and reads its
//! application's next event when its call returns. A read still in its
//! checks at the bound is left to the rules below.
//!
//! Each application's read under way is its work in flight in the
//! schedule (`verbatim_reader_rules::in_flight`,
//! `docs/design/logic-tests.md`, "The work in flight, shared by both
//! readers"), which abandons a thread whose read runs past
//! [`READ_DEADLINE`], the budget and the one call that can be under way
//! when the budget passes, and, as NVDA's watchdog cancels a slow call
//! once the user has moved on (`docs/nvda/main-loop-and-watchdog.md`,
//! "The watchdog: detection"), a thread whose read of a focus is still
//! under way while the window in front belongs to the read's application
//! on another UI thread than the window being read. That is judged at
//! [`MOVED_ON_GRACE`] after the read's start and every [`MOVED_ON_GRACE`]
//! after, with no foreground event as evidence, once the read's window is
//! known, from the event's cached window handle or from the sender's
//! nearest window once the read has found it ([`WatchedRead::learn`]); a
//! selection or a notification is never judged so. One process can own
//! windows on several UI threads, as File Explorer's folder windows, the
//! taskbar and the Alt+Tab switcher are all `explorer.exe`'s, and the
//! application's focus on another thread must not wait behind a slow read
//! of the first. Either way a walk armed is published first, the event is
//! dropped unless it was published, a new thread takes the application's
//! later events, and the abandoned thread, once UIA gives up, says nothing
//! more, forgets the focus it read, which Core never got, and ends or
//! reads on. An application whose abandoned threads reach
//! [`ABANDONED_LIMIT`](verbatim_listener_logic::focus::ABANDONED_LIMIT) gets no new thread until one returns: its events
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
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_LegacyIAccessibleStatePropertyId,
};

use verbatim_model::{NodeDetails, Pid, State, TraceId};
use verbatim_uia::map::{CachedUiaParts, add_legacy_checked_state, wants_legacy_checked_state};
use verbatim_uia::{
    Ancestor, AncestorGates, Cached, ElementExt as _, Fill, FocusInput, ListenerEvent, Node,
    NodeGates, Shape, Uia,
};
use verbatim_uia_rops::{Ancestry, FocusAncestry, FocusQuery, Progress, focus_ancestry_classic};

use verbatim_listener_logic::focus::{
    Arrival, COMPARISON_FAILED, CONSOLE_OWN, FocusEvent, HIDDEN_FRAME, HUNG, Judged, MSAA_CLASS,
    Meta, NO_WINDOW, NOT_FOCUSED, NOT_RELATED, NOTIFICATION_NO_WINDOW, OUT_OF_TIME_BEFORE_LOOKUP,
    UNFOCUSED_OUT_OF_TIME, arrival_of, nearest_timeout_reason,
};
use verbatim_listener_logic::schedule::{
    AlarmChange, QueuePlace, Request, Schedule, ScheduleEffect, ScheduleInput, Waking,
};
use verbatim_model::protocol::{
    AncestorsEnd, DeliveredFact, UiaAncestorFact, UiaFocusRead, UiaSnapshotFact,
};
use verbatim_process::alarm::Alarm;
use verbatim_process::clock::now_us;
use verbatim_process::clock::{Clock, ObservationClock};
use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_reader_rules::MOVED_ON_GRACE;
use verbatim_reader_rules::Window;
use verbatim_reader_rules::acceptance::{EventName, RequestedEvents, accepts_event};
use verbatim_reader_rules::class::{ClassVerdict, WindowClasses, class_verdict};
use verbatim_reader_rules::in_flight::RuleWindow;
use verbatim_win32::window::ForegroundReader;
use verbatim_win32::window::SystemWindows;
use verbatim_win32::window::window_class_name;
use verbatim_win32::window::{
    front_is_another_thread_of_its_application, top_level_of, window_belongs_to_hidden_frame,
    window_is_hung,
};

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

/// What a read does once it learns its window ([`WatchedRead::learn`]):
/// tells the schedule, under the readers' lock.
type OnLearn = Box<dyn Fn(isize) + Send + Sync>;

/// What a read does once it has passed NVDA's checks and its bound is
/// known ([`WatchedRead::arm`]): tells the schedule that bound, by the
/// system's clock, under the readers' lock.
type OnArm = Box<dyn Fn(Instant) + Send + Sync>;

/// One read as its thread and the readers' shell share it. Each read has
/// its own, so an abandoned read that finds its window later changes
/// nothing of another's.
///
/// Its request, by which the schedule's work in flight names it
/// ([`Request`]): what it learns, its bound and its return are given to
/// the schedule under it, and the schedule refuses them once the read is no
/// longer the one under way.
///
/// The window it is of: the sender's own window from the event's cache as
/// the read starts, for a focus ([`Reading::window`]), and the sender's
/// nearest window once the read of a focus whose sender is not a window has
/// found it ([`learn`](Self::learn)), which the schedule is told; 0 while
/// neither is known, and always for a selection or a notification. The
/// schedule judges from it whether the user has moved on
/// ([`MOVED_ON_GRACE`]).
///
/// And, once the read has passed every check NVDA makes before it queues a
/// focus, its progress ([`ReadProgress`]): what it has read so far and its
/// bound, from which the shell publishes it, as the schedule asks, should a
/// call of its walk still be under way at the bound
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback"). The shell reads the progress and never takes it: the reading
/// thread takes it once its walk ends.
#[derive(Clone, Default)]
pub(crate) struct WatchedRead(Arc<Watched>);

#[derive(Default)]
struct Watched {
    request: Option<Request>,
    window: AtomicIsize,
    /// The top-level window of the read's window, once the read has found
    /// it, 0 before.
    top_level: AtomicIsize,
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
    /// The read `request` of window `hwnd`, 0 for one not yet known, which
    /// does `on_learn` once it learns its window and `on_arm` once its bound
    /// is known; `after_elsewhere` says whether the focus event before it
    /// was another application's.
    fn new(
        request: Request,
        hwnd: isize,
        (on_learn, on_arm): (Option<OnLearn>, Option<OnArm>),
        after_elsewhere: bool,
    ) -> Self {
        Self(Arc::new(Watched {
            request: Some(request),
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
            on_learn(hwnd);
        }
    }

    /// The request the read is, `None` for a read no schedule watches.
    fn request(&self) -> Option<Request> {
        self.0.request
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

    /// Whether `other` is this read.
    fn same(&self, other: &WatchedRead) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The read has passed NVDA's checks with `progress`, which says its
    /// bound, and the schedule is told when that is.
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

    /// Takes the read's progress, ending it: the shell has nothing more to
    /// publish from.
    fn take_progress(&self) -> Option<ReadProgress> {
        self.lock_progress().take()
    }

    /// Puts `progress` back, its walk ended, for a last call
    /// ([`ReadProgress::finished`]).
    fn restore_progress(&self, progress: ReadProgress) {
        *self.lock_progress() = Some(progress);
    }

    /// The read as it is published at its bound, from its progress
    /// ([`read_at_bound`]), read and not taken; `None` when the read has no
    /// progress, its walk having ended. Called under the readers' lock, as
    /// the schedule asks ([`ScheduleEffect::Publish`]); the local calls
    /// that complete the chain are made on the thread that asks.
    fn prepare_publish(&self) -> Option<UiaFocusRead> {
        Some(read_at_bound(self.lock_progress().as_ref()?))
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
    same_as_last: Option<verbatim_model::EarlierElement>,
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

/// Whether a focus whose nearest window has the classes `classes` is
/// skipped ([`MSAA_CLASS`]): the class verdict every outpost's arbitrator
/// gives calls it MSAA ([`class_verdict`]), unless a test forces UIA
/// (`forced_uia`, [`FocusReader::forces_uia`]).
fn skipped_by_class(classes: &WindowClasses, forced_uia: bool) -> bool {
    !forced_uia && class_verdict(classes) == ClassVerdict::NonUia
}

/// The focus a reading thread read last: the evidence that a runtime id
/// now names another element (`same_as_last`), where the ancestor read
/// stops, and, while it is current, what a menu opening is compared with
/// (`docs/design/ownership.md`). It is the
/// thread's own fact: no judgement of Core's is kept, so a focus Core
/// refused can be the last read, and Core decides what is spoken.
struct LastFocus {
    runtime_id: Vec<i32>,
    element: IUIAutomationElement,
    /// Its raw-view ancestors, nearest first, or `None` when its read left
    /// them unknown: the next read then reuses nothing of this focus.
    ancestors: Option<Vec<UiaAncestorFact>>,
    /// The window it was judged with, NVDA's object's `windowHandle`.
    window: isize,
    /// Its event's place in the order every event was queued in, by which
    /// a later menu opening tells whether it is still current
    /// ([`QueuePlace::current_since`]).
    place: u64,
}

/// One application's focus reader: a UIA client of its own and the focus
/// it read last. Lives on one reading thread.
pub struct FocusReader {
    uia: Uia,
    last: Option<LastFocus>,
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
    /// Whether a test has this reader skip no focus by its window's class
    /// ([`forces_uia`](Self::forces_uia)).
    forced_uia: AtomicBool,
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
    window_readability: verbatim_win32::window::WindowReadabilityReader,
}

/// Told the name of each call a focus read's classic walk is about to make
/// ([`FocusReader::observe_calls`]).
pub type CallObserver = Arc<dyn Fn(&'static str) + Send + Sync>;

impl FocusReader {
    /// A reader with a UIA client of its own, made on the calling thread,
    /// which joins COM's multithreaded apartment. Every read is the classic
    /// walk, which makes no remote operation
    /// (`docs/design/listener-without-connections.md`, section 2).
    ///
    /// # Errors
    ///
    /// The COM error when the client cannot be made.
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            uia: Uia::new()?,
            last: None,
            foreground: Arc::new(verbatim_win32::window::foreground_window_handle),
            clock: Arc::new(Instant::now),
            observed: Arc::new(now_us),
            last_top: None,
            forced_uia: AtomicBool::new(false),
            window: WatchedRead::default(),
            calls: None,
            window_readability: Arc::new(verbatim_win32::window::window_unreadable),
        })
    }

    /// Forgets the focus it read last: the schedule answered that its read
    /// was abandoned, so it is a focus Core never got, which no later read
    /// compares with or reuses.
    pub fn forget_last(&mut self) {
        self.last = None;
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
    pub fn set_window_readability_reader(
        &mut self,
        reader: verbatim_win32::window::WindowReadabilityReader,
    ) {
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

    /// Sets whether this reader skips no focus by its window's class: for
    /// a test that has the listener read a focus in a window on NVDA's
    /// MSAA class list, which it otherwise skips.
    pub fn forces_uia(&self, forced: bool) {
        self.forced_uia.store(forced, Ordering::Relaxed);
    }

    /// [`read`](Self::read), or for a selection or a notification, whose
    /// captured `fact` is given, [`check_fact`](Self::check_fact), in the
    /// read `watched`, which the reading threads' watchdog watches
    /// ([`WatchedRead`]): a focus's read tells it the sender's nearest
    /// window once it has found it.
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
        let judged = self.judge_before_reading(
            sender,
            (&fields, &event.runtime_id),
            (kind, arrival.queue()),
            &budget,
        );
        // The focus's top-level window, once its window is known, placed in
        // the order this application's focuses arrive: the walk's bound is
        // the short one when it is not the top-level window of the focus
        // before, as the listener observed them (the Terms of
        // `docs/design/focus-pipeline-rework.md`), and another
        // application's focus before it is always elsewhere. A local call.
        let known = match judged {
            Ok((window, _)) => window,
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
        let (nearest, require_focus) = match judged {
            Ok(judged) => judged,
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
                // The sender's keyboard focus, NVDA's last check, is the
                // walk's to read, after the earlier element's, in NVDA's
                // order (step 6), unless the checks made it already; its
                // nearest window was found for NVDA's native check, so the
                // read does not ask it again
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
                    require_focus,
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
                    window: nearest,
                    place: arrival.place,
                });
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
            window: nearest,
            place: arrival.place,
        });
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
    /// (757 to 868), which, before the native check (step 2), compares the
    /// sender with the current focus: a sender UI Automation counts as the
    /// current focus is redirected to it, judged with the focus's window and
    /// its keyboard focus read from the focus's element, without the native
    /// check. The current focus is this reading thread's own last read while
    /// that read is current for the opening, `queue`: nothing moved away
    /// from the application since that read's event was queued
    /// ([`QueuePlace::current_since`]; `docs/design/ownership.md`). Its drop while a focus is pending is made as it
    /// arrives (`Readers::focus_pending`).
    ///
    /// NVDA's duplicate test (step 3) is not made here: a focus that passes
    /// the other checks is read in full, and Core, which holds the focus,
    /// judges whether it repeats it (`docs/design/ownership.md`).
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
    /// focus being dropped. A menu opening's comparison is its read's first
    /// call, made within the budget.
    ///
    /// Answers the window the focus is judged with, and whether the read's
    /// walk is to read the sender's keyboard focus, step 6, after the
    /// earlier element's.
    fn judge_before_reading(
        &self,
        held: &Cached<ListenerEvent>,
        (sender, runtime_id): (&ListenerEvent, &[i32]),
        (kind, queue): (FocusEvent, QueuePlace),
        budget: &Budget,
    ) -> Result<(isize, bool), &'static str> {
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
                // one that fails drops the event. The focus object here is
                // the thread's own last read while it is current; with none,
                // nothing is compared.
                let current = self
                    .last
                    .as_ref()
                    .filter(|last| queue.current_since(last.place));
                let same = current.filter(|_| !out_of_time()).map(|last| {
                    budget.call("the comparison with the current focus");
                    self.uia.compare_elements(element, &last.element)
                });
                if tracing::enabled!(tracing::Level::DEBUG) {
                    Self::log_sender(sender, runtime_id, same.as_ref());
                }
                match (&same, current) {
                    (Some(Err(_)), _) => {
                        return Err(COMPARISON_FAILED);
                    }
                    (Some(Ok(true)), Some(last)) => (last.window, &last.element),
                    _ => (
                        self.native_window(held, sender, budget, same.is_some())?,
                        element,
                    ),
                }
            }
            // Only focuses and menu openings are read; a selection or a
            // notification is checked ([`Self::check_fact`]).
            FocusEvent::Focus | FocusEvent::Selection | FocusEvent::Notification => {
                if tracing::enabled!(tracing::Level::DEBUG) {
                    Self::log_sender(sender, runtime_id, None);
                }
                (self.native_window(held, sender, budget, false)?, element)
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
            Window::new(window),
            &windows,
            &RequestedEvents::default(),
        ) {
            tracing::debug!(
                window,
                foreground = windows.foreground,
                "UIA focus not related to the foreground window"
            );
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
        // it is: dropped before it is read. A local read of the window's
        // property.
        if window_belongs_to_hidden_frame(if hwnd != 0 { hwnd } else { window }) {
            return Err(HIDDEN_FRAME);
        }
        // 6. NVDA's last check before it queues the focus,
        // `shouldAllowUIAFocusEvent` (`UIAHandler/__init__.py` 948,
        // `NVDAObjects/UIA/__init__.py` 1632): the sender's keyboard focus,
        // read live with a plain property read (`CurrentHasKeyboardFocus`
        // on the event's element, client side, or on the focus's for a
        // menu opening redirected to it), a failed read letting the event
        // through. It is a plain read, not one inside a remote operation,
        // because Windows Terminal's second identity of its terminal
        // answers the two differently, and only the plain read drops it as
        // NVDA does (`docs/design/focus-pipeline.md`, section 16.8). The
        // sender's is made by the read's classic walk
        // (`FocusQuery::require_focus`), after the keyboard focus of the
        // element held under the same runtime id, as NVDA reads its last
        // queued focus's before the sender's (`UIAHandler/__init__.py` 902
        // to 917 and 948), so a focus only being lost cannot read as a new
        // element (`docs/design/focus-pipeline-rework-remaining.md`, "The
        // keep-id evidence, read in NVDA's order"); the walk makes it once,
        // since a focus that moves on while it is read is followed by the
        // focus event of where it went
        // (`docs/design/listener-without-connections.md`, section 3, step
        // 8).
        //
        // Out of time, the live read is not made, and the sender's keyboard
        // focus is taken from the event's cache instead, as it stood when
        // the event was raised: a sender that did not have it then is
        // dropped. A live read that fails, by UIA's transaction timeout or
        // otherwise, lets the focus through, as NVDA's `except COMError:
        // return True` does (Dickson, 2026-10-21). A menu opening redirected
        // to the current focus is judged by the focus's element, whose
        // keyboard focus no cache of this event holds, read here.
        let sender_is_subject = std::ptr::eq(subject, element);
        if out_of_time() {
            if sender_is_subject && sender.lacked_keyboard_focus() {
                return Err(UNFOCUSED_OUT_OF_TIME);
            }
            return Ok((window, false));
        }
        if sender_is_subject {
            return Ok((window, true));
        }
        budget.call("the keyboard focus");
        if let Ok(false) = subject.has_keyboard_focus() {
            return Err(NOT_FOCUSED);
        }
        Ok((window, false))
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
    /// local calls only, so the two processes agree on it. The rest of `isUIAWindow`, the probe and the post-probe
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
            let nearest = nearest
                .map_err(|_| nearest_timeout_reason(started.elapsed()))?
                .ok_or(NO_WINDOW)?;
            self.window.learn(nearest);
            nearest
        } else {
            hwnd
        };
        if skipped_by_class(
            &verbatim_win32::window::window_classes(nearest),
            self.forced_uia.load(Ordering::Relaxed),
        ) {
            return Err(MSAA_CLASS);
        }
        Ok(nearest)
    }

    /// Logs, at debug, what section 16.3 of `docs/design/focus-pipeline.md`
    /// asks to see of each read: the sender's runtime id, class name,
    /// framework, and own native window handle, `hwnd`, and, for a menu
    /// opening compared with the thread's current last read, whether UI
    /// Automation counts it as the same element (`same_as_current`). All
    /// but the comparison come from the event's cache, as NVDA's debug
    /// strings do; nothing is read live for the log.
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
    /// Judges and reads one event, which `meta` describes, queued at
    /// `queue`, in the read `watched`, which it tells the window it is of
    /// once the read finds it.
    fn read(
        &self,
        reader: Option<&mut Self::Reader>,
        meta: Meta,
        job: Self::Job,
        watched_and_queue: (&WatchedRead, QueuePlace),
    ) -> Judged;
    /// The window `job`'s event is of as far as the event says, for the
    /// watchdog's judgement of whether the user has moved on from it
    /// ([`WatchedRead`]): `Some(0)` for an event judged so whose window
    /// its read is still to learn, and `None`, the default, for an event
    /// that is never judged so.
    fn window(&self, _job: &Self::Job) -> Option<isize> {
        None
    }
    /// The schedule answered that the thread's last read was abandoned:
    /// the reader forgets it, a focus Core never got. Nothing, the
    /// default.
    fn forget_last(&self, _reader: &mut Self::Reader) {}
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
    /// What answers an event's marker, made with the event
    /// ([`Readers::push`]) and kept by the readers' shell beside the
    /// schedule, never in it, until the event's outcome consumes it; one
    /// dropped unanswered answers the marker itself, so no path that loses
    /// an event leaves its outpost waiting on the marker.
    type Owed: Send + 'static;
    /// Sends what became of an event of `pid`'s through `owed`, the event's
    /// own answer. Called under the readers' lock, so what one
    /// application's events became is sent in the order they arrived.
    fn outcome(&self, pid: Pid, meta: Meta, owed: Self::Owed, judged: Judged);
    /// `window`'s top-level window, 0 for none: a local call
    /// ([`top_level_of`], the default), made outside the readers' lock.
    fn top_level(&self, window: isize) -> isize {
        top_level_of(window)
    }
}

/// What the readers' shell keeps under its lock beside the schedule: the
/// threads, one record for each application the schedule holds, the idle
/// waiters' closures, and the answer owed for each event the schedule
/// holds, by marker.
struct ReadersState<J, O> {
    schedule: Schedule<J>,
    owed: HashMap<u64, O>,
    threads: Vec<JoinHandle<()>>,
    apps: HashMap<Pid, AppShell>,
    idle_waiters: HashMap<u64, IdleWaiter>,
    next_waiter: u64,
}

/// What the shell keeps for one application the schedule holds: made when
/// a thread is put in charge of its events, and dropped whole when the
/// schedule lets the application go ([`ScheduleEffect::Forget`]), which it
/// does only once no thread is in charge of its events or abandoned to a
/// read of them, so no thread waits on its condition variable or returns
/// to its read; in debug builds every step checks that its keys are the
/// schedule's applications.
#[derive(Default)]
struct AppShell {
    /// What the application's thread in charge waits on for its next event.
    wake: Arc<Condvar>,
    /// Its read under way, until the read returns or is abandoned.
    watched: Option<WatchedRead>,
}

/// What [`Readers::when_idle`] runs.
pub(crate) type IdleWaiter = Box<dyn FnOnce() + Send>;

struct Shared<R: Reading> {
    reading: R,
    /// The schedule's clock: the system's, unless a test sets its own
    /// ([`Readers::with_clock`]).
    clock: Clock,
    /// The time the schedule's microseconds count from.
    origin: Instant,
    state: Mutex<ReadersState<R::Job, R::Owed>>,
    /// The watchdog's alarm: its timer set, under the readers' lock, as the
    /// schedule says, to the soonest time anything is due, which wakes no
    /// thread, and cancelled once nothing is; its event set at the
    /// shutdown (`docs/design/wake-ups.md`, item 3).
    alarm: Alarm,
    /// The failed cached reads the reading threads have made
    /// (`verbatim_uia::calls`).
    failed_reads: AtomicU32,
}

impl<R: Reading> Shared<R> {
    fn lock(&self) -> MutexGuard<'_, ReadersState<R::Job, R::Owed>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `at` in the schedule's microseconds.
    fn micros(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.origin).as_micros()).unwrap_or(u64::MAX)
    }

    /// The system's time at which the schedule's clock reaches `us`, for
    /// the alarm: as long from now as the schedule's clock has left.
    fn alarm_at(&self, us: u64) -> Instant {
        Instant::now() + Duration::from_micros(us.saturating_sub(self.now()))
    }

    /// Now, in the schedule's microseconds.
    fn now(&self) -> u64 {
        self.micros((self.clock)())
    }
}

/// What a reading thread is, for the effects that concern it.
struct ThreadSide<'a, R: Reading> {
    reader: &'a mut Option<R::Reader>,
    generation: &'a mut u64,
}

/// Steps the schedule with `input` and applies its effects in order,
/// under the readers' lock: for a reading thread, `thread` is that thread,
/// and the event it is to read next is returned. A thread that cannot be
/// started, and the schedule's asks, are set aside: once every other effect
/// of the step is applied, each failed start is stepped, then each ask is
/// answered in order, each as a step of its own, which handles its own
/// effects and asks the same way, all before the lock is let go. So every
/// outcome of a step is sent before a failed start's or an answer's step
/// runs, an idle waiter that step runs comes after them, and what is owed
/// matches the schedule at each step's end.
fn step<R: Reading>(
    shared: &Arc<Shared<R>>,
    state: &mut ReadersState<R::Job, R::Owed>,
    input: ScheduleInput<R::Job>,
    mut thread: Option<ThreadSide<'_, R>>,
) -> Option<(Meta, R::Job, WatchedRead, QueuePlace)> {
    let effects = state.schedule.step(input, shared.now());
    let mut next = None;
    let mut asks = Vec::new();
    let mut failed = Vec::new();
    for effect in effects {
        match effect {
            ScheduleEffect::Outcome { pid, meta, judged } => {
                if let Some(owed) = state.owed.remove(&meta.marker) {
                    shared.reading.outcome(pid, meta, owed, judged);
                } else {
                    tracing::error!(%pid, marker = meta.marker, "an outcome for a marker nothing is owed for");
                    debug_assert!(false, "an outcome for marker {}, owed nothing", meta.marker);
                }
            }
            ScheduleEffect::Spawn {
                pid,
                generation,
                reuse: true,
            } => {
                if let Some(thread) = thread.as_mut() {
                    *thread.generation = generation;
                }
                state.apps.entry(pid).or_default();
            }
            ScheduleEffect::Spawn {
                pid,
                generation,
                reuse: false,
            } => failed.extend(spawn(shared, state, pid, generation)),
            ScheduleEffect::Wake(Waking::Thread(pid)) => {
                if let Some(app) = state.apps.get(&pid) {
                    app.wake.notify_one();
                }
            }
            ScheduleEffect::Wake(Waking::All) => {
                for app in state.apps.values() {
                    app.wake.notify_all();
                }
            }
            ScheduleEffect::Unwatch { pid } => {
                let _ = state.apps.get_mut(&pid).and_then(|app| app.watched.take());
            }
            ScheduleEffect::Forget { pid } => drop(state.apps.remove(&pid)),
            ScheduleEffect::ForgetLast => {
                if let Some(ThreadSide {
                    reader: Some(reader),
                    ..
                }) = thread.as_mut()
                {
                    shared.reading.forget_last(reader);
                }
            }
            ScheduleEffect::Alarm(AlarmChange::Set(Some(due))) => {
                shared.alarm.set(shared.alarm_at(due));
            }
            ScheduleEffect::Alarm(AlarmChange::Set(None)) => shared.alarm.cancel(),
            ScheduleEffect::Alarm(AlarmChange::Now) => shared.alarm.wake(),
            ScheduleEffect::RunIdle(waiter) => {
                if let Some(waiter) = state.idle_waiters.remove(&waiter) {
                    waiter();
                }
            }
            ScheduleEffect::Job(job) => {
                // Only a judged read is told its window or its bound, so
                // only it is given what to do then.
                let (pid, request) = (job.pid, job.request);
                let watched = WatchedRead::new(
                    request,
                    job.window.known().map_or(0, Window::get),
                    if job.window == RuleWindow::Never {
                        (None, None)
                    } else {
                        (
                            Some(on_learn(shared, pid, request)),
                            Some(on_arm(shared, pid, request)),
                        )
                    },
                    job.after_elsewhere,
                );
                state.apps.entry(pid).or_default().watched = Some(watched.clone());
                next = Some((job.meta, job.work, watched, job.queue));
            }
            ask @ (ScheduleEffect::AskMovedOn { .. } | ScheduleEffect::Publish { .. }) => {
                asks.push(ask);
            }
        }
    }
    if cfg!(debug_assertions) {
        check_tables(state);
    }
    for failure in failed {
        let more = step(shared, state, failure, None);
        debug_assert!(more.is_none(), "a failed start's step gives no job");
    }
    for ask in asks {
        let answer = answer(shared, state, &ask);
        let more = step(shared, state, answer, None);
        debug_assert!(more.is_none(), "an answer's step gives no job");
    }
    next
}

/// The shell's tables match the schedule once a step's effects are
/// applied, checked in debug builds: every transition that ends an event
/// answered its marker, so what is owed is exactly what the schedule still
/// holds, and there is one record for each application the schedule holds.
fn check_tables<J, O>(state: &ReadersState<J, O>) {
    let mut owed: Vec<u64> = state.owed.keys().copied().collect();
    owed.sort_unstable();
    debug_assert_eq!(owed, state.schedule.markers(), "the owed markers");
    let mut apps: Vec<Pid> = state.apps.keys().copied().collect();
    apps.sort_unstable();
    debug_assert_eq!(
        apps,
        state.schedule.held().collect::<Vec<_>>(),
        "the applications' records"
    );
}

/// The answer to the schedule's ask `ask`, made under the readers' lock
/// with local calls only: whether the user has moved on from a read's
/// window ([`Reading::moved_on`]), or the read published from its
/// progress, found by its request, not by its application's current entry.
fn answer<R: Reading>(
    shared: &Shared<R>,
    state: &ReadersState<R::Job, R::Owed>,
    ask: &ScheduleEffect<R::Job>,
) -> ScheduleInput<R::Job> {
    match *ask {
        ScheduleEffect::AskMovedOn {
            pid,
            request,
            window,
        } => ScheduleInput::MovedOn {
            pid,
            request,
            moved_on: shared.reading.moved_on(window.get()),
        },
        ScheduleEffect::Publish {
            pid,
            request,
            then_abandon,
        } => {
            let watched = state
                .apps
                .get(&pid)
                .and_then(|app| app.watched.as_ref())
                .filter(|watched| watched.request() == Some(request));
            ScheduleInput::Published {
                pid,
                request,
                read: watched.and_then(WatchedRead::prepare_publish).map(Box::new),
                then_abandon,
            }
        }
        _ => unreachable!("only an ask is answered"),
    }
}

/// The reading threads, one per application, and their watchdog: the
/// shell of the pure [`Schedule`].
pub(crate) struct Readers<R: Reading> {
    shared: Arc<Shared<R>>,
    watchdog: Mutex<Option<JoinHandle<()>>>,
}

impl<R: Reading> Readers<R> {
    /// Readers doing `reading`, each read abandoned past `deadline`, or
    /// once the user has moved on from its window ([`Reading::moved_on`]),
    /// judged at [`MOVED_ON_GRACE`] and every [`MOVED_ON_GRACE`] after.
    ///
    /// # Panics
    ///
    /// Panics if the watchdog's thread cannot be spawned, which means the
    /// process is out of OS thread resources.
    pub(crate) fn new(reading: R, deadline: Duration) -> Self {
        Self::with_clock(reading, deadline, Arc::new(Instant::now))
    }

    /// [`new`](Self::new)'s readers, whose schedule reads now from `clock`:
    /// a test whose clock stands still has nothing come due but what it
    /// moves the clock to.
    fn with_clock(reading: R, deadline: Duration, clock: Clock) -> Self {
        let shared = Arc::new(Shared {
            reading,
            origin: clock(),
            clock,
            state: Mutex::new(ReadersState {
                schedule: Schedule::new(deadline),
                owed: HashMap::new(),
                threads: Vec::new(),
                apps: HashMap::new(),
                idle_waiters: HashMap::new(),
                next_waiter: 0,
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
    /// waiting ([`READ_DEADLINE`]). `owed` answers its marker: kept until
    /// its outcome. Never waits on a read.
    pub(crate) fn push(&self, pid: Pid, meta: Meta, job: R::Job, owed: R::Owed) {
        let shared = &self.shared;
        let kind = shared.reading.kind(&job);
        let key = shared
            .reading
            .key(&job)
            .filter(|(key_kind, _)| *key_kind == kind)
            .map(|(_, key)| key.to_vec());
        let window = shared.reading.window(&job);
        let mut state = shared.lock();
        state.owed.insert(meta.marker, owed);
        step(
            shared,
            &mut state,
            ScheduleInput::Push {
                pid,
                meta,
                job,
                kind,
                key,
                window: RuleWindow::from_cached(window),
            },
            None,
        );
    }

    /// Whether a focus event is waiting for its read or being read, for a
    /// menu opening of `pid`'s: NVDA drops a menu opening that arrives
    /// while a focus is pending (`isPendingEvents("gainFocus")`,
    /// `UIAHandler/__init__.py` 772 to 777), and here a focus still to be
    /// judged stands for one NVDA has queued and not yet run
    /// ([`Schedule::focus_pending`]).
    pub(crate) fn focus_pending(&self, pid: Option<Pid>) -> bool {
        self.shared.lock().schedule.focus_pending(pid)
    }

    /// Ends `pid`'s reading thread, the listener having learned that its
    /// outpost has ended: the thread reads the events already waiting, then
    /// releases its reader and ends, unless another event comes first.
    /// Never waits on a read.
    pub(crate) fn outpost_ended(&self, pid: Pid) {
        let mut state = self.shared.lock();
        step(
            &self.shared,
            &mut state,
            ScheduleInput::OutpostEnded { pid },
            None,
        );
    }

    /// Runs `waiter` once no event queued before this call is waiting for
    /// its read or being read, at once if none is, under the readers'
    /// lock, behind the outcome of every one of them: the idle barrier's
    /// answer goes behind them (`docs/design/communication.md`, section
    /// 6.7). Events queued after this call are not waited for. Never waits
    /// on a read. Once the readers are stopping, `waiter` is dropped unrun.
    pub(crate) fn when_idle(&self, waiter: IdleWaiter) {
        let mut state = self.shared.lock();
        if state.schedule.stopping() {
            return;
        }
        state.next_waiter += 1;
        let id = state.next_waiter;
        state.idle_waiters.insert(id, waiter);
        step(
            &self.shared,
            &mut state,
            ScheduleInput::WhenIdle { waiter: id },
            None,
        );
    }

    /// A foreground event of window `window` reached the listener's hook:
    /// it moves away from each application whose last read, or read under
    /// way, is of another top-level window, for menu currentness, and
    /// judges nothing else. Never waits on a read.
    pub(crate) fn foreground_changed(&self, window: isize) {
        // A local call, outside the lock.
        let top_level = self.shared.reading.top_level(window);
        let mut state = self.shared.lock();
        let running_tops = state
            .apps
            .iter()
            .filter_map(|(pid, app)| {
                let watched = app.watched.as_ref()?;
                Some((*pid, Window::new(watched.top_level())))
            })
            .collect();
        step(
            &self.shared,
            &mut state,
            ScheduleInput::Foreground {
                top_level: Window::new(top_level),
                running_tops,
            },
            None,
        );
    }

    /// The listener's `WinEvent` thread forwarded an MSAA focus of `pid`'s:
    /// it moves away from every other application, for menu currentness.
    pub(crate) fn msaa_focus(&self, pid: Pid) {
        let mut state = self.shared.lock();
        step(
            &self.shared,
            &mut state,
            ScheduleInput::MsaaFocus { pid },
            None,
        );
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
            step(&self.shared, &mut state, ScheduleInput::Stop, None);
            state.idle_waiters.clear();
            std::mem::take(&mut state.threads)
        };
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

/// A reading thread's name.
const READING_THREAD: &str = "verbatim-listener-read";

/// The error the system refuses a thread with when it is out of memory,
/// `ERROR_NOT_ENOUGH_MEMORY`.
const ERROR_NOT_ENOUGH_MEMORY: i32 = 8;

/// Starts a reading thread in charge of `pid`'s events as `generation`:
/// with no thread to be had, the failure, for the caller to step the
/// schedule with, which drops its waiting events.
fn spawn<R: Reading>(
    shared: &Arc<Shared<R>>,
    state: &mut ReadersState<R::Job, R::Owed>,
    pid: Pid,
    generation: u64,
) -> Option<ScheduleInput<R::Job>> {
    state.apps.entry(pid).or_default();
    let reading = Arc::clone(shared);
    // A thread the system cannot start, as one out of memory refuses it.
    #[cfg(test)]
    let refused = verbatim_faults::hit(
        verbatim_faults::Fault::ThreadStart,
        &(READING_THREAD, pid, generation),
    );
    #[cfg(not(test))]
    let refused = false;
    let spawned = if refused {
        Err(std::io::Error::from_raw_os_error(ERROR_NOT_ENOUGH_MEMORY))
    } else {
        thread::Builder::new()
            .name(READING_THREAD.to_owned())
            .spawn(move || run(&reading, pid, generation))
    };
    match spawned {
        Ok(thread) => {
            state.threads.retain(|thread| !thread.is_finished());
            state.threads.push(thread);
            None
        }
        Err(error) => {
            tracing::error!(%pid, %error, "a reading thread could not be started");
            Some(ScheduleInput::SpawnFailed { pid, generation })
        }
    }
}

/// A reading thread: reads `pid`'s events in order while it is the thread
/// in charge of them, then releases its reader and ends.
fn run<R: Reading>(shared: &Arc<Shared<R>>, pid: Pid, generation: u64) {
    let mut generation = generation;
    let mut reader = shared.reading.reader(pid);
    while let Some((mut meta, job, window, queue)) =
        next_job(shared, pid, &mut reader, &mut generation)
    {
        let judged = shared
            .reading
            .read(reader.as_mut(), meta, job, (&window, queue));
        // The reading thread's end, for the latency ledger's split of the
        // listener's handling from its queue and its write.
        meta.timing.listener_handled_at_us = now_us();
        shared
            .failed_reads
            .fetch_add(verbatim_uia::calls::take_failed_reads(), Ordering::Relaxed);
        let top = window.top_level();
        let request = window
            .request()
            .expect("a job's read is made with its request");
        let mut state = shared.lock();
        if let Some(app) = state.apps.get_mut(&pid)
            && app
                .watched
                .as_ref()
                .is_some_and(|watched| watched.same(&window))
        {
            app.watched = None;
        }
        step(
            shared,
            &mut state,
            ScheduleInput::Returned {
                pid,
                request,
                meta,
                judged,
                top: Window::new(top),
            },
            Some(ThreadSide {
                reader: &mut reader,
                generation: &mut generation,
            }),
        );
        if !state.schedule.in_charge(pid, generation) {
            break;
        }
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
    reader: &mut Option<R::Reader>,
    generation: &mut u64,
) -> Option<(Meta, R::Job, WatchedRead, QueuePlace)> {
    let mut state = shared.lock();
    loop {
        let input = ScheduleInput::Next {
            pid,
            generation: *generation,
        };
        let side = ThreadSide {
            reader: &mut *reader,
            generation: &mut *generation,
        };
        if let Some(job) = step(shared, &mut state, input, Some(side)) {
            return Some(job);
        }
        if !state.schedule.in_charge(pid, *generation) {
            return None;
        }
        let wake = Arc::clone(&state.apps.entry(pid).or_default().wake);
        state = wake.wait(state).unwrap_or_else(PoisonError::into_inner);
    }
}

/// What `pid`'s read `request` does once it learns its window
/// ([`WatchedRead::learn`]): steps the schedule, whose work in flight then
/// judges the moved-on rule at the grace's end, or at once when the grace
/// has passed. One uncontended lock and no wake. Never waits on a read.
fn on_learn<R: Reading>(shared: &Arc<Shared<R>>, pid: Pid, request: Request) -> OnLearn {
    let shared = Arc::downgrade(shared);
    Box::new(move |window: isize| {
        let (Some(shared), Some(window)) = (shared.upgrade(), Window::new(window)) else {
            return;
        };

        let mut state = shared.lock();
        step(
            &shared,
            &mut state,
            ScheduleInput::Learned {
                pid,
                request,
                window,
            },
            None,
        );
    })
}

/// What `pid`'s read `request` does once it has passed NVDA's checks and
/// its bound is known ([`WatchedRead::arm`]): steps the schedule with the
/// bound, which sets the watchdog's alarm by it, a system call that wakes
/// no thread, so the read is published at its bound should a call of its
/// walk still be under way then.
fn on_arm<R: Reading>(shared: &Arc<Shared<R>>, pid: Pid, request: Request) -> OnArm {
    let shared = Arc::downgrade(shared);
    Box::new(move |bound_at: Instant| {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let mut state = shared.lock();
        let bound = shared.micros(bound_at);
        step(
            &shared,
            &mut state,
            ScheduleInput::Armed {
                pid,
                request,
                bound,
            },
            None,
        );
    })
}

/// The watchdog: each time its alarm goes off, steps the schedule, whose
/// reads under way judge what is due (a deadline, a bound, or the
/// half-second judgment of whether the user has moved on) and whose events
/// waiting past their application's bound at the abandoned limit are
/// dropped; the schedule asks what it needs, answered in the same step
/// ([`step`]). The watchdog sleeps on its alarm ([`Shared::alarm`]), which
/// the schedule sets after every step to the soonest time anything is due:
/// so a read that returns before its grace's end costs the watchdog no
/// wake-up.
fn watchdog<R: Reading>(shared: &Arc<Shared<R>>) {
    loop {
        let mut state = shared.lock();
        if state.schedule.stopping() {
            return;
        }
        step(shared, &mut state, ScheduleInput::Tick, None);
        // The alarm keeps a time set, or an event, from now until the wait
        // takes it, so nothing set once the lock is let go is lost.
        drop(state);
        shared.alarm.wait();
    }
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
    set_up: SetUpReader,
    outcomes: Mutex<std::sync::mpsc::Sender<(u64, Judged)>>,
}

impl Reading for ThreadReading {
    /// The event's element and its own window handle from the event's
    /// cache.
    type Job = (verbatim_uia::Agile<ListenerEvent>, isize);
    type Reader = FocusReader;

    fn reader(&self, _: Pid) -> Option<FocusReader> {
        let mut reader = FocusReader::new().ok()?;
        (self.set_up)(&mut reader);
        Some(reader)
    }

    fn read(
        &self,
        reader: Option<&mut FocusReader>,
        meta: Meta,
        (element, _): Self::Job,
        (watched, queue): (&WatchedRead, QueuePlace),
    ) -> Judged {
        let Some(reader) = reader else {
            return Judged::Dropped("its reading thread has no UIA client");
        };
        match element.resolve() {
            Ok(sender) => reader.judge_watched(
                &sender,
                (FocusEvent::Focus, None),
                (arrival_of(meta, queue), watched.clone()),
            ),
            Err(_) => Judged::Dropped("its element could not be resolved"),
        }
    }

    fn window(&self, (_, hwnd): &Self::Job) -> Option<isize> {
        Some(*hwnd)
    }

    fn forget_last(&self, reader: &mut FocusReader) {
        reader.forget_last();
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

    /// Nothing is owed: every outcome is sent to the test's receiver.
    type Owed = ();

    fn outcome(&self, _: Pid, meta: Meta, (): (), judged: Judged) {
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
                set_up,
                outcomes: Mutex::new(outcomes),
            },
            READ_DEADLINE,
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
            timing: verbatim_model::protocol::EventTiming {
                observed_at_us,
                ..verbatim_model::protocol::EventTiming::default()
            },
        };
        self.readers
            .push(pid, meta, (sender.agile()?, sender.read().role.hwnd), ());
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

    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread::ThreadId;
    use verbatim_reader_rules::MOVED_ON_GRACE;

    use verbatim_listener_logic::focus::{MOVED_ON, NOT_IN_TIME};

    use super::*;
    use verbatim_model::protocol::{EventTiming, Responder};

    /// The window every stand-in event's sender is, unless a test says
    /// its senders are not windows.
    const WINDOW: isize = 0x5150;

    /// The nearest window a stand-in read finds, when a test has it find
    /// one.
    const NEAREST: isize = 0x7170;

    /// A stand-in reading: each job is held until its gate opens, if it
    /// has one, and every outcome is sent to the test with the thread that
    /// read the event and the reason, as is each thread that ends. Every
    /// event's sender is window `hwnd`, 0 for a
    /// sender that is not a window; each read finds the sender's nearest
    /// window to be `nearest` before it is held, 0 for none. Whether the
    /// user has moved on from a window is what `moved_on` says, each
    /// judgement sent to the test. Each thread's reader remembers the marker
    /// of the last event it read, which each read sends the test as it
    /// starts and which the schedule's answer to an abandoned read's return
    /// clears, as the listener's reader forgets its last focus. Each read
    /// is dropped as "read", unless `reads` says it is read; each fact sent
    /// is sent to the test; and a window is its own top-level window.
    struct Script {
        outcomes: Mutex<Sender<(Pid, u64, &'static str)>>,
        read_on: Mutex<Sender<(u64, ThreadId)>>,
        finished: Mutex<Sender<ThreadId>>,
        moved_on: AtomicBool,
        /// Whether its events are notifications rather than focuses.
        notifications: AtomicBool,
        asked: Mutex<Sender<isize>>,
        hwnd: isize,
        nearest: isize,
        lasts: Mutex<Sender<(u64, Option<u64>)>>,
        /// Whether its reads read their focus, `None`, or drop it, and for
        /// what reason.
        reads: Mutex<Option<&'static str>>,
        facts: Mutex<Sender<DeliveredFact>>,
        /// Each outcome sent and each judgement asked, in the order made.
        order: Mutex<Vec<String>>,
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
            ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
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
        /// The thread, and the marker of the last event it read.
        type Reader = (ThreadId, Option<u64>);

        fn reader(&self, _: Pid) -> Option<(ThreadId, Option<u64>)> {
            Some((thread::current().id(), None))
        }

        fn forget_last(&self, (_, last): &mut (ThreadId, Option<u64>)) {
            *last = None;
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
            reader: Option<&mut (ThreadId, Option<u64>)>,
            _: Meta,
            (marker, gate, _): Self::Job,
            (watched, _): (&WatchedRead, QueuePlace),
        ) -> Judged {
            if self.nearest != 0 {
                watched.learn(self.nearest);
            }
            if let Some((thread, last)) = reader {
                let _ = self
                    .lasts
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send((marker, last.replace(marker)));
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

        fn moved_on(&self, window: isize) -> bool {
            // Read before the judgement is sent, so a test that changes the
            // answer once it has seen a judgement changes only later ones.
            let moved_on = self.moved_on.load(Ordering::SeqCst);
            self.order
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("asked {window}"));
            let _ = self
                .asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(window);
            moved_on
        }

        fn finish(&self, reader: Option<(ThreadId, Option<u64>)>) {
            if let Some((thread, _)) = reader {
                let _ = self
                    .finished
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send(thread);
            }
        }

        /// The answer to the event's marker, sent as a fact to the test.
        type Owed = Responder<DeliveredFact>;

        fn outcome(&self, pid: Pid, meta: Meta, owed: Responder<DeliveredFact>, judged: Judged) {
            let reason = match judged {
                Judged::Dropped(reason) => reason,
                Judged::Read(_) => "it was read",
                ref other @ Judged::Passed(_) => {
                    panic!("the stand-in neither drops nor reads: {other:?}")
                }
            };
            owed.answer(judged.into_fact(meta.marker, meta.kind));
            self.order
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("outcome {} {}", pid.0, meta.marker));
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
        finished: Receiver<ThreadId>,
        asked: Receiver<isize>,
        facts: Receiver<DeliveredFact>,
        lasts: Receiver<(u64, Option<u64>)>,
    }

    impl Fixture {
        /// Queues `job`, of `pid`'s event `meta`, its marker answered as
        /// the listener answers one: by its outcome, or, dropped unanswered,
        /// as a drop that leaves its application's gap record to its bound.
        fn push(&self, pid: Pid, meta: Meta, job: <Script as Reading>::Job) {
            let facts = self
                .readers
                .shared
                .reading
                .facts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let marker = meta.marker;
            let owed = Responder::new(move |fact: Option<DeliveredFact>| {
                let _ = facts.send(fact.unwrap_or(DeliveredFact::UiaFocusDropped {
                    marker,
                    focus: true,
                    no_window: false,
                }));
            });
            self.readers.push(pid, meta, job, owed);
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

    /// Readers abandoning a read past `deadline`, or once the user has
    /// moved on, judged at [`MOVED_ON_GRACE`] and every grace after, whose
    /// events' senders are windows that their reads find no other window
    /// for, and a user who has not moved on.
    fn fixture(deadline: Duration) -> Fixture {
        fixture_of(deadline, (WINDOW, 0))
    }

    /// Readers abandoning a read past `deadline`, or once the user has
    /// moved on, whose events' senders are window `hwnd`, 0 for senders
    /// that are not windows, and whose reads find the sender's nearest
    /// window to be `nearest`, 0 for none.
    fn fixture_of(deadline: Duration, (hwnd, nearest): (isize, isize)) -> Fixture {
        fixture_by(deadline, (hwnd, nearest), Arc::new(Instant::now))
    }

    /// [`fixture_of`]'s readers, whose schedule's clock is `clock`.
    fn fixture_by(deadline: Duration, (hwnd, nearest): (isize, isize), clock: Clock) -> Fixture {
        let (outcomes_tx, outcomes) = mpsc::channel();
        let (read_tx, read_on) = mpsc::channel();
        let (finished_tx, finished) = mpsc::channel();
        let (asked_tx, asked) = mpsc::channel();
        let (facts_tx, facts) = mpsc::channel();
        let (lasts_tx, lasts) = mpsc::channel();
        Fixture {
            readers: Readers::with_clock(
                Script {
                    outcomes: Mutex::new(outcomes_tx),
                    read_on: Mutex::new(read_tx),
                    finished: Mutex::new(finished_tx),
                    moved_on: AtomicBool::new(false),
                    notifications: AtomicBool::new(false),
                    asked: Mutex::new(asked_tx),
                    hwnd,
                    nearest,
                    lasts: Mutex::new(lasts_tx),
                    reads: Mutex::new(Some("read")),
                    facts: Mutex::new(facts_tx),
                    order: Mutex::new(Vec::new()),
                },
                deadline,
                clock,
            ),
            outcomes,
            read_on,
            finished,
            asked,
            facts,
            lasts,
        }
    }

    /// A schedule's clock that stands still until the test moves it, in
    /// microseconds from its start.
    struct Still {
        start: Instant,
        at: Arc<AtomicU64>,
    }

    impl Still {
        /// The clock, for the readers.
        fn clock(&self) -> Clock {
            let (start, at) = (self.start, Arc::clone(&self.at));
            Arc::new(move || start + Duration::from_micros(at.load(Ordering::SeqCst)))
        }
    }

    /// [`fixture`]'s readers, whose schedule's clock stands at its start
    /// until [`tick_at`] moves it: nothing comes due but what a test ticks
    /// by hand, and the watchdog's own ticks judge nothing more.
    fn still_fixture(deadline: Duration) -> (Fixture, Still) {
        let still = Still {
            start: Instant::now(),
            at: Arc::new(AtomicU64::new(0)),
        };
        (fixture_by(deadline, (WINDOW, 0), still.clock()), still)
    }

    /// Moves `still` to `at` microseconds and steps the watchdog's tick
    /// there by hand, both under `state`, the readers' lock, which the
    /// watchdog's own tick waits for.
    fn tick_at(
        fixture: &Fixture,
        still: &Still,
        state: &mut ReadersState<<Script as Reading>::Job, <Script as Reading>::Owed>,
        at: u64,
    ) {
        still.at.store(at, Ordering::SeqCst);
        step(&fixture.readers.shared, state, ScheduleInput::Tick, None);
    }

    /// [`MOVED_ON_GRACE`] in microseconds.
    fn grace_us() -> u64 {
        u64::try_from(MOVED_ON_GRACE.as_micros()).expect("the grace in microseconds")
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

    /// A focus whose nearest window's class is on NVDA's MSAA class list,
    /// and not under a shell window, is skipped with no call, by the class
    /// verdict the outposts' arbitrators give, and its drop carries no fact
    /// for the gap record, which the outpost closes by its own reports; one
    /// on no list, or under a shell
    /// window, is read, and so is any focus while a test forces UIA.
    #[test]
    fn a_focus_whose_windows_class_is_on_the_msaa_list_is_skipped() {
        let edit = WindowClasses::new("Edit", "Notepad");
        assert!(skipped_by_class(&edit, false));
        assert!(!skipped_by_class(&edit, true), "a test forcing UIA");
        assert!(!skipped_by_class(
            &WindowClasses::new("Windows.UI.Input.InputSite.WindowClass", "Notepad"),
            false
        ));
        assert!(!skipped_by_class(
            &WindowClasses::new("Edit", "Shell_TrayWnd"),
            false
        ));
        assert_eq!(
            Judged::Dropped(MSAA_CLASS).into_fact(9, FocusEvent::Focus),
            DeliveredFact::UiaFocusDropped {
                marker: 9,
                focus: true,
                no_window: false,
            }
        );
    }

    #[test]
    fn an_applications_events_are_read_in_the_order_they_arrived_on_one_thread() {
        let fixture = fixture(Duration::from_secs(60));
        for marker in 1..=3 {
            fixture.push(Pid(5), meta(marker), (marker, None, Vec::new()));
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
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
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
        fixture.push(Pid(5), meta(1), (1, Some(gate), vec![42, 1]));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        // Event 1 is being read, so it stays; 2 waits and is replaced by 4,
        // another element's 3 stays.
        fixture.push(Pid(5), meta(2), (2, None, vec![42, 2]));
        fixture.push(Pid(5), meta(3), (3, None, vec![42, 3]));
        fixture.push(Pid(5), meta(4), (4, None, vec![42, 2]));
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

    /// The marker each fact the readers sent answers, and whether the fact
    /// is a read.
    fn answered(facts: impl Iterator<Item = DeliveredFact>) -> Vec<(u64, bool)> {
        facts
            .map(|fact| match fact {
                DeliveredFact::UiaFocusRead { marker, .. } => (marker, true),
                DeliveredFact::UiaFocusDropped {
                    marker,
                    focus: true,
                    no_window: false,
                } => (marker, false),
                other => panic!("not a focus's read or drop: {other:?}"),
            })
            .collect()
    }

    /// An event the schedule drops for a newer one of its element answers
    /// its marker once, with its drop, and every other marker is answered
    /// once by its read.
    #[test]
    fn a_replaced_events_marker_is_answered_once() {
        let fixture = fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), vec![42, 1]));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.push(Pid(5), meta(2), (2, None, vec![42, 2]));
        fixture.push(Pid(5), meta(3), (3, None, vec![42, 2]));
        open.send(()).expect("the gate opens");
        let facts = (0..3).map(|_| fixture.facts.recv_timeout(WAIT).expect("a fact"));
        assert_eq!(answered(facts), [(2, false), (1, false), (3, false)]);
        assert_eq!(fixture.readers.stop(), 1);
        assert_eq!(answered(fixture.facts.try_iter()), []);
    }

    /// The readers stopped and dropped with an event waiting behind a read
    /// answer the read's marker by its outcome once it returns, and the
    /// waiting event's, which the schedule leaves without one, with the
    /// drop its owed answer sends as it is dropped: each once.
    #[test]
    fn the_readers_stopped_with_an_event_waiting_answer_its_marker() {
        let Fixture {
            readers,
            read_on,
            facts,
            outcomes,
            ..
        } = fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        let owed = |marker: u64| {
            let facts = readers
                .shared
                .reading
                .facts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            Responder::new(move |fact: Option<DeliveredFact>| {
                let _ = facts.send(fact.unwrap_or(DeliveredFact::UiaFocusDropped {
                    marker,
                    focus: true,
                    no_window: false,
                }));
            })
        };
        readers.push(Pid(5), meta(1), (1, Some(gate), Vec::new()), owed(1));
        assert_eq!(read_on.recv_timeout(WAIT).expect("a read").0, 1);
        readers.push(Pid(5), meta(2), (2, None, Vec::new()), owed(2));
        thread::scope(|scope| {
            let stopping = scope.spawn(|| readers.stop());
            // The read returns only once the readers are stopping.
            let waited = Instant::now();
            while !readers.shared.lock().schedule.stopping() {
                assert!(waited.elapsed() < WAIT, "the readers never stopped");
                thread::yield_now();
            }
            open.send(()).expect("the gate opens");
            assert_eq!(stopping.join().expect("stopped"), 1);
        });
        drop(readers);
        // The waiting event has no outcome of the schedule's: its owed
        // answer, dropped with the readers, answers its marker.
        let reasons: Vec<(Pid, u64, &str)> = outcomes.try_iter().collect();
        assert_eq!(reasons, [(Pid(5), 1, "read")]);
        assert_eq!(answered(facts.try_iter()), [(1, false), (2, false)]);
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
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.push(Pid(5), meta(2), (2, None, Vec::new()));
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
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.readers.when_idle(Box::new(move || {
            let _ = outcomes.send((Pid(0), 1, "idle"));
        }));
        fixture.push(Pid(5), meta(2), (2, None, Vec::new()));
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
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.push(Pid(6), meta(2), (2, None, Vec::new()));
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
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, held_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        fixture.push(Pid(5), meta(2), (2, None, Vec::new()));
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

    /// NVDA's watchdog rule: a read held past its grace is judged at the
    /// grace's end and every grace after, with no foreground event, and
    /// abandoned once the user has moved on to another thread of its
    /// application; its event is dropped for that, and a new thread reads
    /// the application's next event.
    #[test]
    fn a_held_read_is_abandoned_once_the_user_moves_on_past_its_grace() {
        let fixture = fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, held_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        fixture.push(Pid(5), meta(2), (2, None, Vec::new()));
        assert_eq!(
            fixture.asked.recv_timeout(WAIT).expect("the grace's check"),
            WINDOW,
            "the grace's end asks about the read's window"
        );
        fixture.set_moved_on(true);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, MOVED_ON),
            "the next half second's check found the user had moved on"
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
        let asked: Vec<isize> = fixture.asked.try_iter().collect();
        assert_eq!(asked, [WINDOW], "one more check, which abandoned it");
    }

    /// A held read is not judged within its grace, and neither a
    /// foreground change nor a tick a microsecond before the grace's end
    /// asks; the tick at the grace's end judges it.
    #[test]
    fn a_held_read_within_its_grace_is_left_alone_though_the_user_moved_on() {
        let (fixture, still) = still_fixture(Duration::from_secs(60));
        fixture.set_moved_on(true);
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.readers.foreground_changed(WINDOW);
        {
            let mut state = fixture.readers.shared.lock();
            tick_at(&fixture, &still, &mut state, grace_us() - 1);
            assert_eq!(
                fixture.asked.try_recv(),
                Err(mpsc::TryRecvError::Empty),
                "nothing asked within the grace"
            );
            assert_eq!(fixture.outcomes.try_recv(), Err(mpsc::TryRecvError::Empty));
            tick_at(&fixture, &still, &mut state, grace_us());
            assert_eq!(
                fixture.outcomes.try_recv(),
                Ok((Pid(5), 1, MOVED_ON)),
                "judged at the grace's end"
            );
        }
        open.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 1);
        let asked: Vec<isize> = fixture.asked.try_iter().collect();
        assert_eq!(asked, [WINDOW]);
    }

    /// Past its grace a held read is left alone while the user has not
    /// moved on, judged at the grace's end and every grace after, so only
    /// the deadline could abandon it
    /// (`a_read_past_its_deadline_is_dropped_and_a_new_thread_reads_on`):
    /// the read's own outcome is sent once it returns.
    #[test]
    fn a_held_read_past_its_grace_is_left_alone_while_the_user_has_not_moved_on() {
        let fixture = fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, read_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        for check in ["the grace's check", "the next half second's check"] {
            assert_eq!(fixture.asked.recv_timeout(WAIT).expect(check), WINDOW);
        }
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
    /// the user has moved on: only its deadline abandons it, and nothing is
    /// asked.
    #[test]
    fn a_windowless_sender_s_read_is_not_judged_before_its_window_is_known() {
        let fixture = fixture_of(Duration::from_millis(800), (0, 0));
        fixture.set_moved_on(true);
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, NOT_IN_TIME)
        );
        open.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 1);
        assert_eq!(fixture.asked.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    /// Once a read whose sender is not a window has found the sender's
    /// nearest window, the read is judged by that window at its grace's
    /// end, with no foreground event, and abandoned: that window is asked
    /// about once, and nothing else is asked.
    #[test]
    fn a_read_that_found_its_sender_s_nearest_window_is_judged_by_it() {
        let fixture = fixture_of(Duration::from_secs(60), (0, NEAREST));
        fixture.set_moved_on(true);
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, MOVED_ON)
        );
        open.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 1);
        let asked: Vec<isize> = fixture.asked.try_iter().collect();
        assert_eq!(asked, [NEAREST]);
    }

    /// An event queued behind a read abandoned as the user moved on is
    /// read by a new thread and judged at its own read's grace end, and
    /// abandoned as moved on too.
    #[test]
    fn a_read_taken_by_a_new_thread_is_judged_at_its_own_grace_end() {
        let fixture = fixture(Duration::from_secs(60));
        fixture.set_moved_on(true);
        let (open_first, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        let (open_second, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, MOVED_ON)
        );
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 2);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, MOVED_ON)
        );
        open_first.send(()).expect("the first held read waits");
        open_second.send(()).expect("the second held read waits");
        assert_eq!(fixture.readers.stop(), 2);
        assert_eq!(fixture.outcomes.try_recv().ok(), None);
        let asked: Vec<isize> = fixture.asked.try_iter().collect();
        assert_eq!(asked, [WINDOW, WINDOW], "each read's grace check");
    }

    /// Each ask of the schedule's is answered once, after the step's other
    /// effects, and before the readers' lock is let go: with the lock held,
    /// the tick at a read's grace's end is made by hand, and by its return
    /// the user's moving on has been asked once and the read's drop sent.
    #[test]
    fn each_ask_is_answered_once_before_the_lock_is_let_go() {
        let (fixture, still) = still_fixture(Duration::from_secs(60));
        fixture.set_moved_on(true);
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        {
            let mut state = fixture.readers.shared.lock();
            tick_at(&fixture, &still, &mut state, grace_us());
            assert_eq!(fixture.asked.try_recv(), Ok(WINDOW), "asked once");
            assert_eq!(fixture.asked.try_recv(), Err(mpsc::TryRecvError::Empty));
            assert_eq!(fixture.outcomes.try_recv(), Ok((Pid(5), 1, MOVED_ON)));
        }
        open.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 1);
    }

    /// A step that asks of one application and sends another's outcomes
    /// sends those outcomes first, so what is owed matches the schedule
    /// when the answer's step runs: application 6's read passes its
    /// deadline in the tick that asks about application 5's, begun later.
    #[test]
    fn a_step_with_an_ask_and_another_applications_outcomes_keeps_the_owed_markers() {
        let (fixture, still) = still_fixture(Duration::from_secs(1));
        let (open_six, gate) = mpsc::channel();
        fixture.push(Pid(6), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        {
            let mut state = fixture.readers.shared.lock();
            tick_at(&fixture, &still, &mut state, grace_us());
            assert_eq!(fixture.asked.try_recv(), Ok(WINDOW), "the grace's check");
        }
        // Application 5's read begins at application 6's grace's end.
        let (open_five, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 2);
        fixture
            .readers
            .shared
            .reading
            .order
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        {
            // At application 6's deadline and application 5's grace's end,
            // before application 5's deadline.
            let mut state = fixture.readers.shared.lock();
            tick_at(&fixture, &still, &mut state, 2 * grace_us());
        }
        assert_eq!(
            *fixture
                .readers
                .shared
                .reading
                .order
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            ["outcome 6 1".to_owned(), format!("asked {WINDOW}")],
            "application 6's drop, then the answer to application 5's ask"
        );
        open_six.send(()).expect("the held read waits");
        open_five.send(()).expect("the held read waits");
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(6), 1, NOT_IN_TIME)
        );
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, "read")
        );
        assert_eq!(fixture.readers.stop(), 2);
    }

    /// With [`ABANDONED_LIMIT`] of an application's threads abandoned and
    /// still in their calls, its next events are not dropped: they wait,
    /// pending focuses for the application's own menu openings and not for
    /// another application's, and the first abandoned thread whose call
    /// returns before the application's bound reads them in the order they
    /// arrived.
    #[test]
    fn an_event_at_the_abandoned_limit_waits_for_a_thread_that_returns() {
        let fixture = fixture(Duration::from_secs(60));
        fixture.set_moved_on(true);
        let (open_first, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, first) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, MOVED_ON)
        );
        let (open_second, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 2);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, MOVED_ON)
        );
        fixture.push(Pid(5), meta(3), (3, None, vec![42, 3]));
        fixture.push(Pid(5), meta(4), (4, None, vec![42, 4]));
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
        let lasts: Vec<(u64, Option<u64>)> = fixture.lasts.try_iter().collect();
        assert_eq!(
            lasts,
            [(1, None), (2, None), (3, None), (4, Some(3))],
            "the returning thread forgot its abandoned read: Core never got it"
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
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "its read did not return in time")
        );
        let (open_second, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, "its read did not return in time")
        );
        fixture.push(Pid(5), meta(3), (3, None, Vec::new()));
        fixture.push(Pid(5), meta(4), (4, None, Vec::new()));
        let waited = "it waited too long behind its application's abandoned reads";
        let outcomes: Vec<(Pid, u64, &str)> = (0..2)
            .map(|_| fixture.outcomes.recv_timeout(WAIT).expect("an outcome"))
            .collect();
        assert_eq!(outcomes, [(Pid(5), 3, waited), (Pid(5), 4, waited)]);
        fixture.push(Pid(5), meta(5), (5, None, Vec::new()));
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

    /// A reading thread that cannot start answers its application's
    /// events as dropped with no thread to read them, the idle barrier
    /// waiting on them is answered, the application's next event starts a
    /// thread whose read is delivered, and the readers stop.
    #[test]
    fn a_reading_thread_that_cannot_start_answers_its_events_and_the_next_event_is_read() {
        // An application no other test pushes events for, so the selector
        // names this test's first thread start alone.
        const APPLICATION: Pid = Pid(9_901);
        let refusal = verbatim_faults::arm(
            verbatim_faults::Fault::ThreadStart,
            |&(name, pid, _): &(&'static str, Pid, u64)| {
                name == READING_THREAD && pid == APPLICATION
            },
            1,
        );
        let fixture = fixture(Duration::from_secs(60));
        fixture.push(APPLICATION, meta(1), (1, None, Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (APPLICATION, 1, "no thread to read it")
        );
        let outcomes = fixture
            .readers
            .shared
            .reading
            .outcomes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        fixture.readers.when_idle(Box::new(move || {
            let _ = outcomes.send((Pid(0), 0, "idle"));
        }));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(0), 0, "idle"),
            "the idle barrier is answered"
        );
        fixture.push(APPLICATION, meta(2), (2, None, Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (APPLICATION, 2, "read"),
            "the next event started a thread that read it"
        );
        assert_eq!(fixture.readers.stop(), 1);
        drop(refusal);
    }

    /// Found by the read scheduler's generated test
    /// (`docs/design/logic-tests.md`, "The listener's scheduler"): a tick
    /// abandons two applications' reads, the first with an event waiting,
    /// whose new thread cannot start. The failed start is stepped once the
    /// tick's effects are applied, so the second application's drop is sent
    /// before the waiting event's, and the idle waiter runs after both.
    #[test]
    fn a_failed_start_is_stepped_after_its_steps_other_effects() {
        // Applications no other test pushes events for, so the selector
        // names this test's second thread start of the first alone.
        const FIRST: Pid = Pid(9_902);
        const SECOND: Pid = Pid(9_903);
        let refusal = verbatim_faults::arm(
            verbatim_faults::Fault::ThreadStart,
            |&(name, pid, generation): &(&'static str, Pid, u64)| {
                name == READING_THREAD && pid == FIRST && generation > 1
            },
            1,
        );
        let (fixture, still) = still_fixture(Duration::from_secs(60));
        let (open_first, gate) = mpsc::channel();
        fixture.push(FIRST, meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        fixture.push(FIRST, meta(2), (2, None, Vec::new()));
        let (open_second, gate) = mpsc::channel();
        fixture.push(SECOND, meta(3), (3, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 3);
        let outcomes = fixture
            .readers
            .shared
            .reading
            .outcomes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        fixture.readers.when_idle(Box::new(move || {
            let _ = outcomes.send((Pid(0), 0, "idle"));
        }));
        {
            let mut state = fixture.readers.shared.lock();
            tick_at(&fixture, &still, &mut state, 60_000_000);
        }
        let sent: Vec<(Pid, u64, &'static str)> = (0..4)
            .map(|_| fixture.outcomes.recv_timeout(WAIT).expect("an outcome"))
            .collect();
        assert_eq!(
            sent,
            [
                (FIRST, 1, NOT_IN_TIME),
                (SECOND, 3, NOT_IN_TIME),
                (FIRST, 2, "no thread to read it"),
                (Pid(0), 0, "idle"),
            ]
        );
        open_first.send(()).expect("the held read waits");
        open_second.send(()).expect("the held read waits");
        assert_eq!(fixture.readers.stop(), 2);
        drop(refusal);
    }

    #[test]
    fn a_thread_ends_when_its_applications_outpost_ends() {
        let fixture = fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
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
        // Let go, nothing of it is kept.
        assert_eq!(
            fixture
                .readers
                .shared
                .lock()
                .apps
                .keys()
                .collect::<Vec<_>>(),
            Vec::<&Pid>::new()
        );
        // The application runs on: its next event starts a new thread.
        fixture.push(Pid(5), meta(2), (2, None, Vec::new()));
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
