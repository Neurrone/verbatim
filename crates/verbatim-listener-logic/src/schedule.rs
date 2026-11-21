//! The listener's reading threads' schedule, as pure transitions (design
//! `docs/design/readers-tell-core-asks-build1.md`, "The listener in this
//! build" and Table 9): one reading thread per application process
//! generation, which runs that application's jobs, Core's requests ahead
//! of the releases of kept elements.
//!
//! A request is answered once. Core's Cancel drops a queued request,
//! answered cancelled; a running one is abandoned with a fresh thread when
//! the reason is the foreground, within [`ABANDONED_LIMIT`] abandoned
//! threads, and left alone for any other reason, its answer sent when its
//! call returns. At the limit the application has no thread until an
//! abandoned one returns, and its jobs wait; Core's due time and Cancel
//! bound a request's wait.
//!
//! A thread with nothing to run waits for the idle time the check threads
//! use ([`IDLE_US`]), then lets its record go, unless abandoned threads of
//! the record are still out, which keep it until they return. A process's
//! exit moves its record out of the running applications, so no later job
//! reaches it.
//!
//! The listener is idle when no job queued before the idle barrier's
//! waiter is waiting or running ([`Schedule::when_idle`]); abandoned
//! threads do not count. The schedule makes no call, reads no clock and
//! holds no lock: the shell (`verbatim_listener::reading`) keeps the lock
//! and the threads, and applies each [`Effects`] under the lock.

use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroUsize;

use verbatim_model::Pid;
use verbatim_model::request::{CancelReason, RequestNumber};

use crate::check_threads::IDLE_US;

/// How many of an application's reading threads may be abandoned to
/// requests and still in their calls before it gets no new thread: its
/// jobs then wait for the first of those threads whose call returns, Core's
/// due time and Cancel bounding a request's wait.
pub const ABANDONED_LIMIT: usize = 2;

/// One run of one process: a process id reused by a new process is a new
/// generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppKey {
    /// The process id.
    pub pid: Pid,
    /// Which run of that id.
    pub generation: u64,
}

/// What a job is, for the schedule's rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// Core's request `number`.
    Request(RequestNumber),
    /// A release of kept elements.
    Release,
}

impl JobKind {
    /// The request it is, if it is one.
    fn number(self) -> Option<RequestNumber> {
        match self {
            Self::Request(number) => Some(number),
            Self::Release => None,
        }
    }
}

/// Why a job ended without being run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// Cancelled by Core: a request is answered cancelled.
    Cancelled,
    /// Its application's process ended with no thread to run it: a request
    /// is answered "not there".
    Exited,
    /// No thread could be started for it.
    NoThread,
    /// The listener is stopping: a request is dropped unanswered.
    Stopped,
}

/// A job ended without being run.
#[derive(Debug, PartialEq, Eq)]
pub struct Ended<J> {
    /// The request it is, if it is one.
    pub number: Option<RequestNumber>,
    /// The job.
    pub job: J,
    /// Why.
    pub ending: Ending,
}

/// What a transition asks the shell to do, in this order: start the
/// threads, wake the waiting ones, forget the records let go, end the jobs
/// not run, answer the requests abandoned, and run the idle waiters.
#[derive(Debug)]
pub struct Effects<J, W> {
    /// Threads to start, each in charge of its application's jobs as its
    /// thread number.
    pub start: Vec<(AppKey, u64)>,
    /// Applications whose thread in charge is to be woken.
    pub wake: Vec<AppKey>,
    /// Records let go: no thread is in charge of them or abandoned to one
    /// of their jobs.
    pub forget: Vec<AppKey>,
    /// Jobs ended without being run.
    pub ended: Vec<Ended<J>>,
    /// Running requests whose thread was abandoned for the foreground:
    /// each answered abandoned.
    pub abandoned: Vec<RequestNumber>,
    /// Idle waiters to run, in order.
    pub waiters: Vec<W>,
}

impl<J, W> Default for Effects<J, W> {
    fn default() -> Self {
        Self {
            start: Vec::new(),
            wake: Vec::new(),
            forget: Vec::new(),
            ended: Vec::new(),
            abandoned: Vec::new(),
            waiters: Vec::new(),
        }
    }
}

