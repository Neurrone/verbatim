//! Every sequence of up to four keys, pass-ons, permit changes and pauses
//! through the decision machine, from a view that lets keys be held,
//! checked against invariants that need no model
//! (`books/dev/src/reference/crates/verbatim-input.md`), for each keyboard
//! layout's gesture map, built from the one gesture table
//! ([`verbatim_input::Tables`]).
//!
//! The alphabet is twenty steps: the press and release of Right Arrow (a
//! press of a key already down is its auto-repeat), A, Shift, Alt,
//! Control, the left Windows key, Insert and Caps Lock, the last two both
//! Verbatim keys; T's press; Core's pass-on of the oldest place in line;
//! the permit moved to its next value; and a pause past the multi-press
//! timeout. The gesture map is the table's for the layout, with
//! Verbatim+Right Arrow, Verbatim+T, Verbatim+Alt+T,
//! Verbatim+Control+Shift+T, Verbatim+Windows+T and Verbatim+Left Alt
//! bound as well, so a caret key is also a gesture's key, every kind of
//! mask is walked, and Alt's press can be a gesture's own.
//! Steps are 33 ms apart, inside the multi-press timeout, unless a pause
//! comes between. A release is offered only for a key down, and a pass-on
//! only while a place waits. The permit never falls to `None` while a
//! deferred press or a mask waits in line: a release would then pass at
//! once, ahead of its press or of the mask, the known limitation of "The
//! hook's hold decision" (`books/dev/src/explanation/design/key-holding.md`, "Limitations").
//! The walk's `None` stands for a view that lets no key be held, so no
//! mask is due for a gesture made with it; the hook's
//! other `None`, a view that lets keys be held with no text and nothing in
//! line, decides as `Defer` does with an empty line, which the walk
//! covers.
//!
//! After every step: no release reaches the application before its press;
//! Alt's, the Windows key's, or Control's and Shift's together, release
//! does not reach it after a gesture made with them down in it until that
//! gesture's mask has; and the machine's line holds a place exactly while
//! one is given out and not passed on. After every sequence, every key let
//! go and every place passed on, the application has every key up. What
//! each transition's verdict is, gesture naming, multi-press counting and
//! the double tap are stated by `state.rs`'s unit tests and the examples
//! here.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use verbatim_input::{
    DecisionConfig, DecisionMachine, GestureMap, HoldPermit, KeyEvent, KeyInput, KeySource,
    KeyboardLayout, ScriptAction, Tables, Verdict,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Press(K),
    Release(K),
    PassOn,
    NextPermit,
    Pause,
}

