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
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use verbatim_ia2::acquire::Purpose;
use verbatim_ia2::{CHILDID_SELF, WinEventKind};
use verbatim_model::{
    Backend, CallCounts, CaretWatch, NodeId, NodeSnapshot, NormalizedEvent, PropertyChange, Role,
    State, StateSet, TerminalOutput, TextOp, TextReply, TraceId,
};
use verbatim_uia::{
    Cached, Fill, HelpTextRefresh, Kept, Node, SelectedItem, SuggestionsRefresh,
    nearest_window_handle,
};
use windows::Win32::UI::Accessibility::{
    UIA_ControllerForPropertyId, UIA_HelpTextPropertyId, UIA_NamePropertyId,
    UIA_RangeValueValuePropertyId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use crate::protocol::{
    DeliveredFact, EventTiming, FocusedControl, OutpostToSupervisor, Query, QueryOutcome,
    QueryResult, UiaFocusRead, UiaSnapshotFact, WALK_DEADLINE, now_us,
};

use super::Context;
use super::acceptance::{self, EventName, SystemWindows};
use super::alarm::Alarm;
use super::focus_path::{self, FocusPath};
use super::gap::{self, FocusGap};
use super::intake::{Entry, Handling, Item, Object, UiaEvent, UiaKind, window_of};
use super::read::{self, Client, ReadError};
use super::text_reads::{self, CARET_WATCH_BOUND, OpenWatch};
use super::window::{
    MOVED_ON_GRACE, console_client, event_window_unreadable, focus_window_of,
    foreground_window_handle, front_is_another_thread_of_its_application, is_descendant_window,
    is_shell_transition_window, now_ms, top_level_of, window_belongs_to_hidden_frame, window_facts,
    window_is_hidden_frame,
};
use crate::arbitration::window_class_name;
use crate::terminal::keys;
use crate::terminal::reading::{Action, Event, Reading};
use crate::terminal::{Found, Memory, ReadMode};
use crate::text::Watched;

/// The deadline for handling an event, a focus, or a focus-now query: NVDA's
/// `NORMAL_CORE_ALIVE_TIMEOUT` (`watchdog.py`), the time NVDA waits for an
/// application that is slow to answer before cancelling the call. An
/// application that is starting up, or the Start menu's search window as it
/// opens, can take two or three seconds to answer a read that then succeeds
/// (found live in the end-to-end suite); NVDA announces the focus late, and
/// a shorter deadline here dropped it for good.
const HANDLING_DEADLINE: Duration = Duration::from_secs(10);

/// How long the worker waits for a UIA focus after a `WinEvent` focus in a
/// UIA window before it reads its application's focus itself
/// (`docs/design/listener-without-connections.md`, decision 11; Fable,
/// 2026-10-21, pending Dickson's acceptance), counted from the observation
/// of the first such focus no UIA focus has answered, not from when the
/// worker took it up. A UIA focus follows its `WinEvent`
/// within 1 to 35 ms in Settings, and in the same millisecond at the median
/// in File Explorer; a newly started File Explorer process's first one came
/// 222 to 514 ms after the first item's `WinEvent` in
/// `explorer_folder_window`, and 515 to 2,694 ms after the process's first
/// focus `WinEvent` in a UIA window in 55 new windows of
/// `measure_focus_explorer` (2026-10-21), all but two under a second; when
/// UI Automation's delivery fails none comes, in Settings for 14.5 seconds.
/// A read made before a late UIA focus costs one read and nothing heard
/// twice, since Core takes the late focus as a duplicate of the read's; a
/// read made late leaves the user in silence. So the bound is a second
/// rather than above every late delivery measured.
const UIA_FOCUS_BOUND: Duration = Duration::from_secs(1);

/// The deadline for one navigation step or an activation.
const STEP_DEADLINE: Duration = Duration::from_millis(400);

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
    /// The watchdog's wake: set for each entry's next check, cancelled when
    /// the entry is published, and woken for a foreground fact or the
    /// shutdown, so the watchdog wakes only when there is something to judge.
    alarm: Alarm,
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
    /// Windows raises once the change is made (`docs/design/phase6-design.md`,
    /// "Foreground events against the foreground window"). The alarm's event
    /// stays set until the watchdog takes it, so the wake is never lost.
    pub(super) fn foreground_changed(&self) {
        self.alarm.wake();
    }

    /// Starts the worker's deadline for one entry concerning `window` (0 for
    /// none), and sets the alarm for its first check, which wakes no
    /// thread: the watchdog wakes only if the entry is still running then.
    fn start(&self, deadline: Duration, running: Option<(u64, TraceId)>, window: isize) {
        let mut state = self.lock();
        let now = Instant::now();
        state.deadline = Some(now + deadline);
        state.started = Some((now, window));
        state.running = running;
        self.alarm.set(next_check(now, now + deadline, now, window));
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
        self.alarm.cancel();
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
    /// How many focuses and foreground changes this outpost has reported,
    /// or focuses it recognized as the focus already, so the worker can tell
    /// when an entry of a focus group was settled.
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
    /// When the last focus this outpost reported was observed, in
    /// milliseconds since the Unix epoch, or when it was reported, for a
    /// focus with no observation time.
    focus_observed_at_ms: u64,
    /// The newest observation time of the focuses Core queued from another
    /// application after this outpost's ([`Tracking::focus_queued_elsewhere`]).
    queued_elsewhere_at_ms: u64,
    /// Whether the last MSAA focus reported was a simple child, by the
    /// child id its object was acquired with: a child id can be reused, so
    /// a focus event at its address is never a duplicate of it.
    focus_was_simple_child: bool,
    /// The dialog the last foreground report gathered text for, which the
    /// focus that follows it reuses.
    dialog: read::DialogMemo,
    /// The last focus's top-level window, when it was the foreground
    /// window as the focus entered it: the window NVDA holds the
    /// foreground object for ([`Worker::is_foreground_window`]).
    foreground_window: Option<isize>,
    /// The last focus or foreground change this outpost reported, which
    /// NVDA's foreground tests compare a foreground change with
    /// ([`foreground_repeats_focus`]).
    queued: Option<QueuedFocus>,
    /// The UIA focus whose value is its first selected item's name, as
    /// the listener read it (`UiaFocusRead::value_from_selection`): the
    /// selection of one of its items is a change of its value
    /// ([`Worker::selection_as_value`]).
    value_from_selection: Option<NodeId>,
}

/// A focus or foreground change an outpost reported, as NVDA's
/// `lastQueuedFocusObject` holds it for `processForegroundWinEvent`: its
/// own window, which may be a child window; for an MSAA focus or a
/// foreground change, the window, object id and child id of the event it
/// came from; and when it was observed, so that a focus Core queued from
/// another application since replaces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QueuedFocus {
    window: isize,
    event: Option<(isize, i32, i32)>,
    observed_at_ms: u64,
}

/// The event address a foreground change has: the system raises its
/// foreground event on the window object, `OBJID_WINDOW` and
/// `CHILDID_SELF`, which the hook takes as the client object, as NVDA's
/// event callback does (`IAccessibleHandler/internalWinEventHandler.py`
/// 104 to 106); the fact forwarded carries the window alone.
fn foreground_event(hwnd: isize) -> (isize, i32, i32) {
    (hwnd, OBJID_CLIENT.0, CHILDID_SELF)
}

/// Why NVDA's `processForegroundWinEvent` drops a foreground change of
/// window `hwnd` that is in front, given `last`, the last focus or
/// foreground change this outpost reported, when no focus Core queued
/// from another application has replaced it (`superseded`), if it does
/// (`IAccessibleHandler/__init__.py` 842 to 867): the last focus's own
/// window is `hwnd` or a child window of it (`is_descendant`, NVDA's
/// `winUser.isDescendantWindow`), or the last focus came from an MSAA
/// event with the foreground event's own window, object id and child id.
/// The second test can be met only where the first is not, by a last focus
/// whose window is not its event's; an outpost records an MSAA focus's
/// event window as its window, so the first decides here.
fn foreground_repeats_focus(
    last: Option<QueuedFocus>,
    superseded: bool,
    hwnd: isize,
    is_descendant: impl Fn(isize, isize) -> bool,
) -> Option<&'static str> {
    let last = last.filter(|_| !superseded)?;
    if is_descendant(hwnd, last.window) {
        Some("the focus is already in the window")
    } else if last.event == Some(foreground_event(hwnd)) {
        Some("it repeats the focus's own event")
    } else {
        None
    }
}

impl Tracking {
    /// The window of the last focus this outpost reported.
    pub(super) fn window(&self) -> Option<isize> {
        self.window
    }

