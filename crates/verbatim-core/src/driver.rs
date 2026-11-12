//! Core's driver: a sequence with waits written as one async function, run
//! on the reducer thread inside the reduce step that wakes it.
//!
//! [`Core`] is the reducer's state ([`SrState`]) with the paused functions
//! beside it. A function is started by [`spawn`] and runs at once, in
//! place, to its first wait; it is resumed in place by the one thing it
//! waits for ([`answered`], [`signal`], [`due`] or [`reap`]), and what it
//! emits is returned to the caller that started or woke it, so its effects
//! take that caller's place in the step's list. Nothing here is a thread, a
//! lock or a queue: a function is polled only from inside a reduce step,
//! with a waker that does nothing, and one input wakes functions in the
//! order they were started.
//!
//! A function reaches the state only through its [`Cx`], between waits.
//! The driver lends the whole [`Core`] to the function for the length of
//! one poll by moving it into a cell the two share and moving it back when
//! the poll returns; [`Cx::with`] moves it out of the cell for the length
//! of a closure and back. The moves nest, so reducer code run by a
//! function can start or wake another function, and no reference to the
//! state is ever held across a wait.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use verbatim_model::{Effect, OutpostId, QueryId, TraceId};

use crate::queries::Ending;
use crate::state::SrState;

/// The reducer's state and the functions paused beside it.
///
/// It reads as the state it holds, which is all the shell's views need. It
/// is neither cloned, compared nor written whole: the state alone is, and
/// only while nothing is paused ([`Core::checkpoint`]).
#[derive(Default)]
pub struct Core {
    state: SrState,
    tasks: Tasks,
    /// How this crate's unit tests copy a state with functions paused.
    #[cfg(test)]
    replayed: Replayed,
}

impl Core {
    /// An initial state with nothing paused.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether no function is paused: the only moments the flight recorder
    /// may take a checkpoint, since a paused function cannot be copied.
    #[must_use]
    pub fn quiescent(&self) -> bool {
        self.tasks.paused.is_empty()
    }

    /// The state, when nothing is paused beside it: what a checkpoint
    /// clones and a dump writes.
    #[must_use]
    pub fn checkpoint(&self) -> Option<&SrState> {
        self.quiescent().then_some(&self.state)
    }

    /// The earliest time a paused function waits for, in microseconds on
    /// the run's shared clock: a view read after each step, as the state's
    /// timers are, which the shell delivers with [`due`] once it is past.
    #[must_use]
    pub fn next_due(&self) -> Option<u64> {
        self.tasks
            .paused
            .values()
            .filter_map(|paused| paused.wait.due_us)
            .min()
    }
}

impl From<SrState> for Core {
    fn from(state: SrState) -> Self {
        Self {
            #[cfg(test)]
            replayed: Replayed {
                base: state.clone(),
                inputs: Vec::new(),
            },
            state,
            tasks: Tasks::default(),
        }
    }
}

/// For this crate's unit tests, which copy the state at every step to try
/// each input from it: the state Core was made from and every input since,
/// from which a copy with the same functions paused is made by replaying.
#[cfg(test)]
#[derive(Default)]
struct Replayed {
    base: SrState,
    inputs: Vec<verbatim_model::Input>,
}

#[cfg(test)]
impl Core {
    /// Notes `input`, about to be reduced.
    pub(crate) fn note(&mut self, input: &verbatim_model::Input) {
        self.replayed.inputs.push(input.clone());
    }
}

/// A copy for a unit test: of the state alone when nothing is paused, and
/// otherwise made by replaying, since a paused function cannot be copied.
#[cfg(test)]
impl Clone for Core {
    fn clone(&self) -> Self {
        if let Some(state) = self.checkpoint() {
            return Self::from(state.clone());
        }
        let mut copy = Self::from(self.replayed.base.clone());
        for input in &self.replayed.inputs {
            let _ = crate::reduce::reduce(&mut copy, input);
        }
        copy
    }
}

/// For a unit test's comparisons: the states alone.
#[cfg(test)]
impl PartialEq for Core {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
    }
}

/// For a unit test's round trip through a dump's form: the state alone.
#[cfg(test)]
impl<'de> serde::Deserialize<'de> for Core {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        SrState::deserialize(deserializer).map(Self::from)
    }
}

impl Deref for Core {
    type Target = SrState;

    fn deref(&self) -> &SrState {
        &self.state
    }
}

impl DerefMut for Core {
    fn deref_mut(&mut self) -> &mut SrState {
        &mut self.state
    }
}

/// A state with functions paused beside it equals no state alone.
impl PartialEq<SrState> for Core {
    fn eq(&self, other: &SrState) -> bool {
        self.checkpoint() == Some(other)
    }
}

