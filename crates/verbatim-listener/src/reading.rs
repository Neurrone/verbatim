//! The listener's reading threads (`books/dev/src/reference/crates/verbatim-listener.md`,
//! "Tells, checks and Core's requests"; `books/dev/src/reference/crates/verbatim-listener-logic.md`,
//! "The reading threads' schedule"): the shell of the pure schedule
//! ([`verbatim_listener_logic::schedule`]), one thread per application
//! process generation, running that application's jobs, Core's requests
//! ahead of the releases of kept elements.
//!
//! The schedule and the threads' wakes sit under one lock; every
//! transition's effects are applied under it, so a request's answer, sent
//! as its job ends, goes ahead of the idle barrier's answer that its end
//! lets run. A thread makes its calls with no lock held. A thread whose
//! request Core cancels for the foreground is abandoned, and its result
//! discarded when its call returns. Threads are not
//! joined at the listener's shutdown: one inside a call is let go, as the
//! check threads are.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use verbatim_listener_logic::schedule::{
    AppKey, Effects, Ended, JobKind, Next, Returned, Schedule,
};
use verbatim_model::Pid;
use verbatim_model::request::{Cancel, RequestNumber};

/// What the idle barrier runs once the jobs queued before it have ended.
pub(crate) type IdleWaiter = Box<dyn FnOnce() + Send>;

/// The work the reading threads do: the listener's, or a test's stand-in.
pub(crate) trait Reading: Send + Sync + 'static {
    /// One job.
    type Job: Send + 'static;
    /// What a thread keeps between its jobs, made on the thread.
    type Thread;
    /// What a job's run hands back for its end under the lock.
    type Done: Send;
    /// What a job's end lets go of once the lock is let go: a release can
    /// call into an application.
    type Leftover;
    /// Makes a thread's state for `pid`'s jobs, on that thread.
    fn thread(&self, pid: Pid) -> Self::Thread;
    /// Runs `job`, the request `number` when it is one, with no lock held.
    fn run(
        &self,
        thread: &mut Self::Thread,
        job: Self::Job,
        number: Option<RequestNumber>,
    ) -> Self::Done;
    /// A job's end, under the lock: what it owes is sent when `send`, its
    /// job still its own; an abandoned job's result is discarded.
    fn done(&self, done: Self::Done, send: bool) -> Self::Leftover;
    /// A job ended without being run, under the lock.
    fn ended(&self, ended: Ended<Self::Job>);
    /// The thread running request `number` was abandoned for the
    /// foreground, under the lock: the request is answered abandoned.
    fn abandoned(&self, number: RequestNumber);
    /// Lets a thread's state go as the thread ends, on that thread.
    fn finish(&self, thread: Self::Thread);
}

/// What sits under the lock.
struct State<J> {
    schedule: Schedule<J, IdleWaiter>,
    /// What each application's thread in charge waits on.
    wakes: HashMap<AppKey, Arc<Condvar>>,
    /// The threads started, finished ones let go as more start.
    threads: Vec<JoinHandle<()>>,
}

/// The time the schedule reads, a test's or the system's.
pub(crate) type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

struct Shared<R: Reading> {
    reading: R,
    clock: Clock,
    /// The time the schedule's microseconds count from.
    origin: Instant,
    state: Mutex<State<R::Job>>,
    /// The failed cached reads the threads have made
    /// (`verbatim_uia::calls`).
    failed_reads: AtomicU32,
}

impl<R: Reading> Shared<R> {
    fn lock(&self) -> MutexGuard<'_, State<R::Job>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Now, in the schedule's microseconds.
    fn now(&self) -> u64 {
        u64::try_from(
            (self.clock)()
                .saturating_duration_since(self.origin)
                .as_micros(),
        )
        .unwrap_or(u64::MAX)
    }

    /// Applies `effects` under the lock, in the schedule's order; a thread
    /// that cannot be started has its failure applied as a transition of
    /// its own.
    fn apply(self: &Arc<Self>, state: &mut State<R::Job>, effects: Effects<R::Job, IdleWaiter>) {
        let mut failed = Vec::new();
        for (app, thread) in effects.start {
            if !self.spawn(state, app, thread) {
                failed.push((app, thread));
            }
        }
        for app in effects.wake {
            if let Some(wake) = state.wakes.get(&app) {
                wake.notify_all();
            }
        }
        for app in effects.forget {
            state.wakes.remove(&app);
        }
        for ended in effects.ended {
            self.reading.ended(ended);
        }
        for number in effects.abandoned {
            self.reading.abandoned(number);
        }
        for waiter in effects.waiters {
            waiter();
        }
        for (app, thread) in failed {
            let effects = state.schedule.spawn_failed(app, thread);
            self.apply(state, effects);
        }
    }

    /// Starts thread `thread` in charge of `app`'s jobs: whether it
    /// started.
    fn spawn(self: &Arc<Self>, state: &mut State<R::Job>, app: AppKey, thread: u64) -> bool {
        let wake = Arc::clone(state.wakes.entry(app).or_default());
        let shared = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name(READING_THREAD.to_owned())
            .spawn(move || run(&shared, app, thread, &wake));
        match spawned {
            Ok(handle) => {
                state.threads.retain(|thread| !thread.is_finished());
                state.threads.push(handle);
                true
            }
            Err(error) => {
                tracing::error!(pid = %app.pid, %error, "a reading thread could not be started");
                false
            }
        }
    }
}

