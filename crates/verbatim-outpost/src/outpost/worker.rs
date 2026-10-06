//! The worker and its watchdog (outpost redesign, "Inside an outpost").
//!
//! One worker thread takes entries from the intake queue in order and
//! finishes each before starting the next. It is the only thread that calls
//! into the application, so events and replies leave the outpost in the order
//! their entries joined the queue. This is NVDA's model, one thread doing all
//! the work, with one such thread per application.
//!
//! Being the only thread that calls into the application, the worker is also
//! where those calls are counted: the backend crates count each call on the
//! thread that makes it, and the worker takes the count when it publishes an
//! event or a reply, which carries it to Core with the timing
//! ([`take_calls`]). Calls an entry makes without publishing anything, for an
//! event it drops, are taken and logged when the entry ends and belong to no
//! trace (`docs/performance.md`, "Cancelled traces").
//!
//! The watchdog watches the worker's deadline. If a call hangs past it, the
//! watchdog abandons the worker (a hung cross-process call cannot be stopped
//! safely), answers the stuck query "abandoned" if it was a query, counts the
//! abandoned worker, and starts a replacement that continues with the rest of
//! the queue. An abandoned worker that eventually returns publishes nothing,
//! lowers the count, and exits.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use verbatim_ia2::{CHILDID_SELF, WinEventKind};
use verbatim_model::{
    Backend, CallCounts, NodeId, NodeSnapshot, NormalizedEvent, PropertyChange, Role, State,
    TraceId,
};
use verbatim_uia::map::{
    cached_process_id, snapshot_from_cached_element, with_legacy_checked_state,
};
use verbatim_uia::{map::snapshot_parts_from_cached_element, nearest_window_handle};
use windows::Win32::UI::Accessibility::{
    UIA_NamePropertyId, UIA_RangeValueValuePropertyId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use crate::protocol::{
    DeliveredFact, EventTiming, FocusedControl, OutpostToSupervisor, Query, QueryOutcome,
    QueryResult, UiaSnapshotFact, now_us,
};

use super::Context;
use super::intake::{Entry, HeldFocus, Item, Object, Planned, UiaEvent, UiaKind, window_of};
use super::read::{self, Client, ReadError};
use super::text_reads::{self, CONSOLE_WINDOW_CLASS};
use super::window::{
    focus_window_of, front_is_another_thread_of_its_application, now_ms,
    window_belongs_to_hidden_frame, window_facts, window_is_foreground,
};
use crate::arbitration::window_class_name;
use windows::Win32::UI::Accessibility::IUIAutomationElement;

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

/// How long a UIA focus waits to find its element live, for ancestors and
/// navigation, before it is reported from the event alone. Such a read
/// normally answers in 10 to 100 ms.
const FOCUS_READ_WAIT: Duration = Duration::from_secs(1);

/// How many times a follow-up looks for the live element of a focus reported
/// from its event alone.
const FOCUS_RESOLVE_ATTEMPTS: u32 = 3;

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

/// The deadline for a text request: a caret key's wait for evidence takes
/// up to 300 ms in a terminal, and the reads after it a few calls more.
const TEXT_DEADLINE: Duration = Duration::from_secs(2);

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

    /// Runs `send` under the watch lock if `generation` is still the worker in
    /// charge, ending its entry first, and says whether it ran. The watchdog
    /// abandons under the same lock, so an entry is either published or
    /// abandoned, never both.
    fn publish(&self, generation: u64, send: impl FnOnce(&mut WatchState)) -> bool {
        let mut state = self.lock();
        if state.generation != generation {
            return false;
        }
        state.deadline = None;
        state.started = None;
        state.running = None;
        send(&mut state);
        true
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
    role: Option<Role>,
    /// The batch in which a focus was last reported.
    batch: Option<u64>,
    /// The last focus this outpost reported and its ancestors, outermost
    /// first: where the next focus's ancestor walk can stop and reuse the
    /// rest, as NVDA's does.
    chain: Vec<NodeSnapshot>,
    /// The window of the last focus this outpost reported, for events on
    /// that focus, which then need no call to find their window.
    window: Option<isize>,
    /// How many focuses (not foreground changes) this outpost has reported,
    /// so the worker can tell when a focus candidate was reported.
    reported: u64,
    /// The dialog the last foreground report gathered text for, which the
    /// focus that follows it reuses.
    dialog: read::DialogMemo,
}

impl Tracking {
    /// The window of the last focus this outpost reported.
    pub(super) fn window(&self) -> Option<isize> {
        self.window
    }
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
    context.watch.publish(generation, |state| {
        let touched = context
            .uia_registry
            .take_touched()
            .into_iter()
            .chain(context.msaa_registry.take_touched());
        state.record(message.carries_nodes(), touched.map(NodeId::number));
        context.outbound.send(message);
    })
}

/// The cross-process calls this thread has made through either backend
/// since the last take, resetting the count. Only the worker calls into the
/// application, so on the worker this is what the entry in hand has made.
pub(super) fn take_calls() -> CallCounts {
    verbatim_uia::calls::take() + verbatim_ia2::calls::take()
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

/// The activity id of Windows Terminal's output notifications, each a piece
/// of what a program wrote.
const TERMINAL_OUTPUT_ACTIVITY: &str = "TerminalTextOutput";

/// Whether a UIA notification is a terminal's output notification, which
/// the generic notification handling ignores: the outpost finds a
/// terminal's output by diffing its text, and speaking the notifications
/// too would speak every line twice (`phase6-design.md`, "Why notifications
/// exist, and what went wrong with them"). Only a terminal's own are
/// ignored, keyed by the control, as NVDA's terminal overlays ignore them.
fn is_terminal_output_notification(
    role: Role,
    notification: &verbatim_model::Notification,
) -> bool {
    role == Role::Terminal && notification.activity_id.as_deref() == Some(TERMINAL_OUTPUT_ACTIVITY)
}

/// Why the watchdog abandons a worker at `now`, if it does: the entry it
/// started at `started`, concerning `window` (0 for none), has passed its
/// `deadline`, or has waited [`MOVED_ON_GRACE`] and `moved_on` says the user
/// has left `window` for another thread of its application.
fn abandon_reason(
    now: Instant,
    deadline: Instant,
    started: Instant,
    window: isize,
    moved_on: impl FnOnce(isize) -> bool,
) -> Option<&'static str> {
    if now >= deadline {
        Some("a call passed its deadline")
    } else if window != 0 && now >= started + MOVED_ON_GRACE && moved_on(window) {
        Some("the user moved on from a slow window")
    } else {
        None
    }
}

/// When the watchdog next checks a worker it did not abandon at `now`: at
/// the deadline for an entry concerning no window, else at the end of the
/// grace and every [`MOVED_ON_GRACE`] after it, never later than the
/// deadline.
fn next_check(now: Instant, deadline: Instant, started: Instant, window: isize) -> Instant {
    let check = if window == 0 {
        deadline
    } else if now < started + MOVED_ON_GRACE {
        started + MOVED_ON_GRACE
    } else {
        now + MOVED_ON_GRACE
    };
    check.min(deadline)
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
        let reason = abandon_reason(
            now,
            deadline,
            started,
            window,
            front_is_another_thread_of_its_application,
        );
        let Some(reason) = reason else {
            let wait = next_check(now, deadline, started, window).saturating_duration_since(now);
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
                timing: EventTiming::default(),
            });
        }
        spawn_worker(Arc::clone(context), generation);
    }
}

