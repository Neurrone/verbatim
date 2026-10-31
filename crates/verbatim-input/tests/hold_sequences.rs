//! Every short sequence of keys, pass-ons and permit changes through the
//! decision machine, checked against a reference model of what the
//! application should receive (`docs/design/logic-foundations.md`, "The
//! hook's hold decision", "Tests").
//!
//! The alphabet is eleven steps: Right Arrow's press (an auto-repeat when
//! it is already down) and release, A's, Shift's and the Verbatim
//! modifier's (Insert's) press and release, T's press, Core's pass-on of
//! the oldest place in line, and the permit moved to its next value.
//! Verbatim+Right Arrow and Verbatim+T are bound, and Right Arrow and
//! Shift+Right Arrow held, so a caret key is also a gesture's key. Steps
//! are 33 ms apart, inside the multi-press timeout. A release is offered
//! only for a key down, and a pass-on only while a place waits. The permit
//! never falls to `None` while a deferred press waits in line: its release
//! would then pass at once, ahead of the press, the known limitation of
//! "The hook's hold decision". After
//! every step the machine's verdict and raised gesture must equal the
//! model's, and after every sequence, every key let go and every place
//! passed on, the application must have every key up, having received no
//! release before its press.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use verbatim_input::{
    DecisionConfig, DecisionMachine, GestureMap, HoldPermit, KeyEvent, KeyInput, KeySource, Verdict,
};
use verbatim_model::GestureId;

/// The keys of the alphabet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum K {
    Right,
    A,
    Shift,
    Insert,
    T,
}

impl K {
    /// The key's virtual-key code and extended flag.
    fn code(self) -> (u16, bool) {
        match self {
            K::Right => (0x27, true),
            K::A => (0x41, false),
            K::Shift => (0xA0, false),
            K::Insert => (0x2D, true),
            K::T => (0x54, false),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Press(K),
    Release(K),
    PassOn,
    NextPermit,
}

const STEPS: [Step; 11] = [
    Step::Press(K::Right),
    Step::Release(K::Right),
    Step::Press(K::A),
    Step::Release(K::A),
    Step::Press(K::Shift),
    Step::Release(K::Shift),
    Step::Press(K::Insert),
    Step::Release(K::Insert),
    Step::Press(K::T),
    Step::PassOn,
    Step::NextPermit,
];

/// The time between steps, in microseconds.
const STEP_US: u64 = 33_000;
/// The multi-press timeout, NVDA's default, in microseconds.
const TIMEOUT_US: u64 = 500_000;

/// The reference model's record of a lone Verbatim-modifier tap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lone {
    Idle,
    Alone,
    Tapped(u64),
    Passing,
}

/// The model's record of a key down: whether a press of it has reached the
/// application since it went down or was last held, and whether one was
/// swallowed for a gesture.
#[derive(Clone, Copy, Debug, Default)]
struct Down {
    reached: bool,
    swallowed: bool,
}

/// The reference model: the rules of `docs/parity/input.md` and the hold
/// decision for this alphabet's five keys, written as plainly as they read.
#[derive(Clone, Debug)]
struct Model {
    down: BTreeMap<K, Down>,
    insert_modifier: bool,
    shift: bool,
    lone: Lone,
    last: Option<(&'static str, u64)>,
    count: u8,
    line: VecDeque<Option<(K, bool)>>,
    place: u32,
}

impl Model {
    fn new() -> Self {
        Self {
            down: BTreeMap::new(),
            insert_modifier: false,
            shift: false,
            lone: Lone::Idle,
            last: None,
            count: 0,
            line: VecDeque::new(),
            place: 0,
        }
    }

    fn queue(&mut self, transition: Option<(K, bool)>) -> u32 {
        self.place += 1;
        self.line.push_back(transition);
        self.place
    }

    /// A transition that would reach the application: deferred while a
    /// place waits and the permit lets it be; passed otherwise.
    fn reach(&mut self, key: K, pressed: bool, permit: HoldPermit) -> Verdict {
        if permit != HoldPermit::None && !self.line.is_empty() {
            Verdict::Defer(self.queue(Some((key, pressed))))
        } else {
            Verdict::Pass
        }
    }

