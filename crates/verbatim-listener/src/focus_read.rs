//! The focus listener's judging and reading of each UIA focus event, from
//! the event's own element, on a thread for the element's application
//! (`books/dev/src/reference/adr/0001-focus.md`, "Who reads a UI Automation focus"; decision D13
//! as amended
//! on 2026-10-11).
//!
//! A UIA element cannot be handed from the process that received its event
//! to another (experiment E-marshal), so the listener, which receives every
//! focus event, is the one process that can judge a focus by its sender, as
//! NVDA does, and read it. Its callback drops a sender whose cached window
//! is hung, and otherwise only queues the sender for the thread of the
//! sender's application and returns; the thread, one per
//! application, created when the application's first focus event arrives,
//! does NVDA's flusher-thread work for each of its application's events in
//! arrival order ([`FocusReader::read`]):
//!
//! - NVDA's focus handler's checks, in its order, before anything of the
//!   sender is read, as the pure step function
//!   `verbatim_listener_logic::checks::next` names them, each call made
//!   here and its answer recorded (`FocusReader::judge_before_reading`;
//!   `books/dev/src/reference/adr/0001-focus.md`, "The listener's checks, ported from NVDA's focus
//!   handler"):
//!   one whose nearest window's
//!   class is on NVDA's MSAA class list, or that has none, is not native
//!   and is dropped, the window's verdict otherwise being its outpost's
//!   (`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact"); one whose
//!   nearest window fails NVDA's
//!   acceptance test is dropped; the console window's own element, which
//!   NVDA's `consoleUIAWindow` refuses, and a focus in Core's hidden frame
//!   are dropped; and a sender without the keyboard focus, read with a
//!   plain property read, is dropped, as NVDA's `shouldAllowUIAFocusEvent`
//!   drops it, a read that fails letting it through. A shell window that
//!   only stages a switch is read and goes to
//!   Core like any focus, and Core's rules decide what is said of it. A
//!   menu opening is judged by NVDA's automation event handler's checks
//!   instead, with a sender UI Automation counts as the thread's own last
//!   read, while that read is current, judged as that focus, with its
//!   window and without the native check. NVDA's duplicate test is not
//!   made: a focus that passes the other checks is read in full, and Core,
//!   which holds the focus, judges a repeat
//!   (`books/dev/src/explanation/design/ownership.md`);
//! - then the classic read, every call on an element the listener holds,
//!   with no connection to any window and no remote operation
//!   (`books/dev/src/explanation/design/listener-without-connections.md`, sections 2 and 3):
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
//! A hung window, by the listener's one window readability reader, which
//! its UIA callbacks and its reading threads' readers are given (NVDA tests
//! a UIA event's sender's cached window as it arrives,
//! `_shouldSkipEventForHungWindow`, `UIAHandler/utils.py` 438): a sender's
//! cached window found hung or ghosted as the event arrives drops it in
//! the callback, with no marker sent and nothing queued; and a step of the
//! read's walk that reaches a ghost's or a hung application's window ends
//! the walk there, the chain going on from that window as a plain window.
//! A read is never ended for its own window's hang: it waits for the
//! application, and its focus is spoken when the application answers
//! (Dickson, 2026-11-07). Neither reports a hang: only the outpost tells
//! Core.
//!
//! Time limits: a read is several calls, each on an element the listener
//! holds, each waiting for the application up to UIA's transaction
//! timeout, set once for the process (`verbatim_uia::UIA_TRANSACTION_TIMEOUT`).
//! A check whose call fails or times out lets the focus through, as NVDA's
//! do on a COM error; a step of the walk that fails or times out ends the
//! walk, and the chain goes on from the parent window of the outermost
//! element's nearest window, read live by the outpost, as NVDA's failed
//! parent read goes on by `Window._get_parent`; a read whose sender's own
//! read fails is reported from the event's cache the same way. A call over
//! a second is logged ([`CallLog`]).
//!
//! Each application's read under way is its work in flight in the
//! schedule (`verbatim_reader_rules::in_flight`,
//! `books/dev/src/reference/crates/verbatim-reader-rules.md`), which abandons a thread whose read runs past
//! [`FOCUS_READ_DEADLINE`], or, as NVDA's watchdog cancels a slow call once
//! the user has moved on (`docs/nvda/main-loop-and-watchdog.md`, "The
//! watchdog: detection"), whose read is still under way while the window
//! in front belongs to the read's application on another UI thread than
//! the window being read. That is judged at [`MOVED_ON_GRACE`](verbatim_reader_rules::MOVED_ON_GRACE) after the
//! read's start and every [`MOVED_ON_GRACE`](verbatim_reader_rules::MOVED_ON_GRACE) after, with no foreground
//! event as evidence, once the read's window is known, from the event's
//! cached window handle or from the sender's nearest window once the read
//! has found it ([`WatchedRead::learn`]); a selection or a notification is
//! never judged so. One process can own windows on several UI threads, as
//! File Explorer's folder windows, the taskbar and the Alt+Tab switcher
//! are all `explorer.exe`'s, and the application's focus on another thread
//! must not wait behind a slow read of the first. Either way the event is
//! dropped, a new thread takes the application's later events, and the
//! abandoned thread, once UIA gives up, says nothing more, forgets the
//! focus it read, which Core never got, and ends or reads on. An
//! application whose abandoned threads reach
//! [`ABANDONED_LIMIT`](verbatim_listener_logic::focus::ABANDONED_LIMIT) gets no new thread until one returns: its events
//! wait, a newer event of an element replacing its waiting one, and the
//! first abandoned thread whose call returns reads them, unless
//! [`FOCUS_READ_DEADLINE`] has passed since the application reached the limit,
//! when the watchdog drops every event waiting and each later one is
//! dropped as it arrives, until a thread returns (one bound per
//! application, which [`FOCUS_READ_DEADLINE`] states); an event waiting so
//! is a pending focus for its own application's menu openings only
//! ([`Readers::focus_pending`]). So the application's outpost, whose later
//! events wait behind each marker, runs at most that long behind the hung
//! reads, once, however many events the application raises meanwhile.
//!
//! The deadline does not bound the call under way: the transaction timeout
//! bounds each provider call, not a UIA operation
//! (`books/dev/src/reference/uia-bugs.md`, "A timeout bounds each request and
//! window message, not an operation"), so one operation that makes several
//! provider calls, as the nearest-window lookup's
//! `NormalizeElementBuildCache` does, can run past the deadline with each
//! of its provider calls answered within the timeout, and its read is then
//! abandoned though the application still answers.
//!
//! A thread keeps its focus before, NVDA's duplicate evidence, for as long
//! as its application has an outpost: it ends, once it has read every event
//! waiting, when the listener learns that the application's outpost has
//! ended ([`Readers::outpost_ended`]) or that its process has exited
//! ([`Readers::application_exited`]), and at the listener's shutdown
//! (`books/dev/src/reference/crates/verbatim-listener.md`, "How a UIA focus is judged and read").
//! An event that arrives
//! after that, for an application still running, starts a new thread.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_LegacyIAccessibleStatePropertyId,
};

use verbatim_model::{FOCUS_READ_DEADLINE, NodeDetails, Pid, State, TraceId};
use verbatim_uia::map::{
    CachedUiaParts, add_legacy_checked_state, is_toast_window_class, wants_legacy_checked_state,
};
use verbatim_uia::{
    Ancestor, AncestorGates, Cached, ElementExt as _, Fill, FocusInput, ListenerEvent,
    ListenerSelection, Node, NodeGates, Shape, Uia,
};
use verbatim_uia_rops::{Ancestry, FocusAncestry, FocusQuery, Progress, focus_ancestry_classic};