/// What a thread does next.
#[derive(Debug, PartialEq, Eq)]
pub enum Next<J> {
    /// Run this job, the request `number` when it is one.
    Run {
        /// The job.
        job: J,
        /// The request it is, if it is one.
        number: Option<RequestNumber>,
    },
    /// Nothing to run: wait until this time, or until woken.
    Wait {
        /// When the idle time ends.
        until_us: u64,
    },
    /// The thread exits.
    Exit,
}

/// What a thread's return from its job decides.
#[derive(Debug, PartialEq, Eq)]
pub struct Returned {
    /// Whether the job's outcome is sent: its job was still its own and
    /// the listener is not stopping. An abandoned job's result is
    /// discarded.
    pub send: bool,
    /// The thread number it goes on as, asking for its next job; `None`
    /// when it exits.
    pub reader: Option<u64>,
}

/// One job waiting.
#[derive(Debug)]
struct Queued<J> {
    /// Its place in the order every job was queued in.
    place: u64,
    kind: JobKind,
    job: J,
}

/// The jobs waiting for an application's thread: Core's requests ahead of
/// everything else.
#[derive(Debug)]
struct Waiting<J> {
    requests: VecDeque<Queued<J>>,
    rest: VecDeque<Queued<J>>,
}

impl<J> Default for Waiting<J> {
    fn default() -> Self {
        Self {
            requests: VecDeque::new(),
            rest: VecDeque::new(),
        }
    }
}

impl<J> Waiting<J> {
    fn is_empty(&self) -> bool {
        self.requests.is_empty() && self.rest.is_empty()
    }

    fn pop(&mut self) -> Option<Queued<J>> {
        self.requests.pop_front().or_else(|| self.rest.pop_front())
    }

    fn oldest(&self) -> Option<u64> {
        let request = self.requests.front().map(|queued| queued.place);
        let rest = self.rest.front().map(|queued| queued.place);
        request.into_iter().chain(rest).min()
    }

    fn push(&mut self, queued: Queued<J>) {
        if queued.kind.number().is_some() {
            self.requests.push_back(queued);
        } else {
            self.rest.push_back(queued);
        }
    }

    /// Every job, ended for `ending`.
    fn end_all(self, ending: Ending) -> impl Iterator<Item = Ended<J>> {
        self.requests
            .into_iter()
            .chain(self.rest)
            .map(move |queued| Ended {
                number: queued.kind.number(),
                job: queued.job,
                ending,
            })
    }

    /// Takes out request `number`, if it waits.
    fn take_request(&mut self, number: RequestNumber) -> Option<Queued<J>> {
        let at = self
            .requests
            .iter()
            .position(|queued| queued.kind.number() == Some(number))?;
        self.requests.remove(at)
    }
}

/// The job a thread in charge is running.
#[derive(Clone, Copy, Debug)]
struct Running {
    place: u64,
    number: Option<RequestNumber>,
}

/// A thread in charge, fewer than [`ABANDONED_LIMIT`] abandoned.
#[derive(Debug)]
struct Charge<J> {
    reader: u64,
    /// The record's threads abandoned to requests, still in their calls.
    abandoned: usize,
    running: Option<Running>,
    /// When it last had nothing to run.
    idle_since: u64,
    waiting: Waiting<J>,
}

/// An application's reader.
#[derive(Debug)]
enum Reader<J> {
    /// No thread in charge, nothing waits, `abandoned` threads still out.
    Idle { abandoned: NonZeroUsize },
    /// A thread in charge.
    InCharge(Charge<J>),
    /// [`ABANDONED_LIMIT`] threads abandoned, none in charge: jobs wait
    /// for the first to return.
    Starved(Waiting<J>),
}

/// One application generation's record.
#[derive(Debug)]
struct Record<J> {
    generation: u64,
    reader: Reader<J>,
}

impl<J> Record<J> {
    fn charge(&self) -> Option<&Charge<J>> {
        match &self.reader {
            Reader::InCharge(charge) => Some(charge),
            Reader::Idle { .. } | Reader::Starved(_) => None,
        }
    }

    fn charge_mut(&mut self) -> Option<&mut Charge<J>> {
        match &mut self.reader {
            Reader::InCharge(charge) => Some(charge),
            Reader::Idle { .. } | Reader::Starved(_) => None,
        }
    }

