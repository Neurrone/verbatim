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
//! The recorder keeps the state as it was when the front entry started,
//! and every input since; a checkpoint is a fresh instance built from that
//! state by running the entry's function again and replaying the inputs.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use verbatim_model::request::{Outcome as WireOutcome, Reply, RequestNumber};

use super::inbox::{Effect, Timed};
use super::request::{Outcome, Refused, Request};
use super::table::{Ending, Live, Order, Owner, Stage, Table};
use super::{Foreground, Function, Instance, Logic, Reader};
use crate::focus::backend::BackendCache;

/// Core: its state, shared with the paused function, the function, and
/// the recorder.
pub(crate) struct Core<L: Logic> {
    pub(super) shared: Rc<RefCell<Shared<L>>>,
    /// The front entry's function, paused in its one ask; present exactly
    /// when the queue has a front.
    task: Option<Function>,
    pub(super) record: Record<L>,
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
    /// Set by an [`Asking`] when it returns pending, cleared before each
    /// poll: a function that returns pending with this unset waited on
    /// something other than an ask.
    parked: bool,
    /// Asks dropped after their request was made and before their outcome
    /// was taken; zero whenever an entry ends.
    unanswered: u32,
}

/// Core's state: what a dump shows and a checkpoint copies.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct State<E, F> {
    /// The time of the input being handled, in microseconds on the run's
    /// shared clock.
    pub(super) now_us: u64,
    /// Core's facts, the [`Logic`]'s.
    pub(super) facts: F,
    /// The front entry.
    pub(super) front: Option<Front<E>>,
    /// The entries waiting behind it, in arrival order.
    pub(super) waiting: VecDeque<E>,
    /// The requests.
    pub(super) table: Table,
    /// The running instance of each reader that can take requests.
    pub(super) readers: BTreeMap<Reader, Instance>,
    /// The foreground record, `None` before the listener's first report.
    pub(super) foreground: Option<Foreground>,
    /// The backend verdicts of windows (Table 4).
    pub(super) backend: BackendCache,
}

impl<E, F: Default> Default for State<E, F> {
    fn default() -> Self {
        Self {
            now_us: 0,
            facts: F::default(),
            front: None,
            waiting: VecDeque::new(),
            table: Table::default(),
            readers: BTreeMap::new(),
            foreground: None,
            backend: BackendCache::default(),
        }
    }
}

/// The front entry, the outcome of each request it has had, and its
/// request, absent only while its function runs between two requests.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Front<E> {
    /// The entry.
    pub(super) entry: E,
    /// The outcome of each request it had, as a dump writes it.
    pub(super) answers: Vec<Shown>,
    /// Its request, waiting or out.
    pub(super) asked: Option<RequestNumber>,
}

/// An outcome as a dump writes it, decoded into its request's own type.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Shown(String);

impl fmt::Debug for Shown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The state when the front entry started, and every input since; no
/// base while the queue has no front, when the state itself is the
/// checkpoint.
pub(super) struct Record<L: Logic> {
    base: Option<State<L::Entry, L::Facts>>,
    since: Vec<Timed<L::Entry>>,
}

