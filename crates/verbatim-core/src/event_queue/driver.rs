//! The async driver: the front entry's function, paused in its one ask,
//! run on Core's thread with a waker that does nothing and no runtime.
//!
//! The driver starts the front entry's function in place inside the step
//! that takes the entry, and resumes it in place inside the step that ends
//! its request, so whatever the function emits takes that step's place in
//! the effects. The function reaches Core only through its [`Cx`], which
//! lends Core's facts as a borrow and asks. [`Asking`] is the only future
//! the function can await that ever waits: the driver checks, at every
//! pause, that the function paused in an ask with its request live, and at
//! every entry's end that each request it sent had its one outcome.
//!
//! Core's state, [`State`], is plain data that can be cloned, compared and
//! written to a dump: Core's facts and the [`Queue`], which keeps the front
//! entry's checkpoint, the state when it started with each request it
//! asked and each ending it had. The paused function is not state: it sits
//! in a [`Core`] beside the state, which is never cloned or written, and a
//! clone, a restored dump or a replay rebuilds it by running the front
//! entry's function again from its checkpoint with the recorded endings
//! ([`Core::rebuild`]).

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use serde::{Deserialize, Serialize};
use verbatim_model::request::{Outcome as WireOutcome, Reply, Request as Wire, RequestNumber};

use super::inbox::{Effect, Timed};
use super::limiter::{self, Item};
use super::request::{Outcome, Refused, Request};
use super::table::{Ending, Live, Order, Owner, Stage, Table};
use super::{Foreground, Function, Instance, Logic, Reader};
use crate::focus::backend::BackendCache;

/// Core: the front entry's paused function and what it shares with the
/// driver. The state sits in it only while a step runs, or for good in the
/// tests that drive the queue alone.
pub(crate) struct Core<L: Logic> {
    pub(super) shared: Rc<RefCell<Shared<L>>>,
    /// The front entry's function, paused in its one ask; present exactly
    /// when the queue has a front.
    task: Option<Function>,
    /// Every entry that ended, with the outcome of each request it had: the
    /// tests' audit of the one-outcome rule, not Core's state.
    #[cfg(test)]
    pub(super) ended: Vec<String>,
}

/// What the driver and the front's function share.
pub(crate) struct Shared<L: Logic> {
    pub(super) state: State<L::Entry, L::Facts>,
    /// What was done while a step ran, taken by the step at its end.
    pub(super) effects: Vec<Effect<L::Said>>,
    /// The slot of the front's request, where its ending goes.
    slot: Option<Rc<dyn Slot>>,
    /// Set by an [`Asking`] when it returns pending, cleared before each
    /// poll: a function that returns pending with this unset waited on
    /// something other than an ask.
    parked: bool,
    /// Asks dropped after their request was made and before their outcome
    /// was taken; zero whenever an entry ends.
    unanswered: u32,
}

/// Core's state: its facts and its queue. Plain data, which a dump shows
/// and a clone copies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State<E, F> {
    /// Core's facts, the [`Logic`]'s.
    pub(crate) facts: F,
    /// The queue.
    pub(crate) queue: Queue<E, F>,
}

impl<E, F: Default> Default for State<E, F> {
    fn default() -> Self {
        Self {
            facts: F::default(),
            queue: Queue::default(),
        }
    }
}

/// The event queue's state: the front, the waiting entries, the requests,
/// the readers, the foreground record, the backend cache and the front's
/// checkpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Queue<E, F> {
    /// The time of the input being handled, in microseconds on the run's
    /// shared clock.
    pub(super) now_us: u64,
    /// The front entry.
    pub(super) front: Option<Front<E>>,
    /// The entries waiting behind it, in arrival order.
    pub(super) waiting: VecDeque<Item<E>>,
    /// The requests.
    pub(super) table: Table,
    /// The running instance of each reader that can take requests.
    #[serde(with = "super::pairs")]
    pub(super) readers: BTreeMap<Reader, Instance>,
    /// The foreground record, `None` before the listener's first report.
    pub(super) foreground: Option<Foreground>,
    /// The backend verdicts of windows (Table 4).
    pub(super) backend: BackendCache,
    /// The front entry's checkpoint, present exactly when there is a front.
    pub(super) checkpoint: Option<Box<Checkpoint<E, F>>>,
    /// Entries Core appends later, each with the time it joins the back,
    /// in the order they were made: the check after a menu or switcher
    /// ends.
    pub(super) timers: Vec<(u64, E)>,
    /// The outpost generation record (Table 5).
    #[serde(default)]
    pub(crate) record: crate::supervisor::record::Record,
}

