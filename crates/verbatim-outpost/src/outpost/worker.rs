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

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use verbatim_ia2::{CHILDID_SELF, WinEventKind};
use verbatim_model::{Backend, NodeSnapshot, NormalizedEvent, PropertyChange, Role, TraceId};
use verbatim_uia::map::snapshot_from_cached_element;
use verbatim_uia::{map::snapshot_parts_from_cached_element, nearest_window_handle};
use windows::Win32::UI::Accessibility::{UIA_NamePropertyId, UIA_ValueValuePropertyId};
use windows::Win32::UI::WindowsAndMessaging::OBJID_WINDOW;

use crate::protocol::{
    DeliveredFact, OutpostToSupervisor, Query, QueryOutcome, QueryResult, UiaSnapshotFact,
};

use super::Context;
use super::intake::{Entry, Item, Object, Planned, UiaEvent, UiaKind};
use super::read::{self, Client, ReadError};
use super::window::{
    focus_window, window_belongs_to_hidden_frame, window_facts, window_is_foreground,
    window_is_hidden_frame,
};

/// The deadline for one event: an acquisition and a mapping.
const EVENT_DEADLINE: Duration = Duration::from_millis(400);

/// The deadline for a focus, which also reads the ancestors and the selected
/// child.
const FOCUS_DEADLINE: Duration = Duration::from_millis(1500);

/// The deadline for one navigation step or an activation.
const STEP_DEADLINE: Duration = Duration::from_millis(400);

/// The deadline for a focus-now query: the window, the focus, and its
/// ancestors.
const FOCUS_NOW_DEADLINE: Duration = Duration::from_secs(2);

/// The deadline for an ancestor walk or a tree dump, each up to 64 hops or
/// 4096 nodes.
const WALK_DEADLINE: Duration = Duration::from_secs(5);

/// How long after a menu closes the worker waits for a focus event before
/// reading the real focus itself.
pub(super) const MENU_CLOSE_GRACE: Duration = Duration::from_millis(50);

/// The worker incarnation in charge, its deadline, and the abandoned count.
#[derive(Default)]
struct WatchState {
    generation: u64,
    deadline: Option<Instant>,
    /// The query the worker is running, so an abandonment can answer it.
    running: Option<(u64, TraceId)>,
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