/// The worker's thread: its loop, then the release of the UIA objects
/// `verbatim-uia` kept for this thread, before the thread exits rather than
/// from its thread-local destructors under the loader lock.
fn run(context: &Context, generation: u64) {
    // Held objects are agile references, resolved in this apartment.
    if let Err(error) = verbatim_uia::init_mta() {
        tracing::warn!(%error, "the worker could not join the multithreaded apartment");
    }
    run_loop(context, generation);
    verbatim_uia::release_thread_state();
}

/// The worker's loop, until the intake closes or this worker is abandoned.
fn run_loop(context: &Context, generation: u64) {
    let mut client = Client::default();
    // When the current batch's foreground change was confirmed. `next` names
    // the window only with a batch's first entry, and the foreground fact
    // need not be that entry, so the time is kept for the whole batch.
    let mut confirmed: Option<(u64, Option<u64>)> = None;
    while let Some((planned, batch, foreground)) = context.intake.next() {
        if let Some(hwnd) = foreground {
            confirmed = Some((batch, wait_for_foreground(hwnd)));
        }
        let foreground_at_ms = confirmed
            .filter(|(confirmed_batch, _)| *confirmed_batch == batch)
            .and_then(|(_, at)| at);
        let mut run = |entry: Entry, menu: bool| {
            run_entry(
                context,
                &mut client,
                generation,
                (batch, foreground_at_ms),
                entry,
                menu,
            )
        };
        let outcome = match planned {
            Planned::Run(entry) => run(entry, false),
            Planned::Menu(entry) => run(entry, true),
            Planned::Focus(entries) => {
                // Newest first, until one is reported.
                let mut outcome = Ok(());
                for entry in entries {
                    let reported = context.tracking().reported;
                    outcome = run(entry, false);
                    if outcome.is_err() || context.tracking().reported != reported {
                        break;
                    }
                }
                outcome
            }
        };
        if outcome.is_err() {
            return;
        }
    }
}

/// Handles one entry of `batch`, whose foreground change, if it holds one,
/// was confirmed at `foreground_at_ms`, under its deadline. `Err` when this
/// worker was abandoned meanwhile and must exit.
fn run_entry(
    context: &Context,
    client: &mut Client,
    generation: u64,
    (batch, foreground_at_ms): (u64, Option<u64>),
    entry: Entry,
    menu: bool,
) -> Result<(), ()> {
    let timing = EventTiming {
        dequeued_at_us: now_us(),
        ..entry.timing
    };
    let (deadline, running) = budget(&entry);
    context
        .watch
        .start(deadline, running, window_of(&entry.item));
    let description = describe(&entry.item);
    let trace = entry.trace;
    let started = Instant::now();
    let handled = catch_unwind(AssertUnwindSafe(|| {
        let mut worker = Worker {
            context,
            client,
            generation,
            batch,
            foreground_at_ms,
            timing,
        };
        if menu {
            worker.menu_opened(&entry);
        } else {
            worker.handle(entry);
        }
    }));
    context
        .arbitrator()
        .renew_probes_since(started, Instant::now());
    // Calls made after the entry's last publish, or by an entry that
    // published nothing: they belong to no trace.
    let unpublished = take_calls();
    tracing::debug!(
        %trace,
        item = %description,
        elapsed_us = started.elapsed().as_micros(),
        ?unpublished,
        "handled"
    );
    match context.watch.finish(generation) {
        Err(()) => {
            tracing::info!(
                elapsed_ms = started.elapsed().as_millis(),
                deadline_ms = deadline.as_millis(),
                "an abandoned worker returned; its result is discarded"
            );
            return Err(());
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
                outcome: QueryOutcome::Failed("the outpost failed handling the query".to_owned()),
                timing: EventTiming::default(),
            });
        }
        Ok(None) => {
            if handled.is_err() {
                tracing::error!("the worker failed handling an event");
            }
        }
    }
    Ok(())
}

/// Waits, up to [`FOREGROUND_WAIT`], for `hwnd` to become the foreground
/// window, as NVDA holds back event handling after a foreground event until
/// the foreground window matches (`_shouldGetEvents`, issue 3831). Local
/// calls only; the application is never asked.
fn wait_for_foreground(hwnd: isize) -> Option<u64> {
    let deadline = Instant::now() + FOREGROUND_WAIT;
    while !window_is_foreground(hwnd) {
        if Instant::now() >= deadline {
            tracing::debug!(
                hwnd,
                "the foreground window did not become the event's window"
            );
            return None;
        }
        thread::sleep(FOREGROUND_POLL);
    }
    Some(now_ms())
}

