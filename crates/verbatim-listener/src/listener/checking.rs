//! The listener's check threads (design
//! `docs/design/readers-tell-core-asks-build1.md`, "Focus and foreground",
//! "The listener in this build" and Tables 6 and 11): every UI Automation
//! event the listener hears, but the progress path's range-value changes,
//! goes from its callback, which makes no call, to its application's check
//! thread, which makes every call NVDA's handler for that event's kind
//! makes at delivery ([`verbatim_listener_logic::check`]), then tells Core
//! of it with their answers, one application's events in their order.
//!
//! The records and their queues sit under one lock ([`Checks::state`]),
//! with the decisions of [`verbatim_listener_logic::check_threads`]; a tell
//! is placed under it, and under the lock that orders the listener's tells
//! ([`Tells`]), so no tell of an ended application is sent and the idle
//! barrier's answer follows every tell of an event pushed before it. The
//! element of each told event is kept behind the token the tell names
//! ([`verbatim_listener_logic::kept`]), and every element let go is
//! released as a job on its application's reading thread.
//!
//! The lock order is the reading threads' lock, then this one, then the
//! tells': the idle barrier passes from the reading threads to here, and a
//! release is handed to the reading threads only once this lock is let go.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use windows::Win32::UI::Accessibility::{
    IUIAutomationTreeWalker, UIA_ListItemControlTypeId, UIA_MenuItemControlTypeId,
};

use verbatim_listener_logic::check::{Call, Check, ProbeRecord, SWITCHER_ITEM_CLASS, Walked};
use verbatim_listener_logic::check_threads::{CheckThreads, Pushed, Take};
use verbatim_listener_logic::kept::{Ended, KeptElements, Start};
use verbatim_listener_logic::schedule::JobKind;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::Probe;
use verbatim_model::tell::{ElementToken, EventObject, Payload, Read};
use verbatim_model::{Pid, StateSet, TraceId, WindowHandle};
use verbatim_process::clock::now_us;
use verbatim_reader_rules::class::class_verdict;
use verbatim_uia::{Agile, Cached, Direction, ElementExt, ListenerEvent, Request, Uia};

use super::Outgoing;
use super::asked::{UiaJob, UiaReading};
use super::tells::{Applications, Held, Pending, PendingWindow, SystemReads, Tells};
use crate::reading::{IdleWaiter, Readers};
use verbatim_win32::window_facts::WindowPart;

/// The UI Automation class of the Office menu item NVDA's
/// `PlaceholderNetUITWMenuItem` is made of (`NVDAObjects/UIA/__init__.py`
/// 1227 to 1231).
const NET_UI_MENU_ITEM_CLASS: &str = "NetUITWMenuItem";

/// The name of each check thread.
const CHECK_THREAD: &str = "verbatim-listener-check";

/// One UI Automation event, as its callback hands it to its application's
/// check thread: the element, kept with no call, and what came with it.
pub(super) struct CheckEvent {
    trace_id: TraceId,
    kind: EventKind,
    process: Pid,
    /// When the callback received it, on the run's shared clock.
    at_us: u64,
    element: Agile<ListenerEvent>,
    runtime_id: RuntimeId,
    /// The element's own window from its cache, 0 for none.
    own_window: isize,
    /// Its kind's properties from its cache.
    cached: PropertyValues,
    payload: Payload,
    /// Whether its cache names a `NetUITWMenuItem` menu item with no name,
    /// whose previous sibling NVDA's object reads.
    placeholder_candidate: bool,
    /// Whether its cache names a list item of the task switcher's class,
    /// whose parent's automation id the check reads.
    switcher_item: bool,
}

