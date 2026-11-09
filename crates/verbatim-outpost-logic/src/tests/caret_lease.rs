//! The caret rework's generated test (`docs/design/caret-selection-rework.md`,
//! "Tests"): Core's reducer and the outpost's together, so that a report's
//! use of the node's one baseline is real, driven by an application
//! ([`world`]) and compared after every step with a reference model written
//! from the parity documents and Dickson's dated decisions ([`model`]).
//! All contact with production types is in [`seam`], which the builder
//! changes alone.

use std::cell::RefCell;
use std::collections::BTreeSet;

use proptest::prelude::*;
use proptest::test_runner::{Config, TestError, TestRunner};

mod model;
mod seam;
mod world;

use model::{Heard, Model, Report};
use seam::{Seam, Seen};
use world::{Action, KEYS, Key, Us, World};

/// A case's start: the first field focused, on a clock that begins here.
const START: Us = 1_000_000;

/// One case's run: the world, the model and the seam, and what was heard.
struct Run {
    world: World,
    model: Model,
    seam: Seam,
    now: Us,
    keys: usize,
    pressed: Vec<Key>,
    final_asked: bool,
    heard: Vec<Heard>,
    exited: bool,
    failure: Option<String>,
}

impl Run {
    fn new() -> Self {
        let mut world = World::new();
        world.note(START);
        let mut model = Model::default();
        let text = world.fields[0].clone();
        model.first_focus(0, &text, START);
        let mut run = Self {
            world,
            model,
            seam: Seam::new(),
            now: START,
            keys: 0,
            pressed: Vec::new(),
            final_asked: false,
            heard: Vec::new(),
            exited: false,
            failure: None,
        };
        let seen = run.seam.focus(0, &text, START, START, false);
        run.take_seen(seen);
        run.compare();
        run
    }

    /// Applies what crossing the seam showed to the model and the record
    /// of speech, the first failure kept.
    fn take_seen(&mut self, seen: Vec<Seen>) {
        for seen in seen {
            let result = match seen {
                Seen::Heard(heard) => {
                    self.heard.push(heard);
                    Ok(())
                }
                Seen::Passed(id) => self.model.passed(id, self.now).map(|()| {
                    self.world.give(self.pressed[id]);
                }),
                Seen::Committed(field, text, before_read) => {
                    self.model.committed(field, &text, before_read);
                    Ok(())
                }
                Seen::Abandoned(role) => {
                    self.model.abandoned(role);
                    Ok(())
                }
                Seen::BeforeReadAsked => {
                    self.model.before_read_asked(self.now);
                    Ok(())
                }
                Seen::Report {
                    text,
                    began,
                    ended,
                    final_read,
                } => {
                    self.model.report(
                        &Report {
                            text,
                            began,
                            ended,
                            final_read,
                        },
                        self.now,
                    );
                    Ok(())
                }
                Seen::FinalReadAsked => {
                    self.model.final_read_asked();
                    self.final_asked = true;
                    Ok(())
                }
            };
            if let Err(failure) = result {
                self.failure.get_or_insert(failure);
            }
        }
    }

    /// Compares the facts with the model after a step.
    fn compare(&mut self) {
        let caret = self.seam.core_caret();
        if let Err(failure) = self.model.check(&self.heard, self.now, caret) {
            self.failure.get_or_insert(failure);
        }
    }