    fn press(&mut self, key: K, now: u64, permit: HoldPermit) -> (Verdict, Option<(String, u8)>) {
        let hands = match self.lone {
            Lone::Passing => true,
            Lone::Tapped(at) => key == K::Insert && now - at < TIMEOUT_US,
            Lone::Idle | Lone::Alone => false,
        };
        let modifier = key == K::Insert && !hands;
        self.lone = if hands {
            Lone::Passing
        } else if modifier {
            Lone::Alone
        } else {
            Lone::Idle
        };
        if modifier {
            self.insert_modifier = true;
        }
        if key == K::Shift {
            self.shift = true;
        }
        let swallowed_before = self.down.get(&key).is_some_and(|down| down.swallowed);
        self.down.entry(key).or_default();
        if matches!(key, K::Right | K::T) && self.insert_modifier && !self.shift {
            let name = if key == K::Right {
                "kb:rightarrow+verbatim"
            } else {
                "kb:t+verbatim"
            };
            let repeat = if swallowed_before {
                self.count
            } else {
                self.count = match self.last {
                    Some((last, at)) if last == name && now - at < TIMEOUT_US => {
                        self.count.saturating_add(1)
                    }
                    _ => 0,
                };
                self.last = Some((name, now));
                self.count
            };
            self.down.get_mut(&key).expect("down").swallowed = true;
            return (Verdict::Swallow, Some((name.to_owned(), repeat)));
        }
        if !modifier && key != K::Shift {
            self.last = None;
        }
        if modifier {
            self.down.get_mut(&key).expect("down").swallowed = true;
            return (Verdict::Swallow, None);
        }
        let held = key == K::Right && !self.insert_modifier;
        if held && permit == HoldPermit::HoldAndDefer {
            self.down.get_mut(&key).expect("down").reached = false;
            return (Verdict::Hold(self.queue(None)), None);
        }
        self.down.get_mut(&key).expect("down").reached = true;
        (self.reach(key, true, permit), None)
    }

    fn release(&mut self, key: K, now: u64, permit: HoldPermit) -> Verdict {
        self.lone = match self.lone {
            Lone::Passing => Lone::Idle,
            Lone::Alone | Lone::Tapped(_) if key == K::Insert => Lone::Tapped(now),
            lone => lone,
        };
        if key == K::Insert {
            self.insert_modifier = false;
        }
        if key == K::Shift {
            self.shift = false;
        }
        match self.down.remove(&key) {
            Some(down) if !down.reached => Verdict::Swallow,
            _ => self.reach(key, false, permit),
        }
    }
}

/// What a place in line sends the application when passed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sent {
    /// A held press, sent with its own release.
    Held(K),
    /// A deferred transition.
    Transition(K, bool),
}

/// The machine, the model, and what the application has received.
#[derive(Clone)]
struct World {
    machine: DecisionMachine,
    model: Model,
    now: u64,
    permit: HoldPermit,
    user_down: BTreeSet<K>,
    line: VecDeque<(u32, Sent)>,
    app_down: BTreeSet<K>,
    places: BTreeSet<u32>,
    trail: Vec<Step>,
}

impl World {
    fn new() -> Self {
        let id = |raw| GestureId::parse(raw).expect("valid id");
        let map = GestureMap::new([id("kb:rightarrow+verbatim"), id("kb:t+verbatim")])
            .with_held([id("kb:rightarrow"), id("kb:shift+rightarrow")]);
        Self {
            machine: DecisionMachine::new(DecisionConfig::default(), map),
            model: Model::new(),
            now: 1_000_000_000,
            permit: HoldPermit::None,
            user_down: BTreeSet::new(),
            line: VecDeque::new(),
            app_down: BTreeSet::new(),
            places: BTreeSet::new(),
            trail: Vec::new(),
        }
    }

    fn allows(&self, step: Step) -> bool {
        match step {
            Step::Release(key) => self.user_down.contains(&key),
            Step::PassOn => !self.line.is_empty(),
            Step::NextPermit => {
                self.permit != HoldPermit::Defer
                    || !self
                        .line
                        .iter()
                        .any(|&(_, sent)| matches!(sent, Sent::Transition(_, true)))
            }
            Step::Press(_) => true,
        }
    }

    /// The application receives `key`'s transition.
    fn app(&mut self, key: K, pressed: bool) -> Result<(), String> {
        if pressed {
            self.app_down.insert(key);
        } else if !self.app_down.remove(&key) {
            return Err(format!(
                "{key:?}'s release reached the application before its press"
            ));
        }
        Ok(())
    }

    fn push(&mut self, place: u32, sent: Sent) -> Result<(), String> {
        if !self.places.insert(place) {
            return Err(format!("place {place} entered the line twice"));
        }
        self.line.push_back((place, sent));
        Ok(())
    }