impl CheckEvent {
    /// The event of `kind` whose sender is `sender`, read as `fields`, of
    /// `process`, received at `at_us`: `states` are its states when its
    /// kind caches them (an element selected), `payload` what its kind
    /// brings beyond its properties. Local calls: `None` when the element
    /// cannot be kept for its check.
    pub(super) fn new(
        kind: EventKind,
        (sender, fields): (&Cached<ListenerEvent>, &ListenerEvent),
        process: Pid,
        at_us: u64,
        (states, payload): (Option<StateSet>, Payload),
    ) -> Option<Self> {
        let element = match sender.agile() {
            Ok(element) => element,
            Err(error) => {
                tracing::warn!(%error, ?kind, "a UIA event's element could not be kept for its check");
                return None;
            }
        };
        let name = fields.name.as_deref().unwrap_or_default();
        Some(Self {
            trace_id: TraceId::mint(),
            kind,
            process,
            at_us,
            element,
            runtime_id: RuntimeId(sender.runtime_id()),
            own_window: fields.role.hwnd,
            cached: cached_of(kind, fields, process, states),
            payload,
            placeholder_candidate: fields.role.class_name.as_deref()
                == Some(NET_UI_MENU_ITEM_CLASS)
                && fields.role.control_type == Some(UIA_MenuItemControlTypeId.0)
                && name.is_empty(),
            switcher_item: fields.role.class_name.as_deref() == Some(SWITCHER_ITEM_CLASS)
                && fields.role.control_type == Some(UIA_ListItemControlTypeId.0),
        })
    }
}

/// The properties `kind` caches ([`verbatim_model::event_kind::EventKindEntry::cached`]),
/// read from the event's cache, `fields`, of `process`, with `states` for a
/// kind that caches them: no call.
fn cached_of(
    kind: EventKind,
    fields: &ListenerEvent,
    process: Pid,
    states: Option<StateSet>,
) -> PropertyValues {
    let text = |value: &Option<String>| {
        value
            .as_ref()
            .filter(|text| !text.is_empty())
            .map(|text| PropertyValue::Text(text.clone()))
    };
    PropertyValues(
        kind.entry()
            .cached
            .iter()
            .map(|&id| PropertyAnswer {
                id,
                value: match id {
                    PropertyId::Name => text(&fields.name),
                    PropertyId::Role => Some(PropertyValue::Role(fields.role.role())),
                    PropertyId::States => states.map(PropertyValue::States),
                    PropertyId::ClassName => text(&fields.role.class_name),
                    PropertyId::AutomationId => text(&fields.automation_id),
                    PropertyId::NativeWindow => fields.role.window().map(PropertyValue::Window),
                    PropertyId::Process => Some(PropertyValue::Process(process)),
                    PropertyId::HasTextPattern => Some(PropertyValue::Flag(fields.text_available)),
                    // No kind the listener hears caches another.
                    _ => None,
                },
            })
            .collect(),
    )
}

/// `cached` with `answer` in its property's place, when there is one.
fn with_answer(mut cached: PropertyValues, answer: Option<PropertyAnswer>) -> PropertyValues {
    if let Some(answer) = answer {
        match cached.0.iter_mut().find(|cached| cached.id == answer.id) {
            Some(place) => *place = answer,
            None => cached.0.push(answer),
        }
    }
    cached
}

/// Every application's kept elements (Table 6), under their own lock,
/// taken after the reading threads' and the check threads' locks, with no
/// call made under it: an element let go is handed back, for release on
/// its application's reading thread.
#[derive(Default)]
pub(crate) struct Kept {
    kept: Mutex<HashMap<Pid, KeptElements<Agile<ListenerEvent>>>>,
}

impl Kept {
    fn lock(&self) -> MutexGuard<'_, HashMap<Pid, KeptElements<Agile<ListenerEvent>>>> {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The application whose elements hold `token`.
    pub(crate) fn owner(&self, token: ElementToken) -> Option<Pid> {
        self.lock()
            .iter()
            .find(|(_, kept)| kept.holds(token))
            .map(|(pid, _)| *pid)
    }

    /// `pid`'s reading thread starts the read of `token` (column A).
    pub(crate) fn start_read(&self, pid: Pid, token: ElementToken) -> Start<Agile<ListenerEvent>> {
        self.lock()
            .get_mut(&pid)
            .map_or(Start::NotThere, |kept| kept.start_read(token))
    }