    fn step(&mut self, action: Action) {
        if self.exited || self.failure.is_some() {
            return;
        }
        self.final_asked = false;
        self.world.note(self.now);
        match action {
            Action::Press { key, held } => {
                let id = self.keys;
                self.keys += 1;
                let which = KEYS[usize::from(key) % KEYS.len()];
                self.pressed.push(which);
                let held = held || self.model.key_waits() || self.model.report_waits();
                self.model.press(which, held, self.now);
                let seen = self.seam.press(id, which, held, self.now);
                self.take_seen(seen);
            }
            Action::Apply { effect } => {
                if self.world.has_given() {
                    self.world.apply(effect);
                    self.world.note(self.now);
                }
            }
            Action::Select => {
                self.world.select();
                self.world.note(self.now);
            }
            Action::Event => {
                if !self.world.on_button {
                    let seen = self.seam.caret_event(self.world.focus, self.now);
                    self.take_seen(seen);
                }
            }
            Action::EventBehind => self.event_behind(),
            Action::Finish => {
                if self.seam.read_in_flight() {
                    let field = self.seam.reading_field();
                    let text = self.world.fields[field].clone();
                    let seen = self.seam.read_done(&text, self.now);
                    self.take_seen(seen);
                }
            }
            Action::Abandon => {
                let seen = self.seam.abandon();
                self.take_seen(seen);
            }
            Action::Focus { other, ago } => self.focus(other, ago),
            Action::FocusButton { new_window } => self.focus_button(new_window),
            Action::FocusWaits { ago } => self.focus_waits(ago),
            Action::FocusAnswer => self.focus_answer(),
            Action::Tick(ms) => {
                self.now += u64::from(ms) * 1_000;
                let seen = self.seam.tick(self.now);
                self.take_seen(seen);
                let asked = self.final_asked || self.seam.final_read_outstanding();
                if let Err(failure) = self.model.clock(self.now, asked) {
                    self.failure.get_or_insert(failure);
                }
            }
            Action::OutpostEnd => {
                self.model.outpost_ended(self.now);
                let seen = self.seam.outpost_ended();
                self.take_seen(seen);
            }
            Action::Exit => {
                self.model.exit();
                let seen = self.seam.exit();
                self.take_seen(seen);
                self.exited = true;
            }
        }
        self.compare();
    }

    /// A caret event of a field the focus is not on: the one it left, or
    /// the one it is moving to while that report waits.
    fn event_behind(&mut self) {
        let behind = self
            .model
            .waiting_field()
            .or(self.world.left.filter(|&left| left != self.world.focus));
        if let Some(field) = behind {
            let seen = self.seam.caret_event(field, self.now);
            self.take_seen(seen);
        }
    }

    fn focus(&mut self, other: bool, ago: u8) {
        if self.model.report_waits() {
            return;
        }
        let to = if other && !self.world.on_button {
            1 - self.world.focus
        } else {
            self.world.focus
        };
        let began = self.now - u64::from(ago) * 10_000;
        let text = self.world.text_at(to, began);
        let back = self.model.in_other_window();
        if self.world.focus != to {
            self.world.left = Some(self.world.focus);
        }
        self.world.focus = to;
        self.world.on_button = false;
        self.model.focus(to, &text, began, self.now);
        let seen = self.seam.focus(to, &text, began, self.now, back);
        self.take_seen(seen);
    }

    fn focus_button(&mut self, new_window: bool) {
        if self.model.report_waits() || !self.model.outpost_alive() || self.world.on_button {
            return;
        }
        self.world.left = Some(self.world.focus);
        self.world.on_button = true;
        self.model.focus_button(new_window, self.now);
        let seen = self.seam.focus_button(new_window, self.now);
        self.take_seen(seen);
    }

    fn focus_waits(&mut self, ago: u8) {
        if self.model.report_waits()
            || !self.model.outpost_alive()
            || !self.model.focus_is(0)
            || self.world.on_button
        {
            return;
        }
        let began = self.now - u64::from(ago) * 10_000;
        let text = self.world.text_at(1, began);
        self.model.focus_waits(1, &text);
        let seen = self.seam.focus_waits(1, &text, began, self.now);
        self.take_seen(seen);
    }

    fn focus_answer(&mut self) {
        let Some(to) = self.model.waiting_field() else {
            return;
        };
        let text = self.world.fields[to].clone();
        self.world.left = Some(self.world.focus);
        self.world.focus = to;
        self.model.focus(to, &text, self.now, self.now);
        let seen = self.seam.focus_answer(to, &text, self.now, self.now);
        self.take_seen(seen);
    }
}

/// One generated step.
fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        5 => (0..7u8, any::<bool>()).prop_map(|(key, held)| Action::Press { key, held }),
        5 => any::<bool>().prop_map(|effect| Action::Apply { effect }),
        2 => Just(Action::Select),
        5 => Just(Action::Event),
        1 => Just(Action::EventBehind),
        6 => Just(Action::Finish),
        1 => Just(Action::Abandon),
        2 => (any::<bool>(), 0..4u8).prop_map(|(other, ago)| Action::Focus { other, ago }),
        1 => any::<bool>().prop_map(|new_window| Action::FocusButton { new_window }),
        2 => (0..4u8).prop_map(|ago| Action::FocusWaits { ago }),
        2 => Just(Action::FocusAnswer),
        6 => prop_oneof![Just(10u8), Just(30), Just(60), Just(120)].prop_map(Action::Tick),
        1 => Just(Action::OutpostEnd),
        1 => Just(Action::Exit),
    ]
}

