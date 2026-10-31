//! Replay tests of [`InFlight`], one per rule, and one generated test that
//! compares it with a reference model.

use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngAlgorithm, RngSeed, TestRunner};

use super::*;

/// A millisecond, in microseconds.
const MS: u64 = 1_000;

/// The grace, in microseconds.
const GRACE: u64 = 500 * MS;

/// The window the tests' items are of.
const WINDOW: isize = 0x10;

fn publish(request: Request, then_abandon: Option<Reason>) -> Decision<u32> {
    Decision::Ask(Ask::Publish {
        request,
        then_abandon,
    })
}

fn moved_on(request: Request) -> Decision<u32> {
    Decision::Ask(Ask::MovedOn {
        request,
        window: WINDOW,
    })
}

fn abandoned(dead: u64, reason: Reason, published: bool) -> Decision<u32> {
    Decision::Abandon(Abandoned {
        dead,
        replacement: dead + 1,
        reason,
        item: 7,
        published,
    })
}

/// One late tick past the deadline, the bound and the moved-on rule's
/// judgment is decided by the deadline, publishing the armed walk first;
/// one past the bound and the judgment but before the deadline publishes,
/// and the publication's answer then judges the moved-on rule.
#[test]
fn a_late_tick_judges_the_deadline_then_the_bound_then_the_moved_on_rule() {
    let mut late = InFlight::new();
    let request = late.start(7, 2_000 * MS, Some(WINDOW), 0);
    late.armed(request, 1_000 * MS);
    assert_eq!(
        late.tick(3_000 * MS),
        publish(request, Some(Reason::Deadline))
    );
    assert_eq!(
        late.published(request, true, Some(Reason::Deadline), 3_000 * MS),
        abandoned(0, Reason::Deadline, true)
    );

    let mut early = InFlight::new();
    let request = early.start(7, 2_000 * MS, Some(WINDOW), 0);
    early.armed(request, 1_000 * MS);
    assert_eq!(early.tick(1_500 * MS), publish(request, None));
    assert_eq!(
        early.published(request, true, None, 1_500 * MS),
        moved_on(request)
    );
    assert_eq!(early.next_wake(), Some(2_000 * MS));
}

/// Each reason for an abandonment publishes an armed walk first, and the
/// item leaves with its walk published.
#[test]
fn an_armed_walk_is_published_before_each_abandonment() {
    let mut deadline = InFlight::new();
    let request = deadline.start(7, 400 * MS, None, 0);
    deadline.armed(request, 1_000 * MS);
    assert_eq!(
        deadline.tick(400 * MS),
        publish(request, Some(Reason::Deadline))
    );
    assert_eq!(
        deadline.published(request, true, Some(Reason::Deadline), 400 * MS),
        abandoned(0, Reason::Deadline, true)
    );

    let mut moved = InFlight::new();
    let request = moved.start(7, 10_000 * MS, Some(WINDOW), 0);
    moved.armed(request, 2_000 * MS);
    assert_eq!(moved.tick(GRACE), moved_on(request));
    assert_eq!(
        moved.moved_on(request, true),
        publish(request, Some(Reason::MovedOn))
    );
    assert_eq!(
        moved.published(request, true, Some(Reason::MovedOn), GRACE),
        abandoned(0, Reason::MovedOn, true)
    );
    assert_eq!(moved.generation(), 1);
    assert!(moved.running().is_none());
}

/// A window known at the start is judged at the grace's end and every
/// grace after; one learned past the grace is judged at once, and every
/// grace after.
#[test]
fn the_moved_on_rule_is_judged_every_half_second() {
    let mut known = InFlight::new();
    let request = known.start(7, 10_000 * MS, Some(WINDOW), 0);
    assert_eq!(known.next_wake(), Some(GRACE));
    assert_eq!(known.tick(GRACE - 1), Decision::Nothing);
    assert_eq!(known.tick(GRACE), moved_on(request));
    assert_eq!(known.moved_on(request, false), Decision::Nothing);
    assert_eq!(known.next_wake(), Some(2 * GRACE));
    assert_eq!(known.tick(2 * GRACE - 1), Decision::Nothing);
    assert_eq!(known.tick(2 * GRACE), moved_on(request));

    let mut learned = InFlight::new();
    let request = learned.start(7, 10_000 * MS, Some(0), 0);
    assert_eq!(learned.next_wake(), Some(10_000 * MS), "no window to judge");
    assert_eq!(learned.tick(600 * MS), Decision::Nothing);
    assert_eq!(
        learned.learned(request, WINDOW, 700 * MS),
        moved_on(request)
    );
    assert_eq!(learned.next_wake(), Some(700 * MS + GRACE));

    let mut early = InFlight::new();
    let request = early.start(7, 10_000 * MS, Some(0), 0);
    assert_eq!(early.learned(request, WINDOW, 100 * MS), Decision::Nothing);
    assert_eq!(early.next_wake(), Some(GRACE), "judged at the grace's end");
}

