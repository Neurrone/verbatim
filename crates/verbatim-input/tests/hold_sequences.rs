//! Every sequence of up to four keys, pass-ons, permit changes and pauses
//! through the decision machine, from a view that lets keys be held, checked against a reference model of
//! what the application should receive (`docs/crates/verbatim-input.md`),
//! for each keyboard layout's bindings.
//!
//! The alphabet is twenty steps: the press and release of Right Arrow (a
//! press of a key already down is its auto-repeat), A, Shift, Alt,
//! Control, the left Windows key, Insert and Caps Lock, the last two both
//! Verbatim keys; T's press; Core's pass-on of the oldest place in line;
//! the permit moved to its next value; and a pause past the multi-press
//! timeout. The gesture map is the one the app builds for the layout (its
//! table, the menu gesture, the clearing keys observed and the caret keys
//! held), with Verbatim+Right Arrow, Verbatim+T, Verbatim+Alt+T,
//! Verbatim+Control+Shift+T and Verbatim+Windows+T bound as well, so a
//! caret key is also a gesture's key and every kind of mask is walked.
//! Steps are 33 ms apart, inside the multi-press timeout, unless a pause
//! comes between. A release is offered only for a key down, and a pass-on
//! only while a place waits. The permit never falls to `None` while a
//! deferred press or a mask waits in line: a release would then pass at
//! once, ahead of its press or of the mask, the known limitation of "The
//! hook's hold decision". The walk's `None` stands for a view that lets no
//! key be held, so the model expects no mask for a gesture made with it;
//! the hook's other `None`, a view that lets keys be held with no text and
//! nothing in line, decides as `Defer` does with an empty line, which the
//! walk covers. After every step the machine's verdict, raised gesture
//! and mask must equal the model's; a modifier down in the application
//! when a gesture was masked must not come up there before the mask; and
//! after every sequence, every key let go and every place passed on, the
//! application must have every key up, having received no release before
//! its press.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use verbatim_input::{
    DecisionConfig, DecisionMachine, GestureMap, HoldPermit, KeyEvent, KeyInput, KeySource,
    KeyboardLayout, Verdict, bindings_for, caret_bindings, clearing_keys,
};
use verbatim_model::GestureId;

/// The keys of the alphabet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum K {
    Right,
    A,
    Shift,
    Alt,
    Control,
    Windows,
    Insert,
    Caps,
    T,
}

impl K {
    /// The key's virtual-key code and extended flag.
    fn code(self) -> (u16, bool) {
        match self {
            K::Right => (0x27, true),
            K::A => (0x41, false),
            K::Shift => (0xA0, false),
            K::Alt => (0xA4, false),
            K::Control => (0xA2, false),
            K::Windows => (0x5B, true),
            K::Insert => (0x2D, true),
            K::Caps => (0x14, false),
            K::T => (0x54, false),
        }
    }

    /// The key's name as the main key of a gesture.
    fn name(self) -> &'static str {
        match self {
            K::Right => "rightarrow",
            K::A => "a",
            K::Shift => "leftshift",
            K::Alt => "leftalt",
            K::Control => "leftcontrol",
            K::Windows => "leftwindows",
            K::Insert => "insert",
            K::Caps => "capslock",
            K::T => "t",
        }
    }

    /// A normal modifier's name in a chord.
    fn modifier_name(self) -> Option<&'static str> {
        match self {
            K::Shift => Some("shift"),
            K::Alt => Some("alt"),
            K::Control => Some("control"),
            K::Windows => Some("leftwindows"),
            _ => None,
        }
    }

    /// Whether the key is a Verbatim key, as both are by default.
    fn verbatim(self) -> bool {
        matches!(self, K::Insert | K::Caps)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Press(K),
    Release(K),
    PassOn,
    NextPermit,
    Pause,
}

const STEPS: [Step; 20] = [
    Step::Press(K::Right),
    Step::Release(K::Right),
    Step::Press(K::A),
    Step::Release(K::A),
    Step::Press(K::Shift),
    Step::Release(K::Shift),
    Step::Press(K::Alt),
    Step::Release(K::Alt),
    Step::Press(K::Control),
    Step::Release(K::Control),
    Step::Press(K::Windows),
    Step::Release(K::Windows),
    Step::Press(K::Insert),
    Step::Release(K::Insert),
    Step::Press(K::Caps),
    Step::Release(K::Caps),
    Step::Press(K::T),
    Step::PassOn,
    Step::NextPermit,
    Step::Pause,
];

/// The time between steps, in microseconds.
const STEP_US: u64 = 33_000;
/// The multi-press timeout, NVDA's default, in microseconds.
const TIMEOUT_US: u64 = 500_000;

