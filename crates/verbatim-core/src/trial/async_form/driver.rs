//! The async form's driver: the event queue and the one request out, with
//! the front entry's handling paused beside them as an async function.
//!
//! The driver owns the queue and the waiting. It starts the front entry's
//! function in place inside the step that takes the entry, and resumes it
//! in place inside the step that ends its request, so whatever the
//! function emits takes that step's place in the effects. It is polled
//! with a waker that does nothing, on Core's thread: no runtime.
//!
//! The function reaches the queue only through its [`Cx`], which lends the
//! queue's facts as a borrow and asks. [`Asking`] is the only future the
//! function can await that ever waits: the driver checks, at every pause,
//! that the function paused in an ask with its request out, and, at every
//! entry's end, that each request it sent had its one outcome.
//!
//! The recorder keeps the state as it was when the front entry started,
//! and every input since; a checkpoint is a fresh instance built from that
//! state by running the entry's function again and replaying the inputs.

use std::cell::{Cell, Ref, RefCell};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use super::super::harness::{
    self, Ask, Effect, Ending, Form, Input, Reader, RequestId, Stop, Timed, Window, within,
};
use super::logic;
use super::request::{Outcome, Request};

/// Core: the queue, shared with the paused function, the function, and
/// the recorder.
pub struct AsyncCore {
    shared: Rc<RefCell<Shared>>,
    /// The front entry's function, paused in its one ask; present exactly
    /// when the queue has a front, which [`AsyncCore::check`] asserts.
    task: Option<Pin<Box<dyn Future<Output = ()>>>>,
    record: Record,
    /// Every entry that ended, with the outcome each of its requests had:
    /// the trial's audit of the one-outcome rule, not Core's state.
    ended: Vec<Ended>,
}

/// What the driver and the front's function share.
struct Shared {
    state: State,
    /// What the function emitted while it ran, taken by the driver when
    /// the poll returns; empty between steps.
    emitted: Vec<Effect>,
    /// Set by an [`Asking`] when it returns pending, cleared before each
    /// poll: a function that returns pending with this unset waited on
    /// something other than an ask.
    parked: bool,
    /// Asks dropped after their request was sent and before their outcome
    /// was taken; zero whenever an entry ends.
    unanswered: u32,
}

/// Core's state: what a dump shows and a checkpoint copies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct State {
    now_us: u64,
    next_request: u64,
    front: Option<Front>,
    waiting: VecDeque<Entry>,
}

/// The front entry, the outcome of each request it has had, and its
/// request out, absent only while its function runs between two requests.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Front {
    entry: Entry,
    answers: Vec<Shown>,
    out: Option<Out>,
}

/// The request out, and where its decoded outcome goes.
#[derive(Clone)]
struct Out {
    id: RequestId,
    reader: Reader,
    window: Window,
    request: harness::Request,
    due_us: u64,
    slot: Rc<dyn Slot>,
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "the slot is where the outcome goes, not a fact a dump shows"
)]
impl fmt::Debug for Out {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Out")
            .field("id", &self.id)
            .field("reader", &self.reader)
            .field("window", &self.window)
            .field("request", &self.request)
            .field("due_us", &self.due_us)
            .finish()
    }
}

/// Two requests out are the same when everything but where the outcome
/// goes is the same: a rebuilt instance has its own function, and so its
/// own slot.
impl PartialEq for Out {
    fn eq(&self, other: &Self) -> bool {
        (
            self.id,
            self.reader,
            self.window,
            &self.request,
            self.due_us,
        ) == (
            other.id,
            other.reader,
            other.window,
            &other.request,
            other.due_us,
        )
    }
}

impl Eq for Out {}

/// An outcome as a dump writes it, decoded into its request's own type.
#[derive(Clone, PartialEq, Eq)]
struct Shown(String);

impl fmt::Debug for Shown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An entry that ended, and the outcome of each request it sent.
pub(crate) struct Ended {
    entry: Entry,
    answers: Vec<Shown>,
}

impl fmt::Debug for Ended {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ended")
            .field("entry", &self.entry)
            .field("answers", &self.answers)
            .finish()
    }
}

/// The state when the front entry started, or when the queue last became
/// empty, and every input since.
struct Record {
    base: State,
    since: Vec<Timed>,
}