/// A published item still stuck is abandoned at once when the user has
/// moved on.
#[test]
fn a_published_item_is_abandoned_at_once_when_the_user_moves_on() {
    let mut in_flight = InFlight::new();
    let request = in_flight.start(7, 8_000 * MS, Some(WINDOW), 0);
    in_flight.armed(request, 2_000 * MS);
    assert_eq!(in_flight.tick(2_000 * MS), publish(request, None));
    assert_eq!(
        in_flight.published(request, true, None, 2_000 * MS),
        moved_on(request)
    );
    assert_eq!(in_flight.moved_on(request, false), Decision::Nothing);
    assert!(!in_flight.accepts(request), "published");
    assert_eq!(in_flight.tick(2_500 * MS), moved_on(request));
    assert_eq!(
        in_flight.moved_on(request, true),
        abandoned(0, Reason::MovedOn, true)
    );
}

/// An answer for a request no longer running is dropped, and the
/// abandonment it carried is not made.
#[test]
fn an_answer_to_a_dead_request_is_dropped() {
    let mut in_flight = InFlight::new();
    let request = in_flight.start(7, 8_000 * MS, Some(WINDOW), 0);
    assert_eq!(in_flight.end(request), Some((7, false)));
    let next = in_flight.start(8, 8_000 * MS, Some(WINDOW), 0);
    assert_eq!(in_flight.moved_on(request, true), Decision::Nothing);
    assert_eq!(
        in_flight.published(request, true, Some(Reason::Deadline), 0),
        Decision::Nothing
    );
    assert_eq!(
        in_flight.published(request, true, None, 0),
        Decision::Nothing
    );
    assert_eq!(in_flight.generation(), 0);
    assert!(in_flight.accepts(next), "the running item is untouched");
    assert_eq!(in_flight.end(request), None);
}

/// A walk that returns before its bound has no bound timed any more.
#[test]
fn a_walk_returned_before_its_bound_is_disarmed() {
    let mut in_flight = InFlight::new();
    let request = in_flight.start(7, 8_000 * MS, None, 0);
    in_flight.armed(request, 2_000 * MS);
    assert_eq!(in_flight.next_wake(), Some(2_000 * MS));
    assert_eq!(in_flight.walk_returned(request, false), Decision::Nothing);
    assert_eq!(in_flight.next_wake(), Some(8_000 * MS));
    assert_eq!(in_flight.tick(2_000 * MS), Decision::Nothing);
}

/// An item handed on to a new request keeps its times, its walk and its
/// moved-on rule, and the old request is refused.
#[test]
fn an_item_handed_on_keeps_its_times() {
    let mut in_flight = InFlight::new();
    let request = in_flight.start(7, 2_000 * MS, Some(WINDOW), 0);
    in_flight.armed(request, 1_000 * MS);
    let handed = in_flight.hand_on().expect("an item running");
    assert_eq!(
        handed,
        Request {
            generation: 0,
            number: 2
        }
    );
    assert!(!in_flight.accepts(request));
    assert!(in_flight.accepts(handed));
    assert_eq!(in_flight.next_wake(), Some(GRACE));
    assert_eq!(in_flight.tick(GRACE), moved_on(handed));
    assert_eq!(in_flight.moved_on(handed, false), Decision::Nothing);
    assert_eq!(in_flight.tick(1_000 * MS), publish(handed, None));
}

/// A generation succeeded without an abandonment refuses the last one's
/// requests and numbers the next.
#[test]
fn a_succeeded_generation_takes_charge() {
    let mut in_flight = InFlight::new();
    let request = in_flight.start(7, 2_000 * MS, None, 0);
    assert_eq!(in_flight.end(request), Some((7, false)));
    assert_eq!(in_flight.succeed(), 1);
    let next = in_flight.start(8, 2_000 * MS, None, 0);
    assert_eq!(
        next,
        Request {
            generation: 1,
            number: 2
        }
    );
    assert!(!in_flight.accepts(request));
}