impl<E, F> Default for Queue<E, F> {
    fn default() -> Self {
        Self {
            now_us: 0,
            front: None,
            waiting: VecDeque::new(),
            table: Table::default(),
            readers: BTreeMap::new(),
            foreground: None,
            backend: BackendCache::default(),
            checkpoint: None,
            timers: Vec::new(),
            record: crate::supervisor::record::Record::default(),
        }
    }
}

impl<E, F> Queue<E, F> {
    /// The application of the window the listener last reported in front,
    /// `None` before its first report.
    pub(crate) fn foreground_process(&self) -> Option<verbatim_model::Pid> {
        self.foreground.map(|foreground| foreground.process)
    }
}

impl<E, F> Queue<E, F> {
    /// Whether the queue is idle for the test harness
    /// (`docs/design/readers-tell-core-asks-build1.md`, "The test
    /// harness's idle"): no entry at the front or waiting, none to be
    /// appended later, no request waiting for its outpost or out, and no
    /// window in the recovery set, so no hang round is due either.
    pub(crate) fn idle(&self) -> bool {
        self.front.is_none()
            && self.waiting.is_empty()
            && self.timers.is_empty()
            && self.table.idle()
    }

    /// The one time Core waits for: the nearest due time, hang round or
    /// entry Core appends later.
    pub(crate) fn next_due(&self) -> Option<u64> {
        self.table
            .next_due()
            .into_iter()
            .chain(self.timers.iter().map(|(at_us, _)| *at_us))
            .chain(self.record.next_due())
            .min()
    }

    /// Sets up `instance` as its reader's current one, starting, so that
    /// its `Ready` is taken: a test's setup, standing for the owner
    /// thread's report (`ListenerStarted`, or an outpost launched and
    /// started) and for a generation that answers every ping, its
    /// liveness due at the clock's end.
    #[cfg(test)]
    pub(crate) fn start_for_tests(&mut self, instance: Instance) {
        use crate::supervisor::record::{Running, Stage};
        match instance.reader {
            Reader::Listener => self.record.listener = Some(instance.generation),
            Reader::Outpost(process) => {
                if !self.record.applications.contains_key(&process) {
                    self.record.launch(process);
                }
                if let Some(application) = self.record.applications.get_mut(&process) {
                    application.stage = Stage::Starting(Running {
                        generation: instance.generation,
                        started_us: self.now_us,
                        pong_us: u64::MAX - crate::supervisor::record::LIVENESS_US,
                    });
                }
            }
        }
    }

    /// Whether `reader` has an instance that takes requests.
    pub(crate) fn ready(&self, reader: Reader) -> bool {
        self.readers.contains_key(&reader)
    }

    /// Whether a frame of `instance` is its reader's current one's: the
    /// newest listener the owner reported started, or an application's
    /// starting or ready generation (design, "The outpost generation
    /// record").
    pub(crate) fn admits(&self, instance: Instance) -> bool {
        match instance.reader {
            Reader::Listener => self.record.listener == Some(instance.generation),
            Reader::Outpost(process) => self.record.current(process, instance.generation),
        }
    }

    /// The front entry and its request, while it has one out.
    pub(crate) fn asking(&self) -> Option<(&E, &verbatim_model::request::Request)> {
        let front = self.front.as_ref()?;
        let live = self.table.live(front.asked?)?;
        Some((&front.entry, &live.request))
    }

    /// The time of the last input handled.
    pub(crate) fn now_us(&self) -> u64 {
        self.now_us
    }

    /// Every request waiting or out, as sent: the ids each names stay held
    /// (design, "Held anchors").
    pub(crate) fn requests(&self) -> impl Iterator<Item = &verbatim_model::request::Request> {
        self.table.requests()
    }

    /// Every entry Core appends later ([`Cx::append_after`]).
    pub(crate) fn later(&self) -> impl Iterator<Item = &E> {
        self.timers.iter().map(|(_, entry)| entry)
    }

    /// The listener generation the owner thread last reported started.
    pub(crate) fn listener(&self) -> Option<u64> {
        self.record.listener
    }