/// An entry of the event queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Focus {
        app: u32,
        window: Window,
        token: u32,
    },
    CaretKey {
        key: u32,
        app: u32,
        window: Window,
        silent: bool,
    },
}

impl Form for AsyncCore {
    fn new() -> Self {
        Self::from_state(State::default())
    }

    fn feed(&mut self, timed: &Timed) -> Vec<Effect> {
        self.record.since.push(timed.clone());
        self.shared.borrow_mut().state.now_us = timed.at_us;
        let mut effects = Vec::new();
        match &timed.input {
            Input::Focus { app, window, token } => {
                if self.watching() {
                    self.give_up(Stop::PendingFocus, &mut effects);
                }
                self.shared
                    .borrow_mut()
                    .state
                    .waiting
                    .push_back(Entry::Focus {
                        app: *app,
                        window: *window,
                        token: *token,
                    });
            }
            Input::CaretKey { key, app, window } => {
                self.later_key(&mut effects);
                self.shared
                    .borrow_mut()
                    .state
                    .waiting
                    .push_back(Entry::CaretKey {
                        key: *key,
                        app: *app,
                        window: *window,
                        silent: false,
                    });
            }
            Input::Answer { id, outcome } => {
                if self.out().is_some_and(|out| out.id == *id) {
                    self.answered(outcome.clone(), &mut effects);
                }
            }
            Input::Foreground { window } => self.foreground(*window, &mut effects),
            Input::ReaderEnded { reader } => {
                if self.out().is_some_and(|out| out.reader == *reader) {
                    self.answered(harness::Outcome::ReaderEnded, &mut effects);
                }
                if *reader == Reader::Listener {
                    self.shared
                        .borrow_mut()
                        .state
                        .waiting
                        .retain(|entry| !matches!(entry, Entry::Focus { .. }));
                }
            }
            Input::Tick => {
                let now_us = self.shared.borrow().state.now_us;
                if self.next_due().is_some_and(|due| due <= now_us) {
                    self.give_up(Stop::DueTime, &mut effects);
                }
            }
        }
        self.take_next(&mut effects);
        self.check();
        effects
    }

    fn dump(&self) -> String {
        format!("{self:?}")
    }

    fn checkpoint(&self) -> Option<Self> {
        Some(self.rebuild())
    }

    fn next_due(&self) -> Option<u64> {
        self.out().map(|out| out.due_us)
    }
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "a dump shows Core's state; the function, the recorder and the audit are not part of it"
)]
impl fmt::Debug for AsyncCore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shared = self.shared.borrow();
        let state = &shared.state;
        formatter
            .debug_struct("AsyncCore")
            .field("now_us", &state.now_us)
            .field("next_request", &state.next_request)
            .field("front", &state.front)
            .field("waiting", &state.waiting)
            .finish()
    }
}

impl AsyncCore {
    fn from_state(state: State) -> Self {
        Self {
            record: Record {
                base: state.clone(),
                since: Vec::new(),
            },
            shared: Rc::new(RefCell::new(Shared {
                state,
                emitted: Vec::new(),
                parked: false,
                unanswered: 0,
            })),
            task: None,
            ended: Vec::new(),
        }
    }

    /// Every entry that has ended in this instance, in order.
    pub(crate) fn ended(&self) -> &[Ended] {
        &self.ended
    }