/// One step of a generated sequence.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// Starts an item with no window (0), a window to learn (1) or a known
    /// window (2), and one of the readers' deadlines.
    Start { window: u8, deadline: u8 },
    /// The item learns its window.
    Learned,
    /// The item's walk is armed, its bound this many milliseconds from now
    /// less 200, so some bounds are already past.
    Armed(u16),
    /// Time advances this many milliseconds, and the reader ticks.
    Advance(u16),
    /// The item's walk returns, past its bound or not.
    WalkReturned(bool),
    /// The item ends.
    End,
    /// A new generation takes charge, when nothing is running.
    Succeed,
}

fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    prop_oneof![
        (0..3u8, 0..4u8).prop_map(|(window, deadline)| Action::Start { window, deadline }),
        Just(Action::Learned),
        (0..3_200u16).prop_map(Action::Armed),
        (0..1_200u16).prop_map(Action::Advance),
        // Steps that land on the grace's, the bounds' and the deadlines'
        // own times.
        prop_oneof![Just(100u16), Just(200), Just(400), Just(500)].prop_map(Action::Advance),
        any::<bool>().prop_map(Action::WalkReturned),
        Just(Action::End),
        Just(Action::Succeed),
    ]
}

/// The readers' deadlines: an outpost's step, text and handling deadlines,
/// and the listener's.
const DEADLINES: [u64; 4] = [400 * MS, 2_000 * MS, 10_000 * MS, 8_000 * MS];

/// What the model expects of a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    Nothing,
    MovedOn,
    Publish(Option<Reason>),
    /// Abandoned, why, and whether published.
    Abandon(Reason, bool),
}

/// The reference model's item: plain fields, each set by the rule that
/// names it.
#[derive(Clone, Debug)]
struct ModelItem {
    id: u32,
    request: Request,
    deadline: u64,
    /// Whether the item is never judged by the moved-on rule.
    never: bool,
    /// Its window, 0 while unknown.
    window: isize,
    /// When the moved-on rule is next judged, once the window is known.
    next: u64,
    started: u64,
    bound: Option<u64>,
    published: bool,
    /// Whether its walk was ever armed.
    ever_armed: bool,
}

/// The reference model: the item running, and the generation.
#[derive(Clone, Debug, Default)]
struct Model {
    item: Option<ModelItem>,
    generation: u64,
}

impl Model {
    fn judge(&mut self, now: u64) -> Expected {
        let Some(item) = self.item.as_mut() else {
            return Expected::Nothing;
        };
        if now >= item.deadline {
            if item.bound.take().is_some() {
                return Expected::Publish(Some(Reason::Deadline));
            }
            return self.abandon(Reason::Deadline);
        }
        if item.bound.is_some_and(|bound| now >= bound) {
            item.bound = None;
            return Expected::Publish(None);
        }
        if !item.never && item.window != 0 && now >= item.next {
            item.next = now + GRACE;
            return Expected::MovedOn;
        }
        Expected::Nothing
    }

    fn abandon(&mut self, reason: Reason) -> Expected {
        let published = self.item.take().is_some_and(|item| item.published);
        self.generation += 1;
        Expected::Abandon(reason, published)
    }

    fn moved_on(&mut self, answer: bool) -> Expected {
        let Some(item) = self.item.as_mut() else {
            return Expected::Nothing;
        };
        if !answer {
            return Expected::Nothing;
        }
        if item.bound.take().is_some() {
            return Expected::Publish(Some(Reason::MovedOn));
        }
        self.abandon(Reason::MovedOn)
    }

    fn published(&mut self, got: bool, then_abandon: Option<Reason>, now: u64) -> Expected {
        let Some(item) = self.item.as_mut() else {
            return Expected::Nothing;
        };
        item.published |= got;
        match then_abandon {
            Some(reason) => self.abandon(reason),
            None => self.judge(now),
        }
    }
}

/// One generated run: the record, the model, the time, the answers to
/// draw, and every item that has left, by id.
struct Run {
    in_flight: InFlight<u32>,
    model: Model,
    now: u64,
    answers: Vec<bool>,
    drawn: usize,
    items: u32,
    left: Vec<u32>,
    trail: Vec<String>,
}