    /// The front entry and every entry waiting behind it, a run's
    /// candidates and the menu slot's included: the ids each names stay
    /// held.
    pub(crate) fn entries(&self) -> impl Iterator<Item = &E> {
        self.front
            .iter()
            .map(|front| &front.entry)
            .chain(self.waiting.iter().flat_map(|item| match item {
                Item::One(entry) | Item::Menu { entry, .. } => std::slice::from_ref(entry),
                Item::Run(candidates) => candidates.as_slice(),
            }))
    }
}

#[cfg(test)]
impl<E, F> Queue<E, F> {
    /// How many items wait behind the front.
    pub(crate) fn waiting_for_tests(&self) -> usize {
        self.waiting.len()
    }

    /// The requests waiting or out, as sent.
    pub(crate) fn table_for_tests(&self) -> Vec<Wire> {
        self.table.requests_for_tests()
    }

    /// The front's checkpoint, to change.
    pub(crate) fn checkpoint_for_tests(&mut self) -> Option<&mut Checkpoint<E, F>> {
        self.checkpoint.as_deref_mut()
    }

    /// The front entry, if any.
    pub(crate) fn front_for_tests(&self) -> Option<&E> {
        self.front.as_ref().map(|front| &front.entry)
    }
}

#[cfg(test)]
impl<E, F> Checkpoint<E, F> {
    /// The requests recorded as asked, to change.
    pub(crate) fn asked_for_tests(&mut self) -> &mut Vec<Wire> {
        &mut self.asked
    }
}

/// The front entry's checkpoint: the state when it started, each request
/// its function asked and each ending it had, which rebuild its paused
/// function.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Checkpoint<E, F> {
    /// The state when the entry started, with no checkpoint of its own.
    base: State<E, F>,
    /// Each request the function asked, in order.
    asked: Vec<Wire>,
    /// Each ending it had, in order, with the time of the input that
    /// brought it.
    answers: Vec<(u64, Ending)>,
}

/// The front entry, the outcome of each request it has had, and its
/// request, absent only while its function runs between two requests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Front<E> {
    /// The entry.
    pub(super) entry: E,
    /// The outcome of each request it had, as a dump writes it.
    pub(super) answers: Vec<Shown>,
    /// Its request, waiting or out.
    pub(super) asked: Option<RequestNumber>,
}

/// An outcome as a dump writes it, decoded into its request's own type.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Shown(String);

impl fmt::Debug for Shown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A checkpoint whose replay did not reach the state it was taken from:
/// replay is not exact, a fault of Core's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReplayMismatch {
    /// What the replay asked and had, as a dump writes it.
    pub(crate) replayed: String,
    /// What the state records.
    pub(crate) original: String,
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "a dump shows Core's state; the function and the tests' audit are not part of it"
)]
impl<L: Logic> fmt::Debug for Core<L> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shared = self.shared.borrow();
        let state = &shared.state;
        formatter
            .debug_struct("Core")
            .field("now_us", &state.queue.now_us)
            .field("front", &state.queue.front)
            .field("waiting", &state.queue.waiting)
            .field("timers", &state.queue.timers)
            .field("table", &state.queue.table)
            .field("readers", &state.queue.readers)
            .field("foreground", &state.queue.foreground)
            .field("backend", &state.queue.backend)
            .field("facts", &state.facts)
            .finish()
    }
}

impl<L: Logic> Default for Core<L> {
    fn default() -> Self {
        Self::from_state(State::default())
    }
}