    /// Core queued a focus from another application, observed at
    /// `observed_at_ms` (0 for a focus Core asked for, taken as now), after
    /// the last focus this outpost reported
    /// ([`SupervisorToOutpost::FocusQueued`]).
    pub(super) fn focus_queued_elsewhere(&mut self, observed_at_ms: u64) {
        let observed_at_ms = if observed_at_ms == 0 {
            now_ms()
        } else {
            observed_at_ms
        };
        self.queued_elsewhere_at_ms = self.queued_elsewhere_at_ms.max(observed_at_ms);
    }

    /// Whether a focus Core queued from another application was observed
    /// no earlier than the last focus this outpost reported, so that focus
    /// is no longer the last focus queued, as NVDA's
    /// `lastQueuedFocusObject` is replaced by a focus from any application.
    /// A focus from elsewhere observed before it is one Core took before
    /// this outpost's, or dropped as stale.
    fn focus_superseded(&self) -> bool {
        self.queued_elsewhere_at_ms >= self.focus_observed_at_ms
    }
}

/// Publishes `message`, an entry's one result, if `generation` is still the
/// worker in charge. The check and the end of the entry's deadline happen
/// under the watch lock, so an abandoned worker can never publish after its
/// abandonment, and a published entry can no longer be abandoned: a query
/// never gets a second reply. The message is sent after the lock is
/// released, written at once unless a write is in flight, and sending
/// never waits (`outbound`).
///
/// Every node issued or looked up since the last publish is taken under the
/// same lock, while no other worker can be touching nodes, and recorded as
/// reported at the position the message takes when it is written. That
/// may include nodes the message does not carry, which are then only kept
/// a little longer.
fn publish(context: &Context, generation: u64, message: OutpostToSupervisor) -> bool {
    let Some(touched) = context.watch.publish(generation, || take_touched(context)) else {
        return false;
    };
    context.outbound.publish(message, touched);
    true
}

/// Publishes `messages`, an entry's results, together, as [`publish`]
/// publishes one: in one write when none is in flight
/// (`docs/design/communication.md`, section 11, the decisions of
/// 2026-10-14 for step 6).
fn publish_all(context: &Context, generation: u64, messages: Vec<OutpostToSupervisor>) -> bool {
    let Some(touched) = context.watch.publish(generation, || take_touched(context)) else {
        return false;
    };
    context.outbound.publish_all(messages, touched);
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

/// Starts the first worker and the watchdog. With `warm`, the first worker
/// creates its UIA client at once instead of on its first read.
pub(super) fn start(context: &Arc<Context>, warm: bool) {
    spawn_worker(Arc::clone(context), 0, warm);
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

fn spawn_worker(context: Arc<Context>, generation: u64, warm: bool) {
    let watch = Arc::clone(&context);
    let worker = thread::Builder::new()
        .name("verbatim-worker".to_owned())
        .spawn(move || run(&context, generation, warm))
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
    context.watch.alarm.wake();
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
    loop {
        context.watch.alarm.wait();
        let mut state = context.watch.lock();
        if state.stopping {
            return;
        }
        // An entry published since its alarm went off leaves no deadline.
        let Some(deadline) = state.deadline else {
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
            context
                .watch
                .alarm
                .set(next_check(now, deadline, started, window));
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
        spawn_worker(Arc::clone(context), generation, false);
    }
}

/// The worker's thread: its loop, then the release of the UIA objects
/// `verbatim-uia` kept for this thread, before the thread exits rather than
/// from its thread-local destructors under the loader lock.
fn run(context: &Context, generation: u64, warm: bool) {
    // Held objects are agile references, resolved in this apartment.
    let joined = verbatim_uia::init_mta();
    if let Err(error) = &joined {
        tracing::warn!(%error, "the worker could not join the multithreaded apartment");
    }
    run_loop(context, generation, warm);
    verbatim_uia::release_thread_state();
    if joined.is_ok() {
        verbatim_uia::leave_mta();
    }
}

/// The worker's loop, until the intake closes or this worker is abandoned.
/// With `warm`, the UIA client is created before the first entry: a spare's
/// worker makes it while the spare waits for its application
/// (`docs/design/focus-pipeline.md`, section 19.2).
fn run_loop(context: &Context, generation: u64, warm: bool) {
    let mut client = Client::default();
    if warm {
        let _ = client.uia();
    }
    // The worker's wait for a UIA focus after a `WinEvent` focus in a UIA
    // window, its own and without a lock; a replacement worker starts with
    // none.
    let mut gap = FocusGap::default();
    while let Some((planned, batch, news)) = context
        .intake
        .next_with_gap(gap.wake_at(Instant::now(), UIA_FOCUS_BOUND))
    {
        // The focus markers the intake resolved without a read before
        // these entries: a copy they answer opens no gap. A gap one closes
        // is logged, once per close, and a close is at most once per gap.
        if let Some(closed) = gap.marker_news(news) {
            tracing::debug!(
                hwnd = closed.hwnd,
                observed_at_ms = closed.observed_at_ms,
                "the wait for a UIA focus is closed by the listener's news: a drop that ends it, or the end of its reads"
            );
        }
        let mut run = |entry: Entry, menu: bool| {
            run_entry(
                context,
                (&mut client, &mut gap),
                generation,
                batch,
                entry,
                menu,
            )
        };
        // A focus group's entry counts as reported when the worker reported
        // a focus or foreground change for it, or found it the focus
        // already; no other entry takes the tracking lock for it.
        let outcome = planned.handle(|entry, handling| match handling {
            Handling::Run => run(entry, false).map(|()| false),
            Handling::Menu => run(entry, true).map(|()| false),
            Handling::Tried => {
                let reported = context.tracking().reported;
                run(entry, false)?;
                Ok(context.tracking().reported != reported)
            }
        });
        if outcome.is_err() {
            return;
        }
        answer_idle(context, gap.is_open());
    }
}

/// Answers the idle barrier's query the worker has reached
/// ([`Item::IdleQuery`]) once no deferred work is pending: no caret key's
/// watch is open, no terminal owes Core a read, and the worker waits for
/// no UIA focus after a `WinEvent` focus (`gap_open`), a wait that ends on
/// its UIA focus or with the read that replaces it: at its bound,
/// [`UIA_FOCUS_BOUND`] after the copy's observation, or, when any marker
/// waits for its resolution then, a focus's, a selection's, a
/// notification's or a menu opening's alike, once that marker is
/// resolved, which can take as long as the listener's read of it, up to
/// its deadline (`focus_read::READ_DEADLINE`, 11 seconds), a marker queued
/// behind its application's abandoned reads up to that deadline again, and
/// as long again for each marker that follows while the gap is open, plus
/// whatever entry the worker is handling and the read, if the gap comes to
/// one. For one focus marker on an application that answers that is about
/// 12.2 seconds, within the harness's 15-second idle timeout; a slower
/// application can exceed it, and the harness then fails the step. Called
/// by the worker in charge after each
/// entry, so the answer goes as soon as the entry that ends the watch,
/// pays the read, or closes the gap is handled, behind what that entry
/// published.
fn answer_idle(context: &Context, gap_open: bool) {
    if gap_open || context.idle_query().is_none() {
        return;
    }
    // Neither lock is held across a call into the application, so a worker
    // abandoned in a terminal's read, which holds the terminals' lock,
    // cannot hold the answer back.
    if context.caret_watch().is_some() || !context.owed_reads().is_empty() {
        return;
    }
    let answered = context.idle_query().take();
    if let Some(round) = answered {
        tracing::debug!(round, "the idle barrier's query is answered");
        context
            .outbound
            .send(OutpostToSupervisor::IdleAnswer { round });
    }
}

/// Handles one entry of `batch` under its deadline. `Err` when this worker
/// was abandoned meanwhile and must exit.
fn run_entry(
    context: &Context,
    (client, gap): (&mut Client, &mut FocusGap),
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
    // Formatted only for the debug log, which is all that reads it: an
    // entry handled at info level costs no string.
    let description = tracing::enabled!(tracing::Level::DEBUG).then(|| describe(&entry.item));
    let trace = entry.trace;
    let started = Instant::now();
    let arbitration_started = context.arbitrator().now();
    gap.set_clock_now(arbitration_started);
    let handled = catch_unwind(AssertUnwindSafe(|| {
        let mut worker = Worker {
            context,
            client,
            gap: &mut *gap,
            clock_now: arbitration_started,
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
        gap.set_clock_now(finished);
    }
    // Calls made after the entry's last publish, or by an entry that
    // published nothing: they belong to no trace.
    let unpublished = take_calls();
    let failed_reads = verbatim_uia::calls::take_failed_reads();
    context
        .failed_reads
        .fetch_add(failed_reads, std::sync::atomic::Ordering::Relaxed);
    tracing::debug!(
        %trace,
        item = description.as_deref().unwrap_or_default(),
        elapsed_us = started.elapsed().as_micros(),
        ?unpublished,
        failed_reads,
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
                Query::Ancestors { .. } | Query::DumpTree | Query::ShellItems { .. } => {
                    WALK_DEADLINE
                }
                Query::Text { .. } => TEXT_DEADLINE,
                _ => STEP_DEADLINE,
            };
            (deadline, Some((*request_id, entry.trace)))
        }
        Item::Fact(..)
        | Item::Msaa { .. }
        | Item::Uia(_)
        | Item::CaretOf { .. }
        | Item::NotedCaret { .. }
        | Item::Settle(_)
        | Item::IdleQuery { .. }
        | Item::Wake => (HANDLING_DEADLINE, None),
        // Releasing thousands of objects after a tree dump takes a while.
        Item::NodesHeld { .. } => (WALK_DEADLINE, None),
    }
}

/// A short description of an entry's item, for the debug log of what the
/// worker spent its time on, formatted only when that log is enabled.
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
        Item::Fact(fact, stamp) => format!("fact {:?}", fact.key(*stamp)),
        Item::Query { query, .. } => format!("query {query:?}"),
        Item::NodesHeld { nodes, .. } => format!("nodes held ({})", nodes.len()),
        Item::CaretOf { node_id } => format!("caret of {node_id:?}"),
        Item::NotedCaret { node_id } => format!("noted caret of {node_id:?}"),
        Item::Settle(_) => "settle".to_owned(),
        Item::IdleQuery { round, .. } => format!("idle query {round}"),
        Item::Wake => "wake".to_owned(),
    }
}

/// The name NVDA's acceptance test knows a UIA event of `kind` by, or `None`
/// for one it does not test: a notification and an active text position,
/// which NVDA's handlers for them never test (`UIAHandler/__init__.py`
/// 1063 and 1158). A caret and a text change go through NVDA's automation
/// event handler, which tests them as any other (`docs/parity/focus-events.md`, "Event
/// acceptance").
fn uia_event_name(kind: &UiaKind) -> Option<EventName> {
    match kind {
        UiaKind::Property(id) if *id == UIA_NamePropertyId.0 => Some(EventName::NameChange),
        UiaKind::Property(id) if *id == UIA_HelpTextPropertyId.0 => {
            Some(EventName::DescriptionChange)
        }
        UiaKind::Property(id)
            if *id == UIA_ValueValuePropertyId.0 || *id == UIA_RangeValueValuePropertyId.0 =>
        {
            Some(EventName::ValueChange)
        }
        UiaKind::Property(_) => Some(EventName::StateChange),
        UiaKind::Selection => Some(EventName::Selection),
        UiaKind::TextSelection => Some(EventName::Caret),
        UiaKind::TextChanged => Some(EventName::TextChange),
        UiaKind::Notification(_) | UiaKind::ActiveTextPosition(_) => None,
    }
}

/// The event for a change of property `id` of the node read as `node`,
/// whose element, for a help text or `ControllerFor` change, is
/// `refreshed` with what that change reads ([`refreshed_for`]), or `None`
/// when the change reports nothing.
fn property_event(
    id: i32,
    node: NodeSnapshot,
    refreshed: Option<&Refreshed>,
) -> Option<NormalizedEvent> {
    Some(match id {
        id if id == UIA_NamePropertyId.0 => NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: PropertyChange::Name(node.name),
            child_count: None,
        },
        id if id == UIA_ValueValuePropertyId.0 || id == UIA_RangeValueValuePropertyId.0 => {
            NormalizedEvent::ValueChanged {
                node_id: node.id,
                value: node.value,
            }
        }
        // NVDA maps the help text to a description change
        // (`UIAPropertyIdsToNVDAEventNames`), whose description is the
        // full description or the help text, read when it is handled.
        id if id == UIA_HelpTextPropertyId.0 => NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: PropertyChange::Description(match refreshed {
                Some(Refreshed::HelpText(element)) => element.read().description.description(),
                _ => None,
            }),
            child_count: None,
        },
        id if id == UIA_ControllerForPropertyId.0 => {
            let open = match refreshed {
                Some(Refreshed::Suggestions(element)) => suggestions_open(element)?,
                _ => return None,
            };
            NormalizedEvent::PropertyChanged {
                node_id: node.id,
                change: PropertyChange::Suggestions(open),
                child_count: None,
            }
        }
        _ => NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: PropertyChange::States(node.states),
            child_count: None,
        },
    })
}

