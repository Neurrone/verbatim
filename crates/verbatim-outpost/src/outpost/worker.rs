//! The worker and its watchdog (outpost redesign, "Inside an outpost").
//!
//! One worker thread takes entries from the intake queue in order and
//! finishes each before starting the next. It is the only thread that calls
//! into the application, so events and replies leave the outpost in the order
//! the intake plans their entries: the order they joined the queue, but for
//! the events of other objects that a focus change overtakes
//! (`intake::overtaken`) and the queries that go ahead of a batch held for
//! its foreground change. This is NVDA's model, one thread doing all the
//! work, with one such thread per application. The worker waits for
//! nothing but its calls into the application and the queue: a caret key's
//! evidence is watched for between entries, and a foreground change is
//! confirmed by the queue.
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
use verbatim_uia::map::{
    cached_process_id, snapshot_from_cached_element, with_legacy_checked_state,
};
use verbatim_uia::{
    ElementExt as _, map::snapshot_parts_from_cached_element, nearest_window_handle,
};
use windows::Win32::UI::Accessibility::{
    UIA_NamePropertyId, UIA_RangeValueValuePropertyId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use crate::protocol::{
    DeliveredFact, EventTiming, FocusedControl, OutpostToSupervisor, Query, QueryOutcome,
    QueryResult, UiaSnapshotFact, now_us,
};

use super::Context;
use super::intake::{Entry, Foreground, Item, Object, Planned, UiaEvent, UiaKind, window_of};
use super::read::{self, Client, ReadError};
use super::text_reads::{self, CARET_WATCH_BOUND, CONSOLE_WINDOW_CLASS, OpenWatch};
use super::window::{
    focus_window_of, foreground_window_handle, front_is_another_thread_of_its_application, now_ms,
    top_level_of, window_belongs_to_hidden_frame, window_facts, window_is_foreground,
    window_is_hidden_frame, window_owner,
};
use crate::arbitration::window_class_name;
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

/// Where the focus-following property subscription listens for a focus
/// whose live UIA element is `element`: on that element alone, or nowhere
/// for a focus with no UIA element. The reducer acts on the focus's own
/// name, value, and state changes and on no other element's. NVDA
/// registers its local group on the focus with `TreeScope_Ancestors` as
/// well, but UIA delivers no ancestor's event to such a registration: a
/// registration on the Windows 11 taskbar clock's child with that scope
/// did not hear the clock's own name change each minute, which one on the
/// clock and one on the taskbar's subtree did (`docs/parity.md`, "UIA
/// event registration").
fn following(
    element: Option<
        windows::core::AgileReference<windows::Win32::UI::Accessibility::IUIAutomationElement>,
    >,
) -> verbatim_uia::Scope {
    element.map_or(verbatim_uia::Scope::Nothing, |element| {
        verbatim_uia::Scope::Elements(vec![element])
    })
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
    // When the current batch's foreground change was confirmed. `next` says
    // so only with a batch's first entry, and the foreground fact need not
    // be that entry, so the time is kept for the whole batch.
    let mut confirmed: Option<(u64, Option<u64>)> = None;
    while let Some((planned, batch, foreground)) = context.intake.next() {
        if let Some(foreground) = foreground {
            let at = match foreground {
                Foreground::Confirmed(at) => Some(at),
                Foreground::NotConfirmed => None,
            };
            confirmed = Some((batch, at));
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
    let arbitration_started = context.arbitrator().now();
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
        Item::Fact(_)
        | Item::Msaa { .. }
        | Item::Uia(_)
        | Item::ResolveFocus { .. }
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
        } => format!("MSAA {kind:?} hwnd={hwnd} object={id_object} child={id_child}"),
        Item::Uia(event) => format!("UIA {:?}", event.kind),
        Item::Fact(fact) => format!("fact {:?}", fact.key()),
        Item::Query { query, .. } => format!("query {query:?}"),
        Item::NodesHeld { nodes, .. } => format!("nodes held ({})", nodes.len()),
        Item::ResolveFocus { attempt, .. } => format!("resolve focus (attempt {attempt})"),
        Item::CaretOf { node_id } => format!("caret of {node_id:?}"),
        Item::Settle(_) => "settle".to_owned(),
        Item::Wake => "wake".to_owned(),
    }
}

/// A UIA focus's parts as NVDA reads them when it announces the focus: its
/// name and role from the event, whose cache NVDA reads them from, and its
/// value, states, and details from `element`, the focused element the
/// outpost read live, as NVDA fetches those afresh when the focus is
/// handled (`docs/parity.md`, "How an outpost turns events into focus
/// reports"). The element has the keyboard focus, as the event said.
fn with_live_reads(fact: &UiaSnapshotFact, element: &IUIAutomationElement) -> UiaSnapshotFact {
    // `element` was read with the base cache request.
    let live = snapshot_parts_from_cached_element(element);
    UiaSnapshotFact {
        runtime_id: fact.runtime_id.clone(),
        role: fact.role,
        name: fact.name.clone(),
        value: live.value,
        states: live.states.with(State::Focused),
        details: live.details,
    }
}

/// A focus that is not reported, for the reason logged where it was found.
struct Dropped;

/// The element the registry holds under a focus's runtime id.
enum Held {
    /// The element, resolved in this apartment.
    Element(IUIAutomationElement),
    /// The element could not be resolved.
    Unresolved,
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
            } => self.resolve_focus(&runtime_id, trace, attempt),
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
        match text_reads::check_watch(self.context, node_id, watch, false) {
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
        let context = self.context;
        if context.caret_read_since(node_id, self.timing.observed_at_us) {
            return false;
        }
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
        match text_reads::check_watch(context, node_id, &open.watch, caret_event) {
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
        for registration in [context.focus_properties.get(), context.text_events.get()]
            .into_iter()
            .flatten()
        {
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
        // following typing.
        if let Some(caret) = caret {
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
    fn terminal_output(&mut self, node_id: NodeId, trace: TraceId, observed_at_ms: u64) {
        self.terminal_event(node_id, Event::TextChanged, None, (trace, observed_at_ms));
    }

    /// Asks for the caret of a newly reported focus that may have text, or
    /// whose role says it may (Core waits to hear which before it speaks
    /// the line or the value), and moves the subscription to caret and text
    /// changes to it (to nothing for a focus without text, or one read
    /// through MSAA, whose caret events come from the hooks).
    fn follow_text(&self, (node_id, role): (NodeId, Role), trace: TraceId) {
        text_reads::forget_no_text(self.context, node_id);
        let has_text = self.follow_text_events((node_id, role));
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

    /// Moves the subscription to caret and text changes to `node` when it
    /// may have text and is read through UIA, and to nothing otherwise
    /// (an edit control's caret events come from the hooks). Returns
    /// whether the node may have text.
    fn follow_text_events(&self, (node_id, role): (NodeId, Role)) -> bool {
        let has_text = text_reads::may_have_text(self.context, node_id, role);
        if let Some(subscription) = self.context.text_events.get() {
            let element = has_text
                .then(|| self.context.uia_registry.element_of(node_id))
                .flatten();
            subscription.retarget(match element {
                Some(element) => verbatim_uia::Scope::Elements(vec![element]),
                None => verbatim_uia::Scope::Nothing,
            });
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
            self.report_foreign_window(trace, observed_at_ms, hwnd);
        }
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
        let followed = self.context.uia_registry.element_of(node_id);
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
                // Move the focus-following UIA property subscription to the
                // new focus, without waiting: selective registration, as
                // NVDA's, on the focus alone, whose changes are the only
                // ones the reducer acts on. A focus read through MSAA has no
                // element and is followed by the hooks, and a foreground
                // report is a window, not the control focus is in.
                if let Some(subscription) = self.context.focus_properties.get() {
                    subscription.retarget(following(followed));
                }
            }
            let mut tracking = self.context.tracking();
            tracking.role = Some(role);
            tracking.batch = Some(self.batch);
            tracking.foreground_since_focus = foreground;
            if !foreground {
                tracking.reported += 1;
                tracking.focus = Some(node_id);
                tracking.focus_states = Some(states);
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
            if !foreground {
                self.context.intake.set_judged(self.judged_ancestors());
            }
            if let Some(node) = text_node {
                self.follow_text((node.id, node.role), trace);
            }
        }
    }

    /// The last focus this outpost reported, with its ancestors.
    fn focus_chain(&self) -> Vec<NodeSnapshot> {
        self.context.tracking().chain.clone()
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
        if self.edit_event(kind, (hwnd, id_object, id_child), trace, observed_at_ms) {
            return;
        }
        let Some(object) = verbatim_ia2::acquire::event_object(hwnd, id_object, id_child) else {
            return;
        };
        let registry = &self.context.msaa_registry;
        if kind == WinEventKind::Selection {
            // A tree view item selected on its way to the focus, from one of
            // its children, is one of the focus's logical ancestors only:
            // NVDA, whose ancestors are reached through `accParent`, sees it
            // as neither the focus nor an ancestor, and says nothing of it.
            if object
                .which_of(&self.logical_ancestors(), registry)
                .is_some()
            {
                tracing::debug!(
                    hwnd,
                    id_object,
                    id_child,
                    "MSAA selection dropped: a logical ancestor of the focus only"
                );
                return;
            }
            let node = object.read(registry, Purpose::Announce);
            let event = NormalizedEvent::SelectionChanged { node };
            self.emit(trace, observed_at_ms, Backend::Msaa, Some(hwnd), event);
            return;
        }
        let focus = self.focus_if_spoken(kind, &object);
        if kind == WinEventKind::ValueChange {
            self.value_change(object, focus, (hwnd, trace, observed_at_ms));
            return;
        }
        let Some(focus) = focus else {
            return;
        };
        let node = object.read(registry, Purpose::Context);
        let event = match kind {
            WinEventKind::NameChange => NormalizedEvent::PropertyChanged {
                node_id: node.id,
                change: PropertyChange::Name(node.name),
                child_count: None,
            },
            WinEventKind::DescriptionChange => NormalizedEvent::PropertyChanged {
                node_id: node.id,
                change: PropertyChange::Description(node.details.description),
                child_count: None,
            },
            WinEventKind::StateChange => NormalizedEvent::PropertyChanged {
                node_id: node.id,
                child_count: self.expanded_child_count(&node, focus, (hwnd, id_child)),
                change: PropertyChange::States(node.states),
            },
            _ => return,
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

    /// The number of children of `node`, a Win32 tree view item at
    /// `(hwnd, id_child)`, when its state change newly expands it and it is
    /// the focus, for Core to say how many it holds ("How many items an
    /// expanded tree view item holds" in `docs/nvda/speech.md`). Whether it
    /// was expanded is known from the focus's states as last read, which
    /// this change then updates.
    fn expanded_child_count(
        &self,
        node: &NodeSnapshot,
        focus: NodeId,
        (hwnd, id_child): (isize, i32),
    ) -> Option<u32> {
        if node.id != focus {
            return None;
        }
        let was_expanded = self
            .context
            .tracking()
            .focus_states
            .replace(node.states)
            .is_some_and(|states| states.contains(State::Expanded));
        (node.role == Role::TreeItem && node.states.contains(State::Expanded) && !was_expanded)
            .then(|| verbatim_ia2::acquire::tree_view_child_count(hwnd, id_child))
            .flatten()
    }

    /// A value change on `object`, the focus when `focus` says so: spoken
    /// as a value change only for the focus, and reported as a progress
    /// bar's whether or not it is the focus, as NVDA's progress bar
    /// behavior reports one (`docs/nvda/object-model.md`, "How a progress
    /// bar reports its value"). An object that is not the focus has its
    /// role read alone first, and is read only when it is a progress bar.
    fn value_change(
        &mut self,
        mut object: verbatim_ia2::acquire::EventObject,
        focus: Option<NodeId>,
        (hwnd, trace, observed_at_ms): (isize, TraceId, u64),
    ) {
        if focus.is_none() && object.role() != Role::ProgressBar {
            return;
        }
        let (node, visible) =
            object.read_with_visibility(&self.context.msaa_registry, Purpose::Context);
        let event = if node.role == Role::ProgressBar && visible {
            NormalizedEvent::ProgressChanged { node }
        } else if focus.is_some() {
            NormalizedEvent::ValueChanged {
                node_id: node.id,
                value: node.value,
            }
        } else {
            return;
        };
        self.emit(trace, observed_at_ms, Backend::Msaa, Some(hwnd), event);
    }

    /// The focus's node, when a change of `kind` on `object` is spoken: a
    /// name, description, or value change only for the focus, and a state
    /// change for the focus or one of its ancestors, as NVDA speaks them.
    /// Any other object is not read: it is told from those by its identity
    /// first, without reading any of its properties.
    fn focus_if_spoken(
        &self,
        kind: WinEventKind,
        object: &verbatim_ia2::acquire::EventObject,
    ) -> Option<NodeId> {
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
        object
            .which_of(&candidates, &self.context.msaa_registry)
            .map(|_| focus)
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

    /// The focus's ancestors whose state changes [`Self::focus_if_spoken`]
    /// judges against it and may report, by their own addresses, for the
    /// intake to keep ahead of a focus change they were observed before
    /// (`intake::State::judged`): those known at their own address, which
    /// an event can name, and neither logical nor the foreground window.
    fn judged_ancestors(&self) -> Vec<Object> {
        let tracking = self.context.tracking();
        let registry = &self.context.msaa_registry;
        let Some((_, ancestors)) = tracking.chain.split_last() else {
            return Vec::new();
        };
        ancestors
            .iter()
            .filter(|node| {
                registry.at_address(node.id) == Some(true)
                    && !self.is_logical_ancestor(node.id)
                    && !self.is_foreground_window(node.id, tracking.foreground_window)
            })
            .filter_map(|node| registry.key_of(node.id))
            .map(|(hwnd, id_object, id_child)| Object::Msaa(hwnd, id_object, id_child))
            .collect()
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
            None => match self.live_focus_element(&focus_id, 0) {
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
            DeliveredFact::Show {
                hwnd,
                id_object,
                id_child,
            } => self.tooltip_shown(hwnd, id_object, id_child, trace, observed_at_ms),
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
        if super::window::parent_class(hwnd).as_deref() != Some("ToastChildWindowClass") {
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

    /// Reports the top-level window of `hwnd`, a focus's window, as the
    /// foreground, read as a foreground report reads it, when another
    /// process owns that window and it is the foreground window: a console
    /// window, which Windows names as its shell's, around the console
    /// host's text area, or the Settings app's frame, `ApplicationFrameHost`'s,
    /// around its content. The window's own outpost reports it on the
    /// foreground change, but nothing orders the two outposts, and a window
    /// reported after a focus inside it is not announced: reported here,
    /// just before the focus, it reaches Core first whichever outpost is
    /// quicker, and the reducer announces it once (`reduce_focus_changed`,
    /// a foreground report for the window already holding the focus says
    /// nothing).
    fn report_foreign_window(&mut self, trace: TraceId, observed_at_ms: u64, hwnd: isize) {
        let top = top_level_of(hwnd);
        if top == 0
            || window_owner(top).1 == self.context.target_pid
            || window_belongs_to_hidden_frame(top)
            || !window_is_foreground(top)
        {
            return;
        }
        let (backend, node) = read::foreground_window(self.context, self.client, top);
        tracing::debug!(
            hwnd = top,
            name = ?node.name,
            "the foreground window of another process reported before its focus"
        );
        self.emit_focus(
            trace,
            observed_at_ms,
            backend,
            Some(top),
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
        // NVDA's limiter handles only the newest focus of everything that
        // arrived since it last ran. A focus in the same window that arrived
        // while this one was read would have been in its batch, as a tree
        // view's focus on its item follows the window's own focus by a
        // moment, and the newer one is what the focus is now.
        if self.context.intake.msaa_focus_waiting(hwnd) {
            tracing::debug!(
                hwnd,
                id_object,
                id_child,
                "MSAA focus dropped: a newer focus in its window is waiting"
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

    /// A UIA focus fact. What the focus is comes from the event, as NVDA
    /// builds the focus from the event's sender, and only when the event
    /// says the element has the keyboard focus (NVDA's
    /// `shouldAllowUIAFocusEvent`): its name and role from the event's
    /// cache, and its value, states, and details from the focused element
    /// read live, as NVDA reads those afresh when it handles the focus
    /// ([`with_live_reads`]). The element itself is in the listener's
    /// process and cannot cross to this one, so the outpost finds its own
    /// copy, for those reads, the ancestors, and navigation, by reading the
    /// focused element, with a short wait: an application busy starting up
    /// can leave that read unanswered for more than ten seconds, or answer
    /// with UIA's stand-in for its window (Windows 11 Notepad's text area as
    /// a nameless edit). Without it the focus is still reported, from the
    /// event alone, with its ancestors unknown, and a follow-up finds the
    /// element later for the focus-following property subscription.
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
        let element = match self.live_focus_element(&fact.runtime_id, fact_hwnd) {
            LiveFocus::Found(element) => Some(element),
            LiveFocus::InAnotherApplication => {
                tracing::debug!("UIA focus dropped: the focus is in another application now");
                return;
            }
            LiveFocus::Elsewhere => {
                // Focus has moved on within the application since the event
                // was raised, and the newer focus's own event reports it:
                // NVDA ignores a focus event whose element no longer has the
                // keyboard focus (`shouldAllowUIAFocusEvent`, since 2027.1),
                // or a container is announced after the item a key moved
                // into. Nothing reads it again: the application's next focus
                // event is the evidence of where the focus is.
                tracing::debug!("UIA focus dropped: another element has the keyboard focus");
                return;
            }
            LiveFocus::Unresolved => None,
        };
        let element_us = reading.elapsed().as_micros();
        // The element the registry holds under this runtime id, if it holds
        // one: an application can give a dead element's id to a new one.
        let held = element
            .as_ref()
            .and_then(|_| self.held_element(&fact.runtime_id));
        let previous = self.focus_chain();
        // The window the focus is in, known without a call: the event's
        // own, else the application's keyboard focus window when the
        // listener captured the event. A remote operation is tried unless
        // that window's elements cannot be imported into one.
        let focus_in = [fact_hwnd, focus_window]
            .into_iter()
            .find(|&hwnd| hwnd != 0);
        let enriching = Instant::now();
        let held_element = match &held {
            Some(Held::Element(element)) => Some(element),
            Some(Held::Unresolved) | None => None,
        };
        let (remote, held_focused) =
            match self.remote_enrichment(element.as_ref(), held_element, &previous, focus_in) {
                Some(read::RemoteEnrichment::NotFocused) => {
                    // Read live in the same round trip: the focus moved on after
                    // the focused element was read, as for `LiveFocus::Elsewhere`.
                    tracing::debug!("UIA focus dropped: the element lost the keyboard focus");
                    return;
                }
                Some(read::RemoteEnrichment::Read {
                    enrichment,
                    window,
                    held_focused,
                }) => (Some((enrichment, window)), held_focused),
                None => (None, None),
            };
        let remote_window = remote.as_ref().and_then(|(_, window)| *window);
        let Ok(reported) =
            self.uia_focus_window((fact_hwnd, focus_window), remote_window, element.as_ref())
        else {
            return;
        };
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
        if let Some(held) = held {
            self.reissue_unless_focused(&fact.runtime_id, held, held_focused);
        }
        let parts = with_live_reads(fact, &element);
        let node = Self::uia_node(context, &parts, Some(&element));
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

    /// The window a UIA focus is reported in, or [`Dropped`] when the focus
    /// is in Core's hidden frame or in a window MSAA owns.
    fn uia_focus_window(
        &self,
        (fact_hwnd, focus_window): (isize, isize),
        remote_window: Option<isize>,
        element: Option<&IUIAutomationElement>,
    ) -> Result<Option<isize>, Dropped> {
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
            remote_window
                .or_else(|| element.and_then(nearest_window_handle))
                .or((focus_window != 0).then_some(focus_window))
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
    /// one and a client to read it with, with `held`, the element the
    /// registry holds under the same runtime id.
    fn remote_enrichment(
        &mut self,
        element: Option<&IUIAutomationElement>,
        held: Option<&IUIAutomationElement>,
        previous: &[NodeSnapshot],
        focus_in: Option<isize>,
    ) -> Option<read::RemoteEnrichment> {
        let element = element?;
        let uia = self.client.uia()?;
        let cache = self.context.uia_cache(uia).ok()?;
        read::uia_remote_enrichment(self.context, uia, &cache, element, held, previous, focus_in)
    }

    /// The element the registry holds under `runtime_id`, `None` when no
    /// node with an element has that id.
    fn held_element(&self, runtime_id: &[i32]) -> Option<Held> {
        let registry = &self.context.uia_registry;
        let agile = registry
            .existing_id(runtime_id)
            .and_then(|id| registry.element_of(id))?;
        Some(agile.resolve().map_or(Held::Unresolved, Held::Element))
    }

    /// Gives `runtime_id` a new node unless `held`, the element its node
    /// stands for, still has the keyboard focus: `held_focused` when the
    /// remote enrichment read it, else read now. NVDA treats a focus event
    /// as a duplicate only while the element it compares equal to still has
    /// the keyboard focus, read live; a held element that has lost it, or
    /// cannot be read (it died), is not this focus whatever its runtime id
    /// says.
    fn reissue_unless_focused(
        &mut self,
        runtime_id: &[i32],
        held: Held,
        held_focused: Option<bool>,
    ) {
        let keeps = match held {
            Held::Element(held) => held_focused.unwrap_or_else(|| self.held_has_focus(&held)),
            Held::Unresolved => false,
        };
        if !keeps {
            tracing::debug!(
                "UIA focus: its runtime id named an element without the focus; a new node is issued"
            );
            drop(self.context.uia_registry.reissue(runtime_id));
        }
    }

    /// Whether `held` has the keyboard focus, read live within
    /// [`FOCUS_READ_WAIT`]: the classic path's check, where no remote
    /// operation read it. A read that fails or does not answer in time is
    /// an element that is gone.
    fn held_has_focus(&mut self, held: &IUIAutomationElement) -> bool {
        self.client.uia().is_some_and(|uia| {
            matches!(
                uia.within(FOCUS_READ_WAIT, |_| held.has_keyboard_focus()),
                Ok(Ok(true))
            )
        })
    }

    /// The live element for the focus `runtime_id` names, read as the
    /// focused element within [`FOCUS_READ_WAIT`]. When the focused element
    /// read is another element of this application and the focus's element
    /// is a window of its own, `own_window` (0 for a windowless element),
    /// that window's element is read and is the focus if it is the same
    /// element and has the keyboard focus, read live: NVDA accepts a UIA
    /// focus event whose own element has the keyboard focus
    /// (`shouldAllowUIAFocusEvent`), whatever the focused element read says.
    fn live_focus_element(&mut self, runtime_id: &[i32], own_window: isize) -> LiveFocus {
        let Some(uia) = self.client.uia() else {
            return LiveFocus::Unresolved;
        };
        let Ok(cache) = self.context.uia_cache(uia) else {
            return LiveFocus::Unresolved;
        };
        let read = &self.context.focused_element;
        let Ok(Ok(element)) = uia.within(FOCUS_READ_WAIT, |uia| read(uia, &cache)) else {
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
        } else if let Some(own) = self.own_element_focused(runtime_id, own_window) {
            LiveFocus::Found(own)
        } else {
            LiveFocus::Elsewhere
        }
    }

    /// The element of `own_window`, the window of the focus `runtime_id`
    /// names, when it is that element and has the keyboard focus, read
    /// within [`FOCUS_READ_WAIT`]; `None` otherwise, and for no window.
    fn own_element_focused(
        &mut self,
        runtime_id: &[i32],
        own_window: isize,
    ) -> Option<IUIAutomationElement> {
        if own_window == 0 {
            return None;
        }
        let uia = self.client.uia()?;
        let cache = self.context.uia_cache(uia).ok()?;
        let read = uia.within(FOCUS_READ_WAIT, |uia| {
            let element = uia.element_from_handle(own_window, &cache).ok()?;
            // Built with the base cache request.
            let same = snapshot_parts_from_cached_element(&element).runtime_id == runtime_id;
            (same && matches!(element.has_keyboard_focus(), Ok(true))).then_some(element)
        });
        let element = read.ok().flatten()?;
        tracing::debug!(
            "UIA focus: the focused element read named another element, but the event's own element has the keyboard focus"
        );
        Some(element)
    }

    /// Queues a follow-up that finds the live element of the focus
    /// `runtime_id` names, reported from its event alone, for the
    /// focus-following property subscription.
    fn resolve_focus_later(&self, runtime_id: &[i32], trace: TraceId, attempt: u32) {
        if attempt > FOCUS_RESOLVE_ATTEMPTS {
            tracing::debug!("the focus's element was not found; its changes are not followed");
            return;
        }
        self.context.intake.push(Entry {
            item: Item::ResolveFocus {
                runtime_id: runtime_id.to_vec(),
                attempt,
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
    fn resolve_focus(&mut self, runtime_id: &[i32], trace: TraceId, attempt: u32) {
        let context = self.context;
        if context.intake.focused() != Some(Object::Uia(runtime_id.to_vec())) {
            return; // Focus has moved on.
        }
        let element = self.client.uia().and_then(|uia| {
            let cache = self.context.uia_cache(uia).ok()?;
            (self.context.focused_element)(uia, &cache).ok()
        });
        let Some(element) = element.filter(|element| {
            // Built with the base cache request.
            snapshot_parts_from_cached_element(element).runtime_id == runtime_id
        }) else {
            self.resolve_focus_later(runtime_id, trace, attempt + 1);
            return;
        };
        let id = context.uia_registry.id_for_element(runtime_id, &element);
        if let Some(subscription) = context.focus_properties.get() {
            subscription.retarget(following(context.uia_registry.element_of(id)));
        }
        // The focus was reported without its element, so its text could not
        // be read: its caret and text events are followed from now on, and
        // its caret reported, whose line Core speaks for the focus.
        let role = context.tracking().role;
        if let Some(role) = role {
            self.follow_text((id, role), trace);
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
                read::describe_dialogs(
                    context,
                    client,
                    window.chain(focus),
                    (&previous, &mut None, false),
                );
                // Core takes the answer's control as its focus, and a caret
                // key there is answered by the control's caret events, so
                // they are followed as a reported focus's are.
                if let Some(control) = &answer.focus {
                    self.follow_text_events((control.node.id, control.node.role));
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
}
