//! The worker and its watchdog (outpost redesign, "Inside an outpost").
//!
//! One worker thread takes entries from the intake queue in order and
//! finishes each before starting the next. It is the only thread that calls
//! into the application, so events and replies leave the outpost in the order
//! the intake plans their entries: the order they joined the queue, but for
//! a batch's menu openings, handled last, and the older focus events of a
//! run of MSAA focus events, tried after its newest. This is NVDA's model,
//! one thread doing all the work, with one such thread per application. The
//! worker waits for nothing but its calls into the application and the
//! queue: a caret key's evidence is watched for between entries.
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

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use verbatim_ia2::acquire::Purpose;
use verbatim_ia2::{CHILDID_SELF, WinEventKind};
use verbatim_model::{
    Backend, CallCounts, CaretWatch, NodeId, NodeSnapshot, NormalizedEvent, PropertyChange, Role,
    State, StateSet, TerminalOutput, TextOp, TextReply, TraceId,
};
use verbatim_uia::map::{snapshot_from_cached_element, snapshot_parts_from_cached_element};
use verbatim_uia::nearest_window_handle;
use windows::Win32::UI::Accessibility::{
    UIA_NamePropertyId, UIA_RangeValueValuePropertyId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use crate::protocol::{
    DeliveredFact, EventTiming, FocusedControl, OutpostToSupervisor, Query, QueryOutcome,
    QueryResult, UiaFocusRead, UiaSnapshotFact, now_us,
};

use super::Context;
use super::acceptance::{self, EventName, SystemWindows};
use super::focus_path::{self, FocusPath};
use super::intake::{Entry, Item, Object, Planned, UiaEvent, UiaKind, window_of};
use super::read::{self, Client, ReadError};
use super::text_reads::{self, CARET_WATCH_BOUND, OpenWatch};
use super::window::{
    console_client, event_window_unreadable, focus_window_of, foreground_window_handle,
    front_is_another_thread_of_its_application, is_shell_transition_window, now_ms, top_level_of,
    window_belongs_to_hidden_frame, window_facts, window_is_hidden_frame, window_is_visible,
};
use crate::arbitration::window_class_name;
use crate::terminal::keys;
use crate::terminal::reading::{Action, Event};
use crate::terminal::{Found, Memory, ReadMode};
use crate::text::Watched;
use windows::Win32::UI::Accessibility::IUIAutomationElement;

/// The deadline for handling an event, a focus, or a focus-now query: NVDA's
/// `NORMAL_CORE_ALIVE_TIMEOUT` (`watchdog.py`), the time NVDA waits for an
/// application that is slow to answer before cancelling the call. An
/// application that is starting up, or the Start menu's search window as it
/// opens, can take two or three seconds to answer a read that then succeeds
/// (found live in the end-to-end suite); NVDA announces the focus late, and
/// a shorter deadline here dropped it for good.
const HANDLING_DEADLINE: Duration = Duration::from_secs(10);

/// How long the watchdog waits on an entry before abandoning it because the
/// user has moved on to a window of the same application on another UI
/// thread: NVDA's `MIN_CORE_ALIVE_TIMEOUT`, after which NVDA cancels a slow
/// call once the foreground has changed. One application can own windows on
/// several threads (Explorer's folder windows, taskbar, and Alt+Tab
/// switcher), and those must not wait behind a slow window the user has
/// left. Windows of other applications have their own outposts and never
/// wait behind this one. The check is made when the grace ends and again
/// each time a foreground fact reaches the outpost, until the deadline:
/// the foreground event of a window of this application, which Windows
/// raises once the window is in front, is the evidence that the user has
/// moved to it.
const MOVED_ON_GRACE: Duration = Duration::from_millis(500);

/// The deadline for one navigation step or an activation.
const STEP_DEADLINE: Duration = Duration::from_millis(400);

/// The deadline for an ancestor walk or a tree dump, each up to 64 hops or
/// 4096 nodes.
const WALK_DEADLINE: Duration = Duration::from_secs(5);

/// The deadline for a text request: one read, or a few, of the node's text;
/// a caret key's watch is checked once within it and then kept open
/// outside it.
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
    /// Whether the outpost is shutting down: the watchdog abandons no
    /// worker from then on, since the shutdown waits for the call in
    /// progress to finish, and it ends.
    stopping: bool,
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
}