    fn waiting_mut(&mut self) -> Option<&mut Waiting<J>> {
        match &mut self.reader {
            Reader::InCharge(charge) => Some(&mut charge.waiting),
            Reader::Starved(waiting) => Some(waiting),
            Reader::Idle { .. } => None,
        }
    }

    /// The place of the oldest job it holds, waiting or running.
    fn oldest(&self) -> Option<u64> {
        match &self.reader {
            Reader::InCharge(charge) => charge
                .running
                .map(|running| running.place)
                .into_iter()
                .chain(charge.waiting.oldest())
                .min(),
            Reader::Starved(waiting) => waiting.oldest(),
            Reader::Idle { .. } => None,
        }
    }
}

/// `ABANDONED_LIMIT`, as the count of a record at the limit.
const AT_LIMIT: NonZeroUsize = match NonZeroUsize::new(ABANDONED_LIMIT) {
    Some(count) => count,
    None => panic!("the abandoned limit is above 0"),
};

/// One less than `ABANDONED_LIMIT`: a return from the limit leaves threads
/// out, as the limit is at least 2.
const BELOW_LIMIT: NonZeroUsize = match NonZeroUsize::new(ABANDONED_LIMIT - 1) {
    Some(count) => count,
    None => panic!("the abandoned limit is above 1"),
};

/// The reading threads' schedule ([module docs](self)): `J` is a job, `W`
/// an idle waiter.
#[derive(Debug)]
pub struct Schedule<J, W> {
    /// The running applications, one per process id: a job looks here
    /// alone, so it cannot reach an exited record.
    running: BTreeMap<Pid, Record<J>>,
    /// The processes that have exited with a thread still in charge or
    /// abandoned.
    exited: BTreeMap<AppKey, Record<J>>,
    /// The last application generation given.
    generations: u64,
    /// The last thread number given.
    readers: u64,
    /// How many jobs have been queued: a job's place.
    queued: u64,
    /// The idle waiters, each with the count of jobs queued when it came.
    waiters: VecDeque<(u64, W)>,
    stopping: bool,
}

impl<J, W> Default for Schedule<J, W> {
    fn default() -> Self {
        Self {
            running: BTreeMap::new(),
            exited: BTreeMap::new(),
            generations: 0,
            readers: 0,
            queued: 0,
            waiters: VecDeque::new(),
            stopping: false,
        }
    }
}

impl<J, W> Schedule<J, W> {
    /// Whether the listener is stopping.
    #[must_use]
    pub fn stopping(&self) -> bool {
        self.stopping
    }

    /// Whether thread `thread` is in charge of `app`'s jobs.
    #[must_use]
    pub fn in_charge(&self, app: AppKey, reader: u64) -> bool {
        !self.stopping
            && self
                .record(app)
                .and_then(Record::charge)
                .is_some_and(|charge| charge.reader == reader)
    }

    fn record(&self, app: AppKey) -> Option<&Record<J>> {
        self.running
            .get(&app.pid)
            .filter(|record| record.generation == app.generation)
            .or_else(|| self.exited.get(&app))
    }

    fn record_mut(&mut self, app: AppKey) -> Option<(&mut Record<J>, bool)> {
        if let Some(record) = self
            .running
            .get_mut(&app.pid)
            .filter(|record| record.generation == app.generation)
        {
            return Some((record, false));
        }
        self.exited.get_mut(&app).map(|record| (record, true))
    }

    /// The number the next thread started takes, given once
    /// [`Self::took_thread`] records it.
    fn next_reader(&self) -> u64 {
        self.readers.saturating_add(1)
    }

    /// Records that thread number `thread` was given.
    fn took_reader(&mut self, reader: u64) {
        self.readers = self.readers.max(reader);
    }

