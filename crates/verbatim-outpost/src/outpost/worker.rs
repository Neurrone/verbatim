//! The worker and its watchdog (architecture section 1, "Outposts").
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
    Backend, CallCounts, CaretReport, CaretWatch, EarlierElement, FocusGroup, FocusText, NodeId,
    NodeSnapshot, NormalizedEvent, PropertyChange, Role, State, StateSet, TextOp, TextPoint,
    TextReply, TraceId, WindowFacts,
};
use verbatim_uia::{
    Cached, Fill, HelpTextRefresh, Kept, SelectedItem, SuggestionsRefresh, nearest_window_handle,
};
use windows::Win32::UI::Accessibility::{
    UIA_ControllerForPropertyId, UIA_HelpTextPropertyId, UIA_NamePropertyId,
    UIA_RangeValueValuePropertyId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use crate::protocol::{
    DeliveredFact, EventTiming, FocusNow, FocusedControl, OutpostToSupervisor, Query, QueryOutcome,
    QueryResult, UiaFocusRead, UiaSnapshotFact, WALK_DEADLINE, now_us,
};

use super::Context;
use super::acceptance::{self, EventName, SystemWindows};
use super::alarm::Alarm;
use super::bound::{self, BoundFocus, FocusProgress};
use super::caret_poll::{self, Poll};
use super::commit::{
    self, Committed, Displaced, Effects, Follow, FollowState, Owed, OwedWork, Report, Stale,
    Target, WatchChange,
};
use super::focus_path::{self, FocusPath};
use super::gap_record::{CopyDropped, GapRead};
use super::held;
use super::intake::{Entry, Handling, Item, Object, UiaEvent, UiaKind, window_of};
use super::read::{self, Client, ReadError};
use super::text_reads::{self, CARET_WATCH_BOUND, OpenWatch};
use super::window::{
    MOVED_ON_GRACE, alt_is_held, console_client, event_window_unreadable, focus_window_of,
    foreground_window_handle, front_is_another_thread_of_its_application, now_ms, top_level_of,
    window_belongs_to_hidden_frame, window_exists, window_facts, window_is_hidden_frame,
};
use crate::arbitration::window_class_name;
use crate::terminal::reading::{Action, Event, Reading};
use crate::text::Watched;
use verbatim_core::focus_rules::{self, ChangeKind, HeldNode};

/// The deadline for handling an event, a focus, or a focus-now query: NVDA's
/// `NORMAL_CORE_ALIVE_TIMEOUT` (`watchdog.py`), the time NVDA waits for an
/// application that is slow to answer before cancelling the call. An
/// application that is starting up, or the Start menu's search window as it
/// opens, can take two or three seconds to answer a read that then succeeds
/// (found live in the end-to-end suite); NVDA announces the focus late, and
/// a shorter deadline here dropped it for good.
const HANDLING_DEADLINE: Duration = Duration::from_secs(10);

/// The deadline for one navigation step or an activation.
const STEP_DEADLINE: Duration = Duration::from_millis(400);

/// The deadline for a text request: one read, or a few, of the node's text;
/// a caret key's watch is checked once within it and then kept open
/// outside it.
const TEXT_DEADLINE: Duration = Duration::from_secs(2);

/// The worker generation in charge, its deadline, the walk the watchdog
/// may publish, and the follow state every commit changes.
#[derive(Default)]
pub(super) struct WatchState {
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
    /// The focus walk of the entry in hand, once armed: the watchdog
    /// publishes its focus at its bound should it still be under way then,
    /// or before it abandons the walk's worker ([`publication`]).
    walk: Option<FocusProgress>,
    /// How many walks have been armed, which numbers each.
    arms: u64,
    /// The number of the walk of the entry in hand published at its bound.
    bound_published: Option<u64>,
    /// What the outpost follows, changed only in a commit.
    pub(super) follow: FollowState,
}

impl WatchState {
    /// Abandons the worker in charge: the next generation takes over, owed
    /// work the abandoned worker was attempting is no longer owed, and the
    /// query it was running, if any, is returned for an "abandoned" reply.
    fn abandon(&mut self) -> Option<(u64, TraceId)> {
        let abandoned = self.generation;
        self.generation += 1;
        self.deadline = None;
        self.started = None;
        self.end_walk();
        if self
            .follow
            .owed
            .as_ref()
            .is_some_and(|owed| owed.attempting == Some(abandoned))
        {
            self.follow.owed = None;
        }
        self.running.take()
    }

    /// The entry in hand has ended: no walk of it is watched.
    fn end_walk(&mut self) {
        self.walk = None;
        self.bound_published = None;
    }

    /// When the watchdog next checks the entry in hand, if it has one: its
    /// next check ([`next_check`]), or its walk's bound when that is sooner.
    fn next_wake(&self, now: Instant) -> Option<Instant> {
        let (deadline, (started, window)) = (self.deadline?, self.started?);
        let check = next_check(now, deadline, started, window);
        Some(
            self.walk
                .as_ref()
                .map_or(check, |walk| check.min(walk.bound_at)),
        )
    }
}

/// How a focus walk that its worker armed returned
/// ([`Watch::walk_returned`]).
pub(super) enum Returned {
    /// Before its bound, or never armed: its worker reports it.
    Disarmed,
    /// Published at its bound: its worker sends nothing else for it.
    Published,
    /// Past its bound and still armed, as the arming numbered here: its
    /// worker publishes it, as the watchdog would.
    PastBound(u64),
    /// Its worker was abandoned.
    Abandoned,
}

/// Shared between the worker, its watchdog, and anything that publishes.
#[derive(Default)]
pub(super) struct Watch {
    state: Mutex<WatchState>,
    /// The watchdog's wake: set for each entry's next check, cancelled when
    /// the entry ends, and woken for a foreground fact or the shutdown, so
    /// the watchdog wakes only when there is something to judge.
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

    /// Reads the follow state with `read`, under the watch lock: never
    /// called with the watch lock held, nor with any lock a commit takes
    /// after it (`super::commit`).
    pub(super) fn follow<T>(&self, read: impl FnOnce(&FollowState) -> T) -> T {
        read(&self.lock().follow)
    }

    /// Forgets the terminals and the caret key's watch with no generation
    /// check: only for the outpost's shutdown, once no worker runs.
    pub(super) fn reset_follow(&self) {
        let mut state = self.lock();
        let terminals = std::mem::take(&mut state.follow.terminals);
        let leftover = std::mem::take(&mut state.follow.leftover);
        let followed = state.follow.followed_element.take();
        state.follow.caret_watch = None;
        drop(state);
        // Outside the lock: they may hold an application's objects.
        drop(terminals);
        drop(leftover);
        drop(followed);
    }

    /// How many abandoned workers have not yet returned.
    pub(super) fn abandoned(&self) -> usize {
        self.abandoned.load(Ordering::Relaxed)
    }

    /// Wakes the watchdog to judge again whether the user has moved on from
    /// the worker's window: a foreground fact reached the outpost, which
    /// Windows raises once the change is made (`docs/crates/verbatim-outpost.md`,
    /// "Foreground changes and focus-now"). The alarm's event
    /// stays set until the watchdog takes it, so the wake is never lost.
    pub(super) fn foreground_changed(&self) {
        self.alarm.wake();
    }

    /// Starts the worker's deadline for one entry concerning `window` (0 for
    /// none), and sets the alarm for its first check, which wakes no
    /// thread: the watchdog wakes only if the entry is still running then.
    fn start(&self, deadline: Duration, running: Option<(u64, TraceId)>, window: isize) {
        let mut state = self.lock();
        Self::start_locked(&mut state, &self.alarm, deadline, running, window);
    }

    fn start_locked(
        state: &mut WatchState,
        alarm: &Alarm,
        deadline: Duration,
        running: Option<(u64, TraceId)>,
        window: isize,
    ) {
        let now = Instant::now();
        state.deadline = Some(now + deadline);
        state.started = Some((now, window));
        state.running = running;
        state.end_walk();
        alarm.set(next_check(now, now + deadline, now, window));
    }

    /// Takes up the owed work of the followed focus for `generation`, if it
    /// is in charge and no worker has attempted it: the attempt is marked
    /// as `generation`'s, and its own deadline is started, with no window,
    /// so only the deadline abandons it, never the moved-on rule
    /// (`docs/design/focus-pipeline-rework.md`, "The outpost's commit
    /// point").
    fn take_owed(&self, generation: u64) -> Option<Owed> {
        let mut state = self.lock();
        if state.generation != generation {
            return None;
        }
        let owed = state
            .follow
            .owed
            .as_mut()
            .filter(|owed| owed.attempting.is_none())?;
        owed.attempting = Some(generation);
        let owed = owed.clone();
        Self::start_locked(&mut state, &self.alarm, HANDLING_DEADLINE, None, 0);
        Some(owed)
    }

    /// Arms the focus walk `walk` of its worker's entry, if that worker is
    /// still in charge: the watchdog publishes its focus at its bound should
    /// the walk still be under way then, and its alarm is set for the
    /// bound when that comes before the entry's next check, which wakes no
    /// thread. Returns the walk's number.
    fn arm(&self, mut walk: FocusProgress) -> Option<u64> {
        let mut state = self.lock();
        if state.generation != walk.generation {
            return None;
        }
        state.arms += 1;
        walk.armed = state.arms;
        state.walk = Some(walk);
        state.bound_published = None;
        if let Some(wake) = state.next_wake(Instant::now()) {
            self.alarm.set(wake);
        }
        Some(state.arms)
    }

    /// The walk `generation` armed has read `step`. Answers whether
    /// `generation` is still in charge: an abandoned walk stops at its next
    /// step.
    fn advance(&self, generation: u64, step: &read::WalkStep<'_>) -> bool {
        let mut state = self.lock();
        if state.generation != generation {
            return false;
        }
        if let Some(walk) = state
            .walk
            .as_mut()
            .filter(|walk| walk.generation == generation)
        {
            walk.take_in(step);
        }
        true
    }

    /// The walk `generation` armed has returned at `now` ([`Returned`]). A
    /// walk still armed past its bound stays armed, for its worker to
    /// publish; one returned before it is disarmed, and its alarm goes
    /// back to the entry's next check.
    fn walk_returned(&self, generation: u64, now: Instant) -> Returned {
        let mut state = self.lock();
        if state.generation != generation {
            return Returned::Abandoned;
        }
        if let Some(walk) = state.walk.as_ref().filter(|walk| now >= walk.bound_at) {
            return Returned::PastBound(walk.armed);
        }
        if state.walk.take().is_some() {
            if let Some(wake) = state.next_wake(now) {
                self.alarm.set(wake);
            }
            return Returned::Disarmed;
        }
        if state.bound_published.take().is_some() {
            Returned::Published
        } else {
            Returned::Disarmed
        }
    }

    /// The one commit (`super::commit`): if `generation` is in charge,
    /// records `effects`' report for Core's judgement, sends its messages,
    /// and applies the rest, all under the watch lock; otherwise does
    /// nothing. What it displaces, and what a watchdog's publication left,
    /// is dropped once the lock is let go. An entry's result ends its
    /// query, so the query never gets a second reply, but not its deadline:
    /// a call that hangs after it is abandoned as one before it is. Returns
    /// what was done, and the caret key's watch taken out by
    /// [`WatchChange::Take`], whose query is then the one an abandonment
    /// answers.
    pub(super) fn commit(
        &self,
        context: &Context,
        generation: u64,
        effects: Effects,
    ) -> (Committed, Option<OpenWatch>) {
        let mut state = self.lock();
        if state.generation != generation {
            drop(state);
            // Outside the lock: the effects may hold an application's
            // objects.
            drop(effects);
            return (Committed::Refused, None);
        }
        let (taken, mut displaced) = Self::commit_locked(&mut state, context, effects);
        displaced.append(std::mem::take(&mut state.follow.leftover));
        drop(state);
        drop(displaced);
        (Committed::Applied, taken)
    }

    fn commit_locked(
        state: &mut WatchState,
        context: &Context,
        mut effects: Effects,
    ) -> (Option<OpenWatch>, Displaced) {
        match effects.report.take() {
            Some(Report::Reported(focus, evidence)) => context.held().reported(focus, evidence),
            Some(Report::Answered(focus)) => {
                let mut held = context.held();
                let focus = answer_to_judge(held.expected(), focus);
                // An answer of the newest report's node is the same
                // element read again.
                held.reported(focus, held::Evidence::Uia(None));
            }
            None => {}
        }
        if let Some(message) = effects.result.take() {
            state.running = None;
            commit::send(context, message);
        }
        if let Some((request_id, message)) = effects.aside.take() {
            if state
                .running
                .is_some_and(|(running, _)| running == request_id)
            {
                state.running = None;
            }
            commit::send(context, message);
        }
        if matches!(
            effects.watch,
            Some(WatchChange::Open(..) | WatchChange::PutBack(_))
        ) {
            state.running = None;
        }
        let generation = state.generation;
        let (taken, displaced) = state.follow.apply(context, generation, effects);
        if let Some(open) = &taken {
            state.running = Some((open.request_id, open.trace));
        }
        (taken, displaced)
    }

    /// Publishes the walk armed as `armed` at its bound, if it is still the
    /// walk armed by the worker in charge: the walk is claimed, so its
    /// worker's late result is discarded, and its publication
    /// ([`publication`]), made under the lock from the walk's progress as it
    /// stands, so every step it took in before the lock was taken is
    /// published, is recorded, sent, and made the followed focus. Local
    /// calls only. Returns the progress published, and what the commit
    /// displaced, to be dropped once the lock is let go; `None` when the
    /// walk was not armed.
    fn commit_publication(
        state: &mut WatchState,
        context: &Context,
        alarm: &Alarm,
        armed: u64,
    ) -> Option<(FocusProgress, Displaced)> {
        let generation = state.generation;
        let walk = state
            .walk
            .take_if(|walk| walk.generation == generation && walk.armed == armed)?;
        let effects = publication(context, &walk);
        let (_, displaced) = Self::commit_locked(state, context, effects);
        state.bound_published = Some(walk.armed);
        if let Some(wake) = state.next_wake(Instant::now()) {
            alarm.set(wake);
        }
        Some((walk, displaced))
    }

    /// Ends the deadline, if `generation` is still in charge, returning the
    /// query still waiting for its reply, if any. `Err` for an abandoned
    /// worker, which is then no longer counted and must exit.
    fn finish(&self, generation: u64) -> Result<Option<(u64, TraceId)>, ()> {
        let mut state = self.lock();
        if state.generation == generation {
            state.deadline = None;
            state.started = None;
            state.end_walk();
            self.alarm.cancel();
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

/// A focus-now answer's control, which the outpost adopts as its focus and
/// follows once the answer is sent ([`Worker::follow_answer`]).
struct AnsweredFocus {
    /// The control.
    node: NodeSnapshot,
    /// Its ancestors, outermost first.
    ancestors: Vec<NodeSnapshot>,
    /// What its caret read found ([`Worker::read_focus_text`]), and when
    /// that read began.
    text: Option<FocusText>,
    read_at: Option<u64>,
    /// When the answer began to be read, in microseconds on the run's
    /// shared clock.
    observed_at_us: u64,
    /// The window whose focused element the answer read.
    window: Option<isize>,
}

/// A focus-now answer's control, `answered`, as the outpost records it
/// among its reports, given `before`, the newest report: an answer for the
/// same node keeps the MSAA event address and the simple-child mark its
/// report recorded, as NVDA's fake focus leaves its last queued focus's
/// event as it was.
fn answer_to_judge(before: Option<&held::Focus>, answered: held::Focus) -> held::Focus {
    match before.filter(|before| before.node.id == answered.node.id) {
        Some(before) => held::Focus {
            event: before.event,
            simple_child: before.simple_child,
            ..answered
        },
        None => answered,
    }
}

/// The object the focus `node` is, by its backend's registry: its runtime id
/// or its MSAA event address, as the intake records the focus.
fn object_of(context: &Context, node: &NodeSnapshot) -> Option<Object> {
    match node.backend {
        Backend::Uia => context.uia_registry.runtime_id_of(node.id).map(Object::Uia),
        Backend::Msaa => context
            .msaa_registry
            .key_of(node.id)
            .map(|(hwnd, object, child)| Object::Msaa(hwnd, object, child)),
    }
}

/// The event address a foreground change has: the system raises its
/// foreground event on the window object, `OBJID_WINDOW` and
/// `CHILDID_SELF`, which the hook takes as the client object, as NVDA's
/// event callback does (`IAccessibleHandler/internalWinEventHandler.py`
/// 104 to 106); the fact forwarded carries the window alone.
fn foreground_event(hwnd: isize) -> (isize, i32, i32) {
    (hwnd, OBJID_CLIENT.0, CHILDID_SELF)
}

/// The numbers of the nodes issued or looked up since the last take.
pub(super) fn take_touched(context: &Context) -> Vec<u64> {
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
/// the grace ends and whenever a foreground fact reaches the outpost. A
/// focus walk armed when its bound passes, or when an abandonment is due,
/// is published first, in the same hold of the watch lock as the
/// abandonment, so no commit of the worker's falls between them
/// (`docs/design/focus-pipeline-rework.md`, "The outpost's commit point").
fn watchdog(context: &Arc<Context>) {
    loop {
        context.watch.alarm.wait();
        let abandoned = match wake(
            context,
            Instant::now(),
            front_is_another_thread_of_its_application,
        ) {
            Wake::Stop => return,
            Wake::Nothing => continue,
            Wake::Abandoned(abandoned) => abandoned,
        };
        let Abandoned {
            reason,
            running,
            generation,
            count,
        } = abandoned;
        tracing::warn!(
            abandoned = count,
            reason,
            "the worker is abandoned and replaced"
        );
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

/// What one wake of the watchdog did ([`wake`]).
enum Wake {
    /// The outpost is shutting down.
    Stop,
    /// Nothing more than a publication, if one was due.
    Nothing,
    /// The worker in charge was abandoned.
    Abandoned(Abandoned),
}

/// A worker abandoned ([`Wake::Abandoned`]): why, the query it was running,
/// for an "abandoned" reply sent once the lock is let go, and the
/// generation that replaces it.
struct Abandoned {
    reason: &'static str,
    running: Option<(u64, TraceId)>,
    generation: u64,
    /// How many abandoned workers have not yet returned.
    count: usize,
}

/// One wake of the watchdog at `now`, with `moved_on` telling whether the
/// user has left a window for another thread of its application: a walk
/// armed past its bound, or whose worker is due to be abandoned, is
/// published first, and the abandonment, judged again from the same `now`,
/// made in the same hold of the watch lock, so no commit of the worker's
/// falls between them. Nothing is sent of the abandonment while the lock is
/// held: the abandoned worker can no longer commit, and its replacement is
/// not started yet, so its query's answer still comes before anything the
/// replacement says.
fn wake(context: &Context, now: Instant, moved_on: impl Fn(isize) -> bool) -> Wake {
    let mut state = context.watch.lock();
    if state.stopping {
        return Wake::Stop;
    }
    // An entry ended since its alarm went off leaves no deadline.
    if state.deadline.is_none() {
        return Wake::Nothing;
    }
    let judge = |state: &WatchState| {
        let deadline = state.deadline?;
        let (started, window) = state.started.unwrap_or((now, 0));
        abandon_reason(now, deadline, started, window, &moved_on)
    };
    let mut reason = judge(&state);
    let due = state
        .walk
        .as_ref()
        .filter(|walk| reason.is_some() || now >= walk.bound_at)
        .map(|walk| walk.armed);
    if let Some(armed) = due {
        // Local calls only, under the lock, from the walk's progress as it
        // stands.
        if let Some((walk, displaced)) =
            Watch::commit_publication(&mut state, context, &context.watch.alarm, armed)
        {
            tracing::info!(
                trace = %walk.focus.trace,
                name = ?walk.focus.node.name,
                abandoning = reason.is_some(),
                "a focus walk was under way at its bound or its worker's abandonment; the focus is published as read so far"
            );
            // Never dropped on the watchdog's thread: the worker in charge
            // drops it after its next commit.
            state.follow.leftover.append(displaced);
        }
        reason = judge(&state);
    }
    let Some(reason) = reason else {
        if let Some(wake) = state.next_wake(now) {
            context.watch.alarm.set(wake);
        }
        return Wake::Nothing;
    };
    let running = state.abandon();
    // Counted under the lock, before the abandoned worker can return and
    // lower the count.
    let count = context.watch.abandoned.fetch_add(1, Ordering::Relaxed) + 1;
    let generation = state.generation;
    // Under the watch lock, as the abandonment: from now on only the new
    // worker records a verdict.
    context.arbitrator().set_in_charge(generation);
    Wake::Abandoned(Abandoned {
        reason,
        running,
        generation,
        count,
    })
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
/// (`docs/design/focus-pipeline.md`, section 19.2). A worker that replaces
/// an abandoned one first attempts the work the followed focus still owes,
/// when the abandoned worker was not attempting it.
fn run_loop(context: &Context, generation: u64, warm: bool) {
    let mut client = Client::default();
    if warm {
        let _ = client.uia();
    }
    if generation != 0 && owed_first(context, &mut client, generation).is_err() {
        return;
    }
    while let Some((planned, batch)) = context.intake.next() {
        let mut run = |entry: Entry, menu: bool, group: Option<FocusGroup>| {
            run_entry(
                context,
                &mut client,
                generation,
                batch,
                entry,
                (menu, group),
            )
        };
        // A focus group's entries carry its number, counted by this
        // outpost generation, and their rank, newest first, so Core takes
        // no older entry once it took a newer one. An entry ends the group
        // when the worker reported a focus for it, or a foreground change
        // of a window that holds no unreleased node of a report's line,
        // which the entry's worker-local flag says.
        let number = planned.is_focus_group().then(|| context.next_focus_group());
        let outcome = planned.handle(|entry, handling| match handling {
            Handling::Run => run(entry, false, None).map(|_| false),
            Handling::Menu => run(entry, true, None).map(|_| false),
            Handling::Tried(rank) => run(
                entry,
                false,
                number.map(|number| FocusGroup { number, rank }),
            ),
        });
        if outcome.is_err() {
            return;
        }
        answer_idle(context);
    }
}

/// Answers the idle barrier's query the worker has reached
/// ([`Item::IdleQuery`]) once no deferred work is pending: no caret key's
/// watch is open and the gap record is closed, so the gap's read, if one
/// is made, goes ahead of the answer. Called by the worker in charge after
/// each entry, so the answer goes as soon as the entry that ends the watch
/// or closes the record is handled, behind what that entry published.
pub(super) fn answer_idle(context: &Context) {
    if context.idle_query().is_none() {
        return;
    }
    // Neither the watch lock nor the intake's is held across a call into
    // the application, so a worker abandoned in a read cannot hold the
    // answer back.
    if context.watch.follow(|follow| follow.caret_watch.is_some()) || context.intake.gap_open() {
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

/// Handles one entry of `batch` under its deadline, as a menu opening with
/// `menu`, and with `group`, its place in a focus group. `Err` when this
/// worker was abandoned meanwhile and must exit; otherwise whether the
/// entry ended its focus group ([`Worker::reported`]).
fn run_entry(
    context: &Context,
    client: &mut Client,
    generation: u64,
    batch: u64,
    entry: Entry,
    (menu, group): (bool, Option<FocusGroup>),
) -> Result<bool, ()> {
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
    // Every verdict the entry asks for is judged at its event's
    // observation, or, for an entry with no event, at its start
    // (`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated
    // fact", the verdicts).
    let judged_at = judged_at(context, &entry);
    context.arbitrator().begin_entry(judged_at, generation);
    let mut reported = false;
    let handled = catch_unwind(AssertUnwindSafe(|| {
        let mut worker = Worker::new(context, client, generation, batch, timing);
        worker.group = group;
        if menu {
            worker.menu_opened(&entry);
        } else {
            worker.handle(entry);
        }
        reported = worker.reported;
    }));
    context.arbitrator().end_entry();
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
    Ok(reported)
}

/// The first act of the worker `generation`, which replaces an abandoned
/// one: the work the followed focus still owes, when the abandoned worker
/// was not attempting it ([`Worker::attempt_owed`]). `Err` when this
/// worker was abandoned meanwhile and must exit.
fn owed_first(context: &Context, client: &mut Client, generation: u64) -> Result<(), ()> {
    let handled = catch_unwind(AssertUnwindSafe(|| {
        Worker::new(context, client, generation, 0, EventTiming::default()).attempt_owed();
    }));
    // What the attempt sends carries its own calls.
    let _ = take_calls();
    if handled.is_err() {
        tracing::error!("the worker failed attempting the followed focus's owed work");
    }
    context.watch.finish(generation).map(|_| ())
}

/// The observation `entry`'s verdicts are judged at: its event's, for an
/// event the outpost's hook or subscriptions took in or a fact the listener
/// sent; for anything else, which has no event, now, on the arbitrator's
/// observation clock, the entry's start.
fn judged_at(context: &Context, entry: &Entry) -> u64 {
    match entry.item {
        Item::Msaa { .. } | Item::Uia(_) | Item::Fact(..) if entry.observed_at_us != 0 => {
            entry.observed_at_us
        }
        _ => context.observed_now(),
    }
}

/// The deadline for an entry, and the query it answers, if it is one.
fn budget(entry: &Entry) -> (Duration, Option<(u64, TraceId)>) {
    match &entry.item {
        Item::Query { request_id, query } => {
            let deadline = match query {
                Query::FocusNow => HANDLING_DEADLINE,
                Query::Ancestors { .. }
                | Query::DumpTree { .. }
                | Query::ShellItems { .. }
                | Query::ShellItemPosition { .. } => WALK_DEADLINE,
                Query::Text { .. } => TEXT_DEADLINE,
                _ => STEP_DEADLINE,
            };
            (deadline, Some((*request_id, entry.trace)))
        }
        Item::Fact(..)
        | Item::Msaa { .. }
        | Item::Uia(_)
        | Item::NotedCaret { .. }
        | Item::Settle(_)
        | Item::IdleQuery { .. }
        | Item::Wake
        | Item::GapBound
        | Item::GapRead { .. } => (HANDLING_DEADLINE, None),
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
        Item::NotedCaret { node_id } => format!("noted caret of {node_id:?}"),
        Item::Settle(_) => "settle".to_owned(),
        Item::IdleQuery { round, .. } => format!("idle query {round}"),
        Item::Wake => "wake".to_owned(),
        Item::GapBound => "the gap record's bound".to_owned(),
        Item::GapRead { window } => format!("the gap record's read, window {window}"),
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

/// Whether a change of UIA property `id` is reported as a change of the
/// element's states ([`property_event`]): every property but the name, the
/// value, the range value, the help text and `ControllerFor`, such as the
/// expand state and the toggle state.
pub(super) fn is_state_property(id: i32) -> bool {
    ![
        UIA_NamePropertyId.0,
        UIA_ValueValuePropertyId.0,
        UIA_RangeValueValuePropertyId.0,
        UIA_HelpTextPropertyId.0,
        UIA_ControllerForPropertyId.0,
    ]
    .contains(&id)
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
        id if is_state_property(id) => NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: PropertyChange::States(node.states),
            child_count: None,
        },
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
        _ => return None,
    })
}

/// An element refreshed with what a change of one of its properties reads
/// when it is handled ([`refreshed_for`]).
enum Refreshed {
    /// For a help text change: its description.
    HelpText(Cached<HelpTextRefresh>),
    /// For a `ControllerFor` change: whether it reports suggestions and
    /// whether its relation names any element.
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
    generation: u64,
    /// The batch the entry belongs to.
    batch: u64,
    /// When the entry was raised, observed, relayed, and dequeued, for the
    /// latency log.
    timing: EventTiming,
    /// The MSAA event address of the focus about to be reported, and
    /// whether its object was acquired as a simple child, which the focus
    /// is recorded with for Core's judgement ([`Worker::focus_to_judge`]).
    msaa_event: Option<((isize, i32, i32), bool)>,
    /// Whether the entry ended its focus group: it reported a focus, or a
    /// foreground change of a window that holds no unreleased node of a
    /// report's focus line. A focus group's older entries are then not
    /// tried.
    reported: bool,
    /// The entry's place in its focus group, which its report carries
    /// (`verbatim_model::FocusGroup`); `None` for an entry of no group.
    group: Option<FocusGroup>,
    /// Whether one of this worker's commits was refused: it was abandoned,
    /// and makes no further call.
    refused: std::cell::Cell<bool>,
    /// While the worker attempts a terminal's owed first read, its node:
    /// the commit of the read's result marks it done.
    owed_attempt: Option<NodeId>,
    /// A caret read made for owed work, committed with what follows it.
    pending_caret_read: Option<(NodeId, u64)>,
    /// Whether the focus about to be reported was reached by redirecting
    /// a control's own focus event to its focused child.
    redirected: bool,
    /// The listener's mark, for the UIA focus about to be reported, that
    /// its value is its first selected item's name: `None` leaves the mark
    /// as it was, as every report but a UIA focus's does.
    #[expect(
        clippy::option_option,
        reason = "a mark to set, to clear, or to leave as it was"
    )]
    value_from_selection: Option<Option<NodeId>>,
    /// The focus path of the UIA focus about to be reported, which its
    /// entry finds its element by before it is committed.
    focus_path: Option<FocusPath>,
    /// What the listener read of the element it held before under the UIA
    /// focus about to be reported, which its report carries.
    earlier: Option<EarlierElement>,
    /// The element kept for the UIA focus about to be reported that the
    /// listener's evidence says is not its element, evicted by the
    /// report's commit ([`Stale`]).
    stale: Option<Stale>,
}

impl<'a> Worker<'a> {
    fn new(
        context: &'a Context,
        client: &'a mut Client,
        generation: u64,
        batch: u64,
        timing: EventTiming,
    ) -> Self {
        Self {
            context,
            client,
            generation,
            batch,
            timing,
            msaa_event: None,
            reported: false,
            group: None,
            refused: std::cell::Cell::new(false),
            owed_attempt: None,
            pending_caret_read: None,
            redirected: false,
            value_from_selection: None,
            focus_path: None,
            earlier: None,
            stale: None,
        }
    }
}

impl Worker<'_> {
    /// Commits `effects` for this worker ([`Watch::commit`]).
    fn commit(&self, effects: Effects) -> Committed {
        let (committed, _) = self
            .context
            .watch
            .commit(self.context, self.generation, effects);
        if committed == Committed::Refused {
            self.refused.set(true);
        }
        committed
    }

    /// Whether this worker was abandoned, as a refused commit told it.
    fn refused(&self) -> bool {
        self.refused.get()
    }

    fn handle(&mut self, entry: Entry) {
        let Entry {
            item,
            trace,
            observed_at_us,
            ..
        } = entry;
        match item {
            Item::Msaa {
                kind,
                hwnd,
                id_object,
                id_child,
                ..
            } => self.msaa_event(kind, hwnd, id_object, id_child, trace, observed_at_us),
            Item::Uia(event) => self.uia_event(event, trace, observed_at_us),
            Item::Fact(fact, _) => self.fact(fact, trace, observed_at_us),
            Item::Query { request_id, query } => self.query(request_id, &query, trace),
            Item::NodesHeld {
                nodes,
                acknowledged,
            } => self.release(&nodes, acknowledged),
            // Read now that nothing else waits, unless a read since saw it.
            Item::NotedCaret { node_id } => self.caret_of(node_id, trace, observed_at_us),
            Item::Settle(done) => self.settle(done, trace),
            Item::IdleQuery {
                round,
                behind_noted,
            } => self.idle_query(round, behind_noted, trace),
            Item::Wake => self.wake(),
            Item::GapBound => self.gap_bound(trace),
            Item::GapRead { window } => self.gap_read(
                GapRead {
                    window,
                    stamp_us: observed_at_us,
                },
                trace,
            ),
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
        if self.refused() {
            return;
        }
        let (watched, read_at) = text_reads::check_watch(self.context, node_id, watch, false);
        let caret_read = read_at.map(|at| (node_id, at));
        match watched {
            Watched::Answered(reply) => {
                let sent_caret = self.reply_caret(node_id, &reply);
                let result = self.reply_message(
                    request_id,
                    trace,
                    QueryOutcome::Done(QueryResult::Text(reply)),
                );
                self.commit(Effects {
                    result: Some(result),
                    sent_caret,
                    caret_read,
                    ..Effects::default()
                });
                // Core's reading rules answer the key from a read that shows
                // what it did, taken now whatever read came before.
                if !self.refused() && self.focused_terminal(node_id) {
                    self.terminal_output(node_id, trace, 0);
                }
            }
            Watched::Watching => {
                let calls = take_calls();
                let opened = Instant::now();
                // The console host raises its caret event only on its
                // cursor's blink timer, so its caret is re-read meanwhile
                // (`super::caret_poll`).
                let poll = text_reads::console_focus(self.context, node_id)
                    .then(|| Poll::new(opened, watch.pressed_at_ms, now_ms()));
                let open = OpenWatch {
                    request_id,
                    trace,
                    node_id,
                    watch: watch.clone(),
                    opened,
                    timing: self.timing,
                    calls,
                    poll,
                };
                let wake = caret_poll::wake_for(&open);
                // The query is answered later, outside its entry, so an
                // abandonment no longer answers it.
                self.commit(Effects {
                    watch: Some(WatchChange::Open(open, wake)),
                    caret_read,
                    ..Effects::default()
                });
                if !self.refused() {
                    tracing::debug!(%trace, ?calls, polled = poll.is_some(), "a caret key's watch is open");
                }
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
    /// for it (Windows Terminal, `PSReadLine`'s menu). The watch is taken
    /// out, its query made the one an abandonment answers, in one commit,
    /// and put back or answered in another.
    fn check_watch_now(&mut self, node_id: NodeId, caret_event: bool) -> bool {
        let context = self.context;
        let (committed, taken) = context.watch.commit(
            context,
            self.generation,
            Effects {
                watch: Some(WatchChange::Take(node_id)),
                ..Effects::default()
            },
        );
        if committed == Committed::Refused {
            self.refused.set(true);
        }
        let Some(mut open) = taken else {
            return false;
        };
        let (watched, read_at) =
            text_reads::check_watch(context, node_id, &open.watch, caret_event);
        let caret_read = read_at.map(|at| (node_id, at));
        match watched {
            Watched::Watching => {
                open.calls += take_calls();
                self.commit(Effects {
                    watch: Some(WatchChange::PutBack(open)),
                    caret_read,
                    ..Effects::default()
                });
                false
            }
            Watched::Answered(reply) => {
                let sent_caret = self.reply_caret(node_id, &reply);
                let calls = take_calls();
                tracing::debug!(
                    trace = %open.trace,
                    waited_ms = open.opened.elapsed().as_millis(),
                    "a caret key's watch is answered"
                );
                let answer = OutpostToSupervisor::Reply {
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
                };
                self.commit(Effects {
                    aside: Some((open.request_id, answer)),
                    watch: Some(WatchChange::Wake(None)),
                    sent_caret,
                    caret_read,
                    ..Effects::default()
                }) == Committed::Applied
            }
        }
    }

    /// The node of the open caret key's watch, when it is the client area
    /// of the edit control `hwnd`.
    fn watched_edit(&self, hwnd: isize) -> Option<NodeId> {
        let node_id = self
            .context
            .watch
            .follow(|follow| follow.caret_watch.as_ref().map(|open| open.node_id))?;
        (self.context.msaa_registry.key_of(node_id) == Some((hwnd, OBJID_CLIENT.0, CHILDID_SELF)))
            .then_some(node_id)
    }

    /// Ends the open caret key's watch, if there is one, without evidence:
    /// answered `WatchEnded`, for which Core says nothing. `why` is logged.
    /// A worker abandoned meanwhile leaves it to the worker that replaces
    /// it.
    fn end_watch(&mut self, why: &'static str) {
        self.commit(Effects {
            watch: Some(WatchChange::End(why)),
            ..Effects::default()
        });
    }

    /// [`Item::Wake`]: re-reads the console host's caret for the open caret
    /// key's watch when its poll's read is due, ending the watch when the
    /// last read finds no evidence, as at its own bound (`super::caret_poll`);
    /// ends the open watch once it has reached [`CARET_WATCH_BOUND`], which
    /// only frees it and says nothing; and otherwise asks to be woken at the
    /// open watch's next read or bound. A worker abandoned in the poll's
    /// read leaves the watch its replacement opened alone.
    fn wake(&mut self) {
        self.poll_caret();
        if self.refused() {
            return;
        }
        let wake = self.context.watch.follow(|follow| {
            follow
                .caret_watch
                .as_ref()
                .map(|open| (open.opened + CARET_WATCH_BOUND, caret_poll::wake_for(open)))
        });
        match wake {
            Some((bound, _)) if Instant::now() >= bound => self.end_watch("its bound passed"),
            Some((_, wake)) => {
                self.commit(Effects {
                    watch: Some(WatchChange::Wake(Some(wake))),
                    ..Effects::default()
                });
            }
            None => {}
        }
    }

    /// The open caret key's watch's poll read, when one is due: answers the
    /// key on evidence, reading the terminal then, ends the watch when the
    /// read was the poll's last, and otherwise sets the poll's next read.
    fn poll_caret(&mut self) {
        let now = Instant::now();
        let due = self.context.watch.follow(|follow| {
            follow
                .caret_watch
                .as_ref()
                .filter(|open| open.poll.is_some_and(|poll| now >= poll.next))
                .map(|open| (open.node_id, open.request_id, open.trace))
        });
        let Some((node_id, request_id, trace)) = due else {
            return;
        };
        if self.check_watch_now(node_id, false) {
            // Core's reading rules answer the key from a read that shows
            // what it did, as after a first check that answers it.
            if self.focused_terminal(node_id) {
                self.terminal_output(node_id, trace, 0);
            }
            return;
        }
        if self.refused() {
            return;
        }
        self.commit(Effects {
            watch: Some(WatchChange::Polled(request_id, Instant::now())),
            ..Effects::default()
        });
    }

    /// The gap record's bound's entry ([`Item::GapBound`]): the record, still
    /// open with its bound passed by the observation clock, is read; one
    /// whose bound has not passed yet, as when the timer fired a little
    /// early, has its timer set again for the rest.
    fn gap_bound(&mut self, trace: TraceId) {
        if let Some(read) = self.context.gap_bound_reached() {
            self.gap_read(read, trace);
        }
    }

    /// The gap's read (`docs/design/focus-pipeline-rework.md`, "Who owns
    /// each duplicated fact", the gap's evidence): no UIA focus answered a
    /// `WinEvent` focus this outpost dropped because it judged its window
    /// UIA, the record's `read.window`, by its bound, or a UIA focus whose
    /// sender has no window above it came beside it. The application's
    /// focus is read through UIA only, once, with the focus-now query's read
    /// and its checks of the foreground thread's focus window before and
    /// after (`read::gap_focus_read`), and reported as a UIA focus is,
    /// stamped with the record's stamp, the newest copy or foreground change
    /// it took, so Core's stale rule drops it when a focus observed after
    /// that, in a window the user has gone to, was applied first. A UIA
    /// focus of the same element that comes after the read is reported
    /// under the same node, which Core takes as a duplicate. Its log says
    /// whether a listener UIA focus had reached the outpost before the
    /// record's stamp, to the microsecond (`uia_focus_before`), and whether
    /// the record's window was gone as the read began (`sender_gone`),
    /// which together tell late delivery from a burst of gone senders; a
    /// read that found no focus also logs how long it took (`elapsed_us`)
    /// and, when a failed call gave one, its error's `HRESULT` (`error`).
    fn gap_read(&mut self, read: GapRead, trace: TraceId) {
        let context = self.context;
        let GapRead { window, stamp_us } = read;
        let waited_ms = now_us().saturating_sub(stamp_us) / 1_000;
        let first_uia_focus_us = context.first_uia_focus_us.load(Ordering::Relaxed);
        let uia_focus_before = first_uia_focus_us != 0 && first_uia_focus_us < stamp_us;
        let sender_gone = !window_exists(window);
        // Its walk is published at its bound, measured from the read's
        // start, should a call of it still be under way then.
        let (timing, batch) = (self.timing, self.batch);
        let watched = walk_watcher(context, self.generation, |node, window| BoundFocus {
            trace,
            observed_at_us: stamp_us,
            timing,
            batch,
            backend: node.backend,
            window: Some(window),
            node: node.clone(),
            object: object_of(context, node),
            msaa_event: None,
            group: None,
        });
        let started = Instant::now();
        let found = read::gap_focus_read(
            context,
            self.client,
            (context.observed_now(), Some(&watched)),
        );
        let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        if self.walk_returned() {
            return;
        }
        let Ok((hwnd, control)) = found else {
            let miss = found.err();
            let error = match miss {
                Some(read::FocusReadMiss::Failed(Some(code))) => {
                    Some(format!("{:#010x}", code.0.cast_unsigned()))
                }
                _ => None,
            };
            tracing::info!(
                %trace,
                window,
                waited_ms,
                uia_focus_before,
                sender_gone,
                ?miss,
                error,
                elapsed_us,
                "no UIA focus answered a WinEvent focus in a UIA window; the gap's read found none"
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
            %trace,
            window,
            focus_window = hwnd,
            waited_ms,
            uia_focus_before,
            sender_gone,
            role = ?node.role,
            name = ?node.name,
            "no UIA focus answered a WinEvent focus in a UIA window; the focus is read and reported"
        );
        let object = object_of(context, &node);
        let backend = node.backend;
        self.emit_focus(
            trace,
            stamp_us,
            backend,
            Some(hwnd),
            node,
            false,
            object,
            (ancestors, selected_child),
        );
    }

    /// Answers [`Outpost::settle`](super::Outpost::settle) once nothing
    /// else is waiting, a noted caret included: otherwise it goes to the
    /// back of the queue again.
    fn settle(&self, done: std::sync::mpsc::Sender<()>, trace: TraceId) {
        let context = self.context;
        if context.intake.busy() {
            context.intake.push_outside_gap(Entry {
                item: Item::Settle(done),
                trace,
                observed_at_us: now_us(),
                timing: EventTiming::default(),
            });
            return;
        }
        // A noted caret waits for nothing else to be ready, and the settle
        // is: it goes first, and the settle after it.
        if let Some(noted) = context.intake.take_idle() {
            context.intake.push_outside_gap(noted);
            context.intake.push_outside_gap(Entry {
                item: Item::Settle(done),
                trace,
                observed_at_us: now_us(),
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
    /// every entry that arrived before it has been handled. A noted caret
    /// is work owed: the query goes directly behind it, ahead of everything
    /// waiting, and waits for nothing more when it is reached again
    /// (`behind_noted`). Otherwise it is kept until no deferred work is
    /// pending, and answered then ([`answer_idle`]), at once if none is.
    /// Entries that arrived after it are not waited for, nor are the carets
    /// they note, so a flood cannot keep the answer back.
    fn idle_query(&self, round: u64, behind_noted: bool, trace: TraceId) {
        let context = self.context;
        if !behind_noted && let Some(noted) = context.intake.take_idle() {
            let query = Entry {
                item: Item::IdleQuery {
                    round,
                    behind_noted: true,
                },
                trace,
                observed_at_us: now_us(),
                timing: EventTiming::default(),
            };
            context.intake.push_ahead([noted, query]);
            return;
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
    /// A focus's own caret comes with the focus
    /// ([`read_focus_text`](Self::read_focus_text)). The read is noted as
    /// covering the events observed before it only in the commit of what it
    /// sends, or of nothing when it sends nothing.
    fn caret_of(&mut self, node_id: NodeId, trace: TraceId, observed_at_us: u64) {
        if !self.is_focus(node_id)
            || self
                .context
                .caret_read_since(node_id, self.timing.observed_at_us)
        {
            return;
        }
        let started = std::time::Instant::now();
        let terminal = self.focused_terminal(node_id);
        let purpose = if terminal {
            text_reads::CaretFor::Terminal
        } else {
            text_reads::CaretFor::Event
        };
        let (report, read_at) = text_reads::report_caret(self.context, node_id, purpose);
        let caret_read = read_at.map(|at| (node_id, at));
        // A caret event's read, which a terminal's output causes too, as
        // its cursor moves.
        tracing::debug!(
            elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            "caret read timing"
        );
        let (event, sent_caret) = match report {
            Ok((caret, false, selection)) => (
                NormalizedEvent::CaretMoved {
                    node_id,
                    caret: caret.clone(),
                    selection_changes: selection.changes,
                    content_changed: selection.content_changed,
                    new_selection: selection.new_selection,
                },
                terminal.then_some((node_id, caret)),
            ),
            unsent => {
                if matches!(unsent, Ok((_, true, _))) {
                    tracing::debug!(
                        "the terminal's caret is the one last sent; it is not sent again"
                    );
                }
                self.commit(Effects {
                    caret_read,
                    ..Effects::default()
                });
                return;
            }
        };
        let backend = if self.context.uia_registry.runtime_id_of(node_id).is_some() {
            Backend::Uia
        } else {
            Backend::Msaa
        };
        let window = self.context.tracked_window();
        let message = self.event_message(trace, observed_at_us, backend, window, event);
        self.commit(Effects {
            result: Some(message),
            sent_caret,
            caret_read,
            ..Effects::default()
        });
    }

    /// Whether `node_id` is the focused terminal and its reading is held
    /// with its text changed since its last read
    /// ([`crate::terminal::reading::Reading::Held`]): output is arriving
    /// that nobody reads until Core asks, and the caret moves with it.
    fn held_with_changes(&self, node_id: NodeId) -> bool {
        self.focused_terminal(node_id)
            && self.context.watch.follow(|follow| {
                follow
                    .terminals
                    .get(&node_id.number())
                    .is_some_and(|terminal| terminal.reading == Reading::Held { changed: true })
            })
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
        if self.held_with_changes(node_id)
            && self
                .context
                .watch
                .follow(|follow| follow.caret_watch.is_none())
        {
            self.commit(Effects {
                absorb: Some(hwnd),
                ..Effects::default()
            });
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
    fn note_caret(&self, node_id: NodeId, trace: TraceId, observed_at_us: u64) {
        tracing::debug!(%trace, "a held terminal's caret event is noted");
        self.context.intake.when_idle(Entry {
            item: Item::NotedCaret { node_id },
            trace,
            observed_at_us,
            timing: self.timing,
        });
    }

    /// A caret event of the text focus `node_id`, from UIA or, for the
    /// console host's text area, from its console: evidence for an open
    /// caret key's watch, and otherwise the caret reported, or noted while
    /// the terminal's reading is held with its text changed.
    ///
    /// In a terminal Core's reading rules answer a key, so the caret event that
    /// is its evidence reads the terminal; and every caret event of a focused
    /// terminal reads it, the read taking the caret too, so that the rules
    /// judge the next key from the caret the terminal moved: Windows
    /// Terminal sometimes moves its caret with no text change, and the
    /// console host's caret event is its cursor's move
    /// (`docs/design/terminal-model.md`, "Where it runs": one caret).
    fn caret_event(&mut self, node_id: NodeId, trace: TraceId, observed_at_us: u64) {
        if self.check_open_watch(node_id, true) {
            if self.focused_terminal(node_id) {
                self.terminal_output(node_id, trace, observed_at_us);
            }
            return;
        }
        if self.refused() {
            return;
        }
        if self.held_with_changes(node_id) {
            self.note_caret(node_id, trace, observed_at_us);
        } else if self.focused_terminal(node_id) {
            self.terminal_output(node_id, trace, observed_at_us);
        } else {
            self.caret_of(node_id, trace, observed_at_us);
        }
    }

    /// The caret a caret key's answer, `reply`, carries, to note as the
    /// caret last sent to Core when `node_id` is the focused terminal.
    fn reply_caret(&self, node_id: NodeId, reply: &TextReply) -> Option<(NodeId, CaretReport)> {
        match reply {
            TextReply::Caret(reply) if self.focused_terminal(node_id) => {
                Some((node_id, reply.caret.clone()))
            }
            _ => None,
        }
    }

    /// The focused terminal, when it is the text area of the console window
    /// `hwnd`, whose console events are its own.
    fn focused_console_terminal(&self, hwnd: isize) -> Option<NodeId> {
        let (focus, window) = self
            .context
            .watch
            .follow(|follow| (follow.tracking.focus, follow.tracking.window));
        focus
            .filter(|_| window == Some(hwnd))
            .filter(|&node_id| self.focused_terminal(node_id))
    }

    /// Whether `node_id` is the focus and a terminal read through UIA, whose
    /// new output the outpost finds by diffing its text. The focus's own
    /// role decides (`Tracking::focus_role`), which a foreground report,
    /// naming a window and not the control focus is in, leaves as it was.
    fn focused_terminal(&self, node_id: NodeId) -> bool {
        self.context.uia_registry.runtime_id_of(node_id).is_some()
            && self.is_focus(node_id)
            && self
                .context
                .watch
                .follow(|follow| follow.tracking.focus_role)
                == Some(Role::Terminal)
    }

    /// Takes a focused terminal's on-demand reading through `event`
    /// ([`crate::terminal::reading`]) and does what the transition says:
    /// reads and sends the read, answers Core's request (`request`, its id
    /// and trace) with it, or only notes the change. A request the
    /// transition reads nothing for is answered that nothing changed. The
    /// transition is made on a copy of the terminal's entry, `sent_caret`
    /// noted on it as the caret last sent when given, and only the commit of
    /// what follows puts the copy in place.
    fn terminal_event(
        &mut self,
        node_id: NodeId,
        event: Event,
        request: Option<(u64, TraceId)>,
        (trace, _): (TraceId, u64),
        sent_caret: Option<CaretReport>,
    ) {
        if event != Event::TextChanged {
            // Anything else may end the hold, and its read must see every
            // update observed after it begins.
            self.context.intake.absorb_console_updates(None);
        }
        let mut terminal = self.terminal_copy(node_id);
        if sent_caret.is_some() {
            terminal.sent_caret = sent_caret;
        }
        if event == Event::Focused {
            terminal.focused();
        }
        let (reading, action) = terminal.reading.next(event);
        terminal.reading = reading;
        match action {
            Action::Nothing | Action::ReplyEmpty => {
                let result = request
                    .map(|request| self.reply_text_message(request, TextReply::TerminalUnchanged));
                let owed_done = self.owed_attempt.take();
                self.commit(Effects {
                    result,
                    terminal: Some((node_id, terminal)),
                    owed_done,
                    ..Effects::default()
                });
                return;
            }
            Action::Report | Action::Reply | Action::ReplySilently | Action::Baseline => {}
        }
        let owed = request.filter(|_| matches!(action, Action::Reply | Action::ReplySilently));
        self.read_terminal(
            node_id,
            terminal,
            owed,
            (None, action == Action::Reply),
            trace,
        );
    }

    /// A copy of terminal `node_id`'s entry, as committed, for a read to
    /// work on; a new terminal's when it has none.
    fn terminal_copy(&self, node_id: NodeId) -> crate::terminal::Terminal {
        self.context.watch.follow(|follow| {
            follow
                .terminals
                .get(&node_id.number())
                .cloned()
                .unwrap_or_default()
        })
    }

    /// A copy of terminal `node_id`'s entry, as committed, when it has one,
    /// for a read of its history or a search, made with no lock held.
    fn terminal_seen(&self, node_id: NodeId) -> Option<crate::terminal::Terminal> {
        self.context
            .watch
            .follow(|follow| follow.terminals.get(&node_id.number()).cloned())
    }

    /// Reads the focused terminal `node_id` into `terminal`, a copy of its
    /// entry, the read of `scope` (a live read for `None`), and sends it to
    /// Core with the caret read with it: as the answer to Core's request
    /// `owed`, or as an event unless it is identical to the last read sent
    /// ([`crate::terminal::Terminal::sends`]). `waited` says whether the
    /// read waited for Core's speech queue. A read that failed answers
    /// `owed` as unanswered. Whatever the read found, its commit puts the
    /// copy in place of the entry; an abandoned worker's copy is dropped.
    fn read_terminal(
        &mut self,
        node_id: NodeId,
        mut terminal: crate::terminal::Terminal,
        owed: Option<(u64, TraceId)>,
        (scope, waited): (Option<verbatim_model::TerminalScope>, bool),
        trace: TraceId,
    ) {
        let read = match self.client.uia() {
            Some(uia) => {
                let started = std::time::Instant::now();
                let read = text_reads::terminal_read(
                    self.context,
                    uia,
                    node_id,
                    &mut terminal,
                    (scope, waited),
                );
                // Each read, end to end, and how long its event waited in
                // the queue: during a flood the reads follow one another, so
                // these say how much of the time the outpost spends in the
                // terminal.
                tracing::debug!(
                    queued_us = self
                        .timing
                        .dequeued_at_us
                        .saturating_sub(self.timing.observed_at_us),
                    elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                    "terminal read timing"
                );
                read
            }
            None => None,
        };
        let Some((answer, caret, read_at)) = read else {
            // Gone or failed: nothing to send, and Core's request is
            // answered as unanswered.
            let result =
                owed.map(|request| self.reply_text_message(request, TextReply::Unanswered));
            let owed_done = self.owed_attempt.take();
            let caret_read = self.pending_caret_read.take();
            self.commit(Effects {
                result,
                terminal: Some((node_id, terminal)),
                owed_done,
                caret_read,
                ..Effects::default()
            });
            return;
        };
        let caret_read = read_at
            .map(|at| (node_id, at))
            .or_else(|| self.pending_caret_read.take());
        let mut snapshot = answer.snapshot;
        // The caret read with the text goes with it: the console host
        // raises no caret event for every character typed, so this keeps
        // Core's caret following typing. A caret key's watch is checked
        // first, as the caret may have moved since the change that caused
        // the read.
        if let Some(caret) = caret {
            self.check_watch_now(node_id, false);
            if self.refused() {
                return;
            }
            terminal.sent_caret = Some(caret.report.clone());
            snapshot.caret = Some(caret.report);
        }
        let read_at_us = answer.started_us;
        let owed_done = self.owed_attempt.take();
        if let Some(request) = owed {
            terminal.answered(&snapshot);
            let result =
                self.reply_text_message(request, TextReply::TerminalSnapshot(Box::new(snapshot)));
            self.commit(Effects {
                result: Some(result),
                terminal: Some((node_id, terminal)),
                owed_done,
                caret_read,
                ..Effects::default()
            });
            return;
        }
        if !terminal.sends(&snapshot) {
            tracing::debug!("the terminal's read is the one last sent; it is not sent again");
            self.commit(Effects {
                terminal: Some((node_id, terminal)),
                owed_done,
                caret_read,
                ..Effects::default()
            });
            return;
        }
        let window = self.context.tracked_window();
        // What the read found is stamped with when the read began, not with
        // when the change that caused it was observed: a read that began
        // after a key may show what the key did.
        let message = self.event_message(
            trace,
            read_at_us,
            Backend::Uia,
            window,
            NormalizedEvent::TerminalSnapshot {
                node_id,
                snapshot: Box::new(snapshot),
            },
        );
        self.commit(Effects {
            result: Some(message),
            terminal: Some((node_id, terminal)),
            owed_done,
            caret_read,
            ..Effects::default()
        });
    }

    /// The answer `reply` to Core's request `request`.
    fn reply_text_message(
        &self,
        (request_id, trace): (u64, TraceId),
        reply: TextReply,
    ) -> OutpostToSupervisor {
        self.reply_message(
            request_id,
            trace,
            QueryOutcome::Done(QueryResult::Text(reply)),
        )
    }

    /// Answers Core's request `request` with `reply`.
    fn reply_text(&self, request: (u64, TraceId), reply: TextReply) {
        let result = self.reply_text_message(request, reply);
        self.commit(Effects {
            result: Some(result),
            ..Effects::default()
        });
    }

    /// Answers Core's request of the focused terminal `node_id` for a read
    /// at once ([`TextOp::TerminalReadNow`], and [`TextOp::ReadCaret`], the
    /// read before a held key, `docs/design/key-holding.md`), whatever its
    /// on-demand state. A fresh read before a review command clears a held
    /// state's mark that the text changed, since it reads what was held.
    fn terminal_read_now(
        &mut self,
        request: (u64, TraceId),
        node_id: NodeId,
        scope: verbatim_model::TerminalScope,
    ) {
        let mut terminal = self.terminal_copy(node_id);
        if scope == verbatim_model::TerminalScope::Fresh
            && terminal.reading == (Reading::Held { changed: true })
        {
            terminal.reading = Reading::Held { changed: false };
        }
        self.read_terminal(
            node_id,
            terminal,
            Some(request),
            (Some(scope), false),
            request.1,
        );
    }

    /// A focused terminal's text changed, or another event asks for a read:
    /// read at once or noted, as its on-demand reading stands. A change
    /// observed before the last read began is covered by that read and
    /// changes nothing (a console terminal's text change comes by UIA and
    /// as a console update both). An `observed_at_us` of 0 is covered by
    /// nothing.
    fn terminal_output(&mut self, node_id: NodeId, trace: TraceId, observed_at_us: u64) {
        let covered = observed_at_us != 0
            && self.context.watch.follow(|follow| {
                follow
                    .terminals
                    .get(&node_id.number())
                    .is_some_and(|terminal| terminal.covers(observed_at_us))
            });
        if covered {
            return;
        }
        self.terminal_event(
            node_id,
            Event::TextChanged,
            None,
            (trace, observed_at_us),
            None,
        );
    }

    /// What the caret read of a focus about to be reported finds, for a
    /// focus that may have text or whose role says it may, `None` for any
    /// other, with when the read began: read while the worker handles the
    /// focus, so the line comes with the focus and nothing is reported
    /// between them, as NVDA's object announcement reads an object's text
    /// within it (`docs/nvda/speech.md`, "What an object with text says").
    /// When something is selected, the selected text is read too, which
    /// Core speaks in place of the line, a protected (password) field's
    /// too, masked as the control gives it. A focus with no text to read is
    /// [`FocusText::NoText`], and Core speaks its value; one whose text
    /// could not be read now is [`FocusText::NotRead`], and Core speaks
    /// neither, never the value, which for a document is all of its text.
    fn read_focus_text(&self, node_id: NodeId, role: Role) -> (Option<FocusText>, Option<u64>) {
        let context = self.context;
        text_reads::forget_no_text(context, node_id);
        let role_has_text = matches!(role, Role::EditableText | Role::Document | Role::Terminal);
        if !role_has_text && !text_reads::may_have_text(context, node_id, role) {
            return (None, None);
        }
        let started = Instant::now();
        let (read, read_at) =
            text_reads::report_caret(context, node_id, text_reads::CaretFor::Focus);
        let text = match read {
            Ok((caret, _, _)) => {
                let selected_text = caret.selection.and_then(|selection| {
                    let op = TextOp::ReadRange {
                        start: TextPoint::At(selection.start),
                        end: TextPoint::At(selection.end),
                        through: None,
                    };
                    match text_reads::answer(context, node_id, &op) {
                        TextReply::Range { text, .. } => Some(text),
                        _ => None,
                    }
                });
                FocusText::Caret {
                    caret,
                    selected_text,
                }
            }
            Err(text_reads::NoCaret::NoText) => FocusText::NoText,
            Err(text_reads::NoCaret::NotRead) => {
                tracing::debug!("the focus's caret could not be read");
                FocusText::NotRead
            }
        };
        tracing::debug!(
            elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            "the focus's caret read"
        );
        (Some(text), read_at)
    }

    /// Attempts the work the followed focus still owes, if this worker is in
    /// charge and no worker has attempted it, once, under a deadline of its
    /// own with no moved-on window (`super::commit::Owed`): a terminal's
    /// first read, which speaks nothing, since what it held before the focus
    /// arrived is not new output, taken once the subscription has moved to
    /// the terminal, so output written after the read began raises a change
    /// that reads it (`docs/design/terminal-model.md`, "Reading rules", rule
    /// 15), its caret read first for a terminal published at its bound; or
    /// a `SensitiveSlider`'s lookup for a thumb published at its bound. The
    /// commit of the attempt's result clears it, read or failed.
    fn attempt_owed(&mut self) {
        if self.refused() {
            return;
        }
        let Some(owed) = self.context.watch.take_owed(self.generation) else {
            return;
        };
        let node_id = owed.node;
        match owed.work {
            OwedWork::FirstRead(caret) => {
                // A terminal published at its bound has its caret read
                // first.
                let caret = caret.or_else(|| {
                    let (text, read_at) = self.read_focus_text(node_id, Role::Terminal);
                    self.pending_caret_read = read_at.map(|at| (node_id, at));
                    if let Some(FocusText::Caret { caret, .. }) = text {
                        Some(caret)
                    } else {
                        None
                    }
                });
                let Some(caret) = caret.filter(|_| self.focused_terminal(node_id)) else {
                    let caret_read = self.pending_caret_read.take();
                    self.commit(Effects {
                        owed_done: Some(node_id),
                        caret_read,
                        ..Effects::default()
                    });
                    return;
                };
                if let Some(subscription) = self.context.focus_events.get() {
                    subscription.settle();
                }
                self.owed_attempt = Some(node_id);
                self.terminal_event(
                    node_id,
                    Event::Focused,
                    None,
                    (owed.trace, owed.observed_at_us),
                    Some(caret),
                );
                self.owed_attempt = None;
                self.pending_caret_read = None;
            }
            OwedWork::Slider => {
                let (chain, role) = self.context.watch.follow(|follow| {
                    let tracking = &follow.tracking;
                    (
                        tracking.chain.clone(),
                        tracking.focus_role.unwrap_or(Role::Unknown),
                    )
                });
                let element = self.slider_target(node_id, role, &chain);
                self.commit(Effects {
                    element,
                    owed_done: Some(node_id),
                    ..Effects::default()
                });
            }
        }
    }

    /// The subscription's target for the focus `node_id` of `role`, with
    /// `chain`, its ancestors and itself, once its parent, a UIA slider of
    /// class `SensitiveSlider` whose element this outpost does not keep,
    /// is found by its path (`docs/parity/focus-context.md`, "A slider that
    /// focuses its thumb"): one connection call and one remote operation,
    /// and the slider kept under the node its runtime id names, the
    /// parent's. `None` when the parent is no such slider or is not found.
    fn slider_target(
        &mut self,
        node_id: NodeId,
        role: Role,
        chain: &[NodeSnapshot],
    ) -> Option<(NodeId, Target)> {
        let parent = sensitive_slider_of(node_id, chain)?;
        // The listener's read keeps no ancestor's element: the slider is
        // found by its path, one connection call and one remote operation,
        // and kept under the node its runtime id names, the parent's.
        let runtime_id = self.context.uia_registry.runtime_id_of(parent)?;
        let found = self.element_by_focus_path(&runtime_id)?;
        let kept = self
            .context
            .uia_registry
            .id_for_element(&runtime_id, &found);
        let slider = self.context.uia_registry.element_of(kept)?;
        let (mut target, _) = target_for(self.context, (node_id, role), chain, false);
        target.wanted = target
            .wanted
            .map(|(node, group, _)| (node, group, Some(kept)));
        target.slider = Some(slider);
        Some((node_id, target))
    }

    /// Releases every node Core does not hold that was reported at or before
    /// the position Core acknowledged. A node reported later, or never
    /// reported, is kept, since Core may not have seen it yet.
    fn release(&self, held: &[u64], acknowledged: u64) {
        let held: HashSet<u64> = held.iter().copied().collect();
        // The one change of follow state made outside `Watch::commit`
        // (`super::commit`), with the same in-charge check: it must take the
        // queue's lock and the registries' inside it, which no commit does.
        // Only the worker in charge releases, and the nodes leave both
        // registries, and their terminals the follow state, under the
        // watch lock and the queue's, so a worker that replaces this one,
        // should it be abandoned while the objects are dropped, can never
        // report a node that is about to vanish, and no message queued
        // meanwhile can record one.
        let (objects, released, terminals) = {
            let mut state = self.context.watch.lock();
            if state.generation != self.generation {
                self.refused.set(true);
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
            let terminals: Vec<crate::terminal::Terminal> = released
                .iter()
                .flat_map(|(released, _)| released)
                .filter_map(|node| state.follow.terminals.remove(node))
                .collect();
            // The reports whose focus node is released are no longer read
            // for, under the same locks as their nodes leave the store.
            if let Some((released, _)) = &released {
                self.context.held().release(released);
            }
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
            (objects, released, terminals)
        };
        text_reads::forget(self.context, released);
        // Outside the lock: releasing an object can call into the
        // application.
        drop(objects);
        drop(terminals);
    }

    /// Emits `event` stamped `observed_at_us`, in microseconds on the run's
    /// shared clock (0 when not known), as the entry's result.
    fn emit(
        &self,
        trace: TraceId,
        observed_at_us: u64,
        backend: Backend,
        window: Option<isize>,
        event: NormalizedEvent,
    ) -> bool {
        let message = self.event_message(trace, observed_at_us, backend, window, event);
        self.commit(Effects {
            result: Some(message),
            ..Effects::default()
        }) == Committed::Applied
    }

    /// `event` as the message that reports it ([`event_message`]).
    fn event_message(
        &self,
        trace: TraceId,
        observed_at_us: u64,
        backend: Backend,
        window: Option<isize>,
        event: NormalizedEvent,
    ) -> OutpostToSupervisor {
        event_message(
            self.context,
            self.timing,
            (trace, observed_at_us),
            (backend, window),
            event,
        )
    }

    /// Reports a focus, or a foreground change, and makes it the focus this
    /// outpost follows, in one commit: the report recorded for Core's
    /// judgement, sent, and a focus followed, its caret, text and the
    /// focus-following subscription following it, so the limiter keeps its
    /// events and the menu rules know where focus is. Then, once committed,
    /// the slider a `SensitiveSlider`'s thumb is in is looked up, and the
    /// work a terminal owes is attempted.
    #[expect(
        clippy::too_many_arguments,
        reason = "a focus report has this many parts"
    )]
    fn emit_focus(
        &mut self,
        trace: TraceId,
        observed_at_us: u64,
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
        let mut memo = self
            .context
            .watch
            .follow(|follow| follow.tracking.dialog.clone());
        read::describe_dialogs(
            self.context,
            ancestors.iter_mut().chain(std::iter::once(&mut node)),
            (&mut memo, foreground),
        );
        let role = node.role;
        let node_id = node.id;
        let earlier = self.earlier.take();
        // A foreground change is recorded by its window's event address.
        let evidence = if backend == Backend::Uia && !foreground {
            held::Evidence::Uia(earlier)
        } else {
            held::Evidence::Msaa
        };
        let reported = self.focus_to_judge(trace, &node, &ancestors, window, foreground);
        let (text, read_at) = if foreground {
            (None, None)
        } else {
            self.read_focus_text(node.id, node.role)
        };
        tracing::debug!(
            ?role,
            name = ?node.name,
            foreground,
            ?backend,
            ancestors = ancestors.len(),
            "focus reported"
        );
        let chain: Vec<NodeSnapshot> = ancestors
            .iter()
            .cloned()
            .chain(std::iter::once(node.clone()))
            .collect();
        let caret = match &text {
            Some(FocusText::Caret { caret, .. }) => Some(caret.clone()),
            _ => None,
        };
        let event = NormalizedEvent::FocusChanged {
            node,
            foreground,
            ancestors,
            selected_child,
            text: text.map(Box::new),
            earlier,
            group: self.group,
        };
        let message = self.event_message(trace, observed_at_us, backend, window, event);
        let mut effects = Effects {
            result: Some(message),
            report: Some(Report::Reported(reported, evidence)),
            dialog: Some(memo),
            caret_read: read_at.map(|at| (node_id, at)),
            value_from_selection: self.value_from_selection.take(),
            focus_path: self.focus_path.take(),
            stale: self.stale.take(),
            ..Effects::default()
        };
        // A foreground report is a window, not the control focus is in: it
        // is not followed.
        let mut slider = None;
        if foreground {
            effects.foreground = Some((node_id, role, self.batch));
        } else {
            let (follow, to_find) = follow_of(
                self.context,
                FocusMade {
                    node: (node_id, role),
                    object,
                    window,
                    chain,
                    batch: self.batch,
                    event: (trace, observed_at_us),
                    redirected: std::mem::take(&mut self.redirected),
                    caret,
                    at_bound: false,
                },
            );
            effects.follow = Some(follow);
            slider = to_find;
        }
        if self.commit(effects) != Committed::Applied {
            return;
        }
        self.reported = true;
        self.after_follow(node_id, role, slider);
    }

    /// What follows the commit that made `node_id`, of `role`, the followed
    /// focus: the lookup of the slider a `SensitiveSlider`'s thumb is in,
    /// when `slider` names one whose element is not kept, and a second
    /// commit adding it while the focus is still followed; then the work
    /// the focus owes.
    fn after_follow(&mut self, node_id: NodeId, role: Role, slider: Option<NodeId>) {
        if slider.is_some() {
            let chain = self
                .context
                .watch
                .follow(|follow| follow.tracking.chain.clone());
            if let Some(element) = self.slider_target(node_id, role, &chain) {
                self.commit(Effects {
                    element: Some(element),
                    ..Effects::default()
                });
            }
        }
        self.attempt_owed();
    }

    /// The focus `node`, with `ancestors`, reported in `window` from the
    /// event traced `trace`, as the outpost records it among its reports
    /// ([`super::held`]): a foreground change carries its event address.
    /// NVDA makes its foreground object when the
    /// focus enters a top-level window, so the window keeps the verdict it
    /// had then while the focus moves within it
    /// ([`is_foreground_window`](Self::is_foreground_window)).
    fn focus_to_judge(
        &mut self,
        trace: TraceId,
        node: &NodeSnapshot,
        ancestors: &[NodeSnapshot],
        window: Option<isize>,
        foreground: bool,
    ) -> held::Focus {
        judged_focus(
            self.context,
            (trace, node, ancestors),
            (window, foreground),
            self.msaa_event.take(),
        )
    }

    /// The focus walk this worker armed has returned: whether it was
    /// published at its bound, so the worker sends nothing else for it. A
    /// walk that returns still armed past its bound, before the watchdog
    /// has published it, is published here by the same commit from the
    /// same progress ([`publication`]), so whichever thread publishes a
    /// walk that ends at its bound, the report is the same. A published
    /// walk counts its entry as reported, and the worker then attempts the
    /// work its focus owes. An abandoned worker's walk counts as published:
    /// it sends nothing more.
    fn walk_returned(&mut self) -> bool {
        let context = self.context;
        match context.watch.walk_returned(self.generation, Instant::now()) {
            Returned::Disarmed => return false,
            Returned::Abandoned => {
                self.refused.set(true);
                return true;
            }
            Returned::Published => {}
            Returned::PastBound(armed) => {
                let mut state = context.watch.lock();
                let published =
                    Watch::commit_publication(&mut state, context, &context.watch.alarm, armed);
                drop(state);
                if let Some((walk, displaced)) = published {
                    drop(displaced);
                    tracing::info!(
                        trace = %walk.focus.trace,
                        name = ?walk.focus.node.name,
                        "a focus walk returned past its bound; the focus is published as read so far"
                    );
                }
                // Published by this worker or, just before, by the watchdog,
                // unless this worker was abandoned meanwhile.
                if matches!(
                    context.watch.walk_returned(self.generation, Instant::now()),
                    Returned::Abandoned
                ) {
                    self.refused.set(true);
                    return true;
                }
            }
        }
        tracing::debug!(
            "the walk returned after the focus was published at its bound; its result is discarded"
        );
        self.reported = true;
        self.attempt_owed();
        true
    }

    /// The last focus this outpost reported, with its ancestors.
    fn focus_chain(&self) -> Vec<NodeSnapshot> {
        let chain = self
            .context
            .watch
            .follow(|follow| follow.tracking.chain.clone());
        read::held_prefix(self.context, chain)
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
        observed_at_us: u64,
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
                self.terminal_output(node_id, trace, observed_at_us);
                self.absorb_while_held(node_id, hwnd);
            }
            return;
        }
        if kind == WinEventKind::ConsoleCaret {
            // The console host's cursor moved: the focused console
            // terminal's caret event, as its UIA caret events are not
            // subscribed to either.
            if let Some(node_id) = self.focused_console_terminal(hwnd) {
                // A watch's poll goes on unless the event's check answers
                // the key: a blink tick left over from an earlier cursor
                // set can come before the key's move.
                self.caret_event(node_id, trace, observed_at_us);
            }
            return;
        }
        if kind == WinEventKind::Destroy {
            if id_object == OBJID_WINDOW.0 && id_child == CHILDID_SELF {
                self.context.arbitrator().forget(hwnd);
                self.context.intake.gap_window_destroyed(hwnd);
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
                self.system_caret_of_uia_focus(&runtime_id, trace, observed_at_us);
            }
            return;
        }
        // UIA owns this window, or another application does.
        if read::own_window_verdict(self.context, hwnd) != Some(false) {
            return;
        }
        if self.edit_event(kind, (hwnd, id_object, id_child), trace, observed_at_us) {
            return;
        }
        let Some(object) = verbatim_ia2::acquire::event_object(hwnd, id_object, id_child) else {
            return;
        };
        let registry = &self.context.msaa_registry;
        if kind == WinEventKind::Selection {
            self.msaa_selection(object, hwnd, (trace, observed_at_us));
            return;
        }
        let spoken = self.spoken_node(kind, &object);
        if kind == WinEventKind::ValueChange {
            self.value_change(object, spoken, (hwnd, trace, observed_at_us));
            return;
        }
        let Some((node_id, role)) = spoken else {
            return;
        };
        // Only the property the event names is read, as NVDA reads only
        // that property of its focus or ancestor object. A state change's
        // states are recorded for the held focus in its commit.
        let mut states_read = None;
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
                states_read = Some((node_id, states));
                NormalizedEvent::PropertyChanged {
                    node_id,
                    child_count: self
                        .expanded_child_count((node_id, role, states), (hwnd, id_child)),
                    change: PropertyChange::States(states),
                }
            }
            _ => return,
        };
        let message = self.event_message(trace, observed_at_us, Backend::Msaa, Some(hwnd), event);
        self.commit(Effects {
            result: Some(message),
            states: states_read,
            ..Effects::default()
        });
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
        observed_at_us: u64,
    ) {
        if self.context.intake.system_caret_unused() {
            return;
        }
        if let Some(node_id) = self.context.uia_registry.existing_id(runtime_id) {
            self.caret_event(node_id, trace, observed_at_us);
        }
    }

    /// An MSAA selection of `object`, from `hwnd`'s hooks, read only as far
    /// as it can be spoken, by the rule the reducer speaks it by
    /// (`focus_rules::selection_target`), judged against the focus Core
    /// holds and those reported that Core has not judged yet
    /// ([`super::held`]). Selecting the focus or one of its ancestors is a
    /// change of its state, as NVDA handles it, so only its states are
    /// read. Any other selection is spoken only inside a focused selection
    /// container of this outpost, so it is read in full only while one of
    /// those focuses is one, and dropped otherwise, having cost its
    /// acquisition alone.
    fn msaa_selection(
        &mut self,
        object: verbatim_ia2::acquire::EventObject,
        hwnd: isize,
        (trace, observed_at_us): (TraceId, u64),
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
            let held = self.context.held();
            let chain: Vec<NodeId> = held.focuses().flat_map(held::Focus::chain_ids).collect();
            let container = held
                .focuses()
                .any(|focus| focus_rules::is_selection_container(focus.node.role));
            (chain, container)
        };
        let event = if let Some(node_id) = object.which_of(&chain, registry)
            && self.selection_states_spoken(node_id)
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
        self.emit(trace, observed_at_us, Backend::Msaa, Some(hwnd), event);
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
        observed_at_us: u64,
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
                        self.caret_of(node_id, trace, observed_at_us);
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
                text_reads::note_value_change(self.context, node_id);
                self.check_open_watch(node_id, false);
                self.emit(
                    trace,
                    observed_at_us,
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
        let was_expanded = self.context.held().expanded_before(node)?;
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
        (hwnd, trace, observed_at_us): (isize, TraceId, u64),
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
        self.emit(trace, observed_at_us, Backend::Msaa, Some(hwnd), event);
    }

    /// The role a node of a focus's chain was last read with: one of the
    /// focuses events are judged against, or one of its ancestors
    /// ([`super::held`]).
    fn known_role(&self, node: NodeId) -> Option<Role> {
        self.context.held().role_of(node)
    }

    /// Whether a selection of `node`, the focus or an ancestor of one of
    /// the focuses events are judged against, is spoken as a change of its
    /// states (`focus_rules::selection_target`): not for a logical ancestor
    /// ([`Self::is_logical_ancestor`]).
    fn selection_states_spoken(&self, node: NodeId) -> bool {
        let held = self.context.held();
        held.focuses().any(|focus| {
            focus_rules::selection_target(
                node,
                (focus.node.id, focus.node.role),
                focus.ancestors.iter().map(|ancestor| ancestor.id),
                |ancestor| !self.is_logical_ancestor(ancestor),
            )
            .is_some()
        })
    }

    /// The node `object` is, with the role it was last read with, when a
    /// change of `kind` on it is spoken, by the rule the reducer speaks it
    /// by (`focus_rules::change_target`), judged against the focus Core
    /// holds and those reported that Core has not judged yet
    /// ([`super::held`]): a name, description, or value change only for a
    /// focus, and a state change for a focus or one of its ancestors, as
    /// NVDA speaks them. Any other object is not read: it is told from
    /// those by its identity first, without reading any of its properties.
    fn spoken_node(
        &self,
        kind: WinEventKind,
        object: &verbatim_ia2::acquire::EventObject,
    ) -> Option<(NodeId, Role)> {
        let change = match kind {
            WinEventKind::NameChange => ChangeKind::Name,
            WinEventKind::DescriptionChange => ChangeKind::Description,
            WinEventKind::StateChange => ChangeKind::States,
            _ => ChangeKind::Value,
        };
        let candidates: Vec<NodeId> = {
            let held = self.context.held();
            // The ancestors NVDA has are those reached through `accParent`,
            // not the focus's logical ones ([`Self::logical_ancestors`]):
            // their state changes are not spoken, as selecting a tree item's
            // parent on its way to the focus would otherwise say "selected".
            // Nor is the foreground window an ancestor here
            // ([`Self::is_foreground_window`]). Ancestors first, then the
            // focus, of each focus judged against.
            let mut candidates: Vec<NodeId> = Vec::new();
            for focus in held.focuses() {
                let ancestors: Vec<NodeId> =
                    focus.ancestors.iter().map(|ancestor| ancestor.id).collect();
                for node in
                    focus_rules::change_candidates(change, focus.node.id, &ancestors, |id| {
                        !self.is_logical_ancestor(id)
                            && !self.is_foreground_window(id, focus.entered_in_front)
                    })
                {
                    if !candidates.contains(&node) {
                        candidates.push(node);
                    }
                }
            }
            candidates
        };
        if candidates.is_empty() {
            return None;
        }
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

    /// The logical ancestors ([`Self::is_logical_ancestor`]) of the
    /// focuses events are judged against ([`super::held`]).
    fn logical_ancestors(&self) -> Vec<NodeId> {
        let chain: Vec<NodeId> = self
            .context
            .held()
            .focuses()
            .flat_map(held::Focus::chain_ids)
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
    fn uia_event(&mut self, event: UiaEvent, trace: TraceId, observed_at_us: u64) {
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
            self.context.tracked_window()
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
                && read::own_window_verdict(self.context, hwnd) != Some(true)
            {
                return; // MSAA owns this window, or another application does.
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
            self.emit(trace, observed_at_us, Backend::Uia, hwnd, selection);
            return;
        }
        if matches!(event.kind, UiaKind::Selection)
            && let Some(change) = self.selection_as_value(&event.parts)
        {
            self.emit(trace, observed_at_us, Backend::Uia, hwnd, change);
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
                    self.caret_event(node_id, trace, observed_at_us);
                }
                return;
            }
            UiaKind::ActiveTextPosition(range) => {
                let event = (&event.parts.runtime_id[..], range, hwnd);
                return self.active_text_position(event, (trace, observed_at_us));
            }
            UiaKind::TextChanged => {
                let changed = (&event.parts.runtime_id[..], element.as_ref());
                return self.text_changed(changed, hwnd, (trace, observed_at_us));
            }
            _ => {}
        }
        // A selection the listener heard carries what Core speaks of the
        // item, filled by UIA with the event (`verbatim_uia::ListenerSelection`),
        // and is reported with it, less the details the theme does not
        // report: no search is made for its element, which could enter a
        // child window another process serves
        // (`docs/design/cross-process-calls-study.md`, instance 3).
        let mut node = Self::uia_node(self.context, &event.parts, element.as_ref());
        if matches!(event.kind, UiaKind::Selection) {
            crate::gates::leave_out_unreported(&mut node.details, &self.context.fetches());
        }
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
        self.emit(trace, observed_at_us, Backend::Uia, hwnd, normalized);
    }

    /// A UIA text change of the text focus whose runtime id it carries: it
    /// checks a caret key's watch open on the node, then is reported, or,
    /// for a terminal, has its new output read.
    fn text_changed(
        &mut self,
        (runtime_id, element): (&[i32], Option<&Cached<Kept>>),
        hwnd: Option<isize>,
        (trace, observed_at_us): (TraceId, u64),
    ) {
        let Some(node_id) = self.context.uia_registry.existing_id(runtime_id) else {
            return;
        };
        text_reads::text_event_from(self.context, node_id, element);
        let terminal = self.focused_terminal(node_id);
        self.check_open_watch(node_id, false);
        if terminal {
            self.terminal_output(node_id, trace, observed_at_us);
            return;
        }
        self.emit(
            trace,
            observed_at_us,
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
        (trace, observed_at_us): (TraceId, u64),
    ) {
        let node_id = self.context.uia_registry.existing_id(runtime_id);
        let (Some(node_id), Some(range)) = (node_id, range) else {
            return;
        };
        if let Some(position) = text_reads::active_position(self.context, node_id, &range.0) {
            self.emit(
                trace,
                observed_at_us,
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
        // The focus Core holds, or will once it has judged this outpost's
        // reports, as NVDA uses its focus object; found by its path when its
        // element is not known.
        let focus_id = self.expected_uia_focus()?;
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
        let window = self.context.tracked_window();
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
        let focus = self.context.held().expected().map(|focus| focus.node.id)?;
        if self
            .context
            .watch
            .follow(|follow| follow.tracking.value_from_selection)
            != Some(focus)
        {
            return None;
        }
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

    /// The runtime id of the focus Core holds, or will hold once it has
    /// judged what this outpost reported ([`held::Held::expected`]), when it
    /// is a UIA focus of this outpost's: the focus a selection is read
    /// against, as NVDA reads it against its focus object.
    fn expected_uia_focus(&self) -> Option<Vec<i32>> {
        let node = self.context.held().expected()?.node.id;
        self.context.uia_registry.runtime_id_of(node)
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
    fn fact(&mut self, fact: DeliveredFact, trace: TraceId, observed_at_us: u64) {
        match fact {
            DeliveredFact::Foreground { hwnd } => self.foreground(hwnd, trace, observed_at_us),
            DeliveredFact::MsaaFocus {
                hwnd,
                id_object,
                id_child,
            } => self.msaa_focus(hwnd, id_object, id_child, trace, observed_at_us),
            DeliveredFact::UiaFocusRead { read, .. } => {
                self.uia_focus(&read, trace, observed_at_us);
            }
            // The intake resolves these; none reaches the worker.
            DeliveredFact::UiaFocusMarker { .. }
            | DeliveredFact::UiaFocusDropped { .. }
            | DeliveredFact::UiaFocusReadsEnded => {}
            // The intake unwraps a marked event as it takes it in.
            DeliveredFact::UiaMarked { fact, .. } => self.fact(*fact, trace, observed_at_us),
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
                observed_at_us,
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
                observed_at_us,
            ),
            DeliveredFact::Alert {
                hwnd,
                id_object,
                id_child,
            } => self.alert(hwnd, id_object, id_child, trace, observed_at_us),
            DeliveredFact::Show {
                hwnd,
                id_object,
                id_child,
            } => self.tooltip_shown(hwnd, id_object, id_child, trace, observed_at_us),
            // Menu openings are planned separately; one reaching here is
            // handled the same way.
            menu @ DeliveredFact::MenuPopup { .. } => {
                self.menu_opened(&Entry {
                    item: Item::Fact(menu, None),
                    trace,
                    observed_at_us,
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
        observed_at_us: u64,
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
            observed_at_us,
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
        observed_at_us: u64,
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
            observed_at_us,
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
    /// follows once the window is in front (`docs/crates/verbatim-outpost.md`,
    /// "Foreground changes and focus-now").
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
    /// Every change in front is read and reported: whether the focus is
    /// already in its window, NVDA's two tests against its last queued
    /// focus, is Core's verdict, which refuses a foreground change of the
    /// window its focus is in (`docs/design/ownership-one-judge.md`, "Focus
    /// groups").
    ///
    /// Within a focus group, a change dropped lets the next older entry be
    /// tried ([`super::intake::try_newest_first`]), as NVDA's `pumpAll`
    /// stops at the first foreground or focus event processed; so does a
    /// change reported whose window holds an unreleased node of a report's
    /// line, which Core may refuse, and every report of the group carries
    /// its rank, so Core takes the older entry only when it refused the
    /// change. A change reported of any other window ends the group.
    fn foreground(&mut self, hwnd: isize, trace: TraceId, observed_at_us: u64) {
        if window_belongs_to_hidden_frame(hwnd) {
            tracing::debug!(hwnd, "foreground dropped: Core's hidden frame");
            return;
        }
        if !self.accepts(EventName::GainFocus, hwnd) || !self.in_front(hwnd) {
            tracing::debug!(hwnd, "foreground dropped: not the foreground window");
            return;
        }
        // NVDA makes the foreground's object as it makes any MSAA event's
        // (`processForegroundWinEvent` through `winEventToNVDAEvent`): a
        // window gone, ghosted, or hung is dropped before the UIA test.
        if !Self::readable(hwnd) {
            return;
        }
        // Accepted: it restamps the gap record open in its window, so the
        // record's read after a return carries the return's stamp, or
        // answers one open in another of the application's top-level
        // windows, before its window's verdict, whichever it is.
        self.context
            .gap_accepted(hwnd, top_level_of(hwnd), observed_at_us, true);
        match read::own_window_verdict(self.context, hwnd) {
            None => return,
            Some(true) => {
                tracing::debug!(hwnd, "foreground dropped: UIA owns the window");
                return;
            }
            Some(false) => {}
        }
        self.report_foreground(hwnd, trace, observed_at_us);
    }

    /// Reads the foreground window `hwnd`'s name and reports it, unless it
    /// is no longer the foreground window by then.
    fn report_foreground(&mut self, hwnd: isize, trace: TraceId, observed_at_us: u64) {
        // Whether the window holds an unreleased node of a report's line,
        // judged before this change is recorded among the reports: then
        // the group's next older entry is tried after it.
        let top_level = top_level_of(hwnd);
        let holds_a_line = top_level != 0
            && self
                .context
                .held()
                .focuses()
                .any(|focus| focus.top_level == top_level);
        let (backend, node) = read::foreground_window(self.context, self.client, hwnd);
        if !self.in_front(hwnd) {
            tracing::info!(
                hwnd,
                "foreground report dropped: no longer the foreground window"
            );
            return;
        }
        // Ordered by when its window was confirmed as the foreground, which
        // is never before the event was observed.
        let confirmed_at_us = now_us().max(observed_at_us);
        self.emit_focus(
            trace,
            confirmed_at_us,
            backend,
            Some(hwnd),
            node,
            true,
            None,
            (Vec::new(), None),
        );
        if self.reported && holds_a_line {
            self.reported = false;
        }
    }

    /// An MSAA focus fact.
    fn msaa_focus(
        &mut self,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        trace: TraceId,
        observed_at_us: u64,
    ) {
        if id_child == CHILDID_SELF && window_belongs_to_hidden_frame(hwnd) {
            return;
        }
        if !self.accepts(EventName::GainFocus, hwnd) || !Self::readable(hwnd) {
            return;
        }
        let Some(uia_owns) = read::own_window_verdict(self.context, hwnd) else {
            return;
        };
        let top_level = top_level_of(hwnd);
        if uia_owns {
            // UIA owns this window; its UIA fact reports the focus, and the
            // gap record waits for it.
            tracing::debug!(hwnd, "MSAA focus dropped: UIA owns the window");
            self.copy_dropped_for_uia((hwnd, top_level), trace, observed_at_us);
            return;
        }
        // Handled through MSAA, it answers a gap record open in another of
        // the application's top-level windows.
        self.context
            .gap_accepted(hwnd, top_level, observed_at_us, false);
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
            .is_redirected_focus(&Object::Msaa(hwnd, id_object, id_child))
        {
            tracing::debug!(
                hwnd,
                id_object,
                id_child,
                "MSAA focus dropped: already reported from its control's focus"
            );
            self.commit(Effects {
                clear_redirect: true,
                ..Effects::default()
            });
            return;
        }
        let Some(mut candidate) = verbatim_ia2::acquire::focus_candidate(hwnd, id_object, id_child)
        else {
            tracing::debug!(hwnd, id_object, id_child, "MSAA focus dropped: unreadable");
            return;
        };
        // A repeat of the focus is read in full and judged in Core, whose
        // duplicate rule compares it with the last focus it took, of any
        // application, as NVDA's `isDuplicateIAccessibleEvent` compares
        // with its last queued focus (`docs/design/ownership-one-judge.md`,
        // "Repeats").
        let resolved_child = candidate.resolved_child();
        // NVDA accepts an MSAA focus only when the object or one of its
        // ancestors has the focused state (`shouldAllowIAccessibleFocusEvent`),
        // which weeds out stale and spurious focus events. It checks the
        // states after the duplicate test, reading them live, going on
        // past a failed `accParent` from the parent window's own object,
        // and above the process boundary by plain windows, never focused,
        // with no call there.
        let context = self.context;
        let client = &mut *self.client;
        if !candidate.has_focused_state(
            read::MAX_ANCESTOR_HOPS,
            &|window| read::reads_as_plain(context, window),
            &mut |window| read::parent_window_for_focused_state(context, client, window),
        ) {
            tracing::debug!(
                hwnd,
                id_object,
                id_child,
                "MSAA focus dropped: nothing has the focused state"
            );
            return;
        }
        // The focus and the ancestors its walk reads are compared with the
        // focus line Core holds and the newest focus Core has not judged, by
        // NVDA's equality (`docs/design/focus-pipeline-rework.md`, "MSAA
        // object equality").
        let candidates = read::msaa_candidates(self.context);
        let node = candidate.read(&self.context.msaa_registry, Some(&candidates));
        self.report_msaa_focus(
            node,
            ((hwnd, id_object, id_child), resolved_child > 0),
            (trace, observed_at_us),
            &candidates,
        );
    }

    /// Reports `node`, the MSAA focus NVDA's checks accepted from the
    /// event at `event`, acquired as a simple child when `simple_child`,
    /// traced `trace` and observed at `observed_at_us`, with its ancestors
    /// walked: the watchdog publishes it at its walk's bound should a call
    /// of the walk still be under way then, and the worker adopts that
    /// publication when the walk returns.
    fn report_msaa_focus(
        &mut self,
        node: NodeSnapshot,
        (event, simple_child): ((isize, i32, i32), bool),
        (trace, observed_at_us): (TraceId, u64),
        candidates: &verbatim_ia2::equality::Candidates,
    ) {
        let (hwnd, id_object, id_child) = event;
        let context = self.context;
        let previous = self.focus_chain();
        let object = self
            .context
            .msaa_registry
            .key_of(node.id)
            .map(|(hwnd, object, child)| Object::Msaa(hwnd, object, child));
        let redirected = object
            .as_ref()
            .is_some_and(|object| *object != Object::Msaa(hwnd, id_object, id_child));
        let msaa_event = (event, simple_child);
        // Its walk, past NVDA's checks, is published at its bound should a
        // call of it still be under way then; the chain goes on from the
        // focus's own window until it reads more.
        let left = read::walk_left(context, hwnd, observed_at_us);
        let (timing, batch, group) = (self.timing, self.batch, self.group);
        let watched = walk_watcher(context, self.generation, |node, window| BoundFocus {
            trace,
            observed_at_us,
            timing,
            batch,
            backend: Backend::Msaa,
            window: Some(window),
            node: node.clone(),
            object: object.clone(),
            msaa_event: Some(msaa_event),
            group,
        });
        let from = match object {
            Some(Object::Msaa(window, _, _)) => window,
            _ => hwnd,
        };
        if !watched(read::WalkStep::Focus {
            node: &node,
            window: hwnd,
            from,
            left,
        }) {
            self.refused.set(true);
            return;
        }
        let enrichment = read::msaa_enrichment(
            context,
            self.client,
            (&node, &previous, Some(candidates)),
            (left, Some(&watched)),
        );
        if self.walk_returned() {
            return;
        }
        self.msaa_event = Some(msaa_event);
        self.redirected = redirected;
        self.emit_focus(
            trace,
            observed_at_us,
            Backend::Msaa,
            Some(hwnd),
            node,
            false,
            object.clone(),
            enrichment,
        );
    }

    /// A focus copy in `window`, of top-level window `top_level`, observed
    /// at `observed_at_us`, that this outpost dropped because it judges the
    /// window UIA: it opens the gap record, or restamps the one open in its
    /// top-level window, unless an answer observed with it stands for it;
    /// a drop for no window observed with it has the application's focus
    /// read at once in its place, stamped with its observation
    /// ([`super::gap_record::GapRecord::copy_dropped`]).
    fn copy_dropped_for_uia(
        &mut self,
        (window, top_level): (isize, isize),
        trace: TraceId,
        observed_at_us: u64,
    ) {
        let dropped = self
            .context
            .gap_copy_dropped(window, top_level, observed_at_us);
        tracing::debug!(
            window,
            ?dropped,
            "the gap record took a copy dropped for UIA"
        );
        if dropped == CopyDropped::ReadNow {
            self.gap_read(
                GapRead {
                    window,
                    stamp_us: observed_at_us,
                },
                trace,
            );
        }
    }

    /// A UIA focus as the focus listener judged and read it from the
    /// event's own element (`docs/design/focus-pipeline.md`, section 12):
    /// it was no repeat of the focus Core holds, its nearest window
    /// passed NVDA's acceptance test against the foreground window, the
    /// element had the keyboard focus when the listener read it, as
    /// NVDA's `shouldAllowUIAFocusEvent` requires, and its states, value,
    /// details, ancestors, and selected child are the element's own, read
    /// with the classic walk, one call at a time on the element the
    /// listener holds. The outpost reports it at once, its node
    /// minted from its runtime id; the listener's evidence that the element
    /// it read before under that runtime id has lost the focus or died
    /// gives the id a new node first. Only then, off the way to speech,
    /// does the outpost find its own copy of the element, which no process
    /// can hand it (experiment E-marshal), by the path the listener read
    /// ([`find_focus_element`](Self::find_focus_element)); for a focus that
    /// may have text, before it is reported, since its caret is read with
    /// it.
    fn uia_focus(&mut self, read: &UiaFocusRead, trace: TraceId, observed_at_us: u64) {
        let context = self.context;
        let judged = (|| {
            // NVDA's acceptance test against the foreground window was
            // made by the listener, before it read the focus, as NVDA
            // makes it before it queues one
            // (`docs/design/focus-pipeline.md`, section 16.2).
            let reported = Self::uia_focus_window(context, read.hwnd, read.window)?;
            // The listener makes no backend verdict; the outpost's one
            // verdict per window decides which backend's focus counts in
            // each of its windows, judged at the read's observation
            // (`docs/design/focus-pipeline-rework.md`, "Who owns each
            // duplicated fact", the verdicts): in a window it judges not
            // UIA, its MSAA focus speaks, and the listener's read is
            // dropped.
            let judged_window = read.window.or(Some(read.hwnd).filter(|&hwnd| hwnd != 0));
            if let Some(window) = judged_window
                && read::own_window_verdict(context, window) != Some(true)
            {
                tracing::debug!(
                    %trace,
                    window,
                    "UIA focus dropped: the outpost judges its window MSAA, whose focus speaks for it, or another application serves it"
                );
                return Err(Dropped);
            }
            Ok(reported)
        })();
        let Ok(reported) = judged else {
            return;
        };
        let runtime_id = &read.snapshot.runtime_id;
        self.note_earlier(read);
        let reading = Instant::now();
        let previous = self.focus_chain();
        let node = Self::uia_node(context, &read.snapshot, None);
        let selected = read
            .selected_child
            .as_ref()
            .map(|child| Self::uia_node(context, child, None));
        let ancestors = read::uia_chain_from_read(
            context,
            self.client,
            (&read.ancestors, read.hwnd, read.window, read.ancestors_end),
            (&previous, observed_at_us.saturating_add(read.bound_us)),
        );
        // Committed with the focus; the entry finds the focus's element by
        // it before then.
        self.focus_path = Some(FocusPath::of(read));
        tracing::debug!(
            %trace,
            enrichment_us = reading.elapsed().as_micros(),
            "UIA focus reported from the listener's read"
        );
        // A focus on the element this outpost already found under its
        // runtime id keeps it: the listener read that it still has the
        // keyboard focus. A repeat comes this far only when the focus
        // before it waited for Core's judgement as it was read, which Core's
        // own rule then drops (section 16.3); otherwise this is the focus
        // coming back to the element after one in another application.
        let found = read.same_as_last == Some(EarlierElement::Focused)
            && context.uia_registry.element_of(node.id).is_some();
        // A focus that may have text needs its own element before it is
        // reported, since its caret is read with it; any other's is found
        // after, off the way to speech.
        let text_first = !found
            && matches!(
                node.role,
                Role::EditableText | Role::Document | Role::Terminal
            );
        if text_first {
            self.find_focus_element(runtime_id, true);
        }
        if self.refused() {
            return;
        }
        self.value_from_selection = Some(read.value_from_selection.then_some(node.id));
        self.emit_focus(
            trace,
            observed_at_us,
            Backend::Uia,
            reported,
            node,
            false,
            Some(Object::Uia(runtime_id.clone())),
            (ancestors, selected),
        );
        if !found
            && !text_first
            && !self.refused()
            && context.intake.focused() == Some(Object::Uia(runtime_id.clone()))
            && let Some(id) = self.find_focus_element(runtime_id, false)
        {
            // The subscription follows the element, and the slider a thumb
            // is in, in a commit made only while the node is the followed
            // focus.
            let (role, chain) = context.watch.follow(|follow| {
                (
                    follow.tracking.focus_role.unwrap_or(Role::Unknown),
                    follow.tracking.chain.clone(),
                )
            });
            let element = match target_for(context, (id, role), &chain, false) {
                (_, Some(_)) => self.slider_target(id, role, &chain),
                (target, None) => Some((id, target)),
            };
            if element.is_some() {
                self.commit(Effects {
                    element,
                    ..Effects::default()
                });
            }
        }
    }

    /// Notes what the listener read of the element it held before under
    /// the UIA focus `read` names, which its report carries
    /// (`docs/design/focus-pipeline-rework.md`, "One object, one identity").
    fn note_earlier(&mut self, read: &UiaFocusRead) {
        let runtime_id = &read.snapshot.runtime_id;
        // A runtime id keeps its node when its application gives it to a
        // new element (`docs/design/focus-pipeline-rework.md`, "One object,
        // one identity"): on the listener's evidence that the element it
        // stood for lost the focus or is gone, the element kept for it is
        // evicted by the report's commit, if still kept then; for one gone,
        // its text caches are dropped now, a cache's write, so the focus's
        // caret read fetches the new element's pattern, and its terminal
        // entry goes in the commit.
        self.earlier = read.same_as_last;
        if let Some(earlier @ (EarlierElement::NotFocused | EarlierElement::Gone)) =
            read.same_as_last
            && let Some(node) = self.context.uia_registry.existing_id(runtime_id)
        {
            let gone = earlier == EarlierElement::Gone;
            tracing::debug!(
                gone,
                "UIA focus: its runtime id named an element that lost the focus or is gone; the node's element is evicted"
            );
            if gone {
                text_reads::forget(self.context, [node.number()]);
            }
            self.stale = Some(Stale {
                node,
                element: self.context.uia_registry.element_of(node),
                gone,
            });
        }
    }

    /// Finds this outpost's own copy of the focus `runtime_id` names, from
    /// the listener's read, by the path the listener read: from the nearest
    /// window in the focus's chain, down by runtime ids, one connection
    /// call and one transaction, and keeps it under the focus's node, whose
    /// id it returns. A `text` focus with no window of its own whose walk up
    /// the listener cut, so that no path leads to it, is found as the
    /// application's focused element instead
    /// ([`element_after_cut_walk`](Self::element_after_cut_walk)). Not
    /// found, as Windows Terminal's tab focus elements,
    /// which are in no tree, are not, nothing is kept, its text cannot be
    /// read, and nothing is followed, as NVDA hears nothing local before
    /// its group is added to a focus (`docs/design/focus-pipeline.md`,
    /// sections 12.5 and 17).
    fn find_focus_element(&mut self, runtime_id: &[i32], text: bool) -> Option<NodeId> {
        let finding = Instant::now();
        let found = self
            .element_by_focus_path(runtime_id)
            .or_else(|| text.then(|| self.element_after_cut_walk(runtime_id))?);
        tracing::debug!(
            found = found.is_some(),
            elapsed_us = finding.elapsed().as_micros(),
            "the focus's own element looked for by its path"
        );
        let Some(element) = found else {
            tracing::debug!("the focus's element was not found; nothing is followed");
            return None;
        };
        Some(
            self.context
                .uia_registry
                .id_for_element(runtime_id, &element),
        )
    }

    /// The element `runtime_id` names, an element of the chain the
    /// listener read for the focus this outpost last reported, found by its
    /// path ([`focus_path::find`]); `None` when it is not in that chain, its
    /// path does not lead to it, or the walk failed.
    fn element_by_focus_path(&mut self, runtime_id: &[i32]) -> Option<Cached<Kept>> {
        let context = self.context;
        let route = match &self.focus_path {
            // The focus about to be reported, whose path is not committed
            // yet.
            Some(path) => context.route_in(path, runtime_id),
            None => context.focus_route(runtime_id),
        }?;
        let uia = self.client.uia()?;
        let nodes = context.node_request(uia).ok()?;
        let remote = context.tries_remote(Some(route.window));
        let (found, import_failed) =
            focus_path::find(uia, &route, &nodes, remote, &context.clock());
        if import_failed {
            context.read_classically(route.window);
        }
        // Read as `Kept`: the callers keep it, or read only its runtime id.
        found.ok().flatten().map(|found| found.narrow())
    }

    /// The element of a windowless focus whose walk up the listener cut
    /// ([`FocusPath::is_cut_at`]): the application's focused element, read
    /// with the focus read's checks, its process-id check among them
    /// ([`read::focused_element_in`]), and taken when its runtime id is the
    /// focus's (`docs/design/focus-pipeline-rework.md`, the audit's
    /// difference 5), since a walk down from the nearest window would find
    /// it only as that window's direct child, and a search below the window
    /// costs a round of provider calls per element. Found only while the
    /// element still has the keyboard focus, a limit.
    fn element_after_cut_walk(&mut self, runtime_id: &[i32]) -> Option<Cached<Kept>> {
        let context = self.context;
        let cut = match &self.focus_path {
            Some(path) => path.is_cut_at(runtime_id),
            None => context.watch.follow(|follow| {
                follow
                    .focus_path
                    .as_ref()
                    .is_some_and(|path| path.is_cut_at(runtime_id))
            }),
        };
        if !cut {
            return None;
        }
        let element =
            read::focused_element_in(context, self.client, read::FocusRead::CutWalk).ok()?;
        if element.runtime_id() != runtime_id {
            tracing::debug!(
                "the application's focused element is not the focus the listener read; the focus's element is not found"
            );
            return None;
        }
        Some(element.narrow())
    }

    /// The window a UIA focus is reported in, or [`Dropped`] when the focus
    /// is in Core's hidden frame. Whether the window uses UIA is the
    /// outpost's verdict, judged by the caller.
    fn uia_focus_window(
        context: &Context,
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
        let judged = reported.or_else(|| focus_window_of(context.target_pid()));
        if judged.is_some_and(window_belongs_to_hidden_frame) {
            return Err(Dropped);
        }
        Ok(reported)
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
        let (batch, role) = self
            .context
            .watch
            .follow(|follow| (follow.tracking.batch, follow.tracking.role));
        if batch == Some(self.batch) && matches!(role, Some(Role::Menu | Role::MenuItem)) {
            return;
        }
        if !self.accepts(EventName::MenuStart, hwnd) || !Self::readable(hwnd) {
            return;
        }
        // UIA owns this window, or another application does.
        if read::own_window_verdict(self.context, hwnd) != Some(false) {
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
            entry.observed_at_us,
            Backend::Msaa,
            Some(window),
            node,
            false,
            Some(Object::Msaa(hwnd, id_object, id_child)),
            (ancestors, None),
        );
    }

    /// Answers a text request `op` about a terminal, when it is one: the
    /// read before a held key, a read Core asks for at once, rows of the
    /// history, the colours after a key, the screen's shape, and Core's
    /// on-demand reading. Returns whether it answered.
    fn terminal_query(
        &mut self,
        request_id: u64,
        trace: TraceId,
        node_id: NodeId,
        op: &TextOp,
    ) -> bool {
        let request = (request_id, trace);
        let focused = self.focused_terminal(node_id);
        match op {
            // The read before a held key is a read of the terminal, whose
            // caret answers the request (`docs/design/key-holding.md`).
            TextOp::ReadCaret if focused => {
                let terminal = self.terminal_copy(node_id);
                self.read_terminal(node_id, terminal, Some(request), (None, false), trace);
                return true;
            }
            TextOp::TerminalReadNow(scope) => {
                if focused {
                    self.terminal_read_now(request, node_id, *scope);
                } else {
                    self.reply_text(request, TextReply::Unanswered);
                }
                return true;
            }
            TextOp::TerminalHistory { above, whole } => {
                let seen = self.terminal_seen(node_id);
                let reply = match self.client.uia() {
                    Some(uia) if focused => text_reads::terminal_history(
                        self.context,
                        uia,
                        (node_id, seen),
                        (*above, *whole),
                    ),
                    _ => TextReply::Unanswered,
                };
                self.reply_text(request, reply);
                return true;
            }
            TextOp::TerminalFind { needle, from, rows } => {
                let seen = self.terminal_seen(node_id);
                let reply = match self.client.uia() {
                    Some(uia) if focused => text_reads::terminal_find(
                        self.context,
                        uia,
                        (node_id, seen),
                        (needle, *from, *rows),
                    ),
                    _ => TextReply::Unanswered,
                };
                self.reply_text(request, reply);
                return true;
            }
            TextOp::TerminalColours { from_ms, until_ms } => {
                if focused {
                    let mut terminal = self.terminal_copy(node_id);
                    if terminal.colours((*from_ms, *until_ms)) {
                        // The last read ended at or after the key without
                        // the colours: one is taken at once with them, sent
                        // as a live read ("Colours").
                        self.read_terminal(node_id, terminal, None, (None, false), trace);
                    } else {
                        self.commit(Effects {
                            terminal: Some((node_id, terminal)),
                            ..Effects::default()
                        });
                    }
                }
                if !self.refused() {
                    self.reply_text(request, TextReply::Done);
                }
                return true;
            }
            TextOp::TerminalShape { height, width } => {
                let mut terminal = self.terminal_copy(node_id);
                terminal.shape(*height, *width);
                let result = self.reply_text_message(request, TextReply::Done);
                self.commit(Effects {
                    result: Some(result),
                    terminal: Some((node_id, terminal)),
                    ..Effects::default()
                });
                return true;
            }
            _ => {}
        }
        if let Some(event) = terminal_event_of(op) {
            // Core's on-demand reading of the focused terminal; a node that
            // is not a focused terminal has nothing to read.
            if !focused {
                let reply = if event == Event::Hold {
                    TextReply::Done
                } else {
                    TextReply::TerminalUnchanged
                };
                self.reply_text(request, reply);
                return true;
            }
            let observed_at_us = now_us();
            if event == Event::Hold {
                self.terminal_event(node_id, event, None, (trace, observed_at_us), None);
                if !self.refused() {
                    self.reply_text(request, TextReply::Done);
                }
            } else {
                self.terminal_event(node_id, event, Some(request), (trace, observed_at_us), None);
            }
            return true;
        }
        false
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
            && self.terminal_query(request_id, trace, *node_id, op)
        {
            return;
        }
        let context = self.context;
        let client = &mut *self.client;
        let started = std::time::Instant::now();
        // The focus-now answer's control, adopted and followed once the
        // answer is sent.
        let mut follow = None;
        let result = match query {
            Query::FocusNow => {
                let (answer, answered) = self.focus_now(trace);
                follow = answered;
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
            Query::DumpTree { locations } => {
                read::dump_tree(context, client, *locations).map(QueryResult::Tree)
            }
            Query::Text { node_id, op } => {
                Ok(QueryResult::Text(text_reads::answer(context, *node_id, op)))
            }
            Query::ShellItems { kind } => {
                super::shell_items::shell_items(context, client, *kind).map(QueryResult::ShellItems)
            }
            Query::ShellItemPosition { kind, item } => {
                super::shell_items::shell_item_position(context, client, *kind, item)
                    .map(QueryResult::ShellItemPosition)
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
        let result = self.reply_message(request_id, trace, outcome);
        match follow {
            Some((answered, judged)) => self.follow_answer(result, answered, judged, trace),
            None => {
                self.commit(Effects {
                    result: Some(result),
                    ..Effects::default()
                });
            }
        }
    }

    /// The answer to a focus-now query, with its control, when it has one,
    /// and the control as events are judged against it, to commit with the
    /// answer and follow ([`follow_answer`](Self::follow_answer)). Core
    /// takes the control as its focus, so its text is read as a reported
    /// focus's is: a text field's line is spoken and Core has its caret.
    fn focus_now(&mut self, trace: TraceId) -> (FocusNow, Option<(AnsweredFocus, held::Focus)>) {
        let context = self.context;
        let (mut answer, window) = read::focus_now(context, self.client);
        let windows = answer.window.iter_mut().map(|(window, _)| window);
        let focus = answer.focus.iter_mut().flat_map(|focus| {
            let FocusedControl {
                node, ancestors, ..
            } = focus;
            ancestors.iter_mut().chain(std::iter::once(node))
        });
        read::describe_dialogs(context, windows.chain(focus), (&mut None, false));
        let answered = answer.focus.as_mut().map(|control| {
            let node = &control.node;
            let (text, read_at) = self.read_focus_text(node.id, node.role);
            control.text = text.clone().map(Box::new);
            AnsweredFocus {
                node: node.clone(),
                ancestors: control.ancestors.clone(),
                text,
                read_at,
                observed_at_us: answer.observed_at_us,
                window,
            }
        });
        // Core takes the answer's control as its focus, as NVDA queues its
        // fake focus: until Core has judged it, events are judged against it
        // too, recorded with the answer's commit.
        let answered = answered.map(|answered| {
            let judged =
                self.focus_to_judge(trace, &answered.node, &answered.ancestors, window, false);
            (answered, judged)
        });
        (answer, answered)
    }

    /// Commits `result`, a focus-now answer, with its control, `answered`,
    /// made the outpost's focus and followed, as a reported focus is
    /// ([`emit_focus`](Self::emit_focus)), and `judged` recorded for Core's
    /// judgement, keeping the MSAA event address of the same focus judged
    /// against before it, which NVDA's second foreground test compares
    /// with ([`answer_to_judge`]): so what the follow-up sends, such as a
    /// terminal's first read, reaches Core after the focus it follows. A
    /// control already followed under the same node is followed again, not
    /// adopted. Either way the answer becomes the last
    /// focus queued, as NVDA's fake focus does.
    fn follow_answer(
        &mut self,
        result: OutpostToSupervisor,
        answered: AnsweredFocus,
        judged: held::Focus,
        trace: TraceId,
    ) {
        let AnsweredFocus {
            node,
            ancestors,
            text,
            read_at,
            observed_at_us,
            window,
        } = answered;
        let (node_id, role) = (node.id, node.role);
        let object = object_of(self.context, &node);
        // A control this outpost already has as its focus, under the same
        // node, is not adopted again, only followed.
        let adopt = self.context.watch.follow(|follow| follow.tracking.focus) != Some(node_id)
            || !self.is_focus(node_id);
        let chain = ancestors.into_iter().chain(std::iter::once(node)).collect();
        let caret = match text {
            Some(FocusText::Caret { caret, .. }) => Some(caret),
            _ => None,
        };
        let (mut follow, slider) = follow_of(
            self.context,
            FocusMade {
                node: (node_id, role),
                object,
                window,
                chain,
                batch: self.batch,
                event: (trace, observed_at_us),
                redirected: false,
                caret,
                at_bound: false,
            },
        );
        follow.adopt = adopt;
        let committed = self.commit(Effects {
            result: Some(result),
            report: Some(Report::Answered(judged)),
            follow: Some(follow),
            caret_read: read_at.map(|at| (node_id, at)),
            ..Effects::default()
        });
        if committed == Committed::Applied {
            self.after_follow(node_id, role, slider);
        }
    }

    /// The reply to the query in hand, carrying the calls made since the
    /// last message.
    fn reply_message(
        &self,
        request_id: u64,
        trace: TraceId,
        outcome: QueryOutcome,
    ) -> OutpostToSupervisor {
        OutpostToSupervisor::Reply {
            trace_id: trace,
            request_id,
            outcome,
            timing: EventTiming {
                published_at_us: now_us(),
                calls: take_calls(),
                ..self.timing
            },
        }
    }
}

/// The focus `node`, with `ancestors`, reported in `window` from the event
/// traced `trace`, as the outpost records it among its reports
/// ([`super::held`]): an MSAA focus carries `msaa_event`, its event's
/// address and whether it is a simple child, and a foreground change its
/// window's event address; each dialog of its line carries the text the
/// report gave it. NVDA makes its foreground object when the focus enters
/// a top-level window, so the window keeps the verdict it had then while
/// the focus moves within it ([`Worker::is_foreground_window`]): a focus
/// inherits it from the newest report in its top-level window, and a
/// foreground change of the window sets it. Local calls.
fn judged_focus(
    context: &Context,
    (trace, node, ancestors): (TraceId, &NodeSnapshot, &[NodeSnapshot]),
    (window, foreground): (Option<isize>, bool),
    msaa_event: Option<((isize, i32, i32), bool)>,
) -> held::Focus {
    let top_level = window.map_or(0, top_level_of);
    // What the focus before it gives it, taken under the lock, which
    // is let go before the foreground window is read.
    let inherited = if foreground {
        None
    } else {
        let held = context.held();
        held.newest_in(top_level)
            .map(|before| before.entered_in_front)
    };
    // A foreground change's window was just found in front.
    let entered_in_front = match inherited {
        Some(entered) => entered,
        None if foreground => Some(top_level).filter(|&top| top != 0),
        None => (top_level != 0 && top_level == context.foreground_window()).then_some(top_level),
    };
    let dialog_texts = ancestors
        .iter()
        .chain(std::iter::once(node))
        .filter(|node| crate::dialog_text::is_dialog(node.role))
        .map(|node| (node.id, node.details.description.clone()))
        .collect();
    let ancestors = ancestors.iter().map(HeldNode::of).collect();
    let (event, simple_child) = match msaa_event {
        Some((event, simple_child)) => (Some(event), simple_child),
        None => (window.filter(|_| foreground).map(foreground_event), false),
    };
    held::Focus {
        trace,
        node: HeldNode::of(node),
        ancestors,
        top_level,
        entered_in_front,
        event,
        simple_child,
        dialog_texts,
    }
}

/// `event` as the message that reports it, from an entry of `timing`,
/// published now with the calls this thread has made since the last.
fn event_message(
    context: &Context,
    timing: EventTiming,
    (trace, observed_at_us): (TraceId, u64),
    (backend, window): (Backend, Option<isize>),
    event: NormalizedEvent,
) -> OutpostToSupervisor {
    let target = context.target_pid();
    let client = window.and_then(|hwnd| console_client(hwnd, target));
    let name = || context.application_name(client.map_or(target, |client| client.0));
    OutpostToSupervisor::Event {
        trace_id: trace,
        observed_at_us,
        backend,
        window: window
            .map(|hwnd| event_window_facts(window_facts(hwnd), &event, name, alt_is_held)),
        application: client,
        timing: EventTiming {
            published_at_us: now_us(),
            calls: take_calls(),
            ..timing
        },
        event,
    }
}

/// A focus about to be committed as the followed one ([`follow_of`]).
struct FocusMade {
    /// The focus and its role.
    node: (NodeId, Role),
    /// The object it is, as the intake records the focus.
    object: Option<Object>,
    /// The window it was reported in.
    window: Option<isize>,
    /// Its ancestors, outermost first, and itself.
    chain: Vec<NodeSnapshot>,
    /// Its entry's batch.
    batch: u64,
    /// The trace and observation of the event it came from.
    event: (TraceId, u64),
    /// Whether it was reached by redirecting a control's own focus event.
    redirected: bool,
    /// The caret read with its report, if any.
    caret: Option<CaretReport>,
    /// Whether it was published at its walk's bound, with nothing read
    /// after its walk.
    at_bound: bool,
}

/// `made` as the follow its commit applies, computed before the commit
/// with local calls only: the subscription's target, whether the focus is
/// a terminal, and what it owes. A terminal owes its first read, its caret
/// read first when it was published at its bound; a thumb published at
/// its bound owes its slider's lookup. Also the slider to look up after
/// the commit, for a thumb the worker reports itself, whose slider's
/// element is not kept.
fn follow_of(context: &Context, made: FocusMade) -> (Follow, Option<NodeId>) {
    let FocusMade {
        node: (node_id, role),
        object,
        window,
        chain,
        batch,
        event: (trace, observed_at_us),
        redirected,
        caret,
        at_bound,
    } = made;
    let terminal = role == Role::Terminal && matches!(object, Some(Object::Uia(_)));
    let console =
        window.is_some_and(|hwnd| window_class_name(hwnd) == text_reads::CONSOLE_WINDOW_CLASS);
    let (target, to_find) = target_for(context, (node_id, role), &chain, terminal && console);
    // A focus whose own element is not kept yet has the slider found with
    // it, once it is.
    let to_find = to_find.filter(|_| target.element.is_some());
    let owed = if terminal {
        match (at_bound, &caret) {
            (true, _) => Some(OwedWork::FirstRead(None)),
            (false, Some(caret)) => Some(OwedWork::FirstRead(Some(caret.clone()))),
            (false, None) => None,
        }
    } else {
        to_find.filter(|_| at_bound).map(|_| OwedWork::Slider)
    };
    let follow = Follow {
        node: node_id,
        role,
        object,
        window,
        chain,
        batch,
        redirected,
        terminal,
        console,
        sent_caret: caret.filter(|_| terminal),
        target,
        owed,
        trace,
        observed_at_us,
        adopt: true,
    };
    (follow, to_find.filter(|_| !at_bound))
}

/// Where the focus-following subscription is moved for the focus
/// `node_id` of `role`, with `chain`, its ancestors and itself, as NVDA
/// moves its local group with the focus (`UIAHandler/__init__.py` 642 to
/// 756; `NVDAObjects/UIA/__init__.py` 1623 to 1630;
/// `docs/design/focus-pipeline.md`, section 17): to the node's own element
/// when the outpost has it, with the extended group when the node may have
/// text, and the base group otherwise; the console host's text area
/// (`console`) takes the extended group without its text and caret
/// events, since its console's updates and caret events tell the outpost
/// of its output and its cursor; and to nothing for a focus whose element
/// the outpost does not have, or one read through MSAA, whose changes and
/// caret events come from the hooks. The element alone, not its
/// descendants, as the reducer acts on the focus's own changes. NVDA
/// registers its group with `TreeScope_Ancestors` as well, but UIA
/// delivers no ancestor's event to such a registration
/// (`docs/parity/backends.md`, "UIA event registration"). A focus whose
/// parent is a UIA slider of class `SensitiveSlider` has that parent
/// followed too, for its value changes alone ([`verbatim_uia::Group::Value`]),
/// since Core speaks them while the focus is in it
/// (`docs/parity/focus-context.md`, "A slider that focuses its thumb"),
/// when its element is kept; the slider is returned to look up when it is
/// not. Registry lookups only.
fn target_for(
    context: &Context,
    (node_id, role): (NodeId, Role),
    chain: &[NodeSnapshot],
    console: bool,
) -> (Target, Option<NodeId>) {
    let has_text = text_reads::may_have_text(context, node_id, role);
    let group = if !has_text {
        verbatim_uia::Group::Base
    } else if role == Role::Terminal && console {
        verbatim_uia::Group::ExtendedWithoutTextEvents
    } else {
        verbatim_uia::Group::Extended
    };
    let element = context.uia_registry.element_of(node_id);
    let parent = sensitive_slider_of(node_id, chain);
    let slider = parent.and_then(|parent| {
        context
            .uia_registry
            .element_of(parent)
            .map(|element| (parent, element))
    });
    let to_find = parent.filter(|_| slider.is_none());
    let wanted = element
        .as_ref()
        .map(|_| (node_id, group, slider.as_ref().map(|(id, _)| *id)));
    (
        Target {
            wanted,
            element,
            slider: slider.map(|(_, element)| element),
        },
        to_find,
    )
}

/// The parent of the focus `node_id` in `chain`, its ancestors and itself,
/// when it is a UIA slider of class `SensitiveSlider`.
fn sensitive_slider_of(node_id: NodeId, chain: &[NodeSnapshot]) -> Option<NodeId> {
    let [.., parent, focus] = chain else {
        return None;
    };
    (focus.id == node_id && focus_rules::is_sensitive_slider(parent)).then_some(parent.id)
}

/// The publication of a walk still under way at its bound, or returned
/// past it, or whose worker is about to be abandoned, from `walk`, its
/// progress as it stands (`docs/design/focus-pipeline-rework.md`, "A window's
/// name, and the one fallback" and "The outpost's commit point"): the
/// focus as read, its chain as [`bound::chain_at_bound`] makes it,
/// completed by plain windows, with no selected child, no dialog text and
/// no text of its own, recorded for Core's judgement, sent, and made the
/// followed focus, with what it owes. The one function both publishers
/// call, the watchdog and the worker whose walk returns past its bound
/// ([`Worker::walk_returned`]), so the report does not depend on which
/// comes first; its commit ([`Watch::commit_publication`]) claims the walk.
/// Local calls only.
fn publication(context: &Context, walk: &FocusProgress) -> Effects {
    let focus = &walk.focus;
    let ancestors = bound::chain_at_bound(context, walk);
    let judged = judged_focus(
        context,
        (focus.trace, &focus.node, &ancestors),
        (focus.window, false),
        focus.msaa_event,
    );
    let chain: Vec<NodeSnapshot> = ancestors
        .iter()
        .cloned()
        .chain(std::iter::once(focus.node.clone()))
        .collect();
    let message = event_message(
        context,
        focus.timing,
        (focus.trace, focus.observed_at_us),
        (focus.backend, focus.window),
        NormalizedEvent::FocusChanged {
            node: focus.node.clone(),
            foreground: false,
            ancestors,
            selected_child: None,
            text: None,
            earlier: None,
            group: focus.group,
        },
    );
    let evidence = if focus.msaa_event.is_some() {
        held::Evidence::Msaa
    } else {
        held::Evidence::Uia(None)
    };
    let (follow, _) = follow_of(
        context,
        FocusMade {
            node: (focus.node.id, focus.node.role),
            object: focus.object.clone(),
            window: focus.window,
            chain,
            batch: focus.batch,
            event: (focus.trace, focus.observed_at_us),
            // An MSAA focus reached by redirecting its control's own focus
            // event to the control's focused child.
            redirected: matches!(
                (&focus.object, focus.msaa_event),
                (Some(Object::Msaa(hwnd, object, child)), Some((event, _)))
                    if (*hwnd, *object, *child) != event
            ),
            caret: None,
            at_bound: true,
        },
    );
    Effects {
        result: Some(message),
        report: Some(Report::Reported(judged, evidence)),
        follow: Some(follow),
        ..Effects::default()
    }
}

/// What tells the watchdog a focus walk's progress, for worker
/// `generation`: arming it with the focus `focus` makes of the node and
/// window read ([`read::WalkStep::Focus`]), and taking in each step after.
/// A walk already past its bound when it begins is not armed: it cuts
/// itself at its first check, and its worker reports it alone. Answers
/// whether `generation` is still in charge, so an abandoned walk stops at
/// its next step.
fn walk_watcher<'a>(
    context: &'a Context,
    generation: u64,
    focus: impl Fn(&NodeSnapshot, isize) -> BoundFocus + 'a,
) -> impl Fn(read::WalkStep<'_>) -> bool + 'a {
    move |step: read::WalkStep<'_>| match step {
        read::WalkStep::Focus {
            node,
            window,
            from,
            left,
        } => {
            left.is_zero()
                || context
                    .watch
                    .arm(FocusProgress::new(
                        generation,
                        focus(node, window),
                        from,
                        left,
                    ))
                    .is_some()
        }
        step => context.watch.advance(generation, &step),
    }
}

/// `facts`, the window facts of an event message carrying `event`, with
/// the window's class, the application's name that `application` gives,
/// and whether Alt is held, as `alt_held` reads it, only for a focus
/// change, the one event Core's rules for an application's windows and
/// elements match (decision D6), so that no other event carries their
/// strings to Core, nor reads the key.
fn event_window_facts(
    mut facts: WindowFacts,
    event: &NormalizedEvent,
    application: impl FnOnce() -> String,
    alt_held: impl FnOnce() -> bool,
) -> WindowFacts {
    if matches!(event, NormalizedEvent::FocusChanged { .. }) {
        facts.application = application();
        facts.alt_held = alt_held();
    } else {
        facts.class = String::new();
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A focus change's window facts keep the window's class, which Core's
    /// staging rule matches, and take the application's name, which its
    /// rule for the task switcher's frame matches, and whether Alt is held,
    /// which its rule for the task switcher's items matches; any other
    /// event's carry none of them.
    #[test]
    fn only_a_focus_change_carries_its_windows_class_and_application() {
        let facts = WindowFacts {
            top_level: verbatim_model::WindowHandle(5),
            root_owner: verbatim_model::WindowHandle(5),
            topmost: false,
            under_active_window: None,
            in_foreground: true,
            class: "ForegroundStaging".to_owned(),
            application: String::new(),
            alt_held: false,
        };
        let focus = NormalizedEvent::FocusChanged {
            node: NodeSnapshot {
                id: NodeId::new(1),
                backend: Backend::Msaa,
                role: Role::Pane,
                name: None,
                value: None,
                states: StateSet::new(),
                details: verbatim_model::NodeDetails::default(),
            },
            foreground: false,
            ancestors: Vec::new(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
        };
        let explorer = || "explorer".to_owned();
        let held = || true;
        assert_eq!(
            event_window_facts(facts.clone(), &focus, explorer, held),
            WindowFacts {
                application: "explorer".to_owned(),
                alt_held: true,
                ..facts.clone()
            }
        );
        let value = NormalizedEvent::ValueChanged {
            node_id: NodeId::new(1),
            value: Some("1".to_owned()),
        };
        assert_eq!(
            event_window_facts(facts.clone(), &value, explorer, held),
            WindowFacts {
                class: String::new(),
                ..facts
            }
        );
    }

    /// A focus in window `window`, from the MSAA event `event`, reported
    /// from the event traced `trace`.
    fn focus_in(trace: TraceId, window: isize, event: Option<(isize, i32, i32)>) -> held::Focus {
        held::Focus {
            trace,
            node: HeldNode {
                id: NodeId::new(1),
                backend: Backend::Msaa,
                role: Role::Button,
                name: None,
                states: StateSet::new(),
                window: None,
            },
            ancestors: Vec::new(),
            top_level: window,
            entered_in_front: None,
            event,
            simple_child: false,
            dialog_texts: Vec::new(),
        }
    }

    /// A focus-now answer for the focus already judged against keeps the
    /// MSAA event address and the simple-child mark its report recorded;
    /// one for another node has neither.
    #[test]
    fn a_focus_now_answer_keeps_the_event_of_the_same_focus() {
        let event = Some((5, OBJID_CLIENT.0, CHILDID_SELF));
        let before = held::Focus {
            simple_child: true,
            ..focus_in(TraceId::mint(), 8, event)
        };
        let answered = focus_in(TraceId::mint(), 9, None);
        let kept = answer_to_judge(Some(&before), answered.clone());
        assert_eq!(
            (kept.trace, kept.top_level, kept.event, kept.simple_child),
            (answered.trace, 9, event, true)
        );
        let other = held::Focus {
            node: HeldNode {
                id: NodeId::new(2),
                ..answered.node.clone()
            },
            ..answered
        };
        let replaced = answer_to_judge(Some(&before), other);
        assert_eq!((replaced.event, replaced.simple_child), (None, false));
    }

    /// The idle barrier's answer never waits on a lock a text read holds
    /// across its calls into the application, the anchors': a worker
    /// abandoned in such a read keeps the lock, and the worker that
    /// replaced it, waiting on that lock with no deadline, would never be
    /// abandoned in turn. The answer takes the watch lock alone, which no
    /// read holds across a call. The test holds the anchors' lock as that
    /// read does.
    #[test]
    fn the_idle_answer_does_not_wait_on_a_text_reads_lock() {
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
        let read_in_progress = outpost.context.uia_anchors();
        outpost.handle_command(&SupervisorToOutpost::IdleQuery { round: 4 });
        let answer = messages.recv_timeout(wait);
        drop(read_in_progress);
        assert_eq!(
            answer.ok(),
            Some(OutpostToSupervisor::IdleAnswer { round: 4 })
        );
        outpost.shutdown();
    }

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock, PoisonError, Weak};

    use super::super::GapTimer;

    /// The gap record's timer as the test times it: the due time it is set
    /// for, if it is set, and, the first time it is cancelled, the worker's
    /// step of opening a record for a copy in another top-level window,
    /// taken as soon as the intake lets it: there and then, while the
    /// intake's lock is free, or on a thread of its own that waits for the
    /// lock, which is joined once the cancelling push is done.
    #[derive(Default)]
    struct InterleavedTimer {
        due_us: Mutex<Option<u64>>,
        context: OnceLock<Weak<Context>>,
        stepped: AtomicBool,
        worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    }

    /// The copy the worker drops for UIA while the record is answered: its
    /// window, its top-level window, and its observation.
    const SECOND_COPY: (isize, isize, u64) = (0x21, 0x20, 2_000);

    impl GapTimer for InterleavedTimer {
        fn set(&self, due_us: u64) {
            *self.due_us.lock().unwrap_or_else(PoisonError::into_inner) = Some(due_us);
        }

        fn cancel(&self) {
            if !self.stepped.swap(true, Ordering::Relaxed)
                && let Some(context) = self.context.get().and_then(Weak::upgrade)
            {
                let held = context.intake.is_held();
                let step = move || {
                    let (window, top_level, at_us) = SECOND_COPY;
                    let _ = context.gap_copy_dropped(window, top_level, at_us);
                };
                if held {
                    *self.worker.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(std::thread::spawn(step));
                } else {
                    step();
                }
            }
            *self.due_us.lock().unwrap_or_else(PoisonError::into_inner) = None;
        }
    }

    /// The pipe thread's push of a UIA focus's outcome that answers the
    /// open gap record, and the worker's drop of a copy in another
    /// top-level window that opens the next, come at once: whichever order
    /// they take, the record left open has its timer set for its bound.
    #[test]
    fn a_record_opened_while_the_last_is_answered_keeps_its_timer() {
        use verbatim_process::overlapped::{End, Flow, pipe};
        use verbatim_process::transport::EVENT_PIPE_BUFFER;

        use crate::protocol::GapAnswer;

        let (_ours, theirs) =
            pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped).expect("a pipe");
        let outpost = super::super::Outpost::new(theirs, std::process::id());
        let timer = Arc::new(InterleavedTimer::default());
        outpost.set_gap_timer(Arc::clone(&timer) as Arc<dyn GapTimer>);
        let _ = timer.context.set(Arc::downgrade(&outpost.context));
        let context = &outpost.context;

        assert_eq!(
            context.gap_copy_dropped(0x11, 0x10, 1_000),
            CopyDropped::Opened { due_us: 1_001_000 }
        );
        context.push(
            Item::Fact(DeliveredFact::UiaFocusMarker { marker: 7 }, None),
            TraceId::mint(),
            1_500,
            EventTiming::default(),
        );
        context.push(
            Item::Fact(
                DeliveredFact::UiaFocusDropped {
                    marker: 7,
                    focus: true,
                    answer: GapAnswer::Answers(Some(0x10)),
                },
                None,
            ),
            TraceId::mint(),
            1_500,
            EventTiming::default(),
        );
        if let Some(worker) = timer
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            worker.join().expect("the worker's step");
        }

        assert!(context.intake.gap_open(), "the second copy's record");
        assert_eq!(
            *timer.due_us.lock().unwrap_or_else(PoisonError::into_inner),
            Some(SECOND_COPY.2 + 1_000_000),
            "its bound"
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
        let context = &quiet_context();
        let trace = TraceId::mint();
        let mut replies = Vec::new();
        context.watch.start(STEP_DEADLINE, Some((7, trace)), 0);

        // The watchdog abandons the worker and answers its query.
        if let Some((request_id, _)) = context.watch.lock().abandon() {
            replies.push((request_id, "abandoned"));
        }
        // The old worker's call returns and it commits its answer.
        let (committed, _) = context.watch.commit(context, 0, reply_effects(7, trace));
        if committed == Committed::Applied {
            replies.push((7, "done"));
        }

        assert_eq!(replies, [(7, "abandoned")]);
    }

    #[test]
    fn a_published_query_can_no_longer_be_abandoned() {
        let context = &quiet_context();
        let trace = TraceId::mint();
        let mut replies = Vec::new();
        context.watch.start(STEP_DEADLINE, Some((7, trace)), 0);

        let (committed, _) = context.watch.commit(context, 0, reply_effects(7, trace));
        if committed == Committed::Applied {
            replies.push((7, "done"));
        }
        // A watchdog that wakes just after finds no query to answer.
        if let Some((request_id, _)) = context.watch.lock().abandon() {
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

    /// A button in no window, node 1, as a focus walk's progress holds it.
    fn bound_focus() -> BoundFocus {
        BoundFocus {
            trace: TraceId::mint(),
            observed_at_us: 1,
            timing: EventTiming::default(),
            batch: 0,
            backend: Backend::Msaa,
            window: None,
            node: node(1, Role::Button),
            object: None,
            msaa_event: None,
            group: None,
        }
    }

    /// A node `id` of `role`, read through MSAA.
    fn node(id: u64, role: Role) -> NodeSnapshot {
        NodeSnapshot {
            id: NodeId::new(id),
            backend: Backend::Msaa,
            role,
            name: None,
            value: None,
            states: StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        }
    }

    /// An outpost's context whose worker and watchdog are stopped, so that
    /// nothing but the test touches the watch: the first worker,
    /// generation 0, is in charge. Its outbound queue is closed, so what a
    /// commit sends goes nowhere; what it records and follows is kept.
    fn quiet_context() -> Arc<Context> {
        use verbatim_process::overlapped::{End, Flow, pipe};
        use verbatim_process::transport::EVENT_PIPE_BUFFER;

        let (_ours, theirs) =
            pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped).expect("a pipe");
        let outpost = super::super::Outpost::new(theirs, std::process::id());
        let context = Arc::clone(&outpost.context);
        outpost.shutdown();
        context.watch.lock().stopping = false;
        context
    }

    /// The effects of the answer to query `request_id`, traced `trace`.
    fn reply_effects(request_id: u64, trace: TraceId) -> Effects {
        Effects {
            result: Some(OutpostToSupervisor::Reply {
                trace_id: trace,
                request_id,
                outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Done)),
                timing: EventTiming::default(),
            }),
            ..Effects::default()
        }
    }

    /// The effects of reporting `node` as the focus and following it, with
    /// `owed` owed.
    fn focus_effects(context: &Context, node: &NodeSnapshot, owed: Option<OwedWork>) -> Effects {
        let (mut follow, _) = follow_of(
            context,
            FocusMade {
                node: (node.id, node.role),
                object: None,
                window: None,
                chain: vec![node.clone()],
                batch: 0,
                event: (TraceId::mint(), 1),
                redirected: false,
                caret: None,
                at_bound: false,
            },
        );
        follow.owed = owed;
        Effects {
            result: Some(OutpostToSupervisor::IdleAnswer { round: 1 }),
            report: Some(Report::Reported(
                judged_focus(context, (TraceId::mint(), node, &[]), (None, false), None),
                held::Evidence::Msaa,
            )),
            follow: Some(follow),
            ..Effects::default()
        }
    }

    /// The node the outpost follows, and how many focuses wait for Core's
    /// judgement.
    fn followed(context: &Context) -> (Option<NodeId>, usize) {
        (
            context.watch.follow(|follow| follow.tracking.focus),
            context.held().focuses().count(),
        )
    }

    /// Arms a walk of `bound_focus` for generation 0 under a fresh entry
    /// concerning `window`, `left` from its bound, returning when the entry
    /// started and the walk's bound.
    fn armed(context: &Context, window: isize, left: Duration) -> (Instant, Instant) {
        context.watch.start(HANDLING_DEADLINE, None, window);
        context
            .watch
            .arm(FocusProgress::new(0, bound_focus(), 0, left))
            .expect("the walk armed");
        let state = context.watch.lock();
        (
            state.started.expect("started").0,
            state.walk.as_ref().expect("armed").bound_at,
        )
    }

    /// A walk that returns before its bound is disarmed, and one that
    /// returns past it, unpublished, stays armed for its worker to publish
    /// it from its progress, as the watchdog would.
    #[test]
    fn a_walk_returned_past_its_bound_is_left_for_its_worker_to_publish() {
        let watch = Watch::default();
        let arm = || {
            watch.start(HANDLING_DEADLINE, None, 0);
            watch.arm(FocusProgress::new(
                0,
                bound_focus(),
                0,
                Duration::from_secs(1),
            ));
            watch.lock().walk.as_ref().expect("the walk armed").bound_at
        };
        let bound = arm();
        assert!(matches!(
            watch.walk_returned(0, bound),
            Returned::PastBound(_)
        ));
        assert!(watch.lock().walk.is_some(), "still armed");
        let before = arm()
            .checked_sub(Duration::from_millis(1))
            .expect("a time before the bound");
        assert!(matches!(watch.walk_returned(0, before), Returned::Disarmed));
        assert!(watch.lock().walk.is_none(), "disarmed");
    }

    /// One wake past both a walk's bound and its worker's deadline
    /// publishes the walk, once, and makes it the followed focus, before
    /// the worker is abandoned.
    #[test]
    fn a_walk_past_its_bound_is_published_before_its_worker_is_abandoned() {
        let context = &quiet_context();
        let (_, bound) = armed(context, 0, Duration::from_millis(500));
        let woken = wake(context, bound + HANDLING_DEADLINE, |_| false);
        assert!(matches!(
            woken,
            Wake::Abandoned(Abandoned { generation: 1, .. })
        ));
        assert_eq!(followed(context), (Some(NodeId::new(1)), 1));
    }

    /// A walk with a 2-second bound whose worker the moved-on rule abandons
    /// at the end of the grace is published before the abandonment.
    #[test]
    fn a_walk_is_published_when_the_user_moves_on_before_its_bound() {
        const WINDOW: isize = 0x1234;
        let context = &quiet_context();
        let (started, bound) = armed(context, WINDOW, Duration::from_secs(2));
        assert!(started + MOVED_ON_GRACE < bound);
        let woken = wake(context, started + MOVED_ON_GRACE, |window| window == WINDOW);
        assert!(matches!(
            woken,
            Wake::Abandoned(Abandoned { generation: 1, .. })
        ));
        assert_eq!(followed(context), (Some(NodeId::new(1)), 1));
    }

    /// A focus published at its walk's bound is the followed focus from
    /// that commit, before its worker learns of it: the tracking and the
    /// intake's focused object are its own.
    #[test]
    fn a_focus_published_at_its_bound_is_followed_at_once() {
        let context = &quiet_context();
        let (_, bound) = armed(context, 0, Duration::from_millis(500));
        context
            .watch
            .lock()
            .walk
            .as_mut()
            .expect("armed")
            .focus
            .object = Some(Object::Msaa(5, OBJID_CLIENT.0, CHILDID_SELF));
        assert!(matches!(wake(context, bound, |_| false), Wake::Nothing));
        assert_eq!(
            (
                context.watch.follow(|follow| follow.tracking.focus),
                context.intake.focused(),
            ),
            (
                Some(NodeId::new(1)),
                Some(Object::Msaa(5, OBJID_CLIENT.0, CHILDID_SELF))
            )
        );
        assert!(matches!(
            context.watch.walk_returned(0, Instant::now()),
            Returned::Published
        ));
    }

    /// An abandoned worker's commit changes nothing: not the judgement
    /// list, the followed focus, the subscription's target, nor what is
    /// owed.
    #[test]
    fn an_abandoned_workers_commit_changes_nothing() {
        let context = &quiet_context();
        context.watch.start(HANDLING_DEADLINE, None, 0);
        let _ = context.watch.lock().abandon();
        let effects = focus_effects(
            context,
            &node(3, Role::Terminal),
            Some(OwedWork::FirstRead(None)),
        );
        let (committed, _) = context.watch.commit(context, 0, effects);
        assert_eq!(committed, Committed::Refused);
        assert_eq!(followed(context), (None, 0));
        assert!(context.watch.follow(|follow| follow.owed.is_none()));
        assert_eq!(context.intake.focused(), None);
    }

    /// Owed work is taken up once, by the worker in charge, under a
    /// deadline only the deadline ends: its next check is the deadline,
    /// even with the user moved on. Abandoning the worker attempting it
    /// clears it, so the worker that replaces it starts with nothing owed.
    #[test]
    fn owed_work_is_attempted_once_and_only_its_deadline_abandons_it() {
        let context = &quiet_context();
        let terminal = node(3, Role::Terminal);
        context.watch.start(HANDLING_DEADLINE, None, 0x1234);
        let effects = focus_effects(context, &terminal, Some(OwedWork::FirstRead(None)));
        assert_eq!(
            context.watch.commit(context, 0, effects).0,
            Committed::Applied
        );
        let _ = context.watch.lock().abandon();
        // The replacement is in an entry concerning a window when it takes
        // the owed work up.
        context.watch.start(HANDLING_DEADLINE, None, 0x1234);
        assert!(
            context.watch.take_owed(1).is_some(),
            "the replacement takes it"
        );
        assert!(context.watch.take_owed(1).is_none(), "once");
        let (deadline, started) = {
            let state = context.watch.lock();
            (
                state.deadline.expect("the attempt's deadline"),
                state.started.expect("started"),
            )
        };
        let after_grace = started.0 + MOVED_ON_GRACE;
        assert_eq!(
            (
                started.1,
                abandon_reason(after_grace, deadline, started.0, started.1, |_| true),
            ),
            (0, None),
            "the moved-on rule does not abandon the attempt"
        );
        let _ = context.watch.lock().abandon();
        assert!(context.watch.follow(|follow| follow.owed.is_none()));
        assert!(context.watch.take_owed(2).is_none());
    }

    /// A newer focus's commit replaces the work the focus before it owed.
    #[test]
    fn a_newer_focus_replaces_the_owed_work_of_the_one_before() {
        let context = &quiet_context();
        context.watch.start(HANDLING_DEADLINE, None, 0);
        let first = focus_effects(
            context,
            &node(3, Role::Terminal),
            Some(OwedWork::FirstRead(None)),
        );
        let _ = context.watch.commit(context, 0, first);
        let second = focus_effects(context, &node(4, Role::Button), None);
        let _ = context.watch.commit(context, 0, second);
        assert!(context.watch.follow(|follow| follow.owed.is_none()));
    }

    /// A terminal read by a worker abandoned during it leaves the
    /// terminal's entry as it was: the read worked on a copy, and its
    /// commit is refused.
    #[test]
    fn a_terminal_read_by_an_abandoned_worker_leaves_its_entry_as_it_was() {
        let context = &quiet_context();
        let terminal = NodeId::new(3);
        context.watch.start(HANDLING_DEADLINE, None, 0);
        let mut copy = context.watch.follow(|follow| {
            follow
                .terminals
                .get(&terminal.number())
                .cloned()
                .unwrap_or_default()
        });
        copy.reading = Reading::Held { changed: true };
        let _ = context.watch.lock().abandon();
        let (committed, _) = context.watch.commit(
            context,
            0,
            Effects {
                terminal: Some((terminal, copy)),
                ..Effects::default()
            },
        );
        assert_eq!(committed, Committed::Refused);
        assert!(
            context
                .watch
                .follow(|follow| !follow.terminals.contains_key(&terminal.number()))
        );
    }

    /// A terminal entry a commit replaces, which may hold the last
    /// reference to a text range of a hung terminal, is handed back by the
    /// apply code, so the commit drops it once the watch lock is let go.
    #[test]
    fn a_terminal_entry_a_commit_replaces_is_handed_back_to_drop_after_the_lock() {
        let context = &quiet_context();
        let terminal = NodeId::new(3);
        let mut follow = FollowState::default();
        let mut old = crate::terminal::Terminal::default();
        old.reading = Reading::Held { changed: true };
        follow.terminals.insert(terminal.number(), old);
        let (_, displaced) = follow.apply(
            context,
            0,
            Effects {
                terminal: Some((terminal, crate::terminal::Terminal::default())),
                ..Effects::default()
            },
        );
        let readings: Vec<Reading> = displaced
            .terminals
            .iter()
            .map(|entry| entry.reading)
            .collect();
        assert_eq!(readings, [Reading::Held { changed: true }]);
        assert_eq!(
            follow
                .terminals
                .get(&terminal.number())
                .map(|entry| entry.reading),
            Some(Reading::default())
        );
    }

    /// The subscription is moved to a focus's element, found after its
    /// report, only while that focus is still the followed one.
    #[test]
    fn a_slider_is_added_only_while_its_focus_is_followed() {
        let context = &quiet_context();
        context.watch.start(HANDLING_DEADLINE, None, 0);
        let followed_now = focus_effects(context, &node(4, Role::Button), None);
        let _ = context.watch.commit(context, 0, followed_now);
        let before = context.watch.follow(|follow| follow.followed);
        let late = Target {
            wanted: Some((
                NodeId::new(3),
                verbatim_uia::Group::Base,
                Some(NodeId::new(2)),
            )),
            element: None,
            slider: None,
        };
        let _ = context.watch.commit(
            context,
            0,
            Effects {
                element: Some((NodeId::new(3), late)),
                ..Effects::default()
            },
        );
        assert_eq!(context.watch.follow(|follow| follow.followed), before);
    }

    /// A walk that meets the chain of the focus before splices the entries
    /// above where they meet, each recorded as carried by the message the
    /// chain goes in, as a node looked up is; a chain whose outer entry the
    /// store has released offers nothing below it to splice from, so the
    /// walk reads on.
    #[test]
    fn a_spliced_entry_is_recorded_as_carried_and_none_is_spliced_from_a_released_one() {
        let context = quiet_context();
        let registry = &context.msaa_registry;
        let window = registry.id_for((7, OBJID_WINDOW.0, CHILDID_SELF));
        let pane = registry.id_for((8, OBJID_WINDOW.0, CHILDID_SELF));
        let entry = |id: NodeId| NodeSnapshot {
            id,
            ..node(0, Role::Pane)
        };
        let previous = vec![entry(window), entry(pane)];
        let _ = take_touched(&context);
        let spliced = read::splice(
            &context,
            &read::held_prefix(&context, previous.clone()),
            vec![entry(pane), node(99, Role::Button)],
            pane,
        );
        assert_eq!(
            spliced.iter().map(|node| node.id).collect::<Vec<_>>(),
            vec![window, pane, NodeId::new(99)]
        );
        assert_eq!(take_touched(&context), vec![window.number()]);
        drop(registry.retain(|id| id != window));
        assert!(read::held_prefix(&context, previous).is_empty());
    }
}