impl Run {
    fn answer(&mut self) -> bool {
        let answer = self.answers[self.drawn % self.answers.len()];
        self.drawn += 1;
        answer
    }

    /// Compares `decision` with `expected`, and answers each ask at once
    /// with a drawn value, until nothing more is asked.
    fn settle(&mut self, mut decision: Decision<u32>, mut expected: Expected) {
        loop {
            match decision {
                Decision::Nothing => {
                    assert_eq!(expected, Expected::Nothing, "{:?}", self.trail);
                    return;
                }
                Decision::Abandon(abandoned) => {
                    assert_eq!(
                        expected,
                        Expected::Abandon(abandoned.reason, abandoned.published),
                        "{:?}",
                        self.trail
                    );
                    assert_eq!(abandoned.replacement, abandoned.dead + 1);
                    assert_eq!(abandoned.replacement, self.in_flight.generation());
                    self.left.push(abandoned.item);
                    return;
                }
                Decision::Ask(Ask::MovedOn { request, window }) => {
                    assert_eq!(expected, Expected::MovedOn, "{:?}", self.trail);
                    let item = self.model.item.as_ref().expect("an item asked about");
                    assert_eq!((request, window), (item.request, item.window));
                    let answer = self.answer();
                    self.trail.push(format!("moved on {answer}"));
                    decision = self.in_flight.moved_on(request, answer);
                    expected = self.model.moved_on(answer);
                }
                Decision::Ask(Ask::Publish {
                    request,
                    then_abandon,
                }) => {
                    assert_eq!(
                        expected,
                        Expected::Publish(then_abandon),
                        "{:?}",
                        self.trail
                    );
                    let item = self.model.item.as_ref().expect("an item published");
                    assert_eq!(request, item.request);
                    assert!(item.ever_armed, "published only after armed");
                    let got = self.answer();
                    self.trail.push(format!("published {got}"));
                    decision = self
                        .in_flight
                        .published(request, got, then_abandon, self.now);
                    expected = self.model.published(got, then_abandon, self.now);
                }
            }
        }
    }

    fn step(&mut self, action: Action) {
        self.trail.push(format!("{action:?} at {}", self.now));
        let generation = self.in_flight.generation();
        let running = self.model.item.as_ref().map(|item| item.request);
        match action {
            Action::Start { window, deadline } => {
                if running.is_some() {
                    return;
                }
                self.items += 1;
                let window = [None, Some(0), Some(WINDOW)][usize::from(window)];
                let deadline = DEADLINES[usize::from(deadline)];
                let request = self.in_flight.start(self.items, deadline, window, self.now);
                self.model.item = Some(ModelItem {
                    id: self.items,
                    request,
                    deadline: self.now + deadline,
                    never: window.is_none(),
                    window: window.unwrap_or(0),
                    next: self.now + GRACE,
                    started: self.now,
                    bound: None,
                    published: false,
                    ever_armed: false,
                });
            }
            Action::Learned => {
                let Some(request) = running else {
                    return;
                };
                self.learned(request);
            }
            Action::Armed(offset) => {
                let Some(request) = running else {
                    return;
                };
                let bound = (self.now + u64::from(offset) * MS).saturating_sub(200 * MS);
                self.in_flight.armed(request, bound);
                let item = self.model.item.as_mut().expect("running");
                if !item.published {
                    item.bound = Some(bound);
                    item.ever_armed = true;
                }
            }
            Action::Advance(ms) => {
                self.now += u64::from(ms) * MS;
                let decision = self.in_flight.tick(self.now);
                let expected = self.model.judge(self.now);
                self.settle(decision, expected);
                assert!(
                    self.in_flight.next_wake().is_none_or(|due| due > self.now),
                    "a wake left due after a tick: {:?}",
                    self.trail
                );
            }
            Action::WalkReturned(past) => {
                let Some(request) = running else {
                    return;
                };
                let decision = self.in_flight.walk_returned(request, past);
                let item = self.model.item.as_mut().expect("running");
                let expected = if item.bound.take().is_some() && past {
                    Expected::Publish(None)
                } else {
                    Expected::Nothing
                };
                self.settle(decision, expected);
            }
            Action::End => {
                let Some(request) = running else {
                    return;
                };
                let item = self.model.item.take().expect("running");
                assert_eq!(
                    self.in_flight.end(request),
                    Some((item.id, item.published)),
                    "{:?}",
                    self.trail
                );
                self.left.push(item.id);
                assert_eq!(self.in_flight.end(request), None, "the item leaves once");
            }
            Action::Succeed => {
                if running.is_some() {
                    return;
                }
                self.model.generation += 1;
                assert_eq!(self.in_flight.succeed(), self.model.generation);
            }
        }
        self.check(generation, running);
    }