    /// A job of `pid`'s arrived at `now_us`: queued for its application's
    /// thread, starting one when none is in charge and fewer than
    /// [`ABANDONED_LIMIT`] are abandoned.
    pub fn push(&mut self, pid: Pid, kind: JobKind, job: J, now_us: u64) -> Effects<J, W> {
        let mut effects = Effects::default();
        if self.stopping {
            effects.ended.push(Ended {
                number: kind.number(),
                job,
                ending: Ending::Stopped,
            });
            return effects;
        }
        self.queued = self.queued.saturating_add(1);
        let queued = Queued {
            place: self.queued,
            kind,
            job,
        };
        let reader = self.next_reader();
        let starts = self
            .running
            .get(&pid)
            .is_none_or(|record| matches!(record.reader, Reader::Idle { .. }));
        if starts {
            self.took_reader(reader);
        }
        match self.running.get_mut(&pid) {
            None => {
                self.generations = self.generations.saturating_add(1);
                let generation = self.generations;
                let mut waiting = Waiting::default();
                waiting.push(queued);
                self.running.insert(
                    pid,
                    Record {
                        generation,
                        reader: Reader::InCharge(Charge {
                            reader,
                            abandoned: 0,
                            running: None,
                            idle_since: now_us,
                            waiting,
                        }),
                    },
                );
                effects.start.push((AppKey { pid, generation }, reader));
            }
            Some(record) => {
                let app = AppKey {
                    pid,
                    generation: record.generation,
                };
                match &mut record.reader {
                    Reader::Idle { abandoned } => {
                        let mut waiting = Waiting::default();
                        waiting.push(queued);
                        record.reader = Reader::InCharge(Charge {
                            reader,
                            abandoned: abandoned.get(),
                            running: None,
                            idle_since: now_us,
                            waiting,
                        });
                        effects.start.push((app, reader));
                    }
                    Reader::InCharge(charge) => {
                        charge.waiting.push(queued);
                        if charge.running.is_none() {
                            effects.wake.push(app);
                        }
                    }
                    Reader::Starved(waiting) => {
                        waiting.push(queued);
                        tracing::warn!(%pid, "a job waits: its application's abandoned reads have not returned");
                    }
                }
            }
        }
        self.settle(&mut effects);
        effects
    }

    /// Thread `thread` of `app` asks at `now_us` for its next job.
    pub fn next(&mut self, app: AppKey, reader: u64, now_us: u64) -> (Next<J>, Effects<J, W>) {
        let mut effects = Effects::default();
        if self.stopping {
            return (Next::Exit, effects);
        }
        let Some((record, exited)) = self.record_mut(app) else {
            return (Next::Exit, effects);
        };
        let Some(charge) = record
            .charge_mut()
            .filter(|charge| charge.reader == reader && charge.running.is_none())
        else {
            return (Next::Exit, effects);
        };
        if let Some(queued) = charge.waiting.pop() {
            let number = queued.kind.number();
            charge.running = Some(Running {
                place: queued.place,
                number,
            });
            self.settle(&mut effects);
            return (
                Next::Run {
                    job: queued.job,
                    number,
                },
                effects,
            );
        }
        let until_us = charge.idle_since.saturating_add(IDLE_US);
        if !exited && now_us < until_us {
            return (Next::Wait { until_us }, effects);
        }
        self.charge_over(app, &mut effects);
        (Next::Exit, effects)
    }

    /// The thread in charge of `app` has nothing more and ends: the record
    /// goes when no thread of it is abandoned, else it waits for them.
    fn charge_over(&mut self, app: AppKey, effects: &mut Effects<J, W>) {
        let Some((record, _)) = self.record_mut(app) else {
            return;
        };
        let abandoned = record.charge().map_or(0, |charge| charge.abandoned);
        match NonZeroUsize::new(abandoned) {
            Some(abandoned) => record.reader = Reader::Idle { abandoned },
            None => self.let_go(app, effects),
        }
    }

    /// Removes `app`'s record: the one place a record goes.
    fn let_go(&mut self, app: AppKey, effects: &mut Effects<J, W>) {
        if self
            .running
            .get(&app.pid)
            .is_some_and(|record| record.generation == app.generation)
        {
            self.running.remove(&app.pid);
        } else {
            self.exited.remove(&app);
        }
        effects.forget.push(app);
    }