/// An element refreshed with what a change of one of its properties reads
/// when it is handled ([`refreshed_for`]).
enum Refreshed {
    /// For a help text change: its description.
    HelpText(Cached<HelpTextRefresh>),
    /// For a `ControllerFor` change: whether it reports suggestions.
    Suggestions(Cached<SuggestionsRefresh>),
}

/// `element` refreshed with what a change of property `id` reads when it is
/// handled, in one call: the description for a help text change, and
/// whether it is editable text or a search field and whether the relation
/// names any element for a `ControllerFor` change. `None` for any other
/// change, which is reported from the event's cache as it stands, and when
/// the refresh fails.
fn refreshed_for(
    uia: Option<&verbatim_uia::Uia>,
    id: i32,
    element: Option<&Cached<Kept>>,
) -> Option<Refreshed> {
    let uia = uia?;
    let live = element?.live();
    if id == UIA_HelpTextPropertyId.0 {
        let request = uia.request_fixed::<HelpTextRefresh>().ok()?;
        live.refresh(&request).ok().map(Refreshed::HelpText)
    } else if id == UIA_ControllerForPropertyId.0 {
        let request = uia.request_fixed::<SuggestionsRefresh>().ok()?;
        live.refresh(&request).ok().map(Refreshed::Suggestions)
    } else {
        None
    }
}

/// For a `ControllerFor` change of `element`, whether its suggestions are
/// now open, or `None` when it does not report them. Only editable text and
/// search fields report suggestions
/// (`EditableTextWithSuggestions.event_controllerForChange`,
/// `NVDAObjects/behaviors.py` 231): whether the relation now names any
/// element, read live by the refresh that read the rest, as NVDA reads
/// `controllerFor` (not in its base cache). A relation the refresh could
/// not read names none.
fn suggestions_open(element: &Cached<SuggestionsRefresh>) -> Option<bool> {
    let refreshed = element.read();
    refreshed
        .reports_suggestions()
        .then_some(refreshed.controls_any)
}

/// A focus that is not reported, for the reason logged where it was found.
struct Dropped;

