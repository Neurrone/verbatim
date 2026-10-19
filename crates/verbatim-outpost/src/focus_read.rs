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
//! - then one remote operation from the sender: its `HasKeyboardFocus`
//!   read again first, for a focus that moves on while it is read; then,
//!   when the sender has
//!   the runtime id of the focus the thread read before, whether that
//!   earlier element still has the keyboard focus, the evidence that the
//!   runtime id still names it; then the sender's own value, states, and
//!   details afresh, its selected child for a list or tab control, and its
//!   ancestors up to the first one the thread knew from the focus before,
//!   whose ancestors are reused from that read; and for a menu item, its
//!   legacy MSAA state, where its checked state may be.
//!
//! The result, or the drop, goes to the application's outpost through
//! verbatim.exe like every fact; the outpost was sent a marker the moment
//! the event arrived, and holds the application's later events until the
//! result or the drop takes the marker's place (section 12.4).
//!
//! Time limits: each read is one transaction, which UIA's call timeout,
//! set once for the process, ends (`verbatim_uia::CALL_TIMEOUT`). A watchdog
//! abandons a thread whose read runs past [`READ_DEADLINE`]: its event is
//! dropped, a new thread takes the application's later events, and the
//! abandoned thread, once UIA gives up, says nothing more and ends. An
//! application whose abandoned threads reach [`ABANDONED_LIMIT`] gets no
//! new thread until one returns, and its events are dropped meanwhile, as
//! NVDA drops a hung window's.
//!
//! A thread keeps its focus before, NVDA's duplicate evidence, for as long
//! as its application has an outpost: it ends, once it has read every event
//! waiting, when the listener learns that the application's outpost has
//! ended ([`Readers::outpost_ended`]), and at the listener's shutdown
//! (`docs/design/focus-pipeline.md`, section 16.6). An event that arrives
//! after that, for an application still running, starts a new thread.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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
    Ancestor, Cached, Direction, ElementExt as _, Fill, FocusInput, ListenerEvent, Live, Shape, Uia,
};
use verbatim_uia_rops::{Ancestry, FocusAncestry, FocusQuery, Path, focus_ancestry};

use crate::ForegroundReader;
pub(crate) use crate::app_locks::{AppLocks, Granted};
use crate::arbitration::{Arbitrator, Clock, Probe, time_limit, uses_uia, window_class_name};
use crate::outpost::acceptance::{EventName, RequestedEvents, SystemWindows, accepts_event};
use crate::outpost::text_reads::CONSOLE_WINDOW_CLASS;
use crate::outpost::window::{
    is_shell_transition_window, top_level_of, window_is_hung, window_thread,
};
use crate::protocol::{
    DeliveredFact, EventTiming, UiaAncestorFact, UiaFocusRead, UiaSnapshotFact, now_us,
};

/// The most ancestors read for a focus, as many as the outpost walks.
const MAX_ANCESTORS: u32 = 64;

/// How long the classic ancestor walk, used where a remote operation
/// cannot import the element, may take before the focus is reported with
/// its ancestors unknown: checked between the walk's calls, each of which
/// UIA ends after its call timeout.
const READ_BUDGET: Duration = Duration::from_secs(2);

/// How long a reading thread may take over one focus event before the
/// watchdog abandons it: a little longer than UIA's call timeout, which
/// ends the read's one transaction.
pub(crate) const READ_DEADLINE: Duration =
    Duration::from_secs(verbatim_uia::CALL_TIMEOUT.as_secs() + 1);