impl Step {
    /// The step as the Rust expression that makes it, as a found sequence
    /// lists it.
    fn code(self) -> String {
        match self {
            Step::Press(key) => format!("Step::Press(K::{key:?})"),
            Step::Release(key) => format!("Step::Release(K::{key:?})"),
            other => format!("Step::{other:?}"),
        }
    }
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

/// The gesture map of `layout`'s gesture table with the walk's own
/// bindings, which run a script as any binding does; which script is the
/// reducer's concern, not the machine's.
fn map_for(layout: KeyboardLayout) -> GestureMap {
    let id = |raw| GestureId::parse(raw).expect("valid id");
    let mut tables = Tables::for_layout(layout, id("kb:verbatim+v"));
    for gesture in [
        "kb:verbatim+rightarrow",
        "kb:verbatim+t",
        "kb:verbatim+leftalt",
        "kb:verbatim+alt+t",
        "kb:verbatim+control+shift+t",
        "kb:verbatim+leftwindows+t",
    ] {
        tables.scripts.insert(id(gesture), ScriptAction::SpeakTime);
    }
    tables.gesture_map()
}

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

/// The machine, and what the application has received.
#[derive(Clone)]
struct World {
    machine: DecisionMachine,
    now: u64,
    permit: HoldPermit,
    user_down: BTreeSet<K>,
    line: VecDeque<(u32, Sent)>,
    app_down: BTreeSet<K>,
    places: BTreeSet<u32>,
    /// Each mask in line, by its press's place, with the modifiers down
    /// in the application when its gesture was made, whose release must
    /// not reach it before that mask's press does.
    mask_due: BTreeMap<u32, BTreeSet<K>>,
    trail: Vec<Step>,
}

impl World {
    fn new(map: GestureMap, permit: HoldPermit) -> Self {
        Self {
            machine: DecisionMachine::new(DecisionConfig::default(), map),
            now: 1_000_000_000,
            permit,
            user_down: BTreeSet::new(),
            line: VecDeque::new(),
            app_down: BTreeSet::new(),
            places: BTreeSet::new(),
            mask_due: BTreeMap::new(),
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

    /// The modifiers that are down in the application, or whose press is in
    /// line to reach it, if they could act there when released alone: Alt,
    /// the Windows key, or Control and Shift together.
    fn acting_modifiers(&self) -> BTreeSet<K> {
        let down = |key: K| {
            self.line
                .iter()
                .rev()
                .find_map(|&(_, sent)| match sent {
                    Sent::Transition(sent, pressed) if sent == key => Some(pressed),
                    _ => None,
                })
                .unwrap_or_else(|| self.app_down.contains(&key))
        };
        let mut acting: BTreeSet<K> = [K::Alt, K::Windows]
            .into_iter()
            .filter(|&key| down(key))
            .collect();
        if down(K::Control) && down(K::Shift) {
            acting.extend([K::Control, K::Shift]);
        }
        acting
    }

    /// The application receives `key`'s transition.
    fn app(&mut self, key: K, pressed: bool) -> Result<(), String> {
        if !pressed && self.mask_due.values().any(|keys| keys.contains(&key)) {
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

    /// Puts a transition's mask in line, and for a gesture made with the
    /// view letting keys be held, notes the modifiers whose release must
    /// wait for that mask.
    fn masked(&mut self, gesture: bool, mask: Option<(u32, u32)>) -> Result<(), String> {
        if gesture && self.permit != HoldPermit::None {
            let acting = self.acting_modifiers();
            match mask {
                Some((press, _)) => {
                    self.mask_due.insert(press, acting);
                }
                None if !acting.is_empty() => {
                    return Err(format!(
                        "a gesture made with {acting:?} down in the application has no mask"
                    ));
                }
                None => {}
            }
        }
        if let Some((press, release)) = mask {
            self.push(press, Sent::Mask(true))?;
            self.push(release, Sent::Mask(false))?;
        }
        Ok(())
    }

    /// Takes `step`, returning the verdict of a key's transition, and
    /// checks it against the line.
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
                self.masked(decision.emitted().is_some(), decision.mask())?;
                if pressed {
                    self.user_down.insert(key);
                } else {
                    self.user_down.remove(&key);
                }
                match decision.verdict() {
                    Verdict::Pass => self.app(key, pressed)?,
                    Verdict::Swallow => {}
                    Verdict::Hold(place) => self.push(place, Sent::Held(key))?,
                    Verdict::Defer(place) => self.push(place, Sent::Transition(key, pressed))?,
                }
                verdict = Some(decision.verdict());
            }
            Step::PassOn => {
                self.now += STEP_US;
                let (place, sent) = self.line.pop_front().expect("a place in line");
                self.machine.passed_on(place);
                match sent {
                    Sent::Held(key) => {
                        self.app(key, true)?;
                        self.app(key, false)?;
                    }
                    Sent::Transition(key, pressed) => self.app(key, pressed)?,
                    Sent::Mask(true) => {
                        self.mask_due.remove(&place);
                    }
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
/// failures, each with its sequence.
fn walk(world: &World, depth: usize, failures: &mut Vec<(Vec<Step>, String)>) {
    for step in STEPS {
        if !world.allows(step) {
            continue;
        }
        let mut next = world.clone();
        if let Err(error) = next.apply(step) {
            failures.push((next.trail.clone(), format!("{:?}: {error}", next.trail)));
            continue;
        }
        if let Err(error) = next.clone().finish() {
            failures.push((
                next.trail.clone(),
                format!("{:?}, then let go: {error}", next.trail),
            ));
        }
        if depth > 1 {
            walk(&next, depth - 1, failures);
        }
    }
}

/// The verdicts of the key transitions in `steps`, from a fresh world with
/// the desktop layout, with the sequence checked as the walk checks it.
fn verdicts(steps: &[Step]) -> Vec<Verdict> {
    let mut world = World::new(map_for(KeyboardLayout::Desktop), HoldPermit::None);
    let verdicts = steps
        .iter()
        .filter_map(|&step| world.apply(step).expect("the sequence holds"))
        .collect();
    world.finish().expect("every key ends up");
    verdicts
}

#[test]
fn every_sequence_of_up_to_four_steps_keeps_the_invariants() {
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
        let mut failures = Vec::new();
        let world = World::new(map_for(layout), HoldPermit::HoldAndDefer);
        walk(&world, DEPTH, &mut failures);
        if let Some((shortest, _)) = failures.iter().min_by_key(|(trail, _)| trail.len()) {
            let steps: Vec<String> = shortest.iter().map(|step| step.code()).collect();
            eprintln!(
                "the shortest failing sequence, as a line for `found()` in crates/verbatim-input/tests/hold_sequences.rs:\n        (KeyboardLayout::{layout:?}, vec![{}]),\n\
                 the found sequences alone: cargo test -p verbatim-input --test hold_sequences -- --exact found_sequences_keep_the_invariants",
                steps.join(", ")
            );
        }
        let failures: Vec<String> = failures.into_iter().map(|(_, failure)| failure).collect();
        assert_eq!(failures, Vec::<String>::new(), "{layout:?}");
    }
}

/// The sequences the walk failed on, each kept as its layout and steps
/// from the walk's start, as the failure's report gives them.
fn found() -> Vec<(KeyboardLayout, Vec<Step>)> {
    Vec::new()
}

/// Every found sequence, from the walk's start, keeps the invariants at
/// each step and once every key is let go.
#[test]
fn found_sequences_keep_the_invariants() {
    for (layout, steps) in found() {
        let mut world = World::new(map_for(layout), HoldPermit::HoldAndDefer);
        for step in steps {
            assert!(world.allows(step), "{step:?} after {:?}", world.trail);
            world.apply(step).expect("the step keeps the invariants");
        }
        world.finish().expect("every key ends up");
    }
}

/// How many steps a walked sequence has, at most: four, since five took
/// 30 seconds with this alphabet. The walk starts with the permit
/// `HoldAndDefer`, so that four steps reach a mask of every kind (Control,
/// Shift, the Verbatim key and T), with the releases that follow it taken
/// by the walk's letting go; `None` is two steps away, and the examples
/// above start from it.
const DEPTH: usize = 4;