/// Runs `actions` in order; `Err` is the first thing that differed from
/// the model.
fn case(actions: &[Action]) -> Result<Outcome, String> {
    let mut run = Run::new();
    for &action in actions {
        run.step(action);
        if let Some(failure) = run.failure.take() {
            return Err(format!("{failure}\nafter {actions:?}"));
        }
    }
    Ok(Outcome {
        heard: run.heard,
        unexpressed: run.seam.unexpressed,
        reached: run.model.reached,
    })
}

/// What a case came to: its speech, what the seam could not express, and
/// the shapes it reached.
struct Outcome {
    heard: Vec<Heard>,
    unexpressed: BTreeSet<&'static str>,
    reached: BTreeSet<&'static str>,
}

/// The shapes the gate's cases must reach.
const REACHED: [&str; 9] = [
    "a key's bound passed",
    "a focus moving first says a key's words",
    "a key behind a focus report",
    "a focus report waiting",
    "a report behind the focus",
    "a silenced key's words",
    "exit with a key in line",
    "an outpost ended",
    "a key on a focus with no text",
];

/// The smallest failing case as a line for [`fixed`], and how to run it.
fn found_report(actions: &[Action], again: &str) -> String {
    format!(
        "the smallest failing case, as a line for `fixed()` in crates/verbatim-outpost-logic/src/tests/caret_lease.rs:\n        vec!{actions:?},\n\
         the fixed cases alone: cargo test -p verbatim-outpost-logic --lib -- --exact tests::caret_lease::fixed_cases_keep_the_recorded_rules\n\
         the run that found it again: {again}"
    )
}

/// The generated cases under `config`, each run twice from a new owner and
/// compared; a failing case's smallest form printed as a line for
/// [`fixed`]. `again` is the command that runs it again. What the cases
/// reached.
///
/// # Panics
///
/// On a failing case or a fact the seam cannot express.
pub(super) fn run_generated(config: Config, again: &str) -> BTreeSet<&'static str> {
    let unexpressed = RefCell::new(BTreeSet::new());
    let reached = RefCell::new(BTreeSet::new());
    let mut runner = TestRunner::new(config);
    let strategy = proptest::collection::vec(action(), 1..40);
    let outcome = runner.run(&strategy, |actions| {
        let first = case(&actions)
            .map_err(|failure| proptest::test_runner::TestCaseError::Fail(failure.into()))?;
        let second = case(&actions)
            .map_err(|failure| proptest::test_runner::TestCaseError::Fail(failure.into()))?;
        assert_eq!(first.heard, second.heard, "two runs of one case differ");
        unexpressed.borrow_mut().extend(first.unexpressed);
        reached.borrow_mut().extend(first.reached);
        Ok(())
    });
    match outcome {
        Ok(()) => {}
        Err(TestError::Fail(reason, actions)) => {
            panic!("{reason}\n{}", found_report(&actions, again));
        }
        Err(TestError::Abort(reason)) => panic!("the generated test aborted: {reason}"),
    }
    let unexpressed = unexpressed.into_inner();
    assert!(
        unexpressed.is_empty(),
        "the seam cannot express these, so this test proves nothing of them: {unexpressed:?}"
    );
    reached.into_inner()
}

#[test]
fn every_generated_case_keeps_the_recorded_rules() {
    let reached = run_generated(
        super::explore::config(0x0c4a_6e70, 512),
        "cargo test -p verbatim-outpost-logic --lib -- --exact tests::caret_lease::every_generated_case_keeps_the_recorded_rules",
    );
    let missed: Vec<&&str> = REACHED
        .iter()
        .filter(|shape| !reached.contains(**shape))
        .collect();
    assert_eq!(
        missed,
        Vec::<&&str>::new(),
        "shapes the cases did not reach"
    );
}

/// The failures found, each kept as its actions: a key held at exit, and a
/// key behind a waiting focus report, whose bound starts when the wait ends.
fn fixed() -> Vec<Vec<Action>> {
    let press = Action::Press { key: 0, held: true };
    vec![
        vec![press, Action::Exit],
        vec![
            Action::FocusWaits { ago: 0 },
            press,
            Action::Tick(120),
            Action::Tick(120),
            Action::FocusAnswer,
            Action::Tick(120),
            Action::Tick(120),
        ],
    ]
}

#[test]
fn fixed_cases_keep_the_recorded_rules() {
    for actions in fixed() {
        assert_eq!(case(&actions).err(), None, "{actions:?}");
    }
}