/// A reading thread's name.
const READING_THREAD: &str = "verbatim-listener-read";

/// A reading thread: runs `app`'s jobs while it is in charge of them, as
/// thread `thread`, waiting on `wake` for more up to the idle time, then
/// lets its state go and ends.
fn run<R: Reading>(shared: &Arc<Shared<R>>, app: AppKey, thread: u64, wake: &Arc<Condvar>) {
    let mut own = shared.reading.thread(app.pid);
    let mut thread = thread;
    while let Some((job, number)) = next(shared, app, thread, wake) {
        let done = shared.reading.run(&mut own, job, number);
        shared
            .failed_reads
            .fetch_add(verbatim_uia::calls::take_failed_reads(), Ordering::Relaxed);
        let (returned, leftover) = {
            let mut state = shared.lock();
            let (returned, effects) = state.schedule.returned(app, thread, shared.now());
            let leftover = shared.reading.done(done, returned.send);
            shared.apply(&mut state, effects);
            (returned, leftover)
        };
        drop(leftover);
        let Returned {
            reader: Some(going_on),
            ..
        } = returned
        else {
            break;
        };
        thread = going_on;
    }
    shared.reading.finish(own);
}

/// The next job for thread `thread` of `app`, waiting on `wake` for one up
/// to the idle time; `None` when the thread is to end.
fn next<R: Reading>(
    shared: &Arc<Shared<R>>,
    app: AppKey,
    thread: u64,
    wake: &Arc<Condvar>,
) -> Option<(R::Job, Option<RequestNumber>)> {
    let mut state = shared.lock();
    loop {
        let now = shared.now();
        let (next, effects) = state.schedule.next(app, thread, now);
        shared.apply(&mut state, effects);
        match next {
            Next::Run { job, number } => return Some((job, number)),
            Next::Exit => return None,
            Next::Wait { until_us } => {
                let wait = Duration::from_micros(until_us.saturating_sub(now));
                state = wake
                    .wait_timeout(state, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
        }
    }
}

/// The reading threads.
pub(crate) struct Readers<R: Reading> {
    shared: Arc<Shared<R>>,
}

impl<R: Reading> Readers<R> {
    /// Readers doing `reading` on the system's clock.
    pub(crate) fn new(reading: R) -> Self {
        Self::with_clock(reading, Arc::new(Instant::now))
    }

    /// [`new`](Self::new)'s readers, whose schedule reads now from `clock`.
    pub(crate) fn with_clock(reading: R, clock: Clock) -> Self {
        let shared = Arc::new(Shared {
            reading,
            origin: clock(),
            clock,
            state: Mutex::new(State {
                schedule: Schedule::default(),
                wakes: HashMap::new(),
                threads: Vec::new(),
            }),
            failed_reads: AtomicU32::new(0),
        });
        Self { shared }
    }

    /// The reading the threads do.
    pub(crate) fn reading(&self) -> &R {
        &self.shared.reading
    }

    /// Queues `job`, of `kind`, for `pid`'s reading thread, starting one
    /// when it has none in charge. Never waits on a call.
    pub(crate) fn push(&self, pid: Pid, kind: JobKind, job: R::Job) {
        let mut state = self.shared.lock();
        let effects = state.schedule.push(pid, kind, job, self.shared.now());
        self.shared.apply(&mut state, effects);
    }

    /// Core cancels a request (`books/dev/src/reference/crates/verbatim-listener-logic.md`, "The reading threads' schedule"). Never waits on a call.
    pub(crate) fn cancel(&self, cancel: Cancel) {
        let mut state = self.shared.lock();
        let effects = state
            .schedule
            .cancel(cancel.number, cancel.reason, self.shared.now());
        self.shared.apply(&mut state, effects);
    }

    /// `pid`'s process has exited: its record leaves the running
    /// applications. Never waits on a call.
    pub(crate) fn application_exited(&self, pid: Pid) {
        let mut state = self.shared.lock();
        let effects = state.schedule.application_exited(pid);
        self.shared.apply(&mut state, effects);
    }

    /// Runs `waiter` once every job queued before this call has ended, at
    /// once when none waits or runs, under the lock. Dropped unrun once
    /// the readers are stopping.
    pub(crate) fn when_idle(&self, waiter: IdleWaiter) {
        let mut state = self.shared.lock();
        let effects = state.schedule.when_idle(waiter);
        self.shared.apply(&mut state, effects);
    }

    /// The failed cached reads the threads have made so far.
    pub(crate) fn failed_reads(&self) -> u32 {
        self.shared.failed_reads.load(Ordering::Relaxed)
    }

    /// Stops the readers for the listener's shutdown: every waiting job is
    /// ended unrun and every waiting thread woken to end; a thread inside
    /// a call is let go. Returns how many threads
    /// were started and not yet seen to end.
    pub(crate) fn stop(&self) -> usize {
        let threads = {
            let mut state = self.shared.lock();
            let effects = state.schedule.stop();
            self.shared.apply(&mut state, effects);
            std::mem::take(&mut state.threads)
        };
        threads
            .into_iter()
            .filter(|thread| !thread.is_finished())
            .count()
    }
}

#[cfg(test)]
mod tests;