impl<L: Logic> Core<L> {
    pub(super) fn from_state(state: State<L::Entry, L::Facts>) -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared {
                state,
                effects: Vec::new(),
                slot: None,
                parked: false,
                unanswered: 0,
            })),
            task: None,
            #[cfg(test)]
            ended: Vec::new(),
        }
    }

    /// The state as a dump writes it: each entry, the answers the front
    /// has had and its request.
    pub(crate) fn dump(&self) -> String {
        format!("{self:?}")
    }

    /// A context on this core, for the tests' own asks.
    #[cfg(test)]
    pub(super) fn cx_for_tests(&self) -> Cx<L> {
        Cx {
            shared: Rc::clone(&self.shared),
        }
    }

    /// How many asks were let go after they were made.
    #[cfg(test)]
    pub(super) fn unanswered_for_tests(&self) -> u32 {
        self.shared.borrow().unanswered
    }

    /// The one time Core waits for, which the shell's timer is set to.
    pub(crate) fn next_due(&self) -> Option<u64> {
        let shared = self.shared.borrow();
        let state = &shared.state;
        state
            .queue
            .next_due()
            .into_iter()
            .chain(L::due(&state.facts))
            .min()
    }

    /// A fresh instance with a copy of this one's state, its function
    /// rebuilt from the front's checkpoint ([`Core::rebuild`]).
    ///
    /// # Errors
    ///
    /// [`ReplayMismatch`] when the rebuilt function does not ask and have
    /// what the checkpoint records.
    #[cfg(test)]
    pub(crate) fn checkpoint(&self) -> Result<Self, ReplayMismatch> {
        let state = self.shared.borrow().state.clone();
        let fresh = Self::rebuild(&state)?;
        fresh.shared.borrow_mut().state = state;
        Ok(fresh)
    }

    /// The paused function of `state`'s front, rebuilt: the function run
    /// again from the checkpoint's state, given each recorded ending in
    /// turn, its effects dropped because they have already been made. The
    /// instance returned holds the checkpoint's state, worked on by the
    /// rebuilt function, in place of `state`, which the caller lends it
    /// for each step ([`Core::step`]).
    ///
    /// # Errors
    ///
    /// [`ReplayMismatch`] when the rebuilt function does not ask exactly
    /// the requests the checkpoint records and pause in the last, or the
    /// state has a front with no checkpoint.
    pub(crate) fn rebuild(state: &State<L::Entry, L::Facts>) -> Result<Self, ReplayMismatch> {
        let Some(front) = &state.queue.front else {
            return Ok(Self::default());
        };
        let Some(checkpoint) = state.queue.checkpoint.as_deref() else {
            return Err(ReplayMismatch {
                replayed: String::from("no checkpoint"),
                original: format!("{front:?}"),
            });
        };
        let mut fresh = Self::from_state(checkpoint.base.clone());
        fresh.shared.borrow_mut().state.queue.checkpoint = Some(Box::new(Checkpoint {
            base: State::default(),
            asked: Vec::new(),
            answers: Vec::new(),
        }));
        fresh.start(front.entry.clone());
        for (at_us, ending) in &checkpoint.answers {
            if fresh.task.is_none() {
                break;
            }
            fresh.shared.borrow_mut().state.queue.now_us = *at_us;
            fresh.resume(ending.clone());
        }
        let mismatch = {
            let shared = fresh.shared.borrow();
            let replayed = shared
                .state
                .queue
                .checkpoint
                .as_deref()
                .map(|replayed| (&replayed.asked, &replayed.answers));
            let paused = fresh.task.is_some() && shared.slot.is_some();
            (replayed != Some((&checkpoint.asked, &checkpoint.answers)) || !paused).then(|| {
                ReplayMismatch {
                    replayed: format!("{replayed:?}, paused {paused}"),
                    original: format!("{:?}", (&checkpoint.asked, &checkpoint.answers)),
                }
            })
        };
        if let Some(mismatch) = mismatch {
            return Err(mismatch);
        }
        {
            let mut shared = fresh.shared.borrow_mut();
            shared.effects.clear();
            shared.state = State::default();
        }
        Ok(fresh)
    }

    /// Handles one input on `state`, lent to this instance for the step,
    /// and returns what Core did. `paused` is the instance kept beside the
    /// state, rebuilt from the state's checkpoint when there is none, as
    /// after a clone, a restored dump or a replay.
    ///
    /// # Errors
    ///
    /// [`ReplayMismatch`] when the function cannot be rebuilt; the state is
    /// left as it was.
    pub(crate) fn step(
        paused: &mut Option<Self>,
        state: &mut State<L::Entry, L::Facts>,
        input: Timed<L::Entry>,
    ) -> Result<Vec<Effect<L::Said>>, ReplayMismatch> {
        let mut core = match paused.take() {
            Some(core) => core,
            None => Self::rebuild(state)?,
        };
        std::mem::swap(&mut core.shared.borrow_mut().state, state);
        let effects = core.feed(input);
        std::mem::swap(&mut core.shared.borrow_mut().state, state);
        *paused = Some(core);
        Ok(effects)
    }

    /// Ends the step: the round settles and the step's effects are
    /// returned.
    pub(super) fn finish_step(&mut self) -> Vec<Effect<L::Said>> {
        self.check();
        let mut shared = self.shared.borrow_mut();
        shared.state.queue.table.settle_round();
        std::mem::take(&mut shared.effects)
    }

    /// The driver's invariants between steps.
    fn check(&self) {
        let shared = self.shared.borrow();
        let queue = &shared.state.queue;
        assert_eq!(
            self.task.is_some(),
            queue.front.is_some(),
            "a paused function exactly when there is a front entry"
        );
        assert_eq!(
            queue.checkpoint.is_some(),
            queue.front.is_some(),
            "a checkpoint exactly when there is a front entry"
        );
        if let Some(front) = &queue.front {
            assert!(
                front
                    .asked
                    .is_some_and(|number| queue.table.live(number).is_some())
                    && shared.slot.is_some(),
                "a paused function has its one request live, whose ending resumes it"
            );
        }
    }

    /// Takes the next waiting entries while there is no front, starting
    /// each one's function from a checkpoint of the state it starts in.
    pub(super) fn take_next(&mut self) {
        loop {
            self.settle();
            if self.task.is_some() {
                break;
            }
            let entry = {
                let mut shared = self.shared.borrow_mut();
                let queue = &mut shared.state.queue;
                let Some(entry) = limiter::take::<L>(&mut queue.waiting) else {
                    break;
                };
                queue.front = Some(Front {
                    entry: entry.clone(),
                    answers: Vec::new(),
                    asked: None,
                });
                // An entry whose function asks nothing ends in its first
                // poll, so nothing is ever rebuilt from its checkpoint, and
                // the state is not copied for one.
                if !L::asks_nothing(&entry) {
                    let base = shared.state.clone();
                    shared.state.queue.checkpoint = Some(Box::new(Checkpoint {
                        base,
                        asked: Vec::new(),
                        answers: Vec::new(),
                    }));
                }
                entry
            };
            self.start(entry);
        }
    }

    /// Starts the front entry's function and runs it to its first request
    /// or its end.
    fn start(&mut self, entry: L::Entry) {
        let cx = Cx {
            shared: Rc::clone(&self.shared),
        };
        self.task = Some(L::run(cx, entry));
        self.poll();
    }

    /// Delivers the front's request's ending to its function, recorded in
    /// the checkpoint, and resumes it.
    pub(super) fn resume(&mut self, ending: Ending) {
        let slot = self.shared.borrow_mut().slot.take();
        let Some(slot) = slot else {
            return;
        };
        let (shown, undecodable) = slot.deliver(ending.clone());
        {
            let mut shared = self.shared.borrow_mut();
            let now_us = shared.state.queue.now_us;
            let queue = &mut shared.state.queue;
            let number = queue.front.as_mut().and_then(|front| {
                front.answers.push(shown);
                front.asked.take()
            });
            if let Some(checkpoint) = queue.checkpoint.as_mut() {
                checkpoint.answers.push((now_us, ending));
            }
            if let (true, Some(number)) = (undecodable, number) {
                shared.effects.push(Effect::Undecodable(number));
            }
        }
        self.poll();
    }

    /// Polls the front's function once.
    fn poll(&mut self) {
        let Some(task) = self.task.as_mut() else {
            return;
        };
        self.shared.borrow_mut().parked = false;
        let polled = task.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        let mut shared = self.shared.borrow_mut();
        match polled {
            Poll::Pending => {
                assert!(
                    shared.parked,
                    "an entry's function waited on something other than an ask"
                );
            }
            Poll::Ready(()) => {
                let front = shared.state.queue.front.take();
                shared.state.queue.checkpoint = None;
                assert!(
                    front.as_ref().is_some_and(|front| front.asked.is_none()),
                    "an entry's function ended with its request live"
                );
                assert_eq!(
                    shared.unanswered, 0,
                    "every request an entry made had its one outcome"
                );
                #[cfg(test)]
                if let Some(front) = front {
                    self.ended
                        .push(format!("{:?} {:?}", front.entry, front.answers));
                }
                drop(shared);
                self.task = None;
            }
        }
    }

    /// Hands `orders` to the shell as effects.
    pub(super) fn order(shared: &mut Shared<L>, orders: Vec<Order>) {
        for order in orders {
            shared.effects.push(match order {
                Order::Send(to, ask) => Effect::Send { to, ask },
                Order::Cancel(to, cancel) => Effect::Cancel { to, cancel },
            });
        }
    }
}

