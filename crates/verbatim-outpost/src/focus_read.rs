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
//!   window the system reports hung is dropped; one whose nearest window
//!   does not use UIA, or that has none, is not native and is dropped; a
//!   sender UI Automation counts as the last focus any of the listener's
//!   readers accepted (`CompareElements`), while that element still has
//!   the keyboard focus, read live, is a repeat and is dropped
//!   ([`LastAccepted`]); one whose nearest window fails NVDA's acceptance
//!   test is dropped; the console window's own element, which NVDA's
//!   `consoleUIAWindow` refuses, and a shell window that only stages a
//!   switch, whose focus NVDA's File Explorer app module leaves unsaid,
//!   are dropped; and a sender without the keyboard focus, read with a
//!   plain property read, is dropped, as NVDA's `shouldAllowUIAFocusEvent`
//!   drops it; a menu opening is judged by NVDA's automation event
//!   handler's checks instead, the same but for the repeat, and with a
//!   sender UI Automation counts as the current focus judged as that
//!   focus, with its window and without the native check;
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
//! The whole read, its checks included, has one time budget,
//! [`READ_BUDGET`], asked before each of its calls after the first, which
//! does not count the raw probe's wait ([`Budget`]): once it has passed,
//! the read makes no more calls and reports what it has read, as the
//! event's cache gives what it has not, with its ancestors unknown, or
//! drops the event when its window is not yet judged. A watchdog abandons
//! a thread whose read runs past [`READ_DEADLINE`], the budget, the
//! probe's wait, and the one call that can be under way when the budget
//! passes, and, as NVDA's watchdog cancels a slow call once the user has
//! moved on (`docs/nvda/main-loop-and-watchdog.md`, "The watchdog:
//! detection"), a thread whose read of a focus has taken
//! [`MOVED_ON_GRACE`](crate::outpost::window::MOVED_ON_GRACE) while the
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
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_LegacyIAccessibleStatePropertyId,
};
use windows::core::AgileReference;

use verbatim_model::{Pid, State, TraceId};
use verbatim_uia::map::{CachedUiaParts, add_legacy_checked_state, wants_legacy_checked_state};
use verbatim_uia::{
    Ancestor, Cached, ElementExt as _, Fill, FocusInput, ListenerEvent, Shape, Uia,
};
use verbatim_uia_rops::{Ancestry, FocusAncestry, FocusQuery, focus_ancestry_classic};

use crate::ForegroundReader;
use crate::arbitration::{Arbitrator, Clock, known_verdict, uses_uia, window_class_name};
use crate::outpost::acceptance::{EventName, RequestedEvents, SystemWindows, accepts_event};
use crate::outpost::text_reads::CONSOLE_WINDOW_CLASS;
use crate::outpost::window::{
    front_is_another_thread_of_its_application, is_shell_transition_window, window_is_hung,
};
use crate::protocol::{
    DeliveredFact, EventTiming, UiaAncestorFact, UiaFocusRead, UiaSnapshotFact, now_us,
};

/// The most ancestors read for a focus, as many as the outpost walks.
const MAX_ANCESTORS: u32 = 64;

/// How long one focus event's read, its checks included, may take before
/// it makes no more calls into the application: asked before each call
/// after the first, each of which UIA ends after its transaction timeout.
/// The raw probe's wait, which can be 3 seconds by itself, is not counted
/// ([`Budget`]), so that the probe of a slow window cannot use up the
/// budget alone and leave the focus's checks unmade
/// (`docs/design/listener-without-connections.md`, section 5). A read of
/// an application that answers takes milliseconds (section 10, step 6),
/// so only a slow application meets it, and its focus is then reported
/// with what was read by then, its ancestors unknown, rather than held
/// (section 9, risk 4).
const READ_BUDGET: Duration = Duration::from_secs(2);

/// How long a reading thread may take over one focus event before the
/// watchdog abandons it, and the bound on an application at
/// [`ABANDONED_LIMIT`], 11 seconds: the [`READ_BUDGET`] of 2 seconds,
/// the raw probe's wait of up to 3 seconds
/// ([`verbatim_uia::RAW_PROBE_TIMEOUT_MS`]), which the budget does not
/// count, UIA's transaction timeout of 5 seconds for the call that can be
/// under way when the budget passes, and a second to spare. It bounds
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
    .saturating_add(Duration::from_millis(
        verbatim_uia::RAW_PROBE_TIMEOUT_MS as u64,
    ))
    .saturating_add(verbatim_uia::TRANSACTION_TIMEOUT)
    .saturating_add(Duration::from_secs(1));

