//! The unasked UI Automation focus read that mockapp's integration
//! harness drives as its stand-in for the listener ([`FocusReader`]),
//! until that harness moves onto the request driver (design
//! `docs/design/readers-tell-core-asks-build1.md`, "The outpost in this
//! build"). The listener process makes no unasked read: each UI Automation
//! event is checked on its application's check thread and told, and Core
//! asks for its kept element's read (`crate::listener`, `asked`). Its call
//! log (`CallLog`) and the depth of an ancestor walk
//! (`MAX_ANCESTORS`) are shared with the listener's asked read.
//!
//! [`FocusReader::read`] makes NVDA's focus handler's checks in their
//! order (`verbatim_listener_logic::checks`), each call made here, then the
//! classic walk on the element the event named: whether an earlier
//! element with the same runtime id still has the keyboard focus, the
//! element afresh, its selected child, and its ancestors up to the first
//! one its last read knew, whose ancestors it reuses while every window
//! title saved with them reads the same, or to the desktop root or the
//! process boundary. A step that fails ends the walk, the chain going on
//! from the parent window of the outermost element's nearest window, as
//! NVDA's failed parent read goes on by `Window._get_parent`; a step that
//! reaches a ghost's or a hung application's window ends it there.
//! [`FocusReader::live_reread`] reads its last focus again with no cache,
//! and [`FocusReader::check_fact`] makes NVDA's checks of a selection's,
//! a notification's and a toast's sender. A call over a second is logged
//! (`CallLog`).

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_LegacyIAccessibleStatePropertyId,
};

use verbatim_model::{NodeDetails, State, TraceId};
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
    Arrival, FocusEvent, Judged, NO_WINDOW, NOT_FOCUSED, NOT_THE_LAST_READ, NOTIFICATION_NO_WINDOW,
    QueuePlace, TOAST_NOT_CORE_WINDOW, TOAST_NOT_READ,
};
use verbatim_model::protocol::{
    AncestorsEnd, DeliveredFact, UiaAncestorFact, UiaFocusRead, UiaSnapshotFact,
};
use verbatim_reader_rules::Window;
use verbatim_reader_rules::acceptance::{EventName, RequestedEvents, accepts_event};
use verbatim_reader_rules::ancestry_cache::{CacheUse, SavedTitle, title_check};
use verbatim_win32::window::ForegroundReader;
use verbatim_win32::window::SystemWindows;
use verbatim_win32::window::WindowReadabilityReader;
use verbatim_win32::window::window_belongs_to_hidden_frame;
use verbatim_win32::window::window_class_name;

/// The most ancestors read for a focus, as many as the outpost walks.
pub(crate) const MAX_ANCESTORS: u32 = 64;

/// The log of one focus read's calls, for diagnosis alone: it times each
/// call by the system's clock, and a call that takes longer than
/// [`SLOW_CALL`] is logged at info with its name and how long it took, so
/// the slowest calls of a read can be seen (Dickson, 2026-10-20). Each
/// call costs one clock reading, at its start, which is also the end of
/// the call before it; the last ends when the log is dropped, at the
/// read's end. It ends nothing.
#[derive(Default)]
pub(crate) struct CallLog {
    /// The call under way, by name, and when it began.
    call: Cell<Option<(&'static str, Instant)>>,
}

/// How long one call of a focus read may take before it is logged at info.
const SLOW_CALL: Duration = Duration::from_secs(1);

impl CallLog {
    /// The call named `name` begins, and the one before it, if any, has
    /// ended.
    pub(crate) fn call(&self, name: &'static str) {
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

    /// Judges and reads the UIA focus event, or menu opening (`kind`),
    /// whose sender is `sender`, as the registration delivers it, and which
    /// came as `arrival`. Every call waits for the application, up to UIA's
    /// transaction timeout.
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
                counts: None,
                ..Node::all_on()
            },
            ..Ancestor::all_on()
        };
        let read = self
            .uia
            .request::<Ancestor>(ancestors)
            .and_then(|request| {
                Ok((
                    request,
                    self.uia.request::<Ancestor>(AncestorGates {
                        node: NodeGates {
                            counts: None,
                            ..Node::all_on()
                        },
                        ..Ancestor::all_on()
                    })?,
                ))
            })
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
                        Ok(Some(nearest)) => Nearest::Found(nearest),
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

#[cfg(test)]
mod tests {
    use super::*;

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