    /// The item `request` learns its window.
    fn learned(&mut self, request: Request) {
        let decision = self.in_flight.learned(request, WINDOW, self.now);
        let item = self.model.item.as_mut().expect("running");
        let expected = if !item.never && item.window == 0 {
            item.window = WINDOW;
            item.next = (item.started + GRACE).max(self.now);
            if self.now >= item.started + GRACE {
                self.model.judge(self.now)
            } else {
                Expected::Nothing
            }
        } else {
            Expected::Nothing
        };
        self.settle(decision, expected);
    }

    /// The invariants after every step.
    fn check(&self, generation: u64, before: Option<Request>) {
        let trail = &self.trail;
        assert_eq!(
            self.in_flight.generation(),
            self.model.generation,
            "{trail:?}"
        );
        assert!(
            self.in_flight.generation() >= generation,
            "the generation never falls"
        );
        let mut left = self.left.clone();
        left.sort_unstable();
        left.dedup();
        assert_eq!(left.len(), self.left.len(), "an item left twice: {trail:?}");
        let running = u32::from(self.model.item.is_some());
        assert_eq!(
            u32::try_from(self.left.len()).expect("few items") + running,
            self.items,
            "every item but the running one has left once: {trail:?}"
        );
        match (&self.model.item, self.in_flight.running()) {
            (None, None) => {}
            (Some(item), Some((request, id, published))) => {
                assert_eq!(
                    (request, *id, published),
                    (item.request, item.id, item.published)
                );
                assert_eq!(
                    self.in_flight.accepts(request),
                    !item.published,
                    "{trail:?}"
                );
                if item.published {
                    assert!(item.ever_armed, "published only after armed");
                }
            }
            (model, record) => panic!("{model:?} against {record:?}: {trail:?}"),
        }
        if let Some(before) = before
            && self
                .model
                .item
                .as_ref()
                .is_none_or(|item| item.request != before)
        {
            assert!(
                !self.in_flight.accepts(before),
                "a request that left is refused"
            );
        }
        if let Some(due) = self.in_flight.next_wake()
            && due <= self.now
        {
            let mut ticked = self.in_flight.clone();
            assert_ne!(
                ticked.tick(self.now),
                Decision::Nothing,
                "a wake due that a tick does not act on: {trail:?}"
            );
        }
    }
}

/// The generated test's configuration: a fixed seed, no failure
/// persistence, no fork and no timeout, as `verbatim-term-sim`'s.
fn config() -> Config {
    Config {
        cases: 2_000,
        max_local_rejects: 65_536,
        max_global_rejects: 1024,
        max_flat_map_regens: 1_000_000,
        failure_persistence: None,
        source_file: None,
        test_name: None,
        fork: false,
        timeout: 0,
        max_shrink_time: 0,
        max_shrink_iters: u32::MAX,
        max_default_size_range: 100,
        result_cache: proptest::test_runner::noop_result_cache,
        verbose: 0,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(20_261_101),
        _non_exhaustive: (),
    }
}

/// Generated sequences of starts, learned windows, armed walks, ticks as
/// time advances, returned walks and ends, each ask answered at once by a
/// drawn value, behave as the reference model says, with every invariant
/// holding after every step.
#[test]
fn generated_sequences_match_the_reference_model() {
    use proptest::collection::vec;
    use proptest::prelude::any;
    let mut runner = TestRunner::new(config());
    let strategy = (vec(action(), 1..60), vec(any::<bool>(), 1..16));
    for _ in 0..config().cases {
        let (actions, answers) = strategy
            .new_tree(&mut runner)
            .expect("a sequence")
            .current();
        let mut run = Run {
            in_flight: InFlight::new(),
            model: Model::default(),
            now: 0,
            answers,
            drawn: 0,
            items: 0,
            left: Vec::new(),
            trail: Vec::new(),
        };
        for action in actions {
            run.step(action);
        }
    }
}