/// One focus read's time budget, [`READ_BUDGET`], on the reader's clock,
/// asked before each of the read's calls after the first. The raw probe's
/// wait is not counted: the budget's end moves on by the time the window's
/// verdict took to decide, up to the console check that may follow the
/// probe ([`FocusReader::native_window`]), so that for a sender with a
/// window of its own, whose probe is the read's first call, the budget
/// starts once the probe has answered.
///
/// It also times each of the read's calls, by the system's clock whatever
/// the reader's: a call that takes longer than [`SLOW_CALL`] is logged at
/// info with its name and how long it took, so the slowest calls behind
/// [`READ_DEADLINE`] can be seen (Dickson, 2026-10-20). Each call costs one
/// clock reading, at its start, which is also the end of the call before
/// it; the last ends when the budget is dropped, at the read's end.
struct Budget {
    clock: Clock,
    /// When the time is up.
    end: Cell<Instant>,
    /// When the window's verdict began to be decided, until it is.
    verdict_began: Cell<Option<Instant>>,
    /// The call under way, by name, and when it began, by the system's
    /// clock.
    call: Cell<Option<(&'static str, Instant)>>,
}

/// How long one call of a focus read may take before it is logged at info.
const SLOW_CALL: Duration = Duration::from_secs(1);

impl Budget {
    /// A budget from now, by `clock`.
    fn new(clock: Clock) -> Self {
        let end = clock() + READ_BUDGET;
        Self {
            clock,
            end: Cell::new(end),
            verdict_began: Cell::new(None),
            call: Cell::new(None),
        }
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

    /// The window's verdict, and with it perhaps the raw probe, begins to
    /// be decided; its time is not counted.
    fn verdict_begins(&self) {
        self.verdict_began.set(Some((self.clock)()));
    }

    /// The raw probe, if one was made, has answered: the budget's end
    /// moves on by the time since [`verdict_begins`](Self::verdict_begins),
    /// once.
    fn probe_ended(&self) {
        if let Some(began) = self.verdict_began.take() {
            let waited = (self.clock)().saturating_duration_since(began);
            self.end.set(self.end.get() + waited);
        }
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

/// What a read does once its window becomes known
/// ([`WatchedRead::learn`]): judges, as the watchdog would, whether the
/// window in front is already another thread's, and wakes the watchdog if
/// it is.
type OnLearn = Box<dyn Fn(&WatchedRead) + Send + Sync>;

/// One read as the reading threads' watchdog watches it, shared by the read
/// and the watchdog. Each read has its own, so an abandoned read that finds
/// its window later changes nothing the watchdog judges.
///
/// The window it is of, for the judgement of whether the user has moved on
/// from it ([`MOVED_ON_GRACE`](crate::outpost::window::MOVED_ON_GRACE)): the
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
/// And whether the watchdog has abandoned it ([`abandon`](Self::abandon)):
/// a read that returns after that makes no focus the last one accepted
/// ([`LastAccepted`]), so it cannot overwrite a newer focus accepted while
/// it was held.
#[derive(Clone, Default)]
pub(crate) struct WatchedRead(Arc<Watched>);

#[derive(Default)]
struct Watched {
    window: AtomicIsize,
    abandoned: AtomicBool,
    /// Whether the window in front has been judged against the read's
    /// window since that became known.
    front_judged: AtomicBool,
    /// Whether it was then another thread's of the read's application.
    front_elsewhere: AtomicBool,
    on_learn: Option<OnLearn>,
}

impl WatchedRead {
    /// A read of window `hwnd`, 0 for one not yet known, which does
    /// `on_learn` once it learns its window.
    fn new(hwnd: isize, on_learn: Option<OnLearn>) -> Self {
        Self(Arc::new(Watched {
            window: AtomicIsize::new(hwnd),
            on_learn,
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

    /// The window the read is of, 0 for none known yet.
    fn window(&self) -> isize {
        self.0.window.load(Ordering::SeqCst)
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

    /// The watchdog has abandoned the read.
    fn abandon(&self) {
        self.0.abandoned.store(true, Ordering::SeqCst);
    }

    /// Whether the watchdog has abandoned the read.
    fn abandoned(&self) -> bool {
        self.0.abandoned.load(Ordering::SeqCst)
    }
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
    /// The fact that resolves marker `marker` with this outcome.
    #[must_use]
    pub fn into_fact(self, marker: u64) -> DeliveredFact {
        match self {
            Judged::Read(read) => DeliveredFact::UiaFocusRead { marker, read },
            Judged::Dropped(_) => DeliveredFact::UiaFocusDropped { marker },
            Judged::Passed(fact) => DeliveredFact::UiaMarked { marker, fact },
        }
    }
}

/// The focus a reading thread read last, for NVDA's duplicate evidence
/// and for the ancestor read to stop where it meets this focus's.
struct LastFocus {
    runtime_id: Vec<i32>,
    element: IUIAutomationElement,
    /// Its raw-view ancestors, nearest first.
    ancestors: Vec<UiaAncestorFact>,
}

/// The last focus any of the listener's reading threads accepted, shared
/// by them all, as NVDA keeps one last focused UIA element for the whole
/// program (`docs/design/focus-pipeline.md`, section 16.3), with the reader
/// that accepted it: a focus event whose sender UI Automation counts as
/// this element, while it still has the keyboard focus, is a repeat and is
/// dropped before it is read ([`FocusReader::read`]). Each thread's own
/// focus before still serves the ancestor read's stop and the evidence
/// that a runtime id now names another element (`same_as_last`).
///
/// NVDA compares with the last focus it queued, whatever reported it, and
/// only while that is a UIA object (`lastQueuedFocusObject`), so once Core
/// queues a newer focus that is not this one, from an outpost's MSAA
/// events, a foreground change, or a focus it asked for, the element no
/// longer counts ([`queued`](Self::queued)).
#[derive(Clone, Default)]
pub struct LastAccepted(Arc<Mutex<Option<Accepted>>>);

/// The last focus accepted: the number of the reader that accepted it, the
/// element, the window it was judged with (NVDA's object's `windowHandle`),
/// the event it came in, and whether a newer focus has been queued since,
/// after which it is no repeat's evidence and not the current focus. It is
/// kept until its reader replaces or releases it, so it is released on a
/// thread in COM's apartment.
struct Accepted {
    reader: u64,
    element: AgileReference<IUIAutomationElement>,
    window: isize,
    arrival: Arrival,
    superseded: bool,
}

/// The UIA event a focus came in: its trace, and when the listener
/// received it, in milliseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    /// The trace minted for the event.
    pub trace: TraceId,
    /// When the event was received.
    pub observed_at_ms: u64,
}

impl LastAccepted {
    fn lock(&self) -> MutexGuard<'_, Option<Accepted>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The last focus accepted, with the window it was judged with, if
    /// there is one, no newer focus has been queued since, and it can be had
    /// on this thread: the current focus, as far as the listener can hold
    /// it, which NVDA's automation event handler compares a menu opening's
    /// sender with.
    fn current(&self) -> Option<(IUIAutomationElement, isize)> {
        self.lock()
            .as_ref()
            .filter(|accepted| !accepted.superseded)
            .and_then(|accepted| {
                accepted
                    .element
                    .resolve()
                    .ok()
                    .map(|element| (element, accepted.window))
            })
    }

    /// Makes `element`, which reader `reader` accepted from the event
    /// `arrival` and judged with window `window` in the read `read`, the
    /// last focus accepted, unless the watchdog has abandoned that read: a
    /// focus accepted since, by the thread that took the application's
    /// later events, is the newer. The read's abandonment is asked under
    /// the lock, and the watchdog marks a read abandoned before a newer
    /// read can begin, so a read abandoned after it is asked accepts its
    /// focus before any newer one does.
    fn accept(
        &self,
        reader: u64,
        element: &IUIAutomationElement,
        (window, arrival): (isize, Arrival),
        read: &WatchedRead,
    ) {
        match AgileReference::new(element) {
            Ok(element) => {
                let mut slot = self.lock();
                if read.abandoned() {
                    tracing::debug!(
                        "an abandoned read's focus is not made the last focus accepted"
                    );
                    return;
                }
                *slot = Some(Accepted {
                    reader,
                    element,
                    window,
                    arrival,
                    superseded: false,
                });
            }
            Err(error) => {
                tracing::debug!(%error, "the last focus accepted could not be kept");
            }
        }
    }

    /// Core took the event traced `trace`, observed at `observed_at_ms`
    /// (0 for a focus it asked for), as the focus. Unless that is the last
    /// focus accepted, or was observed before it, so that the accepted one
    /// is the newer and Core takes it next, the last focus accepted is no
    /// longer the last focus queued, and is no repeat's evidence.
    pub fn queued(&self, trace: TraceId, observed_at_ms: u64) {
        let mut slot = self.lock();
        if let Some(accepted) = slot.as_mut()
            && !accepted.superseded
            && accepted.arrival.trace != trace
            && (observed_at_ms == 0 || observed_at_ms >= accepted.arrival.observed_at_ms)
        {
            tracing::debug!(
                ?trace,
                observed_at_ms,
                "another focus was queued; the last focus accepted is no repeat's evidence"
            );
            accepted.superseded = true;
        }
    }

    /// The watchdog has abandoned the read of the event traced `trace`,
    /// whose drop Core is told of: a focus that read accepted just before
    /// is no current focus and no repeat's evidence, since Core never
    /// received it. Called under the readers' lock after the read is marked
    /// abandoned, so this covers an accept made before the mark, and the
    /// mark one made after ([`accept`](Self::accept)). The element is kept
    /// for its reader to release, on its own thread in COM's apartment.
    pub(crate) fn abandoned(&self, trace: TraceId) {
        let mut slot = self.lock();
        if let Some(accepted) = slot.as_mut()
            && accepted.arrival.trace == trace
        {
            tracing::debug!(
                ?trace,
                "an abandoned read's focus was accepted; it is no repeat's evidence"
            );
            accepted.superseded = true;
        }
    }

    /// Forgets the last focus accepted if reader `reader` accepted it, so
    /// it is released on the thread that ends with that reader, while the
    /// thread is still in COM's apartment.
    fn release(&self, reader: u64) {
        let mut slot = self.lock();
        if slot
            .as_ref()
            .is_some_and(|accepted| accepted.reader == reader)
        {
            *slot = None;
        }
    }
}

/// The next [`FocusReader`]'s number, which names the reader that accepted
/// the [`LastAccepted`] focus.
static NEXT_READER: AtomicU64 = AtomicU64::new(1);

/// One application's focus reader: a UIA client of its own and the focus
/// it read last. Lives on one reading thread.
pub struct FocusReader {
    uia: Uia,
    last: Option<LastFocus>,
    /// This reader's number among the listener's readers.
    number: u64,
    /// The last focus any reader accepted.
    last_accepted: LastAccepted,
    /// Reads the foreground window for NVDA's acceptance test: the
    /// system's, unless a test supplies its own
    /// ([`set_foreground_reader`](Self::set_foreground_reader)).
    foreground: ForegroundReader,
    /// Which of this application's windows use UIA, for NVDA's native
    /// check (`isNativeUIAElement`), decided as an outpost decides it
    /// ([`crate::arbitration::uses_uia`]).
    arbitrator: Mutex<Arbitrator>,
    /// The read under way as the reading threads' watchdog watches it,
    /// told a focus's sender's nearest window once the read finds it, and asked
    /// whether it was abandoned before its focus is accepted
    /// ([`judge_watched`](Self::judge_watched)).
    window: WatchedRead,
}

impl Drop for FocusReader {
    fn drop(&mut self) {
        self.last_accepted.release(self.number);
    }
}

impl FocusReader {
    /// A reader with a UIA client of its own, made on the calling thread,
    /// which joins COM's multithreaded apartment. Every read is the classic
    /// walk, which makes no remote operation
    /// (`docs/design/listener-without-connections.md`, section 2).
    /// `last_accepted` is the last focus any of the listener's readers
    /// accepted, which this one shares.
    ///
    /// # Errors
    ///
    /// The COM error when the client cannot be made.
    pub fn new(last_accepted: LastAccepted) -> windows::core::Result<Self> {
        Ok(Self {
            uia: Uia::new()?,
            last: None,
            number: NEXT_READER.fetch_add(1, Ordering::Relaxed),
            last_accepted,
            foreground: Arc::new(crate::outpost::window::foreground_window_handle),
            arbitrator: Mutex::new(Arbitrator::new(&[])),
            window: WatchedRead::default(),
        })
    }

    /// Reads the foreground window with `reader` from now on, in place of
    /// the system's, for NVDA's acceptance test: a test whose application
    /// runs on a desktop where no window can take the foreground says which
    /// window is the foreground, as it does to an outpost
    /// (`Outpost::set_foreground_reader`).
    pub fn set_foreground_reader(&mut self, reader: ForegroundReader) {
        self.foreground = reader;
    }

    /// Reads the time for its windows' kept verdicts and for the time
    /// budget of its classic walk from `clock` from now on, in place of the
    /// system's, as an outpost does (`Outpost::set_clock`): a test that
    /// counts a read's calls decides when the walk's time is up, however
    /// long its calls take.
    pub fn set_clock(&mut self, clock: Clock) {
        self.arbitrator
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .set_clock(clock);
    }

    /// The reader's clock ([`set_clock`](Self::set_clock)), which its
    /// classic walk measures its time budget by.
    fn clock(&self) -> Clock {
        self.arbitrator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clock()
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
    pub fn read(
        &mut self,
        sender: &Cached<ListenerEvent>,
        kind: FocusEvent,
        arrival: Arrival,
    ) -> Judged {
        let fields = sender.read();
        let element = sender.live().element();
        let hwnd = fields.role.hwnd;
        let event = fields.parts(sender.runtime_id());
        // The read's one time budget, from here, for its checks and its
        // read alike, the probe's wait aside.
        let budget = Budget::new(self.clock());
        let out_of_time = || budget.out_of_time();
        let nearest =
            match self.judge_before_reading(sender, (&fields, &event.runtime_id), kind, &budget) {
                Ok(nearest) => nearest,
                Err(reason) => return Judged::Dropped(reason),
            };
        let previous = self
            .last
            .as_ref()
            .filter(|last| last.runtime_id == event.runtime_id)
            .map(|last| last.element.clone());
        // The focus before and its ancestors, nearest first: where the
        // ancestor read stops ([`FocusQuery::known`]).
        let known: Vec<Vec<i32>> = self
            .last
            .as_ref()
            .map(|last| {
                std::iter::once(last.runtime_id.clone())
                    .chain(
                        last.ancestors
                            .iter()
                            .map(|ancestor| ancestor.snapshot.runtime_id.clone()),
                    )
                    .collect()
            })
            .unwrap_or_default();
        // Every detail: the listener reads for every application, whatever
        // theme decides which details are spoken. A request that cannot be
        // built is a read that failed.
        let input = sender.narrow::<FocusInput>();
        let read = self
            .uia
            .request::<Ancestor>(Ancestor::all_on())
            .map_err(verbatim_uia_rops::Error::Uia)
            .and_then(|request| {
                let fill = Fill::new(&request);
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
                    out_of_time: Some(&out_of_time),
                    require_focus: false,
                    refresh: true,
                    nearest_window: Some(nearest),
                    before_call: Some(&|call| budget.call(call)),
                };
                focus_ancestry_classic(&self.uia, &query)
            });
        match read {
            Ok(FocusAncestry::NotFocused) => {
                Judged::Dropped("its element no longer has the keyboard focus")
            }
            Ok(FocusAncestry::Focused(ancestry)) => self.focused(
                element,
                (hwnd, nearest),
                event,
                &ancestry,
                (arrival, &budget),
            ),
            Err(error) => {
                // NVDA lets a focus event through when its sender's keyboard
                // focus cannot be read (`shouldAllowUIAFocusEvent` answers
                // true on a COM error): reported as the event's cache gives
                // it, its ancestors unknown, whatever that cache says of its
                // keyboard focus. Step 6 has already judged that: a live
                // read that answered true, or failed, lets the focus through
                // in NVDA too, and a cache that said no has already dropped
                // it out of time.
                tracing::debug!(%error, "a UIA focus could not be read; reported from its event");
                self.last = Some(LastFocus {
                    runtime_id: event.runtime_id.clone(),
                    element: element.clone(),
                    ancestors: Vec::new(),
                });
                self.last_accepted
                    .accept(self.number, element, (nearest, arrival), &self.window);
                // Reported as the focus, as `focused` reports one, whatever
                // the event's cache says of its keyboard focus.
                let mut snapshot = fact_of(event);
                snapshot.states = snapshot.states.with(State::Focused);
                Judged::Read(Box::new(UiaFocusRead {
                    hwnd,
                    snapshot,
                    ancestors: None,
                    selected_child: None,
                    value_from_selection: false,
                    window: Some(nearest),
                    same_as_last: None,
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
                            return Judged::Dropped(
                                "it has no window above it, so NVDA makes no object for it",
                            );
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
                    return Judged::Dropped("its sender has no window above it");
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

    /// The read of a focus whose element had the keyboard focus: its name
    /// and role from the event's cache, `event`, as NVDA reads them from
    /// the event's element, and its value, states, and details afresh; its
    /// ancestors as read, followed by those of the focus before above the
    /// one the read stopped at. `arrival` is the event it came in.
    fn focused(
        &mut self,
        element: &IUIAutomationElement,
        (hwnd, nearest): (isize, isize),
        event: CachedUiaParts,
        ancestry: &Ancestry,
        (arrival, budget): (Arrival, &Budget),
    ) -> Judged {
        let path = "classic";
        let live = ancestry
            .element
            .as_ref()
            .map(|fresh| fresh.read().node.parts(fresh.runtime_id()));
        // A combo box without the `Value` pattern, or a looping selector,
        // read afresh: its first selected item is its value.
        let value_from_selection = ancestry
            .element
            .as_ref()
            .is_some_and(|fresh| fresh.read().node.value_from_selection());
        // The role from the fresh read too: the event's cache holds the
        // control type but not what refines it, such as the Toggle pattern
        // a toggle button has, which NVDA reads when it needs it.
        let (role, value, states, details) = match live {
            Some(live) => (live.role, live.value, live.states, live.details),
            None => (event.role, event.value, event.states, event.details),
        };
        let mut snapshot = UiaSnapshotFact {
            runtime_id: event.runtime_id,
            role,
            name: event.name,
            value,
            states: states.with(State::Focused),
            details,
        };
        if wants_legacy_checked_state(snapshot.role, snapshot.states) && !budget.out_of_time() {
            // A menu item without the Toggle pattern: its checked state may
            // be only in its legacy MSAA state, which the classic walk
            // leaves to this one live call, made while the read's time
            // lasts.
            budget.call("the legacy MSAA state");
            let legacy = element
                .current_i32_ignoring_default(UIA_LegacyIAccessibleStatePropertyId)
                .ok()
                .flatten();
            add_legacy_checked_state(&mut snapshot.states, legacy);
        }
        let ancestors = (!ancestry.out_of_time).then(|| {
            let tail = match (ancestry.met_known, &self.last) {
                (Some(met), Some(last)) => reused_ancestors(&last.ancestors, met),
                _ => Vec::new(),
            };
            ancestry
                .ancestors
                .iter()
                .map(ancestor_fact)
                .chain(tail)
                .collect::<Vec<_>>()
        });
        tracing::debug!(
            path,
            read = ancestry.ancestors.len(),
            met_known = ?ancestry.met_known,
            depth_limited = ancestry.depth_limited,
            out_of_time = ancestry.out_of_time,
            window = ?ancestry.window,
            "UIA focus ancestors read"
        );
        if let Some(facts) = &ancestors {
            log_ancestor_facts(path, facts);
        }
        // The classic walk's steps go through the client side, whose window
        // proxy gives a window's element its handle and title, the console
        // window's included (16.4), and it walks to the desktop's root or
        // the process boundary itself; the window is the nearest window
        // found for the acceptance test when the walk named none.
        let window = ancestry.window.or(Some(nearest));
        self.last = Some(LastFocus {
            runtime_id: snapshot.runtime_id.clone(),
            element: element.clone(),
            ancestors: ancestors.clone().unwrap_or_default(),
        });
        self.last_accepted
            .accept(self.number, element, (nearest, arrival), &self.window);
        let mut selected_child = ancestry
            .selected_child
            .as_ref()
            .map(|child| fact_of(child.read().node.parts(child.runtime_id())));
        if value_from_selection {
            snapshot.value = selected_child.take().and_then(|child| child.name);
        }
        Judged::Read(Box::new(UiaFocusRead {
            hwnd,
            snapshot,
            ancestors,
            selected_child,
            value_from_selection,
            window,
            same_as_last: ancestry.previous_focused,
        }))
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
    /// As NVDA queues it as a focus on the menu, it becomes the last focus
    /// accepted.
    ///
    /// The read's time, `budget`, is asked before each call after the
    /// first. Once it is up, a focus whose window's verdict is known is
    /// still spoken (Dickson, 2026-10-20): every check of NVDA's focus
    /// handler and of `shouldAllowUIAFocusEvent`
    /// (`NVDAObjects/UIA/__init__.py`) that needs no call into the
    /// application is made, and one that needs a call is skipped and lets
    /// the focus through, as NVDA lets it through when such a read fails
    /// (`docs/design/listener-without-connections.md`, section 5). Made:
    /// the hung window (step 1), from the event's cached window; the native
    /// check (step 2) from the window's verdict when it is known without a
    /// call, the event dropped when only the probe could decide it
    /// ([`native_window`](Self::native_window)); the acceptance test and
    /// the refusals (steps 4 and 5), local calls; and the sender's keyboard
    /// focus (step 6) from the event's cached `HasKeyboardFocus` in place
    /// of NVDA's live read, a sender whose event says it did not have the
    /// focus being dropped. Skipped, a deliberate divergence from NVDA: the
    /// repeat check (step 3), whose comparison the runtime ids could answer
    /// but which drops a repeat only while the last focus still has the
    /// keyboard focus, read live from that element, which no cache holds;
    /// a repeat out of time is spoken. A menu opening's comparison is its
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
            return Err("its window is hung");
        }
        // The current focus, as far as the listener holds it: the last
        // focus accepted, while no newer focus has been queued (section
        // 16.3). Core's current focus is the last focus it queued, and the
        // listener holds an element only for a UIA focus it accepted
        // itself. An MSAA focus, a foreground change, or a focus Core asked
        // for supersedes it, and NVDA's last focus or focus object is then
        // no UIA object either, so nothing is compared with it.
        let current = self.last_accepted.current();
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
                        return Err("its comparison with the current focus failed");
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
                // 3. NVDA's duplicate rule (`UIAHandler/__init__.py` 902 to
                // 923; section 16.3): the sender is the last focus accepted,
                // by any of the listener's readers, as UIA compares them,
                // and that element still has the keyboard focus, read live.
                // A comparison that fails is no duplicate, as NVDA lets the
                // event through on a COM error, and so is a keyboard focus
                // read that fails: the earlier element is gone. Two calls,
                // the second only for an element UIA equates. Out of time,
                // neither is made, and the focus is no repeat.
                if let (Some(Ok(true)), Some((last, _))) = (compare(), current.as_ref())
                    && kind == FocusEvent::Focus
                    && !out_of_time()
                    && {
                        budget.call("the last focus's keyboard focus");
                        last.has_keyboard_focus().unwrap_or(false)
                    }
                {
                    return Err("a repeat of the last focus, which still has the keyboard focus");
                }
                (nearest, element)
            }
        };
        // 4. NVDA's acceptance test against the same window
        // (`shouldAcceptEvent`, `UIAHandler/__init__.py` 924 to 930, and
        // for a menu opening 825 to 837; sections 16.2 and 16.3). A focus
        // that fails is dropped and never becomes the last accepted, so the
        // same element's focus once its window is in front, as a console's
        // text area's comes, is no repeat.
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
            return Err("its window is not related to the foreground window");
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
            return Err("the console window's own focus");
        }
        // The shell's windows that take the focus only while a switch is
        // staged, which NVDA's File Explorer app module ignores when it
        // handles the focus (`appModules/explorer.py` 443 to 449). NVDA
        // makes such a window its focus object before the app module
        // ignores it; that is not ported here.
        if is_shell_transition_window(window) {
            return Err("a shell window that only stages a switch");
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
        // return True` does (Dickson, 2026-10-20). A menu opening
        // redirected to the current focus is judged by the focus's
        // element, whose keyboard focus no cache of this event holds, so
        // it is let through.
        let lacked_by_its_event =
            || std::ptr::eq(subject, element) && sender.lacked_keyboard_focus();
        if out_of_time() {
            if lacked_by_its_event() {
                return Err(
                    "its event says it did not have the keyboard focus, and its read's time was up",
                );
            }
        } else {
            budget.call("the keyboard focus");
            if let Ok(false) = subject.has_keyboard_focus() {
                return Err("its element no longer has the keyboard focus");
            }
        }
        Ok(window)
    }

    /// Step 2: whether the sender, `held`, whose cache reads as `sender`, is
    /// native UIA (`isNativeUIAElement`, `UIAHandler/__init__.py` 1515 to
    /// 1575): its nearest window, its own else the nearest ancestor's
    /// (`getNearestWindowHandle`, one call), must use UIA as NVDA decides
    /// it (`isUIAWindow`: the class lists, then the window's own provider,
    /// asked with the raw probe),
    /// or NVDA ignores the event ("Ignoring for non native element") and
    /// the window's MSAA events speak for it. A sender with no window
    /// anywhere above it, or whose walk to one fails, is not native either,
    /// and NVDA's object for it could not be made without a window ("no
    /// windowHandle", `NVDAObjects/UIA/__init__.py` 1598 to 1606). Windows
    /// Terminal's second identity of its terminal can be such an element,
    /// its island not yet joined to its window (section 16.8). NVDA's two
    /// exceptions, a `NetUI` container in a Word document and Internet
    /// Explorer 9's downloads list, are Office's and Internet Explorer's,
    /// and are not ported. Answers the nearest window.
    ///
    /// A console window with a provider is checked, as NVDA checks it, for
    /// the complete provider of current Windows, from the held sender
    /// rather than by the window's handle, a connection
    /// (`docs/design/listener-without-connections.md`, section 3, step 5):
    /// the sender is the console's text area, known by its cached
    /// automation id, whose cached Text pattern is read, or the console
    /// window's own element, under which the text area is found. Any other sender in a console window, of which
    /// none is known, gets no answer: its focus is dropped as not native,
    /// and the window is checked again at its next focus.
    ///
    /// The read's time, `budget`, is asked before each call here after the
    /// read's first, `called` saying whether one was made before this
    /// check; the time the window's verdict takes to decide, the raw
    /// probe's wait, is not counted. Once it is up, the window's verdict is still taken when it is
    /// known without a call, from the class lists or a kept verdict; only a
    /// window whose verdict needs the probe, and so perhaps the console
    /// check, drops the focus.
    fn native_window(
        &self,
        held: &Cached<ListenerEvent>,
        sender: &ListenerEvent,
        budget: &Budget,
        mut called: bool,
    ) -> Result<isize, &'static str> {
        const NOT_UIA: &str = "its window does not use UIA, so its MSAA events speak for it";
        let out_of_time = || budget.out_of_time();
        let element = held.live().element();
        let hwnd = sender.role.hwnd;
        let nearest = if hwnd == 0 {
            if called && out_of_time() {
                return Err("its read ran out of time before its window was looked up");
            }
            called = true;
            budget.call("the nearest window");
            let started = Instant::now();
            let nearest = verbatim_uia::nearest_window_handle(element);
            let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            tracing::debug!(
                elapsed_us,
                window = nearest,
                "UIA focus sender's nearest window looked up"
            );
            let nearest =
                nearest.ok_or("it has no window above it, so NVDA makes no object for it")?;
            self.window.learn(nearest);
            nearest
        } else {
            hwnd
        };
        // The probe and the console check are calls into the application
        // too; a verdict already known needs neither.
        if called && out_of_time() {
            return match known_verdict(&self.arbitrator, nearest) {
                Some(true) => Ok(nearest),
                Some(false) => Err(NOT_UIA),
                None => Err("its read ran out of time before its window was judged"),
            };
        }
        // The raw probe, not a connection: the listener connects to no
        // application's window (`docs/design/listener-without-connections.md`,
        // section 2).
        let automation_id = sender.automation_id.as_deref();
        // The console check follows the probe, and its time counts.
        let console = || {
            budget.probe_ended();
            budget.call("the console check");
            (automation_id == Some(verbatim_uia::CONSOLE_TEXT_AREA) || hwnd == nearest)
                .then(|| {
                    verbatim_uia::held_console_reports_formatting(
                        &self.uia,
                        held,
                        automation_id,
                        &out_of_time,
                    )
                })
                .flatten()
        };
        budget.verdict_begins();
        budget.call("the window's verdict, by the raw probe if it is not known");
        let uia = uses_uia(&self.arbitrator, nearest, console);
        budget.probe_ended();
        if !uia {
            return Err(NOT_UIA);
        }
        Ok(nearest)
    }

    /// Logs, at debug, what section 16.3 of `docs/design/focus-pipeline.md`
    /// asks to see of each read before the duplicate rule is built on
    /// `CompareElements`: the sender's runtime id, class name, framework,
    /// and own native window handle, `hwnd`, and whether UI Automation
    /// counts it as the same element as the current focus, the last focus
    /// any of the listener's readers accepted, in any application, while no
    /// newer focus has been queued (`same_as_current`, the duplicate rule's
    /// comparison, and a menu opening's with the focus). All but
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
        snapshot: fact_of(read.node.parts(element.runtime_id())),
    }
}

/// What the listener knows of a focus event when its callback queues it:
/// the marker it sent, and the trace and times its read or drop carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Meta {
    /// The marker sent for the event.
    pub(crate) marker: u64,
    /// The trace minted when the event arrived.
    pub(crate) trace: TraceId,
    /// When the event arrived, in milliseconds since the Unix epoch.
    pub(crate) observed_at_ms: u64,
    /// When it arrived, for the latency log.
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
    /// ([`WatchedRead`]); 0, the default, for none, and for an event that
    /// is never judged so.
    fn window(&self, _job: &Self::Job) -> isize {
        0
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
    /// The kind of event `job` is and the element it is of, by runtime id,
    /// for NVDA's UIA limiter rule: a newer event of one kind of one
    /// element replaces one still waiting ([`Readers::push`]). `None`, the
    /// default, for an event that is never replaced.
    fn key<'a>(&self, _job: &'a Self::Job) -> Option<(FocusEvent, &'a [i32])> {
        None
    }
    /// Releases a thread's reader as the thread ends, on that thread.
    fn finish(&self, reader: Option<Self::Reader>);
    /// Sends what became of an event of `pid`'s. Called under the readers'
    /// lock, so what one application's events became is sent in the order
    /// they arrived.
    fn outcome(&self, pid: Pid, meta: Meta, judged: Judged);
}

/// One event waiting for its read.
struct Queued<J> {
    /// Its place in the order every application's events were queued in
    /// ([`ReadersState::queued`]).
    place: u64,
    /// How many foreground events the listener's hook had delivered when
    /// it was queued ([`ReadersState::foregrounds`]).
    foregrounds: u64,
    meta: Meta,
    job: J,
}

/// The event the thread in charge of an application is reading.
struct Running {
    /// When the thread began it.
    started: Instant,
    meta: Meta,
    /// Its kind, a focus when the job has no key.
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
}

/// What [`Readers::when_idle`] runs.
pub(crate) type IdleWaiter = Box<dyn FnOnce() + Send>;

impl<J> ReadersState<J> {
    /// Runs each waiter for which every event queued before it has its
    /// outcome sent: no application has one of those waiting or being
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
                if app.running.is_some() {
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
    /// Wakes the watchdog when a read starts, and for the shutdown.
    changed: Condvar,
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
            }),
            changed: Condvar::new(),
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
            shared
                .reading
                .outcome(pid, meta, Judged::Dropped("the listener is shutting down"));
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
                    shared.reading.outcome(
                        pid,
                        waiting.meta,
                        Judged::Dropped("a newer focus event of its element replaced it"),
                    );
                } else {
                    kept.push_back(waiting);
                }
            }
            app.queue = kept;
        }
        state.queued += 1;
        let queued = Queued {
            place: state.queued,
            foregrounds: state.foregrounds,
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
            drop_waiting(shared, &mut state, pid);
        } else if app.abandoned >= ABANDONED_LIMIT {
            tracing::warn!(
                %pid,
                waiting = app.queue.len(),
                "a UIA focus waits: its application's reads have not returned"
            );
            // The watchdog bounds its wait.
            shared.changed.notify_all();
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
            app.running
                .as_ref()
                .is_some_and(|running| running.kind == FocusEvent::Focus)
                || (app.live.is_some() || Some(*owner) == pid)
                    && app.queue.iter().any(|queued| {
                        shared
                            .reading
                            .key(&queued.job)
                            .is_some_and(|(kind, _)| kind == FocusEvent::Focus)
                    })
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
    /// is under way that the watchdog has not yet set to be checked when
    /// its grace ends, or one past its grace, wakes the watchdog to judge
    /// again whether the user has moved on from it, or to check it when its
    /// grace ends. Otherwise it wakes no thread. Never waits on a read.
    pub(crate) fn foreground_changed(&self) {
        let mut state = self.shared.lock();
        state.foregrounds += 1;
        if state.wake_on_foreground && state.apps.values().any(|app| app.running.is_some()) {
            self.shared.changed.notify_all();
        }
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
        self.shared.changed.notify_all();
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
            for queued in app.queue.drain(..) {
                shared
                    .reading
                    .outcome(pid, queued.meta, Judged::Dropped("no thread to read it"));
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
        let judged = shared.reading.read(reader.as_mut(), meta, job, &window);
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
            shared.reading.outcome(pid, meta, judged);
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
            meta,
            job,
            ..
        }) = app.queue.pop_front()
        {
            let kind = shared
                .reading
                .key(&job)
                .map_or(FocusEvent::Focus, |(kind, _)| kind);
            // Only a read whose window is not yet known learns it, so only
            // it is given what to do then.
            let window = shared.reading.window(&job);
            let watched = WatchedRead::new(window, (window == 0).then(|| on_learn(shared)));
            app.running = Some(Running {
                started: Instant::now(),
                meta,
                kind,
                watched: watched.clone(),
                foregrounds,
            });
            app.running_place = place;
            shared.changed.notify_all();
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

/// What a read does once it learns its window ([`WatchedRead::learn`]):
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
        if !read.abandoned() && read.judge_front(|window| shared.reading.moved_on(window)) {
            let _state = shared.lock();
            shared.changed.notify_all();
        }
    })
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
/// thread, unless too many of its threads are abandoned already; and drops
/// an event that has waited the deadline since it was queued while its
/// application had no thread to read it.
///
/// Whether the user has moved on is judged once the read's window is
/// known, and at the end of its grace only once a foreground event has
/// come since the read's event was queued, or the window in front was
/// already another thread's when the read's window became known
/// ([`WatchedRead::judge_front`]): the evidence that the user may have
/// left its window. The watchdog wakes when a read starts and at its
/// deadline; at the end of its grace only with that evidence; when a read
/// learns its window and the window in front is another thread's
/// ([`on_learn`]); while an event waits with no thread, at the end of its
/// wait; and, while a read is under way that is past its grace, or within
/// it and not yet set to be checked when its grace ends, on each foreground
/// event ([`Readers::foreground_changed`]). So a read with no such evidence
/// costs the watchdog no wake-up besides its start's.
fn watchdog<R: Reading>(shared: &Arc<Shared<R>>) {
    let mut state = shared.lock();
    loop {
        if state.stopping {
            return;
        }
        let now = Instant::now();
        let foregrounds = state.foregrounds;
        let mut due: Option<Instant> = None;
        let mut wake_on_foreground = false;
        let mut abandoned = Vec::new();
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
            let next = if now < grace_end && armed {
                grace_end
            } else {
                wake_on_foreground = true;
                deadline
            };
            due = Some(due.map_or(next, |due| due.min(next)));
        }
        if !abandoned.is_empty() || !waited.is_empty() {
            for (pid, window, reason) in abandoned {
                abandon(shared, &mut state, pid, window, reason);
            }
            for pid in waited {
                drop_waiting(shared, &mut state, pid);
            }
            continue;
        }
        state.wake_on_foreground = wake_on_foreground;
        state = match due {
            Some(due) => {
                shared
                    .changed
                    .wait_timeout(state, due.saturating_duration_since(now))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
        };
    }
}

/// Drops every event of `pid`'s waiting with no thread to read it, its
/// application's abandoned threads at [`ABANDONED_LIMIT`] past its bound
/// ([`READ_DEADLINE`]), in the order they arrived: each drop resolves its
/// marker, so the application's later events in its outpost are not held
/// behind them.
fn drop_waiting<R: Reading>(shared: &Arc<Shared<R>>, state: &mut ReadersState<R::Job>, pid: Pid) {
    let Some(app) = state.apps.get_mut(&pid) else {
        return;
    };
    if app.live.is_some() || app.queue.is_empty() {
        return;
    }
    tracing::warn!(
        %pid,
        abandoned = app.abandoned,
        dropped = app.queue.len(),
        "UIA focuses waited past the bound behind their application's abandoned reads; they are dropped"
    );
    for queued in app.queue.drain(..) {
        shared
            .reading
            .outcome(pid, queued.meta, Judged::Dropped(WAITED_TOO_LONG));
    }
    state.wake_idle_waiters();
}

/// Abandons the thread reading `pid`'s event, of `window`, for `reason`:
/// it has run past the deadline, or the user has moved on from its window.
/// The read is marked abandoned before anything else, so that it accepts
/// no focus once a newer read can begin ([`LastAccepted::accept`]). The
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
    running.watched.abandon();
    shared.reading.abandoned(&running.meta);
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
    shared
        .reading
        .outcome(pid, running.meta, Judged::Dropped(reason));
    if !app.queue.is_empty() {
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
    /// `last_accepted`, as the listener's reading tells its own.
    struct Script {
        outcomes: Mutex<Sender<(Pid, u64, &'static str)>>,
        read_on: Mutex<Sender<(u64, ThreadId)>>,
        returned: Mutex<Sender<(u64, WatchedRead)>>,
        finished: Mutex<Sender<ThreadId>>,
        moved_on: AtomicBool,
        asked: Mutex<Sender<isize>>,
        hwnd: isize,
        nearest: isize,
        last_accepted: LastAccepted,
    }

    impl Reading for Script {
        /// The event's marker, its gate, and the element it is of, if
        /// any.
        type Job = (u64, Option<Receiver<()>>, Vec<i32>);
        type Reader = ThreadId;

        fn reader(&self, _: Pid) -> Option<ThreadId> {
            Some(thread::current().id())
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
            Judged::Dropped("read")
        }

        fn window(&self, _: &Self::Job) -> isize {
            self.hwnd
        }

        fn abandoned(&self, meta: &Meta) {
            self.last_accepted.abandoned(meta.trace);
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

        fn outcome(&self, pid: Pid, meta: Meta, judged: Judged) {
            let Judged::Dropped(reason) = judged else {
                panic!("the stand-in reads nothing");
            };
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
    }

    impl Fixture {
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
        Fixture {
            readers: Readers::new(
                Script {
                    outcomes: Mutex::new(outcomes_tx),
                    read_on: Mutex::new(read_tx),
                    returned: Mutex::new(returned_tx),
                    finished: Mutex::new(finished_tx),
                    moved_on: AtomicBool::new(false),
                    asked: Mutex::new(asked_tx),
                    hwnd,
                    nearest,
                    last_accepted: LastAccepted::default(),
                },
                deadline,
                grace,
            ),
            outcomes,
            read_on,
            returned,
            finished,
            asked,
        }
    }

    fn meta(marker: u64) -> Meta {
        Meta {
            marker,
            trace: TraceId::mint(),
            observed_at_ms: 0,
            timing: EventTiming::default(),
        }
    }

    const WAIT: Duration = Duration::from_secs(10);

    /// The time the window's verdict takes, the raw probe's wait, is not
    /// counted: a read whose probe waited 3 seconds still has its whole
    /// budget after it.
    #[test]
    fn a_read_s_budget_does_not_count_the_probe_s_wait() {
        let start = Instant::now();
        let time = Arc::new(Mutex::new(start));
        let read = Arc::clone(&time);
        let budget = Budget::new(Arc::new(move || {
            *read.lock().unwrap_or_else(PoisonError::into_inner)
        }));
        let set = |at: Duration| *time.lock().unwrap_or_else(PoisonError::into_inner) = start + at;
        set(Duration::from_millis(100));
        budget.verdict_begins();
        set(Duration::from_millis(3_100));
        budget.probe_ended();
        set(READ_BUDGET + Duration::from_millis(2_999));
        assert!(!budget.out_of_time());
        set(READ_BUDGET + Duration::from_secs(3));
        assert!(budget.out_of_time());
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

    /// The bound at [`ABANDONED_LIMIT`] is the application's: the events
    /// waiting while none of its abandoned threads returns are dropped
    /// together, in order, once the deadline has passed since the limit was
    /// reached, which resolves their markers, and an event pushed after
    /// that is dropped at once, as it is pushed. None of them is read.
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

    /// A focus a read accepted just before the watchdog abandoned it is no
    /// current focus once the watchdog has, since Core was told the event
    /// was dropped; and a read that returns after it was abandoned does not
    /// make its focus the last one accepted, so a newer focus accepted
    /// while it was held stays the last.
    #[test]
    fn a_read_returning_after_it_was_abandoned_accepts_no_focus() {
        const NEWER_WINDOW: isize = 0x8180;
        let uia = Uia::new().expect("a UIA client");
        let element = uia.root_element().expect("the desktop's root element");
        let fixture = fixture_with_grace(Duration::from_secs(60), Duration::from_millis(50));
        let last_accepted = fixture.readers.shared.reading.last_accepted.clone();
        let first = meta(1);
        let arrival = Arrival {
            trace: first.trace,
            observed_at_ms: 0,
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
        last_accepted.accept(1, &element, (WINDOW, arrival), &WatchedRead::default());
        assert_eq!(
            last_accepted.current().map(|(_, window)| window),
            Some(WINDOW)
        );
        fixture.set_moved_on(true);
        fixture.readers.foreground_changed();
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "the user moved on from a slow window")
        );
        assert!(
            last_accepted.current().is_none(),
            "the abandoned read's focus is no current focus"
        );
        open.send(()).expect("the held read waits");
        let (marker, abandoned) = fixture
            .returned
            .recv_timeout(WAIT)
            .expect("the read returns");
        assert_eq!(marker, 1);
        let newer = Arrival {
            trace: TraceId::mint(),
            observed_at_ms: 0,
        };
        last_accepted.accept(
            2,
            &element,
            (NEWER_WINDOW, newer),
            &WatchedRead::new(NEWER_WINDOW, None),
        );
        last_accepted.accept(1, &element, (WINDOW, arrival), &abandoned);
        assert_eq!(
            last_accepted.current().map(|(_, window)| window),
            Some(NEWER_WINDOW)
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