/// The deadline for an entry, and the query it answers, if it is one.
fn budget(entry: &Entry) -> (Duration, Option<(u64, TraceId)>) {
    match &entry.item {
        Item::Query { request_id, query } => {
            let deadline = match query {
                Query::FocusNow => HANDLING_DEADLINE,
                Query::Ancestors { .. } | Query::DumpTree => WALK_DEADLINE,
                Query::Text { .. } => TEXT_DEADLINE,
                _ => STEP_DEADLINE,
            };
            (deadline, Some((*request_id, entry.trace)))
        }
        Item::Fact(_)
        | Item::Msaa { .. }
        | Item::Uia(_)
        | Item::ResolveFocus { .. }
        | Item::CaretOf { .. } => (HANDLING_DEADLINE, None),
        // Releasing thousands of objects after a tree dump takes a while.
        Item::NodesHeld { .. } => (WALK_DEADLINE, None),
    }
}

/// A short description of an entry's item, for the debug log of what the
/// worker spent its time on.
fn describe(item: &Item) -> String {
    match item {
        Item::Msaa {
            kind,
            hwnd,
            id_object,
            id_child,
        } => format!("MSAA {kind:?} hwnd={hwnd} object={id_object} child={id_child}"),
        Item::Uia(event) => format!("UIA {:?}", event.kind),
        Item::Fact(fact) => format!("fact {:?}", fact.key()),
        Item::Query { query, .. } => format!("query {query:?}"),
        Item::NodesHeld { nodes, .. } => format!("nodes held ({})", nodes.len()),
        Item::ResolveFocus { attempt, .. } => format!("resolve focus (attempt {attempt})"),
        Item::CaretOf { node_id } => format!("caret of {node_id:?}"),
    }
}

/// What reading the focused element found for a focus fact.
enum LiveFocus {
    /// The fact's element, live.
    Found(IUIAutomationElement),
    /// The keyboard focus is in another application now: the fact is out of
    /// date, and the newer focus's own event reports it.
    InAnotherApplication,
    /// Another element of this application has the keyboard focus: the
    /// fact is most likely out of date, unless the application is still
    /// starting and answered with a stand-in for its window.
    Elsewhere,
    /// The read did not answer in time or failed.
    Unresolved,
}

/// One entry's handling, by the worker in charge.
struct Worker<'a> {
    context: &'a Context,
    client: &'a mut Client,
    generation: u64,
    /// The batch the entry belongs to.
    batch: u64,
    /// When the batch's foreground change was confirmed: its window had
    /// become the system's foreground window.
    foreground_at_ms: Option<u64>,
    /// When the entry was raised, observed, relayed, and dequeued, for the
    /// latency log.
    timing: EventTiming,
}