/// A checkpoint whose replay did not reach the state it was taken from:
/// replay is not exact, a fault of Core's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReplayMismatch {
    /// The state replayed, as a dump writes it.
    pub(crate) replayed: String,
    /// The state it was taken from.
    pub(crate) original: String,
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "a dump shows Core's state; the function, the recorder and the tests' audit are not part of it"
)]
impl<L: Logic> fmt::Debug for Core<L> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shared = self.shared.borrow();
        let state = &shared.state;
        formatter
            .debug_struct("Core")
            .field("now_us", &state.now_us)
            .field("front", &state.front)
            .field("waiting", &state.waiting)
            .field("table", &state.table)
            .field("readers", &state.readers)
            .field("foreground", &state.foreground)
            .field("backend", &state.backend)
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
                parked: false,
                unanswered: 0,
            })),
            task: None,
            record: Record {
                base: None,
                since: Vec::new(),
            },
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
        self.shared.borrow().state.table.next_due()
    }

    /// A fresh instance with this one's state, built from the recorder:
    /// the state when the front entry started, its function run again to
    /// its first request, and every input since replayed, their effects
    /// dropped because they have already been made.
    ///
    /// # Errors
    ///
    /// [`ReplayMismatch`] when the replay does not reach this state.
    pub(crate) fn checkpoint(&self) -> Result<Self, ReplayMismatch> {
        let Some(base) = &self.record.base else {
            return Ok(Self::from_state(self.shared.borrow().state.clone()));
        };
        let mut fresh = Self::from_state(base.clone());
        fresh.record.base = Some(base.clone());
        let front = fresh.front_entry();
        if let Some(entry) = front {
            fresh.start(entry);
        }
        fresh.shared.borrow_mut().effects.clear();
        for input in &self.record.since {
            let _ = fresh.feed(input.clone());
        }
        let mismatch = {
            let replayed = &fresh.shared.borrow().state;
            let original = &self.shared.borrow().state;
            (replayed != original).then(|| ReplayMismatch {
                replayed: format!("{replayed:?}"),
                original: format!("{original:?}"),
            })
        };
        match mismatch {
            Some(mismatch) => Err(mismatch),
            None => Ok(fresh),
        }
    }

    pub(super) fn note_input(&mut self, input: &Timed<L::Entry>) {
        if self.record.base.is_some() {
            self.record.since.push(input.clone());
        }
    }

    /// Ends the step: the recorder drops its base when the queue has no
    /// front, and the step's effects are returned.
    pub(super) fn finish_step(&mut self) -> Vec<Effect<L::Said>> {
        self.check();
        let mut shared = self.shared.borrow_mut();
        shared.state.table.settle_round();
        if shared.state.front.is_none() {
            self.record.base = None;
            self.record.since.clear();
        }
        std::mem::take(&mut shared.effects)
    }

    /// The driver's invariants between steps.
    fn check(&self) {
        let shared = self.shared.borrow();
        assert_eq!(
            self.task.is_some(),
            shared.state.front.is_some(),
            "a paused function exactly when there is a front entry"
        );
        if let Some(front) = &shared.state.front {
            assert!(
                front
                    .asked
                    .is_some_and(|number| shared.state.table.live(number).is_some()),
                "a paused function has its one request live, whose ending resumes it"
            );
        }
    }

    fn front_entry(&self) -> Option<L::Entry> {
        let shared = self.shared.borrow();
        shared.state.front.as_ref().map(|front| front.entry.clone())
    }

    /// Takes the next waiting entries while there is no front, starting
    /// each one's function.
    pub(super) fn take_next(&mut self) {
        while self.task.is_none() {
            let entry = {
                let mut shared = self.shared.borrow_mut();
                let Some(entry) = shared.state.waiting.pop_front() else {
                    break;
                };
                shared.state.front = Some(Front {
                    entry: entry.clone(),
                    answers: Vec::new(),
                    asked: None,
                });
                entry
            };
            self.record = Record {
                base: Some(self.shared.borrow().state.clone()),
                since: Vec::new(),
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

    /// Delivers a queue request's ending to its function and resumes it.
    pub(super) fn resume(&mut self, number: RequestNumber, slot: &Rc<dyn Slot>, ending: Ending) {
        let (shown, undecodable) = slot.deliver(ending);
        {
            let mut shared = self.shared.borrow_mut();
            if let Some(front) = shared.state.front.as_mut() {
                front.answers.push(shown);
                front.asked = None;
            }
            if undecodable {
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
                let front = shared.state.front.take();
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
    pub(crate) fn waiting(&self) -> Ref<'_, VecDeque<L::Entry>> {
        Ref::map(self.shared.borrow(), |shared| &shared.state.waiting)
    }

    /// The time of the input being handled.
    pub(crate) fn now_us(&self) -> u64 {
        self.shared.borrow().state.now_us
    }

    /// The due time of a request, or a chain of requests, Core decides to
    /// ask now: `after_us` from now ([`super::FOCUS_DUE_US`],
    /// [`super::OTHER_DUE_US`]).
    pub(crate) fn due_in(&self, after_us: u64) -> u64 {
        self.now_us().saturating_add(after_us)
    }

    /// Emits `said`.
    pub(crate) fn emit(&mut self, said: L::Said) {
        self.shared.borrow_mut().effects.push(Effect::Said(said));
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
                let ready = shared.state.readers.get(&reader).copied();
                let now_us = shared.state.now_us;
                let mut orders = Vec::new();
                let state = &mut shared.state;
                let number = state.table.begin(
                    Live {
                        reader,
                        window: request.window(),
                        due_us,
                        request: request.wire(),
                        owner: Owner::Queue(owner),
                        stage: Stage::Waiting,
                        probing: None,
                    },
                    ready,
                    &state.backend,
                    now_us,
                    &mut orders,
                );
                Core::<L>::order(&mut shared, orders);
                let front = shared.state.front.as_mut();
                assert!(
                    front.as_ref().is_some_and(|front| front.asked.is_none()),
                    "only the front entry asks, one request at a time"
                );
                if let Some(front) = front {
                    front.asked = Some(number);
                }
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