/// A result that does not decode as its request's reply.
struct Undecodable;

/// Where one request's ending goes: the [`Asking`] that made it.
pub(crate) trait Slot {
    /// Decodes `ending` into the request's own outcome and puts it in the
    /// slot; returns it as a dump writes it, and whether a result did not
    /// decode.
    fn deliver(&self, ending: Ending) -> (Shown, bool);
}

/// The slot of a request of type `R`, holding its outcome from the moment
/// the queue decodes it until the function's next poll takes it.
struct Typed<R: Request> {
    outcome: Cell<Option<Outcome<R::Reply>>>,
}

impl<R: Request> Typed<R> {
    fn decode(reply: Reply) -> Result<R::Reply, Undecodable> {
        R::decode(reply).ok_or(Undecodable)
    }
}

/// An outcome without a result, as an outcome of any request.
pub(super) fn widen<T>(outcome: Outcome<Infallible>) -> Outcome<T> {
    match outcome {
        Outcome::Result(never) => match never {},
        Outcome::Refused(refused) => Outcome::Refused(refused),
        Outcome::NotThere => Outcome::NotThere,
        Outcome::NotResponding => Outcome::NotResponding,
        Outcome::ReaderEnded => Outcome::ReaderEnded,
        Outcome::NoLongerAwaited(reason) => Outcome::NoLongerAwaited(reason),
    }
}