/// Shared between the worker, its watchdog, and anything that publishes.
#[derive(Default)]
pub(super) struct Watch {
    state: Mutex<WatchState>,
    changed: Condvar,
    /// Abandoned workers that have not yet returned. An atomic, so the reader
    /// answers a ping without waiting on the watch lock.
    abandoned: AtomicUsize,
    /// Every worker thread started, the one in charge and the abandoned
    /// ones alike, for the shutdown to wait for.
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
    /// The watchdog's thread.
    watchdog: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Watch {
    fn lock(&self) -> std::sync::MutexGuard<'_, WatchState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many abandoned workers have not yet returned.
    pub(super) fn abandoned(&self) -> usize {
        self.abandoned.load(Ordering::Relaxed)
    }

    /// Wakes the watchdog to judge again whether the user has moved on from
    /// the worker's window: a foreground fact reached the outpost, which
    /// Windows raises once the change is made (`phase6-design.md`,
    /// "Foreground events against the foreground window"). The watch lock
    /// is taken first, so the watchdog is waiting when it is told and the
    /// wake is never lost.
    pub(super) fn foreground_changed(&self) {
        let state = self.lock();
        self.changed.notify_one();
        drop(state);
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

    /// Claims the right to publish an entry's result for `generation`, if
    /// it is still the worker in charge, ending its entry and running
    /// `claim` under the watch lock; `None` if it is not. The watchdog
    /// abandons under the same lock, so an entry is either published or
    /// abandoned, never both. What is claimed is sent after the lock is
    /// released, and the entry has no deadline left to pass meanwhile.
    fn publish<T>(&self, generation: u64, claim: impl FnOnce() -> T) -> Option<T> {
        let mut state = self.lock();
        if state.generation != generation {
            return None;
        }
        state.deadline = None;
        state.started = None;
        state.running = None;
        Some(claim())
    }

    /// Claims the right to publish the answer to query `request_id` given
    /// outside that query's own entry (a caret key's watch), as
    /// [`publish`](Self::publish) does, but without ending the entry in
    /// hand. The query is no longer the one an abandonment answers, if it
    /// was, so it gets this answer and no other.
    fn publish_aside<T>(
        &self,
        generation: u64,
        request_id: u64,
        claim: impl FnOnce() -> T,
    ) -> Option<T> {
        let mut state = self.lock();
        if state.generation != generation {
            return None;
        }
        if state
            .running
            .is_some_and(|(running, _)| running == request_id)
        {
            state.running = None;
        }
        Some(claim())
    }

    /// Makes `query` the one an abandonment of `generation` answers, while
    /// the entry in hand, an event, reads for it (a caret key's watch).
    /// False when `generation` is no longer in charge.
    fn adopt(&self, generation: u64, query: (u64, TraceId)) -> bool {
        let mut state = self.lock();
        if state.generation != generation {
            return false;
        }
        state.running = Some(query);
        true
    }

    /// The query `generation` is handling will be answered later, outside
    /// its entry (a caret key's watch kept open), so an abandonment no
    /// longer answers it and the entry's end does not either. False when
    /// `generation` is no longer in charge: an abandonment has answered it.
    fn release_query(&self, generation: u64) -> bool {
        let mut state = self.lock();
        if state.generation != generation {
            return false;
        }
        state.running = None;
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
    /// or recognized as the focus already, so the worker can tell when a
    /// focus candidate was settled.
    reported: u64,
    /// The node of the last focus this outpost reported.
    focus: Option<NodeId>,
    /// That focus's states, as last read, so a state change tells whether
    /// it newly expanded the focus.
    focus_states: Option<StateSet>,
    /// That focus's role, as last read, which decides how a change of one
    /// of its properties is read.
    focus_role: Option<Role>,
    /// Whether a foreground change was reported after the last focus: the
    /// focus may then have been elsewhere meanwhile, so a focus event that
    /// repeats it is not a duplicate.
    foreground_since_focus: bool,
    /// The dialog the last foreground report gathered text for, which the
    /// focus that follows it reuses.
    dialog: read::DialogMemo,
    /// The last focus's top-level window, when it was the foreground
    /// window as the focus entered it: the window NVDA holds the
    /// foreground object for ([`Worker::is_foreground_window`]).
    foreground_window: Option<isize>,
    /// A foreground change whose window was the foreground window but not
    /// yet shown when it was handled, with its trace and the time it was
    /// observed: reported once the window is shown, or just before a focus
    /// inside it, whichever comes first ([`Worker::foreground`]).
    unshown_foreground: Option<(isize, TraceId, u64)>,
}

impl Tracking {
    /// The window of the last focus this outpost reported.
    pub(super) fn window(&self) -> Option<isize> {
        self.window
    }
}

/// Publishes `message`, an entry's one result, if `generation` is still the
/// worker in charge. The check and the end of the entry's deadline happen
/// under the watch lock, so an abandoned worker can never publish after its
/// abandonment, and a published entry can no longer be abandoned: a query
/// never gets a second reply. The message is queued after the lock is
/// released, and queuing never waits (`outbound`).
///
/// Every node issued or looked up since the last publish is taken under the
/// same lock, while no other worker can be touching nodes, and recorded as
/// reported at the position the message takes in the queue. That may
/// include nodes the message does not carry, which are then only kept a
/// little longer.
fn publish(context: &Context, generation: u64, message: OutpostToSupervisor) -> bool {
    let Some(touched) = context.watch.publish(generation, || take_touched(context)) else {
        return false;
    };
    context.outbound.publish(message, touched);
    true
}

/// Publishes `message`, the answer to query `request_id` given outside
/// that query's own entry, as [`publish`] does, but without ending the entry
/// in hand: a caret key's watch, answered or ended while the worker handles
/// an event, a focus, or the next key.
fn publish_aside(
    context: &Context,
    generation: u64,
    request_id: u64,
    message: OutpostToSupervisor,
) -> bool {
    let Some(touched) = context
        .watch
        .publish_aside(generation, request_id, || take_touched(context))
    else {
        return false;
    };
    context.outbound.publish(message, touched);
    true
}

/// The numbers of the nodes issued or looked up since the last take.
fn take_touched(context: &Context) -> Vec<u64> {
    context
        .uia_registry
        .take_touched()
        .into_iter()
        .chain(context.msaa_registry.take_touched())
        .map(NodeId::number)
        .collect()
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
    let watched = Arc::clone(context);
    let watchdog = thread::Builder::new()
        .name("verbatim-watchdog".to_owned())
        .spawn(move || watchdog(&watched))
        .expect("spawn the watchdog");
    *context
        .watch
        .watchdog
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(watchdog);
}

fn spawn_worker(context: Arc<Context>, generation: u64) {
    let watch = Arc::clone(&context);
    let worker = thread::Builder::new()
        .name("verbatim-worker".to_owned())
        .spawn(move || run(&context, generation))
        .expect("spawn a worker");
    let mut workers = watch
        .watch
        .workers
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // Workers that have returned are forgotten, so the list holds at most
    // the one in charge and the abandoned ones still in their calls.
    workers.retain(|worker| !worker.is_finished());
    workers.push(worker);
}

/// Stops the worker for the outpost's shutdown, once the intake is closed:
/// the watchdog ends without abandoning anything more, and every worker
/// thread is waited for, the one in charge finishing the entry in hand
/// and each abandoned one returning from its call, however long UIA takes
/// to end a call the application does not answer. Returns how many worker
/// threads were waited for.
pub(super) fn stop(context: &Context) -> usize {
    context.watch.lock().stopping = true;
    context.watch.changed.notify_all();
    let watchdog = context
        .watch
        .watchdog
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(watchdog) = watchdog {
        let _ = watchdog.join();
    }
    // The watchdog has ended, so no worker is started from here on.
    let workers: Vec<thread::JoinHandle<()>> = std::mem::take(
        &mut *context
            .watch
            .workers
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
    );
    let count = workers.len();
    for worker in workers {
        let _ = worker.join();
    }
    count
}

/// The on-demand reading event a terminal text request is, `None` for any
/// other request.
fn terminal_event_of(op: &TextOp) -> Option<Event> {
    match op {
        TextOp::TerminalHold => Some(Event::Hold),
        TextOp::TerminalRead { hold } => Some(Event::Read { hold: *hold }),
        TextOp::TerminalCancel => Some(Event::Cancel),
        _ => None,
    }
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

/// When the watchdog next checks a worker it did not abandon at `now`, if
/// no foreground fact wakes it first ([`Watch::foreground_changed`]): at
/// the end of the grace for an entry concerning a window, while the grace
/// lasts, and otherwise at the deadline.
fn next_check(now: Instant, deadline: Instant, started: Instant, window: isize) -> Instant {
    let check = if window != 0 && now < started + MOVED_ON_GRACE {
        started + MOVED_ON_GRACE
    } else {
        deadline
    };
    check.min(deadline)
}

/// The watchdog: waits for the worker's deadline and abandons a worker that
/// passes it, or that has waited [`MOVED_ON_GRACE`] on a window the user has
/// left for one of the same application on another UI thread, judged when
/// the grace ends and whenever a foreground fact reaches the outpost.
fn watchdog(context: &Arc<Context>) {
    let mut state = context.watch.lock();
    loop {
        if state.stopping {
            return;
        }
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
        // Nothing is sent while the lock is held. The abandoned worker can
        // no longer publish, and its replacement is not started yet, so the
        // answer still comes before anything the replacement says.
        drop(state);
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
        state = context.watch.lock();
    }
}

/// The worker's thread: its loop, then the release of the UIA objects
/// `verbatim-uia` kept for this thread, before the thread exits rather than
/// from its thread-local destructors under the loader lock.
fn run(context: &Context, generation: u64) {
    // Held objects are agile references, resolved in this apartment.
    let joined = verbatim_uia::init_mta();
    if let Err(error) = &joined {
        tracing::warn!(%error, "the worker could not join the multithreaded apartment");
    }
    run_loop(context, generation);
    verbatim_uia::release_thread_state();
    if joined.is_ok() {
        verbatim_uia::leave_mta();
    }
}

/// The worker's loop, until the intake closes or this worker is abandoned.
fn run_loop(context: &Context, generation: u64) {
    let mut client = Client::default();
    while let Some((planned, batch)) = context.intake.next() {
        let mut run = |entry: Entry, menu: bool| {
            run_entry(context, &mut client, generation, batch, entry, menu)
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

/// Handles one entry of `batch` under its deadline. `Err` when this worker
/// was abandoned meanwhile and must exit.
fn run_entry(
    context: &Context,
    client: &mut Client,
    generation: u64,
    batch: u64,
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
    let arbitration_started = context.arbitrator().now();
    let handled = catch_unwind(AssertUnwindSafe(|| {
        let mut worker = Worker {
            context,
            client,
            generation,
            batch,
            timing,
        };
        if menu {
            worker.menu_opened(&entry);
        } else {
            worker.handle(entry);
        }
    }));
    {
        let mut arbitrator = context.arbitrator();
        let finished = arbitrator.now();
        arbitrator.renew_probes_since(arbitration_started, finished);
    }
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
        Item::Fact(..)
        | Item::Msaa { .. }
        | Item::Uia(_)
        | Item::CaretOf { .. }
        | Item::Settle(_)
        | Item::Wake => (HANDLING_DEADLINE, None),
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
            thread,
        } => {
            format!("MSAA {kind:?} hwnd={hwnd} object={id_object} child={id_child} thread={thread}")
        }
        Item::Uia(event) => format!("UIA {:?}", event.kind),
        Item::Fact(DeliveredFact::UiaFocusRead { marker, .. }, _) => {
            format!("UIA focus read {marker}")
        }
        Item::Fact(fact, _) => format!("fact {:?}", fact.key()),
        Item::Query { query, .. } => format!("query {query:?}"),
        Item::NodesHeld { nodes, .. } => format!("nodes held ({})", nodes.len()),
        Item::CaretOf { node_id } => format!("caret of {node_id:?}"),
        Item::Settle(_) => "settle".to_owned(),
        Item::Wake => "wake".to_owned(),
    }
}

/// The name NVDA's acceptance test knows a UIA event of `kind` by, or `None`
/// for one it does not test: a notification, and a text focus's caret,
/// text, and active text position, which are Verbatim's own events of the
/// focus (`docs/parity.md`, "Event acceptance in the outpost").
fn uia_event_name(kind: &UiaKind) -> Option<EventName> {
    match kind {
        UiaKind::Property(id) if *id == UIA_NamePropertyId.0 => Some(EventName::NameChange),
        UiaKind::Property(id)
            if *id == UIA_ValueValuePropertyId.0 || *id == UIA_RangeValueValuePropertyId.0 =>
        {
            Some(EventName::ValueChange)
        }
        UiaKind::Property(_) => Some(EventName::StateChange),
        UiaKind::Selection => Some(EventName::Selection),
        UiaKind::Notification(_)
        | UiaKind::TextSelection
        | UiaKind::TextChanged
        | UiaKind::ActiveTextPosition(_) => None,
    }
}

/// A focus that is not reported, for the reason logged where it was found.
struct Dropped;

/// One entry's handling, by the worker in charge.
struct Worker<'a> {
    context: &'a Context,
    client: &'a mut Client,
    generation: u64,
    /// The batch the entry belongs to.
    batch: u64,
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
                ..
            } => self.msaa_event(kind, hwnd, id_object, id_child, trace, observed_at_ms),
            Item::Uia(event) => self.uia_event(event, trace, observed_at_ms),
            Item::Fact(fact, _) => self.fact(fact, trace, observed_at_ms),
            Item::Query { request_id, query } => self.query(request_id, &query, trace),
            Item::NodesHeld {
                nodes,
                acknowledged,
            } => self.release(&nodes, acknowledged),
            Item::CaretOf { node_id } => self.caret_of(node_id, trace, observed_at_ms, true),
            Item::Settle(done) => self.settle(done, trace),
            Item::Wake => self.wake(),
        }
    }

    /// Handles a caret key's watch for evidence (`TextOp::AwaitCaret`):
    /// ends the watch it replaces, then checks the new one with one read
    /// and answers it when the application has already done what the key
    /// asked. Otherwise the watch stays open, and the worker returns to its
    /// queue: nothing else waits for the key's evidence, which a caret,
    /// text, or selection change brings later ([`check_open_watch`]).
    ///
    /// [`check_open_watch`]: Self::check_open_watch
    fn caret_key(&mut self, request_id: u64, trace: TraceId, node_id: NodeId, watch: &CaretWatch) {
        self.end_watch("the next caret key replaced it");
        let mut watched = text_reads::check_watch(self.context, node_id, watch, false);
        if let Watched::Answered(reply) = &mut watched
            && self.wrapped_removal(node_id, watch, reply, trace)
        {
            watched = Watched::Watching;
        }
        match watched {
            Watched::Answered(reply) => self.reply(
                request_id,
                trace,
                QueryOutcome::Done(QueryResult::Text(reply)),
            ),
            Watched::Watching => {
                let calls = take_calls();
                // Under the watch lock: an abandonment has either answered
                // the request already or can no longer answer it.
                if !self.context.watch.release_query(self.generation) {
                    return;
                }
                let opened = Instant::now();
                *self.context.caret_watch() = Some(OpenWatch {
                    request_id,
                    trace,
                    node_id,
                    watch: watch.clone(),
                    opened,
                    timing: self.timing,
                    calls,
                });
                self.context
                    .intake
                    .wake_at(Some(opened + CARET_WATCH_BOUND));
                tracing::debug!(%trace, ?calls, "a caret key's watch is open");
            }
        }
    }

    /// Checks the open caret key's watch on `node_id`, if there is one, as
    /// the application reports a caret change (`caret_event`), a text
    /// change, or a selection change in it: answers the key once the
    /// evidence is there, and otherwise keeps the watch open. Returns
    /// whether it answered. An event observed before the caret was last
    /// read shows nothing that read did not see, and checks nothing.
    fn check_open_watch(&mut self, node_id: NodeId, caret_event: bool) -> bool {
        if self
            .context
            .caret_read_since(node_id, self.timing.observed_at_us)
        {
            return false;
        }
        self.check_watch_now(node_id, caret_event)
    }

    /// Checks the open caret key's watch on `node_id`, if there is one, as
    /// [`check_open_watch`](Self::check_open_watch) does, whenever the caret
    /// was last read: after a terminal's screen read, which reads its caret
    /// too, so the change that caused it has nothing more to show the watch,
    /// and a terminal that moves its caret after its text raises no event
    /// for it (Windows Terminal, `PSReadLine`'s menu).
    fn check_watch_now(&mut self, node_id: NodeId, caret_event: bool) -> bool {
        let context = self.context;
        let Some(mut open) = context
            .caret_watch()
            .take_if(|open| open.node_id == node_id)
        else {
            return false;
        };
        // While the caret is read, an abandonment answers the key.
        if !context
            .watch
            .adopt(self.generation, (open.request_id, open.trace))
        {
            *context.caret_watch() = Some(open);
            return false;
        }
        let mut watched = text_reads::check_watch(context, node_id, &open.watch, caret_event);
        if let Watched::Answered(reply) = &mut watched
            && self.wrapped_removal(node_id, &open.watch, reply, open.trace)
        {
            watched = Watched::Watching;
        }
        match watched {
            Watched::Watching => {
                open.calls += take_calls();
                if context.watch.release_query(self.generation) {
                    *context.caret_watch() = Some(open);
                }
                false
            }
            Watched::Answered(reply) => {
                context.intake.wake_at(None);
                let calls = take_calls();
                tracing::debug!(
                    trace = %open.trace,
                    waited_ms = open.opened.elapsed().as_millis(),
                    "a caret key's watch is answered"
                );
                publish_aside(
                    context,
                    self.generation,
                    open.request_id,
                    OutpostToSupervisor::Reply {
                        trace_id: open.trace,
                        request_id: open.request_id,
                        outcome: QueryOutcome::Done(QueryResult::Text(reply)),
                        timing: EventTiming {
                            // The watch waited until the evidence was taken
                            // from the queue; the read after it is the
                            // answer's.
                            awaited_at_us: self.timing.dequeued_at_us,
                            awaited_calls: open.calls,
                            calls: open.calls + calls,
                            published_at_us: now_us(),
                            ..open.timing
                        },
                    },
                );
                true
            }
        }
    }

    /// The node of the open caret key's watch, when it is the client area
    /// of the edit control `hwnd`.
    fn watched_edit(&self, hwnd: isize) -> Option<NodeId> {
        let node_id = self.context.caret_watch().as_ref()?.node_id;
        (self.context.msaa_registry.key_of(node_id) == Some((hwnd, OBJID_CLIENT.0, CHILDID_SELF)))
            .then_some(node_id)
    }

    /// Ends the open caret key's watch, if there is one, without evidence:
    /// answered `WatchEnded`, for which Core says nothing. `why` is logged.
    fn end_watch(&mut self, why: &str) {
        let Some(open) = self.context.caret_watch().take() else {
            return;
        };
        self.context.intake.wake_at(None);
        let published = publish_aside(
            self.context,
            self.generation,
            open.request_id,
            OutpostToSupervisor::Reply {
                trace_id: open.trace,
                request_id: open.request_id,
                outcome: QueryOutcome::Done(QueryResult::Text(TextReply::WatchEnded)),
                timing: EventTiming {
                    calls: open.calls,
                    published_at_us: now_us(),
                    ..open.timing
                },
            },
        );
        if published {
            tracing::debug!(
                trace = %open.trace,
                why,
                waited_ms = open.opened.elapsed().as_millis(),
                "a caret key's watch ended without evidence"
            );
        } else {
            // Abandoned: the worker that replaces this one ends it.
            *self.context.caret_watch() = Some(open);
        }
    }

    /// [`Item::Wake`]: ends the open caret key's watch once it has reached
    /// [`CARET_WATCH_BOUND`], which only frees it and says nothing.
    fn wake(&mut self) {
        let bound = self
            .context
            .caret_watch()
            .as_ref()
            .map(|open| open.opened + CARET_WATCH_BOUND);
        match bound {
            Some(bound) if Instant::now() >= bound => self.end_watch("its bound passed"),
            Some(bound) => self.context.intake.wake_at(Some(bound)),
            None => {}
        }
    }

    /// Answers [`Outpost::settle`](super::Outpost::settle) once nothing
    /// else is waiting, the follow-ups the entries before it queued
    /// included: otherwise it goes to the back of the queue again.
    fn settle(&self, done: std::sync::mpsc::Sender<()>, trace: TraceId) {
        let context = self.context;
        if context.intake.busy() {
            context.intake.push(Entry {
                item: Item::Settle(done),
                trace,
                observed_at_ms: now_ms(),
                timing: EventTiming::default(),
            });
            return;
        }
        if let Some(registration) = context.focus_events.get() {
            registration.settle();
        }
        context.outbound.flush();
        let _ = done.send(());
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
    /// no text to read is reported as `NoText`, and Core speaks its value
    /// instead of a line. A focus whose text could not be read now reports
    /// nothing: Core speaks its line when its caret is next reported, as a
    /// caret event of the application's says the text can be read, and
    /// never its value, which for a document is all of its text, as NVDA
    /// never speaks the value of an object with text.
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
        let started = std::time::Instant::now();
        let report = text_reads::report_caret(self.context, node_id, after_focus);
        // A caret event's read, which a terminal's output causes too, as
        // its cursor moves.
        tracing::debug!(
            elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            after_focus,
            "caret read timing"
        );
        let event = match report {
            Ok(caret) => NormalizedEvent::CaretMoved { node_id, caret },
            Err(text_reads::NoCaret::NoText) if after_focus => NormalizedEvent::NoText { node_id },
            Err(text_reads::NoCaret::NotRead) if after_focus => {
                tracing::debug!(
                    "the focus's caret could not be read; its line is spoken when its caret is next reported"
                );
                return;
            }
            Err(_) => return,
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
            self.terminal_event(node_id, Event::Focused, None, (trace, observed_at_ms));
        }
    }

    /// Whether `node_id` is the focus and a terminal read through UIA, whose
    /// new output the outpost finds by diffing its text.
    fn focused_terminal(&self, node_id: NodeId) -> bool {
        self.context.uia_registry.runtime_id_of(node_id).is_some()
            && self.is_focus(node_id)
            && self.context.tracking().role == Some(Role::Terminal)
    }

    /// For `reply`, the answer to a key judged by where the caret landed
    /// (`watch`) in the focused terminal `node_id`, whose caret left its
    /// row: reads the terminal, and adds the text removed when the key
    /// removed a typed line that had wrapped onto more rows
    /// ([`keys::wrapped_removal`]). Returns whether that removal is still
    /// under way, the watch then staying open.
    fn wrapped_removal(
        &mut self,
        node_id: NodeId,
        watch: &CaretWatch,
        reply: &mut TextReply,
        trace: TraceId,
    ) -> bool {
        let TextReply::Caret(reply) = reply else {
            return false;
        };
        if !watch.landing || reply.same_line != Some(false) || !self.focused_terminal(node_id) {
            return false;
        }
        let before = self
            .context
            .terminals()
            .get(&node_id.number())
            .and_then(|terminal| terminal.screen_at(watch.pressed_at_ms).cloned());
        let Some(before) = before else {
            tracing::debug!("no read of the terminal kept ended before the key");
            return false;
        };
        self.terminal_event(node_id, Event::TextChanged, None, (trace, now_ms()));
        let now = self
            .context
            .terminals()
            .get(&node_id.number())
            .and_then(|terminal| terminal.memory().cloned());
        let Some(now) = now else {
            return false;
        };
        let removal = keys::wrapped_removal(&before, &now);
        tracing::debug!(?removal, "a landing key's answer in a terminal");
        match removal {
            Some(keys::WrappedRemoval::Removed(text)) => {
                reply.removed = Some(text);
                false
            }
            Some(keys::WrappedRemoval::UnderWay) => true,
            None => false,
        }
    }

    /// Takes a focused terminal's on-demand reading through `event`
    /// ([`crate::terminal::reading`]) and does what the transition says:
    /// reads and reports what is new, answers Core's request (`request`, its
    /// id and trace, asked now, or the one owed from an earlier read the
    /// terminal disturbed), or only notes the change. An answer the
    /// transition no longer owes is given, empty.
    #[expect(
        clippy::too_many_lines,
        reason = "one transition and the read it calls for, top to bottom"
    )]
    fn terminal_event(
        &mut self,
        node_id: NodeId,
        event: Event,
        request: Option<(u64, TraceId)>,
        (trace, observed_at_ms): (TraceId, u64),
    ) {
        let (action, owed, superseded) = {
            let mut terminals = self.context.terminals();
            let terminal = terminals.entry(node_id.number()).or_default();
            let (reading, action) = terminal.reading.next(event);
            terminal.reading = reading;
            let mut owed = terminal.owed.take();
            // A request asked now takes the place of one owed.
            let superseded = request.and_then(|request| owed.replace(request));
            (action, owed, superseded)
        };
        if let Some(earlier) = superseded {
            self.reply_terminal(earlier, TerminalOutput::default());
        }
        let mode = match action {
            Action::Nothing => {
                if owed.is_some() {
                    self.context
                        .terminals()
                        .entry(node_id.number())
                        .or_default()
                        .owed = owed;
                }
                return;
            }
            Action::ReplyEmpty => {
                if let Some(request) = owed {
                    self.reply_terminal(request, TerminalOutput::default());
                }
                return;
            }
            Action::Baseline => {
                if let Some(request) = owed {
                    self.reply_terminal(request, TerminalOutput::default());
                }
                ReadMode::Baseline
            }
            Action::Report | Action::Reply => ReadMode::Change,
            Action::ReplySilently => ReadMode::Cancel,
        };
        let replies = matches!(action, Action::Reply | Action::ReplySilently);
        let owed = owed.filter(|_| replies);
        let Some(uia) = self.client.uia() else {
            if let Some(request) = owed {
                self.reply_terminal(request, TerminalOutput::default());
            }
            return;
        };
        let started = std::time::Instant::now();
        let read = text_reads::terminal_output(self.context, uia, node_id, mode);
        // Each read, end to end, and how long its event waited in the
        // queue: during a flood the reads follow one another, so these say
        // how much of the time the outpost spends in the terminal.
        tracing::debug!(
            ?event,
            ?action,
            queued_us = self
                .timing
                .dequeued_at_us
                .saturating_sub(self.timing.observed_at_us),
            elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            found = ?read.as_ref().map(|(found, _)| found),
            "terminal read timing"
        );
        let window = self.context.tracking().window;
        let (found, caret) = match read {
            Some(read) => read,
            // Gone or failed: nothing to say, and Core's request is
            // answered empty.
            None => (
                Found::Output(TerminalOutput::default(), Memory::default()),
                None,
            ),
        };
        // The caret read with the text: the console host raises no caret
        // event for every character typed, so this keeps Core's copy of it
        // following typing. A caret key's watch is checked first, as the
        // caret may have moved since the change that caused the read.
        if let Some(caret) = caret {
            self.check_watch_now(node_id, false);
            self.emit(
                trace,
                observed_at_ms,
                Backend::Uia,
                window,
                NormalizedEvent::CaretMoved { node_id, caret },
            );
        }
        match found {
            Found::Output(output, _) => {
                if let Some(request) = owed {
                    // An answer too large for one message carries its
                    // first part, and the rest follows as output.
                    let mut parts =
                        crate::terminal::split(output, crate::terminal::MESSAGE_TEXT_BUDGET)
                            .into_iter();
                    self.reply_terminal(request, parts.next().unwrap_or_default());
                    for output in parts {
                        self.emit(
                            trace,
                            observed_at_ms,
                            Backend::Uia,
                            window,
                            NormalizedEvent::TerminalOutput { node_id, output },
                        );
                    }
                } else if action == Action::Report && !output.is_empty() {
                    self.emit(
                        trace,
                        observed_at_ms,
                        Backend::Uia,
                        window,
                        NormalizedEvent::TerminalOutput { node_id, output },
                    );
                }
            }
            Found::Unsettled if owed.is_some() => {
                // The writing that disturbed it raises a change, which
                // reads again and answers.
                let mut terminals = self.context.terminals();
                let terminal = terminals.entry(node_id.number()).or_default();
                terminal.reading = terminal.reading.next(Event::ReplyUnsettled).0;
                terminal.owed = owed;
            }
            // A report the terminal wrote to while it was read: the change
            // that disturbed it raises another.
            Found::Unsettled => {}
        }
    }

    /// Answers Core's terminal request `request` with `output`.
    fn reply_terminal(&self, (request_id, trace): (u64, TraceId), output: TerminalOutput) {
        self.reply(
            request_id,
            trace,
            QueryOutcome::Done(QueryResult::Text(TextReply::Terminal(Box::new(output)))),
        );
    }

    /// A focused terminal's text changed: read at once or noted, as its
    /// on-demand reading stands.
    /// A change observed before the last read that settled began is
    /// covered by that read and changes nothing (a console terminal's text
    /// change comes by UIA and as a console update both).
    fn terminal_output(&mut self, node_id: NodeId, trace: TraceId, observed_at_ms: u64) {
        let covered = observed_at_ms != 0
            && self
                .context
                .terminals()
                .get(&node_id.number())
                .is_some_and(|terminal| terminal.covers(observed_at_ms));
        if covered {
            return;
        }
        self.terminal_event(node_id, Event::TextChanged, None, (trace, observed_at_ms));
    }

    /// Asks for the caret of a newly reported focus that may have text, or
    /// whose role says it may (Core waits to hear which before it speaks
    /// the line or the value), and moves the focus-following subscription
    /// to it ([`follow_focus`](Self::follow_focus)).
    fn follow_text(&self, (node_id, role): (NodeId, Role), trace: TraceId) {
        text_reads::forget_no_text(self.context, node_id);
        let has_text = self.follow_focus((node_id, role));
        let role_has_text = matches!(role, Role::EditableText | Role::Document | Role::Terminal);
        if has_text || role_has_text {
            self.context.intake.push(Entry {
                item: Item::CaretOf { node_id },
                trace,
                observed_at_ms: now_ms(),
                timing: EventTiming {
                    observed_at_us: now_us(),
                    ..EventTiming::default()
                },
            });
        }
    }

    /// Moves the focus-following subscription to the focus `node`, as NVDA
    /// moves its local group with the focus (`UIAHandler/__init__.py` 642 to
    /// 756; `NVDAObjects/UIA/__init__.py` 1623 to 1630;
    /// `docs/design/focus-pipeline.md`, section 17): to the node's own
    /// element when the outpost has it, with the extended group when the
    /// node may have text, and the base group otherwise; and to nothing for
    /// a focus whose element the outpost does not have, or one read through
    /// MSAA, whose changes and caret events come from the hooks. The element
    /// alone, not its descendants, as the reducer acts on the focus's own
    /// changes. NVDA registers its group with `TreeScope_Ancestors` as well,
    /// but UIA delivers no ancestor's event to such a registration: one on
    /// the Windows 11 taskbar clock's child with that scope did not hear the
    /// clock's own name change each minute, which one on the clock and one
    /// on the taskbar's subtree did (`docs/parity.md`, "UIA event
    /// registration"). A move to the node and group already followed is not
    /// asked of the subscription, so it makes no call; one to the same
    /// element under another node makes none either, as the subscription
    /// compares the elements. Returns whether the node may have text.
    fn follow_focus(&self, (node_id, role): (NodeId, Role)) -> bool {
        let has_text = text_reads::may_have_text(self.context, node_id, role);
        let element = self.context.uia_registry.element_of(node_id);
        let group = if has_text {
            verbatim_uia::Group::Extended
        } else {
            verbatim_uia::Group::Base
        };
        let wanted = element.as_ref().map(|_| (node_id, group));
        let mut followed = self
            .context
            .followed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *followed == wanted {
            tracing::debug!(
                ?wanted,
                "the focus-following subscription already follows this focus; it is not moved"
            );
            return has_text;
        }
        tracing::debug!(
            from = ?*followed,
            to = ?wanted,
            runtime_id = ?self.context.uia_registry.runtime_id_of(node_id),
            "the focus-following subscription is moved"
        );
        *followed = wanted;
        drop(followed);
        if let Some(subscription) = self.context.focus_events.get() {
            match element {
                Some(element) => {
                    subscription.retarget_to(verbatim_uia::Scope::Elements(vec![element]), group);
                }
                None => subscription.retarget(verbatim_uia::Scope::Nothing),
            }
        }
        has_text
    }

    /// Releases every node Core does not hold that was reported at or before
    /// the position Core acknowledged. A node reported later, or never
    /// reported, is kept, since Core may not have seen it yet.
    fn release(&self, held: &[u64], acknowledged: u64) {
        let held: HashSet<u64> = held.iter().copied().collect();
        // Only the worker in charge releases, and the nodes leave both
        // registries under the watch lock and the queue's, so a worker that
        // replaces this one, should it be abandoned while the objects are
        // dropped, can never report a node that is about to vanish, and no
        // message queued meanwhile can record one.
        let (objects, released) = {
            let state = self.context.watch.lock();
            if state.generation != self.generation {
                return;
            }
            let (position, released) =
                self.context
                    .outbound
                    .release(&held, acknowledged, |released| {
                        let keep = |id: NodeId| !released.contains(&id.number());
                        (
                            self.context.uia_registry.retain(keep),
                            self.context.msaa_registry.retain(keep),
                        )
                    });
            drop(state);
            tracing::debug!(
                ?held,
                acknowledged,
                position,
                released = ?released.as_ref().map(|(released, _)| released),
                "releasing the nodes Core no longer holds"
            );
            let Some((released, objects)) = released else {
                return;
            };
            (objects, released)
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
                application: window.and_then(|hwnd| console_client(hwnd, self.context.target_pid)),
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
        if !foreground && let Some(hwnd) = window {
            self.report_unshown_foreground(top_level_of(hwnd));
        }
        let mut node = node;
        let mut ancestors = ancestors;
        if backend == Backend::Uia {
            read::console_text_area(&mut node, window);
        }
        // A dialog the focus has newly entered, or one in front, says its
        // own text as its description.
        let previous = self.focus_chain();
        let mut memo = self.context.tracking().dialog.take();
        read::describe_dialogs(
            self.context,
            ancestors
                .iter_mut()
                .flatten()
                .chain(std::iter::once(&mut node)),
            (&previous, &mut memo, foreground),
        );
        self.context.tracking().dialog = memo;
        let role = node.role;
        let node_id = node.id;
        let states = node.states;
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
        if self.emit(trace, observed_at_ms, backend, window, event) {
            // A caret key's watch on another node ends with the focus,
            // saying nothing, as NVDA's wait gives way to a focus change.
            if self
                .context
                .caret_watch()
                .as_ref()
                .is_some_and(|open| open.node_id != node_id)
            {
                self.end_watch("the focus moved");
            }
            if !foreground {
                self.context.intake.set_focused(object);
            }
            let mut tracking = self.context.tracking();
            tracking.role = Some(role);
            tracking.batch = Some(self.batch);
            tracking.foreground_since_focus = foreground;
            if !foreground {
                tracking.reported += 1;
                tracking.focus = Some(node_id);
                tracking.focus_states = Some(states);
                tracking.focus_role = Some(role);
                // NVDA makes its foreground object when the focus enters a
                // top-level window, so the window keeps the verdict it had
                // then while the focus moves within it.
                let top = window.map(top_level_of).filter(|&top| top != 0);
                if top != tracking.window.map(top_level_of) {
                    let foreground = self.context.foreground_window();
                    tracking.foreground_window = top.filter(|&top| top == foreground);
                }
                tracking.window = window;
                // Unknown ancestors leave the previous chain, as the reducer
                // keeps it.
                if let Some(chain) = chain {
                    tracking.chain = chain;
                }
            }
            drop(tracking);
            // Move the focus-following subscription to the new focus,
            // without waiting, and ask for its caret. A foreground report
            // is a window, not the control focus is in.
            if let Some(node) = text_node {
                self.follow_text((node.id, node.role), trace);
            }
        }
    }

    /// The last focus this outpost reported, with its ancestors.
    fn focus_chain(&self) -> Vec<NodeSnapshot> {
        self.context.tracking().chain.clone()
    }

    /// Whether an event named `event` from `hwnd` is accepted by NVDA's
    /// test against the foreground window as the event is handled
    /// (`acceptance`), made before any call into the application. A dropped
    /// event is logged.
    fn accepts(&self, event: EventName, hwnd: isize) -> bool {
        let windows = SystemWindows {
            foreground: self.context.foreground_window(),
        };
        let accepted =
            acceptance::accepts_event(event, hwnd, &windows, &self.context.requested_events);
        if !accepted {
            tracing::debug!(
                hwnd,
                ?event,
                foreground = windows.foreground,
                "event dropped: its window is not related to the foreground window"
            );
        }
        accepted
    }

    /// Whether `hwnd` is the system's foreground window now, read by the
    /// reader the outpost was given (`Outpost::set_foreground_reader`). A
    /// foreground fact whose window is not is not reported, as NVDA's
    /// `processForegroundWinEvent` drops it; the reducer accepts every
    /// foreground fact on the strength of this check.
    fn in_front(&self, hwnd: isize) -> bool {
        self.context.foreground_window() == hwnd
    }

    /// Whether the event's own window, `hwnd`, can be read: NVDA's checks
    /// after acceptance and before the window's UIA test and the object's
    /// acquisition (`window::event_window_unreadable`). A dropped event is
    /// logged.
    fn readable(hwnd: isize) -> bool {
        let reason = event_window_unreadable(hwnd);
        if let Some(reason) = reason {
            tracing::debug!(hwnd, reason, "event dropped: its window cannot be read");
        }
        reason.is_none()
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
        if kind == WinEventKind::WindowShown {
            self.window_shown(hwnd);
            return;
        }
        if kind == WinEventKind::ConsoleUpdate {
            // The console host's text changed. Its UIA text changes stop
            // reaching a client now and then in the middle of a large write,
            // while these go on, so they are the focused console terminal's
            // text changes too (`docs/crates/verbatim-outpost.md`,
            // "Terminals").
            let (focus, window) = {
                let tracking = self.context.tracking();
                (tracking.focus, tracking.window)
            };
            if let Some(node_id) = focus.filter(|_| window == Some(hwnd))
                && self.focused_terminal(node_id)
            {
                // As for a UIA text change: a caret key's evidence is
                // checked before the read, which reads the caret too and
                // would leave the change nothing new to show.
                self.check_open_watch(node_id, false);
                self.terminal_output(node_id, trace, observed_at_ms);
            }
            return;
        }
        if kind == WinEventKind::Destroy {
            if id_object == OBJID_WINDOW.0 && id_child == CHILDID_SELF {
                self.context.arbitrator().forget(hwnd);
                self.context.forget_window(hwnd);
                // A reused window handle must never inherit these nodes.
                self.context.msaa_registry.forget_window(hwnd);
            }
            return;
        }
        // NVDA's order of checks: acceptance, the window's state, the
        // window's UIA test, and only then the object.
        if let Some(event) = EventName::of_win_event(kind)
            && !self.accepts(event, hwnd)
        {
            return;
        }
        if !Self::readable(hwnd) {
            return;
        }
        if read::window_uses_uia(self.context, hwnd) {
            return; // UIA owns this window.
        }
        if self.edit_event(kind, (hwnd, id_object, id_child), trace, observed_at_ms) {
            return;
        }
        let Some(object) = verbatim_ia2::acquire::event_object(hwnd, id_object, id_child) else {
            return;
        };
        let registry = &self.context.msaa_registry;
        if kind == WinEventKind::Selection {
            self.msaa_selection(object, hwnd, (trace, observed_at_ms));
            return;
        }
        let spoken = self.spoken_node(kind, &object);
        if kind == WinEventKind::ValueChange {
            self.value_change(object, spoken, (hwnd, trace, observed_at_ms));
            return;
        }
        let Some((node_id, role)) = spoken else {
            return;
        };
        // Only the property the event names is read, as NVDA reads only
        // that property of its focus or ancestor object.
        let event = match kind {
            WinEventKind::NameChange => NormalizedEvent::PropertyChanged {
                node_id,
                change: PropertyChange::Name(object.name(role, registry)),
                child_count: None,
            },
            WinEventKind::DescriptionChange => NormalizedEvent::PropertyChanged {
                node_id,
                change: PropertyChange::Description(object.description(role, registry)),
                child_count: None,
            },
            WinEventKind::StateChange => {
                let states = object.states(role);
                NormalizedEvent::PropertyChanged {
                    node_id,
                    child_count: self
                        .expanded_child_count((node_id, role, states), (hwnd, id_child)),
                    change: PropertyChange::States(states),
                }
            }
            _ => return,
        };
        self.emit(trace, observed_at_ms, Backend::Msaa, Some(hwnd), event);
    }

    /// An MSAA selection of `object`, from `hwnd`'s hooks, read only as far
    /// as it can be spoken. Selecting the focus or one of its ancestors is a
    /// change of its state, as NVDA handles it, so only its states are
    /// read. Any other selection is spoken only inside a focused selection
    /// container of this outpost (Core's `is_selection_container`), so it is
    /// read in full only while the focus this outpost last reported is
    /// one, and dropped otherwise, having cost its acquisition alone.
    fn msaa_selection(
        &mut self,
        object: verbatim_ia2::acquire::EventObject,
        hwnd: isize,
        (trace, observed_at_ms): (TraceId, u64),
    ) {
        let registry = &self.context.msaa_registry;
        // A tree view item selected on its way to the focus, from one of
        // its children, is one of the focus's logical ancestors only: NVDA,
        // whose ancestors are reached through `accParent`, sees it as
        // neither the focus nor an ancestor, and says nothing of it.
        if object
            .which_of(&self.logical_ancestors(), registry)
            .is_some()
        {
            tracing::debug!(
                hwnd,
                "MSAA selection dropped: a logical ancestor of the focus only"
            );
            return;
        }
        let (chain, container) = {
            let tracking = self.context.tracking();
            let chain: Vec<NodeId> = tracking
                .chain
                .iter()
                .map(|node| node.id)
                .chain(tracking.focus)
                .collect();
            let container = tracking
                .focus_role
                .is_some_and(verbatim_core::is_selection_container);
            (chain, container)
        };
        let event = if let Some(node_id) = object.which_of(&chain, registry)
            && let Some(role) = self.known_role(node_id)
        {
            NormalizedEvent::PropertyChanged {
                node_id,
                change: PropertyChange::States(object.states(role)),
                child_count: None,
            }
        } else if container {
            NormalizedEvent::SelectionChanged {
                node: object.read(registry, Purpose::Announce),
            }
        } else {
            tracing::debug!(
                hwnd,
                "MSAA selection dropped: the focus is no selection container"
            );
            return;
        };
        self.emit(trace, observed_at_ms, Backend::Msaa, Some(hwnd), event);
    }

    /// The caret, a text selection, or the text of the focus, when it is an
    /// edit control: reported from the control's messages, without reading
    /// its MSAA object (whose value is its whole text). Whether the event
    /// was one of these, and so handled.
    fn edit_event(
        &mut self,
        kind: WinEventKind,
        (hwnd, id_object, id_child): (isize, i32, i32),
        trace: TraceId,
        observed_at_ms: u64,
    ) -> bool {
        let client = Object::Msaa(hwnd, OBJID_CLIENT.0, CHILDID_SELF);
        let of_focus = self.context.intake.focused() == Some(client);
        match kind {
            WinEventKind::Caret | WinEventKind::TextSelectionChange => {
                if of_focus {
                    let node_id =
                        self.context
                            .msaa_registry
                            .id_for((hwnd, OBJID_CLIENT.0, CHILDID_SELF));
                    if !self.check_open_watch(node_id, true) {
                        self.caret_of(node_id, trace, observed_at_ms, false);
                    }
                } else if let Some(node_id) = self.watched_edit(hwnd) {
                    // A caret key's watch on an edit control Core took as
                    // its focus from a focus-now answer.
                    self.check_open_watch(node_id, true);
                }
                true
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
                self.check_open_watch(node_id, false);
                self.emit(
                    trace,
                    observed_at_ms,
                    Backend::Msaa,
                    Some(hwnd),
                    NormalizedEvent::TextChanged { node_id },
                );
                true
            }
            _ => false,
        }
    }

    /// The number of children of `node`, of `role` and now with `states`, a
    /// Win32 tree view item at `(hwnd, id_child)`, when its state change
    /// newly expands it and it is the focus, for Core to say how many it
    /// holds ("How many items an expanded tree view item holds" in
    /// `docs/nvda/speech.md`). Whether it was expanded is known from the
    /// focus's states as last read, which this change then updates.
    fn expanded_child_count(
        &self,
        (node, role, states): (NodeId, Role, StateSet),
        (hwnd, id_child): (isize, i32),
    ) -> Option<u32> {
        let was_expanded = {
            let mut tracking = self.context.tracking();
            if tracking.focus != Some(node) {
                return None;
            }
            tracking
                .focus_states
                .replace(states)
                .is_some_and(|states| states.contains(State::Expanded))
        };
        (role == Role::TreeItem && states.contains(State::Expanded) && !was_expanded)
            .then(|| verbatim_ia2::acquire::tree_view_child_count(hwnd, id_child))
            .flatten()
    }

    /// A value change on `object`, the focus when `focus` names its node and
    /// the role it was last read with: spoken as a value change only for the
    /// focus, and reported as a progress bar's whether or not it is the
    /// focus, as NVDA's progress bar behavior reports one
    /// (`docs/nvda/object-model.md`, "How a progress bar reports its
    /// value"). The focus has only its value read, or, when it is a progress
    /// bar, what a progress bar's report uses; an object that is not the
    /// focus has its role read alone first, and only a progress bar is read
    /// further.
    fn value_change(
        &mut self,
        mut object: verbatim_ia2::acquire::EventObject,
        focus: Option<(NodeId, Role)>,
        (hwnd, trace, observed_at_ms): (isize, TraceId, u64),
    ) {
        let registry = &self.context.msaa_registry;
        let progress_bar = match focus {
            Some((_, role)) => role == Role::ProgressBar,
            None => object.role() == Role::ProgressBar,
        };
        let progress = progress_bar
            .then(|| object.progress(focus, registry))
            .flatten();
        let event = match (progress, focus) {
            (Some(bar), _) => NormalizedEvent::ProgressChanged { bar },
            (None, Some((node_id, role))) => NormalizedEvent::ValueChanged {
                node_id,
                value: object.value(role),
            },
            (None, None) => return,
        };
        self.emit(trace, observed_at_ms, Backend::Msaa, Some(hwnd), event);
    }

    /// The role a node of the focus's chain was last read with: the focus,
    /// or one of its ancestors as last reported.
    fn known_role(&self, node: NodeId) -> Option<Role> {
        let tracking = self.context.tracking();
        if tracking.focus == Some(node) {
            return tracking.focus_role;
        }
        tracking
            .chain
            .iter()
            .find(|known| known.id == node)
            .map(|known| known.role)
    }

    /// The node `object` is, with the role it was last read with, when a
    /// change of `kind` on it is spoken: a name, description, or value
    /// change only for the focus, and a state change for the focus or one
    /// of its ancestors, as NVDA speaks them. Any other object is not read:
    /// it is told from those by its identity first, without reading any of
    /// its properties.
    fn spoken_node(
        &self,
        kind: WinEventKind,
        object: &verbatim_ia2::acquire::EventObject,
    ) -> Option<(NodeId, Role)> {
        let (focus, mut candidates) = {
            let tracking = self.context.tracking();
            // The ancestors NVDA has are those reached through `accParent`,
            // not the focus's logical ones ([`Self::logical_ancestors`]):
            // their state changes are not spoken, as selecting a tree item's
            // parent on its way to the focus would otherwise say "selected".
            // Nor is the foreground window an ancestor here
            // ([`Self::is_foreground_window`]).
            let ancestors: Vec<NodeId> = if kind == WinEventKind::StateChange {
                tracking
                    .chain
                    .iter()
                    .map(|node| node.id)
                    .filter(|&id| {
                        !self.is_logical_ancestor(id)
                            && !self.is_foreground_window(id, tracking.foreground_window)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            (tracking.focus?, ancestors)
        };
        candidates.push(focus);
        let node = object.which_of(&candidates, &self.context.msaa_registry)?;
        Some((node, self.known_role(node)?))
    }

    /// Whether `node`, one of the focus's ancestors, was reached through a
    /// tree view item's logical parents
    /// (`verbatim_ia2::acquire::ancestor_chain`) rather than `accParent`:
    /// `accParent` only ever reaches whole objects, so an ancestor that is a
    /// simple child of its window's object is a logical one.
    fn is_logical_ancestor(&self, node: NodeId) -> bool {
        self.context
            .msaa_registry
            .key_of(node)
            .is_some_and(|(_, _, child)| child != CHILDID_SELF)
    }

    /// Whether `node`, one of the focus's ancestors, is the foreground
    /// window's own object, its client area or its window object, the
    /// window being the focus's top-level window and the foreground when
    /// the focus entered it: NVDA never speaks a state change on
    /// it, as a modal dialog's owner being disabled when the dialog opens,
    /// live-checked against NVDA on 2026-10-08 (eleven captures, never
    /// "unavailable"). NVDA's state-change gate speaks for an ancestor only
    /// when the event's object is the very instance in its ancestor list,
    /// and an event finds an existing instance only in its live-object
    /// table, keyed by the event's address. When the focus enters a
    /// top-level window, NVDA reads its ancestors and then creates the
    /// foreground object, for the foreground window's client area, which
    /// takes the address's place in the table and is held as the
    /// foreground, so an event on that window resolves to the foreground
    /// object, never to the ancestor (`docs/parity.md`, "A top-level
    /// window's state change"). A top-level window that is not the
    /// foreground, such as a popup menu's, keeps its ancestor.
    fn is_foreground_window(&self, node: NodeId, foreground: Option<isize>) -> bool {
        foreground.is_some_and(|foreground| {
            self.context
                .msaa_registry
                .key_of(node)
                .is_some_and(|(hwnd, id_object, child)| {
                    hwnd == foreground
                        && child == CHILDID_SELF
                        && (id_object == OBJID_CLIENT.0 || id_object == OBJID_WINDOW.0)
                })
        })
    }

    /// The focus's logical ancestors ([`Self::is_logical_ancestor`]).
    fn logical_ancestors(&self) -> Vec<NodeId> {
        let chain: Vec<NodeId> = self
            .context
            .tracking()
            .chain
            .iter()
            .map(|node| node.id)
            .collect();
        chain
            .into_iter()
            .filter(|&id| self.is_logical_ancestor(id))
            .collect()
    }

    /// A UIA event from this outpost's own subscriptions, or a selection or
    /// notification fact from the listener. NVDA's order: whether the event
    /// is the focus's, then whether its window is UIA's, then the acceptance
    /// test against the foreground window, and only then the event's own
    /// handling (`IUIAutomationPropertyChangedEventHandler_HandlePropertyChangedEvent`
    /// and its siblings, `source/UIAHandler/__init__.py`).
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
        if let UiaKind::Notification(notification) = &event.kind {
            // NVDA neither arbitrates nor tests a notification: it is
            // spoken only from the focus's application, which Core judges.
            if is_terminal_output_notification(event.parts.role, notification) {
                return; // The diff of the terminal's text reports it.
            }
        } else {
            if let Some(hwnd) = hwnd
                && !read::window_uses_uia(self.context, hwnd)
            {
                return; // MSAA owns this window.
            }
            // A windowless element with no window found is tested with its
            // application's focus window when the listener captured it.
            if let Some(name) = uia_event_name(&event.kind)
                && !self.accepts(name, hwnd.unwrap_or(event.focus_window))
            {
                return;
            }
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
                    text_reads::text_event_from(self.context, node_id, element.as_ref());
                    if !self.check_open_watch(node_id, true) {
                        self.caret_of(node_id, trace, observed_at_ms, false);
                    }
                }
                return;
            }
            UiaKind::ActiveTextPosition(range) => {
                let event = (&event.parts.runtime_id[..], range, hwnd);
                return self.active_text_position(event, (trace, observed_at_ms));
            }
            UiaKind::TextChanged => {
                let changed = (&event.parts.runtime_id[..], element.as_ref());
                return self.text_changed(changed, hwnd, (trace, observed_at_ms));
            }
            _ => {}
        }
        let node = Self::uia_node(self.context, &event.parts, element.as_ref());
        let normalized = match event.kind {
            UiaKind::Property(id) if id == UIA_NamePropertyId.0 => {
                NormalizedEvent::PropertyChanged {
                    node_id: node.id,
                    change: PropertyChange::Name(node.name),
                    child_count: None,
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
                child_count: None,
            },
            UiaKind::Selection => NormalizedEvent::SelectionChanged { node },
            UiaKind::Notification(notification) => NormalizedEvent::Notification {
                node_id: node.id,
                notification,
            },
            UiaKind::TextSelection | UiaKind::TextChanged | UiaKind::ActiveTextPosition(_) => {
                return;
            }
        };
        self.emit(trace, observed_at_ms, Backend::Uia, hwnd, normalized);
    }

    /// A UIA text change of the text focus whose runtime id it carries: it
    /// checks a caret key's watch open on the node, then is reported, or,
    /// for a terminal, has its new output read.
    fn text_changed(
        &mut self,
        (runtime_id, element): (&[i32], Option<&IUIAutomationElement>),
        hwnd: Option<isize>,
        (trace, observed_at_ms): (TraceId, u64),
    ) {
        let Some(node_id) = self.context.uia_registry.existing_id(runtime_id) else {
            return;
        };
        text_reads::text_event_from(self.context, node_id, element);
        self.check_open_watch(node_id, false);
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

    /// An active text position change for the text focus whose runtime id
    /// it carries, the start of its range reported as a position in the
    /// node's text. As NVDA's
    /// handler, an event whose element or range cannot be had is dropped,
    /// and one from a window the system reports hung never gets this far.
    fn active_text_position(
        &mut self,
        (runtime_id, range, hwnd): (&[i32], Option<super::intake::ActiveRange>, Option<isize>),
        (trace, observed_at_ms): (TraceId, u64),
    ) {
        let node_id = self.context.uia_registry.existing_id(runtime_id);
        let (Some(node_id), Some(range)) = (node_id, range) else {
            return;
        };
        if let Some(position) = text_reads::active_position(self.context, node_id, &range.0) {
            self.emit(
                trace,
                observed_at_ms,
                Backend::Uia,
                hwnd,
                NormalizedEvent::ActiveTextPositionChanged { node_id, position },
            );
        }
    }

    /// The selection of the element `runtime_id` names as a selection in a
    /// list the focus controls, when it is one: the focused element names,
    /// in its `ControllerFor` relation, an element the selected one is inside
    /// ("Selection in a list the focus controls" in `docs/nvda/events.md`).
    fn controlled_selection(&mut self, runtime_id: &[i32]) -> Option<NormalizedEvent> {
        // The focus this outpost last reported, as NVDA uses its focus
        // object; found by its path when its element is not known.
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
            None => self.element_by_focus_path(&focus_id)?,
        };
        // NVDA reads the relation live for each selection event; the
        // relation and the search under what it names are one remote
        // operation where the focus's window allows one.
        let window = self.context.tracking().window;
        let uia = self.client.uia()?;
        let properties = verbatim_uia::cached_properties(self.context.fetches());
        let query = verbatim_uia_rops::ControlledQuery {
            focused: &focused,
            selected: runtime_id,
            properties: &properties,
        };
        let remote = self.context.tries_remote(window);
        let (selected, path) = verbatim_uia_rops::controlled_selection(uia, &query, remote).ok()?;
        if let verbatim_uia_rops::Path::Fallback(error) = &path {
            tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
            if let (verbatim_uia_rops::Error::Import(_), Some(hwnd)) = (error, window) {
                self.context.read_classically(top_level_of(hwnd));
            }
        }
        let selected = selected?;
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

    /// A fact routed from the listener, or one the outpost's own hook took
    /// in: an MSAA focus, foreground change, menu opening, alert, or
    /// tooltip, whichever brought it.
    fn fact(&mut self, fact: DeliveredFact, trace: TraceId, observed_at_ms: u64) {
        // The listener refuses a UIA focus in such a window before it
        // reads it.
        let focus_window = match &fact {
            DeliveredFact::Foreground { hwnd } | DeliveredFact::MsaaFocus { hwnd, .. } => {
                Some(*hwnd)
            }
            _ => None,
        };
        if let Some(hwnd) = focus_window
            && is_shell_transition_window(hwnd)
        {
            tracing::debug!(
                hwnd,
                "focus dropped: a shell window that only stages a switch"
            );
            return;
        }
        match fact {
            DeliveredFact::Foreground { hwnd } => self.foreground(hwnd, trace, observed_at_ms),
            DeliveredFact::MsaaFocus {
                hwnd,
                id_object,
                id_child,
            } => self.msaa_focus(hwnd, id_object, id_child, trace, observed_at_ms),
            DeliveredFact::UiaFocusRead { read, .. } => {
                self.uia_focus(&read, trace, observed_at_ms);
            }
            // The intake resolves these; none reaches the worker.
            DeliveredFact::UiaFocusMarker { .. }
            | DeliveredFact::UiaFocusDropped { .. }
            | DeliveredFact::UiaFocusReadsEnded => {}
            DeliveredFact::UiaSelection {
                hwnd,
                focus_window,
                snapshot,
            } => self.uia_event(
                UiaEvent {
                    kind: UiaKind::Selection,
                    parts: snapshot,
                    hwnd,
                    focus_window,
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
                    focus_window: 0,
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
            DeliveredFact::Show {
                hwnd,
                id_object,
                id_child,
            } => self.tooltip_shown(hwnd, id_object, id_child, trace, observed_at_ms),
            // Menu openings are planned separately; one reaching here is
            // handled the same way.
            menu @ (DeliveredFact::MenuPopup { .. } | DeliveredFact::UiaMenuOpened { .. }) => {
                self.menu_opened(&Entry {
                    item: Item::Fact(menu, None),
                    trace,
                    observed_at_ms,
                    timing: crate::protocol::EventTiming::default(),
                });
            }
        }
    }

    /// A tooltip window shown. A help balloon is reported as an alert, for
    /// the reducer to speak queued from any application, as NVDA's
    /// notification behavior speaks a help balloon, which NVDA reports by
    /// default; an ordinary tooltip is not, as NVDA does not report
    /// tooltips by default.
    fn tooltip_shown(
        &mut self,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        trace: TraceId,
        observed_at_ms: u64,
    ) {
        // NVDA accepts a show event by its window's class alone, whatever
        // the foreground; the hook that took this one, the outpost's own or
        // the focus listener's, took it only from a tooltip window
        // (`verbatim_ia2::hook`), so it passes.
        if !Self::readable(hwnd) {
            return;
        }
        let Some(mut object) = verbatim_ia2::acquire::event_object(hwnd, id_object, id_child)
        else {
            return;
        };
        if object.role() != Role::HelpBalloon {
            return;
        }
        let node = object.read(&self.context.msaa_registry, Purpose::Announce);
        self.emit(
            trace,
            observed_at_ms,
            Backend::Msaa,
            Some(hwnd),
            NormalizedEvent::Alert { node },
        );
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
        if !self.accepts(EventName::Alert, hwnd) {
            return;
        }
        if super::window::parent_class(hwnd).as_deref() != Some("ToastChildWindowClass") {
            return;
        }
        if !Self::readable(hwnd) {
            return;
        }
        let Some(node) = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            &self.context.msaa_registry,
            verbatim_ia2::acquire::Purpose::Announce,
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
    /// the window, unless the window is not the system's foreground window
    /// when the change is handled or when it is read, or is read through
    /// UI Automation.
    ///
    /// It is stamped with the time its window was confirmed as the
    /// foreground, read after the window's name, not the time Windows
    /// raised the event, so the old foreground window's own focus events
    /// handled meanwhile are never newer than the change. NVDA judges a
    /// foreground event against the foreground window when it processes
    /// the event, and drops it when they differ (`processForegroundWinEvent`,
    /// `IAccessibleHandler/__init__.py` 834 to 841); so does this, without
    /// waiting for the window to come in front. Measured live: the system's
    /// foreground event reached an out-of-context hook only once
    /// `GetForegroundWindow` named its window, and the events that did not
    /// were stale or raised early by the application itself, as File
    /// Explorer raises one as it creates its window and the system's own
    /// follows once the window is in front (`phase6-design.md`,
    /// "Foreground events against the foreground window").
    ///
    /// A window that is the foreground window but not yet shown is reported
    /// once it is shown, its name read then, rather than at once: File
    /// Explorer raises the foreground event of a folder window it opens in
    /// front before showing it, and a window's title is the one it is shown
    /// with (`phase6-design.md`, "File Explorer opened without the
    /// foreground right"). Its show event brings it
    /// ([`window_shown`](Self::window_shown)); a focus inside it that comes
    /// first has it reported just before
    /// ([`report_unshown_foreground`](Self::report_unshown_foreground)),
    /// since a window reported after a focus inside it is not announced.
    ///
    /// A window not yet in front is dropped and never kept, as NVDA drops
    /// it; so is a window read through UI Automation, for which NVDA drops
    /// every MSAA event, a foreground event included (`winEventToNVDAEvent`,
    /// `IAccessibleHandler/__init__.py` 576 to 582). Either window is
    /// announced as the focus enters it, from the focus's ancestry, where the
    /// top-level window is an ancestor like any other: the reducer speaks
    /// the entered ancestors when the focus's top-level window changes
    /// (`docs/design/focus-pipeline.md`, section 16.4). A console window's
    /// foreground event comes before the window is in front, and the window
    /// is read through UI Automation besides, so its title is the console
    /// window element's, the text area's parent.
    fn foreground(&mut self, hwnd: isize, trace: TraceId, observed_at_ms: u64) {
        // A newer foreground change replaces one still waiting.
        self.context.tracking().unshown_foreground = None;
        if window_belongs_to_hidden_frame(hwnd) {
            tracing::debug!(hwnd, "foreground dropped: Core's hidden frame");
            return;
        }
        if !self.accepts(EventName::GainFocus, hwnd) || !self.in_front(hwnd) {
            tracing::debug!(hwnd, "foreground dropped: not the foreground window");
            return;
        }
        if read::window_uses_uia(self.context, hwnd) {
            tracing::debug!(hwnd, "foreground dropped: UIA owns the window");
            return;
        }
        if !window_is_visible(hwnd) {
            tracing::debug!(hwnd, "foreground waits for its window to be shown");
            self.context.tracking().unshown_foreground = Some((hwnd, trace, observed_at_ms));
            return;
        }
        self.report_foreground(hwnd, trace, observed_at_ms);
    }

    /// A top-level window of the application was shown: the foreground
    /// change waiting for it, if any, is reported now, while the window is
    /// still the foreground window.
    fn window_shown(&mut self, hwnd: isize) {
        let waiting = {
            let mut tracking = self.context.tracking();
            match tracking.unshown_foreground {
                Some((waiting, ..)) if waiting == hwnd => tracking.unshown_foreground.take(),
                _ => None,
            }
        };
        if let Some((hwnd, trace, observed_at_ms)) = waiting {
            if self.in_front(hwnd) {
                self.report_foreground(hwnd, trace, observed_at_ms);
            } else {
                tracing::debug!(hwnd, "foreground dropped: shown no longer in front");
            }
        }
    }

    /// Reports the foreground change waiting for its window, `top`, to be
    /// shown, just before a focus inside it is reported, if it is still the
    /// foreground window.
    fn report_unshown_foreground(&mut self, top: isize) {
        let waiting = {
            let mut tracking = self.context.tracking();
            match tracking.unshown_foreground {
                Some((waiting, ..)) if waiting == top => tracking.unshown_foreground.take(),
                _ => None,
            }
        };
        if let Some((hwnd, trace, observed_at_ms)) = waiting
            && self.in_front(hwnd)
        {
            tracing::debug!(hwnd, "foreground reported before a focus inside its window");
            self.report_foreground(hwnd, trace, observed_at_ms);
        }
    }

    /// Reads the foreground window `hwnd`'s name and reports it, unless it
    /// is no longer the foreground window by then.
    fn report_foreground(&mut self, hwnd: isize, trace: TraceId, observed_at_ms: u64) {
        let (backend, node) = read::foreground_window(self.context, self.client, hwnd);
        if !self.in_front(hwnd) {
            tracing::info!(
                hwnd,
                "foreground report dropped: no longer the foreground window"
            );
            return;
        }
        self.emit_focus(
            trace,
            observed_at_ms.max(now_ms()),
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
        if !self.accepts(EventName::GainFocus, hwnd) || !Self::readable(hwnd) {
            return;
        }
        if read::window_uses_uia(self.context, hwnd) {
            // UIA owns this window; its UIA fact reports the focus.
            tracing::debug!(hwnd, "MSAA focus dropped: UIA owns the window");
            return;
        }
        // A control taking the focus raised focus on itself and then on its
        // focused child, and the control's event was already reported as
        // the child (`verbatim_ia2::acquire::focus_candidate`): NVDA handles
        // the two together and reports the child once.
        if self
            .context
            .intake
            .take_redirected_focus(&Object::Msaa(hwnd, id_object, id_child))
        {
            tracing::debug!(
                hwnd,
                id_object,
                id_child,
                "MSAA focus dropped: already reported from its control's focus"
            );
            return;
        }
        let Some(mut candidate) = verbatim_ia2::acquire::focus_candidate(hwnd, id_object, id_child)
        else {
            tracing::debug!(hwnd, id_object, id_child, "MSAA focus dropped: unreadable");
            return;
        };
        // NVDA drops a focus event naming the address of the focus it last
        // queued, unless that focus was a child by id, whose id can be
        // reused (`isDuplicateIAccessibleEvent`), before reading anything
        // of it. The focus may have been in another application since a
        // foreground change, which this outpost does not see, so a focus
        // after one is never a duplicate.
        let (address_hwnd, address_object, address_child) = candidate.key();
        let repeated = address_child == CHILDID_SELF
            && !self.context.tracking().foreground_since_focus
            && self.context.intake.focused()
                == Some(Object::Msaa(address_hwnd, address_object, address_child));
        if repeated {
            tracing::debug!(
                hwnd,
                id_object,
                id_child,
                "MSAA focus dropped: the focus already"
            );
            self.context.tracking().reported += 1;
            return;
        }
        // NVDA accepts an MSAA focus only when the object or one of its
        // ancestors has the focused state (`shouldAllowIAccessibleFocusEvent`),
        // which weeds out stale and spurious focus events. It checks the
        // states first, reading them live, before anything else.
        if !candidate.has_focused_state(read::MAX_ANCESTOR_HOPS) {
            tracing::debug!(
                hwnd,
                id_object,
                id_child,
                "MSAA focus dropped: nothing has the focused state"
            );
            return;
        }
        let node = candidate.read(&self.context.msaa_registry);
        let previous = self.focus_chain();
        let enrichment = read::msaa_enrichment(self.context, self.client, &node, &previous);
        let object = self
            .context
            .msaa_registry
            .key_of(node.id)
            .map(|(hwnd, object, child)| Object::Msaa(hwnd, object, child));
        let redirected = object
            .as_ref()
            .is_some_and(|object| *object != Object::Msaa(hwnd, id_object, id_child));
        self.emit_focus(
            trace,
            observed_at_ms,
            Backend::Msaa,
            Some(hwnd),
            node,
            false,
            object.clone(),
            enrichment,
        );
        if redirected && self.context.intake.focused() == object {
            self.context.intake.set_focus_redirected();
        }
    }

    /// Whether a UIA focus the listener read passes NVDA's acceptance test
    /// against the foreground window, made as the focus is handled, before
    /// any call into the application. The listener has judged the rest
    /// already: the element had the keyboard focus, read live, and the
    /// console window's own focus never comes.
    ///
    /// The test runs against the element's nearest window, as NVDA's runs
    /// against `getNearestWindowHandle(sender)`
    /// (`docs/design/focus-pipeline.md`, section 16.2): its own window, else
    /// the nearest its listener's read found, else the nearest ancestor's.
    /// An element with no window in its chain is tested with 0, which
    /// passes, as NVDA lets such a focus through untested
    /// (`shouldAcceptEvent` cannot filter without a window handle).
    fn allows_uia_focus(&self, read: &UiaFocusRead) -> bool {
        let window = Some(read.hwnd)
            .filter(|&hwnd| hwnd != 0)
            .or(read.window)
            .or_else(|| {
                read.ancestors.as_ref().and_then(|ancestors| {
                    ancestors
                        .iter()
                        .map(|ancestor| ancestor.hwnd)
                        .find(|&hwnd| hwnd != 0)
                })
            });
        self.accepts(EventName::GainFocus, window.unwrap_or(0))
    }

    /// A UIA focus as the focus listener judged and read it from the
    /// event's own element (`docs/design/focus-pipeline.md`, section 12):
    /// the element had the keyboard focus when the listener read it, as
    /// NVDA's `shouldAllowUIAFocusEvent` requires, and its states, value,
    /// details, ancestors, and selected child are the element's own, read
    /// in one remote operation. The outpost reports it at once, its node
    /// minted from its runtime id; the listener's evidence that the element
    /// it read before under that runtime id has lost the focus or died
    /// gives the id a new node first. Only then, off the way to speech,
    /// does the outpost find its own copy of the element, which no process
    /// can hand it (experiment E-marshal), by the path the listener read
    /// ([`find_focus_element`](Self::find_focus_element)).
    fn uia_focus(&mut self, read: &UiaFocusRead, trace: TraceId, observed_at_ms: u64) {
        let context = self.context;
        if !self.allows_uia_focus(read) {
            return;
        }
        let Ok(reported) = self.uia_focus_window((read.hwnd, read.focus_window), read.window)
        else {
            return;
        };
        let runtime_id = &read.snapshot.runtime_id;
        if read.same_as_last == Some(false) {
            tracing::debug!(
                "UIA focus: its runtime id named an element that lost the focus or is gone; a new node is issued"
            );
            drop(context.uia_registry.reissue(runtime_id));
        }
        let reading = Instant::now();
        let previous = self.focus_chain();
        let node = Self::uia_node(context, &read.snapshot, None);
        let selected = read
            .selected_child
            .as_ref()
            .map(|child| Self::uia_node(context, child, None));
        let ancestors = read.ancestors.as_ref().and_then(|ancestors| {
            read::uia_chain_from_read(context, self.client, ancestors, &previous)
        });
        *context.focus_path() = Some(FocusPath::of(read));
        tracing::debug!(
            %trace,
            enrichment_us = reading.elapsed().as_micros(),
            "UIA focus reported from the listener's read"
        );
        // A focus on the element this outpost already found under its
        // runtime id keeps it: the listener read that it still has the
        // keyboard focus. A plain repeat never comes this far, as the
        // listener drops it (section 16.3); this is the focus coming back
        // to the element after one in another application.
        let found =
            read.same_as_last == Some(true) && context.uia_registry.element_of(node.id).is_some();
        self.emit_focus(
            trace,
            observed_at_ms,
            Backend::Uia,
            reported,
            node,
            false,
            Some(Object::Uia(runtime_id.clone())),
            (ancestors, selected),
        );
        if !found {
            self.find_focus_element(runtime_id, trace);
        }
    }

    /// Finds this outpost's own copy of the focus `runtime_id` names, just
    /// reported from the listener's read, by the path the listener read:
    /// from the nearest window in the focus's chain, down by runtime ids,
    /// one connection call and one transaction. Found, it is kept under the
    /// focus's node and the focus-following subscription is moved to it
    /// ([`follow_focus`](Self::follow_focus)); not found, as Windows
    /// Terminal's tab focus elements, which are in no tree, are not, nothing
    /// is followed, as NVDA hears nothing local before its group is added to
    /// a focus (`docs/design/focus-pipeline.md`, sections 12.5 and 17).
    /// Nothing is looked for once another focus has been reported.
    fn find_focus_element(&mut self, runtime_id: &[i32], trace: TraceId) {
        if self.context.intake.focused() != Some(Object::Uia(runtime_id.to_vec())) {
            return;
        }
        let finding = Instant::now();
        let found = self.element_by_focus_path(runtime_id);
        tracing::debug!(
            found = found.is_some(),
            elapsed_us = finding.elapsed().as_micros(),
            "the focus's own element looked for by its path"
        );
        if let Some(element) = found {
            self.follow_focus_element(runtime_id, &element, trace);
        } else {
            tracing::debug!("the focus's element was not found; nothing is followed");
        }
    }

    /// The element `runtime_id` names, an element of the chain the
    /// listener read for the focus this outpost last reported, found by its
    /// path ([`focus_path::find`]); `None` when it is not in that chain, or
    /// its path does not lead to it.
    fn element_by_focus_path(&mut self, runtime_id: &[i32]) -> Option<IUIAutomationElement> {
        let context = self.context;
        let route = context.focus_route(runtime_id)?;
        let uia = self.client.uia()?;
        let properties = verbatim_uia::cached_properties(context.fetches());
        let remote = context.tries_remote(Some(route.window));
        let (found, import_failed) = focus_path::find(uia, &route, &properties, remote);
        if import_failed {
            context.read_classically(route.window);
        }
        found
    }

    /// The window a UIA focus is reported in, or [`Dropped`] when the focus
    /// is in Core's hidden frame or in a window MSAA owns.
    fn uia_focus_window(
        &self,
        (fact_hwnd, focus_window): (isize, isize),
        remote_window: Option<isize>,
    ) -> Result<Option<isize>, Dropped> {
        // The event's own window; else the element's nearest, which the
        // listener's read found with its ancestors; else this application's
        // keyboard focus window when the listener received the event, which
        // hosts the element when the event is current but, in an
        // application with several windows, may be another of its windows
        // when the event is late; else, for deciding the backend only, this
        // application's focus window now, which is never reported.
        let reported = if fact_hwnd != 0 {
            Some(fact_hwnd)
        } else {
            remote_window.or((focus_window != 0).then_some(focus_window))
        };
        let judged = reported.or_else(|| focus_window_of(self.context.target_pid));
        if judged.is_some_and(window_belongs_to_hidden_frame) {
            return Err(Dropped);
        }
        if let Some(hwnd) = judged
            && !read::window_uses_uia(self.context, hwnd)
        {
            // MSAA owns this window; its MSAA fact reports the focus.
            tracing::debug!(hwnd, "UIA focus dropped: MSAA owns the window");
            return Err(Dropped);
        }
        Ok(reported)
    }

    /// Keeps `element`, found for the focus `runtime_id` names, which was
    /// reported from its event alone, under the focus's node, and follows
    /// its changes.
    fn follow_focus_element(
        &mut self,
        runtime_id: &[i32],
        element: &IUIAutomationElement,
        trace: TraceId,
    ) {
        let context = self.context;
        let id = context.uia_registry.id_for_element(runtime_id, element);
        // The focus was reported without its element, so its text could not
        // be read: its changes are followed from now on, and its caret
        // reported, whose line Core speaks for the focus.
        let role = context.tracking().role;
        match role {
            Some(role) => self.follow_text((id, role), trace),
            None => {
                self.follow_focus((id, Role::Unknown));
            }
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
        if let Item::Fact(DeliveredFact::UiaMenuOpened { hwnd, snapshot }, _) = &entry.item {
            self.uia_menu_opened(*hwnd, snapshot, entry);
            return;
        }
        let Item::Fact(
            DeliveredFact::MenuPopup {
                hwnd,
                id_object,
                id_child,
            },
            _,
        ) = entry.item
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
        if !self.accepts(EventName::MenuStart, hwnd) || !Self::readable(hwnd) {
            return;
        }
        if read::window_uses_uia(self.context, hwnd) {
            return;
        }
        let Some(node) = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            &self.context.msaa_registry,
            verbatim_ia2::acquire::Purpose::Announce,
        ) else {
            return;
        };
        if node.role != Role::Menu {
            return;
        }
        // Verbatim's own menu opens from Core's hidden frame, shown and
        // brought to the foreground for it, whose own foreground report is
        // dropped because the frame transits focus. NVDA announces the
        // foreground window a menu opens from before the menu, so the menu
        // is reported in the frame's window, the foreground window, with the
        // frame, titled "Verbatim", as its ancestor: the reducer names a new
        // foreground window from the top of the focus's ancestry when no
        // foreground report named it, then the menu.
        let foreground = foreground_window_handle();
        let (window, ancestors) = if window_is_hidden_frame(foreground) {
            let (_, frame) = read::foreground_window(self.context, self.client, foreground);
            (foreground, vec![frame])
        } else {
            (hwnd, Vec::new())
        };
        self.emit_focus(
            entry.trace,
            entry.observed_at_ms,
            Backend::Msaa,
            Some(window),
            node,
            false,
            Some(Object::Msaa(hwnd, id_object, id_child)),
            (Some(ancestors), None),
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
        // NVDA treats a menu opening as a focus on the menu.
        if !self.accepts(EventName::GainFocus, hwnd.unwrap_or(0)) {
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

    /// A query from Core, answered with exactly one reply: at once, or, for
    /// a caret key's watch kept open, when its evidence comes or it ends.
    fn query(&mut self, request_id: u64, query: &Query, trace: TraceId) {
        if let Query::Text {
            node_id,
            op: TextOp::AwaitCaret(watch),
        } = query
        {
            self.caret_key(request_id, trace, *node_id, watch);
            return;
        }
        if let Query::Text { node_id, op } = query
            && let Some(event) = terminal_event_of(op)
        {
            // Core's on-demand reading of the focused terminal; a node that
            // is not a focused terminal has nothing to read.
            if !self.focused_terminal(*node_id) {
                let reply = if event == Event::Hold {
                    TextReply::Done
                } else {
                    TextReply::Terminal(Box::default())
                };
                self.reply(
                    request_id,
                    trace,
                    QueryOutcome::Done(QueryResult::Text(reply)),
                );
                return;
            }
            let observed_at_ms = now_ms();
            if event == Event::Hold {
                self.terminal_event(*node_id, event, None, (trace, observed_at_ms));
                self.reply(
                    request_id,
                    trace,
                    QueryOutcome::Done(QueryResult::Text(TextReply::Done)),
                );
            } else {
                self.terminal_event(
                    *node_id,
                    event,
                    Some((request_id, trace)),
                    (trace, observed_at_ms),
                );
            }
            return;
        }
        let context = self.context;
        let client = &mut *self.client;
        let started = std::time::Instant::now();
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
                read::describe_dialogs(context, window.chain(focus), (&previous, &mut None, false));
                // Core takes the answer's control as its focus, and a caret
                // key there is answered by the control's caret events, so
                // they are followed as a reported focus's are.
                if let Some(control) = &answer.focus {
                    self.follow_focus((control.node.id, control.node.role));
                }
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
                Ok(QueryResult::Text(text_reads::answer(context, *node_id, op)))
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
        self.reply(request_id, trace, outcome);
    }

    /// Publishes the reply to the query in hand, ending its entry.
    fn reply(&self, request_id: u64, trace: TraceId, outcome: QueryOutcome) {
        publish(
            self.context,
            self.generation,
            OutpostToSupervisor::Reply {
                trace_id: trace,
                request_id,
                outcome,
                timing: EventTiming {
                    published_at_us: now_us(),
                    calls: take_calls(),
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
        let published = watch.publish(0, || replies.push((7, "done"))).is_some();

        assert!(!published, "an abandoned worker publishes nothing");
        assert_eq!(replies, [(7, "abandoned")]);
    }

    #[test]
    fn a_published_query_can_no_longer_be_abandoned() {
        let watch = Watch::default();
        let mut replies = Vec::new();
        watch.start(STEP_DEADLINE, Some((7, TraceId::mint())), 0);

        assert!(watch.publish(0, || replies.push((7, "done"))).is_some());
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
            deadline,
            "and is checked again when a foreground fact wakes the watchdog, or at the deadline"
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
}