/// How many of an application's reading threads may be abandoned and still
/// in their calls before its focus events are dropped without a read.
const ABANDONED_LIMIT: usize = 2;

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
    /// `arrival` and judged with window `window`, the last focus accepted.
    fn accept(&self, reader: u64, element: &IUIAutomationElement, window: isize, arrival: Arrival) {
        match AgileReference::new(element) {
            Ok(element) => {
                *self.lock() = Some(Accepted {
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
    remote: bool,
    /// Windows whose elements could not be imported into a remote
    /// operation (client-side proxies), with the thread that owned each,
    /// read the classic way from then on.
    classic_windows: HashSet<(isize, u32)>,
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
    /// The application this reader serves and the listener's locks, for
    /// the classic walk's steps up into another application's window,
    /// which take that application's lock (section 20.3); `None` for a
    /// reader that takes none.
    locks: Option<(Pid, Arc<AppLocks>)>,
}

impl Drop for FocusReader {
    fn drop(&mut self) {
        self.last_accepted.release(self.number);
    }
}

impl FocusReader {
    /// A reader with a UIA client of its own, made on the calling thread,
    /// which joins COM's multithreaded apartment. With `remote` off, every
    /// read is the classic walk (`uia.remote_operations` in
    /// `settings.toml`). `last_accepted` is the last focus any of the
    /// listener's readers accepted, which this one shares.
    ///
    /// # Errors
    ///
    /// The COM error when the client cannot be made.
    pub fn new(remote: bool, last_accepted: LastAccepted) -> windows::core::Result<Self> {
        Ok(Self {
            uia: Uia::new()?,
            remote,
            classic_windows: HashSet::new(),
            last: None,
            number: NEXT_READER.fetch_add(1, Ordering::Relaxed),
            last_accepted,
            foreground: Arc::new(crate::outpost::window::foreground_window_handle),
            arbitrator: Mutex::new(Arbitrator::new(&[])),
            locks: None,
        })
    }

    /// Takes the lock of another application, from `locks`, for each step
    /// of the classic walk up into that application's window from `pid`'s,
    /// the application this reader serves
    /// (`docs/design/focus-pipeline.md`, section 20.3).
    pub(crate) fn set_app_locks(&mut self, pid: Pid, locks: Arc<AppLocks>) {
        self.locks = Some((pid, locks));
    }

    /// The lock of the application whose process serves the top-level
    /// window `top`, when that is not this reader's own application and the
    /// listener has its lock: what each step of a classic walk up into that
    /// window takes. The process is opened once, to read its creation time,
    /// a local call.
    fn lock_up_into(&self, top: isize) -> Option<verbatim_uia::lock::AppLock> {
        let (own, locks) = self.locks.as_ref()?;
        let provider = verbatim_uia::provider_process(top)?;
        if provider == own.0 {
            return None;
        }
        let (_process, created) = verbatim_uia::lock::open_process(provider).ok()?;
        locks.find(Pid(provider), created)
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

    /// Judges and reads the UIA focus event, or menu opening (`kind`),
    /// whose sender is `sender`, as the registration delivers it, and which
    /// came as `arrival`. `focus_window` is the keyboard focus window of
    /// the element's application when the event arrived, for an element
    /// that is not a window (0 otherwise).
    pub fn read(
        &mut self,
        sender: &Cached<ListenerEvent>,
        focus_window: isize,
        kind: FocusEvent,
        arrival: Arrival,
    ) -> Judged {
        let fields = sender.read();
        let element = sender.live().element();
        let hwnd = fields.role.hwnd;
        let window = if hwnd == 0 { focus_window } else { hwnd };
        let event = fields.parts(sender.runtime_id());
        let nearest = match self.judge_before_reading(element, &fields, &event.runtime_id, kind) {
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
        let classic_key = (window, window_thread(window));
        let remote = self.remote && !self.classic_windows.contains(&classic_key);
        // Every detail: the listener reads for every application, whatever
        // theme decides which details are spoken. A request that cannot be
        // built is a read that failed.
        let input = sender.narrow::<FocusInput>();
        // A classic walk's step up from a window inside another
        // application's top-level window, as from the Settings page's
        // content into its frame, holds that application's lock (section
        // 20.3); a top-level window's step reaches the desktop root.
        let lock_up_from = |window: isize| {
            let top = top_level_of(window);
            (top != window).then(|| self.lock_up_into(top)).flatten()
        };
        let clock = self.clock();
        let out_of_time = time_limit(&clock, READ_BUDGET);
        let read = self
            .uia
            .request::<Ancestor>(Ancestor::all_on())
            .map_err(verbatim_uia_rops::Error::Uia)
            .and_then(|request| {
                let fill = Fill::new(&request);
                let query = FocusQuery {
                    element: &input,
                    known: &known,
                    previous: previous.as_ref(),
                    depth_limit: MAX_ANCESTORS,
                    fill: &fill,
                    out_of_time: Some(&out_of_time),
                    require_focus: true,
                    refresh: true,
                    lock_up_from: Some(&lock_up_from),
                };
                focus_ancestry(&self.uia, &query, remote)
            });
        match read {
            Ok((FocusAncestry::NotFocused, _)) => {
                Judged::Dropped("its element no longer has the keyboard focus")
            }
            Ok((FocusAncestry::Focused(ancestry), path)) => {
                if let Path::Fallback(error) = &path {
                    tracing::warn!(window, %error, "a remote operation failed; read the classic way");
                    if matches!(error, verbatim_uia_rops::Error::Import(_)) && window != 0 {
                        self.classic_windows.insert(classic_key);
                    }
                }
                self.focused(
                    element,
                    (hwnd, focus_window, nearest),
                    event,
                    (&ancestry, path.name()),
                    arrival,
                )
            }
            Err(error) => {
                // NVDA lets a focus event through when its sender's keyboard
                // focus cannot be read (`shouldAllowUIAFocusEvent` answers
                // true on a COM error): reported as the event's cache gives
                // it, its ancestors unknown, unless that cache says it did
                // not have the keyboard focus.
                tracing::debug!(%error, "a UIA focus could not be read; reported from its event");
                if !event.states.contains(State::Focused) {
                    return Judged::Dropped(
                        "its event says it did not have the keyboard focus, and it could not be read",
                    );
                }
                self.last = Some(LastFocus {
                    runtime_id: event.runtime_id.clone(),
                    element: element.clone(),
                    ancestors: Vec::new(),
                });
                self.last_accepted
                    .accept(self.number, element, nearest, arrival);
                Judged::Read(Box::new(UiaFocusRead {
                    hwnd,
                    focus_window,
                    snapshot: fact_of(event),
                    ancestors: None,
                    selected_child: None,
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
    /// takes no call.
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
        (hwnd, focus_window, nearest): (isize, isize, isize),
        event: CachedUiaParts,
        (ancestry, path): (&Ancestry, &'static str),
        arrival: Arrival,
    ) -> Judged {
        let live = ancestry
            .element
            .as_ref()
            .map(|fresh| fresh.read().node.parts(fresh.runtime_id()));
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
        if wants_legacy_checked_state(snapshot.role, snapshot.states) {
            // A menu item without the Toggle pattern: its checked state may
            // be only in its legacy MSAA state, which the remote program
            // read with the focus. The classic walk, which a window whose
            // elements cannot be imported into a program is read with, left
            // it to this one live call.
            let legacy = ancestry.legacy_state.unwrap_or_else(|| {
                element
                    .current_i32_ignoring_default(UIA_LegacyIAccessibleStatePropertyId)
                    .ok()
                    .flatten()
            });
            add_legacy_checked_state(&mut snapshot.states, legacy);
        }
        let mut ancestors = (!ancestry.out_of_time).then(|| {
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
        // The console host's provider answers no native window handle for
        // its window inside a remote operation (only the client side's
        // window proxy supplies it), so for a windowless element whose read
        // named no window, the nearest window found for the acceptance test
        // stands (3f4dbb4), and the ancestor that is that window is given
        // its handle, which the outpost finds its own copy of the element
        // from.
        let mut window = ancestry.window;
        if window.is_none() && hwnd == 0 {
            window = Some(nearest);
            if let (Some(found), Some(ancestors)) = (window, ancestors.as_mut()) {
                self.mark_window(found, ancestors);
            }
        }
        if ancestry.met_known.is_none()
            && !ancestry.depth_limited
            && let Some(facts) = ancestors.as_mut()
        {
            let outermost = ancestry.ancestors.last().map_or_else(
                || Live::new(element.clone()),
                |outermost| outermost.live().clone(),
            );
            self.reach_the_top_level(&outermost, hwnd, facts);
        }
        if let Some(facts) = &ancestors {
            log_ancestor_facts("sent", facts);
        }
        self.last = Some(LastFocus {
            runtime_id: snapshot.runtime_id.clone(),
            element: element.clone(),
            ancestors: ancestors.clone().unwrap_or_default(),
        });
        self.last_accepted
            .accept(self.number, element, nearest, arrival);
        Judged::Read(Box::new(UiaFocusRead {
            hwnd,
            focus_window,
            snapshot,
            ancestors,
            selected_child: ancestry
                .selected_child
                .as_ref()
                .map(|child| fact_of(child.read().node.parts(child.runtime_id()))),
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
    fn judge_before_reading(
        &self,
        element: &IUIAutomationElement,
        sender: &ListenerEvent,
        runtime_id: &[i32],
        kind: FocusEvent,
    ) -> Result<isize, &'static str> {
        let hwnd = sender.role.hwnd;
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
                .map(|(focus, _)| self.uia.compare_elements(element, focus));
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
                match (compare(), current.as_ref()) {
                    (Some(Err(_)), _) => {
                        return Err("its comparison with the current focus failed");
                    }
                    (Some(Ok(true)), Some((focus, window))) => (*window, focus),
                    _ => (self.native_window(element, sender)?, element),
                }
            }
            // Only focuses and menu openings are read; a selection or a
            // notification is checked ([`Self::check_fact`]).
            FocusEvent::Focus | FocusEvent::Selection | FocusEvent::Notification => {
                let nearest = self.native_window(element, sender)?;
                // 3. NVDA's duplicate rule (`UIAHandler/__init__.py` 902 to
                // 923; section 16.3): the sender is the last focus accepted,
                // by any of the listener's readers, as UIA compares them,
                // and that element still has the keyboard focus, read live.
                // A comparison that fails is no duplicate, as NVDA lets the
                // event through on a COM error, and so is a keyboard focus
                // read that fails: the earlier element is gone. Two calls,
                // the second only for an element UIA equates.
                if let (Some(Ok(true)), Some((last, _))) = (compare(), current.as_ref())
                    && kind == FocusEvent::Focus
                    && last.has_keyboard_focus().unwrap_or(false)
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
        // through.
        //
        // Until 2026-10-12 this check was made only inside the remote
        // operation that reads the focus (Option B, 2026-10-06), its first
        // instruction, to save this call. That is not the same read. An
        // element imported into a remote operation is the provider's own
        // object, in the application, while a plain read goes through the
        // element as this client holds it. For almost every element the
        // two answer alike, but Windows Terminal raises some focus events
        // from a second identity of its terminal (a runtime id starting
        // 14), and for that identity they differ: the remote operation
        // answered that it had the keyboard focus, the plain read that it
        // had not. NVDA never took one (captured 6 times in 15 second
        // windows, 9 ms after it took the same terminal's focus by its
        // usual identity; that NVDA, alpha-57645, read the event's cached
        // value, and NVDA has read it plainly since its #20764, as here,
        // sections 16.8 and 16.9); the listener took them and "terminal"
        // was said twice. So the check must be this plain read,
        // here, before the read; the remote operation keeps its own check,
        // for a focus that moves on while it is read
        // (`docs/design/focus-pipeline.md`, section 16.8).
        if matches!(subject.has_keyboard_focus(), Ok(false)) {
            return Err("its element no longer has the keyboard focus");
        }
        Ok(window)
    }

    /// Step 2: whether the sender, `element`, whose event's cache is
    /// `sender`, is native UIA (`isNativeUIAElement`, `UIAHandler/__init__.py` 1515 to
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
    /// automation id, or the console window's own element, under which the
    /// text area is found. Any other sender in a console window, of which
    /// none is known, gets no answer: its focus is dropped as not native,
    /// and the window is checked again at its next focus.
    fn native_window(
        &self,
        element: &IUIAutomationElement,
        sender: &ListenerEvent,
    ) -> Result<isize, &'static str> {
        let hwnd = sender.role.hwnd;
        let nearest = if hwnd == 0 {
            let started = Instant::now();
            let nearest = verbatim_uia::nearest_window_handle(element);
            let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            tracing::debug!(
                elapsed_us,
                window = nearest,
                "UIA focus sender's nearest window looked up"
            );
            nearest.ok_or("it has no window above it, so NVDA makes no object for it")?
        } else {
            hwnd
        };
        // The raw probe, not a connection: the listener connects to no
        // application's window (`docs/design/listener-without-connections.md`,
        // section 2).
        let automation_id = sender.automation_id.as_deref();
        let console = || {
            (automation_id == Some(verbatim_uia::CONSOLE_TEXT_AREA) || hwnd == nearest)
                .then(|| {
                    verbatim_uia::held_console_reports_formatting(&self.uia, element, automation_id)
                })
                .flatten()
        };
        if !uses_uia(&self.arbitrator, nearest, Probe::Raw, console) {
            return Err("its window does not use UIA, so its MSAA events speak for it");
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

    /// Completes `facts`, the ancestors a read found, nearest first, ending
    /// at `outermost` (the focus itself, whose own window is `hwnd`, when
    /// there are none), up to the top-level window of the focus's process,
    /// as NVDA's raw-view walk reaches it, but never across the process
    /// boundary. A remote operation's walk ends at the top-level element of
    /// the focus's own process, whose parent the program reports as null;
    /// in the Settings app that is the content's window, inside the frame
    /// window of another process (`ApplicationFrameHost`'s), where the
    /// chain ends, and the outpost continues it from the frame through
    /// MSAA (`docs/design/listener-without-connections.md`, section 3,
    /// step 11). When the outermost ancestor is neither a top-level window
    /// nor below the boundary, the walk goes on the classic way, one tree
    /// walker step per ancestor, until the desktop root, an ancestor whose
    /// window's parent is another process's, or an ancestor of the focus
    /// before, whose own ancestors are then reused. An outermost ancestor
    /// that is a top-level window, or below the boundary, costs no call; a
    /// windowless one, one call, which finds the root.
    fn reach_the_top_level(&self, outermost: &Live, hwnd: isize, facts: &mut Vec<UiaAncestorFact>) {
        let window = facts.last().map_or(hwnd, |fact| fact.hwnd);
        if window != 0 && top_level_of(window) == window {
            tracing::debug!(
                window,
                "UIA focus ancestors: the outermost is a top-level window; no classic walk"
            );
            return;
        }
        if window != 0 && verbatim_uia::parent_across_process_boundary(window).is_some() {
            tracing::debug!(
                window,
                "UIA focus ancestors: the outermost is below the process boundary; no classic walk"
            );
            return;
        }
        let (Ok(request), Ok(walker), Ok(root)) = (
            self.uia.request::<Ancestor>(Ancestor::all_on()),
            self.uia.raw_view_walker(),
            self.uia.root_element(),
        ) else {
            return;
        };
        let root = verbatim_uia::runtime_id(&root);
        let known: Vec<&[i32]> = self
            .last
            .iter()
            .flat_map(|last| {
                std::iter::once(last.runtime_id.as_slice()).chain(
                    last.ancestors
                        .iter()
                        .map(|ancestor| ancestor.snapshot.runtime_id.as_slice()),
                )
            })
            .collect();
        let limit = usize::try_from(MAX_ANCESTORS).unwrap_or(usize::MAX);
        // A step up from this application's element into a window of
        // another process, as from the Settings page's content into its
        // frame in `ApplicationFrameHost`, returns UIA's window proxy after
        // about two seconds when it arrives while a remote operation from
        // another process waits to be evaluated in that process (a UIA
        // defect, section 20.1): each step takes that process's lock, read
        // once here from the top-level window, while the listener holds it.
        let frame_lock = (window != 0)
            .then(|| self.lock_up_into(top_level_of(window)))
            .flatten();
        let mut current = outermost.clone();
        while facts.len() < limit {
            // A failed step, a provider that did not answer in time
            // included, ends the walk with what it has.
            let step = || current.walk(&walker, Direction::Parent, &request);
            let stepped = match &frame_lock {
                Some(lock) => verbatim_uia::lock::guarded_by(lock, "a classic walk step up", step),
                None => step(),
            };
            let parent = match stepped {
                Ok(parent) => parent,
                Err(error) => {
                    tracing::debug!(%error, "UIA focus ancestors: a classic walk step failed");
                    break;
                }
            };
            let runtime_id = parent.runtime_id();
            if runtime_id == root {
                tracing::debug!("UIA focus ancestors: the classic walk reached the root");
                break;
            }
            let fact = ancestor_fact(&parent);
            log_ancestor_fact("classic walk", &fact);
            let below_the_boundary =
                fact.hwnd != 0 && verbatim_uia::parent_across_process_boundary(fact.hwnd).is_some();
            facts.push(fact);
            if let Some(met) = known
                .iter()
                .position(|known| *known == runtime_id.as_slice())
            {
                if let Some(last) = &self.last {
                    facts.extend(reused_ancestors(&last.ancestors, met));
                }
                break;
            }
            if below_the_boundary {
                tracing::debug!(
                    "UIA focus ancestors: the classic walk reached the process boundary"
                );
                break;
            }
            current = parent.live().clone();
        }
    }

    /// Gives the ancestor that is window `hwnd`'s own element its handle,
    /// read as the window's element's runtime id: one call, through the
    /// client side's window proxy, whose properties and content flag then
    /// replace the in-provider read's (16.4: the console window's title).
    /// Nothing changes when the window's element cannot be read or is no
    /// ancestor read.
    fn mark_window(&self, hwnd: isize, ancestors: &mut [UiaAncestorFact]) {
        let Ok(request) = self.uia.request::<Ancestor>(Ancestor::all_on()) else {
            return;
        };
        let Ok(window) = self.uia.window_element(hwnd, &request) else {
            tracing::debug!(hwnd, "mark_window: the window's element could not be read");
            return;
        };
        let runtime_id = window.runtime_id();
        if let Some(ancestor) = ancestors
            .iter_mut()
            .find(|ancestor| ancestor.hwnd == 0 && ancestor.snapshot.runtime_id == runtime_id)
        {
            // What the in-provider read gave this element is not what a
            // client sees: the console host's provider names no window, and
            // the client side's window proxy supplies its title, as NVDA's
            // raw-view parent read through the proxy has it
            // (`NVDAObjects/UIA/__init__.py` 2136 to 2141). So the window's
            // properties and standing as content are taken from this
            // client-side read, already made.
            let read = window.read();
            ancestor.hwnd = hwnd;
            ancestor.snapshot = fact_of(read.node.parts(runtime_id));
            ancestor.content = read.is_control_and_content();
            log_ancestor_fact("mark_window", ancestor);
        } else {
            tracing::debug!(
                hwnd,
                ?runtime_id,
                "mark_window: no ancestor read is the window's element"
            );
        }
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
    /// Judges and reads one event, which `meta` describes.
    fn read(&self, reader: Option<&mut Self::Reader>, meta: Meta, job: Self::Job) -> Judged;
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

/// One application's reading.
struct App<J> {
    /// Its events waiting for a read, oldest first, each with its place
    /// in the order every application's events were queued in
    /// ([`ReadersState::queued`]).
    queue: VecDeque<(u64, Meta, J)>,
    /// The thread in charge of its events, by generation, if one is.
    live: Option<u64>,
    /// When the thread in charge began the event it is reading, that
    /// event, and its kind (a focus when the job has no key).
    running: Option<(Instant, Meta, FocusEvent)>,
    /// The place of the event being read in the order events were queued,
    /// while one is.
    running_place: u64,
    /// Its threads abandoned to reads that have not returned yet.
    abandoned: usize,
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
                    app.queue.front().map(|(place, _, _)| *place)
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
    state: Mutex<ReadersState<R::Job>>,
    /// Wakes the watchdog when a read starts, and for the shutdown.
    changed: Condvar,
    /// The failed cached reads the reading threads have made
    /// (`verbatim_uia::calls`).
    failed_reads: AtomicU32,
    /// The applications' locks each thread reads with
    /// (`docs/design/focus-pipeline.md`, section 20.4), or `None` for
    /// readers that take none.
    locks: Option<Arc<AppLocks>>,
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
    /// Readers doing `reading`, each read abandoned past `deadline`, each
    /// thread taking its application's lock from `locks`, when given,
    /// before each read (`docs/design/focus-pipeline.md`, section 20.4): its
    /// thread's lock, which the guarded UIA calls of its read take
    /// (`verbatim_uia::lock`).
    ///
    /// # Panics
    ///
    /// Panics if the watchdog's thread cannot be spawned, which means the
    /// process is out of OS thread resources.
    pub(crate) fn with_locks(reading: R, deadline: Duration, locks: Option<Arc<AppLocks>>) -> Self {
        let shared = Arc::new(Shared {
            reading,
            deadline,
            state: Mutex::new(ReadersState {
                apps: HashMap::new(),
                next_generation: 0,
                threads: Vec::new(),
                stopping: false,
                queued: 0,
                idle_waiters: VecDeque::new(),
            }),
            changed: Condvar::new(),
            failed_reads: AtomicU32::new(0),
            locks,
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
    /// for `pid`'s reading thread, starting one if it has none. Never waits
    /// on a read.
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
        if app.live.is_none() && app.abandoned >= ABANDONED_LIMIT {
            tracing::warn!(%pid, "a UIA focus is dropped: its application's reads have not returned");
            shared.reading.outcome(
                pid,
                meta,
                Judged::Dropped("its application's reads have not returned"),
            );
            return;
        }
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
            for (place, waiting_meta, waiting) in app.queue.drain(..) {
                if shared.reading.key(&waiting) == Some(key) {
                    shared.reading.outcome(
                        pid,
                        waiting_meta,
                        Judged::Dropped("a newer focus event of its element replaced it"),
                    );
                } else {
                    kept.push_back((place, waiting_meta, waiting));
                }
            }
            app.queue = kept;
        }
        state.queued += 1;
        let place = state.queued;
        let Some(app) = state.apps.get_mut(&pid) else {
            return;
        };
        app.queue.push_back((place, meta, job));
        if app.live.is_some() {
            app.wake.notify_one();
        } else {
            start(shared, &mut state, pid);
        }
    }

    /// Whether a focus event of any application is waiting for its read or
    /// being read: NVDA drops a menu opening that arrives while a focus is
    /// pending (`isPendingEvents("gainFocus")`, `UIAHandler/__init__.py`
    /// 772 to 777), and here a focus still to be judged stands for one
    /// NVDA has queued and not yet run.
    pub(crate) fn focus_pending(&self) -> bool {
        let shared = &self.shared;
        let state = shared.lock();
        state.apps.values().any(|app| {
            app.running
                .is_some_and(|(_, _, kind)| kind == FocusEvent::Focus)
                || app.queue.iter().any(|(_, _, job)| {
                    shared
                        .reading
                        .key(job)
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

    /// The failed cached reads the reading threads have made so far.
    pub(crate) fn failed_reads(&self) -> u32 {
        self.shared.failed_reads.load(Ordering::Relaxed)
    }

    /// Stops every reading thread for the listener's shutdown: nothing is
    /// queued from now on, what is waiting is not read, and every thread is
    /// waited for, each abandoned one returning once UIA gives up on its
    /// call. Returns how many threads were waited for.
    pub(crate) fn stop(&self) -> usize {
        // A thread waiting for its application's lock waits no more.
        if let Some(locks) = &self.shared.locks {
            locks.stop();
        }
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
            for (_, meta, _) in app.queue.drain(..) {
                shared
                    .reading
                    .outcome(pid, meta, Judged::Dropped("no thread to read it"));
            }
            state.wake_idle_waiters();
        }
    }
}

/// A reading thread: reads `pid`'s events in order while it is the thread
/// in charge of them, then releases its reader and ends.
fn run<R: Reading>(shared: &Arc<Shared<R>>, pid: Pid, generation: u64) {
    let mut generation = generation;
    // The application's process, held while the thread serves it so its
    // id is not reused meanwhile, and its creation time, which with the id
    // names the lock the thread reads with (section 20.4). A process that
    // cannot be opened is read without a lock: Verbatim cannot give it an
    // outpost either.
    let identity = shared.locks.as_ref().and_then(|_| {
        verbatim_uia::lock::open_process(pid.0)
            .inspect_err(|error| {
                tracing::debug!(%pid, %error, "a reading thread cannot open its application; it reads without its lock");
            })
            .ok()
    });
    let mut reader = shared.reading.reader(pid);
    while let Some((mut meta, job)) = next_job(shared, pid, generation) {
        let judged = match take_lock(shared, pid, identity.as_ref().map(|(_, created)| *created)) {
            Some(Granted::Stopped) => Judged::Dropped("the listener is shutting down"),
            _ => shared.reading.read(reader.as_mut(), meta, job),
        };
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
        // thread in charge and events waiting, this one takes them.
        app.abandoned = app.abandoned.saturating_sub(1);
        tracing::info!(%pid, "an abandoned UIA focus read returned; its result is discarded");
        if app.live.is_some() || app.queue.is_empty() || state.stopping {
            break;
        }
        state.next_generation += 1;
        generation = state.next_generation;
        app.live = Some(generation);
    }
    shared.reading.finish(reader);
    verbatim_uia::lock::set_thread_lock(None);
    drop(identity);
}

/// Before a read on `pid`'s thread, whose process was created at
/// `created`: makes the thread's lock its application's, the one it holds
/// when that names the process, else the table's, waiting for it to
/// arrive. Answers what it was given, `None` for readers or a thread that
/// take no lock.
fn take_lock<R: Reading>(shared: &Shared<R>, pid: Pid, created: Option<u64>) -> Option<Granted> {
    let (locks, created) = (shared.locks.as_ref()?, created?);
    let held = verbatim_uia::lock::thread_lock();
    if held.as_ref().is_some_and(|lock| lock.names(pid.0, created)) {
        return held.map(Granted::Lock);
    }
    let started = Instant::now();
    let granted = locks.lock_for(held.as_ref(), pid, created);
    let waited_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    match &granted {
        Granted::Lock(lock) => {
            tracing::debug!(%pid, waited_us, "a reading thread took its application's lock");
            verbatim_uia::lock::set_thread_lock(Some(lock.clone()));
        }
        Granted::Without => {
            tracing::trace!(%pid, waited_us, "a reading thread reads without a lock: its application has no outpost");
            verbatim_uia::lock::set_thread_lock(None);
        }
        Granted::Stopped => {}
    }
    Some(granted)
}

/// The next event for the thread `generation` to read for `pid`, waiting
/// for one; `None` when the thread is no longer in charge, the readers are
/// stopping, or the application's outpost has ended and no event is
/// waiting ([`Readers::outpost_ended`]).
fn next_job<R: Reading>(shared: &Shared<R>, pid: Pid, generation: u64) -> Option<(Meta, R::Job)> {
    let mut state = shared.lock();
    loop {
        if state.stopping {
            return None;
        }
        let app = state.apps.get_mut(&pid)?;
        if app.live != Some(generation) {
            return None;
        }
        if let Some((place, meta, job)) = app.queue.pop_front() {
            let kind = shared
                .reading
                .key(&job)
                .map_or(FocusEvent::Focus, |(kind, _)| kind);
            app.running = Some((Instant::now(), meta, kind));
            app.running_place = place;
            shared.changed.notify_all();
            return Some((meta, job));
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

/// The watchdog: abandons a thread whose read has run past the deadline,
/// reports its event dropped, and gives the application's later events to
/// a new thread, unless too many of its threads are abandoned already.
fn watchdog<R: Reading>(shared: &Arc<Shared<R>>) {
    let mut state = shared.lock();
    loop {
        if state.stopping {
            return;
        }
        let due = state
            .apps
            .values()
            .filter_map(|app| app.running.map(|(started, _, _)| started + shared.deadline))
            .min();
        let Some(due) = due else {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
            continue;
        };
        let now = Instant::now();
        if now < due {
            state = shared
                .changed
                .wait_timeout(state, due - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
            continue;
        }
        let late: Vec<Pid> = state
            .apps
            .iter()
            .filter(|(_, app)| {
                app.running
                    .is_some_and(|(started, _, _)| now >= started + shared.deadline)
            })
            .map(|(pid, _)| *pid)
            .collect();
        for pid in late {
            abandon(shared, &mut state, pid);
        }
    }
}

/// Abandons the thread reading `pid`'s event, which has run past the
/// deadline.
fn abandon<R: Reading>(shared: &Arc<Shared<R>>, state: &mut ReadersState<R::Job>, pid: Pid) {
    let Some(app) = state.apps.get_mut(&pid) else {
        return;
    };
    let Some((_, meta, _)) = app.running.take() else {
        return;
    };
    app.live = None;
    app.abandoned += 1;
    let abandoned = app.abandoned;
    tracing::warn!(%pid, abandoned, "a UIA focus read did not return in time; its thread is abandoned");
    shared.reading.outcome(
        pid,
        meta,
        Judged::Dropped("its read did not return in time"),
    );
    if abandoned >= ABANDONED_LIMIT {
        for (_, meta, _) in app.queue.drain(..) {
            shared.reading.outcome(
                pid,
                meta,
                Judged::Dropped("its application's reads have not returned"),
            );
        }
    } else if !app.queue.is_empty() {
        start(shared, state, pid);
    }
    state.wake_idle_waiters();
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread::ThreadId;

    use super::*;

    /// A stand-in reading: each job is held until its gate opens, if it
    /// has one, and every outcome is sent to the test with the thread that
    /// read the event and the reason, as is each thread that ends.
    struct Script {
        outcomes: Mutex<Sender<(Pid, u64, &'static str)>>,
        read_on: Mutex<Sender<(u64, ThreadId)>>,
        finished: Mutex<Sender<ThreadId>>,
        /// Each read's marker and the lock its thread read with.
        locked: Mutex<Sender<(u64, Option<verbatim_uia::lock::AppLock>)>>,
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
        ) -> Judged {
            let _ = self
                .locked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send((marker, verbatim_uia::lock::thread_lock()));
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
            Judged::Dropped("read")
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
        finished: Receiver<ThreadId>,
        locked: Receiver<(u64, Option<verbatim_uia::lock::AppLock>)>,
    }

    fn fixture(deadline: Duration) -> Fixture {
        fixture_with(deadline, None)
    }

    fn fixture_with(deadline: Duration, locks: Option<Arc<AppLocks>>) -> Fixture {
        let (outcomes_tx, outcomes) = mpsc::channel();
        let (read_tx, read_on) = mpsc::channel();
        let (finished_tx, finished) = mpsc::channel();
        let (locked_tx, locked) = mpsc::channel();
        Fixture {
            readers: Readers::with_locks(
                Script {
                    outcomes: Mutex::new(outcomes_tx),
                    read_on: Mutex::new(read_tx),
                    finished: Mutex::new(finished_tx),
                    locked: Mutex::new(locked_tx),
                },
                deadline,
                locks,
            ),
            outcomes,
            read_on,
            finished,
            locked,
        }
    }

    /// The test's own process, which a reading thread can open, and its
    /// creation time.
    fn this_process() -> (Pid, u64) {
        let (_, created) =
            verbatim_uia::lock::open_process(std::process::id()).expect("the test's own process");
        (Pid(std::process::id()), created)
    }

    fn app_lock((pid, created): (Pid, u64)) -> verbatim_uia::lock::AppLock {
        verbatim_uia::lock::AppLock::new(
            pid.0,
            created,
            Arc::new(verbatim_uia::lock::new_mutex().expect("a mutex")),
        )
    }

    /// The mutex a read's lock holds, by its handle's address, or `None`
    /// for a read without one.
    fn mutex_of(lock: Option<&verbatim_uia::lock::AppLock>) -> Option<usize> {
        lock.map(|lock| Arc::as_ptr(lock.mutex()) as usize)
    }

    #[test]
    fn a_reading_thread_waits_for_its_lock_and_reads_once_it_arrives() {
        let locks = Arc::new(AppLocks::default());
        let fixture = fixture_with(Duration::from_secs(60), Some(Arc::clone(&locks)));
        let process = this_process();
        fixture
            .readers
            .push(process.0, meta(1), (1, None, Vec::new()));
        locks.found_nothing(1);
        assert!(
            fixture.locked.try_recv().is_err(),
            "nothing was read before the lock"
        );
        let lock = app_lock(process);
        locks.add(lock.clone());
        let (marker, read_with) = fixture.locked.recv_timeout(WAIT).expect("a read");
        assert_eq!(
            (marker, mutex_of(read_with.as_ref())),
            (1, mutex_of(Some(&lock)))
        );
        assert_eq!(fixture.readers.stop(), 1);
    }

    #[test]
    fn a_lock_whose_creation_time_differs_from_the_processes_is_not_taken() {
        let locks = Arc::new(AppLocks::default());
        let fixture = fixture_with(Duration::from_secs(60), Some(Arc::clone(&locks)));
        let process = this_process();
        fixture
            .readers
            .push(process.0, meta(1), (1, None, Vec::new()));
        locks.found_nothing(1);
        locks.add(app_lock((process.0, process.1 + 1)));
        // The thread looked again and found nothing for its process.
        locks.found_nothing(2);
        assert!(
            fixture.locked.try_recv().is_err(),
            "nothing was read with the other lock"
        );
        let lock = app_lock(process);
        locks.add(lock.clone());
        let (_, read_with) = fixture.locked.recv_timeout(WAIT).expect("a read");
        assert_eq!(mutex_of(read_with.as_ref()), mutex_of(Some(&lock)));
        assert_eq!(fixture.readers.stop(), 1);
    }

    #[test]
    fn an_answer_with_no_lock_and_the_processes_creation_time_lets_its_thread_read() {
        let locks = Arc::new(AppLocks::default());
        let fixture = fixture_with(Duration::from_secs(60), Some(Arc::clone(&locks)));
        let process = this_process();
        fixture
            .readers
            .push(process.0, meta(1), (1, None, Vec::new()));
        locks.found_nothing(1);
        locks.add_unlocked(process.0, Some(process.1));
        let (_, read_with) = fixture.locked.recv_timeout(WAIT).expect("a read");
        assert_eq!(mutex_of(read_with.as_ref()), None);
        assert_eq!(fixture.readers.stop(), 1);
    }

    #[test]
    fn an_answer_with_no_lock_and_no_creation_time_lets_its_thread_read() {
        let locks = Arc::new(AppLocks::default());
        let fixture = fixture_with(Duration::from_secs(60), Some(Arc::clone(&locks)));
        let process = this_process();
        fixture
            .readers
            .push(process.0, meta(1), (1, None, Vec::new()));
        locks.found_nothing(1);
        locks.add_unlocked(process.0, None);
        let (_, read_with) = fixture.locked.recv_timeout(WAIT).expect("a read");
        assert_eq!(mutex_of(read_with.as_ref()), None);
        assert_eq!(fixture.readers.stop(), 1);
    }

    #[test]
    fn a_thread_that_cannot_open_its_process_reads_without_a_lock() {
        let locks = Arc::new(AppLocks::default());
        let fixture = fixture_with(Duration::from_secs(60), Some(Arc::clone(&locks)));
        // No process has this id: process ids are multiples of four.
        let gone = Pid(u32::MAX - 2);
        fixture.readers.push(gone, meta(1), (1, None, Vec::new()));
        let (_, read_with) = fixture.locked.recv_timeout(WAIT).expect("a read");
        assert_eq!(mutex_of(read_with.as_ref()), None);
        assert_eq!(fixture.readers.stop(), 1);
    }

    #[test]
    fn a_thread_holding_its_lock_reads_with_it_after_the_table_lets_it_go() {
        let locks = Arc::new(AppLocks::default());
        let fixture = fixture_with(Duration::from_secs(60), Some(Arc::clone(&locks)));
        let process = this_process();
        let lock = app_lock(process);
        locks.add(lock.clone());
        fixture
            .readers
            .push(process.0, meta(1), (1, None, Vec::new()));
        let (_, first) = fixture.locked.recv_timeout(WAIT).expect("the first read");
        locks.close(process.0, process.1);
        fixture
            .readers
            .push(process.0, meta(2), (2, None, Vec::new()));
        let (_, second) = fixture.locked.recv_timeout(WAIT).expect("the second read");
        assert_eq!(
            (mutex_of(first.as_ref()), mutex_of(second.as_ref())),
            (mutex_of(Some(&lock)), mutex_of(Some(&lock)))
        );
        assert_eq!(fixture.readers.stop(), 1);
    }

    #[test]
    fn the_listeners_shutdown_ends_a_thread_that_waits_for_its_lock() {
        let locks = Arc::new(AppLocks::default());
        let fixture = fixture_with(Duration::from_secs(60), Some(Arc::clone(&locks)));
        let process = this_process();
        fixture
            .readers
            .push(process.0, meta(1), (1, None, Vec::new()));
        locks.found_nothing(1);
        assert_eq!(fixture.readers.stop(), 1);
        assert_eq!(
            fixture.outcomes.recv_timeout(WAIT).expect("an outcome"),
            (process.0, 1, "the listener is shutting down")
        );
        assert!(fixture.locked.try_recv().is_err(), "nothing was read");
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