impl std::fmt::Debug for Core {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Core")
            .field("state", &self.state)
            .field("paused", &self.tasks.paused.len())
            .finish_non_exhaustive()
    }
}

/// The state alone is written: a paused function has no written form.
impl serde::Serialize for Core {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.state.serialize(serializer)
    }
}

/// The paused functions, by the order they were started.
#[derive(Default)]
struct Tasks {
    next: u64,
    paused: BTreeMap<u64, Paused>,
}

/// One paused function.
struct Paused {
    future: Pin<Box<dyn Future<Output = ()>>>,
    shared: Rc<Shared>,
    /// What it waits for.
    wait: Wait,
}

/// What the driver and one function share: the state while it is lent,
/// what the function has emitted, the wait it registers and what woke it.
#[derive(Default)]
struct Shared {
    core: Cell<Core>,
    emitted: Cell<Vec<Effect>>,
    wait: Cell<Option<Wait>>,
    woke: Cell<Option<Woke>>,
}

/// A kind of input a function waits for by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    /// The review press in the key line gives way: its bound passed, or a
    /// focus moved the navigator while it read.
    PressGivesWay,
    /// The key line's one deadline passed for the current caret key, or a
    /// focus left a key that is moving by sentence: the wait it bounds is
    /// over.
    KeyBound,
    /// The reducer ended the current caret key: a report was its evidence,
    /// the focus or a controlled selection moved on, or its outpost ended.
    KeyEnded,
}

/// What a function waits for: the first of an answer, any of some
/// signals and a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Wait {
    /// The query whose one ending wakes it.
    pub(crate) query: Option<QueryId>,
    pub(crate) signals: &'static [Signal],
    /// A time on the run's shared clock, in microseconds.
    pub(crate) due_us: Option<u64>,
}

/// What ended a wait.
#[expect(
    clippy::large_enum_variant,
    reason = "built once and moved into the function it wakes"
)]
pub(crate) enum Woke {
    /// Its query ended, under `trace_id`, the answer's trace.
    Answered {
        ending: Ending,
        trace_id: TraceId,
        /// When the answer was read, in microseconds, or 0.
        at_us: u64,
    },
    /// A signal, with the time its input carried, in microseconds, or 0.
    Signalled(Signal, u64),
    /// Its time passed.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no converted sequence waits on a time yet")
    )]
    Due,
    /// Its query was ended with no answer: forgotten at its outpost's end,
    /// or removed with a terminal's model.
    Lost,
}

/// A function's handle to the driver.
pub(crate) struct Cx {
    shared: Rc<Shared>,
}

impl Cx {
    /// Runs `work` on the state. Called only from inside the function,
    /// which runs only while the driver has lent the state.
    pub(crate) fn with<R>(&self, work: impl FnOnce(&mut Core) -> R) -> R {
        let mut core = self.shared.core.take();
        let result = work(&mut core);
        self.shared.core.set(core);
        result
    }

    /// Emits `effects`, after everything the function has emitted in this
    /// step.
    pub(crate) fn emit(&self, effects: impl IntoIterator<Item = Effect>) {
        let mut emitted = self.shared.emitted.take();
        emitted.extend(effects);
        self.shared.emitted.set(emitted);
    }

    /// Runs `work` on the state and emits what it returns.
    pub(crate) fn run(&self, work: impl FnOnce(&mut Core) -> Vec<Effect>) {
        let effects = self.with(work);
        self.emit(effects);
    }

    /// Waits for the first of what `wait` names.
    pub(crate) fn wait(&self, wait: Wait) -> impl Future<Output = Woke> + '_ {
        Waiting {
            cx: self,
            wait: Some(wait),
        }
    }
}

/// One wait: registered at its first poll, over at its second.
struct Waiting<'a> {
    cx: &'a Cx,
    wait: Option<Wait>,
}

impl Future for Waiting<'_> {
    type Output = Woke;

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Woke> {
        if let Some(wait) = self.wait.take() {
            self.cx.shared.wait.set(Some(wait));
            return Poll::Pending;
        }
        match self.cx.shared.woke.take() {
            Some(woke) => Poll::Ready(woke),
            None => Poll::Pending,
        }
    }
}

/// Starts `task` and runs it, in place, to its first wait, returning what
/// it emitted. A function that ends without waiting is never kept.
pub(crate) fn spawn<Task>(core: &mut Core, task: impl FnOnce(Cx) -> Task) -> Vec<Effect>
where
    Task: Future<Output = ()> + 'static,
{
    let shared = Rc::new(Shared::default());
    let future = Box::pin(task(Cx {
        shared: Rc::clone(&shared),
    }));
    let id = core.tasks.next;
    core.tasks.next = id.saturating_add(1);
    poll(
        core,
        id,
        Paused {
            future,
            shared,
            wait: Wait::default(),
        },
    )
}