    /// A fresh instance with this one's state, built from the recorder:
    /// the state when the front entry started, its function run again to
    /// its first request, and every input since replayed, their effects
    /// dropped because they have already been made.
    fn rebuild(&self) -> Self {
        let mut fresh = Self::from_state(self.record.base.clone());
        let front = fresh.front_entry();
        if let Some(entry) = front {
            fresh.start(&entry, &mut Vec::new());
        }
        for input in &self.record.since {
            fresh.feed(input);
        }
        assert_eq!(
            fresh.shared.borrow().state,
            self.shared.borrow().state,
            "a rebuilt instance has the state it was built from"
        );
        fresh
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
                front.out.is_some(),
                "a paused function has its one request out, whose ending resumes it"
            );
        }
        assert!(shared.emitted.is_empty(), "every emitted effect is taken");
    }

    fn front_entry(&self) -> Option<Entry> {
        let shared = self.shared.borrow();
        shared.state.front.as_ref().map(|front| front.entry.clone())
    }

    fn out(&self) -> Option<Ref<'_, Out>> {
        Ref::filter_map(self.shared.borrow(), |shared| {
            shared
                .state
                .front
                .as_ref()
                .and_then(|front| front.out.as_ref())
        })
        .ok()
    }

    fn watching(&self) -> bool {
        self.out()
            .is_some_and(|out| matches!(out.request, harness::Request::Watch { .. }))
    }

    /// A key arrived: every caret key ahead of it is marked silent, and a
    /// watching one is cancelled.
    fn later_key(&mut self, effects: &mut Vec<Effect>) {
        {
            let mut shared = self.shared.borrow_mut();
            let state = &mut shared.state;
            if let Some(Front {
                entry: Entry::CaretKey { silent, .. },
                ..
            }) = &mut state.front
            {
                *silent = true;
            }
            for entry in &mut state.waiting {
                if let Entry::CaretKey { silent, .. } = entry {
                    *silent = true;
                }
            }
        }
        if self.watching() {
            self.give_up(Stop::LaterKey, effects);
        }
    }

    /// The foreground moved to `foreground`: the front's request and every
    /// waiting entry outside its tree end.
    fn foreground(&mut self, foreground: Window, effects: &mut Vec<Effect>) {
        if self
            .out()
            .is_some_and(|out| !within(out.window, foreground))
        {
            self.give_up(Stop::Foreground, effects);
        }
        let mut shared = self.shared.borrow_mut();
        let mut kept = VecDeque::new();
        for entry in shared.state.waiting.drain(..) {
            match entry {
                Entry::Focus { window, .. } | Entry::CaretKey { window, .. }
                    if within(window, foreground) =>
                {
                    kept.push_back(entry);
                }
                Entry::CaretKey { key, .. } => effects.push(Effect::SendKey(key)),
                Entry::Focus { .. } => {}
            }
        }
        shared.state.waiting = kept;
    }

    fn give_up(&mut self, reason: Stop, effects: &mut Vec<Effect>) {
        let id = self.out().map(|out| out.id);
        if let Some(id) = id {
            effects.push(Effect::Cancel { id, reason });
            self.resume(Ending::Stopped(reason), effects);
        }
    }

    fn answered(&mut self, outcome: harness::Outcome, effects: &mut Vec<Effect>) {
        if outcome == harness::Outcome::NotResponding {
            effects.push(Effect::NotRespondingSound);
        }
        self.resume(Ending::Answered(outcome), effects);
    }

    /// Ends the front's request with `ending`, decoded once here into the
    /// request's own type, and runs its function to its next request or
    /// its end. A result that does not decode is a protocol error: the
    /// request ends as "the reader ended", and the function never sees the
    /// reply.
    fn resume(&mut self, ending: Ending, effects: &mut Vec<Effect>) {
        {
            let mut shared = self.shared.borrow_mut();
            let front = shared
                .state
                .front
                .as_mut()
                .expect("a request ends only while an entry is at the front");
            let out = front
                .out
                .take()
                .expect("a request ends only while it is out");
            let shown = match ending {
                Ending::Answered(harness::Outcome::Result(reply)) => out
                    .slot
                    .decode(reply)
                    .unwrap_or_else(|Undecodable| out.slot.end(Outcome::ReaderEnded)),
                Ending::Answered(harness::Outcome::Refused(reason)) => {
                    out.slot.end(Outcome::Refused(reason))
                }
                Ending::Answered(harness::Outcome::NotThere) => out.slot.end(Outcome::NotThere),
                Ending::Answered(harness::Outcome::NotResponding) => {
                    out.slot.end(Outcome::NotResponding)
                }
                Ending::Answered(harness::Outcome::ReaderEnded) => {
                    out.slot.end(Outcome::ReaderEnded)
                }
                Ending::Stopped(reason) => out.slot.end(Outcome::GivenUp(reason)),
            };
            front.answers.push(shown);
        }
        self.poll(effects);
    }

    fn take_next(&mut self, effects: &mut Vec<Effect>) {
        while self.task.is_none() {
            let entry = {
                let mut shared = self.shared.borrow_mut();
                let Some(entry) = shared.state.waiting.pop_front() else {
                    break;
                };
                shared.state.front = Some(Front {
                    entry: entry.clone(),
                    answers: Vec::new(),
                    out: None,
                });
                entry
            };
            self.record = Record {
                base: self.shared.borrow().state.clone(),
                since: Vec::new(),
            };
            self.start(&entry, effects);
        }
        if self.task.is_none() && self.record.base.front.is_some() {
            self.record = Record {
                base: self.shared.borrow().state.clone(),
                since: Vec::new(),
            };
        }
    }

    /// Starts the front entry's function and runs it to its first request
    /// or its end.
    fn start(&mut self, entry: &Entry, effects: &mut Vec<Effect>) {
        let cx = Cx {
            shared: Rc::clone(&self.shared),
        };
        self.task = Some(logic::start(cx, entry));
        self.poll(effects);
    }

    /// Polls the front's function once.
    fn poll(&mut self, effects: &mut Vec<Effect>) {
        let Some(task) = self.task.as_mut() else {
            return;
        };
        self.shared.borrow_mut().parked = false;
        let polled = task.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        let mut shared = self.shared.borrow_mut();
        effects.append(&mut shared.emitted);
        match polled {
            Poll::Pending => {
                assert!(
                    shared.parked,
                    "an entry's function waited on something other than an ask"
                );
            }
            Poll::Ready(()) => {
                let front = shared
                    .state
                    .front
                    .take()
                    .expect("a function runs only for the front entry");
                assert!(
                    front.out.is_none(),
                    "an entry's function ended with its request out"
                );
                assert_eq!(
                    shared.unanswered, 0,
                    "every request an entry sent had its one outcome"
                );
                self.ended.push(Ended {
                    entry: front.entry,
                    answers: front.answers,
                });
                self.task = None;
            }
        }
    }
}