    /// The read of `pid`'s `token` ended with `element` (column B): the
    /// element to release, if any.
    pub(crate) fn read_ended(
        &self,
        pid: Pid,
        token: ElementToken,
        element: Agile<ListenerEvent>,
        focus: bool,
    ) -> Option<Agile<ListenerEvent>> {
        let mut kept = self.lock();
        match kept.get_mut(&pid) {
            Some(kept) => match kept.read_ended(token, element, focus) {
                Ended::Kept => None,
                Ended::Release(element) => Some(element),
            },
            None => Some(element),
        }
    }

    /// A focus request names the readers' last focus, `named` (column C):
    /// every other read element of every application, with its
    /// application, to release.
    pub(crate) fn focus_request(
        &self,
        named: Option<ElementToken>,
    ) -> Vec<(Pid, Vec<Agile<ListenerEvent>>)> {
        self.lock()
            .iter_mut()
            .map(|(pid, kept)| (*pid, kept.focus_request(named)))
            .filter(|(_, released)| !released.is_empty())
            .collect()
    }

    /// Keeps `element` of `pid`'s behind `token`, received at `at_us`, and
    /// sweeps the application's kept elements at `now_us` (column D): the
    /// elements to release.
    fn keep(
        &self,
        (pid, token): (Pid, ElementToken),
        (element, runtime_id): (Agile<ListenerEvent>, RuntimeId),
        (at_us, now_us): (u64, u64),
    ) -> Vec<Agile<ListenerEvent>> {
        let mut kept = self.lock();
        let kept = kept.entry(pid).or_default();
        kept.keep(token, element, runtime_id, at_us);
        kept.sweep(now_us)
    }