    /// Takes `step`, returning the verdict of a key's transition, and
    /// checks it against the model and the line.
    fn apply(&mut self, step: Step) -> Result<Option<Verdict>, String> {
        self.trail.push(step);
        let mut verdict = None;
        match step {
            Step::Press(key) | Step::Release(key) => {
                let pressed = matches!(step, Step::Press(_));
                self.now += STEP_US;
                let (vk, extended) = key.code();
                let decision = self.machine.on_key(KeyInput {
                    event: KeyEvent {
                        vk,
                        scan_code: 0,
                        extended,
                        injected: false,
                        pressed,
                    },
                    now: self.now,
                    num_lock: false,
                    source: KeySource::User,
                    permit: self.permit,
                });
                let raised = decision
                    .emitted
                    .map(|raised| (raised.gesture.as_str().to_owned(), raised.repeat));
                let expected = if pressed {
                    self.model.press(key, self.now, self.permit)
                } else {
                    (self.model.release(key, self.now, self.permit), None)
                };
                if (decision.verdict, raised.clone()) != expected {
                    return Err(format!(
                        "{key:?} {}: the machine {:?} {raised:?}, the model {expected:?}",
                        if pressed { "down" } else { "up" },
                        decision.verdict
                    ));
                }
                if pressed {
                    self.user_down.insert(key);
                } else {
                    self.user_down.remove(&key);
                }
                match decision.verdict {
                    Verdict::Pass => self.app(key, pressed)?,
                    Verdict::Swallow => {}
                    Verdict::Hold(place) => self.push(place, Sent::Held(key))?,
                    Verdict::Defer(place) => self.push(place, Sent::Transition(key, pressed))?,
                }
                verdict = Some(decision.verdict);
            }
            Step::PassOn => {
                self.now += STEP_US;
                let (place, sent) = self.line.pop_front().expect("a place in line");
                self.machine.passed_on(place);
                self.model.line.pop_front();
                match sent {
                    Sent::Held(key) => {
                        self.app(key, true)?;
                        self.app(key, false)?;
                    }
                    Sent::Transition(key, pressed) => self.app(key, pressed)?,
                }
            }
            Step::NextPermit => {
                self.permit = match self.permit {
                    HoldPermit::None => HoldPermit::HoldAndDefer,
                    HoldPermit::HoldAndDefer => HoldPermit::Defer,
                    HoldPermit::Defer => HoldPermit::None,
                };
            }
        }
        if self.machine.has_outstanding() == self.line.is_empty() {
            return Err("the machine's line differs from the places given out".to_owned());
        }
        Ok(verdict)
    }

    /// Lets every key go and passes every place on, then checks that the
    /// application has every key up.
    fn finish(mut self) -> Result<(), String> {
        for key in self.user_down.clone() {
            self.apply(Step::Release(key))?;
        }
        while !self.line.is_empty() {
            self.apply(Step::PassOn)?;
        }
        if self.app_down.is_empty() {
            Ok(())
        } else {
            Err(format!("left down in the application: {:?}", self.app_down))
        }
    }
}

/// Walks every sequence of up to `depth` steps after `world`, collecting
/// failures, and returns the number of sequences walked.
fn walk(world: &World, depth: usize, failures: &mut Vec<String>) -> u64 {
    let mut walked = 0;
    for step in STEPS {
        if !world.allows(step) {
            continue;
        }
        let mut next = world.clone();
        if let Err(error) = next.apply(step) {
            failures.push(format!("{:?}: {error}", next.trail));
            continue;
        }
        walked += 1;
        if let Err(error) = next.clone().finish() {
            failures.push(format!("{:?}, then let go: {error}", next.trail));
        }
        if depth > 1 {
            walked += walk(&next, depth - 1, failures);
        }
    }
    walked
}

/// The verdicts of the key transitions in `steps`, from a fresh world,
/// with the sequence checked as the walk checks it.
fn verdicts(steps: &[Step]) -> Vec<Verdict> {
    let mut world = World::new();
    let verdicts = steps
        .iter()
        .filter_map(|&step| world.apply(step).expect("the sequence holds"))
        .collect();
    world.finish().expect("every key ends up");
    verdicts
}

#[test]
fn every_sequence_of_up_to_five_steps_matches_the_model() {
    use Step::{NextPermit, Press, Release};
    // A press passed, then held once the focus has text: the held press is
    // sent with its own release, so the user's release is swallowed.
    assert_eq!(
        verdicts(&[
            Press(K::Right),
            NextPermit,
            Press(K::Right),
            Release(K::Right)
        ]),
        [Verdict::Pass, Verdict::Hold(1), Verdict::Swallow]
    );
    // A press passed, then the Verbatim modifier down, and the key's
    // auto-repeat completing a gesture: the release is delivered.
    assert_eq!(
        verdicts(&[
            Press(K::Right),
            Press(K::Insert),
            Press(K::Right),
            Release(K::Right)
        ]),
        [
            Verdict::Pass,
            Verdict::Swallow,
            Verdict::Swallow,
            Verdict::Pass
        ]
    );
    let mut failures = Vec::new();
    let walked = walk(&World::new(), 5, &mut failures);
    assert_eq!(failures, Vec::<String>::new());
    assert_eq!(walked, WALKED);
}

/// The number of sequences the walk takes, so that a change to the
/// alphabet or its pruning is seen.
const WALKED: u64 = 18_929;
