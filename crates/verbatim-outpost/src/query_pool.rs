//! A small pool of query threads with per-call deadlines and thread
//! abandonment (architecture section 1, recovery ladder rung 2).
//!
//! Each worker owns its own UIA client (created lazily on first use, in the
//! multithreaded apartment) so nothing COM is shared across threads. Every
//! cross-process accessibility call carries a deadline (the founding rule of
//! architecture section 1), whether request/response or fire-and-forget:
//!
//! - [`QueryPool::run`] — a deadline-guarded request/response call (a fetch, the
//!   arbitration probe, the synthetic focus query). If the deadline expires the
//!   caller stops awaiting the result, the blocked worker is abandoned (a hung
//!   cross-process COM call cannot be safely cancelled — D9), the parked-thread
//!   counter is bumped, and a replacement worker is spawned to keep capacity.
//! - [`QueryPool::submit_deadline`] — fire-and-forget work (out-of-context MSAA
//!   event acquisition, a fetch's snapshot re-read) whose result nobody awaits,
//!   but still deadline-guarded: a single watchdog thread tracks each submitted
//!   job's deadline and, if the job has not signalled completion by then, bumps
//!   the parked-thread counter and spawns a replacement worker — the same
//!   abandonment and visibility semantics `run` gives, so a hung acquisition
//!   restores capacity and shows up in `parked_count` (the wedge policy reads
//!   it) instead of silently draining the pool to zero.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use verbatim_uia::Uia;

/// A unit of work handed to a worker, given exclusive access to that worker's
/// state (chiefly its UIA client).
type Job = Box<dyn FnOnce(&mut Worker) + Send>;

/// One deadline-guarded fire-and-forget job the watchdog tracks: the instant
/// its deadline expires, and a flag the job sets when it completes. If the flag
/// is still clear at the deadline, the job is presumed hung on a cross-process
/// call and its worker abandoned.
struct Watch {
    deadline_at: Instant,
    done: Arc<AtomicBool>,
}

/// Per-worker state. The UIA client is created on first use, on the worker
/// thread, so it never crosses threads.
pub struct Worker {
    uia: Option<Uia>,
    uia_failed: bool,
}

impl Worker {
    fn new() -> Self {
        Self {
            uia: None,
            uia_failed: false,
        }
    }

    /// Returns this worker's UIA client, creating it on first use. Returns
    /// `None` if the client could not be created (retried on the next call
    /// only if it has not already hard-failed).
    pub fn uia(&mut self) -> Option<&Uia> {
        if self.uia.is_none() && !self.uia_failed {
            match Uia::new() {
                Ok(client) => self.uia = Some(client),
                Err(error) => {
                    self.uia_failed = true;
                    tracing::warn!(%error, "query worker could not create a UIA client");
                }
            }
        }
        self.uia.as_ref()
    }
}

/// A pool of query-executing worker threads.
#[derive(Clone)]
pub struct QueryPool {
    jobs_tx: Sender<Job>,
    jobs_rx: Receiver<Job>,
    parked: Arc<AtomicUsize>,
    /// Registers a fire-and-forget job with the watchdog thread (see
    /// [`Self::submit_deadline`]).
    watch_tx: Sender<Watch>,
}

impl QueryPool {
    /// Creates a pool of `size` worker threads plus its single watchdog thread.
    #[must_use]
    pub fn new(size: usize) -> Self {
        let (jobs_tx, jobs_rx) = unbounded::<Job>();
        let (watch_tx, watch_rx) = unbounded::<Watch>();
        let parked = Arc::new(AtomicUsize::new(0));
        // The watchdog spawns replacement workers for abandoned jobs, so it
        // holds its own clone of the jobs receiver and the parked counter.
        let watchdog_jobs_rx = jobs_rx.clone();
        let watchdog_parked = Arc::clone(&parked);
        let _ = thread::Builder::new()
            .name("verbatim-query-watchdog".to_owned())
            .spawn(move || watchdog_loop(&watch_rx, &watchdog_jobs_rx, &watchdog_parked));
        let pool = Self {
            jobs_tx,
            jobs_rx,
            parked,
            watch_tx,
        };
        for _ in 0..size.max(1) {
            spawn_worker(&pool.jobs_rx);
        }
        pool
    }

    /// The number of workers abandoned to timed-out calls so far — the signal
    /// the supervisor uses to decide when to kill and respawn (ladder rung 3).
    #[must_use]
    pub fn parked_count(&self) -> usize {
        self.parked.load(Ordering::Relaxed)
    }