/// `ending` as Core's outcome, a result left as it arrived.
pub(super) fn outcome_of(ending: Ending) -> Result<Reply, Outcome<Infallible>> {
    match ending {
        Ending::Answered(WireOutcome::Result(reply)) => Ok(reply),
        Ending::Answered(WireOutcome::Refused(refusal)) => {
            Err(Outcome::Refused(Refused::Reader(refusal)))
        }
        Ending::Answered(WireOutcome::NotThere) => Err(Outcome::NotThere),
        Ending::Answered(WireOutcome::NotResponding) => Err(Outcome::NotResponding),
        Ending::Answered(WireOutcome::Ended) => Err(Outcome::ReaderEnded),
        Ending::NoLongerAwaited(reason) => Err(Outcome::NoLongerAwaited(reason)),
    }
}

impl<R: Request> Slot for Typed<R> {
    fn deliver(&self, ending: Ending) -> (Shown, bool) {
        let (outcome, undecodable) = match outcome_of(ending) {
            Ok(reply) => match Self::decode(reply) {
                Ok(reply) => (Outcome::Result(reply), false),
                Err(Undecodable) => (Outcome::Refused(Refused::Undecodable), true),
            },
            Err(outcome) => (widen(outcome), false),
        };
        let shown = Shown(format!("{outcome:?}"));
        let earlier = self.outcome.replace(Some(outcome));
        assert!(earlier.is_none(), "a request has one outcome");
        (shown, undecodable)
    }
}

/// The front function's context: it lends Core's facts and asks. Nothing
/// else it offers can be awaited.
pub(crate) struct Cx<L: Logic> {
    shared: Rc<RefCell<Shared<L>>>,
}