/// The reference model's record of a lone Verbatim-key tap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lone {
    Idle,
    Alone(K),
    Tapped(K, u64),
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

/// The gestures bound, observed and held, as the model reads them.
#[derive(Debug)]
struct Bindings {
    bound: BTreeSet<GestureId>,
    observed: BTreeSet<GestureId>,
    held: BTreeSet<GestureId>,
}

impl Bindings {
    /// The app's gesture map for `layout` with the walk's own bindings, and
    /// the same sets for the model.
    fn for_layout(layout: KeyboardLayout) -> (GestureMap, Self) {
        let id = |raw| GestureId::parse(raw).expect("valid id");
        let bound: BTreeSet<GestureId> = bindings_for(layout)
            .into_iter()
            .map(|(gesture, _)| gesture)
            .chain(
                [
                    "kb:verbatim+v",
                    "kb:verbatim+rightarrow",
                    "kb:verbatim+t",
                    "kb:verbatim+alt+t",
                    "kb:verbatim+control+shift+t",
                    "kb:verbatim+leftwindows+t",
                ]
                .map(id),
            )
            .collect();
        let held: BTreeSet<GestureId> = caret_bindings()
            .into_iter()
            .map(|(gesture, _)| gesture)
            .collect();
        let clearing = clearing_keys();
        let map = GestureMap::new(bound.iter().cloned())
            .with_observed(clearing.iter().cloned())
            .with_held(held.iter().cloned());
        let observed = clearing.into_iter().chain(held.iter().cloned()).collect();
        (
            map,
            Self {
                bound,
                observed,
                held,
            },
        )
    }
}

/// The reference model: the rules of `docs/parity/input.md`, the hold
/// decision and the menu guard for this alphabet's keys, written as
/// plainly as they read.
#[derive(Clone, Debug)]
struct Model {
    down: BTreeMap<K, Down>,
    /// The Verbatim keys down acting as the Verbatim key.
    verbatim_held: BTreeSet<K>,
    /// The normal modifiers down.
    modifiers_held: BTreeSet<K>,
    lone: Lone,
    last: Option<(GestureId, u64)>,
    count: u8,
    line: usize,
    place: u32,
}

impl Model {
    fn new() -> Self {
        Self {
            down: BTreeMap::new(),
            verbatim_held: BTreeSet::new(),
            modifiers_held: BTreeSet::new(),
            lone: Lone::Idle,
            last: None,
            count: 0,
            line: 0,
            place: 0,
        }
    }

    fn queue(&mut self) -> u32 {
        self.place += 1;
        self.line += 1;
        self.place
    }

    /// A transition that would reach the application: deferred while a
    /// place waits and the permit lets it be, unless it is Caps Lock, a
    /// lock key; passed otherwise.
    fn reach(&mut self, key: K, permit: HoldPermit) -> Verdict {
        if permit != HoldPermit::None && self.line > 0 && key != K::Caps {
            Verdict::Defer(self.queue())
        } else {
            Verdict::Pass
        }
    }

    /// The modifiers down whose press reached the application.
    fn reached_modifiers(&self) -> BTreeSet<K> {
        self.modifiers_held
            .iter()
            .copied()
            .filter(|key| self.down.get(key).is_some_and(|down| down.reached))
            .collect()
    }

    /// Whether the modifiers down in the application could act alone: Alt
    /// or the Windows key, or Control and Shift together.
    fn may_act_alone(&self) -> bool {
        let reached = self.reached_modifiers();
        reached.contains(&K::Alt)
            || reached.contains(&K::Windows)
            || (reached.contains(&K::Control) && reached.contains(&K::Shift))
    }

