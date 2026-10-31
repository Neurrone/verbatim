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
const WINDOW: Window = match Window::new(0x10) {
    Some(window) => window,
    None => panic!("0x10 is not 0"),
};

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
    let request = late.start(7, 2_000 * MS, RuleWindow::Known(WINDOW), 0);
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
    let request = early.start(7, 2_000 * MS, RuleWindow::Known(WINDOW), 0);
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
    let request = deadline.start(7, 400 * MS, RuleWindow::Never, 0);
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
    let request = moved.start(7, 10_000 * MS, RuleWindow::Known(WINDOW), 0);
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
    let request = known.start(7, 10_000 * MS, RuleWindow::Known(WINDOW), 0);
    assert_eq!(known.next_wake(), Some(GRACE));
    assert_eq!(known.tick(GRACE - 1), Decision::Nothing);
    assert_eq!(known.tick(GRACE), moved_on(request));
    assert_eq!(known.moved_on(request, false), Decision::Nothing);
    assert_eq!(known.next_wake(), Some(2 * GRACE));
    assert_eq!(known.tick(2 * GRACE - 1), Decision::Nothing);
    assert_eq!(known.tick(2 * GRACE), moved_on(request));

    let mut learned = InFlight::new();
    let request = learned.start(7, 10_000 * MS, RuleWindow::Unknown, 0);
    assert_eq!(learned.next_wake(), Some(10_000 * MS), "no window to judge");
    assert_eq!(learned.tick(600 * MS), Decision::Nothing);
    assert_eq!(
        learned.learned(request, WINDOW, 700 * MS),
        moved_on(request)
    );
    assert_eq!(learned.next_wake(), Some(700 * MS + GRACE));

    let mut early = InFlight::new();
    let request = early.start(7, 10_000 * MS, RuleWindow::Unknown, 0);
    assert_eq!(early.learned(request, WINDOW, 100 * MS), Decision::Nothing);
    assert_eq!(early.next_wake(), Some(GRACE), "judged at the grace's end");
}

/// A published item still stuck is abandoned at once when the user has
/// moved on.
#[test]
fn a_published_item_is_abandoned_at_once_when_the_user_moves_on() {
    let mut in_flight = InFlight::new();
    let request = in_flight.start(7, 8_000 * MS, RuleWindow::Known(WINDOW), 0);
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
    let request = in_flight.start(7, 8_000 * MS, RuleWindow::Known(WINDOW), 0);
    assert_eq!(in_flight.end(request), Some((7, false)));
    let next = in_flight.start(8, 8_000 * MS, RuleWindow::Known(WINDOW), 0);
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
    let request = in_flight.start(7, 8_000 * MS, RuleWindow::Never, 0);
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
    let request = in_flight.start(7, 2_000 * MS, RuleWindow::Known(WINDOW), 0);
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
    let request = in_flight.start(7, 2_000 * MS, RuleWindow::Never, 0);
    assert_eq!(in_flight.end(request), Some((7, false)));
    assert_eq!(in_flight.succeed(), 1);
    let next = in_flight.start(8, 2_000 * MS, RuleWindow::Never, 0);
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
    /// The item is handed on to a new request.
    HandOn,
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
        Just(Action::HandOn),
        Just(Action::End),
        Just(Action::Succeed),
    ]
}

/// The readers' deadlines: an outpost's step, text and handling deadlines,
/// and the listener's.
const DEADLINES: [u64; 4] = [400 * MS, 2_000 * MS, 10_000 * MS, 8_000 * MS];

/// The answers a run gives, each drawn by the time it is given at, so the
/// reference model knows them from the times alone.
struct Answers(Vec<bool>);

impl Answers {
    /// Whether the user has moved on, asked at `now`.
    fn moved_on(&self, now: u64) -> bool {
        self.at(now, 0)
    }

    /// Whether the walk was published, asked at `now`.
    fn published(&self, now: u64) -> bool {
        self.at(now, 1)
    }

    fn at(&self, now: u64, salt: u64) -> bool {
        let index = (now / MS + salt) % u64::try_from(self.0.len()).expect("a few answers");
        self.0[usize::try_from(index).expect("a small index")]
    }
}

/// What happens to one item, as its timeline records it.
#[derive(Clone, Copy, Debug)]
enum Event {
    /// It learns its window.
    Learned,
    /// Its walk is armed, with this bound.
    Armed(u64),
    /// Its walk returns, past its bound or not.
    Returned(bool),
    /// The reader ticks.
    Tick,
    /// It ends.
    End,
}

/// The fate of one item: still running, ended, or abandoned for a reason,
/// each with whether its walk was published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fate {
    Running(bool),
    Ended(bool),
    Abandoned(Reason, bool),
}