/// A result that does not decode as its request's reply.
struct Undecodable;

/// Where one request's outcome goes: the [`Asking`] that sent it.
trait Slot {
    /// Decodes a result and puts it in the slot.
    fn decode(&self, reply: harness::Reply) -> Result<Shown, Undecodable>;
    /// Puts an outcome without a result in the slot.
    fn end(&self, outcome: Outcome<Infallible>) -> Shown;
}

/// The slot of a request of type `R`, holding its outcome from the moment
/// the queue decodes it until the function's next poll takes it.
struct Typed<R: Request> {
    outcome: Cell<Option<Outcome<R::Reply>>>,
}

impl<R: Request> Typed<R> {
    fn put(&self, outcome: Outcome<R::Reply>) -> Shown {
        let shown = Shown(format!("{outcome:?}"));
        let earlier = self.outcome.replace(Some(outcome));
        assert!(earlier.is_none(), "a request has one outcome");
        shown
    }
}

impl<R: Request> Slot for Typed<R> {
    fn decode(&self, reply: harness::Reply) -> Result<Shown, Undecodable> {
        let reply = R::decode(reply).ok_or(Undecodable)?;
        Ok(self.put(Outcome::Result(reply)))
    }

    fn end(&self, outcome: Outcome<Infallible>) -> Shown {
        self.put(outcome.widen())
    }
}

/// The front function's context: it lends the queue's facts and asks.
/// Nothing else it offers can be awaited.
pub(crate) struct Cx {
    shared: Rc<RefCell<Shared>>,
}

/// The queue's facts, lent for as long as the borrow of the context they
/// came from lasts, which ends before the function's next ask.
pub(crate) struct Facts<'a>(Ref<'a, Shared>);

impl Facts<'_> {
    /// The time of the input being handled.
    pub(crate) fn now_us(&self) -> u64 {
        self.0.state.now_us
    }

    /// Whether a later key has marked this caret key silent.
    pub(crate) fn silent(&self) -> bool {
        matches!(
            self.0.state.front,
            Some(Front {
                entry: Entry::CaretKey { silent: true, .. },
                ..
            })
        )
    }

    /// Whether a focus waits behind this entry.
    pub(crate) fn focus_waiting(&self) -> bool {
        self.0
            .state
            .waiting
            .iter()
            .any(|entry| matches!(entry, Entry::Focus { .. }))
    }
}