    fn press(&mut self, key: K, now: u64, permit: HoldPermit, map: &Bindings) -> Expected {
        let hands = match self.lone {
            Lone::Passing => true,
            Lone::Tapped(tapped, at) => key == tapped && now - at < TIMEOUT_US,
            Lone::Idle | Lone::Alone(_) => false,
        };
        let modifier = key.verbatim() && !hands;
        self.lone = if hands {
            Lone::Passing
        } else if modifier {
            Lone::Alone(key)
        } else {
            Lone::Idle
        };
        if modifier {
            self.verbatim_held.insert(key);
        }
        if key.modifier_name().is_some() {
            self.modifiers_held.insert(key);
        }
        let mut parts: Vec<&str> = self
            .verbatim_held
            .iter()
            .filter(|&&held| held != key)
            .map(|_| "verbatim")
            .chain(
                self.modifiers_held
                    .iter()
                    .filter(|&&held| held != key)
                    .filter_map(|held| held.modifier_name()),
            )
            .collect();
        parts.push(if modifier { "verbatim" } else { key.name() });
        let gesture = GestureId::parse(&format!("kb:{}", parts.join("+"))).expect("valid id");
        let swallowed_before = self.down.get(&key).is_some_and(|down| down.swallowed);
        self.down.entry(key).or_default();
        if map.bound.contains(&gesture) {
            let repeat = if swallowed_before {
                self.count
            } else {
                self.count = match &self.last {
                    Some((last, at)) if *last == gesture && now - at < TIMEOUT_US => {
                        self.count.saturating_add(1)
                    }
                    _ => 0,
                };
                self.last = Some((gesture.clone(), now));
                self.count
            };
            self.down.get_mut(&key).expect("down").swallowed = true;
            let mask = (permit != HoldPermit::None && self.may_act_alone())
                .then(|| (self.queue(), self.queue()));
            return (
                Verdict::Swallow,
                Some((gesture.as_str().to_owned(), repeat)),
                mask,
            );
        }
        if !modifier && key.modifier_name().is_none() {
            self.last = None;
        }
        if modifier {
            self.down.get_mut(&key).expect("down").swallowed = true;
            return (Verdict::Swallow, None, None);
        }
        let held = map.observed.contains(&gesture) && map.held.contains(&gesture);
        if held && permit == HoldPermit::HoldAndDefer {
            self.down.get_mut(&key).expect("down").reached = false;
            return (Verdict::Hold(self.queue()), None, None);
        }
        self.down.get_mut(&key).expect("down").reached = true;
        (self.reach(key, permit), None, None)
    }

    fn release(&mut self, key: K, now: u64, permit: HoldPermit) -> Verdict {
        self.lone = match self.lone {
            Lone::Passing => Lone::Idle,
            Lone::Alone(candidate) | Lone::Tapped(candidate, _) if candidate == key => {
                Lone::Tapped(key, now)
            }
            lone => lone,
        };
        self.verbatim_held.remove(&key);
        self.modifiers_held.remove(&key);
        match self.down.remove(&key) {
            Some(down) if !down.reached => Verdict::Swallow,
            _ => self.reach(key, permit),
        }
    }
}

/// What the model expects of a key transition: its verdict, the gesture
/// it raised with its count, and the places of its mask.
type Expected = (Verdict, Option<(String, u8)>, Option<(u32, u32)>);

/// What a place in line sends the application when passed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sent {
    /// A held press, sent with its own release.
    Held(K),
    /// A deferred transition.
    Transition(K, bool),
    /// A mask's press or release.
    Mask(bool),
}

/// The machine, the model, and what the application has received.
#[derive(Clone)]
struct World<'a> {
    bindings: &'a Bindings,
    machine: DecisionMachine,
    model: Model,
    now: u64,
    permit: HoldPermit,
    user_down: BTreeSet<K>,
    line: VecDeque<(u32, Sent)>,
    app_down: BTreeSet<K>,
    places: BTreeSet<u32>,
    /// The modifiers down in the application when a gesture was masked,
    /// whose release must not reach it before the mask does.
    mask_due: BTreeSet<K>,
    trail: Vec<Step>,
}