impl<L: Logic> Cx<L> {
    /// Core's facts as they are now, lent until the borrow ends, which is
    /// before the function's next ask.
    pub(crate) fn facts(&self) -> Ref<'_, L::Facts> {
        Ref::map(self.shared.borrow(), |shared| &shared.state.facts)
    }

    /// Core's facts, to change.
    pub(crate) fn facts_mut(&mut self) -> RefMut<'_, L::Facts> {
        RefMut::map(self.shared.borrow_mut(), |shared| &mut shared.state.facts)
    }

    /// The entries waiting behind this one, in arrival order.
    pub(crate) fn waiting(&self) -> Ref<'_, VecDeque<Item<L::Entry>>> {
        Ref::map(self.shared.borrow(), |shared| &shared.state.queue.waiting)
    }

    /// The entries waiting behind this one, to change in place: what a
    /// confirmation read is written onto its waiting candidate.
    pub(crate) fn waiting_mut(&mut self) -> RefMut<'_, VecDeque<Item<L::Entry>>> {
        RefMut::map(self.shared.borrow_mut(), |shared| {
            &mut shared.state.queue.waiting
        })
    }

    /// Takes out of the queue the waiting single entries `pick` chooses, in
    /// their order, for the front's function to judge before its own.
    pub(crate) fn take_waiting(&mut self, pick: impl Fn(&L::Entry) -> bool) -> Vec<L::Entry> {
        let mut shared = self.shared.borrow_mut();
        let waiting = &mut shared.state.queue.waiting;
        let mut taken = Vec::new();
        let mut kept = VecDeque::with_capacity(waiting.len());
        for item in waiting.drain(..) {
            match item {
                Item::One(entry) if pick(&entry) => taken.push(entry),
                other => kept.push_back(other),
            }
        }
        *waiting = kept;
        taken
    }

    /// Takes out of the queue the waiting single entries `pick` chooses
    /// ahead of the first focus-kind entry, in their order: a focus, a run
    /// or the menu slot's entry ends the search.
    pub(crate) fn take_waiting_before(
        &mut self,
        pick: impl Fn(&L::Entry) -> bool,
    ) -> Vec<L::Entry> {
        let mut shared = self.shared.borrow_mut();
        let state = &mut shared.state;
        let facts = &state.facts;
        let waiting = &mut state.queue.waiting;
        let mut taken = Vec::new();
        let mut kept = VecDeque::with_capacity(waiting.len());
        let mut searching = true;
        for item in waiting.drain(..) {
            if searching {
                let focus_kind = match &item {
                    Item::One(entry) => match L::place(facts, entry) {
                        super::Place::Other { focus_kind, .. } => focus_kind,
                        super::Place::Msaa { class, .. } => matches!(
                            class,
                            super::MsaaClass::Focus
                                | super::MsaaClass::Foreground
                                | super::MsaaClass::Menu
                        ),
                    },
                    Item::Run(_) | Item::Menu { .. } => true,
                };
                if focus_kind {
                    searching = false;
                } else if let Item::One(entry) = &item
                    && pick(entry)
                {
                    if let Item::One(entry) = item {
                        taken.push(entry);
                    }
                    continue;
                }
            }
            kept.push_back(item);
        }
        *waiting = kept;
        taken
    }

    /// The backend verdicts of windows.
    pub(crate) fn backend(&self) -> Ref<'_, BackendCache> {
        Ref::map(self.shared.borrow(), |shared| &shared.state.queue.backend)
    }

    /// The foreground record, `None` before the listener's first report.
    pub(crate) fn foreground(&self) -> Option<Foreground> {
        self.shared.borrow().state.queue.foreground
    }

    /// The running instance of `reader`, if it can take requests.
    pub(crate) fn instance(&self, reader: Reader) -> Option<Instance> {
        self.shared
            .borrow()
            .state
            .queue
            .readers
            .get(&reader)
            .copied()
    }

    /// The time of the input being handled.
    pub(crate) fn now_us(&self) -> u64 {
        self.shared.borrow().state.queue.now_us
    }

    /// The due time of a request, or a chain of requests, Core decides to
    /// ask now: `after_us` from now ([`super::FOCUS_DUE_US`],
    /// [`super::OTHER_DUE_US`]).
    pub(crate) fn due_in(&self, after_us: u64) -> u64 {
        self.now_us().saturating_add(after_us)
    }

    /// Appends `entry` to the back of the queue `after_us` from now.
    pub(crate) fn append_after(&mut self, after_us: u64, entry: L::Entry) {
        let mut shared = self.shared.borrow_mut();
        let at_us = shared.state.queue.now_us.saturating_add(after_us);
        shared.state.queue.timers.push((at_us, entry));
    }

    /// Whether an entry [`append_after`](Self::append_after) set to come,
    /// one `picks` takes, is due within `window_us` of `after_us` from now.
    pub(crate) fn timer_due_near(
        &self,
        after_us: u64,
        window_us: u64,
        picks: impl Fn(&L::Entry) -> bool,
    ) -> bool {
        let shared = self.shared.borrow();
        let at_us = shared.state.queue.now_us.saturating_add(after_us);
        shared
            .state
            .queue
            .timers
            .iter()
            .any(|(due_us, entry)| due_us.abs_diff(at_us) <= window_us && picks(entry))
    }

    /// Moves `entry`, the front's own entry as it is to be judged again,
    /// to the back of the queue, behind every entry waiting, keeping its
    /// speech fence: the front's function then ends.
    pub(crate) fn requeue(&mut self, entry: L::Entry) {
        let mut shared = self.shared.borrow_mut();
        let state = &mut shared.state;
        super::limiter::arrive::<L>(&state.facts, &mut state.queue.waiting, entry);
    }

    /// Emits `said`, stamped by its entry ([`Logic::stamp`]).
    pub(crate) fn emit(&mut self, mut said: L::Said) {
        let mut shared = self.shared.borrow_mut();
        if let Some(front) = &shared.state.queue.front {
            L::stamp(&front.entry, &mut said);
        }
        shared.effects.push(Effect::Said(said));
    }

    /// Emits `said` as its function stamped it, not by its entry.
    pub(crate) fn emit_stamped(&mut self, said: L::Said) {
        self.shared.borrow_mut().effects.push(Effect::Said(said));
    }

    /// Sends `request` outside the event queue under the due time
    /// `due_us`, in order with the queue's requests, awaiting no answer:
    /// its ending changes nothing in Core. A rebuilt function's sends are
    /// not made again, since its state is let go.
    pub(crate) fn send<R: Request>(&mut self, request: &R, due_us: u64) {
        let mut shared = self.shared.borrow_mut();
        let reader = request.reader();
        let ready = shared.state.queue.readers.get(&reader).copied();
        let now_us = shared.state.queue.now_us;
        let mut orders = Vec::new();
        let state = &mut shared.state;
        state.queue.table.begin(
            Live {
                reader,
                window: request.window(),
                due_us,
                request: request.wire(),
                owner: Owner::Core,
                stage: Stage::Waiting,
                probing: None,
            },
            ready,
            &state.queue.backend,
            now_us,
            &mut orders,
        );
        Core::<L>::order(&mut shared, orders);
    }

    /// Sends at once the requests Core's facts want ([`Logic::wanted`]),
    /// in order with this function's requests, as a commit sends a
    /// terminal's baseline just after its `Follow`.
    pub(crate) fn send_wanted(&mut self) {
        let mut shared = self.shared.borrow_mut();
        let wanted = L::wanted(&mut shared.state.facts);
        Core::<L>::begin_core(&mut shared, wanted);
    }

    /// Asks `request` under the due time `due_us` and waits for its one
    /// outcome.
    pub(crate) fn ask<R: Request>(&mut self, request: R, due_us: u64) -> Asking<'_, L, R> {
        Asking {
            cx: self,
            stage: AskStage::Unsent { request, due_us },
        }
    }
}