/// Polls `paused` once with the state lent to it, and keeps it if it
/// waits again.
fn poll(core: &mut Core, id: u64, mut paused: Paused) -> Vec<Effect> {
    paused.shared.core.set(std::mem::take(core));
    let polled = paused
        .future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    *core = paused.shared.core.take();
    let emitted = paused.shared.emitted.take();
    if polled.is_pending() {
        // Every wait is one of the driver's, which registers itself.
        let wait = paused.shared.wait.take();
        debug_assert!(wait.is_some(), "a function waits only through its Cx");
        if let Some(wait) = wait {
            paused.wait = wait;
            core.tasks.paused.insert(id, paused);
        }
    }
    emitted
}

/// Wakes function `id` with `woke`, in place.
fn resume(core: &mut Core, id: u64, woke: Woke) -> Vec<Effect> {
    let Some(paused) = core.tasks.paused.remove(&id) else {
        return Vec::new();
    };
    paused.shared.woke.set(Some(woke));
    poll(core, id, paused)
}

/// The functions `which` picks, in the order they were started.
fn waiting(core: &Core, which: impl Fn(&Wait) -> bool) -> Vec<u64> {
    core.tasks
        .paused
        .iter()
        .filter(|(_, paused)| which(&paused.wait))
        .map(|(id, _)| *id)
        .collect()
}

/// Query `query` ended with `ending`: the function waiting for it goes on,
/// in place. An ending no function waits for runs nothing.
pub(crate) fn answered(
    core: &mut Core,
    query: QueryId,
    ending: Ending,
    trace_id: TraceId,
    at_us: u64,
) -> Vec<Effect> {
    let Some(id) = waiting(core, |wait| wait.query == Some(query))
        .first()
        .copied()
    else {
        return Vec::new();
    };
    resume(
        core,
        id,
        Woke::Answered {
            ending,
            trace_id,
            at_us,
        },
    )
}

/// Whether a function waits for query `query`.
pub(crate) fn awaits(core: &Core, query: QueryId) -> bool {
    !waiting(core, |wait| wait.query == Some(query)).is_empty()
}

/// Wakes each function waiting for `signal`, in the order they were
/// started.
pub(crate) fn signal(core: &mut Core, signal: Signal, at_us: u64) -> Vec<Effect> {
    let mut effects = Vec::new();
    for id in waiting(core, |wait| wait.signals.contains(&signal)) {
        effects.extend(resume(core, id, Woke::Signalled(signal, at_us)));
    }
    effects
}

/// Time has reached `at_us`: wakes each function whose time that is, in
/// the order they were started.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "no converted sequence waits on a time yet")
)]
pub(crate) fn due(core: &mut Core, at_us: u64) -> Vec<Effect> {
    let mut effects = Vec::new();
    for id in waiting(core, |wait| wait.due_us.is_some_and(|due| due <= at_us)) {
        effects.extend(resume(core, id, Woke::Due));
    }
    effects
}

/// Outpost `outpost` has ended: the queries functions wait for there are
/// turned to nothing before the outpost's queries end, since "gone" is no
/// answer to a function, which [`reap`] then ends.
pub(crate) fn forget_queries_at(core: &mut Core, outpost: OutpostId) {
    let queries: Vec<QueryId> = core
        .tasks
        .paused
        .values()
        .filter_map(|paused| paused.wait.query)
        .collect();
    for query in queries {
        core.queries.forget_at(query, outpost);
    }
}

/// Wakes each function whose query has ended with no answer, as lost: the
/// one ending of a function whose outpost ended or whose terminal's model
/// went, which then runs its own last step.
pub(crate) fn reap(core: &mut Core) -> Vec<Effect> {
    let mut effects = Vec::new();
    for id in waiting(core, |wait| wait.query.is_some()) {
        let lost = core
            .tasks
            .paused
            .get(&id)
            .and_then(|paused| paused.wait.query)
            .is_some_and(|query| !core.queries.has_open(query));
        if lost {
            effects.extend(resume(core, id, Woke::Lost));
        }
    }
    effects
}

/// Verbatim is exiting: every paused function is dropped, nothing more
/// being said or asked.
pub(crate) fn exit(core: &mut Core) {
    core.tasks.paused.clear();
}

#[cfg(test)]
mod tests {
    use verbatim_model::protocol::QueryOutcome;
    use verbatim_model::{NodeId, OutpostId};

    use super::*;
    use crate::queries::{Asking, Then};

    fn node() -> NodeId {
        NodeId::in_outpost(OutpostId(1), 1)
    }

    fn marker(number: u64) -> Effect {
        Effect::PassHeldKey(TraceId::from_raw(number))
    }

    fn query_of(effects: &[Effect]) -> QueryId {
        match effects.last() {
            Some(Effect::ReadNode { query_id, .. }) => *query_id,
            other => panic!("no request last: {other:?}"),
        }
    }