    /// Runs `work` on a worker and waits up to `deadline` for its result.
    /// Returns `None` if the deadline expires (the worker is abandoned and a
    /// replacement spawned) or the pool is shutting down.
    pub fn run<T, F>(&self, deadline: Duration, work: F) -> Option<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Worker) -> T + Send + 'static,
    {
        let (result_tx, result_rx) = bounded::<T>(1);
        let job: Job = Box::new(move |worker| {
            // If the deadline already passed, the receiver is gone and the send
            // simply fails; the work still ran to completion on the worker.
            let _ = result_tx.send(work(worker));
        });
        self.jobs_tx.send(job).ok()?;
        match result_rx.recv_timeout(deadline) {
            Ok(value) => Some(value),
            Err(RecvTimeoutError::Timeout) => {
                self.parked.fetch_add(1, Ordering::Relaxed);
                spawn_worker(&self.jobs_rx);
                tracing::warn!(
                    parked = self.parked_count(),
                    "query call abandoned after deadline; worker parked, replacement spawned"
                );
                None
            }
            Err(RecvTimeoutError::Disconnected) => None,
        }
    }

    /// Submits fire-and-forget `work` to run on a worker, deadline-guarded by
    /// the watchdog: the job runs as `run`'s does, but nobody awaits its result;
    /// if it has not completed within `deadline` the watchdog abandons its
    /// worker (bumps [`parked_count`](Self::parked_count), spawns a replacement),
    /// exactly as `run` does on a timeout. Use this for every cross-process
    /// accessibility call whose result is not awaited; an unbounded submit would
    /// let one hung call drain the pool invisibly.
    pub fn submit_deadline<F>(&self, deadline: Duration, work: F)
    where
        F: FnOnce(&mut Worker) + Send + 'static,
    {
        let done = Arc::new(AtomicBool::new(false));
        let done_for_job = Arc::clone(&done);
        let job: Job = Box::new(move |worker| {
            work(worker);
            done_for_job.store(true, Ordering::Relaxed);
        });
        // If the job send fails the pool is shutting down; skip the watch too.
        if self.jobs_tx.send(job).is_ok() {
            let _ = self.watch_tx.send(Watch {
                deadline_at: Instant::now() + deadline,
                done,
            });
        }
    }
}

/// Spawns one worker thread that pulls jobs off `jobs_rx` until the pool is
/// dropped. Shared by pool construction, `run`'s timeout replacement, and the
/// watchdog's abandonment replacement.
fn spawn_worker(jobs_rx: &Receiver<Job>) {
    let jobs_rx = jobs_rx.clone();
    let _ = thread::Builder::new()
        .name("verbatim-query".to_owned())
        .spawn(move || {
            let mut worker = Worker::new();
            while let Ok(job) = jobs_rx.recv() {
                job(&mut worker);
            }
        });
}

/// The watchdog thread behind [`QueryPool::submit_deadline`]: it holds the
/// pending fire-and-forget jobs, wakes at the earliest deadline (or when a new
/// job registers), and for each job past its deadline that has not completed,
/// abandons the worker running it — bumping `parked` and spawning a replacement,
/// with the same warning `run` logs. Exits when every [`QueryPool`] clone has
/// dropped (the watch sender disconnects).
fn watchdog_loop(watch_rx: &Receiver<Watch>, jobs_rx: &Receiver<Job>, parked: &Arc<AtomicUsize>) {
    let mut pending: Vec<Watch> = Vec::new();
    loop {
        // Wake at the soonest deadline, or block until a job registers when
        // there is nothing to watch.
        let now = Instant::now();
        let next_wake = pending
            .iter()
            .map(|watch| watch.deadline_at.saturating_duration_since(now))
            .min();
        let received = match next_wake {
            Some(timeout) => watch_rx.recv_timeout(timeout),
            None => watch_rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(watch) => pending.push(watch),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // Drop completed jobs; abandon any past its deadline that has not.
        let now = Instant::now();
        pending.retain(|watch| {
            if watch.done.load(Ordering::Relaxed) {
                return false;
            }
            if now >= watch.deadline_at {
                parked.fetch_add(1, Ordering::Relaxed);
                spawn_worker(jobs_rx);
                tracing::warn!(
                    parked = parked.load(Ordering::Relaxed),
                    "submitted query abandoned after deadline; worker parked, replacement spawned"
                );
                return false;
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_work_result() {
        let pool = QueryPool::new(2);
        let value = pool.run(Duration::from_secs(5), |_| 21 * 2);
        assert_eq!(value, Some(42));
    }

    #[test]
    fn timed_out_call_is_abandoned_and_capacity_restored() {
        let pool = QueryPool::new(1);
        // A single worker blocked past the deadline: the call is abandoned.
        let timed_out = pool.run(Duration::from_millis(50), |_| {
            thread::sleep(Duration::from_millis(400));
            1
        });
        assert_eq!(timed_out, None);
        assert_eq!(pool.parked_count(), 1);
        // A replacement worker was spawned, so the pool still serves calls.
        let recovered = pool.run(Duration::from_secs(5), |_| 7);
        assert_eq!(recovered, Some(7));
    }

    #[test]
    fn a_hung_submit_deadline_job_parks_and_capacity_is_restored() {
        let pool = QueryPool::new(1);
        // A single worker hung well past its deadline: the watchdog abandons it.
        pool.submit_deadline(Duration::from_millis(50), |_| {
            thread::sleep(Duration::from_millis(600));
        });
        // Give the watchdog time to reach the deadline and park the worker.
        thread::sleep(Duration::from_millis(250));
        assert_eq!(pool.parked_count(), 1);
        // Capacity is restored: a run is served by the replacement worker while
        // the original is still blocked in the hung job.
        let recovered = pool.run(Duration::from_secs(5), |_| 7);
        assert_eq!(recovered, Some(7));
    }

    #[test]
    fn a_completing_submit_deadline_job_does_not_park() {
        let pool = QueryPool::new(1);
        let (tx, rx) = bounded::<u32>(1);
        // Completes essentially immediately, well before its deadline.
        pool.submit_deadline(Duration::from_millis(200), move |_| {
            let _ = tx.send(99);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(99));
        // Wait past the deadline: the watchdog must see completion, not park.
        thread::sleep(Duration::from_millis(350));
        assert_eq!(pool.parked_count(), 0);
    }
}
