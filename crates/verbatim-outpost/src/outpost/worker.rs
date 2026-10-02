//! The worker and its watchdog (outpost redesign, "Inside an outpost").
//!
//! One worker thread takes entries from the intake queue in order and
//! finishes each before starting the next. It is the only thread that calls
//! into the application, so events and replies leave the outpost in the order
//! their entries joined the queue. This is NVDA's model, one thread doing all
//! the work, with one such thread per application.
//!
//! The watchdog watches the worker's deadline. If a call hangs past it, the
//! watchdog abandons the worker (a hung cross-process call cannot be stopped
//! safely), answers the stuck query "abandoned" if it was a query, counts the
//! abandoned worker, and starts a replacement that continues with the rest of
//! the queue. An abandoned worker that eventually returns publishes nothing,
//! lowers the count, and exits.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use verbatim_ia2::{CHILDID_SELF, WinEventKind};
use verbatim_model::{
    Backend, NodeId, NodeSnapshot, NormalizedEvent, PropertyChange, Role, TraceId,
};
use verbatim_uia::map::snapshot_from_cached_element;
use verbatim_uia::{map::snapshot_parts_from_cached_element, nearest_window_handle};
use windows::Win32::UI::Accessibility::{UIA_NamePropertyId, UIA_ValueValuePropertyId};
use windows::Win32::UI::WindowsAndMessaging::OBJID_WINDOW;

use crate::protocol::{
    DeliveredFact, OutpostToSupervisor, Query, QueryOutcome, QueryResult, UiaSnapshotFact,
};

use super::Context;
use super::intake::{Entry, Item, Object, Planned, UiaEvent, UiaKind, window_of};
use super::read::{self, Client, ReadError};
use super::window::{
    focus_window, front_is_another_thread_of_its_application, window_belongs_to_hidden_frame,
    window_facts, window_is_foreground,
};

/// The deadline for handling an event, a focus, or a focus-now query: NVDA's
/// `NORMAL_CORE_ALIVE_TIMEOUT` (`watchdog.py`), the time NVDA waits for an
/// application that is slow to answer before cancelling the call. An
/// application that is starting up, or the Start menu's search window as it
/// opens, can take two or three seconds to answer a read that then succeeds
/// (found live in the end-to-end suite); NVDA announces the focus late, and
/// a shorter deadline here dropped it for good.
const HANDLING_DEADLINE: Duration = Duration::from_secs(10);

/// The longest the worker waits, before a batch holding a foreground change,
/// for that change's window to become the foreground window. Measured live
/// on 2026-10-02 over 245 such events (msinfo32, Notepad, and Verbatim's own
/// windows): 5 to 100 ms, median 44 ms. A window that has not arrived by
/// then was refused the foreground, and the batch is handled anyway.
const FOREGROUND_WAIT: Duration = Duration::from_millis(250);

/// How often the worker checks the foreground window while it waits.
const FOREGROUND_POLL: Duration = Duration::from_millis(10);

/// How long the watchdog waits on an entry before abandoning it because the
/// user has moved on to a window of the same application on another UI
/// thread: NVDA's `MIN_CORE_ALIVE_TIMEOUT`, after which NVDA cancels a slow
/// call once the foreground has changed. One application can own windows on
/// several threads (Explorer's folder windows, taskbar, and Alt+Tab
/// switcher), and those must not wait behind a slow window the user has
/// left. Windows of other applications have their own outposts and never
/// wait behind this one. The check is repeated at this interval until the
/// deadline.
const MOVED_ON_GRACE: Duration = Duration::from_millis(500);

/// The deadline for one navigation step or an activation.
const STEP_DEADLINE: Duration = Duration::from_millis(400);

/// The deadline for an ancestor walk or a tree dump, each up to 64 hops or
/// 4096 nodes.
const WALK_DEADLINE: Duration = Duration::from_secs(5);

/// The worker incarnation in charge, its deadline, and the abandoned count.
#[derive(Default)]
struct WatchState {
    generation: u64,
    deadline: Option<Instant>,
    /// The query the worker is running, so an abandonment can answer it.
    running: Option<(u64, TraceId)>,
    /// When the worker started its entry, and the window the entry concerns
    /// (0 for none), for abandoning it once the user has moved on.
    started: Option<(Instant, isize)>,
    /// How many messages that carry node ids have been published: the
    /// position of the last one.
    position: u64,
    /// The position of the last message that may have reported each node,
    /// by node number.
    reported: HashMap<u64, u64>,
}