/// One entry's handling, by the worker in charge.
struct Worker<'a> {
    context: &'a Context,
    client: &'a mut Client,
    /// The wait for a UIA focus after a `WinEvent` focus in a UIA window.
    gap: &'a mut FocusGap,
    /// When the entry began, by the outpost's clock.
    clock_now: Instant,
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
            // Read now that nothing else waits, unless a read since saw it.
            Item::NotedCaret { node_id } => self.caret_of(node_id, trace, observed_at_ms, false),
            Item::Settle(done) => {
                // A gap due is work still to do: its read comes first.
                self.gap_wake();
                self.settle(done, trace);
            }
            Item::IdleQuery {
                round,
                behind_follow_ups,
            } => self.idle_query(round, behind_follow_ups, trace),
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
        // Every update from now on is evidence the watch may need.
        self.context.intake.absorb_console_updates(None);
        self.end_watch("the next caret key replaced it");
        let mut watched = text_reads::check_watch(self.context, node_id, watch, false);
        if let Watched::Answered(reply) = &mut watched
            && self.wrapped_removal(node_id, watch, reply, trace)
        {
            watched = Watched::Watching;
        }
        match watched {
            Watched::Answered(reply) => {
                self.sent_reply_caret(node_id, &reply);
                self.reply(
                    request_id,
                    trace,
                    QueryOutcome::Done(QueryResult::Text(reply)),
                );
            }
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
                self.sent_reply_caret(node_id, &reply);
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
        self.gap_wake();
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

    /// [`Item::Wake`] for the wait for a UIA focus after a `WinEvent` focus
    /// in a UIA window (`gap`): takes in another application's focus Core
    /// has queued, and reads the application's focus once the gap is due
    /// and no marker waits for its read, whatever the wake was for, since
    /// that marker's resolution decides first. The markers the intake
    /// resolved without a read came with the entry ([`run_loop`]).
    fn gap_wake(&mut self) {
        if !self.gap.is_open() {
            return;
        }
        let elsewhere = self.context.tracking().queued_elsewhere_at_ms;
        if elsewhere > 0 {
            self.gap.focus_after(elsewhere);
        }
        if !self.gap.is_due(self.clock_now, UIA_FOCUS_BOUND) || self.context.intake.marker_waiting()
        {
            return;
        }
        let unreadable = self.gap.unreadable();
        if let Some(open) = self.gap.take_due(self.clock_now, UIA_FOCUS_BOUND) {
            self.read_focus_after_gap(open, unreadable);
        }
    }

    /// Reads and reports the application's focus, which no UIA focus
    /// followed the `WinEvent` focus `open` names: the focus-now query's
    /// read (`read::focused_control_in`), reported as a UIA focus is. A UIA
    /// focus of the same element that comes after it is reported under the
    /// same node, which Core takes as a duplicate.
    fn read_focus_after_gap(&mut self, open: gap::Open, unreadable: bool) {
        let context = self.context;
        let waited_ms = self
            .clock_now
            .saturating_duration_since(open.opened)
            .as_millis();
        let observed_at_ms = now_ms();
        let Some((hwnd, control)) = read::focused_control_in(context, self.client) else {
            tracing::info!(
                hwnd = open.hwnd,
                waited_ms,
                unreadable,
                "no UIA focus followed a WinEvent focus in a UIA window; the focus read found none"
            );
            return;
        };
        let FocusedControl {
            node,
            ancestors,
            selected_child,
            ..
        } = control;
        tracing::info!(
            hwnd = open.hwnd,
            waited_ms,
            unreadable,
            role = ?node.role,
            name = ?node.name,
            "no UIA focus followed a WinEvent focus in a UIA window; the focus is read and reported"
        );
        let object = match node.backend {
            Backend::Uia => context.uia_registry.runtime_id_of(node.id).map(Object::Uia),
            Backend::Msaa => context
                .msaa_registry
                .key_of(node.id)
                .map(|(hwnd, object, child)| Object::Msaa(hwnd, object, child)),
        };
        let (node_id, role, backend) = (node.id, node.role, node.backend);
        self.emit_focus(
            TraceId::mint(),
            observed_at_ms,
            backend,
            Some(hwnd),
            node,
            false,
            object,
            (Some(ancestors), selected_child),
        );
        self.follow_focus((node_id, role));
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
        // A noted caret waits for nothing else to be ready, and the settle
        // is: it goes first, and the settle after it.
        if let Some(noted) = context.intake.take_idle() {
            context.intake.push(noted);
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

    /// The idle barrier's query, reached at the front of the intake, so
    /// every entry that arrived before it has been handled. A follow-up one
    /// of them queued, the caret of a focus just reported, is part of that
    /// entry's work, and a noted caret is work owed: the query goes
    /// directly behind them, the noted caret first, and waits for nothing
    /// more when it is reached again (`behind_follow_ups`). Otherwise it is
    /// kept until no deferred work is pending, and answered then
    /// ([`answer_idle`]), at once if none is. Entries that arrived after it
    /// are not waited for, nor are the follow-ups they queue or the carets
    /// they note, so a flood cannot keep the answer back.
    fn idle_query(&self, round: u64, behind_follow_ups: bool, trace: TraceId) {
        let context = self.context;
        if !behind_follow_ups {
            let noted = context.intake.take_idle();
            if noted.is_some()
                || context
                    .intake
                    .any_waiting(|entry| matches!(entry.item, Item::CaretOf { .. }))
            {
                let query = Entry {
                    item: Item::IdleQuery {
                        round,
                        behind_follow_ups: true,
                    },
                    trace,
                    observed_at_ms: now_ms(),
                    timing: EventTiming::default(),
                };
                context
                    .intake
                    .push_behind_follow_ups(noted.into_iter().chain([query]));
                return;
            }
        }
        *context.idle_query() = Some(round);
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
        let terminal = self.focused_terminal(node_id);
        let report =
            text_reads::report_caret(self.context, node_id, after_focus, terminal && !after_focus);
        // A caret event's read, which a terminal's output causes too, as
        // its cursor moves.
        tracing::debug!(
            elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            after_focus,
            "caret read timing"
        );
        let event = match report {
            Ok((_, true)) => {
                tracing::debug!("the terminal's caret is the one last sent; it is not sent again");
                return;
            }
            Ok((caret, false)) => {
                if terminal {
                    self.sent_caret(node_id, &caret);
                }
                NormalizedEvent::CaretMoved { node_id, caret }
            }
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

    /// Whether `node_id` is the focused terminal and its reading is held
    /// with its text changed since its last read
    /// ([`crate::terminal::reading::Reading::Held`]): output is arriving
    /// that nobody reads until Core asks, and the caret moves with it.
    fn held_with_changes(&self, node_id: NodeId) -> bool {
        self.focused_terminal(node_id)
            && self
                .context
                .terminals()
                .get(&node_id.number())
                .is_some_and(|terminal| terminal.reading == Reading::Held { changed: true })
    }

    /// After a console update of the focused console terminal `node_id`
    /// in window `hwnd`: while its reading is held with its text changed
    /// and no caret key's watch is open, the worker would do nothing for
    /// another update, so the intake drops the window's updates without
    /// waking it ([`Intake::absorb_console_updates`]) until a request, a
    /// cancel, a focus, or a caret key clears the mark. A held burst's
    /// updates, about one per line, otherwise woke the worker once each.
    ///
    /// [`Intake::absorb_console_updates`]: super::intake::Intake::absorb_console_updates
    fn absorb_while_held(&self, node_id: NodeId, hwnd: isize) {
        if self.held_with_changes(node_id) && self.context.caret_watch().is_none() {
            self.context.intake.absorb_console_updates(Some(hwnd));
        }
    }

    /// Notes a caret event of the held terminal `node_id` whose text has
    /// changed, instead of reading its caret: during a flood held for Core
    /// it comes with nearly every write, and reading each told Core of a
    /// caret the next write moved again. Its caret is read once nothing else
    /// is waiting ([`Item::NotedCaret`]), so Core's caret, and a review
    /// cursor following it, catch up when the burst ends; the read Core
    /// asks for reads the caret with the text, and covers it if it comes
    /// first. NVDA's console handler likewise queues a caret event only
    /// when one is not already pending (`winConsoleHandler.py`, 169).
    fn note_caret(&self, node_id: NodeId, trace: TraceId, observed_at_ms: u64) {
        tracing::debug!(%trace, "a held terminal's caret event is noted");
        self.context.intake.when_idle(Entry {
            item: Item::NotedCaret { node_id },
            trace,
            observed_at_ms,
            timing: self.timing,
        });
    }

    /// A caret event of the text focus `node_id`, from UIA or, for the
    /// console host's text area, from its console: evidence for an open
    /// caret key's watch, and otherwise the caret reported, or noted while
    /// the terminal's reading is held with its text changed.
    fn caret_event(&mut self, node_id: NodeId, trace: TraceId, observed_at_ms: u64) {
        if self.check_open_watch(node_id, true) {
            return;
        }
        if self.held_with_changes(node_id) {
            self.note_caret(node_id, trace, observed_at_ms);
        } else {
            self.caret_of(node_id, trace, observed_at_ms, false);
        }
    }

    /// Notes `caret` as the caret last sent to Core for the focused
    /// terminal `node_id` ([`crate::terminal::Terminal::sent_caret`]).
    fn sent_caret(&self, node_id: NodeId, caret: &verbatim_model::CaretReport) {
        if self.focused_terminal(node_id) {
            self.context
                .terminals()
                .entry(node_id.number())
                .or_default()
                .sent_caret = Some(caret.clone());
        }
    }

    /// Notes the caret a caret key's answer, `reply`, carries as the caret
    /// last sent to Core for the focused terminal `node_id`.
    fn sent_reply_caret(&self, node_id: NodeId, reply: &TextReply) {
        if let TextReply::Caret(reply) = reply {
            self.sent_caret(node_id, &reply.caret);
        }
    }

    /// The focused terminal, when it is the text area of the console window
    /// `hwnd`, whose console events are its own.
    fn focused_console_terminal(&self, hwnd: isize) -> Option<NodeId> {
        let (focus, window) = {
            let tracking = self.context.tracking();
            (tracking.focus, tracking.window)
        };
        focus
            .filter(|_| window == Some(hwnd))
            .filter(|&node_id| self.focused_terminal(node_id))
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
        if event != Event::TextChanged {
            // Anything else may end the hold, and its read must see every
            // update observed after it begins.
            self.context.intake.absorb_console_updates(None);
        }
        let (action, owed, superseded) = {
            let mut terminals = self.context.terminals();
            let terminal = terminals.entry(node_id.number()).or_default();
            if event == Event::Focused {
                terminal.focus_arrived();
            }
            let (reading, action) = terminal.reading.next(event);
            terminal.reading = reading;
            let mut owed = self.context.owed_reads().remove(&node_id.number());
            // A request asked now takes the place of one owed.
            let superseded = request.and_then(|request| owed.replace(request));
            (action, owed, superseded)
        };
        if let Some(earlier) = superseded {
            self.reply_terminal(earlier, TerminalOutput::default());
        }
        let mode = match action {
            Action::Nothing => {
                if let Some(owed) = owed {
                    self.context.owed_reads().insert(node_id.number(), owed);
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
        let mut caret_moved = None;
        if let Some(caret) = caret {
            self.check_watch_now(node_id, false);
            // A caret Core already has is not sent again, unless the read
            // answers Core's request.
            if caret.unchanged && owed.is_none() {
                tracing::debug!("the terminal's caret is the one last sent; it is not sent again");
            } else {
                self.sent_caret(node_id, &caret.report);
                caret_moved = Some(NormalizedEvent::CaretMoved {
                    node_id,
                    caret: caret.report,
                });
            }
        }
        // A read that reports output as well as the caret sends both in one
        // write, the caret first; anything else the read says follows the
        // caret, sent alone.
        let reports_output = matches!(
            &found,
            Found::Output(output, _)
                if owed.is_none() && action == Action::Report && !output.is_empty()
        );
        if let Some(caret_moved) = caret_moved {
            if reports_output && let Found::Output(output, _) = found {
                self.emit_all(
                    trace,
                    observed_at_ms,
                    Backend::Uia,
                    window,
                    vec![
                        caret_moved,
                        NormalizedEvent::TerminalOutput { node_id, output },
                    ],
                );
                return;
            }
            self.emit(trace, observed_at_ms, Backend::Uia, window, caret_moved);
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
                if let Some(owed) = owed {
                    self.context.owed_reads().insert(node_id.number(), owed);
                }
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
        // A terminal gaining the focus has its caret reported afresh.
        if let Some(terminal) = self.context.terminals().get_mut(&node_id.number()) {
            terminal.sent_caret = None;
        }
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
    /// node may have text, and the base group otherwise; the console host's
    /// text area takes the extended group without its text and caret
    /// events, since its console's updates and caret events tell the
    /// outpost of its output and its cursor ([`text_reads::console_focus`]);
    /// and to nothing for
    /// a focus whose element the outpost does not have, or one read through
    /// MSAA, whose changes and caret events come from the hooks. The element
    /// alone, not its descendants, as the reducer acts on the focus's own
    /// changes. NVDA registers its group with `TreeScope_Ancestors` as well,
    /// but UIA delivers no ancestor's event to such a registration: one on
    /// the Windows 11 taskbar clock's child with that scope did not hear the
    /// clock's own name change each minute, which one on the clock and one
    /// on the taskbar's subtree did (`docs/parity/backends.md`, "UIA event
    /// registration"). A move to the node and group already followed is not
    /// asked of the subscription, so it makes no call; one to the same
    /// element under another node makes none either, as the subscription
    /// compares the elements. Returns whether the node may have text.
    fn follow_focus(&self, (node_id, role): (NodeId, Role)) -> bool {
        let has_text = text_reads::may_have_text(self.context, node_id, role);
        let element = self.context.uia_registry.element_of(node_id);
        let group = if !has_text {
            verbatim_uia::Group::Base
        } else if role == Role::Terminal && text_reads::console_focus(self.context, node_id) {
            verbatim_uia::Group::ExtendedWithoutTextEvents
        } else {
            verbatim_uia::Group::Extended
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
                    subscription.retarget_to(
                        verbatim_uia::Scope::Elements(vec![element.reference()]),
                        group,
                    );
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
            self.event_message(trace, observed_at_ms, backend, window, event),
        )
    }

    /// Emits `events`, an entry's, together, as [`emit`](Self::emit) emits
    /// one: written with one write when none is in flight. The calls made
    /// so far are counted in the first.
    fn emit_all(
        &self,
        trace: TraceId,
        observed_at_ms: u64,
        backend: Backend,
        window: Option<isize>,
        events: Vec<NormalizedEvent>,
    ) -> bool {
        let messages = events
            .into_iter()
            .map(|event| self.event_message(trace, observed_at_ms, backend, window, event))
            .collect();
        publish_all(self.context, self.generation, messages)
    }

    /// `event` as the message that reports it, published now with the
    /// calls made since the last.
    fn event_message(
        &self,
        trace: TraceId,
        observed_at_ms: u64,
        backend: Backend,
        window: Option<isize>,
        event: NormalizedEvent,
    ) -> OutpostToSupervisor {
        OutpostToSupervisor::Event {
            trace_id: trace,
            observed_at_ms,
            backend,
            window: window.map(window_facts),
            application: window.and_then(|hwnd| console_client(hwnd, self.context.target_pid())),
            timing: EventTiming {
                published_at_us: now_us(),
                calls: take_calls(),
                ..self.timing
            },
            event,
        }
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
            // A focus observed after a `WinEvent` focus no UIA focus has
            // answered yet answers it; a foreground change says nothing of
            // the focus inside its window, and does not.
            if !foreground {
                self.gap.focus_after(observed_at_ms);
            }
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
                self.note_focused(object, role, window);
            }
            let mut tracking = self.context.tracking();
            tracking.role = Some(role);
            tracking.batch = Some(self.batch);
            tracking.foreground_since_focus = foreground;
            tracking.reported += 1;
            // NVDA's last queued focus, which every focus and foreground
            // change it queues replaces; an MSAA focus adds its event's
            // address once reported ([`Worker::msaa_focus`]).
            tracking.queued = window.map(|window| QueuedFocus {
                window,
                event: foreground.then(|| foreground_event(window)),
                observed_at_ms: if observed_at_ms == 0 {
                    now_ms()
                } else {
                    observed_at_ms
                },
            });
            if !foreground {
                tracking.focus_observed_at_ms = if observed_at_ms == 0 {
                    now_ms()
                } else {
                    observed_at_ms
                };
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

    /// Tells the intake of the focus just reported, `object` of `role` in
    /// `window`: a terminal read through UIA has its caret event planned
    /// after its text changes, and a console host's terminal, whose
    /// console caret event is its caret event, has the system caret's
    /// events, which tell of the same caret at the same tick, dropped
    /// (`docs/parity/text.md`).
    fn note_focused(&self, object: Option<Object>, role: Role, window: Option<isize>) {
        let terminal = role == Role::Terminal && matches!(object, Some(Object::Uia(_)));
        self.context.intake.set_focused(object);
        if terminal {
            self.context
                .intake
                .set_focus_is_terminal(window.unwrap_or(0));
            if window
                .is_some_and(|hwnd| window_class_name(hwnd) == text_reads::CONSOLE_WINDOW_CLASS)
            {
                self.context.intake.drop_system_caret();
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
        if kind == WinEventKind::ConsoleUpdate {
            // The console host's text changed: its updates are the focused
            // console terminal's text changes, as its UIA text changes are
            // not subscribed to (`docs/crates/verbatim-outpost.md`,
            // "Terminals").
            if let Some(node_id) = self.focused_console_terminal(hwnd) {
                // As for a UIA text change: a caret key's evidence is
                // checked before the read, which reads the caret too and
                // would leave the change nothing new to show.
                self.check_open_watch(node_id, false);
                self.terminal_output(node_id, trace, observed_at_ms);
                self.absorb_while_held(node_id, hwnd);
            }
            return;
        }
        if kind == WinEventKind::ConsoleCaret {
            // The console host's cursor moved: the focused console
            // terminal's caret event, as its UIA caret events are not
            // subscribed to either.
            if let Some(node_id) = self.focused_console_terminal(hwnd) {
                self.caret_event(node_id, trace, observed_at_ms);
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
        // The system caret's event is a caret event on the focus, from
        // whichever window it comes, as NVDA maps it onto its last queued
        // focus. An edit control Core took as its focus from a focus-now
        // answer, with a caret key's watch open on it, is Core's focus
        // rather than the UIA focus this outpost last reported, so its
        // caret events check the watch first.
        if kind == WinEventKind::Caret
            && let Some(Object::Uia(runtime_id)) = self.context.intake.focused()
        {
            if let Some(node_id) = self.watched_edit(hwnd) {
                self.check_open_watch(node_id, true);
            } else {
                self.system_caret_of_uia_focus(&runtime_id, trace, observed_at_ms);
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

    /// The system caret's location change or show, accepted from any
    /// window NVDA's test accepts a location change from: a caret event on
    /// the UIA focus whose `runtime_id` is given, by the path a UIA caret
    /// event takes. NVDA handles these events as a caret event on its last
    /// queued focus whatever API the focus is read through and whichever
    /// window raised them, since some UIA documents, such as Visual
    /// Studio's, raise no UIA caret event of their own. Its two exceptions
    /// are not here: a focus read through IA2, which Verbatim does not read
    /// yet (milestone M7), and Word's UIA document, flooded with them by
    /// Chinese input methods, which belongs to Word's application support
    /// (the roadmap's app-module track, tier D).
    ///
    /// Once the focus has raised a UIA caret event of its own, and in a
    /// focused console terminal, whose console caret event is its caret
    /// event, the system caret's events are not used until the next focus:
    /// they come with the focus's own, as Windows 11 Notepad raises both
    /// for every move, the system caret's first, and each would read the
    /// caret once more for the same move (the move's own event is observed
    /// after that read began, so `Context::caret_read_since` cannot count
    /// the read as covering it). The intake drops them as they arrive
    /// (`Intake::drop_system_caret`); one queued before the mark is
    /// dropped here.
    fn system_caret_of_uia_focus(
        &mut self,
        runtime_id: &[i32],
        trace: TraceId,
        observed_at_ms: u64,
    ) {
        if self.context.intake.system_caret_unused() {
            return;
        }
        if let Some(node_id) = self.context.uia_registry.existing_id(runtime_id) {
            self.caret_event(node_id, trace, observed_at_ms);
        }
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
    /// object, never to the ancestor (`docs/parity/focus-announcement.md`, "A top-level
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
    /// is the focus's, else whether its window is UIA's, then the acceptance
    /// test against the foreground window, and only then the event's own
    /// handling (`IUIAutomationPropertyChangedEventHandler_HandlePropertyChangedEvent`
    /// and its siblings, `source/UIAHandler/__init__.py`). Verbatim makes
    /// the window's UIA test for the focus's own events too, where NVDA
    /// skips it; the focus's window passed it when the focus was taken, so
    /// the outcome is the same.
    fn uia_event(&mut self, event: UiaEvent, trace: TraceId, observed_at_ms: u64) {
        let element = event
            .element
            .as_ref()
            .and_then(super::EventElement::resolve);
        // The event's window: its own, else the recorded window of the focus
        // it concerns, else its element's. Never the system's focus window,
        // which can belong to another application.
        let of_focus =
            self.context.intake.focused() == Some(Object::Uia(event.parts.runtime_id.clone()));
        let hwnd = if event.hwnd != 0 {
            Some(event.hwnd)
        } else if of_focus {
            self.context.tracking().window
        } else if event.focus_window != 0 && element.is_none() {
            // A windowless selection's nearest window, which the listener's
            // reading thread walked up to (`FocusReader::check_fact`).
            Some(event.focus_window)
        } else {
            element
                .as_ref()
                .and_then(|element| nearest_window_handle(element.live().element()))
        };
        // NVDA neither arbitrates nor tests a notification against the
        // foreground: it is spoken only from the focus's application, which
        // Core judges. NVDA's app module does drop one whose element has no
        // window above it, which is not ported yet (roadmap, the audits'
        // items). A terminal's were ignored by the listener
        // (`verbatim_uia::map::ignores_notifications`).
        if !matches!(event.kind, UiaKind::Notification(_)) {
            if let Some(hwnd) = hwnd
                && !read::window_uses_uia(self.context, hwnd)
            {
                return; // MSAA owns this window.
            }
            // A windowless element with no window found has no window to
            // test (0), and is accepted: the listener's nearest window for a
            // selection fact is already among those tried above.
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
        if matches!(event.kind, UiaKind::Selection)
            && let Some(change) = self.selection_as_value(&event.parts)
        {
            self.emit(trace, observed_at_ms, Backend::Uia, hwnd, change);
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
                    if self.is_focus(node_id) {
                        // The focus reports its caret moves itself: the
                        // system caret's events are dropped until the next
                        // focus (`Worker::system_caret_of_uia_focus`).
                        self.context.intake.drop_system_caret();
                    }
                    self.caret_event(node_id, trace, observed_at_ms);
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
        let (parts, element) = self.fresh_selection(&event, element);
        let node = Self::uia_node(self.context, &parts, element.as_ref());
        let normalized = match event.kind {
            UiaKind::Property(id) => {
                let refreshed = refreshed_for(self.client.uia(), id, element.as_ref());
                let Some(normalized) = property_event(id, node, refreshed.as_ref()) else {
                    return;
                };
                normalized
            }
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
        (runtime_id, element): (&[i32], Option<&Cached<Kept>>),
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
    /// When the selected one is inside none, the first controlled element's
    /// current selection is reported in its place
    /// (`verbatim_uia_rops::Controlled::selected`), unless the selection
    /// names the focus itself, which the reducer reads as the focus's own
    /// (`reduce_selection_changed`): NVDA's base `event_selection` finds the
    /// focus in none of the elements it controls and falls through to its
    /// state change handler (`NVDAObjects/__init__.py` 1314 to 1339).
    fn controlled_selection(&mut self, runtime_id: &[i32]) -> Option<NormalizedEvent> {
        // The focus this outpost last reported, as NVDA uses its focus
        // object; found by its path when its element is not known.
        let Some(Object::Uia(focus_id)) = self.context.intake.focused() else {
            return None;
        };
        let current_instead = runtime_id != focus_id.as_slice();
        let registry = &self.context.uia_registry;
        let known = registry
            .existing_id(&focus_id)
            .and_then(|id| registry.element_of(id))
            .and_then(|agile| agile.resolve().ok());
        let focused: Cached<Kept> = match known {
            Some(element) => element,
            None => self.element_by_focus_path(&focus_id)?,
        };
        // NVDA reads the relation live for each selection event; the
        // relation and the search under what it names are one remote
        // operation where the focus's window allows one.
        let window = self.context.tracking().window;
        let uia = self.client.uia()?;
        let nodes = self.context.node_request(uia).ok()?;
        let fill = Fill::new(&nodes);
        let query = verbatim_uia_rops::ControlledQuery {
            focused: focused.live().element(),
            selected: runtime_id,
            fill: &fill,
            current_instead,
        };
        let remote = self.context.tries_remote(window);
        let (answer, path) = verbatim_uia_rops::controlled_selection(uia, &query, remote).ok()?;
        if let verbatim_uia_rops::Path::Fallback(error) = &path {
            tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
            if let (verbatim_uia_rops::Error::Import(_), Some(hwnd)) = (error, window) {
                self.context.read_classically(top_level_of(hwnd));
            }
        }
        if answer.selected.is_none() && !answer.controlled.is_empty() {
            // The search ran and did not find the element, and no current
            // selection was read from the first controlled element: it was
            // not asked for, the event named that element, it has no
            // Selection pattern, or nothing is selected in it. What the
            // relation named.
            tracing::debug!(
                selected = ?runtime_id,
                controlled = ?answer.controlled,
                current_instead,
                "a selection in none of the elements the focus controls, and no current selection read from the first"
            );
        }
        let selected = answer.selected?;
        let reported = selected.runtime_id();
        if reported != runtime_id {
            // Not found, so the first controlled element's current
            // selection, for a selection Settings' search suggestions raise
            // for an item it has already replaced
            // (`docs/parity/changes-and-selection.md`).
            tracing::debug!(
                selected = ?runtime_id,
                ?reported,
                controlled = ?answer.controlled,
                "a selection in none of the elements the focus controls; the first one's current selection is reported"
            );
        }
        let registry = &self.context.uia_registry;
        // The controller's runtime id is UIA's own, kept with the element,
        // whatever its cache holds.
        let controller = focused.runtime_id();
        Some(NormalizedEvent::ControlledSelection {
            controller: registry.id_for_element(&controller, &focused),
            node: selected.snapshot(registry),
        })
    }

    /// The selection of the item `parts` names as a change of the focus's
    /// value, when the focus's value is its first selected item's name
    /// (`verbatim_uia::map::value_from_selection`) and the item, which
    /// does not have the keyboard focus, is one of the focus's children:
    /// NVDA's list item, whose state change under such a focused parent
    /// raises a value change on it, read as the selected item's name
    /// (`NVDAObjects/UIA/__init__.py` 2746 to 2767 and 2786 to 2800).
    /// One call finds the item among the focus's children with its name
    /// and whether it has the keyboard focus, both as they are now, as
    /// NVDA reads the item's `hasFocus` and the combo box's value live: the
    /// event's cache can hold no name for an item an application names
    /// late.
    fn selection_as_value(&mut self, parts: &UiaSnapshotFact) -> Option<NormalizedEvent> {
        let focus = {
            let tracking = self.context.tracking();
            tracking
                .focus
                .filter(|&focus| tracking.value_from_selection == Some(focus))?
        };
        let registry = &self.context.uia_registry;
        let focused = registry
            .element_of(focus)
            .and_then(|agile| agile.resolve().ok())?;
        let uia = self.client.uia()?;
        let request = uia.request_fixed::<SelectedItem>().ok()?;
        let Some(child) = uia
            .find_child_by_runtime_id(focused.live(), &parts.runtime_id, &request)
            .ok()
            .flatten()
        else {
            tracing::debug!("a selection under a focus whose value it is not: not its child");
            return None;
        };
        let item = child.read();
        if item.has_focus {
            // Its focus event says it, as NVDA's item with the focus
            // raises no value change.
            tracing::debug!("a selection of the focus's item that has the keyboard focus");
            return None;
        }
        Some(NormalizedEvent::ValueChanged {
            node_id: focus,
            value: item.name(),
        })
    }

    /// The parts and element an event is reported with: its own, but for a
    /// selection the listener heard, which carries the item's identity,
    /// role, and name alone, NVDA's base cache request: its states, value,
    /// and details are read afresh, as NVDA reads them when it handles the
    /// event, from the focus's own subtree, where a selection Core speaks
    /// lies ([`selected_element`](Self::selected_element)); not found
    /// there, it keeps what it came with.
    fn fresh_selection(
        &mut self,
        event: &UiaEvent,
        element: Option<Cached<Kept>>,
    ) -> (UiaSnapshotFact, Option<Cached<Kept>>) {
        let fresh = (matches!(event.kind, UiaKind::Selection) && element.is_none())
            .then(|| self.selected_element(&event.parts.runtime_id))
            .flatten();
        match fresh {
            Some(found) => {
                let parts = found.read().parts(found.runtime_id());
                (
                    UiaSnapshotFact {
                        runtime_id: parts.runtime_id,
                        role: parts.role,
                        name: parts.name,
                        value: parts.value,
                        states: parts.states,
                        details: parts.details,
                    },
                    Some(found.narrow()),
                )
            }
            None => (event.parts.clone(), element),
        }
    }

    /// The element a selection event of the listener's names, `runtime_id`,
    /// found in the subtree of the focus this outpost reported, when it
    /// holds the focus's element, with the cache request the active theme's
    /// details call for: one search, made inside the application. `None`
    /// when the focus's element is not held or the item is not under it.
    fn selected_element(&mut self, runtime_id: &[i32]) -> Option<Cached<Node>> {
        let Some(Object::Uia(focus_id)) = self.context.intake.focused() else {
            return None;
        };
        let registry = &self.context.uia_registry;
        let focused = registry
            .existing_id(&focus_id)
            .and_then(|id| registry.element_of(id))
            .and_then(|agile| agile.resolve().ok())?;
        let uia = self.client.uia()?;
        let nodes = self.context.node_request(uia).ok()?;
        uia.find_by_runtime_id(focused.live(), runtime_id, &nodes)
            .ok()
            .flatten()
    }

    /// A snapshot from a UIA element's cached parts, its id minted by the
    /// registry, which keeps the element when there is one.
    fn uia_node(
        context: &Context,
        parts: &UiaSnapshotFact,
        element: Option<&Cached<Kept>>,
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
            DeliveredFact::UiaFocusRead { read, focus, .. } => {
                self.uia_focus(&read, focus, trace, observed_at_ms);
            }
            // The intake resolves these; none reaches the worker.
            DeliveredFact::UiaFocusMarker { .. }
            | DeliveredFact::UiaFocusDropped { .. }
            | DeliveredFact::UiaFocusReadsEnded => {}
            // The intake unwraps a marked event as it takes it in.
            DeliveredFact::UiaMarked { fact, .. } => self.fact(*fact, trace, observed_at_ms),
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
            menu @ DeliveredFact::MenuPopup { .. } => {
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
    /// follows once the window is in front (`docs/design/phase6-design.md`,
    /// "Foreground events against the foreground window").
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
    ///
    /// A change in front is dropped too when the last focus or foreground
    /// change this outpost reported is in its window or a child window of
    /// it, or came from the change's own event address, and no focus Core
    /// queued from another application has replaced it, as NVDA's two
    /// tests against its last queued focus drop it
    /// ([`foreground_repeats_focus`]); a focus Core took from another
    /// outpost inside the window is left to the reducer, which ignores a
    /// foreground change of the window the focus is already in.
    ///
    /// Within a focus group, a change reported ends the group's tries, and
    /// one dropped lets the next older entry be tried
    /// ([`super::intake::try_newest_first`]), as NVDA's `pumpAll` stops at
    /// the first foreground or focus event processed.
    fn foreground(&mut self, hwnd: isize, trace: TraceId, observed_at_ms: u64) {
        if window_belongs_to_hidden_frame(hwnd) {
            tracing::debug!(hwnd, "foreground dropped: Core's hidden frame");
            return;
        }
        if !self.accepts(EventName::GainFocus, hwnd) || !self.in_front(hwnd) {
            tracing::debug!(hwnd, "foreground dropped: not the foreground window");
            return;
        }
        // NVDA's two tests against the last focus it queued, before it makes
        // the event's object: local calls only, and the change is not
        // counted as reported, so the group's next older entry is tried.
        let repeats = {
            let tracking = self.context.tracking();
            let superseded = tracking
                .queued
                .is_some_and(|last| tracking.queued_elsewhere_at_ms >= last.observed_at_ms);
            foreground_repeats_focus(tracking.queued, superseded, hwnd, is_descendant_window)
        };
        if let Some(reason) = repeats {
            tracing::debug!(hwnd, reason, "foreground dropped: the focus already");
            return;
        }
        // NVDA makes the foreground's object as it makes any MSAA event's
        // (`processForegroundWinEvent` through `winEventToNVDAEvent`): a
        // window gone, ghosted, or hung is dropped before the UIA test.
        if !Self::readable(hwnd) {
            return;
        }
        if read::window_uses_uia(self.context, hwnd) {
            tracing::debug!(hwnd, "foreground dropped: UIA owns the window");
            return;
        }
        self.report_foreground(hwnd, trace, observed_at_ms);
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
            // UIA owns this window; its UIA fact reports the focus, which
            // the worker waits for (`gap`), counted from the copy's
            // observation: the entry's start by the system's clock and by
            // the outpost's place it on the latter.
            let opened_gap = self.gap.dropped_for_uia(
                (hwnd, top_level_of(hwnd)),
                observed_at_ms,
                (self.clock_now, self.timing.dequeued_at_us / 1_000),
            );
            tracing::debug!(hwnd, opened_gap, "MSAA focus dropped: UIA owns the window");
            return;
        }
        // A control taking the focus raised focus on itself and then on its
        // focused child, and the control's event was already reported as
        // the child (`verbatim_ia2::acquire::focus_candidate`): NVDA reports
        // the child once when the two events fall in one of its pumps,
        // trying them newest first; when the child's comes in a later pump,
        // NVDA reports the control and then the child (`docs/parity/windows-and-processes.md`,
        // "A control's own focus with a focused child").
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
        // queued, after acquiring the object and making the list redirect,
        // unless that last focus was a simple child, by the child id its
        // object was acquired with, whose id can be reused
        // (`isDuplicateIAccessibleEvent`, `IAccessibleChildID > 0`). NVDA
        // compares with the last focus it queued from any application
        // (`lastQueuedFocusObject`), so the last focus this outpost reported
        // stops counting after its own foreground change, and once Core has
        // queued a newer focus from another application, which Core tells
        // this outpost (`SupervisorToOutpost::FocusQueued`).
        let (address_hwnd, address_object, address_child) = candidate.key();
        let resolved_child = candidate.resolved_child();
        let still_last = {
            let tracking = self.context.tracking();
            !tracking.focus_was_simple_child
                && !tracking.foreground_since_focus
                && !tracking.focus_superseded()
        };
        let repeated = still_last
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
        // states after the duplicate test, reading them live.
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
        if self.context.intake.focused() == object {
            let mut tracking = self.context.tracking();
            tracking.focus_was_simple_child = resolved_child > 0;
            if let Some(queued) = tracking.queued.as_mut() {
                queued.event = Some((hwnd, id_object, id_child));
            }
            drop(tracking);
            if redirected {
                self.context.intake.set_focus_redirected();
            }
        }
    }

    /// A UIA focus as the focus listener judged and read it from the
    /// event's own element (`docs/design/focus-pipeline.md`, section 12):
    /// it was no repeat of the last focus accepted, its nearest window
    /// passed NVDA's acceptance test against the foreground window, the
    /// element had the keyboard focus when the listener read it, as
    /// NVDA's `shouldAllowUIAFocusEvent` requires, and its states, value,
    /// details, ancestors, and selected child are the element's own, read
    /// in one remote operation. The outpost reports it at once, its node
    /// minted from its runtime id; the listener's evidence that the element
    /// it read before under that runtime id has lost the focus or died
    /// gives the id a new node first. Only then, off the way to speech,
    /// does the outpost find its own copy of the element, which no process
    /// can hand it (experiment E-marshal), by the path the listener read
    /// ([`find_focus_element`](Self::find_focus_element)).
    fn uia_focus(&mut self, read: &UiaFocusRead, focus: bool, trace: TraceId, observed_at_ms: u64) {
        let context = self.context;
        // The UIA focus a `WinEvent` focus in a UIA window was waiting for,
        // in the copy's top-level window; a menu opening's read is not one.
        let read_top_level = Some(read.hwnd)
            .filter(|&hwnd| hwnd != 0)
            .or(read.window)
            .map(top_level_of)
            .filter(|&top| top != 0);
        let answered_gap_ms = focus
            .then(|| self.gap.uia_answered(observed_at_ms, read_top_level))
            .flatten()
            .map(|open| {
                self.clock_now
                    .saturating_duration_since(open.opened)
                    .as_millis()
            });
        // NVDA's acceptance test against the foreground window was made by
        // the listener, before it read the focus, as NVDA makes it before
        // it queues one (`docs/design/focus-pipeline.md`, section 16.2).
        let Ok(reported) = self.uia_focus_window(read.hwnd, read.window) else {
            return;
        };
        // The listener took the focus only because its window uses UIA
        // (section 18): this outpost keeps that verdict for the window, so
        // the two decide the window alike and it is not probed again.
        if let Some(window) = read.window {
            context.arbitrator().record_probe(window, true);
        }
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
            read::uia_chain_from_read(context, self.client, (ancestors, read.hwnd), &previous)
        });
        *context.focus_path() = Some(FocusPath::of(read));
        tracing::debug!(
            %trace,
            enrichment_us = reading.elapsed().as_micros(),
            ?answered_gap_ms,
            "UIA focus reported from the listener's read"
        );
        // A focus on the element this outpost already found under its
        // runtime id keeps it: the listener read that it still has the
        // keyboard focus. A plain repeat never comes this far, as the
        // listener drops it (section 16.3); this is the focus coming back
        // to the element after one in another application.
        let found =
            read.same_as_last == Some(true) && context.uia_registry.element_of(node.id).is_some();
        context.tracking().value_from_selection = read.value_from_selection.then_some(node.id);
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
    fn element_by_focus_path(&mut self, runtime_id: &[i32]) -> Option<Cached<Kept>> {
        let context = self.context;
        let route = context.focus_route(runtime_id)?;
        let uia = self.client.uia()?;
        let nodes = context.node_request(uia).ok()?;
        let remote = context.tries_remote(Some(route.window));
        let (found, import_failed) =
            focus_path::find(uia, &route, &nodes, remote, &context.clock());
        if import_failed {
            context.read_classically(route.window);
        }
        // Read as `Kept`: the callers keep it, or read only its runtime id.
        found.map(|found| found.narrow())
    }

    /// The window a UIA focus is reported in, or [`Dropped`] when the focus
    /// is in Core's hidden frame. Whether the window uses UIA was decided
    /// by the listener before it took the focus, as NVDA decides it before
    /// it takes one (`isNativeUIAElement`; `docs/design/focus-pipeline.md`,
    /// section 18), and is not decided again here.
    fn uia_focus_window(
        &self,
        fact_hwnd: isize,
        remote_window: Option<isize>,
    ) -> Result<Option<isize>, Dropped> {
        // The event's own window; else the element's nearest, which the
        // listener's read found with its ancestors and sends with every
        // read; else, for a read without one, which only a test makes, and
        // for judging Core's hidden frame only, this application's focus
        // window now, which is never reported.
        let reported = if fact_hwnd != 0 {
            Some(fact_hwnd)
        } else {
            remote_window
        };
        let judged = reported.or_else(|| focus_window_of(self.context.target_pid()));
        if judged.is_some_and(window_belongs_to_hidden_frame) {
            return Err(Dropped);
        }
        Ok(reported)
    }

    /// Keeps `element`, found for the focus `runtime_id` names, which was
    /// reported from its event alone, under the focus's node, and follows
    /// its changes.
    fn follow_focus_element(&mut self, runtime_id: &[i32], element: &Cached<Kept>, trace: TraceId) {
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
    /// already put focus on a menu or menu item (NVDA's condition is a
    /// valid focus in its pump, a duplicate included, while its last
    /// queued focus is a menu or menu item), when the window belongs to UIA
    /// (NVDA does not use MSAA objects proxied from native UIA), or when its
    /// object is not a popup menu; otherwise it becomes a focus on the
    /// popup menu.
    fn menu_opened(&mut self, entry: &Entry) {
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
            Query::ShellItems { kind } => {
                super::shell_items::shell_items(context, client, *kind).map(QueryResult::ShellItems)
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

    /// A last focus in window `window`, from the MSAA event `event`.
    fn last_focus(window: isize, event: Option<(isize, i32, i32)>) -> QueuedFocus {
        QueuedFocus {
            window,
            event,
            observed_at_ms: 1,
        }
    }

    /// NVDA's first foreground test: a change of the last focus's own
    /// window, or of a window it is a child window of, is dropped; one of
    /// another window is not, and nor is any once a focus from another
    /// application replaced the last focus, or with no last focus.
    #[test]
    fn a_foreground_change_of_the_last_focus_s_window_or_its_parent_is_dropped() {
        // Window 7 is a child window of 5; 6 is not related to either.
        let is_descendant =
            |parent: isize, child: isize| parent == child || (parent, child) == (5, 7);
        let last = Some(last_focus(7, None));
        assert_eq!(
            foreground_repeats_focus(last, false, 7, is_descendant),
            Some("the focus is already in the window")
        );
        assert_eq!(
            foreground_repeats_focus(last, false, 5, is_descendant),
            Some("the focus is already in the window")
        );
        assert_eq!(
            foreground_repeats_focus(last, false, 6, is_descendant),
            None
        );
        assert_eq!(foreground_repeats_focus(last, true, 7, is_descendant), None);
        assert_eq!(
            foreground_repeats_focus(None, false, 7, is_descendant),
            None
        );
    }

    /// NVDA's second foreground test: a change whose event address, the
    /// window with `OBJID_CLIENT` and `CHILDID_SELF`, is the last focus's
    /// own event's is dropped, though the last focus's window is not the
    /// change's; another address is not.
    #[test]
    fn a_foreground_change_with_the_last_focus_s_own_event_address_is_dropped() {
        let unrelated = |parent: isize, child: isize| parent == child;
        assert_eq!(
            foreground_repeats_focus(
                Some(last_focus(8, Some((5, OBJID_CLIENT.0, CHILDID_SELF)))),
                false,
                5,
                unrelated
            ),
            Some("it repeats the focus's own event")
        );
        assert_eq!(
            foreground_repeats_focus(
                Some(last_focus(8, Some((5, OBJID_WINDOW.0, CHILDID_SELF)))),
                false,
                5,
                unrelated
            ),
            None
        );
    }

    /// The idle barrier's answer never waits on the terminals' lock, which
    /// a terminal's read holds across its calls into the application: a
    /// worker abandoned in such a read keeps the lock, and the worker that
    /// replaced it, waiting on that lock with no deadline, would never be
    /// abandoned in turn. The test holds the lock as that read does.
    #[test]
    fn the_idle_answer_does_not_wait_on_the_terminals_lock() {
        use std::io::BufReader;
        use std::sync::mpsc;

        use verbatim_process::overlapped::{End, Flow, pipe};
        use verbatim_process::transport::{BlockingReader, EVENT_PIPE_BUFFER};

        use crate::protocol::{SupervisorToOutpost, read_message};

        let (ours, theirs) =
            pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped).expect("a pipe");
        let outpost = super::super::Outpost::new(theirs, std::process::id());
        let (messages_tx, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(BlockingReader::new(ours).expect("a reader"));
            while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
                if messages_tx.send(message).is_err() {
                    return;
                }
            }
        });
        let wait = Duration::from_secs(10);
        assert!(matches!(
            messages.recv_timeout(wait),
            Ok(OutpostToSupervisor::Ready { .. })
        ));
        let read_in_progress = outpost.context.terminals();
        outpost.handle_command(&SupervisorToOutpost::IdleQuery { round: 4 });
        let answer = messages.recv_timeout(wait);
        drop(read_in_progress);
        assert_eq!(
            answer.ok(),
            Some(OutpostToSupervisor::IdleAnswer { round: 4 })
        );
        outpost.shutdown();
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