    /// Starts the worker's deadline for one entry.
    fn start(&self, deadline: Duration, running: Option<(u64, TraceId)>) {
        let mut state = self.lock();
        state.deadline = Some(Instant::now() + deadline);
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
            Ok(state.running.take())
        } else {
            drop(state);
            let _ = self
                .abandoned
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
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
    /// Whether a menu closed and no focus has been reported since.
    menu_closed: bool,
    /// The batch in which a focus was last reported.
    focus_batch: Option<u64>,
}

/// Publishes `message`, an entry's one result, if `generation` is still the
/// worker in charge. The check, the end of the entry's deadline, and the
/// send happen under the watch lock, so an abandoned worker can never
/// publish after its abandonment, and a published entry can no longer be
/// abandoned: a query never gets a second reply.
fn publish(context: &Context, generation: u64, message: OutpostToSupervisor) -> bool {
    let mut state = context.watch.lock();
    if state.generation != generation {
        return false;
    }
    state.deadline = None;
    state.running = None;
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
/// passes it.
fn watchdog(context: &Arc<Context>) {
    let mut state = context.watch.lock();
    loop {
        match state.deadline {
            None => {
                state = context
                    .watch
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            Some(deadline) if Instant::now() < deadline => {
                let wait = deadline.saturating_duration_since(Instant::now());
                state = context
                    .watch
                    .changed
                    .wait_timeout(state, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            Some(_) => {
                state.generation += 1;
                let abandoned = context.watch.abandoned.fetch_add(1, Ordering::Relaxed) + 1;
                state.deadline = None;
                let running = state.running.take();
                let generation = state.generation;
                tracing::warn!(
                    abandoned,
                    "a call passed its deadline; the worker is abandoned and replaced"
                );
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
    }
}

/// The worker's loop.
fn run(context: &Context, generation: u64) {
    let mut client = Client::default();
    while let Some((planned, batch)) = context.intake.next() {
        let (deadline, running) = match &planned {
            Planned::Run(entry) | Planned::Menu(entry) => budget(entry),
        };
        context.watch.start(deadline, running);
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
        match context.watch.finish(generation) {
            Err(()) => return,
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

/// The deadline for an entry, and the query it answers, if it is one.
fn budget(entry: &Entry) -> (Duration, Option<(u64, TraceId)>) {
    match &entry.item {
        Item::Query { request_id, query } => {
            let deadline = match query {
                Query::FocusNow => FOCUS_NOW_DEADLINE,
                Query::Ancestors { .. } | Query::DumpTree => WALK_DEADLINE,
                _ => STEP_DEADLINE,
            };
            (deadline, Some((*request_id, entry.trace)))
        }
        Item::Fact(_) | Item::CheckFocus => (FOCUS_DEADLINE, None),
        Item::Msaa { .. } | Item::Uia(_) => (EVENT_DEADLINE, None),
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
            Item::CheckFocus => self.check_focus(),
            Item::Query { request_id, query } => self.query(request_id, &query, trace),
        }
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
        let event = NormalizedEvent::FocusChanged {
            node,
            foreground,
            ancestors,
            selected_child,
        };
        if self.emit(trace, observed_at_ms, backend, window, event) {
            if !foreground {
                self.context.intake.set_focused(object);
            }
            let mut tracking = self.context.tracking();
            tracking.focus_role = Some(role);
            tracking.menu_closed = false;
            tracking.focus_batch = Some(self.batch);
        }
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
        match kind {
            WinEventKind::Destroy => {
                if id_object == OBJID_WINDOW.0 && id_child == CHILDID_SELF {
                    self.context.arbitrator().forget(hwnd);
                }
                return;
            }
            WinEventKind::MenuEnd => {
                self.context.tracking().menu_closed = true;
                self.context.arm_focus_check();
                return;
            }
            _ => {}
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
            // Menu openings are planned separately; one reaching here was
            // planned as an ordinary entry by mistake and is handled the
            // same way.
            menu @ DeliveredFact::MenuPopup { .. } => self.menu_opened(&Entry {
                item: Item::Fact(menu),
                trace,
                observed_at_ms,
            }),
        }
    }

    /// A foreground change: reported at once, named or not, as a focus on
    /// the window, unless the window is no longer the system's foreground
    /// window by the time it is read.
    fn foreground(&mut self, hwnd: isize, trace: TraceId, observed_at_ms: u64) {
        if window_belongs_to_hidden_frame(hwnd) {
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
            return; // UIA owns this window; its UIA fact reports the focus.
        }
        let Some(node) = verbatim_ia2::acquire::snapshot_from_focus_event(
            hwnd,
            id_object,
            id_child,
            &self.context.msaa_registry,
        ) else {
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
            return;
        };
        // SAFETY: `element` was built with the base cache request.
        let parts = unsafe { snapshot_parts_from_cached_element(&element) };
        if parts.runtime_id != fact.runtime_id {
            return; // Focus has moved on; the newer fact follows.
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
            return; // MSAA owns this window; its MSAA fact reports the focus.
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

    /// A menu opening, handled after the batch's focus events, as NVDA's
    /// MSAA handler does. Ignored when a focus reported in the same batch
    /// already put focus on a menu or menu item, when the window belongs to
    /// UIA (NVDA does not use MSAA objects proxied from native UIA), or when
    /// its object is not a popup menu; otherwise it becomes a focus on the
    /// popup menu.
    fn menu_opened(&mut self, entry: &Entry) {
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

    /// After a menu closed: if no focus has been reported since, read the
    /// real focus and report it.
    fn check_focus(&mut self) {
        if !self.context.tracking().menu_closed {
            return;
        }
        let Some(control) = read::focused_control(self.context, self.client) else {
            self.context.tracking().menu_closed = false;
            return;
        };
        let hwnd = focus_window().filter(|&hwnd| !window_is_hidden_frame(hwnd));
        let backend = control.node.backend;
        let object = self.object_of(&control.node);
        self.emit_focus(
            TraceId::mint(),
            super::window::now_ms(),
            backend,
            hwnd,
            control.node,
            false,
            object,
            (control.ancestors, control.selected_child),
        );
    }

    /// The accessible object a node this outpost issued stands for.
    fn object_of(&self, node: &NodeSnapshot) -> Option<Object> {
        if let Some(runtime_id) = self.context.uia_registry.runtime_id_of(node.id) {
            return Some(Object::Uia(runtime_id));
        }
        self.context
            .msaa_registry
            .key_of(node.id)
            .map(|(hwnd, object, child)| Object::Msaa(hwnd, object, child))
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