    /// Thread `thread` of `app` returned at `now_us` from its job.
    pub fn returned(&mut self, app: AppKey, reader: u64, now_us: u64) -> (Returned, Effects<J, W>) {
        let mut effects = Effects::default();
        let stopping = self.stopping;
        let in_charge = self
            .record_mut(app)
            .and_then(|(record, _)| record.charge_mut())
            .filter(|charge| charge.reader == reader);
        if let Some(charge) = in_charge {
            charge.running = None;
            charge.idle_since = now_us;
            self.settle(&mut effects);
            return (
                Returned {
                    send: !stopping,
                    reader: Some(reader),
                },
                effects,
            );
        }
        // Abandoned while it ran: its result is discarded.
        tracing::info!(pid = %app.pid, "an abandoned read returned; its result is discarded");
        let reused = self.next_reader();
        let Some((record, _)) = self.record_mut(app) else {
            tracing::error!(target: verbatim_model::DEFECT_TARGET, pid = %app.pid, "an abandoned read returned of a record no longer held");
            return (
                Returned {
                    send: false,
                    reader: None,
                },
                effects,
            );
        };
        let mut gone = false;
        let mut reader = None;
        match &mut record.reader {
            Reader::Idle { abandoned } => match NonZeroUsize::new(abandoned.get() - 1) {
                Some(less) => *abandoned = less,
                None => gone = true,
            },
            Reader::InCharge(charge) => charge.abandoned = charge.abandoned.saturating_sub(1),
            Reader::Starved(waiting) if stopping || waiting.is_empty() => {
                let waiting = std::mem::take(waiting);
                effects.ended.extend(waiting.end_all(Ending::Stopped));
                record.reader = Reader::Idle {
                    abandoned: BELOW_LIMIT,
                };
            }
            Reader::Starved(waiting) => {
                // The returning thread takes what waits.
                let waiting = std::mem::take(waiting);
                record.reader = Reader::InCharge(Charge {
                    reader: reused,
                    abandoned: BELOW_LIMIT.get(),
                    running: None,
                    idle_since: now_us,
                    waiting,
                });
                reader = Some(reused);
            }
        }
        if reader.is_some() {
            self.took_reader(reused);
        }
        if gone {
            self.let_go(app, &mut effects);
        }
        self.settle(&mut effects);
        (
            Returned {
                send: false,
                reader,
            },
            effects,
        )
    }

    /// Core cancels request `number` for `reason` at `now_us` (Table 9): a
    /// queued one is ended cancelled; a running one is abandoned with a
    /// fresh thread for the foreground, and left alone for any other
    /// reason; an unknown one, already answered, is left.
    pub fn cancel(
        &mut self,
        number: RequestNumber,
        reason: CancelReason,
        now_us: u64,
    ) -> Effects<J, W> {
        let mut effects = Effects::default();
        let keys: Vec<(AppKey, bool)> = self
            .running
            .iter()
            .map(|(pid, record)| {
                (
                    AppKey {
                        pid: *pid,
                        generation: record.generation,
                    },
                    false,
                )
            })
            .chain(self.exited.keys().map(|app| (*app, true)))
            .collect();
        for (app, _) in &keys {
            let Some((record, _)) = self.record_mut(*app) else {
                continue;
            };
            if let Some(queued) = record
                .waiting_mut()
                .and_then(|waiting| waiting.take_request(number))
            {
                effects.ended.push(Ended {
                    number: Some(number),
                    job: queued.job,
                    ending: Ending::Cancelled,
                });
                self.settle(&mut effects);
                return effects;
            }
        }
        let running = keys.into_iter().find(|(app, _)| {
            self.record(*app)
                .and_then(Record::charge)
                .and_then(|charge| charge.running)
                .is_some_and(|running| running.number == Some(number))
        });
        let Some((app, exited)) = running else {
            return effects;
        };
        if !reason.abandons_call() {
            return effects;
        }
        effects.abandoned.push(number);
        self.abandon(app, exited, now_us, &mut effects);
        self.settle(&mut effects);
        effects
    }