/// The reference model: one item's timeline, from its start, scanned for
/// its fate.
#[derive(Clone, Debug)]
struct Timeline {
    id: u32,
    start: u64,
    deadline: u64,
    window: RuleWindow,
    events: Vec<(u64, Event)>,
}

impl Timeline {
    /// The item's fate after its events so far, with every answer drawn by
    /// its time. Its walk is published at a judgment past its bound or at
    /// a return past it, or before an abandonment while armed, when the
    /// answer then says so. It is abandoned at the first judgment at or
    /// past its deadline, or when the user is found to have moved on at a
    /// judgment of its window: the grace after its start, or after the
    /// window is learned, and each grace after the last. A judgment is a
    /// tick, or a window learned past the grace.
    fn fate(&self, answers: &Answers) -> Fate {
        let grace_end = self.start + GRACE;
        let mut next = self.window.known().map(|_| grace_end);
        let mut bound = None;
        let mut published = false;
        for &(now, event) in &self.events {
            let judged = match event {
                Event::Learned => {
                    if self.window == RuleWindow::Unknown && next.is_none() {
                        next = Some(grace_end.max(now));
                        now >= grace_end
                    } else {
                        false
                    }
                }
                Event::Armed(at) => {
                    if !published {
                        bound = Some(at);
                    }
                    false
                }
                Event::Returned(past) => {
                    if bound.take().is_some() && past {
                        published |= answers.published(now);
                    }
                    false
                }
                Event::Tick => true,
                Event::End => return Fate::Ended(published),
            };
            if !judged {
                continue;
            }
            let mut publish = |bound: &mut Option<u64>| {
                if bound.take().is_some() {
                    published |= answers.published(now);
                }
            };
            if now >= self.start + self.deadline {
                publish(&mut bound);
                return Fate::Abandoned(Reason::Deadline, published);
            }
            if bound.is_some_and(|at| now >= at) {
                publish(&mut bound);
            }
            if let Some(at) = next
                && now >= at
            {
                next = Some(now + GRACE);
                if answers.moved_on(now) {
                    publish(&mut bound);
                    return Fate::Abandoned(Reason::MovedOn, published);
                }
            }
        }
        Fate::Running(published)
    }
}

/// One generated run: the record, the time, the answers, the item running
/// and its request, the fate of one that left in the step, and every item
/// that has left, by id.
struct Run {
    in_flight: InFlight<u32>,
    now: u64,
    answers: Answers,
    items: u32,
    item: Option<(Timeline, Request)>,
    left_as: Option<(u32, Fate)>,
    left: Vec<u32>,
    succeeded: u64,
    abandonments: u64,
    trail: Vec<String>,
}

impl Run {
    /// Answers each ask at once, by the time, until nothing more is asked.
    fn settle(&mut self, mut decision: Decision<u32>) {
        loop {
            let (_, request) = self.item.as_ref().expect("an item decided on");
            let request = *request;
            match decision {
                Decision::Nothing => return,
                Decision::Abandon(abandoned) => {
                    assert_eq!(abandoned.replacement, abandoned.dead + 1);
                    assert_eq!(abandoned.replacement, self.in_flight.generation());
                    self.abandonments += 1;
                    self.left_as = Some((
                        abandoned.item,
                        Fate::Abandoned(abandoned.reason, abandoned.published),
                    ));
                    return;
                }
                Decision::Ask(Ask::MovedOn {
                    request: asked,
                    window,
                }) => {
                    assert_eq!((asked, window), (request, WINDOW), "{:?}", self.trail);
                    let answer = self.answers.moved_on(self.now);
                    decision = self.in_flight.moved_on(request, answer);
                }
                Decision::Ask(Ask::Publish {
                    request: asked,
                    then_abandon,
                }) => {
                    assert_eq!(asked, request, "{:?}", self.trail);
                    let got = self.answers.published(self.now);
                    decision = self
                        .in_flight
                        .published(request, got, then_abandon, self.now);
                }
            }
        }
    }

    /// Records `event` in the running item's timeline, now.
    fn record(&mut self, event: Event) {
        if let Some((timeline, _)) = self.item.as_mut() {
            timeline.events.push((self.now, event));
        }
    }