    /// The kept elements of `pid`'s with `runtime_id`, with their tokens.
    fn with_runtime_id(
        &self,
        pid: Pid,
        runtime_id: &RuntimeId,
    ) -> Vec<(ElementToken, Agile<ListenerEvent>)> {
        self.lock()
            .get(&pid)
            .map(|kept| {
                kept.with_runtime_id(runtime_id)
                    .map(|(token, element)| (token, element.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `pid`'s application ended (column D): every element it keeps but
    /// one in read.
    fn ended(&self, pid: Pid) -> Vec<Agile<ListenerEvent>> {
        self.lock()
            .remove(&pid)
            .map(|mut kept| kept.ended())
            .unwrap_or_default()
    }

    /// Every element kept, for the listener's shutdown.
    fn clear(&self) -> HashMap<Pid, KeptElements<Agile<ListenerEvent>>> {
        std::mem::take(&mut *self.lock())
    }
}

/// The records under the lock: the threads' decisions, and the wake of
/// each application's thread.
struct State {
    threads: CheckThreads<CheckEvent, IdleWaiter>,
    wakes: HashMap<Pid, Arc<Condvar>>,
}

/// Every application's check thread, the elements kept for their tells,
/// and what the threads place tells and release elements through.
pub(crate) struct Checks {
    state: Mutex<State>,
    kept: Arc<Kept>,
    /// The last token minted.
    tokens: AtomicU64,
    /// Each application's executable name, for its tells.
    applications: Mutex<Applications>,
    outgoing: Arc<Outgoing>,
    readers: Arc<Readers<UiaReading>>,
}

impl Checks {
    pub(super) fn new(
        outgoing: &Arc<Outgoing>,
        readers: &Arc<Readers<UiaReading>>,
        kept: &Arc<Kept>,
    ) -> Self {
        Self {
            state: Mutex::new(State {
                threads: CheckThreads::default(),
                wakes: HashMap::new(),
            }),
            kept: Arc::clone(kept),
            tokens: AtomicU64::new(0),
            applications: Mutex::new(Applications::default()),
            outgoing: Arc::clone(outgoing),
            readers: Arc::clone(readers),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn tells(&self) -> &Tells {
        &self.outgoing.tells
    }

    /// Hands `event` of `pid`'s to its application's check thread,
    /// starting one when it has none. Never waits on a call.
    pub(super) fn hand(self: &Arc<Self>, pid: Pid, event: CheckEvent) {
        let mut state = self.lock();
        match state.threads.push(pid, event) {
            Pushed::Start { generation } => {
                let wake = Arc::new(Condvar::new());
                state.wakes.insert(pid, Arc::clone(&wake));
                let checks = Arc::clone(self);
                let spawned = std::thread::Builder::new()
                    .name(CHECK_THREAD.to_owned())
                    .spawn(move || run(&checks, pid, generation, &wake));
                if let Err(error) = spawned {
                    tracing::error!(%pid, %error, "a check thread could not be started");
                    let (dropped, waiters) = state.threads.ended(pid);
                    state.wakes.remove(&pid);
                    for waiter in waiters {
                        waiter();
                    }
                    drop(state);
                    self.release(
                        pid,
                        dropped.into_iter().map(|event| event.element).collect(),
                    );
                }
            }
            Pushed::Wake => {
                if let Some(wake) = state.wakes.get(&pid) {
                    wake.notify_one();
                }
            }
            Pushed::Queued => {}
            // The listener is stopping: its element is released here,
            // never the last reference while its callback holds one.
            Pushed::Dropped(event) => drop(event),
        }
    }

    /// Runs `waiter` once every event handed before it is told or dropped,
    /// at once when none waits, under the lock, so the idle barrier's
    /// answer follows their tells. Dropped once the listener is stopping.
    pub(super) fn when_idle(&self, waiter: IdleWaiter) {
        let mut state = self.lock();
        if let Some(waiter) = state.threads.when_idle(waiter) {
            waiter();
        }
    }

    /// `pid`'s process ended: its check thread's queued events are dropped
    /// untold and its running check's tell dropped, and every element it
    /// keeps is released on its reading thread, before the reading threads
    /// learn of the end, so the thread that drains its events releases
    /// them.
    pub(super) fn application_exited(&self, pid: Pid) {
        let mut state = self.lock();
        let (dropped, waiters) = state.threads.ended(pid);
        if let Some(wake) = state.wakes.remove(&pid) {
            wake.notify_one();
        }
        for waiter in waiters {
            waiter();
        }
        drop(state);
        let kept = self.kept.ended(pid);
        let released = dropped
            .into_iter()
            .map(|event| event.element)
            .chain(kept)
            .collect();
        self.release(pid, released);
    }

    /// The listener shuts down: every queued event is dropped untold,
    /// every thread ends at its next take, one inside a call abandoned
    /// rather than waited for, and every kept element is released here.
    pub(super) fn stop(&self) {
        let mut state = self.lock();
        let dropped = state.threads.stop();
        for wake in state.wakes.drain().map(|(_, wake)| wake) {
            wake.notify_one();
        }
        drop(state);
        drop(dropped);
        drop(self.kept.clear());
    }

    /// Releases `elements` of `pid`'s as a job on its reading thread.
    pub(super) fn release(&self, pid: Pid, elements: Vec<Agile<ListenerEvent>>) {
        if elements.is_empty() {
            return;
        }
        self.readers
            .push(pid, JobKind::Release, UiaJob::Release(elements));
    }

    /// The kept elements of `pid`'s with `runtime_id`, with their tokens,
    /// for the check thread to read: their references, taken under the
    /// kept elements' lock and read once it is let go.
    fn kept_with(
        &self,
        pid: Pid,
        runtime_id: &RuntimeId,
    ) -> Vec<(ElementToken, Agile<ListenerEvent>)> {
        self.kept.with_runtime_id(pid, runtime_id)
    }

    /// The end of `pid`'s thread `generation`'s check of `event`, which
    /// found `window` and `checked`: told, its element kept behind a new
    /// token, while the record is the thread's own; else its element
    /// released here, at its check's end.
    fn checked(&self, (pid, generation): (Pid, u64), event: CheckEvent, told: Told) {
        let mut state = self.lock();
        let (own, waiters) = state.threads.checked(pid, generation);
        let mut released = Vec::new();
        let mut unplaced = None;
        if own {
            let token = ElementToken(self.tokens.fetch_add(1, Ordering::Relaxed).wrapping_add(1));
            released = self.kept.keep(
                (pid, token),
                (event.element, event.runtime_id.clone()),
                (event.at_us, now_us()),
            );
            let application = self
                .applications
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .name(event.process);
            let tell = Pending {
                trace_id: event.trace_id,
                kind: event.kind,
                process: event.process,
                application,
                thread: told.thread,
                at_us: event.at_us,
                object: EventObject::Uia {
                    token,
                    runtime_id: event.runtime_id,
                },
                cached: with_answer(event.cached, told.parent),
                payload: event.payload,
                window: told.window,
                checked: Some(told.checked),
            };
            self.tells().place(Held::Tell(Box::new(tell)));
        } else {
            unplaced = Some(event.element);
        }
        for waiter in waiters {
            waiter();
        }
        drop(state);
        drop(unplaced);
        self.release(pid, released);
    }
}

/// What a check found, for its tell.
struct Told {
    window: PendingWindow,
    /// The thread that owns the window found, 0 for none.
    thread: u32,
    checked: verbatim_model::tell::Checked,
    /// The parent's automation id the check read, which the tell carries
    /// among its cached properties.
    parent: Option<PropertyAnswer>,
}

/// A check thread: checks `pid`'s events in order while its record is its
/// own, as `generation`, waiting for the next one up to the idle time,
/// then lets its UI Automation client go and ends. Its client is created
/// after the process's first setup of UI Automation, which
/// [`Uia::new`] waits for, since a cache request fails while that setup
/// runs on another thread
/// (`uia-facts/a-cache-request-fails-while-uias-first-time-setup-runs-on-another-thread`).
fn run(checks: &Arc<Checks>, pid: Pid, generation: u64, wake: &Arc<Condvar>) {
    let uia = Uia::new()
        .inspect_err(
            |error| tracing::warn!(%pid, %error, "a check thread could not create a UIA client"),
        )
        .ok();
    let walker = uia.as_ref().and_then(|uia| uia.raw_view_walker().ok());
    let parent_request = uia
        .as_ref()
        .and_then(|uia| uia.request_fixed::<ListenerEvent>().ok());
    let mut record = ProbeRecord::default();
    while let Some(event) = next(checks, pid, generation, wake) {
        let told = check(
            checks,
            (pid, &event),
            (walker.as_ref(), parent_request.as_ref()),
            &mut record,
        );
        checks.checked((pid, generation), event, told);
    }
    drop(parent_request);
    drop(walker);
    let joined = uia.is_some();
    drop(uia);
    verbatim_uia::release_thread_state();
    if joined {
        verbatim_uia::leave_mta();
    }
}

/// The next event for `pid`'s thread `generation` to check, waiting for
/// one up to the idle time; `None` when the thread is to end.
fn next(checks: &Checks, pid: Pid, generation: u64, wake: &Arc<Condvar>) -> Option<CheckEvent> {
    let mut state = checks.lock();
    loop {
        let now = now_us();
        match state.threads.take(pid, generation, now) {
            Take::Check(event) => return Some(event),
            Take::Wait { until_us } => {
                let wait = Duration::from_micros(until_us.saturating_sub(now));
                state = wake
                    .wait_timeout(state, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            Take::Exit => {
                if state
                    .wakes
                    .get(&pid)
                    .is_some_and(|own| Arc::ptr_eq(own, wake))
                {
                    state.wakes.remove(&pid);
                }
                return None;
            }
        }
    }
}

/// Makes every call NVDA's handler for `event`'s kind makes at delivery
/// ([`Check`]), with no lock held, each with UI Automation's transaction
/// timeout, and gathers what its tell carries.
fn check(
    checks: &Checks,
    (pid, event): (Pid, &CheckEvent),
    (walker, parent_request): (
        Option<&IUIAutomationTreeWalker>,
        Option<&Request<ListenerEvent>>,
    ),
    record: &mut ProbeRecord,
) -> Told {
    let sender = event.element.resolve().ok();
    let kept = checks.kept_with(pid, &event.runtime_id);
    let mut check = Check::new(
        event.kind,
        event.placeholder_candidate,
        kept.iter().map(|(token, _)| *token).collect(),
    )
    .reading_parent(event.switcher_item);
    let failed = || Read::Failed;
    while let Some(call) = check.next(record, now_us()) {
        match call {
            Call::NearestWindow => check.walked(sender.as_ref().map_or(Read::Failed, |sender| {
                verbatim_uia::nearest_window_lookup(sender.live().element())
                    .map_or(Read::Failed, Read::Answered)
            })),
            Call::Classes(window) => {
                check.classed(class_verdict(&verbatim_win32::window::window_classes(
                    window,
                )));
            }
            Call::Probe(window) => {
                let began_us = now_us();
                let (thread, _) = verbatim_win32::window::window_owner(window);
                let provider = verbatim_uia::raw_probe(window).has_provider();
                check.probed(
                    Probe {
                        window: handle(window),
                        thread,
                        provider,
                        began_us,
                    },
                    record,
                );
            }
            Call::KeptFocus(token) => {
                let focused = kept
                    .iter()
                    .find(|(kept, _)| *kept == token)
                    .and_then(|(_, element)| element.resolve().ok())
                    .map_or_else(failed, |element| keyboard_focus(&element));
                check.kept_focus(token, focused);
            }
            Call::PreviousSibling => {
                let has = match (sender.as_ref(), walker) {
                    (Some(sender), Some(walker)) => {
                        match sender.live().step(walker, Direction::PreviousSibling) {
                            Ok(_) => Read::Answered(true),
                            // The null element `windows` reports for none.
                            Err(error) if error.code().is_ok() => Read::Answered(false),
                            Err(_) => Read::Failed,
                        }
                    }
                    _ => Read::Failed,
                };
                check.previous_sibling(has);
            }
            Call::ParentAutomationId => {
                let id = match (sender.as_ref(), walker, parent_request) {
                    (Some(sender), Some(walker), Some(request)) => {
                        match sender.live().walk(walker, Direction::Parent, request) {
                            Ok(parent) => Read::Answered(parent.read().automation_id),
                            // The null element `windows` reports for none.
                            Err(error) if error.code().is_ok() => Read::Answered(None),
                            Err(_) => Read::Failed,
                        }
                    }
                    _ => Read::Failed,
                };
                check.parent_automation_id(id);
            }
            Call::KeyboardFocus => {
                check.keyboard_focus(sender.as_ref().map_or_else(failed, keyboard_focus));
            }
        }
    }
    let (found, checked, parent) = check.finish();
    let window = match found {
        Walked::Found(window) => Some(window),
        Walked::NotWalked => (event.own_window != 0).then_some(event.own_window),
        Walked::NoWindow | Walked::Failed => None,
    };
    Told {
        window: match (window, found) {
            (Some(window), _) => PendingWindow::Found(WindowPart::read(&SystemReads, window, None)),
            (None, Walked::Failed) => PendingWindow::Failed,
            (None, _) => PendingWindow::NoWindow,
        },
        thread: window.map_or(0, |window| verbatim_win32::window::window_owner(window).0),
        checked,
        parent,
    }
}

/// Whether `element` has the keyboard focus, read live: a failed read is
/// told as one.
fn keyboard_focus(element: &Cached<ListenerEvent>) -> Read<bool> {
    element
        .live()
        .element()
        .has_keyboard_focus()
        .map_or(Read::Failed, Read::Answered)
}

/// `window` as the model names a window.
fn handle(window: isize) -> WindowHandle {
    WindowHandle(i64::try_from(window).unwrap_or(0).cast_unsigned())
}