impl WatchState {
    /// Abandons the worker in charge: the next generation takes over, and
    /// the query it was running, if any, is returned for an "abandoned"
    /// reply.
    fn abandon(&mut self) -> Option<(u64, TraceId)> {
        self.generation += 1;
        self.deadline = None;
        self.started = None;
        self.running.take()
    }

    /// Records a published message: one that carries node ids takes the
    /// next position, and each of `reported` is recorded at the current one.
    fn record(&mut self, carries_nodes: bool, reported: impl IntoIterator<Item = u64>) {
        if carries_nodes {
            self.position += 1;
        }
        for number in reported {
            self.reported.insert(number, self.position);
        }
    }

    /// Takes the nodes to release for a held list acknowledging position
    /// `acknowledged`: those not held that were reported at or before it.
    /// A node reported later, or never reported, is kept, since Core may not
    /// have seen it yet.
    fn take_releasable(&mut self, held: &HashSet<u64>, acknowledged: u64) -> HashSet<u64> {
        let released: HashSet<u64> = self
            .reported
            .iter()
            .filter(|&(number, &position)| position <= acknowledged && !held.contains(number))
            .map(|(&number, _)| number)
            .collect();
        self.reported.retain(|number, _| !released.contains(number));
        released
    }
}

/// Shared between the worker, its watchdog, and anything that publishes.
#[derive(Default)]
pub(super) struct Watch {
    state: Mutex<WatchState>,
    changed: Condvar,
    /// Abandoned workers that have not yet returned. An atomic, so the reader
    /// answers a ping without waiting on the watch lock.
    abandoned: AtomicUsize,
}

impl Watch {
    fn lock(&self) -> std::sync::MutexGuard<'_, WatchState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many abandoned workers have not yet returned.
    pub(super) fn abandoned(&self) -> usize {
        self.abandoned.load(Ordering::Relaxed)
    }

    /// Starts the worker's deadline for one entry concerning `window` (0 for
    /// none).
    fn start(&self, deadline: Duration, running: Option<(u64, TraceId)>, window: isize) {
        let mut state = self.lock();
        let now = Instant::now();
        state.deadline = Some(now + deadline);
        state.started = Some((now, window));
        state.running = running;
        drop(state);
        self.changed.notify_one();
    }

    /// Ends the deadline, if `generation` is still in charge, returning the
    /// query still waiting for its reply, if any. `Err` for an abandoned
    /// worker, which is then no longer counted and must exit.
    fn finish(&self, generation: u64) -> Result<Option<(u64, TraceId)>, ()> {
        let mut state = self.lock();
        if state.generation == generation {
            state.deadline = None;
            state.started = None;
            Ok(state.running.take())
        } else {
            drop(state);
            let _ = self
                .abandoned
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    Some(count.saturating_sub(1))
                });
            Err(())
        }
    }
}

/// What the worker remembers between entries, across worker replacements.
#[derive(Default)]
pub(super) struct Tracking {
    /// The role of the focus this outpost last reported.
    focus_role: Option<Role>,
    /// The batch in which a focus was last reported.
    focus_batch: Option<u64>,
}

/// Publishes `message`, an entry's one result, if `generation` is still the
/// worker in charge. The check, the end of the entry's deadline, and the
/// send happen under the watch lock, so an abandoned worker can never
/// publish after its abandonment, and a published entry can no longer be
/// abandoned: a query never gets a second reply.
///
/// Under the same lock, a message that carries node ids takes the next
/// position, and every node issued or looked up since the last publish is
/// recorded as reported at the current position. That may include nodes the
/// message does not carry, which are then only kept a little longer.
fn publish(context: &Context, generation: u64, message: OutpostToSupervisor) -> bool {
    let mut state = context.watch.lock();
    if state.generation != generation {
        return false;
    }
    state.deadline = None;
    state.started = None;
    state.running = None;
    let touched = context
        .uia_registry
        .take_touched()
        .into_iter()
        .chain(context.msaa_registry.take_touched());
    state.record(message.carries_nodes(), touched.map(NodeId::number));
    context.outbound.send(message);
    true
}

/// Starts the first worker and the watchdog.
pub(super) fn start(context: &Arc<Context>) {
    spawn_worker(Arc::clone(context), 0);
    let context = Arc::clone(context);
    thread::Builder::new()
        .name("verbatim-watchdog".to_owned())
        .spawn(move || watchdog(&context))
        .expect("spawn the watchdog");
}