    /// A function that emits `first`, asks, and emits `then` once its
    /// query ends, or `otherwise` when anything else wakes it.
    fn asker(
        core: &mut Core,
        first: u64,
        then: u64,
        otherwise: u64,
        due_us: Option<u64>,
    ) -> Vec<Effect> {
        spawn(core, move |cx| async move {
            cx.emit([marker(first)]);
            let (query, request) =
                cx.with(|core| core.queries.ask(Asking::ReadNode(node()), Then::Task));
            cx.emit([request]);
            let wait = Wait {
                query: Some(query),
                signals: &[Signal::PressGivesWay],
                due_us,
            };
            match cx.wait(wait).await {
                Woke::Answered { .. } => cx.emit([marker(then)]),
                _ => cx.emit([marker(otherwise)]),
            }
        })
    }

    #[test]
    fn a_function_runs_in_place_to_its_wait_and_goes_on_at_its_answer() {
        let mut core = Core::new();
        let started = asker(&mut core, 1, 2, 3, None);
        assert_eq!(started.first(), Some(&marker(1)));
        assert!(!core.quiescent());
        assert!(core.checkpoint().is_none());
        let query = query_of(&started);
        let ending = Ending::Outcome(QueryOutcome::Gone);
        // The table routes the ending, as an outpost's answer is routed.
        let then = core.queries.end(query).expect("the query is open");
        let resumed =
            crate::queries::ended(&mut core, TraceId::from_raw(9), query, then, ending, 0);
        assert_eq!(resumed, [marker(2)]);
        assert!(core.quiescent());
        assert!(core.checkpoint().is_some());
    }

    #[test]
    fn a_signal_wakes_the_functions_waiting_for_it_in_the_order_started() {
        let mut core = Core::new();
        let _ = asker(&mut core, 1, 2, 3, None);
        let _ = asker(&mut core, 4, 5, 6, None);
        assert_eq!(
            signal(&mut core, Signal::PressGivesWay, 0),
            [marker(3), marker(6)]
        );
        assert!(core.quiescent());
    }

    #[test]
    fn the_earliest_time_is_one_view_and_wakes_what_is_due() {
        let mut core = Core::new();
        let _ = asker(&mut core, 1, 2, 3, Some(700));
        let _ = asker(&mut core, 4, 5, 6, Some(500));
        assert_eq!(core.next_due(), Some(500));
        assert_eq!(due(&mut core, 600), [marker(6)]);
        assert_eq!(core.next_due(), Some(700));
        assert_eq!(due(&mut core, 700), [marker(3)]);
        assert_eq!(core.next_due(), None);
    }

    #[test]
    fn a_function_whose_query_ended_unanswered_is_woken_lost() {
        let mut core = Core::new();
        let started = asker(&mut core, 1, 2, 3, None);
        assert_eq!(reap(&mut core), []);
        forget_queries_at(&mut core, OutpostId(1));
        let gone = crate::queries::outpost_gone(&mut core, TraceId::from_raw(9), OutpostId(1));
        assert_eq!(gone, []);
        assert!(!core.queries.has_open(query_of(&started)));
        assert_eq!(reap(&mut core), [marker(3)]);
        assert!(core.quiescent());
    }

    #[test]
    fn the_recorder_takes_no_checkpoint_while_a_function_is_paused() {
        let mut core = Core::new();
        let mut recorder = crate::ReducerRecorder::new(4, 1 << 20, SrState::new());
        let _ = asker(&mut core, 1, 2, 3, None);
        // A segment closes at two entries, half the window: none does here,
        // so nothing is dropped and the window grows past its four.
        for _ in 0..6 {
            recorder.record_input(verbatim_model::Input::Tick, 0, &core);
        }
        assert_eq!(recorder.len(), 6);
        let _ = signal(&mut core, Signal::PressGivesWay, 0);
        // The first step with nothing paused closes the segment, and the
        // window, over its bound, drops it whole.
        recorder.record_input(verbatim_model::Input::Tick, 0, &core);
        assert_eq!(recorder.len(), 0);
        assert_eq!(
            recorder.checkpoint(),
            core.checkpoint().expect("nothing is paused")
        );
    }

    #[test]
    fn a_function_started_by_another_runs_inside_it() {
        let mut core = Core::new();
        let effects = spawn(&mut core, |cx| async move {
            cx.emit([marker(1)]);
            cx.run(|core| asker(core, 2, 3, 4, None));
            cx.emit([marker(5)]);
        });
        assert_eq!(effects.first(), Some(&marker(1)));
        assert_eq!(effects.get(1), Some(&marker(2)));
        assert_eq!(effects.last(), Some(&marker(5)));
        assert!(!core.quiescent(), "the inner function still waits");
    }
}