impl Cx {
    /// The queue's facts as they are now.
    pub(crate) fn facts(&self) -> Facts<'_> {
        Facts(self.shared.borrow())
    }

    pub(crate) fn emit(&mut self, effect: Effect) {
        self.shared.borrow_mut().emitted.push(effect);
    }

    /// Sends `request`, naming `window`, under the chain's due time
    /// `due_us`, and waits for its one outcome.
    pub(crate) fn ask<R: Request>(
        &mut self,
        request: R,
        window: Window,
        due_us: u64,
    ) -> Asking<'_, R> {
        Asking {
            cx: self,
            stage: AskStage::Unsent {
                request,
                window,
                due_us,
            },
        }
    }
}

/// One request: sent at its first poll, ready at the poll after the queue
/// has put its outcome in its slot.
pub(crate) struct Asking<'a, R: Request> {
    cx: &'a mut Cx,
    stage: AskStage<R>,
}

enum AskStage<R: Request> {
    Unsent {
        request: R,
        window: Window,
        due_us: u64,
    },
    Sent(Rc<Typed<R>>),
    Done,
}

impl<R: Request> Future for Asking<'_, R> {
    type Output = Outcome<R::Reply>;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut shared = this.cx.shared.borrow_mut();
        match std::mem::replace(&mut this.stage, AskStage::Done) {
            AskStage::Unsent {
                request,
                window,
                due_us,
            } => {
                shared.state.next_request = shared.state.next_request.saturating_add(1);
                let id = RequestId(shared.state.next_request);
                let reader = request.reader();
                let wire = request.wire();
                shared.emitted.push(Effect::Ask(Ask {
                    id,
                    reader,
                    window,
                    request: wire.clone(),
                }));
                let slot = Rc::new(Typed::<R> {
                    outcome: Cell::new(None),
                });
                let front = shared
                    .state
                    .front
                    .as_mut()
                    .expect("only the front entry's function runs");
                assert!(front.out.is_none(), "an entry has one request out");
                front.out = Some(Out {
                    id,
                    reader,
                    window,
                    request: wire,
                    due_us,
                    slot: Rc::clone(&slot) as Rc<dyn Slot>,
                });
                this.stage = AskStage::Sent(slot);
                shared.parked = true;
                Poll::Pending
            }
            AskStage::Sent(slot) => {
                if let Some(outcome) = slot.outcome.take() {
                    return Poll::Ready(outcome);
                }
                this.stage = AskStage::Sent(slot);
                shared.parked = true;
                Poll::Pending
            }
            AskStage::Done => panic!("an ask is polled after its outcome"),
        }
    }
}

impl<R: Request> Drop for Asking<'_, R> {
    fn drop(&mut self) {
        // An ask is dropped in its sent stage only before its outcome was
        // returned: a function that let go of a request it sent.
        if matches!(self.stage, AskStage::Sent(_)) {
            let mut shared = self.cx.shared.borrow_mut();
            shared.unanswered = shared.unanswered.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Waker};

    use super::super::request::Caret;
    use super::{AsyncCore, Cx, Entry, Form, Front};
    use crate::trial::harness::Window;

    const EDIT: Window = Window { top: 1, id: 11 };

    /// A core whose front is a caret key with no function yet.
    fn core_with_front() -> AsyncCore {
        let core = AsyncCore::new();
        core.shared.borrow_mut().state.front = Some(Front {
            entry: Entry::CaretKey {
                key: 40,
                app: 1,
                window: EDIT,
                silent: false,
            },
            answers: Vec::new(),
            out: None,
        });
        core
    }

    #[test]
    #[should_panic(expected = "an entry's function waited on something other than an ask")]
    fn a_function_waiting_on_another_future_is_caught() {
        let mut core = core_with_front();
        core.task = Some(Box::pin(std::future::pending::<()>()));
        core.poll(&mut Vec::new());
    }

    #[test]
    fn an_ask_let_go_after_sending_is_counted() {
        let core = core_with_front();
        let mut cx = Cx {
            shared: Rc::clone(&core.shared),
        };
        let mut asking = cx.ask(Caret { app: 1 }, EDIT, 0);
        let polled = Pin::new(&mut asking).poll(&mut Context::from_waker(Waker::noop()));
        assert!(polled.is_pending());
        drop(asking);
        assert_eq!(core.shared.borrow().unanswered, 1);
    }
}