fn spawn_worker(context: Arc<Context>, generation: u64) {
    thread::Builder::new()
        .name("verbatim-worker".to_owned())
        .spawn(move || run(&context, generation))
        .expect("spawn a worker");
}

/// The watchdog: waits for the worker's deadline and abandons a worker that
/// passes it, or that has waited [`MOVED_ON_GRACE`] on a window the user has
/// left for one of the same application on another UI thread.
fn watchdog(context: &Arc<Context>) {
    let mut state = context.watch.lock();
    loop {
        let Some(deadline) = state.deadline else {
            state = context
                .watch
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
            continue;
        };
        let now = Instant::now();
        let (started, window) = state.started.unwrap_or((now, 0));
        let reason = if now >= deadline {
            Some("a call passed its deadline")
        } else if window != 0
            && now >= started + MOVED_ON_GRACE
            && front_is_another_thread_of_its_application(window)
        {
            Some("the user moved on from a slow window")
        } else {
            None
        };
        let Some(reason) = reason else {
            let check = if window == 0 {
                deadline
            } else if now < started + MOVED_ON_GRACE {
                started + MOVED_ON_GRACE
            } else {
                now + MOVED_ON_GRACE
            };
            let wait = check.min(deadline).saturating_duration_since(now);
            state = context
                .watch
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
            continue;
        };
        let running = state.abandon();
        let abandoned = context.watch.abandoned.fetch_add(1, Ordering::Relaxed) + 1;
        let generation = state.generation;
        tracing::warn!(abandoned, reason, "the worker is abandoned and replaced");
        if let Some((request_id, trace_id)) = running {
            context.outbound.send(OutpostToSupervisor::Reply {
                trace_id,
                request_id,
                outcome: QueryOutcome::Abandoned,
            });
        }
        spawn_worker(Arc::clone(context), generation);
    }
}

/// The worker's loop.
fn run(context: &Context, generation: u64) {
    // Held objects are agile references, resolved in this apartment.
    if let Err(error) = verbatim_uia::init_mta() {
        tracing::warn!(%error, "the worker could not join the multithreaded apartment");
    }
    let mut client = Client::default();
    while let Some((planned, batch, foreground)) = context.intake.next() {
        if let Some(hwnd) = foreground {
            wait_for_foreground(hwnd);
        }
        let (deadline, running, window) = match &planned {
            Planned::Run(entry) | Planned::Menu(entry) => {
                let (deadline, running) = budget(entry);
                (deadline, running, window_of(&entry.item))
            }
        };
        context.watch.start(deadline, running, window);
        let started = Instant::now();
        let handled = catch_unwind(AssertUnwindSafe(|| {
            let mut worker = Worker {
                context,
                client: &mut client,
                generation,
                batch,
            };
            match planned {
                Planned::Run(entry) => worker.handle(entry),
                Planned::Menu(entry) => worker.menu_opened(&entry),
            }
        }));
        context
            .arbitrator()
            .renew_probes_since(started, Instant::now());
        match context.watch.finish(generation) {
            Err(()) => {
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis(),
                    deadline_ms = deadline.as_millis(),
                    "an abandoned worker returned; its result is discarded"
                );
                return;
            }
            // A query whose handling panicked before replying still gets its
            // one reply.
            Ok(Some((request_id, trace_id))) => {
                if handled.is_err() {
                    tracing::error!(request_id, "the worker failed handling a query");
                }
                context.outbound.send(OutpostToSupervisor::Reply {
                    trace_id,
                    request_id,
                    outcome: QueryOutcome::Failed(
                        "the outpost failed handling the query".to_owned(),
                    ),
                });
            }
            Ok(None) => {
                if handled.is_err() {
                    tracing::error!("the worker failed handling an event");
                }
            }
        }
    }
}

/// Waits, up to [`FOREGROUND_WAIT`], for `hwnd` to become the foreground
/// window, as NVDA holds back event handling after a foreground event until
/// the foreground window matches (`_shouldGetEvents`, issue 3831). Local
/// calls only; the application is never asked.
fn wait_for_foreground(hwnd: isize) {
    let deadline = Instant::now() + FOREGROUND_WAIT;
    while !window_is_foreground(hwnd) {
        if Instant::now() >= deadline {
            tracing::debug!(
                hwnd,
                "the foreground window did not become the event's window"
            );
            return;
        }
        thread::sleep(FOREGROUND_POLL);
    }
}

