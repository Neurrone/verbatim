//! Operations with a deadline of their own, run on a thread of their own
//! (`docs/design/focus-pipeline.md`, section 6.1).
//!
//! UIA's timeouts bound each call UIA makes into a provider, not an
//! operation: a `FindFirst` over a window's subtree, or a dialog's text
//! gathered from its children, makes many provider calls, and waits a full
//! [`CALL_TIMEOUT`](crate::CALL_TIMEOUT) for each one a held application
//! does not answer (experiment E-TT: 13.2 seconds under a one-second
//! timeout). A UIA call cannot be cancelled, so such an operation is run on
//! a [`BoundedClient`]'s thread, with its own client, while the caller
//! waits for its answer until a deadline. An operation that passes its
//! deadline is abandoned with its thread: the caller goes on without the
//! answer, the thread is left to return from its call and end, and the
//! next operation starts a new thread. This is the outpost worker's own
//! watchdog rule, abandon and replace, for one operation rather than a
//! whole entry.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use verbatim_model::CallCounts;

use crate::client::Uia;

/// An operation queued for the thread, with its answer's way back.
type Job = Box<dyn FnOnce(&Uia) + Send>;

/// Why a bounded operation has no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unanswered {
    /// It passed its deadline, and its thread was abandoned.
    DeadlinePassed,
    /// The thread could not create its UIA client, or the operation
    /// panicked.
    NoClient,
}

/// A UIA client on a thread of its own, for operations with a deadline of
/// their own ([module docs](self)). The thread and its client are created
/// on the first [`run`](Self::run), and again after one is abandoned.
#[derive(Default)]
pub struct BoundedClient {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The way to the thread in charge, if one is running.
    jobs: Option<mpsc::Sender<Job>>,
    /// The number of the thread in charge, counted from 1 as each starts.
    generation: u64,
    /// Every thread started, the one in charge and the abandoned ones still
    /// in their calls, for [`BoundedClient::close`] to wait for.
    threads: Vec<JoinHandle<()>>,
    /// How many threads have been abandoned.
    abandoned: usize,
    /// Whether [`BoundedClient::close`] has run: nothing more is started.
    closed: bool,
}

impl BoundedClient {
    /// Runs `operation` on this client's thread, with its client, and
    /// returns its answer, or [`Unanswered::DeadlinePassed`] once
    /// `deadline` has passed without one: the thread is then abandoned, and
    /// `operation` finishes on it unheard. The calls `operation` makes are
    /// added to the calling thread's count ([`crate::calls`]) when it
    /// answers in time.
    ///
    /// What `operation` captures and answers crosses threads, so a UIA
    /// object travels as an agile reference. An abandoned operation goes on
    /// running once its call returns, so it must not change anything the
    /// caller relies on.
    ///
    /// # Errors
    ///
    /// [`Unanswered::DeadlinePassed`] as above; [`Unanswered::NoClient`]
    /// when the thread could not create its client, or `operation`
    /// panicked, or this client is closed.
    pub fn run<T: Send + 'static>(
        &self,
        deadline: Duration,
        operation: impl FnOnce(&Uia) -> T + Send + 'static,
    ) -> Result<T, Unanswered> {
        let (answer_tx, answer_rx) = mpsc::sync_channel::<(T, CallCounts)>(1);
        let job: Job = Box::new(move |uia| {
            // A panic drops the way back, which the caller hears as no
            // client, and leaves the thread serving.
            let answer = catch_unwind(AssertUnwindSafe(|| operation(uia)));
            let calls = crate::calls::take();
            if let Ok(answer) = answer {
                let _ = answer_tx.send((answer, calls));
            }
        });
        let Some((jobs, generation)) = self.jobs() else {
            return Err(Unanswered::NoClient);
        };
        if jobs.send(job).is_err() {
            return Err(Unanswered::NoClient);
        }
        match answer_rx.recv_timeout(deadline) {
            Ok((answer, calls)) => {
                crate::calls::add(calls);
                Ok(answer)
            }
            Err(RecvTimeoutError::Timeout) => {
                self.abandon(generation);
                Err(Unanswered::DeadlinePassed)
            }
            Err(RecvTimeoutError::Disconnected) => Err(Unanswered::NoClient),
        }
    }

    /// How many threads have been abandoned so far.
    #[must_use]
    pub fn abandoned(&self) -> usize {
        self.lock().abandoned
    }

    /// Ends this client: no operation runs from now on, and every thread
    /// started is waited for, the one in charge finishing what is queued and
    /// each abandoned one returning from its call, however long UIA takes to
    /// end a call the application does not answer. Returns how many threads
    /// were waited for.
    pub fn close(&self) -> usize {
        let threads = {
            let mut state = self.lock();
            state.closed = true;
            state.jobs = None;
            std::mem::take(&mut state.threads)
        };
        let count = threads.len();
        for thread in threads {
            let _ = thread.join();
        }
        count
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The way to the thread in charge and its number, starting one if
    /// none runs.
    fn jobs(&self) -> Option<(mpsc::Sender<Job>, u64)> {
        let mut state = self.lock();
        if state.closed {
            return None;
        }
        if let Some(jobs) = &state.jobs {
            return Some((jobs.clone(), state.generation));
        }
        let (jobs, queue) = mpsc::channel::<Job>();
        let thread = thread::Builder::new()
            .name("verbatim-uia-bounded".to_owned())
            .spawn(move || serve(&queue))
            .ok()?;
        // Threads that have ended are forgotten, so the list holds at most
        // the one in charge and the abandoned ones still in their calls.
        state.threads.retain(|thread| !thread.is_finished());
        state.threads.push(thread);
        state.jobs = Some(jobs.clone());
        state.generation += 1;
        Some((jobs, state.generation))
    }

    /// Abandons thread `generation`, if it is still the one in charge: it
    /// ends once its call returns, running nothing more.
    fn abandon(&self, generation: u64) {
        let mut state = self.lock();
        if state.jobs.is_some() && state.generation == generation {
            state.jobs = None;
            state.abandoned += 1;
            tracing::warn!(
                abandoned = state.abandoned,
                "a bounded UIA operation passed its deadline; its thread is abandoned"
            );
        }
    }
}

impl Drop for BoundedClient {
    fn drop(&mut self) {
        self.close();
    }
}

/// The thread's body: its client, then each operation in turn until its
/// queue closes, which abandonment does by dropping the way to it; then
/// the client and this crate's thread state are released and the thread
/// leaves the apartment its client joined.
fn serve(queue: &mpsc::Receiver<Job>) {
    let uia = match Uia::new() {
        Ok(uia) => uia,
        Err(error) => {
            tracing::warn!(%error, "a bounded UIA thread could not create its client");
            return;
        }
    };
    while let Ok(job) = queue.recv() {
        job(&uia);
    }
    drop(uia);
    crate::release_thread_state();
    crate::com::leave_mta();
}
