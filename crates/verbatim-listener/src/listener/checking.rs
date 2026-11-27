//! The listener's check threads (`books/dev/src/reference/crates/verbatim-listener.md`,
//! "Tells, checks and Core's requests"; `books/dev/src/reference/crates/verbatim-listener-logic.md`,
//! "The check", "The check threads" and "Kept elements"): every UI Automation
//! event the listener hears goes from its callback, which makes no call,
//! to its application's check
//! thread, which makes every call NVDA's handler for that event's kind
//! makes at delivery ([`verbatim_listener_logic::check`]), then tells Core
//! of it with their answers, one application's events in their order.
//!
//! The records and their queues sit under one lock ([`Checks::state`]),
//! with the decisions of [`verbatim_listener_logic::check_threads`]; a tell
//! is placed under it, and under the lock that orders the listener's tells
//! ([`Tells`]), so no tell of an ended application is sent and the idle
//! barrier's answer follows every tell of an event pushed before it. The
//! element of each told event but a progress bar's is kept behind the
//! token the tell names
//! ([`verbatim_listener_logic::kept`]) until Core's held view leaves it
//! out, and every element a view lets go is released as a job on its
//! application's reading thread.
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

use verbatim_listener_logic::check::{
    Call, CheckCalls, CheckFacts, ProbeRecord, SWITCHER_ITEM_CLASS, Walked,
};
use verbatim_listener_logic::check_threads::{CheckThreads, Pushed, Take};
use verbatim_listener_logic::kept::{ListenerKept, Start, View};
use verbatim_listener_logic::schedule::JobKind;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::Probe;
use verbatim_model::tell::{AcceptanceFacts, ElementToken, EventObject, Payload, Read};
use verbatim_model::{Pid, StateSet, TraceId, WindowHandle};
use verbatim_process::clock::now_us;
use verbatim_reader_rules::class::WindowClasses;
use verbatim_uia::{Agile, Cached, Direction, ElementExt, ListenerEvent, Request, Uia};

use super::Outgoing;
use super::asked::{UiaJob, UiaReading};
use super::tells::{Applications, Held, Known, Pending, PendingWindow, SystemReads, Tells, told};
use crate::reading::{IdleWaiter, Readers};
use verbatim_model::hung::WindowState;
use verbatim_model::tell::Hung;
use verbatim_win32::window_facts::{Raised, Served, WindowPart, WindowReads};

/// The UI Automation class of the Office menu item NVDA's
/// `PlaceholderNetUITWMenuItem` is made of (`NVDAObjects/UIA/__init__.py`
/// 1227 to 1231).
const NET_UI_MENU_ITEM_CLASS: &str = "NetUITWMenuItem";

/// The name of each check thread.
const CHECK_THREAD: &str = "verbatim-listener-check";

