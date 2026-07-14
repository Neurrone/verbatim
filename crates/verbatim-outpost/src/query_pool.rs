//! A small pool of query threads with per-call deadlines and thread
//! abandonment (architecture section 1, recovery ladder rung 2).
//!
//! Each worker owns its own UIA client (created lazily on first use, in the
//! multithreaded apartment) so nothing COM is shared across threads. Two kinds
//! of work are submitted:
//!
//! - [`QueryPool::run`] — a deadline-guarded request/response call (a fetch, the
//!   arbitration probe, the synthetic focus query). If the deadline expires the
//!   caller stops awaiting the result, the blocked worker is abandoned (a hung
//!   cross-process COM call cannot be safely cancelled — D9), the parked-thread
//!   counter is bumped, and a replacement worker is spawned to keep capacity.
//! - [`QueryPool::submit`] — fire-and-forget work (out-of-context MSAA event
//!   acquisition), run on a worker without a deadline; a genuinely hung app is
//!   caught at the coarser heartbeat level and the supervisor respawns.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use verbatim_uia::Uia;

/// A unit of work handed to a worker, given exclusive access to that worker's
/// state (chiefly its UIA client).
type Job = Box<dyn FnOnce(&mut Worker) + Send>;

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
}

impl QueryPool {
    /// Creates a pool of `size` worker threads.
    #[must_use]
    pub fn new(size: usize) -> Self {
        let (jobs_tx, jobs_rx) = unbounded::<Job>();
        let pool = Self {
            jobs_tx,
            jobs_rx,
            parked: Arc::new(AtomicUsize::new(0)),
        };
        for _ in 0..size.max(1) {
            pool.spawn_worker();
        }
        pool
    }

    /// The number of workers abandoned to timed-out calls so far — the signal
    /// the supervisor uses to decide when to kill and respawn (ladder rung 3).
    #[must_use]
    pub fn parked_count(&self) -> usize {
        self.parked.load(Ordering::Relaxed)
    }

    fn spawn_worker(&self) {
        let jobs_rx = self.jobs_rx.clone();
        let _ = thread::Builder::new()
            .name("verbatim-query".to_owned())
            .spawn(move || {
                let mut worker = Worker::new();
                while let Ok(job) = jobs_rx.recv() {
                    job(&mut worker);
                }
            });
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
                self.spawn_worker();
                tracing::warn!(
                    parked = self.parked_count(),
                    "query call abandoned after deadline; worker parked, replacement spawned"
                );
                None
            }
            Err(RecvTimeoutError::Disconnected) => None,
        }
    }

    /// Submits fire-and-forget `work` to run on a worker with no deadline.
    pub fn submit<F>(&self, work: F)
    where
        F: FnOnce(&mut Worker) + Send + 'static,
    {
        let job: Job = Box::new(work);
        let _ = self.jobs_tx.send(job);
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
    fn submit_runs_work() {
        let pool = QueryPool::new(1);
        let (tx, rx) = bounded::<u32>(1);
        pool.submit(move |_| {
            let _ = tx.send(99);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(99));
    }
}