impl Worker<'_> {
    fn handle(&mut self, entry: Entry) {
        let Entry {
            item,
            trace,
            observed_at_ms,
            ..
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
            Item::ResolveFocus {
                runtime_id,
                attempt,
                held,
            } => self.resolve_focus(&runtime_id, trace, attempt, held),
            Item::CaretOf { node_id } => self.caret_of(node_id, trace, observed_at_ms, true),
        }
    }

    /// Whether `node_id` is the focus this outpost last reported.
    fn is_focus(&self, node_id: NodeId) -> bool {
        let object = if let Some(runtime_id) = self.context.uia_registry.runtime_id_of(node_id) {
            Object::Uia(runtime_id)
        } else if let Some((hwnd, object, child)) = self.context.msaa_registry.key_of(node_id) {
            Object::Msaa(hwnd, object, child)
        } else {
            return false;
        };
        self.context.intake.focused() == Some(object)
    }

    /// Reports the caret of `node_id` as `CaretMoved`, when it is still the
    /// focus and the worker has not read its caret since the event that
    /// asked: a caret key's answer, read after the event, already told Core.
    /// For the report that follows a new focus (`after_focus`), a focus with
    /// no text to read, or whose caret could not be read, is reported as
    /// `NoText`, and Core speaks its value instead of a line.
    fn caret_of(
        &mut self,
        node_id: NodeId,
        trace: TraceId,
        observed_at_ms: u64,
        after_focus: bool,
    ) {
        if !self.is_focus(node_id)
            || self
                .context
                .caret_read_since(node_id, self.timing.observed_at_us)
        {
            return;
        }
        let event = match text_reads::report_caret(self.context, node_id, after_focus) {
            Some(caret) => NormalizedEvent::CaretMoved { node_id, caret },
            None if after_focus => NormalizedEvent::NoText { node_id },
            None => return,
        };
        let backend = if self.context.uia_registry.runtime_id_of(node_id).is_some() {
            Backend::Uia
        } else {
            Backend::Msaa
        };
        let window = self.context.tracking().window;
        self.emit(trace, observed_at_ms, backend, window, event);
        if after_focus && self.focused_terminal(node_id) {
            // Where the terminal's text ends now: what it held before the
            // focus arrived is not new output.
            if let Some(uia) = self.client.uia() {
                let _ = text_reads::terminal_output(self.context, uia, node_id, true);
            }
        }
    }

    /// Whether `node_id` is the focus and a terminal read through UIA, whose
    /// new output the outpost finds by diffing its text.
    fn focused_terminal(&self, node_id: NodeId) -> bool {
        self.context.uia_registry.runtime_id_of(node_id).is_some()
            && self.is_focus(node_id)
            && self.context.tracking().role == Some(Role::Terminal)
    }

    /// Reports a focused terminal's new output, when its text really
    /// changed: a redraw with the same text sends nothing.
    fn terminal_output(&mut self, node_id: NodeId, trace: TraceId, observed_at_ms: u64) {
        let Some(uia) = self.client.uia() else {
            return;
        };
        let Some(output) = text_reads::terminal_output(self.context, uia, node_id, false) else {
            return;
        };
        if output.is_empty() {
            return;
        }
        let window = self.context.tracking().window;
        self.emit(
            trace,
            observed_at_ms,
            Backend::Uia,
            window,
            NormalizedEvent::TerminalOutput { node_id, output },
        );
    }

    /// Asks for the caret of a newly reported focus that may have text, or
    /// whose role says it may (Core waits to hear which before it speaks
    /// the line or the value), and moves the subscription to caret and text
    /// changes to it (to nothing for a focus without text, or one read
    /// through MSAA, whose caret events come from the hooks).
    fn follow_text(&self, node: &NodeSnapshot, trace: TraceId) {
        let has_text = text_reads::may_have_text(self.context, node);
        let role_has_text = matches!(
            node.role,
            Role::EditableText | Role::Document | Role::Terminal
        );
        if let Some(subscription) = self.context.text_events.get() {
            let element = has_text
                .then(|| self.context.uia_registry.element_of(node.id))
                .flatten();
            subscription.retarget(match element {
                Some(element) => verbatim_uia::Scope::Elements(vec![element]),
                None => verbatim_uia::Scope::Nothing,
            });
        }
        if has_text || role_has_text {
            self.context.intake.push(Entry {
                item: Item::CaretOf { node_id: node.id },
                trace,
                observed_at_ms: now_ms(),
                timing: EventTiming {
                    observed_at_us: now_us(),
                    ..EventTiming::default()
                },
            });
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
        let (objects, released) = {
            let mut state = self.context.watch.lock();
            let released = state.take_releasable(&held, acknowledged);
            if released.is_empty() {
                return;
            }
            let keep = |id: NodeId| !released.contains(&id.number());
            (
                (
                    self.context.uia_registry.retain(keep),
                    self.context.msaa_registry.retain(keep),
                ),
                released,
            )
        };
        text_reads::forget(self.context, released);
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
                timing: EventTiming {
                    published_at_us: now_us(),
                    calls: take_calls(),
                    ..self.timing
                },
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
        &mut self,
        trace: TraceId,
        observed_at_ms: u64,
        backend: Backend,
        window: Option<isize>,
        node: NodeSnapshot,
        foreground: bool,
        object: Option<Object>,
        (ancestors, selected_child): read::Enrichment,
    ) {
        let mut node = node;
        let mut ancestors = ancestors;
        // The console host's text area is a terminal, known by its window
        // (Windows Terminal's control is known by its UIA class). Its name,
        // "Text Area", is not localized, so it is dropped, as NVDA's console
        // class drops it, and the focus says "terminal".
        if backend == Backend::Uia
            && matches!(node.role, Role::EditableText | Role::Document)
            && window.is_some_and(|hwnd| window_class_name(hwnd) == CONSOLE_WINDOW_CLASS)
        {
            node.role = Role::Terminal;
            node.name = None;
        }
        // A dialog the focus has newly entered, or one in front, says its
        // own text as its description.
        let previous = self.focus_chain();
        let mut memo = self.context.tracking().dialog.take();
        read::describe_dialogs(
            self.context,
            self.client,
            ancestors
                .iter_mut()
                .flatten()
                .chain(std::iter::once(&mut node)),
            (&previous, &mut memo, foreground),
        );
        self.context.tracking().dialog = memo;
        let role = node.role;
        let text_node = (!foreground).then(|| node.clone());
        tracing::debug!(
            ?role,
            name = ?node.name,
            foreground,
            ?backend,
            ancestors_unknown = ancestors.is_none(),
            "focus reported"
        );
        let chain = ancestors.as_ref().map(|ancestors| {
            ancestors
                .iter()
                .cloned()
                .chain(std::iter::once(node.clone()))
                .collect::<Vec<_>>()
        });
        let event = NormalizedEvent::FocusChanged {
            node,
            foreground,
            ancestors_unknown: ancestors.is_none(),
            ancestors: ancestors.unwrap_or_default(),
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
            tracking.role = Some(role);
            tracking.batch = Some(self.batch);
            if !foreground {
                tracking.reported += 1;
                tracking.window = window;
                // Unknown ancestors leave the previous chain, as the reducer
                // keeps it.
                if let Some(chain) = chain {
                    tracking.chain = chain;
                }
            }
            drop(tracking);
            if let Some(node) = text_node {
                self.follow_text(&node, trace);
            }
        }
    }

    /// The last focus this outpost reported, with its ancestors.
    fn focus_chain(&self) -> Vec<NodeSnapshot> {
        self.context.tracking().chain.clone()
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
                self.context.forget_window(hwnd);
                // A reused window handle must never inherit these nodes.
                self.context.msaa_registry.forget_window(hwnd);
            }
            return;
        }
        if read::window_uses_uia(self.context, hwnd) {
            return; // UIA owns this window.
        }
        // The caret, a text selection, or the text of the focus, when it is
        // an edit control: reported from the control's messages, without
        // reading its MSAA object (whose value is its whole text).
        let client = Object::Msaa(hwnd, OBJID_CLIENT.0, CHILDID_SELF);
        let of_focus = self.context.intake.focused() == Some(client);
        match kind {
            WinEventKind::Caret | WinEventKind::TextSelectionChange => {
                if of_focus {
                    let node_id =
                        self.context
                            .msaa_registry
                            .id_for((hwnd, OBJID_CLIENT.0, CHILDID_SELF));
                    self.caret_of(node_id, trace, observed_at_ms, false);
                }
                return;
            }
            WinEventKind::ValueChange
                if of_focus
                    && id_object == OBJID_CLIENT.0
                    && id_child == CHILDID_SELF
                    && verbatim_ia2::edit::edit_api_version(
                        &crate::arbitration::normalize_class_name(&window_class_name(hwnd)),
                    )
                    .is_some() =>
            {
                let node_id =
                    self.context
                        .msaa_registry
                        .id_for((hwnd, OBJID_CLIENT.0, CHILDID_SELF));
                self.emit(
                    trace,
                    observed_at_ms,
                    Backend::Msaa,
                    Some(hwnd),
                    NormalizedEvent::TextChanged { node_id },
                );
                return;
            }
            _ => {}
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
        // The event's window: its own, else the recorded window of the focus
        // it concerns, else its element's. Never the system's focus window,
        // which can belong to another application.
        let of_focus =
            self.context.intake.focused() == Some(Object::Uia(event.parts.runtime_id.clone()));
        let hwnd = if event.hwnd != 0 {
            Some(event.hwnd)
        } else if of_focus {
            self.context.tracking().window
        } else {
            element.as_ref().and_then(nearest_window_handle)
        };
        // NVDA does not arbitrate notifications; every other event is
        // dropped when MSAA owns its window.
        if !matches!(event.kind, UiaKind::Notification(_))
            && let Some(hwnd) = hwnd
            && !read::window_uses_uia(self.context, hwnd)
        {
            return; // MSAA owns this window.
        }
        if let UiaKind::Notification(notification) = &event.kind
            && is_terminal_output_notification(event.parts.role, notification)
        {
            return; // The diff of the terminal's text reports it.
        }
        if matches!(event.kind, UiaKind::Selection)
            && let Some(selection) = self.controlled_selection(&event.parts.runtime_id)
        {
            self.emit(trace, observed_at_ms, Backend::Uia, hwnd, selection);
            return;
        }
        // A text focus's caret and text: the subscription follows only the
        // focus, so these are the focus's.
        match event.kind {
            UiaKind::TextSelection => {
                if let Some(node_id) = self
                    .context
                    .uia_registry
                    .existing_id(&event.parts.runtime_id)
                {
                    self.caret_of(node_id, trace, observed_at_ms, false);
                }
                return;
            }
            UiaKind::TextChanged => {
                if let Some(node_id) = self
                    .context
                    .uia_registry
                    .existing_id(&event.parts.runtime_id)
                {
                    if self.focused_terminal(node_id) {
                        self.terminal_output(node_id, trace, observed_at_ms);
                        return;
                    }
                    self.emit(
                        trace,
                        observed_at_ms,
                        Backend::Uia,
                        hwnd,
                        NormalizedEvent::TextChanged { node_id },
                    );
                }
                return;
            }
            _ => {}
        }
        let node = Self::uia_node(self.context, &event.parts, element.as_ref());
        let normalized = match event.kind {
            UiaKind::Property(id) if id == UIA_NamePropertyId.0 => {
                NormalizedEvent::PropertyChanged {
                    node_id: node.id,
                    change: PropertyChange::Name(node.name),
                }
            }
            UiaKind::Property(id)
                if id == UIA_ValueValuePropertyId.0 || id == UIA_RangeValueValuePropertyId.0 =>
            {
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
            UiaKind::TextSelection | UiaKind::TextChanged => return,
        };
        self.emit(trace, observed_at_ms, Backend::Uia, hwnd, normalized);
    }

    /// The selection of the element `runtime_id` names as a selection in a
    /// list the focus controls, when it is one: the focused element names,
    /// in its `ControllerFor` relation, an element the selected one is inside
    /// ("Selection in a list the focus controls" in `docs/nvda/events.md`).
    fn controlled_selection(&mut self, runtime_id: &[i32]) -> Option<NormalizedEvent> {
        // The focus this outpost last reported, as NVDA uses its focus
        // object; read live, briefly, only when its element is not known.
        let Some(Object::Uia(focus_id)) = self.context.intake.focused() else {
            return None;
        };
        let registry = &self.context.uia_registry;
        let known = registry
            .existing_id(&focus_id)
            .and_then(|id| registry.element_of(id))
            .and_then(|agile| agile.resolve().ok());
        let focused = match known {
            Some(element) => element,
            None => match self.live_focus_element(&focus_id) {
                LiveFocus::Found(element) => element,
                LiveFocus::InAnotherApplication | LiveFocus::Elsewhere | LiveFocus::Unresolved => {
                    return None;
                }
            },
        };
        let uia = self.client.uia()?;
        let cache = self.context.uia_cache(uia).ok()?;
        let selected = uia
            .controlled_descendant(&focused, runtime_id, &cache)
            .ok()
            .flatten()?;
        let registry = &self.context.uia_registry;
        // Both elements were built with the base cache request.
        let controller = snapshot_parts_from_cached_element(&focused).runtime_id;
        Some(NormalizedEvent::ControlledSelection {
            controller: registry.id_for_element(&controller, &focused),
            node: snapshot_from_cached_element(&selected, registry),
        })
    }

    /// A snapshot from a UIA element's cached parts, its id minted by the
    /// registry, which keeps the element when there is one.
    fn uia_node(
        context: &Context,
        parts: &UiaSnapshotFact,
        element: Option<&windows::Win32::UI::Accessibility::IUIAutomationElement>,
    ) -> NodeSnapshot {
        let registry = &context.uia_registry;
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
            DeliveredFact::UiaFocus {
                hwnd,
                focus_window,
                snapshot,
            } => {
                self.uia_focus((hwnd, focus_window), &snapshot, trace, observed_at_ms);
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
                    timing: crate::protocol::EventTiming::default(),
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
    ///
    /// It is stamped with the time its window was confirmed as the
    /// foreground, not the time Windows raised the event: Windows raises it
    /// before the change completes (measured live: Notepad's event arrived
    /// while the desktop was still the foreground window, which it stayed
    /// for about 130 ms more), and the old foreground window's own focus
    /// events in that interval would otherwise be newer than the change
    /// and make Core drop it as stale. NVDA judges a foreground event
    /// against the foreground window when it processes the event, so it
    /// orders the change at the same point.
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
        // Confirmed by the wait before the batch, or, when the wait timed
        // out, by the check just made.
        let confirmed_at_ms = self.foreground_at_ms.unwrap_or_else(now_ms);
        self.emit_focus(
            trace,
            observed_at_ms.max(confirmed_at_ms),
            backend,
            Some(hwnd),
            node,
            true,
            None,
            (Some(Vec::new()), None),
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
        let previous = self.focus_chain();
        let enrichment = read::msaa_enrichment(self.context, self.client, &node, &previous);
        // NVDA accepts an MSAA focus only when the object or one of its
        // ancestors has the focused state (`shouldAllowIAccessibleFocusEvent`),
        // which weeds out stale and spurious focus events. Ancestors that
        // could not be read in time leave the event accepted.
        if let (false, Some(ancestors)) = (node.states.contains(State::Focused), &enrichment.0)
            && !ancestors
                .iter()
                .any(|ancestor| ancestor.states.contains(State::Focused))
        {
            tracing::debug!(
                hwnd,
                id_object,
                id_child,
                "MSAA focus dropped: nothing has the focused state"
            );
            return;
        }
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

    /// A UIA focus fact. What the focus is comes from the event, as NVDA
    /// builds the focus from the event's sender and its cached properties,
    /// and only when the event says the element has the keyboard focus
    /// (NVDA's `shouldAllowUIAFocusEvent`). The element itself is in the
    /// listener's process and cannot cross to this one, so the outpost finds
    /// its own copy, for the ancestors and for navigation, by reading the
    /// focused element, with a short wait: an application busy starting up
    /// can leave that read unanswered for more than ten seconds, or answer
    /// with UIA's stand-in for its window (Windows 11 Notepad's text area as
    /// a nameless edit). Without it the focus is still reported, with its
    /// ancestors unknown, and a follow-up finds the element later for the
    /// focus-following property subscription.
    ///
    /// A fact whose element has lost the keyboard focus to another
    /// application is dropped: the newer focus's own event reports it, and
    /// NVDA would find the element's window no longer in the foreground.
    fn uia_focus(
        &mut self,
        (fact_hwnd, focus_window): (isize, isize),
        fact: &UiaSnapshotFact,
        trace: TraceId,
        observed_at_ms: u64,
    ) {
        let context = self.context;
        if !fact.states.contains(State::Focused) {
            tracing::debug!("UIA focus dropped: the element does not have the keyboard focus");
            return;
        }
        let reading = Instant::now();
        let element = match self.live_focus_element(&fact.runtime_id) {
            LiveFocus::Found(element) => Some(element),
            LiveFocus::InAnotherApplication => {
                tracing::debug!("UIA focus dropped: the focus is in another application now");
                return;
            }
            LiveFocus::Elsewhere => {
                // Focus has moved on within the application since the event
                // was raised, and the newer focus's own event follows: NVDA
                // drops such an event once its element no longer has the
                // keyboard focus (`shouldAllowUIAFocusEvent`, since 2027.1),
                // or a container is announced after the item a key moved
                // into. An application still starting can answer with a
                // stand-in for its window instead, so the event is held, and
                // reported if the follow-up finds its element focused after
                // all.
                tracing::debug!("UIA focus held: another element has the keyboard focus");
                self.hold_focus((fact_hwnd, focus_window), fact, trace, observed_at_ms);
                return;
            }
            LiveFocus::Unresolved => None,
        };
        let element_us = reading.elapsed().as_micros();
        let previous = self.focus_chain();
        // The window the focus is in, known without a call: the event's
        // own, else the application's keyboard focus window when the
        // listener captured the event. A remote operation is tried unless
        // that window's elements cannot be imported into one.
        let focus_in = [fact_hwnd, focus_window]
            .into_iter()
            .find(|&hwnd| hwnd != 0);
        let enriching = Instant::now();
        let remote = match self.remote_enrichment(element.as_ref(), &previous, focus_in) {
            Some(read::RemoteEnrichment::NotFocused) => {
                // Read live in the same round trip: the focus moved on after
                // the focused element was read, as for `LiveFocus::Elsewhere`.
                tracing::debug!("UIA focus held: the element lost the keyboard focus");
                self.hold_focus((fact_hwnd, focus_window), fact, trace, observed_at_ms);
                return;
            }
            Some(read::RemoteEnrichment::Read { enrichment, window }) => Some((enrichment, window)),
            None => None,
        };
        // The event's own window; else the element's nearest, which a
        // remote read found with its ancestors, or, when the provider could
        // not tell it, the classic walk finds; else, for an element not
        // resolved, this application's keyboard focus window when the
        // listener captured the event, which hosts the element when the
        // event is current but, in an application with several windows, may
        // be another of its windows when the event is late; else, for
        // deciding the backend only, this application's focus window now,
        // which is never reported.
        //
        // The console host's provider answers no native window handle for
        // its window inside a remote operation (only the client side's
        // window proxy supplies it), and its window is not this process's
        // by `GetWindowThreadProcessId`, which names the console's client
        // instead, so for its text area only the classic walk finds the
        // window.
        let reported = if fact_hwnd != 0 {
            Some(fact_hwnd)
        } else {
            remote
                .as_ref()
                .and_then(|(_, window)| *window)
                .or_else(|| element.as_ref().and_then(nearest_window_handle))
                .or((focus_window != 0).then_some(focus_window))
        };
        let judged = reported.or_else(|| focus_window_of(context.target_pid));
        if judged.is_some_and(window_belongs_to_hidden_frame) {
            return;
        }
        if let Some(hwnd) = judged
            && !read::window_uses_uia(context, hwnd)
        {
            // MSAA owns this window; its MSAA fact reports the focus.
            tracing::debug!(hwnd, "UIA focus dropped: MSAA owns the window");
            return;
        }
        let object = Some(Object::Uia(fact.runtime_id.clone()));
        let Some(element) = element else {
            tracing::debug!("UIA focus reported from the event: its element was not found in time");
            let node = Self::uia_node(context, fact, None);
            self.emit_focus(
                trace,
                observed_at_ms,
                Backend::Uia,
                reported,
                node,
                false,
                object,
                (None, None),
            );
            self.resolve_focus_later(&fact.runtime_id, trace, 1);
            return;
        };
        let node = Self::uia_node(context, fact, Some(&element));
        let node = with_legacy_checked_state(&element, node); // Menu items only.
        let enrichment = match remote {
            Some((enrichment, _)) => enrichment,
            None => self.classic_enrichment(&element, node.role, &previous),
        };
        tracing::debug!(
            %trace,
            element_us,
            enrichment_us = enriching.elapsed().as_micros(),
            total_us = reading.elapsed().as_micros(),
            "UIA focus read"
        );
        self.emit_focus(
            trace,
            observed_at_ms,
            Backend::Uia,
            reported,
            node,
            false,
            object,
            enrichment,
        );
    }

    /// [`read::uia_enrichment`], the classic walk, for a focus's element.
    fn classic_enrichment(
        &mut self,
        element: &IUIAutomationElement,
        role: Role,
        previous: &[NodeSnapshot],
    ) -> read::Enrichment {
        let Some(uia) = self.client.uia() else {
            return (None, None);
        };
        match self.context.uia_cache(uia) {
            Ok(cache) => read::uia_enrichment(self.context, uia, &cache, element, role, previous),
            Err(_) => (None, None),
        }
    }

    /// [`read::uia_remote_enrichment`] for a focus's element, when there is
    /// one and a client to read it with.
    fn remote_enrichment(
        &mut self,
        element: Option<&IUIAutomationElement>,
        previous: &[NodeSnapshot],
        focus_in: Option<isize>,
    ) -> Option<read::RemoteEnrichment> {
        let element = element?;
        let uia = self.client.uia()?;
        let cache = self.context.uia_cache(uia).ok()?;
        read::uia_remote_enrichment(self.context, uia, &cache, element, previous, focus_in)
    }

    /// Holds back a UIA focus fact whose element does not have the keyboard
    /// focus now, for a follow-up that reports it if its element is found
    /// focused after all.
    fn hold_focus(
        &self,
        windows: (isize, isize),
        fact: &UiaSnapshotFact,
        trace: TraceId,
        observed_at_ms: u64,
    ) {
        self.resolve_focus_later_held(
            &fact.runtime_id,
            trace,
            1,
            Some(Box::new(HeldFocus {
                windows,
                fact: fact.clone(),
                observed_at_ms,
            })),
        );
    }

    /// The live element for the focus `runtime_id` names, read as the
    /// focused element within [`FOCUS_READ_WAIT`].
    fn live_focus_element(&mut self, runtime_id: &[i32]) -> LiveFocus {
        let Some(uia) = self.client.uia() else {
            return LiveFocus::Unresolved;
        };
        let Ok(cache) = self.context.uia_cache(uia) else {
            return LiveFocus::Unresolved;
        };
        let Ok(Ok(element)) = uia.within(FOCUS_READ_WAIT, |uia| uia.focused_element(&cache)) else {
            return LiveFocus::Unresolved;
        };
        // `element` was built with the base cache request.
        let (found, process) = (
            snapshot_parts_from_cached_element(&element).runtime_id,
            cached_process_id(&element),
        );
        if found == runtime_id {
            LiveFocus::Found(element)
        } else if process.is_some_and(|pid| pid != 0 && pid != self.context.target_pid) {
            LiveFocus::InAnotherApplication
        } else {
            LiveFocus::Elsewhere
        }
    }

    /// Queues a follow-up that finds the live element of the focus
    /// `runtime_id` names, reported from its event alone, for the
    /// focus-following property subscription.
    fn resolve_focus_later(&self, runtime_id: &[i32], trace: TraceId, attempt: u32) {
        self.resolve_focus_later_held(runtime_id, trace, attempt, None);
    }

    /// [`resolve_focus_later`](Self::resolve_focus_later), carrying a focus
    /// held back until its element is found focused.
    fn resolve_focus_later_held(
        &self,
        runtime_id: &[i32],
        trace: TraceId,
        attempt: u32,
        held: Option<Box<HeldFocus>>,
    ) {
        if attempt > FOCUS_RESOLVE_ATTEMPTS {
            if held.is_some() {
                tracing::debug!("a held focus never had the keyboard focus; it is dropped");
                return;
            }
            tracing::debug!("the focus's element was not found; its changes are not followed");
            return;
        }
        self.context.intake.push(Entry {
            item: Item::ResolveFocus {
                runtime_id: runtime_id.to_vec(),
                attempt,
                held,
            },
            trace,
            observed_at_ms: 0,
            timing: crate::protocol::EventTiming::default(),
        });
    }

    /// The follow-up [`resolve_focus_later`](Self::resolve_focus_later)
    /// queued: while the focus is still the one it names, reads the focused
    /// element with the full wait and, when it is that focus, keeps it for
    /// navigation and follows its changes; otherwise tries again later.
    fn resolve_focus(
        &mut self,
        runtime_id: &[i32],
        trace: TraceId,
        attempt: u32,
        held: Option<Box<HeldFocus>>,
    ) {
        let context = self.context;
        if context.intake.focused() != Some(Object::Uia(runtime_id.to_vec())) {
            return; // Focus has moved on.
        }
        if let Some(held) = held {
            // Reported now if its element has the keyboard focus after all,
            // read afresh as the event would have been.
            match self.live_focus_element(runtime_id) {
                LiveFocus::Found(_) => {
                    self.uia_focus(held.windows, &held.fact, trace, held.observed_at_ms);
                }
                LiveFocus::InAnotherApplication => {}
                LiveFocus::Elsewhere | LiveFocus::Unresolved => {
                    self.resolve_focus_later_held(runtime_id, trace, attempt + 1, Some(held));
                }
            }
            return;
        }
        let element = self.client.uia().and_then(|uia| {
            let cache = self.context.uia_cache(uia).ok()?;
            uia.focused_element(&cache).ok()
        });
        let Some(element) = element.filter(|element| {
            // Built with the base cache request.
            snapshot_parts_from_cached_element(element).runtime_id == runtime_id
        }) else {
            self.resolve_focus_later(runtime_id, trace, attempt + 1);
            return;
        };
        let id = context.uia_registry.id_for_element(runtime_id, &element);
        let followed: Vec<_> = context.uia_registry.element_of(id).into_iter().collect();
        if let Some(subscription) = context.focus_properties.get() {
            subscription.retarget(verbatim_uia::Scope::Elements(followed));
        }
        tracing::debug!("the focus's element was found; its changes are followed");
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
            if tracking.batch == Some(self.batch)
                && matches!(tracking.role, Some(Role::Menu | Role::MenuItem))
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
            (Some(Vec::new()), None),
        );
    }

    /// A UIA menu opening, which NVDA treats as a focus on the menu unless a
    /// focus is already pending: ignored when a focus was reported in the same
    /// batch, or when the window does not belong to UIA.
    fn uia_menu_opened(&mut self, fact_hwnd: isize, parts: &UiaSnapshotFact, entry: &Entry) {
        if self.context.tracking().batch == Some(self.batch) {
            return;
        }
        let hwnd = if fact_hwnd != 0 {
            Some(fact_hwnd)
        } else {
            focus_window_of(self.context.target_pid)
        };
        if hwnd.is_some_and(window_belongs_to_hidden_frame) {
            return;
        }
        if let Some(hwnd) = hwnd
            && !read::window_uses_uia(self.context, hwnd)
        {
            return;
        }
        let node = Self::uia_node(self.context, parts, None);
        self.emit_focus(
            entry.trace,
            entry.observed_at_ms,
            Backend::Uia,
            hwnd,
            node,
            false,
            Some(Object::Uia(parts.runtime_id.clone())),
            (Some(Vec::new()), None),
        );
    }

    /// A query from Core, answered with exactly one reply.
    fn query(&mut self, request_id: u64, query: &Query, trace: TraceId) {
        let context = self.context;
        let client = &mut *self.client;
        let started = std::time::Instant::now();
        let mut awaited = None;
        let result = match query {
            Query::FocusNow => {
                let mut answer = read::focus_now(context, client);
                let previous = context.tracking().chain.clone();
                let window = answer.window.iter_mut().map(|(window, _)| window);
                let focus = answer.focus.iter_mut().flat_map(|focus| {
                    let FocusedControl {
                        node, ancestors, ..
                    } = focus;
                    ancestors.iter_mut().chain(std::iter::once(node))
                });
                read::describe_dialogs(
                    context,
                    client,
                    window.chain(focus),
                    (&previous, &mut None, false),
                );
                Ok(QueryResult::Focus(answer))
            }
            Query::Navigate { node_id, kind } => {
                read::navigate(context, client, *node_id, *kind).map(QueryResult::Navigated)
            }
            Query::Activate { node_id } => {
                read::activate(context, client, *node_id).map(QueryResult::Activated)
            }
            Query::Ancestors { node_id } => {
                read::ancestors(context, client, *node_id).map(QueryResult::Ancestors)
            }
            Query::DumpTree => read::dump_tree(context, client).map(QueryResult::Tree),
            Query::Text { node_id, op } => {
                let (reply, wait) = text_reads::answer(context, *node_id, op);
                awaited = wait;
                Ok(QueryResult::Text(reply))
            }
        };
        let outcome = match result {
            Ok(result) => QueryOutcome::Done(result),
            Err(ReadError::Gone) => QueryOutcome::Gone,
            Err(ReadError::Failed(reason)) => QueryOutcome::Failed(reason),
        };
        tracing::debug!(
            ?query,
            ?outcome,
            elapsed_ms = started.elapsed().as_millis(),
            "query answered"
        );
        publish(
            context,
            self.generation,
            OutpostToSupervisor::Reply {
                trace_id: trace,
                request_id,
                outcome,
                timing: EventTiming {
                    published_at_us: now_us(),
                    awaited_at_us: awaited.map_or(0, |awaited| awaited.at_us),
                    awaited_calls: awaited.map(|awaited| awaited.calls).unwrap_or_default(),
                    calls: awaited.map(|awaited| awaited.calls).unwrap_or_default() + take_calls(),
                    ..self.timing
                },
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_terminals_output_notifications_are_ignored() {
        let notification = |activity: &str| verbatim_model::Notification {
            kind: verbatim_model::NotificationKind::ActionCompleted,
            processing: verbatim_model::NotificationProcessing::All,
            display_string: Some("hello".to_owned()),
            activity_id: Some(activity.to_owned()),
        };
        assert!(is_terminal_output_notification(
            Role::Terminal,
            &notification("TerminalTextOutput")
        ));
        // Another of a terminal's notifications, or another control's with
        // the same activity id, is spoken as usual.
        assert!(!is_terminal_output_notification(
            Role::Terminal,
            &notification("Windows.Shell.SnapComponent.SnapHotKeyResults")
        ));
        assert!(!is_terminal_output_notification(
            Role::EditableText,
            &notification("TerminalTextOutput")
        ));
    }

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
    fn an_abandoned_query_gets_only_the_abandoned_reply() {
        let watch = Watch::default();
        let trace = TraceId::mint();
        let mut replies = Vec::new();
        watch.start(STEP_DEADLINE, Some((7, trace)), 0);

        // The watchdog abandons the worker and answers its query.
        if let Some((request_id, _)) = watch.lock().abandon() {
            replies.push((request_id, "abandoned"));
        }
        // The old worker's call returns and it tries to publish its answer.
        let published = watch.publish(0, |_| replies.push((7, "done")));

        assert!(!published, "an abandoned worker publishes nothing");
        assert_eq!(replies, [(7, "abandoned")]);
    }

    #[test]
    fn a_published_query_can_no_longer_be_abandoned() {
        let watch = Watch::default();
        let mut replies = Vec::new();
        watch.start(STEP_DEADLINE, Some((7, TraceId::mint())), 0);

        assert!(watch.publish(0, |_| replies.push((7, "done"))));
        // A watchdog that wakes just after finds no query to answer.
        if let Some((request_id, _)) = watch.lock().abandon() {
            replies.push((request_id, "abandoned"));
        }

        assert_eq!(replies, [(7, "done")]);
    }

    #[test]
    fn a_worker_is_abandoned_once_the_user_moves_on_after_the_grace() {
        const WINDOW: isize = 0x1234;
        let started = Instant::now();
        let deadline = started + HANDLING_DEADLINE;
        let in_grace = started + MOVED_ON_GRACE / 2;
        let after_grace = started + MOVED_ON_GRACE;

        assert_eq!(
            abandon_reason(in_grace, deadline, started, WINDOW, |_| true),
            None,
            "within the grace the worker is left alone even when the user moved on"
        );
        assert_eq!(
            next_check(in_grace, deadline, started, WINDOW),
            after_grace,
            "and is checked again when the grace ends"
        );
        assert_eq!(
            abandon_reason(after_grace, deadline, started, WINDOW, |window| {
                window == WINDOW
            }),
            Some("the user moved on from a slow window")
        );
        assert_eq!(
            abandon_reason(after_grace, deadline, started, WINDOW, |_| false),
            None,
            "a user still on the slow window waits for the deadline"
        );
        assert_eq!(
            next_check(after_grace, deadline, started, WINDOW),
            after_grace + MOVED_ON_GRACE,
            "and is checked again every grace period"
        );
    }

    #[test]
    fn an_entry_with_no_window_waits_for_its_deadline() {
        let started = Instant::now();
        let deadline = started + HANDLING_DEADLINE;
        let later = started + MOVED_ON_GRACE * 4;
        assert_eq!(abandon_reason(later, deadline, started, 0, |_| true), None);
        assert_eq!(next_check(later, deadline, started, 0), deadline);
        assert_eq!(
            abandon_reason(deadline, deadline, started, 0, |_| false),
            Some("a call passed its deadline")
        );
        assert_eq!(
            next_check(
                started + Duration::from_millis(9_900),
                deadline,
                started,
                0x1234
            ),
            deadline,
            "a check never falls after the deadline"
        );
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