/// One UI Automation event, as its callback hands it to its application's
/// check thread: the element, kept with no call, and what came with it.
/// Public so that mockapp's tests check and tell mockapp's own events
/// with the listener's code ([`check_event`], [`CheckEvent::into_tell`]).
pub struct CheckEvent {
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
    #[must_use]
    pub fn new(
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

    /// The element's runtime id, from its event's cache.
    #[must_use]
    pub fn runtime_id(&self) -> &RuntimeId {
        &self.runtime_id
    }

    /// The tell of this event, whose check found `told`: its object made
    /// by `object` from the element and its runtime id (kept behind a
    /// token, or named by its runtime id alone), its application's name
    /// `application`, and the parent's automation id the check read in its
    /// cached properties' place. Its window's foreground facts, when its
    /// check did not read them, are read as it is placed
    /// ([`Pending::finish`]).
    pub fn into_tell(
        self,
        told: Told,
        object: impl FnOnce(Agile<ListenerEvent>, RuntimeId) -> EventObject,
        application: String,
    ) -> Pending {
        let event = self;
        Pending {
            trace_id: event.trace_id,
            kind: event.kind,
            process: event.process,
            application,
            thread: told.thread,
            at_us: event.at_us,
            object: object(event.element, event.runtime_id),
            cached: with_answer(event.cached, told.parent),
            payload: event.payload,
            window: told.window,
            checked: Some(told.checked),
        }
    }
}

/// The properties `kind` caches ([`verbatim_model::event_kind::EventKindEntry::cached`]),
/// read from the event's cache, `fields`, of `process`, with `states` for a
/// kind that caches them: no call. Public so that mockapp's tests make a
/// tell's cached properties as the listener makes them.
#[must_use]
pub fn cached_of(
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

/// Every application's kept elements and Core's last held view
/// ([`ListenerKept`]), under their own lock, taken after the reading
/// threads' and the check threads' locks and before the writer's, whose
/// record of the tells it numbered judges a view, with no call made under
/// it; and the listener's generation, which every view and request names.
/// An element let go is handed back, for release on a thread that may
/// block.
pub(crate) struct Kept {
    /// The listener's generation.
    generation: u64,
    state: Mutex<ListenerKept<Agile<ListenerEvent>>>,
}

/// Elements let go, by application, to release.
pub(crate) type Released = Vec<(Pid, Vec<Agile<ListenerEvent>>)>;

impl Kept {
    /// The kept elements of the listener of `generation`.
    pub(crate) fn new(generation: u64) -> Self {
        Self {
            generation,
            state: Mutex::new(ListenerKept::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ListenerKept<Agile<ListenerEvent>>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `generation` is this listener's: a view or request naming
    /// another is of a listener that ended or the next one's, and its
    /// tokens name nothing here.
    pub(crate) fn is_own(&self, generation: u64) -> bool {
        generation == self.generation
    }

    /// The application whose elements hold `token`.
    pub(crate) fn owner(&self, token: ElementToken) -> Option<Pid> {
        self.lock().owner(token)
    }

    /// `pid`'s reading thread starts the read of `token` for request
    /// `request`, a focus read when `focus` (column A).
    pub(crate) fn start_read(
        &self,
        pid: Pid,
        token: ElementToken,
        read: (u64, bool),
    ) -> Start<Agile<ListenerEvent>> {
        self.lock().start_read((pid, token), read)
    }

    /// The read of `pid`'s `token` ended with `element` (column B), its
    /// answer sent when `sent`: the element to release, if any
    /// ([`ListenerKept::read_ended`]).
    pub(crate) fn read_ended(
        &self,
        outgoing: &Outgoing,
        at: (Pid, ElementToken),
        element: Agile<ListenerEvent>,
        sent: bool,
    ) -> Option<Agile<ListenerEvent>> {
        let mut kept = self.lock();
        outgoing.with_reported(|reported| kept.read_ended(at, element, sent, reported))
    }

    /// Core cancelled request `request` for the foreground (column E): the
    /// kept reference of the element in its read, with its application, to
    /// release; its reading thread releases its own as its call returns.
    pub(crate) fn read_abandoned(
        &self,
        outgoing: &Outgoing,
        request: u64,
    ) -> Option<(Pid, Agile<ListenerEvent>)> {
        let mut kept = self.lock();
        outgoing.with_reported(|reported| kept.read_abandoned(request, reported))
    }

    /// Core's held view of this generation arrived (columns C and D): kept
    /// as the last view, and every element it rules out handed back by
    /// application to release ([`ListenerKept::view`]).
    pub(crate) fn view(&self, outgoing: &Outgoing, view: View) -> Released {
        let mut kept = self.lock();
        outgoing.with_reported(|reported| kept.view(view, reported))
    }

    /// Keeps `element` of `pid`'s behind `token`, its event of `kind`.
    fn keep(
        &self,
        at: (Pid, ElementToken),
        told: (Agile<ListenerEvent>, RuntimeId),
        kind: EventKind,
    ) {
        self.lock().keep(at, told, kind);
    }

    /// The kept elements of `pid`'s a check of an event with `runtime_id`
    /// reads the keyboard focus of, with their tokens (column G).
    fn with_runtime_id(
        &self,
        pid: Pid,
        runtime_id: &RuntimeId,
    ) -> Vec<(ElementToken, Agile<ListenerEvent>)> {
        self.lock().with_runtime_id(pid, runtime_id)
    }

    /// `pid`'s application ended (column F): every element it keeps, the
    /// kept reference of one in read included.
    fn ended(&self, outgoing: &Outgoing, pid: Pid) -> Vec<Agile<ListenerEvent>> {
        let mut kept = self.lock();
        outgoing.with_reported(|reported| kept.ended(pid, reported))
    }

    /// Every element kept, for the listener's shutdown.
    fn clear(&self) -> Vec<Agile<ListenerEvent>> {
        self.lock().clear()
    }
}

impl Known for Checks {
    fn told(&self, process: Pid, answered: Option<u32>) -> String {
        told(&mut self.lock().applications, process, answered)
    }

    fn hung(&self, hung: Hung) -> Option<Hung> {
        // A window no process is read as serving marks no application, and
        // Core watches no such window.
        let process = hung.window.process?;
        self.lock()
            .applications
            .step(process, WindowState::Hung(hung))
    }

    fn foreground(&self) {
        self.lock().applications.foreground();
    }
}

/// The records under the lock: the threads' decisions, and the wake of
/// each application's thread.
struct State {
    threads: CheckThreads<CheckEvent, IdleWaiter>,
    wakes: HashMap<Pid, Arc<Condvar>>,
    /// Each application's executable name, for its tells, the `WinEvent`
    /// thread's included.
    applications: Applications,
}

/// Every application's check thread, the elements kept for their tells,
/// and what the threads place tells and release elements through.
pub(crate) struct Checks {
    state: Mutex<State>,
    kept: Arc<Kept>,
    /// The last token minted.
    tokens: AtomicU64,
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
                applications: Applications::default(),
            }),
            kept: Arc::clone(kept),
            tokens: AtomicU64::new(0),
            outgoing: Arc::clone(outgoing),
            readers: Arc::clone(readers),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hands `event` of `pid`'s, a UI Automation event not dropped, to its
    /// application's check thread, starting one when it has none, once the
    /// thread of its sender's window, `answered` (0 for none), is unmarked
    /// in `pid`'s record, under one lock. Never waits on a call.
    pub(super) fn hand(self: &Arc<Self>, pid: Pid, answered: u32, event: CheckEvent) {
        let mut state = self.lock();
        let _ = state
            .applications
            .step(pid, WindowState::Responsive { thread: answered });
        let pushed = state.threads.push(pid, event);
        self.pushed(state, pid, pushed);
    }

    fn tells(&self) -> &Tells {
        &self.outgoing.tells
    }

    /// Carries out what a push of `pid`'s did, under the lock `state`,
    /// which it lets go before releasing anything.
    fn pushed(
        self: &Arc<Self>,
        mut state: MutexGuard<'_, State>,
        pid: Pid,
        pushed: Pushed<CheckEvent>,
    ) {
        match pushed {
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
        let kept = self.kept.ended(&self.outgoing, pid);
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

    /// Releases `elements` of `pid`'s as a job on its reading thread, a
    /// thread that may block, which has a UI Automation client.
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
    /// released here, at its check's end. A progress bar's change keeps no
    /// element: its tell carries all Core reads of it, and names the
    /// element by its runtime id alone.
    fn checked(&self, (pid, generation): (Pid, u64), event: CheckEvent, told: Told) {
        let mut state = self.lock();
        let (own, waiters) = state.threads.checked(pid, generation);
        let mut unplaced = None;
        if own {
            let application = state.applications.name(event.process);
            let kind = event.kind;
            let tell = event.into_tell(
                told,
                |element, runtime_id| {
                    if kind == EventKind::UiaProgress {
                        unplaced = Some(element);
                        EventObject::Followed { runtime_id }
                    } else {
                        let token = ElementToken(
                            self.tokens.fetch_add(1, Ordering::Relaxed).wrapping_add(1),
                        );
                        self.kept
                            .keep((pid, token), (element, runtime_id.clone()), kind);
                        EventObject::Uia { token, runtime_id }
                    }
                },
                application,
            );
            self.tells().pass_hold(Held::Tell(Box::new(tell)));
        } else {
            unplaced = Some(event.element);
        }
        for waiter in waiters {
            waiter();
        }
        drop(state);
        drop(unplaced);
    }
}

/// What a check found, for its tell ([`CheckEvent::into_tell`]).
pub struct Told {
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
        let sender = event.element.resolve().ok();
        // The kept elements' references, taken under their lock and
        // resolved once it is let go; one that cannot be resolved answers
        // its read as failed.
        let kept: Vec<(ElementToken, Option<Cached<ListenerEvent>>)> = checks
            .kept_with(pid, &event.runtime_id)
            .into_iter()
            .map(|(token, element)| (token, element.resolve().ok()))
            .collect();
        let told = check_event(
            &event,
            (sender.as_ref(), &kept),
            (walker.as_ref(), parent_request.as_ref()),
            (&mut record, &now_us),
            &SystemReads,
            &mut |_| {},
        );
        drop(kept);
        drop(sender);
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

/// The kept elements with an event's runtime id, each resolved, `None`
/// for one that could not be.
pub type KeptElements = [(ElementToken, Option<Cached<ListenerEvent>>)];

/// The calls of one event's check, made on its element, `sender` (`None`
/// when it could not be resolved, which fails every read of it), and on
/// the kept elements with its runtime id, `kept`, each with UI
/// Automation's transaction timeout, the window facts read through
/// `reads` and the clock read by `now`.
struct SenderCalls<'a, R> {
    sender: Option<&'a Cached<ListenerEvent>>,
    kept: &'a KeptElements,
    walker: Option<&'a IUIAutomationTreeWalker>,
    parent_request: Option<&'a Request<ListenerEvent>>,
    reads: &'a R,
    now: &'a dyn Fn() -> u64,
    observe: &'a mut dyn FnMut(Call),
    /// The acceptance facts, read where NVDA judges acceptance.
    accepted: Option<AcceptanceFacts>,
    /// The event's window's serving process as the event's sender names
    /// it, its provider: for a console window its console host, so the
    /// facts name the process the tell does.
    served: Served,
}

impl<R: WindowReads> CheckCalls for SenderCalls<'_, R> {
    fn calling(&mut self, call: Call) {
        (self.observe)(call);
    }

    fn nearest_window(&mut self) -> Read<Option<isize>> {
        self.sender.map_or(Read::Failed, |sender| {
            verbatim_uia::nearest_window_lookup(sender.live().element())
                .map_or(Read::Failed, Read::Answered)
        })
    }

    fn classes(&mut self, window: isize) -> WindowClasses {
        verbatim_win32::window::window_classes(window)
    }

    fn probe(&mut self, window: isize) -> Probe {
        let began_us = (self.now)();
        let thread = verbatim_win32::window::window_thread(window);
        Probe {
            window: WindowHandle::from(window),
            thread,
            provider: verbatim_uia::raw_probe(window).has_provider(),
            began_us,
        }
    }

    fn acceptance(&mut self, window: isize) {
        self.accepted = Some(
            WindowPart::read(self.reads, window, Some(self.served)).with_foreground(self.reads),
        );
    }

    fn kept_focus(&mut self, token: ElementToken) -> Read<bool> {
        self.kept
            .iter()
            .find(|(kept, _)| *kept == token)
            .and_then(|(_, element)| element.as_ref())
            .map_or(Read::Failed, keyboard_focus)
    }

    fn previous_sibling(&mut self) -> Read<bool> {
        let (Some(sender), Some(walker)) = (self.sender, self.walker) else {
            return Read::Failed;
        };
        match sender.live().step(walker, Direction::PreviousSibling) {
            Ok(_) => Read::Answered(true),
            // The null element `windows` reports for none.
            Err(error) if error.code().is_ok() => Read::Answered(false),
            Err(_) => Read::Failed,
        }
    }

    fn parent_automation_id(&mut self) -> Read<Option<String>> {
        let (Some(sender), Some(walker), Some(request)) =
            (self.sender, self.walker, self.parent_request)
        else {
            return Read::Failed;
        };
        match sender.live().walk(walker, Direction::Parent, request) {
            Ok(parent) => Read::Answered(parent.read().automation_id),
            // The null element `windows` reports for none.
            Err(error) if error.code().is_ok() => Read::Answered(None),
            Err(_) => Read::Failed,
        }
    }

    fn keyboard_focus(&mut self) -> Read<bool> {
        self.sender.map_or(Read::Failed, keyboard_focus)
    }
}

/// Makes every call NVDA's handler for `event`'s kind makes at delivery
/// ([`verbatim_listener_logic::check::check`]) on its element, `sender`,
/// and on the kept elements with its runtime id, `kept`, with no lock
/// held, and gathers what its tell carries. `record` is the thread's probe
/// record and `now` its clock; window facts are read through `reads`, and
/// `observe` sees each call before it is made. Public so that mockapp's
/// tests check mockapp's own events with the listener's code.
pub fn check_event(
    event: &CheckEvent,
    (sender, kept): (Option<&Cached<ListenerEvent>>, &KeptElements),
    (walker, parent_request): (
        Option<&IUIAutomationTreeWalker>,
        Option<&Request<ListenerEvent>>,
    ),
    (record, now): (&mut ProbeRecord, &dyn Fn() -> u64),
    reads: &impl WindowReads,
    observe: &mut dyn FnMut(Call),
) -> Told {
    let facts = CheckFacts::new(
        event.kind,
        event.placeholder_candidate,
        kept.iter().map(|(token, _)| *token).collect(),
    )
    .reading_parent(event.switcher_item);
    let mut calls = SenderCalls {
        sender,
        kept,
        walker,
        parent_request,
        reads,
        now,
        observe,
        accepted: None,
        served: Served::Raised(Raised::Provider(event.process)),
    };
    let found = verbatim_listener_logic::check::check(&facts, (record, now()), &mut calls);
    let window = match found.walked {
        Walked::Found(window) => Some(window),
        Walked::NotWalked => (event.own_window != 0).then_some(event.own_window),
        Walked::NoWindow | Walked::Failed => None,
    };
    Told {
        window: match (window, found.walked, calls.accepted) {
            (Some(_), _, Some(facts)) => PendingWindow::Whole(Box::new(facts)),
            (Some(window), _, None) => {
                PendingWindow::Found(WindowPart::read(reads, window, Some(calls.served)))
            }
            (None, Walked::Failed, _) => PendingWindow::Failed,
            (None, _, _) => PendingWindow::NoWindow,
        },
        thread: window.map_or(0, verbatim_win32::window::window_thread),
        checked: found.checked,
        parent: found.parent,
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