    fn step(&mut self, action: Action) {
        self.trail.push(format!("{action:?} at {}", self.now));
        let generation = self.in_flight.generation();
        let running = self.item.as_ref().map(|(_, request)| *request);
        match action {
            Action::Start { window, deadline } => {
                if running.is_some() {
                    return;
                }
                self.items += 1;
                let window = [
                    RuleWindow::Never,
                    RuleWindow::Unknown,
                    RuleWindow::Known(WINDOW),
                ][usize::from(window)];
                let deadline = DEADLINES[usize::from(deadline)];
                let request = self.in_flight.start(self.items, deadline, window, self.now);
                let timeline = Timeline {
                    id: self.items,
                    start: self.now,
                    deadline,
                    window,
                    events: Vec::new(),
                };
                self.item = Some((timeline, request));
            }
            Action::Learned => {
                let Some(request) = running else {
                    return;
                };
                self.record(Event::Learned);
                let decision = self.in_flight.learned(request, WINDOW, self.now);
                self.settle(decision);
            }
            Action::Armed(offset) => {
                let Some(request) = running else {
                    return;
                };
                let bound = (self.now + u64::from(offset) * MS).saturating_sub(200 * MS);
                self.record(Event::Armed(bound));
                self.in_flight.armed(request, bound);
            }
            Action::Advance(ms) => {
                self.now += u64::from(ms) * MS;
                let decision = self.in_flight.tick(self.now);
                if running.is_some() {
                    self.record(Event::Tick);
                    self.settle(decision);
                } else {
                    assert_eq!(decision, Decision::Nothing);
                }
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
                self.record(Event::Returned(past));
                let decision = self.in_flight.walk_returned(request, past);
                self.settle(decision);
            }
            Action::HandOn => {
                let handed = self.in_flight.hand_on();
                let Some((_, request)) = self.item.as_mut() else {
                    assert_eq!(handed, None);
                    return;
                };
                let handed = handed.expect("an item running is handed on");
                assert_eq!(handed.generation, request.generation);
                assert!(handed.number > request.number);
                *request = handed;
            }
            Action::End => {
                let Some(request) = running else {
                    return;
                };
                self.record(Event::End);
                let ended = self.in_flight.end(request).expect("the item running ends");
                assert_eq!(self.in_flight.end(request), None, "the item leaves once");
                self.left_as = Some((ended.0, Fate::Ended(ended.1)));
            }
            Action::Succeed => {
                if running.is_some() {
                    return;
                }
                self.succeeded += 1;
                assert_eq!(self.in_flight.succeed(), generation + 1);
            }
        }
        self.check(generation, running);
    }

    /// The item's fate, and the invariants, after every step.
    fn check(&mut self, generation: u64, before: Option<Request>) {
        let trail = self.trail.clone();
        if let Some((timeline, request)) = &self.item {
            let fate = timeline.fate(&self.answers);
            if let Some((id, left_as)) = self.left_as.take() {
                assert_eq!((id, left_as), (timeline.id, fate), "{trail:?}");
                self.left.push(id);
                self.item = None;
            } else {
                let Fate::Running(published) = fate else {
                    panic!("{fate:?} of an item still running: {trail:?}");
                };
                assert_eq!(
                    self.in_flight
                        .running()
                        .map(|(request, id, published)| (request, *id, published)),
                    Some((*request, timeline.id, published)),
                    "{trail:?}"
                );
                assert_eq!(self.in_flight.accepts(*request), !published);
                if published {
                    assert!(
                        timeline
                            .events
                            .iter()
                            .any(|(_, event)| matches!(event, Event::Armed(_))),
                        "published only after armed"
                    );
                }
            }
        }
        if self.item.is_none() {
            assert!(self.in_flight.running().is_none(), "{trail:?}");
        }
        assert_eq!(
            self.in_flight.generation(),
            self.succeeded + self.abandonments,
            "the generation rises by one per abandonment or succession: {trail:?}"
        );
        assert!(self.in_flight.generation() >= generation);
        let mut left = self.left.clone();
        left.sort_unstable();
        left.dedup();
        assert_eq!(left.len(), self.left.len(), "an item left twice: {trail:?}");
        assert_eq!(
            u32::try_from(self.left.len()).expect("few items") + u32::from(self.item.is_some()),
            self.items,
            "every item but the running one has left once: {trail:?}"
        );
        if let Some(before) = before
            && self
                .item
                .as_ref()
                .is_none_or(|(_, request)| *request != before)
        {
            assert!(
                !self.in_flight.accepts(before),
                "a request that left or was handed on is refused"
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
/// time advances, returned walks, hand-ons and ends, each ask answered at
/// once by a value drawn by its time, give each item the fate a scan of its
/// timeline names, with every invariant holding after every step.
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
            now: 0,
            answers: Answers(answers),
            items: 0,
            item: None,
            left_as: None,
            left: Vec::new(),
            succeeded: 0,
            abandonments: 0,
            trail: Vec::new(),
        };
        for action in actions {
            run.step(action);
        }
    }
}