    /// The thread in charge of `app` is abandoned to its running request:
    /// a fresh thread takes what waits, unless the record's abandoned
    /// threads reach the limit.
    fn abandon(&mut self, app: AppKey, exited: bool, now_us: u64, effects: &mut Effects<J, W>) {
        let fresh = self.next_reader();
        let Some((record, _)) = self.record_mut(app) else {
            return;
        };
        let Reader::InCharge(charge) = &mut record.reader else {
            return;
        };
        let count = charge.abandoned.saturating_add(1);
        let waiting = std::mem::take(&mut charge.waiting);
        tracing::warn!(pid = %app.pid, abandoned = count, "a request's read was abandoned for the foreground");
        record.reader = match NonZeroUsize::new(count) {
            Some(count) if count >= AT_LIMIT => {
                if exited {
                    effects.ended.extend(waiting.end_all(Ending::Exited));
                    Reader::Idle {
                        abandoned: AT_LIMIT,
                    }
                } else {
                    Reader::Starved(waiting)
                }
            }
            Some(count) if waiting.is_empty() => Reader::Idle { abandoned: count },
            _ => {
                effects.start.push((app, fresh));
                Reader::InCharge(Charge {
                    reader: fresh,
                    abandoned: count,
                    running: None,
                    idle_since: now_us,
                    waiting,
                })
            }
        };
        if effects.start.last() == Some(&(app, fresh)) {
            self.took_reader(fresh);
        }
    }

    /// The process running under `pid` has exited: its record leaves the
    /// running applications, what waits with no thread ended "not there",
    /// and a thread in charge runs what waits, then ends with no idle time.
    pub fn application_exited(&mut self, pid: Pid) -> Effects<J, W> {
        let mut effects = Effects::default();
        let Some(mut record) = self.running.remove(&pid) else {
            return effects;
        };
        let app = AppKey {
            pid,
            generation: record.generation,
        };
        match &mut record.reader {
            Reader::Starved(waiting) => {
                let waiting = std::mem::take(waiting);
                effects.ended.extend(waiting.end_all(Ending::Exited));
                record.reader = Reader::Idle {
                    abandoned: AT_LIMIT,
                };
            }
            Reader::InCharge(charge) if charge.running.is_none() => effects.wake.push(app),
            Reader::InCharge(_) | Reader::Idle { .. } => {}
        }
        self.exited.insert(app, record);
        self.settle(&mut effects);
        effects
    }

    /// Thread `thread` could not be started for `app`: what waits for it
    /// is ended, and its charge is over.
    pub fn spawn_failed(&mut self, app: AppKey, reader: u64) -> Effects<J, W> {
        let mut effects = Effects::default();
        let Some(charge) = self
            .record_mut(app)
            .and_then(|(record, _)| record.charge_mut())
            .filter(|charge| charge.reader == reader)
        else {
            return effects;
        };
        tracing::error!(pid = %app.pid, "no reading reader could be started; its jobs are ended");
        let waiting = std::mem::take(&mut charge.waiting);
        effects.ended.extend(waiting.end_all(Ending::NoThread));
        self.charge_over(app, &mut effects);
        self.settle(&mut effects);
        effects
    }

    /// The idle barrier's `waiter`: run once every job queued before it is
    /// no longer waiting or running, at once when none is. Dropped once
    /// the listener is stopping.
    pub fn when_idle(&mut self, waiter: W) -> Effects<J, W> {
        let mut effects = Effects::default();
        if !self.stopping {
            self.waiters.push_back((self.queued, waiter));
            self.settle(&mut effects);
        }
        effects
    }

    /// The listener is stopping: every waiting job is ended unrun, every
    /// thread waiting is woken to exit, and the idle waiters dropped.
    pub fn stop(&mut self) -> Effects<J, W> {
        let mut effects = Effects::default();
        self.stopping = true;
        self.waiters.clear();
        let keys: Vec<AppKey> = self
            .running
            .iter()
            .map(|(pid, record)| AppKey {
                pid: *pid,
                generation: record.generation,
            })
            .chain(self.exited.keys().copied())
            .collect();
        for app in keys {
            if let Some((record, _)) = self.record_mut(app)
                && let Some(waiting) = record.waiting_mut()
            {
                let waiting = std::mem::take(waiting);
                effects.ended.extend(waiting.end_all(Ending::Stopped));
            }
            effects.wake.push(app);
        }
        effects
    }

    /// After a transition: runs each idle waiter whose earlier jobs have
    /// all ended.
    fn settle(&mut self, effects: &mut Effects<J, W>) {
        let oldest = self
            .running
            .values()
            .chain(self.exited.values())
            .filter_map(Record::oldest)
            .min();
        while let Some(&(before, _)) = self.waiters.front()
            && oldest.is_none_or(|oldest| before < oldest)
        {
            if let Some((_, waiter)) = self.waiters.pop_front() {
                effects.waiters.push(waiter);
            }
        }
    }
}

#[cfg(test)]
mod tests;