impl<'a> World<'a> {
    fn new(map: GestureMap, bindings: &'a Bindings, permit: HoldPermit) -> Self {
        Self {
            bindings,
            machine: DecisionMachine::new(DecisionConfig::default(), map),
            model: Model::new(),
            now: 1_000_000_000,
            permit,
            user_down: BTreeSet::new(),
            line: VecDeque::new(),
            app_down: BTreeSet::new(),
            places: BTreeSet::new(),
            mask_due: BTreeSet::new(),
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
                        .any(|&(_, sent)| matches!(sent, Sent::Transition(_, true) | Sent::Mask(_)))
            }
            Step::Press(_) | Step::Pause => true,
        }
    }

    /// The application receives `key`'s transition.
    fn app(&mut self, key: K, pressed: bool) -> Result<(), String> {
        if !pressed && self.mask_due.contains(&key) {
            return Err(format!(
                "{key:?}'s release reached the application before the mask of a gesture made with it down"
            ));
        }
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
                    may_mask: self.permit != HoldPermit::None,
                });
                let raised = decision
                    .emitted
                    .map(|raised| (raised.gesture.as_str().to_owned(), raised.repeat));
                let expected = if pressed {
                    self.model.press(key, self.now, self.permit, self.bindings)
                } else {
                    (self.model.release(key, self.now, self.permit), None, None)
                };
                if (decision.verdict, raised.clone(), decision.mask) != expected {
                    return Err(format!(
                        "{key:?} {}: the machine {:?} {raised:?} {:?}, the model {expected:?}",
                        if pressed { "down" } else { "up" },
                        decision.verdict,
                        decision.mask
                    ));
                }
                if let Some((press, release)) = decision.mask {
                    self.push(press, Sent::Mask(true))?;
                    self.push(release, Sent::Mask(false))?;
                    self.mask_due = self.model.reached_modifiers();
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
                self.model.line -= 1;
                match sent {
                    Sent::Held(key) => {
                        self.app(key, true)?;
                        self.app(key, false)?;
                    }
                    Sent::Transition(key, pressed) => self.app(key, pressed)?,
                    Sent::Mask(true) => self.mask_due.clear(),
                    Sent::Mask(false) => {}
                }
            }
            Step::NextPermit => {
                self.permit = match self.permit {
                    HoldPermit::None => HoldPermit::HoldAndDefer,
                    HoldPermit::HoldAndDefer => HoldPermit::Defer,
                    HoldPermit::Defer => HoldPermit::None,
                };
            }
            Step::Pause => self.now += TIMEOUT_US,
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
/// failures.
fn walk(world: &World, depth: usize, failures: &mut Vec<String>) {
    for step in STEPS {
        if !world.allows(step) {
            continue;
        }
        let mut next = world.clone();
        if let Err(error) = next.apply(step) {
            failures.push(format!("{:?}: {error}", next.trail));
            continue;
        }
        if let Err(error) = next.clone().finish() {
            failures.push(format!("{:?}, then let go: {error}", next.trail));
        }
        if depth > 1 {
            walk(&next, depth - 1, failures);
        }
    }
}

/// The verdicts of the key transitions in `steps`, from a fresh world with
/// the desktop layout, with the sequence checked as the walk checks it.
fn verdicts(steps: &[Step]) -> Vec<Verdict> {
    let (map, bindings) = Bindings::for_layout(KeyboardLayout::Desktop);
    let mut world = World::new(map, &bindings, HoldPermit::None);
    let verdicts = steps
        .iter()
        .filter_map(|&step| world.apply(step).expect("the sequence holds"))
        .collect();
    world.finish().expect("every key ends up");
    verdicts
}

#[test]
fn every_sequence_of_up_to_four_steps_matches_the_model() {
    use Step::{NextPermit, Pause, Press, Release};
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
    // Verbatim+Alt+T made with Alt down in the application: its mask
    // takes places 1 and 2, and Alt's release waits behind it.
    assert_eq!(
        verdicts(&[
            Press(K::Alt),
            Press(K::Insert),
            NextPermit,
            Press(K::T),
            Release(K::Alt)
        ]),
        [
            Verdict::Pass,
            Verdict::Swallow,
            Verdict::Swallow,
            Verdict::Defer(3)
        ]
    );
    // The same with a view that lets no key be held: no mask, and Alt's
    // release passes at once.
    assert_eq!(
        verdicts(&[
            Press(K::Alt),
            Press(K::Insert),
            Press(K::T),
            Release(K::Alt)
        ]),
        [
            Verdict::Pass,
            Verdict::Swallow,
            Verdict::Swallow,
            Verdict::Pass
        ]
    );
    // Caps Lock tapped twice passes the second press to Windows; after a
    // pause past the multi-press timeout it is the Verbatim key again.
    assert_eq!(
        verdicts(&[
            Press(K::Caps),
            Release(K::Caps),
            Press(K::Caps),
            Release(K::Caps),
            Pause,
            Press(K::Caps)
        ]),
        [
            Verdict::Swallow,
            Verdict::Swallow,
            Verdict::Pass,
            Verdict::Pass,
            Verdict::Swallow
        ]
    );
    for layout in [KeyboardLayout::Desktop, KeyboardLayout::Laptop] {
        let (map, bindings) = Bindings::for_layout(layout);
        let mut failures = Vec::new();
        let world = World::new(map, &bindings, HoldPermit::HoldAndDefer);
        walk(&world, DEPTH, &mut failures);
        assert_eq!(failures, Vec::<String>::new(), "{layout:?}");
    }
}

/// How many steps a walked sequence has, at most: four, since five took
/// 30 seconds with this alphabet. The walk starts with the permit
/// `HoldAndDefer`, so that four steps reach a mask of every kind (Control,
/// Shift, the Verbatim key and T), with the releases that follow it taken
/// by the walk's letting go; `None` is two steps away, and the examples
/// above start from it.
const DEPTH: usize = 4;