/// The deadline for an entry, and the query it answers, if it is one.
fn budget(entry: &Entry) -> (Duration, Option<(u64, TraceId)>) {
    match &entry.item {
        Item::Query { request_id, query } => {
            let deadline = match query {
                Query::FocusNow => HANDLING_DEADLINE,
                Query::Ancestors { .. } | Query::DumpTree => WALK_DEADLINE,
                _ => STEP_DEADLINE,
            };
            (deadline, Some((*request_id, entry.trace)))
        }
        Item::Fact(_) | Item::Msaa { .. } | Item::Uia(_) => (HANDLING_DEADLINE, None),
        // Releasing thousands of objects after a tree dump takes a while.
        Item::NodesHeld { .. } => (WALK_DEADLINE, None),
    }
}

/// One entry's handling, by the worker in charge.
struct Worker<'a> {
    context: &'a Context,
    client: &'a mut Client,
    generation: u64,
    /// The batch the entry belongs to.
    batch: u64,
}

impl Worker<'_> {
    fn handle(&mut self, entry: Entry) {
        let Entry {
            item,
            trace,
            observed_at_ms,
        } = entry;
        match item {
            Item::Msaa {
                kind,
                hwnd,
                id_object,
                id_child,
            } => self.msaa_event(kind, hwnd, id_object, id_child, trace, observed_at_ms),
            Item::Uia(event) => self.uia_event(event, trace, observed_at_ms),
            Item::Fact(fact) => self.fact(fact, trace, observed_at_ms),
            Item::Query { request_id, query } => self.query(request_id, &query, trace),
            Item::NodesHeld {
                nodes,
                acknowledged,
            } => self.release(&nodes, acknowledged),
        }
    }

    /// Releases every node Core does not hold that was reported at or before
    /// the position Core acknowledged. A node reported later, or never
    /// reported, is kept, since Core may not have seen it yet.
    fn release(&self, held: &[u64], acknowledged: u64) {
        let held: HashSet<u64> = held.iter().copied().collect();
        // The nodes leave both registries under the watch lock, so a worker
        // that replaces this one, should it be abandoned while the objects
        // are dropped, can never report a node that is about to vanish.
        let objects = {
            let mut state = self.context.watch.lock();
            let released = state.take_releasable(&held, acknowledged);
            if released.is_empty() {
                return;
            }
            let keep = |id: NodeId| !released.contains(&id.number());
            (
                self.context.uia_registry.retain(keep),
                self.context.msaa_registry.retain(keep),
            )
        };
        // Outside the lock: releasing an object can call into the
        // application.
        drop(objects);
    }

    fn emit(
        &self,
        trace: TraceId,
        observed_at_ms: u64,
        backend: Backend,
        window: Option<isize>,
        event: NormalizedEvent,
    ) -> bool {
        publish(
            self.context,
            self.generation,
            OutpostToSupervisor::Event {
                trace_id: trace,
                observed_at_ms,
                backend,
                window: window.map(window_facts),
                event,
            },
        )
    }

    /// Reports a focus and remembers it, so the limiter keeps its events and
    /// the menu rules know where focus is.
    #[expect(
        clippy::too_many_arguments,
        reason = "a focus report has this many parts"
    )]
    fn emit_focus(
        &self,
        trace: TraceId,
        observed_at_ms: u64,
        backend: Backend,
        window: Option<isize>,
        node: NodeSnapshot,
        foreground: bool,
        object: Option<Object>,
        (ancestors, selected_child): (Vec<NodeSnapshot>, Option<NodeSnapshot>),
    ) {
        let role = node.role;
        tracing::debug!(?role, name = ?node.name, foreground, ?backend, "focus reported");
        let event = NormalizedEvent::FocusChanged {
            node,
            foreground,
            ancestors,
            selected_child,
        };
        let followed = self.uia_elements(&event);
        if self.emit(trace, observed_at_ms, backend, window, event) {
            if !foreground {
                self.context.intake.set_focused(object);
                // Move the focus-following UIA property subscription to the
                // new focus and its ancestors, without waiting. A foreground
                // report is a window, not the control focus is in.
                if let Some(subscription) = self.context.focus_properties.get() {
                    subscription.retarget(verbatim_uia::Scope::Elements(followed));
                }
            }
            let mut tracking = self.context.tracking();
            tracking.focus_role = Some(role);
            tracking.focus_batch = Some(self.batch);
        }
    }

    /// The live UIA elements behind a focus event's node and ancestors, for
    /// the focus-following property subscription. MSAA nodes have none.
    fn uia_elements(
        &self,
        event: &NormalizedEvent,
    ) -> Vec<windows::core::AgileReference<windows::Win32::UI::Accessibility::IUIAutomationElement>>
    {
        let NormalizedEvent::FocusChanged {
            node, ancestors, ..
        } = event
        else {
            return Vec::new();
        };
        std::iter::once(node)
            .chain(ancestors)
            .filter_map(|node| self.context.uia_registry.element_of(node.id))
            .collect()
    }

    /// An MSAA event from this outpost's own hooks.
    fn msaa_event(
        &mut self,
        kind: WinEventKind,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        trace: TraceId,
        observed_at_ms: u64,
    ) {
        if kind == WinEventKind::Destroy {
            if id_object == OBJID_WINDOW.0 && id_child == CHILDID_SELF {
                self.context.arbitrator().forget(hwnd);
                // A reused window handle must never inherit these nodes.
                self.context.msaa_registry.forget_window(hwnd);
            }
            return;
        }
        if read::window_uses_uia(self.context, hwnd) {
            return; // UIA owns this window.
        }
        let Some(node) = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            &self.context.msaa_registry,
        ) else {
            return;
        };
        let event = match kind {
            WinEventKind::ValueChange => NormalizedEvent::ValueChanged {
                node_id: node.id,
                value: node.value,
            },
            WinEventKind::NameChange => NormalizedEvent::PropertyChanged {
                node_id: node.id,
                change: PropertyChange::Name(node.name),
            },
            WinEventKind::StateChange => NormalizedEvent::PropertyChanged {
                node_id: node.id,
                change: PropertyChange::States(node.states),
            },
            WinEventKind::Selection => NormalizedEvent::SelectionChanged { node },
            _ => return,
        };
        self.emit(trace, observed_at_ms, Backend::Msaa, Some(hwnd), event);
    }

    /// A UIA event from this outpost's own subscriptions.
    fn uia_event(&mut self, event: UiaEvent, trace: TraceId, observed_at_ms: u64) {
        let element = event
            .element
            .as_ref()
            .and_then(|agile| agile.resolve().ok());
        let hwnd = if event.hwnd != 0 {
            Some(event.hwnd)
        } else {
            element
                .as_ref()
                .and_then(nearest_window_handle)
                .or_else(focus_window)
        };
        if let Some(hwnd) = hwnd
            && !read::window_uses_uia(self.context, hwnd)
        {
            return; // MSAA owns this window.
        }
        let node = self.uia_node(&event.parts, element.as_ref());
        let normalized = match event.kind {
            UiaKind::Property(id) if id == UIA_NamePropertyId.0 => {
                NormalizedEvent::PropertyChanged {
                    node_id: node.id,
                    change: PropertyChange::Name(node.name),
                }
            }
            UiaKind::Property(id) if id == UIA_ValueValuePropertyId.0 => {
                NormalizedEvent::ValueChanged {
                    node_id: node.id,
                    value: node.value,
                }
            }
            UiaKind::Property(_) => NormalizedEvent::PropertyChanged {
                node_id: node.id,
                change: PropertyChange::States(node.states),
            },
            UiaKind::Selection => NormalizedEvent::SelectionChanged { node },
            UiaKind::Notification(notification) => NormalizedEvent::Notification {
                node_id: node.id,
                notification,
            },
        };
        self.emit(trace, observed_at_ms, Backend::Uia, hwnd, normalized);
    }

    /// A snapshot from a UIA element's cached parts, its id minted by the
    /// registry, which keeps the element when there is one.
    fn uia_node(
        &self,
        parts: &UiaSnapshotFact,
        element: Option<&windows::Win32::UI::Accessibility::IUIAutomationElement>,
    ) -> NodeSnapshot {
        let registry = &self.context.uia_registry;
        let id = match element {
            Some(element) => registry.id_for_element(&parts.runtime_id, element),
            None => registry.id_for(&parts.runtime_id),
        };
        NodeSnapshot {
            id,
            backend: Backend::Uia,
            role: parts.role,
            name: parts.name.clone(),
            value: parts.value.clone(),
            states: parts.states,
            details: parts.details.clone(),
        }
    }

    /// A focus fact routed from the listener.
    fn fact(&mut self, fact: DeliveredFact, trace: TraceId, observed_at_ms: u64) {
        match fact {
            DeliveredFact::Foreground { hwnd } => self.foreground(hwnd, trace, observed_at_ms),
            DeliveredFact::MsaaFocus {
                hwnd,
                id_object,
                id_child,
            } => self.msaa_focus(hwnd, id_object, id_child, trace, observed_at_ms),
            DeliveredFact::UiaFocus { hwnd, snapshot } => {
                self.uia_focus(hwnd, &snapshot, trace, observed_at_ms);
            }
            DeliveredFact::UiaSelection { hwnd, snapshot } => self.uia_event(
                UiaEvent {
                    kind: UiaKind::Selection,
                    parts: snapshot,
                    hwnd,
                    element: None,
                },
                trace,
                observed_at_ms,
            ),
            DeliveredFact::UiaNotification {
                hwnd,
                snapshot,
                notification,
            } => self.uia_event(
                UiaEvent {
                    kind: UiaKind::Notification(notification),
                    parts: snapshot,
                    hwnd,
                    element: None,
                },
                trace,
                observed_at_ms,
            ),
            DeliveredFact::Alert {
                hwnd,
                id_object,
                id_child,
            } => self.alert(hwnd, id_object, id_child, trace, observed_at_ms),
            // Menu openings are planned separately; one reaching here is
            // handled the same way.
            menu @ (DeliveredFact::MenuPopup { .. } | DeliveredFact::UiaMenuOpened { .. }) => {
                self.menu_opened(&Entry {
                    item: Item::Fact(menu),
                    trace,
                    observed_at_ms,
                });
            }
        }
    }

    /// An MSAA alert. Only a toast (its window's parent has the class
    /// `ToastChildWindowClass`) is reported, for the reducer to speak queued
    /// from any application, as NVDA's notification behavior speaks it.
    /// NVDA speaks other alerts only when the object's role is alert, it has
    /// content, and it is not already among the focus's ancestors; Verbatim
    /// has no alert role yet, so those are not reported.
    fn alert(
        &mut self,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        trace: TraceId,
        observed_at_ms: u64,
    ) {
        if super::window::parent_class(hwnd).as_deref() != Some("ToastChildWindowClass") {
            return;
        }
        let Some(node) = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            &self.context.msaa_registry,
        ) else {
            return;
        };
        self.emit(
            trace,
            observed_at_ms,
            Backend::Msaa,
            Some(hwnd),
            NormalizedEvent::Alert { node },
        );
    }

    /// A foreground change: reported at once, named or not, as a focus on
    /// the window, unless the window is no longer the system's foreground
    /// window by the time it is read.
    fn foreground(&mut self, hwnd: isize, trace: TraceId, observed_at_ms: u64) {
        if window_belongs_to_hidden_frame(hwnd) {
            tracing::debug!(hwnd, "foreground dropped: Core's hidden frame");
            return;
        }
        let (backend, node) = read::foreground_window(self.context, self.client, hwnd);
        if !window_is_foreground(hwnd) {
            tracing::info!(
                hwnd,
                "foreground report dropped: no longer the foreground window"
            );
            return;
        }
        self.emit_focus(
            trace,
            observed_at_ms,
            backend,
            Some(hwnd),
            node,
            true,
            None,
            (Vec::new(), None),
        );
    }

    /// An MSAA focus fact.
    fn msaa_focus(
        &mut self,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        trace: TraceId,
        observed_at_ms: u64,
    ) {
        if id_child == CHILDID_SELF && window_belongs_to_hidden_frame(hwnd) {
            return;
        }
        if read::window_uses_uia(self.context, hwnd) {
            // UIA owns this window; its UIA fact reports the focus.
            tracing::debug!(hwnd, "MSAA focus dropped: UIA owns the window");
            return;
        }
        let Some(node) = verbatim_ia2::acquire::snapshot_from_focus_event(
            hwnd,
            id_object,
            id_child,
            &self.context.msaa_registry,
        ) else {
            tracing::debug!(hwnd, id_object, id_child, "MSAA focus dropped: unreadable");
            return;
        };
        let enrichment = read::msaa_enrichment(self.context, &node);
        let object = self
            .context
            .msaa_registry
            .key_of(node.id)
            .map(|(hwnd, object, child)| Object::Msaa(hwnd, object, child));
        self.emit_focus(
            trace,
            observed_at_ms,
            Backend::Msaa,
            Some(hwnd),
            node,
            false,
            object,
            enrichment,
        );
    }

    /// A UIA focus fact, resolved with one `focused_element` call compared
    /// against the fact's runtime id. A mismatch means focus has already
    /// moved and a newer fact will arrive, so this one is dropped. The
    /// element in hand serves the window, the ancestors, and the selected
    /// child.
    fn uia_focus(
        &mut self,
        fact_hwnd: isize,
        fact: &UiaSnapshotFact,
        trace: TraceId,
        observed_at_ms: u64,
    ) {
        let context = self.context;
        let Some(uia) = self.client.uia() else {
            return;
        };
        let Ok(cache) = uia.base_cache_request() else {
            return;
        };
        let Ok(element) = uia.focused_element(&cache) else {
            tracing::debug!("UIA focus dropped: no focused element");
            return;
        };
        // SAFETY: `element` was built with the base cache request.
        let parts = unsafe { snapshot_parts_from_cached_element(&element) };
        if parts.runtime_id != fact.runtime_id {
            // Not the focused element, which is no proof that focus moved on:
            // the Start menu's search results raise focus events while the
            // keyboard focus stays in the search box. NVDA trusts the event's
            // sender, so the fact's own snapshot is reported; there is no
            // live element, so no ancestors.
            tracing::debug!(
                fact = ?fact.runtime_id,
                focused = ?parts.runtime_id,
                "UIA focus reported from the fact: not the focused element"
            );
            self.uia_fact_focus(fact_hwnd, fact, trace, observed_at_ms);
            return;
        }
        let hwnd = if fact_hwnd != 0 {
            Some(fact_hwnd)
        } else {
            nearest_window_handle(&element).or_else(focus_window)
        };
        if hwnd.is_some_and(window_belongs_to_hidden_frame) {
            return;
        }
        if let Some(hwnd) = hwnd
            && !read::window_uses_uia(context, hwnd)
        {
            // MSAA owns this window; its MSAA fact reports the focus.
            tracing::debug!(hwnd, "UIA focus dropped: MSAA owns the window");
            return;
        }
        // SAFETY: `element` was built with the base cache request.
        let node = unsafe { snapshot_from_cached_element(&element, &context.uia_registry) };
        let enrichment = read::uia_enrichment(context, uia, &cache, &element, node.role);
        self.emit_focus(
            trace,
            observed_at_ms,
            Backend::Uia,
            hwnd,
            node,
            false,
            Some(Object::Uia(parts.runtime_id)),
            enrichment,
        );
    }

    /// A UIA focus fact whose element is not the focused element, reported from
    /// the fact's cached snapshot alone: when it names its window, only if that
    /// window is in the system's foreground window; when it does not, with no
    /// window facts, for the reducer to judge by its application, as NVDA
    /// accepts an event whose window it cannot tell. The window that has the
    /// focus now is never borrowed for it: a late fact from a closed menu would
    /// then pass as being in the foreground.
    fn uia_fact_focus(
        &mut self,
        fact_hwnd: isize,
        fact: &UiaSnapshotFact,
        trace: TraceId,
        observed_at_ms: u64,
    ) {
        let hwnd = (fact_hwnd != 0).then_some(fact_hwnd);
        if let Some(hwnd) = hwnd {
            if window_belongs_to_hidden_frame(hwnd) {
                return;
            }
            if !read::window_uses_uia(self.context, hwnd) {
                tracing::debug!(hwnd, "UIA focus dropped: MSAA owns the window");
                return;
            }
            // NVDA trusts the sender but still requires its window to be in
            // the foreground window when it handles the event.
            if !window_facts(hwnd).in_foreground {
                tracing::debug!(
                    hwnd,
                    "UIA focus dropped: not the focused element and not in the foreground window"
                );
                return;
            }
        }
        let node = self.uia_node(fact, None);
        self.emit_focus(
            trace,
            observed_at_ms,
            Backend::Uia,
            hwnd,
            node,
            false,
            Some(Object::Uia(fact.runtime_id.clone())),
            (Vec::new(), None),
        );
    }

    /// A menu opening, handled after the batch's focus events, as NVDA's
    /// MSAA handler does. Ignored when a focus reported in the same batch
    /// already put focus on a menu or menu item, when the window belongs to
    /// UIA (NVDA does not use MSAA objects proxied from native UIA), or when
    /// its object is not a popup menu; otherwise it becomes a focus on the
    /// popup menu.
    fn menu_opened(&mut self, entry: &Entry) {
        if let Item::Fact(DeliveredFact::UiaMenuOpened { hwnd, snapshot }) = &entry.item {
            self.uia_menu_opened(*hwnd, snapshot, entry);
            return;
        }
        let Item::Fact(DeliveredFact::MenuPopup {
            hwnd,
            id_object,
            id_child,
        }) = entry.item
        else {
            return;
        };
        {
            let tracking = self.context.tracking();
            if tracking.focus_batch == Some(self.batch)
                && matches!(tracking.focus_role, Some(Role::Menu | Role::MenuItem))
            {
                return;
            }
        }
        if read::window_uses_uia(self.context, hwnd) {
            return;
        }
        let Some(node) = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            &self.context.msaa_registry,
        ) else {
            return;
        };
        if node.role != Role::Menu {
            return;
        }
        self.emit_focus(
            entry.trace,
            entry.observed_at_ms,
            Backend::Msaa,
            Some(hwnd),
            node,
            false,
            Some(Object::Msaa(hwnd, id_object, id_child)),
            (Vec::new(), None),
        );
    }

    /// A UIA menu opening, which NVDA treats as a focus on the menu unless a
    /// focus is already pending: ignored when a focus was reported in the same
    /// batch, or when the window does not belong to UIA.
    fn uia_menu_opened(&mut self, fact_hwnd: isize, parts: &UiaSnapshotFact, entry: &Entry) {
        if self.context.tracking().focus_batch == Some(self.batch) {
            return;
        }
        let hwnd = if fact_hwnd != 0 {
            Some(fact_hwnd)
        } else {
            focus_window()
        };
        if hwnd.is_some_and(window_belongs_to_hidden_frame) {
            return;
        }
        if let Some(hwnd) = hwnd
            && !read::window_uses_uia(self.context, hwnd)
        {
            return;
        }
        let node = self.uia_node(parts, None);
        self.emit_focus(
            entry.trace,
            entry.observed_at_ms,
            Backend::Uia,
            hwnd,
            node,
            false,
            Some(Object::Uia(parts.runtime_id.clone())),
            (Vec::new(), None),
        );
    }

    /// A query from Core, answered with exactly one reply.
    fn query(&mut self, request_id: u64, query: &Query, trace: TraceId) {
        let context = self.context;
        let client = &mut *self.client;
        let result = match query {
            Query::FocusNow => Ok(QueryResult::Focus(read::focus_now(context, client))),
            Query::Navigate { node_id, kind } => {
                read::navigate(context, client, *node_id, *kind).map(QueryResult::Navigated)
            }
            Query::Activate { node_id } => {
                read::activate(context, client, *node_id).map(|()| QueryResult::Activated)
            }
            Query::Ancestors { node_id } => {
                read::ancestors(context, client, *node_id).map(QueryResult::Ancestors)
            }
            Query::DumpTree => read::dump_tree(context, client).map(QueryResult::Tree),
        };
        let outcome = match result {
            Ok(result) => QueryOutcome::Done(result),
            Err(ReadError::Gone) => QueryOutcome::Gone,
            Err(ReadError::Failed(reason)) => QueryOutcome::Failed(reason),
        };
        publish(
            context,
            self.generation,
            OutpostToSupervisor::Reply {
                trace_id: trace,
                request_id,
                outcome,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_returning_abandoned_worker_never_publishes_and_lowers_the_count() {
        let watch = Watch::default();
        let trace = TraceId::mint();
        watch.start(STEP_DEADLINE, Some((7, trace)), 0);
        let running = watch.lock().abandon();
        watch.abandoned.fetch_add(1, Ordering::Relaxed);
        assert_eq!(running, Some((7, trace)), "the stuck query is answered");

        assert_eq!(watch.finish(0), Err(()), "the old worker must exit");
        assert_eq!(watch.abandoned(), 0, "and is no longer counted");
        assert_eq!(watch.finish(1), Ok(None), "its replacement is in charge");
    }

    #[test]
    fn a_held_list_releases_only_nodes_core_has_seen_and_does_not_hold() {
        let mut state = WatchState::default();
        state.record(true, [1, 2]);
        state.record(true, [3]);
        // A message without node ids takes no position.
        state.record(false, [4]);

        let held = HashSet::from([2]);
        assert_eq!(state.take_releasable(&held, 1), HashSet::from([1]));
        assert_eq!(
            state.take_releasable(&HashSet::new(), 2),
            HashSet::from([2, 3, 4])
        );
        state.record(true, [5]);
        assert!(
            state.take_releasable(&HashSet::new(), 2).is_empty(),
            "a node reported after the acknowledged position is kept"
        );
    }
}