/// One request: made at its first poll, ready at the poll after the queue
/// has put its outcome in its slot.
pub(crate) struct Asking<'a, L: Logic, R: Request> {
    cx: &'a mut Cx<L>,
    stage: AskStage<R>,
}

enum AskStage<R: Request> {
    Unsent { request: R, due_us: u64 },
    Made(Rc<Typed<R>>),
    Done,
}

impl<L: Logic, R: Request> Future for Asking<'_, L, R> {
    type Output = Outcome<R::Reply>;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut shared = this.cx.shared.borrow_mut();
        match std::mem::replace(&mut this.stage, AskStage::Done) {
            AskStage::Unsent { request, due_us } => {
                let slot = Rc::new(Typed::<R> {
                    outcome: Cell::new(None),
                });
                let owner: Rc<dyn Slot> = slot.clone();
                let reader = request.reader();
                let ready = shared.state.queue.readers.get(&reader).copied();
                let now_us = shared.state.queue.now_us;
                let wire = request.wire();
                let mut orders = Vec::new();
                let state = &mut shared.state;
                if let Some(checkpoint) = state.queue.checkpoint.as_mut() {
                    checkpoint.asked.push(wire.clone());
                }
                let number = state.queue.table.begin(
                    Live {
                        reader,
                        window: request.window(),
                        due_us,
                        request: wire,
                        owner: Owner::Queue,
                        stage: Stage::Waiting,
                        probing: None,
                    },
                    ready,
                    &state.queue.backend,
                    now_us,
                    &mut orders,
                );
                Core::<L>::order(&mut shared, orders);
                let front = shared.state.queue.front.as_mut();
                assert!(
                    front.as_ref().is_some_and(|front| front.asked.is_none()),
                    "only the front entry asks, one request at a time"
                );
                if let Some(front) = front {
                    front.asked = Some(number);
                }
                shared.slot = Some(owner);
                this.stage = AskStage::Made(slot);
                shared.parked = true;
                Poll::Pending
            }
            AskStage::Made(slot) => {
                if let Some(outcome) = slot.outcome.take() {
                    return Poll::Ready(outcome);
                }
                this.stage = AskStage::Made(slot);
                shared.parked = true;
                Poll::Pending
            }
            AskStage::Done => {
                drop(shared);
                Poll::Pending
            }
        }
    }
}

impl<L: Logic, R: Request> Drop for Asking<'_, L, R> {
    fn drop(&mut self) {
        // An ask is dropped in its made stage only before its outcome was
        // returned: a function that let go of a request it made.
        if matches!(self.stage, AskStage::Made(_)) {
            let mut shared = self.cx.shared.borrow_mut();
            shared.unanswered = shared.unanswered.saturating_add(1);
        }
    }
}