use verbatim_listener_logic::checks::{self, Check, Gathered, Nearest, Step};
use verbatim_listener_logic::focus::{
    Arrival, FocusEvent, Judged, Meta, NO_WINDOW, NOT_FOCUSED, NOT_THE_LAST_READ,
    NOTIFICATION_NO_WINDOW, TOAST_NOT_CORE_WINDOW, TOAST_NOT_READ, arrival_of,
};
use verbatim_listener_logic::schedule::{
    AlarmChange, AppKey, QueuePlace, Request, Schedule, ScheduleEffect, ScheduleInput, Waking,
};
use verbatim_model::protocol::{
    AncestorsEnd, DeliveredFact, UiaAncestorFact, UiaFocusRead, UiaSnapshotFact,
};
use verbatim_process::alarm::Alarm;
use verbatim_process::clock::Clock;
use verbatim_process::clock::now_us;
use verbatim_reader_rules::Window;
use verbatim_reader_rules::acceptance::{EventName, RequestedEvents, accepts_event};
use verbatim_reader_rules::ancestry_cache::{CacheUse, SavedTitle, title_check};
use verbatim_reader_rules::in_flight::{Reason, RuleWindow};
use verbatim_win32::window::ForegroundReader;
use verbatim_win32::window::SystemWindows;
use verbatim_win32::window::window_class_name;
use verbatim_win32::window::{Readability, WindowReadabilityReader};
use verbatim_win32::window::{
    front_is_another_thread_of_its_application, top_level_of, window_belongs_to_hidden_frame,
};

/// The most ancestors read for a focus, as many as the outpost walks.
const MAX_ANCESTORS: u32 = 64;

/// The log of one focus read's calls, for diagnosis alone: it times each
/// call by the system's clock, and a call that takes longer than
/// [`SLOW_CALL`] is logged at info with its name and how long it took, so
/// the slowest calls of a read can be seen (Dickson, 2026-10-20). Each
/// call costs one clock reading, at its start, which is also the end of
/// the call before it; the last ends when the log is dropped, at the
/// read's end. It ends nothing.
#[derive(Default)]
struct CallLog {
    /// The call under way, by name, and when it began.
    call: Cell<Option<(&'static str, Instant)>>,
}

/// How long one call of a focus read may take before it is logged at info.
const SLOW_CALL: Duration = Duration::from_secs(1);

impl CallLog {
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
}

impl Drop for CallLog {
    /// The read has ended, and with it its last call.
    fn drop(&mut self) {
        if self.call.get().is_some() {
            self.call_ended(Instant::now());
        }
    }
}

/// What a read does once it learns its window ([`WatchedRead::learn`]), or
/// its top-level window ([`WatchedRead::place`]): tells the schedule, under
/// the readers' lock.
type OnLearn = Box<dyn Fn(isize) + Send + Sync>;

/// One read as its thread and the readers' shell share it. Each read has
/// its own, so an abandoned read that finds its window later changes
/// nothing of another's.
///
/// Its request, by which the schedule's work in flight names it
/// ([`Request`]): what it learns and its return are given to the schedule
/// under it, and the schedule refuses them once the read is no longer the
/// one under way.
///
/// The window it is of: the sender's own window from the event's cache as
/// the read starts, for a focus ([`Reading::window`]), and the sender's
/// nearest window once the read of a focus whose sender is not a window has
/// found it ([`learn`](Self::learn)), which the schedule is told; 0 while
/// neither is known, and always for a selection or a notification. The
/// schedule judges from it whether the user has moved on
/// ([`MOVED_ON_GRACE`](verbatim_reader_rules::MOVED_ON_GRACE)). The read's
/// top-level window, once found, the schedule alone keeps, for menu
/// currentness ([`Readers::foreground_changed`]).
#[derive(Clone, Default)]
pub(crate) struct WatchedRead(Arc<Watched>);

#[derive(Default)]
struct Watched {
    request: Option<Request>,
    window: AtomicIsize,
    /// What it does on learning its window, and on finding its top-level
    /// window; `None` for a read never judged.
    reports: Option<(OnLearn, OnLearn)>,
}

impl WatchedRead {
    /// The read `request` of window `hwnd`, 0 for one not yet known, which
    /// does the first of `reports` once it learns its window and the second
    /// once it finds its top-level window.
    fn new(request: Request, hwnd: isize, reports: Option<(OnLearn, OnLearn)>) -> Self {
        Self(Arc::new(Watched {
            request: Some(request),
            window: AtomicIsize::new(hwnd),
            reports,
        }))
    }

    /// The read has found that it is of window `hwnd`.
    pub(crate) fn learn(&self, hwnd: isize) {
        self.0.window.store(hwnd, Ordering::SeqCst);
        if let Some((on_learn, _)) = &self.0.reports {
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

    /// The read has found its window's top-level window, `top`, which the
    /// schedule is told: once per read.
    fn place(&self, top: isize) {
        if let Some((_, on_place)) = &self.0.reports {
            on_place(top);
        }
    }

    /// Whether `other` is this read.
    fn same(&self, other: &WatchedRead) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What a UIA focus read has read so far, which its classic walk adds to
/// as it reads ([`FocusQuery::progress`]): the event's cached facts, the
/// facts read so far, the ancestors read and the nearest window. A read
/// whose walk fails is reported from it.
struct ReadProgress {
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
}

impl ReadProgress {
    /// Takes in what the classic walk has just read.
    fn take_in(&mut self, read: Progress<'_>) {
        match read {
            Progress::PreviousFocused(focused) => self.same_as_last = Some(focused),
            Progress::Element(fresh, table) => {
                let read = fresh.read();
                let mut parts = read.node.parts(fresh.runtime_id());
                parts.details.set_table(table);
                self.fresh = Some((parts, read.node.value_from_selection()));
            }
            Progress::SelectedChild(child, table) => {
                let mut parts = child.read().node.parts(child.runtime_id());
                parts.details.set_table(table);
                self.selected_child = Some(fact_of(parts));
            }
            Progress::Ancestor(ancestor) => {
                if self.ancestors.is_empty() {
                    self.parent_automation_id = ancestor.read().automation_id;
                }
                self.ancestors.push(ancestor_fact(ancestor));
            }
        }
    }

    /// The parent window of the nearest window of the outermost element
    /// read, the focus or an ancestor, where the chain goes on after a
    /// failure, as NVDA's failed parent read goes on by `Window._get_parent`
    /// (`NVDAObjects/window/__init__.py` 287 to 293), the nearest window
    /// itself never in the chain. Local calls.
    fn continue_from(&self) -> isize {
        verbatim_uia::parent_window(verbatim_uia::outermost_nearest_window(
            self.nearest,
            std::iter::once(self.hwnd).chain(self.ancestors.iter().map(|ancestor| ancestor.hwnd)),
        ))
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

/// The focus a reading thread read last: the evidence that a runtime id
/// now names another element (`same_as_last`), where the ancestor read
/// stops, and, while it is current, what a menu opening is compared with
/// (`books/dev/src/explanation/design/ownership.md`). It is the
/// thread's own fact: no judgement of Core's is kept, so a focus Core
/// refused can be the last read, and Core decides what is spoken.
struct LastFocus {
    runtime_id: Vec<i32>,
    element: IUIAutomationElement,
    /// The element as its event's cache holds it, which a live re-read
    /// walks from ([`FocusReader::live_reread`]).
    input: Cached<FocusInput>,
    /// The fact of its event's cache: its name and role.
    event: UiaSnapshotFact,
    /// Its own window from its event's cache, 0 for none.
    hwnd: isize,
    /// Its ancestry cache: its raw-view ancestors, nearest first, or `None`
    /// when its read left them unknown: the next read then reuses nothing
    /// of this focus.
    ancestry_cache: Option<Vec<UiaAncestorFact>>,
    /// The window text of each window entry of the ancestry cache, read by
    /// `InternalGetWindowText` when the entry was read live, which the
    /// title check reads again before the cache is reused.
    titles: Vec<SavedTitle>,
    /// The window it was judged with, NVDA's object's `windowHandle`.
    window: isize,
    /// Its event's place in the order every event was queued in, by which
    /// a later menu opening tells whether it is still current
    /// ([`QueuePlace::current_since`]).
    place: u64,
    /// Its event's trace, by which Core's live re-read names it.
    trace: TraceId,
    /// When its event was received, on the run's shared clock, which its
    /// live re-read's report keeps.
    observed_at_us: u64,
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
    /// Whether a test has this reader skip no focus by its window's class
    /// ([`forces_uia`](Self::forces_uia)).
    forced_uia: AtomicBool,
    /// The read under way as the reading threads' watchdog watches it,
    /// told a focus's sender's nearest window once the read finds it
    /// ([`judge_watched`](Self::judge_watched)).
    window: WatchedRead,
    /// Told the name of each call of the classic walk before it is made,
    /// when a test asks ([`observe_calls`](Self::observe_calls)).
    calls: Option<CallObserver>,
    /// Judges whether a window can be read, or is a ghost's or a hung
    /// application's: each window a step of the classic walk reaches. The
    /// system's judgement, unless the listener gives its own
    /// ([`set_window_readability_reader`](Self::set_window_readability_reader)).
    window_readability: WindowReadabilityReader,
}

/// Told the name of each call a focus read's classic walk is about to make
/// ([`FocusReader::observe_calls`]).
pub type CallObserver = Arc<dyn Fn(&'static str) + Send + Sync>;

impl FocusReader {
    /// A reader with a UIA client of its own, made on the calling thread,
    /// which joins COM's multithreaded apartment. Every read is the classic
    /// walk, which makes no remote operation
    /// (`books/dev/src/explanation/design/listener-without-connections.md`, section 2).
    ///
    /// # Errors
    ///
    /// The COM error when the client cannot be made.
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            uia: Uia::new()?,
            last: None,
            foreground: Arc::new(verbatim_win32::window::foreground_window_handle),
            forced_uia: AtomicBool::new(false),
            window: WatchedRead::default(),
            calls: None,
            window_readability: Arc::new(verbatim_win32::window::readability),
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

    /// Judges with `reader` from now on, in place of the system, whether
    /// each window a step of its reads' classic walk reaches cannot be
    /// read, as an outpost does (`Outpost::set_window_readability_reader`):
    /// the listener gives each reading thread its own one reader, and a
    /// test that cannot wait for Windows to judge an application hung says
    /// which window is.
    pub fn set_window_readability_reader(&mut self, reader: WindowReadabilityReader) {
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
    /// came as `arrival`. Every call waits for the application, up to UIA's
    /// transaction timeout; nothing of the listener's own ends the read
    /// early but the schedule's deadline and moved-on rule.
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
        let calls = CallLog::default();
        let judged = self.judge_before_reading(
            sender,
            (&fields, &event.runtime_id),
            (kind, arrival.queue()),
            &calls,
        );
        // The focus's top-level window, once its window is known, for menu
        // currentness: the schedule's record of where each application's
        // read is. A local call.
        let known = match judged {
            Ok((window, _)) => window,
            Err(_) if hwnd != 0 => hwnd,
            Err(_) => self.window.window(),
        };
        if let Some(top) = Some(known)
            .filter(|&window| window != 0)
            .map(top_level_of)
            .filter(|&top| top != 0)
        {
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
        // ancestor read stops ([`FocusQuery::known`]), its ancestry cache,
        // used only while every window title saved with it reads the same
        // now (the title check, local calls). A focus before whose
        // ancestors are unknown is left out, itself included, since the
        // chain reused from where the read meets it would end there with
        // nothing above it: the read then runs as a first read does, and
        // so does a read whose ancestry cache fails the title check, which
        // walks live to the top or the process boundary.
        let known: Vec<Vec<i32>> = self
            .last
            .as_ref()
            .and_then(|last| {
                let ancestors = last.ancestry_cache.as_ref()?;
                (title_check(&last.titles, verbatim_win32::window::live_title) == CacheUse::Reuse)
                    .then(|| {
                        std::iter::once(last.runtime_id.clone())
                            .chain(
                                ancestors
                                    .iter()
                                    .map(|ancestor| ancestor.snapshot.runtime_id.clone()),
                            )
                            .collect()
                    })
            })
            .unwrap_or_default();
        let input = sender.narrow::<FocusInput>();
        self.walk(
            (&input, element, fact_of(event)),
            (hwnd, nearest),
            (&known, previous.as_ref(), require_focus),
            (arrival, &calls),
        )
    }

    /// Core's live re-read `request` of the read traced `trace`: the held
    /// element of this thread's last read, read again by the classic walk,
    /// one call per ancestor, with no ancestry cache and no evidence of an
    /// earlier element, its name and role from its event's cache as its
    /// first read had them. The read refreshes the ancestry cache and keeps
    /// the last read's place, its menu currency, since it is not a new
    /// event. Answered [`NOT_THE_LAST_READ`] when the thread holds no read,
    /// or a later one.
    pub fn live_reread(&mut self, request: u64, trace: TraceId) -> Judged {
        let Some(last) = self.last.as_ref().filter(|last| last.trace == trace) else {
            return Judged::Dropped(NOT_THE_LAST_READ);
        };
        let input = last.input.clone();
        let element = last.element.clone();
        let event = last.event.clone();
        let (hwnd, nearest) = (last.hwnd, last.window);
        let arrival = Arrival {
            trace,
            observed_at_us: last.observed_at_us,
            place: last.place,
            left_at: 0,
        };
        let calls = CallLog::default();
        let judged = self.walk(
            (&input, &element, event),
            (hwnd, nearest),
            (&[], None, false),
            (arrival, &calls),
        );
        match judged {
            Judged::Read(mut read) => {
                read.answers = Some(request);
                Judged::Read(read)
            }
            other => other,
        }
    }

    /// The walk of a focus read, its query and outcome: `input`, the
    /// focus's element as its event's cache holds it, `element` its live
    /// element and `event` the fact of its event's cache; `hwnd` its own
    /// window, `nearest` its nearest; the walk stopping at `known`, the
    /// ancestry cache's runtime ids, reading whether `previous` still has
    /// the keyboard focus, and the sender's own keyboard focus when
    /// `require_focus`; `arrival` the event it came in.
    fn walk(
        &mut self,
        (input, element, event): (&Cached<FocusInput>, &IUIAutomationElement, UiaSnapshotFact),
        (hwnd, nearest): (isize, isize),
        (known, previous, require_focus): (&[Vec<i32>], Option<&IUIAutomationElement>, bool),
        (arrival, calls): (Arrival, &CallLog),
    ) -> Judged {
        // What the walk reads, kept as it reads it, so a read whose walk
        // fails is reported from what it had read.
        let progress = RefCell::new(ReadProgress {
            hwnd,
            nearest,
            event,
            fresh: None,
            selected_child: None,
            ancestors: Vec::new(),
            parent_automation_id: None,
            same_as_last: None,
        });
        // Every detail: the listener reads for every application, whatever
        // theme decides which details are spoken. A request that cannot be
        // built is a read that failed.
        // The ancestors are read without their place or their cell, as
        // every read of many elements is, and the focus with them
        // (`verbatim_uia::Place`, `verbatim_uia::Cell`).
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
                // (`books/dev/src/explanation/design/listener-without-connections.md`, section 3,
                // step 8).
                let query = FocusQuery {
                    element: input,
                    known,
                    previous,
                    depth_limit: MAX_ANCESTORS,
                    fill: &fill,
                    element_fill: &element_fill,
                    require_focus,
                    refresh: true,
                    nearest_window: Some(nearest),
                    before_call: Some(&|call| {
                        calls.call(call);
                        if let Some(observer) = &self.calls {
                            observer(call);
                        }
                    }),
                    // Read from the element's fresh copy.
                    value_available: None,
                    // Read from the element's fresh copy.
                    is_cell: None,
                    // A ghost's or a hung application's window that a step
                    // reaches ends the walk, by the system's judgement: local
                    // calls.
                    plain: Some(&|window| {
                        (self.window_readability)(window)
                            != verbatim_win32::window::Readability::Readable
                    }),
                    progress: Some(&|read| progress.borrow_mut().take_in(read)),
                };
                focus_ancestry_classic(&self.uia, &query)
            });
        let progress = progress.into_inner();
        match read {
            Ok(FocusAncestry::NotFocused) => Judged::Dropped(NOT_FOCUSED),
            Ok(FocusAncestry::Focused(ancestry)) => {
                self.focused((element, input), progress, &ancestry, (arrival, calls))
            }
            Err(error) => {
                // The sender's own read failed, by UIA's transaction
                // timeout or otherwise: the focus is reported as read so
                // far, the event's cache giving what was not, with the
                // ancestors read before the failure, and the chain goes on
                // from the parent window of the outermost one's nearest
                // window, which the outpost reads live by that window's own
                // object, as NVDA's object walks its parent by
                // `Window._get_parent` (Dickson, 2026-11-07).
                tracing::debug!(%error, "a UIA focus could not be read; reported from what was read");
                self.last = Some(LastFocus {
                    runtime_id: progress.event.runtime_id.clone(),
                    element: element.clone(),
                    input: input.clone(),
                    event: progress.event.clone(),
                    hwnd,
                    ancestry_cache: None,
                    titles: Vec::new(),
                    window: nearest,
                    place: arrival.place,
                    trace: arrival.trace,
                    observed_at_us: arrival.observed_at_us,
                });
                let (snapshot, selected_child, value_from_selection) = focus_of(&progress);
                Judged::Read(Box::new(UiaFocusRead {
                    hwnd,
                    snapshot,
                    ancestors_end: AncestorsEnd::ContinueFrom(progress.continue_from()),
                    ancestors: progress.ancestors,
                    selected_child,
                    value_from_selection,
                    window: Some(nearest),
                    same_as_last: progress.same_as_last,
                    cached_from: None,
                    answers: None,
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
            DeliveredFact::UiaToast { hwnd, snapshot } => self.read_toast(sender, hwnd, snapshot),
            other => Judged::Passed(Box::new(other)),
        }
    }

    /// The toast candidate whose sender is `sender`, its own window `hwnd`,
    /// 0 for none: the rest of NVDA's rule, and the read of what its event's
    /// cache did not carry. Its nearest window is found, one call when it has
    /// no window of its own, as a XAML toast's element has none, and must be
    /// of the class NVDA's rule names, a local call. It is then read for its
    /// value, states, description and position in its set, one call, since
    /// the registration's cache request is kept small, being filled for
    /// every window opened anywhere. Dropped at any failure, as NVDA, having
    /// no object for the toast, says nothing; every drop is logged.
    fn read_toast(
        &self,
        sender: &Cached<ListenerEvent>,
        hwnd: isize,
        snapshot: UiaSnapshotFact,
    ) -> Judged {
        let window = if hwnd == 0 {
            verbatim_uia::nearest_window_handle(sender.live().element())
        } else {
            Some(hwnd)
        };
        let Some(window) = window else {
            tracing::debug!(runtime_id = ?snapshot.runtime_id, "a toast candidate has no window above it");
            return Judged::Dropped(NO_WINDOW);
        };
        let class = window_class_name(window);
        if !is_toast_window_class(&class) {
            tracing::debug!(
                window,
                class,
                "a toast candidate's nearest window is of another class"
            );
            return Judged::Dropped(TOAST_NOT_CORE_WINDOW);
        }
        let read = self
            .uia
            .request_fixed::<ListenerSelection>()
            .and_then(|request| sender.live().refresh(&request));
        match read {
            Ok(toast) => Judged::Passed(Box::new(DeliveredFact::UiaToast {
                hwnd: window,
                snapshot: {
                    let parts = toast.read().parts(snapshot.runtime_id);
                    UiaSnapshotFact {
                        runtime_id: parts.runtime_id,
                        role: parts.role,
                        // Its name as the event carried it, as NVDA's
                        // object for the event reads it.
                        name: snapshot.name,
                        value: parts.value,
                        states: parts.states,
                        details: parts.details,
                    }
                },
            })),
            Err(error) => {
                tracing::debug!(%error, "a toast could not be read");
                Judged::Dropped(TOAST_NOT_READ)
            }
        }
    }

    /// The read of a focus whose element had the keyboard focus, from what
    /// the read kept as it read it, `progress`, and how its walk ended,
    /// `ancestry`: its name and role from the event's cache, as NVDA reads
    /// them from the event's element, and its value, states, and details
    /// afresh ([`focus_of`]); its ancestors as read, followed by those of
    /// the focus before above the one the read stopped at. `arrival` is
    /// the event it came in, and `calls` the read's call log.
    fn focused(
        &mut self,
        (element, input): (&IUIAutomationElement, &Cached<FocusInput>),
        mut progress: ReadProgress,
        ancestry: &Ancestry,
        (arrival, calls): (Arrival, &CallLog),
    ) -> Judged {
        let path = "classic";
        let (hwnd, nearest) = (progress.hwnd, progress.nearest);
        // A walk whose step to a parent failed, by UIA's transaction
        // timeout or any other error, or found none below the root, goes on
        // from the parent window of its outermost element's nearest window,
        // as NVDA's failed parent read goes on by `Window._get_parent`
        // rather than ending the chain; one that stopped at a step into a
        // window read as a plain window, a ghost's or a hung application's,
        // goes on from that window, which the outpost reads as a plain
        // window, and on by the window hierarchy, as NVDA goes on from its
        // plain window; and one cut at the depth limit is completed by
        // plain windows from its outermost element's nearest window, or
        // from its parent window when the outermost element is its own,
        // since that window was never read. The listener connects to none:
        // it names the window, and the outpost goes on from it. Local
        // calls, made only for a walk that did not reach the top.
        let ancestors_end = if let Some(plain) = ancestry.plain_window {
            AncestorsEnd::ContinueFrom(plain)
        } else if ancestry.failed_hop.is_some() {
            AncestorsEnd::ContinueFrom(progress.continue_from())
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
            failed_hop = ?ancestry.failed_hop,
            window = ?ancestry.window,
            "UIA focus ancestors read"
        );
        let (snapshot, selected_child, value_from_selection) = focus_of(&progress);
        let tail = match (ancestry.met_known, &self.last) {
            (
                Some(met),
                Some(LastFocus {
                    ancestry_cache: Some(before),
                    ..
                }),
            ) => cached_ancestors(before, met),
            _ => Vec::new(),
        };
        let live = progress.ancestors.len();
        let cached_from = (!tail.is_empty()).then(|| u32::try_from(live).unwrap_or(u32::MAX));
        let titles = self.saved_titles(&progress.ancestors, &tail);
        let ancestors = std::mem::take(&mut progress.ancestors)
            .into_iter()
            .chain(tail)
            .collect::<Vec<_>>();
        let mut read = UiaFocusRead {
            hwnd,
            snapshot,
            ancestors,
            ancestors_end,
            selected_child,
            value_from_selection,
            // The classic walk's steps go through the client side, whose
            // window proxy gives a window's element its handle and title,
            // the console window's included (16.4), and it walks to the
            // desktop's root or the process boundary itself; the window is
            // the nearest window found for the acceptance test when the
            // walk named none.
            window: ancestry.window.or(Some(nearest)),
            same_as_last: ancestry.previous_focused,
            cached_from,
            answers: None,
        };
        log_ancestor_facts(path, &read.ancestors);
        if wants_legacy_checked_state(read.snapshot.role, read.snapshot.states) {
            // A menu item without the Toggle pattern: its checked state may
            // be only in its legacy MSAA state, which the classic walk
            // leaves to this one live call.
            calls.call("the legacy MSAA state");
            let legacy = element
                .current_i32_ignoring_default(UIA_LegacyIAccessibleStatePropertyId)
                .ok()
                .flatten();
            add_legacy_checked_state(&mut read.snapshot.states, legacy);
        }
        // Only a chain that reaches the top is where a later read may stop
        // and reuse the rest, since the outpost goes on from a cut one.
        let top = read.ancestors_end == AncestorsEnd::Top;
        self.last = Some(LastFocus {
            runtime_id: read.snapshot.runtime_id.clone(),
            element: element.clone(),
            input: input.clone(),
            event: progress.event.clone(),
            hwnd,
            ancestry_cache: top.then(|| read.ancestors.clone()),
            titles: if top { titles } else { Vec::new() },
            window: nearest,
            place: arrival.place,
            trace: arrival.trace,
            observed_at_us: arrival.observed_at_us,
        });
        Judged::Read(Box::new(read))
    }

    /// The window texts saved with a new ancestry cache, whose ancestors
    /// read live are `live` and those taken from the cache `tail`: those of
    /// the window entries read live, read now, and those of the entries
    /// taken from the cache, as saved with them. Local calls.
    fn saved_titles(&self, live: &[UiaAncestorFact], tail: &[UiaAncestorFact]) -> Vec<SavedTitle> {
        let before = self.last.as_ref().map_or(&[][..], |last| &last.titles[..]);
        live.iter()
            .filter(|ancestor| ancestor.hwnd != 0)
            .map(|ancestor| {
                (
                    ancestor.hwnd,
                    verbatim_win32::window::window_text(ancestor.hwnd),
                )
            })
            .chain(
                tail.iter()
                    .filter(|ancestor| ancestor.hwnd != 0)
                    .filter_map(|ancestor| {
                        before
                            .iter()
                            .find(|(window, _)| *window == ancestor.hwnd)
                            .cloned()
                    }),
            )
            .collect()
    }

    /// What a focus event goes through before anything of its sender,
    /// `held`, whose cache reads as `sender` and whose runtime id is
    /// `runtime_id`, is read: NVDA's focus handler's checks, or for a menu
    /// opening (`kind`) its automation event handler's, in their order, as
    /// `verbatim_listener_logic::checks::next` names them: this makes each
    /// call it names and records the answer, until it drops the event or
    /// lets it through. Every check reads live; one whose call fails or
    /// times out lets the focus through, as NVDA's do on a COM error.
    ///
    /// The current focus a menu opening is compared with is this reading
    /// thread's own last read while that read is current for the opening,
    /// `queue`: nothing moved away from the application since that read's
    /// event was queued ([`QueuePlace::current_since`];
    /// `books/dev/src/explanation/design/ownership.md`). Its drop while a
    /// focus is pending is made as it arrives (`Readers::focus_pending`).
    ///
    /// Answers the window the focus is judged with, and whether the read's
    /// walk is to read the sender's keyboard focus, step 6, after the
    /// earlier element's, or why the event is dropped.
    fn judge_before_reading(
        &self,
        held: &Cached<ListenerEvent>,
        (sender, runtime_id): (&ListenerEvent, &[i32]),
        (kind, queue): (FocusEvent, QueuePlace),
        calls: &CallLog,
    ) -> Result<(isize, bool), &'static str> {
        let element = held.live().element();
        let current = self
            .last
            .as_ref()
            .filter(|last| kind == FocusEvent::MenuOpened && queue.current_since(last.place));
        let mut gathered = Gathered::new(
            kind,
            sender.role.hwnd,
            current.map(|last| last.window),
            self.forced_uia.load(Ordering::Relaxed),
        );
        loop {
            let check = match checks::next(&gathered) {
                Step::Read {
                    window,
                    require_focus,
                } => {
                    Self::log_sender(sender, runtime_id, gathered.compared);
                    return Ok((window, require_focus));
                }
                Step::Drop(reason) => {
                    Self::log_sender(sender, runtime_id, gathered.compared);
                    return Err(reason);
                }
                Step::Call(check) => check,
            };
            // A check is asked of the current focus only when there is one.
            let current_focus = || current.expect("a check of the current focus with one");
            match check {
                // NVDA's automation event handler (`UIAHandler/__init__.py`
                // 806 to 824) compares the sender with its focus object's
                // element (`compareElements` with `api.getFocusObject()`)
                // before anything else; NVDA catches no error from it.
                Check::Comparison => {
                    calls.call("the comparison with the current focus");
                    gathered.compared = Some(
                        self.uia
                            .compare_elements(element, &current_focus().element)
                            .map_err(drop),
                    );
                }
                // Step 2, the native check: the sender's nearest window,
                // one call (`getNearestWindowHandle`), which a lookup that
                // timed out or found none drops the focus by.
                Check::NearestWindow => {
                    calls.call("the nearest window");
                    let started = Instant::now();
                    let nearest = verbatim_uia::nearest_window_lookup(element);
                    let elapsed_us =
                        u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                    tracing::debug!(
                        elapsed_us,
                        window = ?nearest,
                        "UIA focus sender's nearest window looked up"
                    );
                    gathered.nearest = Some(match nearest {
                        Ok(Some(nearest)) => {
                            self.window.learn(nearest);
                            Nearest::Found(nearest)
                        }
                        Ok(None) => Nearest::None,
                        Err(_) => Nearest::TimedOut,
                    });
                }
                // The window's classes for the class verdict every
                // outpost's arbitrator gives: local calls.
                Check::Classes(window) => {
                    gathered.classes = Some(verbatim_win32::window::window_classes(window));
                }
                // 4. NVDA's acceptance test against the same window
                // (`shouldAcceptEvent`, `UIAHandler/__init__.py` 924 to 930,
                // and for a menu opening 825 to 837; sections 16.2 and
                // 16.3): local calls, timed apart from the call before.
                Check::Acceptance(window) => {
                    calls.call("the acceptance test and the window checks");
                    let windows = SystemWindows {
                        foreground: (self.foreground)(),
                    };
                    let accepted = accepts_event(
                        EventName::GainFocus,
                        Window::new(window),
                        &windows,
                        &RequestedEvents::default(),
                    );
                    if !accepted {
                        tracing::debug!(
                            window,
                            foreground = windows.foreground,
                            "UIA focus not related to the foreground window"
                        );
                    }
                    gathered.accepted = Some(accepted);
                }
                // 5. The console host's window element, whose focus events
                // are never the focus (`consoleUIAWindow`,
                // `NVDAObjects/UIA/winConsoleUIA.py` 358): a local call.
                Check::OwnClass(window) => {
                    gathered.own_class = Some(window_class_name(window));
                }
                // A focus in Core's hidden frame, which its outpost drops
                // whatever it is: a local read of the window's property.
                Check::HiddenFrame(window) => {
                    gathered.hidden_frame = Some(window_belongs_to_hidden_frame(window));
                }
                // 6. A menu opening redirected to the current focus is
                // judged by the focus's element, whose keyboard focus no
                // cache of this event holds: a plain property read, a
                // failed read letting it through.
                Check::KeyboardFocus => {
                    calls.call("the keyboard focus");
                    gathered.keyboard_focus =
                        Some(current_focus().element.has_keyboard_focus().map_err(drop));
                }
            }
        }
    }

    /// Logs, at debug, what the "How a UIA focus is judged and read" section of
    /// `books/dev/src/reference/crates/verbatim-listener.md` records of each read: the sender's runtime id, class name,
    /// framework, and own native window handle, `hwnd`, and, for a menu
    /// opening compared with the thread's current last read, whether UI
    /// Automation counts it as the same element (`same_as_current`, `Err`
    /// for a comparison that failed). All but the comparison come from the
    /// event's cache, as NVDA's debug strings do; nothing is read live for
    /// the log.
    fn log_sender(
        sender: &ListenerEvent,
        runtime_id: &[i32],
        same_as_current: Option<Result<bool, ()>>,
    ) {
        if !tracing::enabled!(tracing::Level::DEBUG) {
            return;
        }
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
/// the read that produced them (`books/dev/src/reference/crates/verbatim-outpost.md`, "Console
/// windows and frames of another process": where a console window element is lost).
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

/// The cached ancestors: those of the ancestry cache, the focus before's
/// ancestors, that lie above the one the new focus's ancestor read stopped
/// at, its cache join, `met`, an index into the read's known runtime ids:
/// the focus before itself, then its ancestors, nearest first. The stop is
/// the last ancestor read, so what is reused begins just above it.
fn cached_ancestors(before: &[UiaAncestorFact], met: usize) -> Vec<UiaAncestorFact> {
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
    /// Whether `window`, an event's sender's cached window, can be read,
    /// by the listener's window readability reader, the one its reading
    /// threads and its UIA callbacks are given, for the callbacks' test as
    /// an event arrives ([`Readers::window_hung`]): the system's judgement
    /// ([`verbatim_win32::window::readability`], local calls only), the
    /// default.
    fn readability(&self, window: isize) -> Readability {
        verbatim_win32::window::readability(window)
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
    apps: HashMap<AppKey, AppShell>,
    idle_waiters: HashMap<u64, IdleWaiter>,
    next_waiter: u64,
}

/// What the shell keeps for one application generation the schedule holds:
/// made when a thread is put in charge of its events, and dropped whole when
/// the schedule lets the record go ([`ScheduleEffect::Forget`]), which it
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
    /// shutdown (`books/dev/src/explanation/design/wake-ups.md`, item 3).
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
                app,
                generation,
                reuse: true,
            } => {
                if let Some(thread) = thread.as_mut() {
                    *thread.generation = generation;
                }
                state.apps.entry(app).or_default();
            }
            ScheduleEffect::Spawn {
                app,
                generation,
                reuse: false,
            } => failed.extend(spawn(shared, state, app, generation)),
            ScheduleEffect::Wake(Waking::Thread(key)) => {
                if let Some(app) = state.apps.get(&key) {
                    app.wake.notify_one();
                }
            }
            ScheduleEffect::Wake(Waking::All) => {
                for app in state.apps.values() {
                    app.wake.notify_all();
                }
            }
            ScheduleEffect::Unwatch { app } => {
                let _ = state.apps.get_mut(&app).and_then(|app| app.watched.take());
            }
            ScheduleEffect::Forget { app } => drop(state.apps.remove(&app)),
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
                // Only a judged read is told its window, so only it is
                // given what to do then.
                let (app, request) = (job.app, job.request);
                let watched = WatchedRead::new(
                    request,
                    job.window.known().map_or(0, Window::get),
                    (job.window != RuleWindow::Never).then(|| {
                        (
                            on_learn(shared, app, request),
                            on_place(shared, app, request),
                        )
                    }),
                );
                state.apps.entry(app).or_default().watched = Some(watched.clone());
                next = Some((job.meta, job.work, watched, job.queue));
            }
            ask @ ScheduleEffect::Judge { .. } => asks.push(ask),
            ScheduleEffect::LogicError(error) => {
                tracing::error!(
                    error,
                    "the read scheduler reached a state its rules exclude"
                );
                debug_assert!(false, "{error}");
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
        let answer = answer(shared, &ask);
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
    let mut apps: Vec<AppKey> = state.apps.keys().copied().collect();
    apps.sort_unstable();
    let mut held: Vec<AppKey> = state.schedule.held().collect();
    held.sort_unstable();
    debug_assert_eq!(apps, held, "the applications' records");
}

/// The answer to the schedule's ask `ask`, made under the readers' lock
/// with local calls only: whether the user has moved on from the read's
/// window ([`Reading::moved_on`]).
fn answer<R: Reading>(shared: &Shared<R>, ask: &ScheduleEffect<R::Job>) -> ScheduleInput<R::Job> {
    let ScheduleEffect::Judge {
        app,
        request,
        window,
    } = *ask
    else {
        unreachable!("only a judgement is asked")
    };
    ScheduleInput::Judged {
        app,
        request,
        verdict: shared
            .reading
            .moved_on(window.get())
            .then_some(Reason::MovedOn),
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
    /// judged at [`MOVED_ON_GRACE`](verbatim_reader_rules::MOVED_ON_GRACE) and every [`MOVED_ON_GRACE`](verbatim_reader_rules::MOVED_ON_GRACE) after.
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
    /// waiting ([`FOCUS_READ_DEADLINE`]). `owed` answers its marker: kept until
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

    /// Whether application `pid` has a reading thread in charge of its
    /// events, or abandoned to a read of them: Core's live re-read of an
    /// application with none is answered gone at once, its last read gone
    /// with its thread.
    pub(crate) fn has_reading_thread(&self, pid: Pid) -> bool {
        self.shared.lock().schedule.current(pid).is_some()
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

    /// `pid`'s process has exited: its record leaves the running
    /// applications, so no later event reaches it, and its readers finish
    /// what they hold. Never waits on a read.
    pub(crate) fn application_exited(&self, pid: Pid) {
        let mut state = self.shared.lock();
        step(
            &self.shared,
            &mut state,
            ScheduleInput::ApplicationExited { pid },
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
        step(
            &self.shared,
            &mut state,
            ScheduleInput::Foreground {
                top_level: Window::new(top_level),
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

    /// Whether `window`, an event's sender's cached window, is hung or
    /// ghosted, by the reading's readability reader
    /// ([`Reading::readability`]), for the UIA callbacks' test as an event
    /// arrives; a sender with no window, 0, is not tested. Local calls,
    /// and no lock.
    pub(crate) fn window_hung(&self, window: isize) -> bool {
        window != 0 && self.shared.reading.readability(window).is_hung()
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

/// Starts a reading thread in charge of `app`'s events as `generation`:
/// with no thread to be had, the failure, for the caller to step the
/// schedule with, which drops its waiting events.
fn spawn<R: Reading>(
    shared: &Arc<Shared<R>>,
    state: &mut ReadersState<R::Job, R::Owed>,
    app: AppKey,
    generation: u64,
) -> Option<ScheduleInput<R::Job>> {
    state.apps.entry(app).or_default();
    let reading = Arc::clone(shared);
    // A thread the system cannot start, as one out of memory refuses it.
    #[cfg(test)]
    let refused = verbatim_faults::hit(
        verbatim_faults::Fault::ThreadStart,
        &(READING_THREAD, app.pid, generation),
    );
    #[cfg(not(test))]
    let refused = false;
    let spawned = if refused {
        Err(std::io::Error::from_raw_os_error(ERROR_NOT_ENOUGH_MEMORY))
    } else {
        thread::Builder::new()
            .name(READING_THREAD.to_owned())
            .spawn(move || run(&reading, app, generation))
    };
    match spawned {
        Ok(thread) => {
            state.threads.retain(|thread| !thread.is_finished());
            state.threads.push(thread);
            None
        }
        Err(error) => {
            tracing::error!(pid = %app.pid, %error, "a reading thread could not be started");
            Some(ScheduleInput::SpawnFailed { app, generation })
        }
    }
}

/// A reading thread: reads `key`'s events in order while it is the thread
/// in charge of them, then releases its reader and ends.
fn run<R: Reading>(shared: &Arc<Shared<R>>, key: AppKey, generation: u64) {
    let mut generation = generation;
    let mut reader = shared.reading.reader(key.pid);
    while let Some((mut meta, job, window, queue)) =
        next_job(shared, key, &mut reader, &mut generation)
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
        let request = window
            .request()
            .expect("a job's read is made with its request");
        let mut state = shared.lock();
        if let Some(app) = state.apps.get_mut(&key)
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
                app: key,
                request,
                meta,
                judged,
            },
            Some(ThreadSide {
                reader: &mut reader,
                generation: &mut generation,
            }),
        );
        if !state.schedule.in_charge(key, generation) {
            break;
        }
    }
    shared.reading.finish(reader);
}

/// The next event for the thread `generation` to read for `key`, waiting
/// for one; `None` when the thread is no longer in charge, the readers are
/// stopping, or the application's outpost has ended and no event is
/// waiting ([`Readers::outpost_ended`]).
fn next_job<R: Reading>(
    shared: &Arc<Shared<R>>,
    key: AppKey,
    reader: &mut Option<R::Reader>,
    generation: &mut u64,
) -> Option<(Meta, R::Job, WatchedRead, QueuePlace)> {
    let mut state = shared.lock();
    loop {
        let input = ScheduleInput::Next {
            app: key,
            generation: *generation,
        };
        let side = ThreadSide {
            reader: &mut *reader,
            generation: &mut *generation,
        };
        if let Some(job) = step(shared, &mut state, input, Some(side)) {
            return Some(job);
        }
        if !state.schedule.in_charge(key, *generation) {
            return None;
        }
        let wake = Arc::clone(&state.apps.entry(key).or_default().wake);
        state = wake.wait(state).unwrap_or_else(PoisonError::into_inner);
    }
}

/// What `app`'s read `request` does once it learns its window
/// ([`WatchedRead::learn`]): steps the schedule, whose work in flight then
/// judges the moved-on rule at the grace's end, or at once when the grace
/// has passed. One uncontended lock and no wake. Never waits on a read.
fn on_learn<R: Reading>(shared: &Arc<Shared<R>>, app: AppKey, request: Request) -> OnLearn {
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
                app,
                request,
                window,
            },
            None,
        );
    })
}

/// What `app`'s read `request` does once it finds its top-level window
/// ([`WatchedRead::place`]): steps the schedule. One uncontended lock and
/// no wake (`books/dev/src/reference/performance.md`). Never waits on a
/// read.
fn on_place<R: Reading>(shared: &Arc<Shared<R>>, app: AppKey, request: Request) -> OnLearn {
    let shared = Arc::downgrade(shared);
    Box::new(move |top: isize| {
        let (Some(shared), Some(top)) = (shared.upgrade(), Window::new(top)) else {
            return;
        };

        let mut state = shared.lock();
        step(
            &shared,
            &mut state,
            ScheduleInput::Placed { app, request, top },
            None,
        );
    })
}

/// The watchdog: each time its alarm goes off, steps the schedule, whose
/// reads under way judge what is due (a deadline, or the half-second
/// judgment of whether the user has moved on) and whose events
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
    /// The listener's window readability reader, which each thread's
    /// reader is given.
    readability: WindowReadabilityReader,
    outcomes: Mutex<std::sync::mpsc::Sender<(u64, Judged)>>,
}

impl Reading for ThreadReading {
    /// The event's element and its own window handle from the event's
    /// cache.
    type Job = (verbatim_uia::Agile<ListenerEvent>, isize);
    type Reader = FocusReader;

    fn reader(&self, _: Pid) -> Option<FocusReader> {
        let mut reader = FocusReader::new().ok()?;
        reader.set_window_readability_reader(Arc::clone(&self.readability));
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

    fn readability(&self, window: isize) -> Readability {
        (self.readability)(window)
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
    /// test sets up its own reader's foreground window, each reader judging
    /// whether a window its walk reaches is hung by `readability`, as the
    /// listener's are by the listener's one reader, and the receiver of
    /// what became of each event, with its marker.
    #[must_use]
    pub fn new(
        set_up: SetUpReader,
        readability: WindowReadabilityReader,
    ) -> (Self, std::sync::mpsc::Receiver<(u64, Judged)>) {
        let (outcomes, received) = std::sync::mpsc::channel();
        let readers = Readers::new(
            ThreadReading {
                set_up,
                readability,
                outcomes: Mutex::new(outcomes),
            },
            FOCUS_READ_DEADLINE,
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
            answers: None,
            cached_from: None,
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

        /// Its windows stand for no real one, and none is hung.
        fn readability(&self, _: isize) -> Readability {
            Readability::Readable
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
        /// as a drop.
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
        still_fixture_of(deadline, (WINDOW, 0))
    }

    /// [`fixture_of`]'s readers, whose schedule's clock stands still as
    /// [`still_fixture`]'s does.
    fn still_fixture_of(deadline: Duration, (hwnd, nearest): (isize, isize)) -> (Fixture, Still) {
        let still = Still {
            start: Instant::now(),
            at: Arc::new(AtomicU64::new(0)),
        };
        (fixture_by(deadline, (hwnd, nearest), still.clock()), still)
    }

    /// [`tick_at`], taking the readers' lock for the tick alone.
    fn tick(fixture: &Fixture, still: &Still, at: u64) {
        let mut state = fixture.readers.shared.lock();
        tick_at(fixture, still, &mut state, at);
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
    /// drop its owed answer sends as it is dropped: each once. The test's
    /// wait on the application's condition variable is registered before
    /// the stop is called, so the stop's wake is what ends it; the reading
    /// thread's own answer to its marker is the evidence that it returned
    /// and read nothing more; and the stop runs on a thread of the test's,
    /// joined through a channel within the test's wait, so a stop that held
    /// the readers' lock while it joined their threads fails the test
    /// rather than hanging it.
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
                }));
            })
        };
        readers.push(Pid(5), meta(1), (1, Some(gate), Vec::new()), owed(1));
        assert_eq!(read_on.recv_timeout(WAIT).expect("a read").0, 1);
        readers.push(Pid(5), meta(2), (2, None, Vec::new()), owed(2));
        let readers = Arc::new(readers);
        // The wait is registered before the stop is called: the test holds
        // the readers' lock while it starts the stop's thread, which waits
        // for the lock, and lets it go only by waiting on pid 5's condition
        // variable, which the stop wakes as it marks the schedule stopping.
        // The thread in charge of pid 5's events is held in its read, not
        // waiting on it, so this wait takes no wake meant for that thread.
        let state = readers.shared.lock();
        let held = state.schedule.current(Pid(5)).expect("pid 5 runs");
        let wake = Arc::clone(&state.apps.get(&held).expect("pid 5 is held").wake);
        let (stopped_tx, stopped) = mpsc::channel();
        thread::spawn({
            let readers = Arc::clone(&readers);
            move || {
                let count = readers.stop();
                let _ = stopped_tx.send((count, readers));
            }
        });
        let (state, waited) = wake
            .wait_timeout(state, WAIT)
            .unwrap_or_else(PoisonError::into_inner);
        assert!(
            !waited.timed_out(),
            "the readers' stop never woke pid 5's thread"
        );
        drop(state);
        open.send(()).expect("the gate opens");
        // The reading thread's own answer to its marker, sent as its read
        // returns: a stop that kept the readers' lock while it joined their
        // threads would hold the read's return, and fail this wait.
        assert_eq!(
            answered(std::iter::once(
                facts
                    .recv_timeout(WAIT)
                    .expect("the read's marker answered")
            )),
            [(1, false)]
        );
        let (count, handed_back) = stopped.recv_timeout(WAIT).expect("the readers stopped");
        assert_eq!(count, 1);
        drop(handed_back);
        drop(readers);
        // The waiting event has no outcome of the schedule's: its owed
        // answer, dropped with the readers, answers its marker.
        let reasons: Vec<(Pid, u64, &str)> = outcomes.try_iter().collect();
        assert_eq!(reasons, [(Pid(5), 1, "read")]);
        assert_eq!(answered(facts.try_iter()), [(2, false)]);
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
        // A waiter due at once runs inside `when_idle`'s step, so this read
        // follows any run it made.
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
        let (fixture, still) = still_fixture(Duration::from_millis(50));
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, held_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        fixture.push(Pid(5), meta(2), (2, None, Vec::new()));
        tick(&fixture, &still, 49_999);
        assert_eq!(
            fixture.outcomes.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "within the deadline"
        );
        tick(&fixture, &still, 50_000);
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
        let (fixture, still) = still_fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, held_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        fixture.push(Pid(5), meta(2), (2, None, Vec::new()));
        tick(&fixture, &still, grace_us());
        assert_eq!(
            fixture.asked.try_recv(),
            Ok(WINDOW),
            "the grace's end asks about the read's window"
        );
        assert_eq!(fixture.outcomes.try_recv(), Err(mpsc::TryRecvError::Empty));
        fixture.set_moved_on(true);
        tick(&fixture, &still, 2 * grace_us());
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, MOVED_ON),
            "the next grace's check found the user had moved on"
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
        let (fixture, still) = still_fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, read_on) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        for (check, at) in [
            ("the grace's check", grace_us()),
            ("the next grace's check", 2 * grace_us()),
        ] {
            tick(&fixture, &still, at);
            assert_eq!(fixture.asked.try_recv(), Ok(WINDOW), "{check}");
            assert_eq!(
                fixture.outcomes.try_recv(),
                Err(mpsc::TryRecvError::Empty),
                "{check}"
            );
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
        let (fixture, still) = still_fixture_of(Duration::from_millis(800), (0, 0));
        fixture.set_moved_on(true);
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        tick(&fixture, &still, grace_us());
        assert_eq!(
            fixture.outcomes.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "not judged at the grace's end"
        );
        tick(&fixture, &still, 800_000);
        assert_eq!(fixture.outcomes.try_recv(), Ok((Pid(5), 1, NOT_IN_TIME)));
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
        let (fixture, still) = still_fixture_of(Duration::from_secs(60), (0, NEAREST));
        fixture.set_moved_on(true);
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        // The read learns the nearest window before it says it reads.
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        tick(&fixture, &still, grace_us());
        assert_eq!(fixture.outcomes.try_recv(), Ok((Pid(5), 1, MOVED_ON)));
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
        let (fixture, still) = still_fixture(Duration::from_secs(60));
        fixture.set_moved_on(true);
        let (open_first, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        let (open_second, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        tick(&fixture, &still, grace_us());
        assert_eq!(fixture.outcomes.try_recv(), Ok((Pid(5), 1, MOVED_ON)));
        // The new thread's read begins at the first read's grace's end.
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 2);
        tick(&fixture, &still, 2 * grace_us() - 1);
        assert_eq!(
            fixture.outcomes.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "not judged before its own grace's end"
        );
        tick(&fixture, &still, 2 * grace_us());
        assert_eq!(fixture.outcomes.try_recv(), Ok((Pid(5), 2, MOVED_ON)));
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
        let (fixture, still) = still_fixture(Duration::from_secs(60));
        fixture.set_moved_on(true);
        let (open_first, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, first) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        tick(&fixture, &still, grace_us());
        assert_eq!(fixture.outcomes.try_recv(), Ok((Pid(5), 1, MOVED_ON)));
        let (open_second, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 2);
        tick(&fixture, &still, 2 * grace_us());
        assert_eq!(fixture.outcomes.try_recv(), Ok((Pid(5), 2, MOVED_ON)));
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
        let (fixture, still) = still_fixture(Duration::from_millis(50));
        let (open_first, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 1);
        tick(&fixture, &still, 50_000);
        assert_eq!(
            fixture.outcomes.try_recv(),
            Ok((Pid(5), 1, "its read did not return in time"))
        );
        let (open_second, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(2), (2, Some(gate), Vec::new()));
        assert_eq!(fixture.read_on.recv_timeout(WAIT).expect("a read").0, 2);
        // The second abandoned read reaches the limit, and the
        // application's bound is a deadline after it.
        tick(&fixture, &still, 100_000);
        assert_eq!(
            fixture.outcomes.try_recv(),
            Ok((Pid(5), 2, "its read did not return in time"))
        );
        fixture.push(Pid(5), meta(3), (3, None, Vec::new()));
        fixture.push(Pid(5), meta(4), (4, None, Vec::new()));
        tick(&fixture, &still, 149_999);
        assert_eq!(
            fixture.outcomes.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "events 3 and 4 wait within the bound"
        );
        tick(&fixture, &still, 150_000);
        let waited = "it waited too long behind its application's abandoned reads";
        let outcomes: Vec<(Pid, u64, &str)> = fixture.outcomes.try_iter().collect();
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
        assert_eq!(
            read,
            Vec::<u64>::new(),
            "events 3, 4 and 5 were never read, only 1 and 2"
        );
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
    /// (`books/dev/src/reference/crates/verbatim-listener-logic.md`): a tick
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
            Vec::<&AppKey>::new()
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

    /// An exit moves the record: the thread reading finishes for the exited
    /// generation, and a new event of the id starts a generation with a
    /// thread of its own, which never waits behind the old one.
    #[test]
    fn an_applications_exit_gives_a_reused_id_a_thread_of_its_own() {
        let fixture = fixture(Duration::from_secs(60));
        let (open, gate) = mpsc::channel();
        fixture.push(Pid(5), meta(1), (1, Some(gate), Vec::new()));
        let (_, first) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        fixture.readers.application_exited(Pid(5));
        fixture.push(Pid(5), meta(2), (2, None, Vec::new()));
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 2, "read"),
            "read while the exited generation's read is out"
        );
        let (_, second) = fixture.read_on.recv_timeout(WAIT).expect("a read");
        assert_ne!(second, first, "the new generation has its own thread");
        open.send(()).expect("the held read waits");
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (Pid(5), 1, "read")
        );
        assert_eq!(
            fixture.finished.recv_timeout(WAIT).expect("a thread ended"),
            first,
            "the exited generation's thread ends with nothing left"
        );
        let _ = fixture.readers.stop();
    }

    #[test]
    fn the_cached_ancestors_begin_above_the_cache_join() {
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
            ids(cached_ancestors(&before, 0)),
            [vec![42, 1], vec![42, 2], vec![42, 3]]
        );
        // Stopped at its second ancestor: those above it.
        assert_eq!(ids(cached_ancestors(&before, 2)), [vec![42, 3]]);
        assert_eq!(ids(cached_ancestors(&before, 3)), Vec::<Vec<i32>>::new());
        assert_eq!(ids(cached_ancestors(&before, 9)), Vec::<Vec<i32>>::new());
    }
}
