//! The caret watch's generated test
//! (`books/dev/src/reference/crates/verbatim-outpost-logic.md` describes
//! it): a caret key
//! from its read before it, through its watch, to what it says, across the
//! hand-off between Core and the outpost. Core's reducer and the outpost's
//! are driven together ([`seam`]) by a world of what real applications and
//! users do ([`world`]), and what is said, and when, is compared after every
//! step with a reference model written from NVDA's rules through the parity
//! documents and Dickson's recorded decisions ([`model`]).
//!
//! The world: Windows 11 Notepad's document, whose every handled key raises
//! its UI Automation events (R1), and the Windows Forms text box, whose
//! events no outpost hears; caret moves and edits at varied delays or not
//! at all; one to four keys at least 30 ms apart; reads of the application
//! of varied latency, some past the hook's 150 ms release; and the
//! application moving its focus to a button, so that a focus is pending
//! while a key starts or watches, or is committed before a key's turn.
//!
//! After every step (everything at one microsecond) what has been heard,
//! with the time each was said, must equal what the model says is heard by
//! then; at the end, all of it.
//!
//! Two things at one microsecond leave their order open, the harness's,
//! so the model refuses such a script and the world does not run it.
//!
//! Not in the world: the console host, whose caret key's read before it
//! and final read are build 3's terminal reads, which no message of
//! `verbatim_model` carries yet, while Core runs terminals' keys on
//! today's key line; sentence keys and selections, whose rules this model
//! does not take; and the caret tells and value changes the application's
//! events also give Core, which Core's hand-written caret tell tests cover (the
//! outpost hears Notepad's events here, for the watch, and tells Core
//! nothing of them).

#[path = "caret_watch/model.rs"]
mod model;
#[path = "caret_watch/seam.rs"]
mod seam;
#[path = "caret_watch/world.rs"]
mod world;

use std::fmt::Write as _;

use proptest::test_runner::{Config, RngAlgorithm, RngSeed, TestCaseError, TestError, TestRunner};

use model::{Expected, Heard, Model};
use seam::{Seam, Seen};
use world::{App, FIRST, FocusMove, Handling, Input, Key, Kind, MS, Press, Script, Us};

/// What happens in the world at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Happening {
    /// The application changes the field for key `0`'s stroke.
    Change(usize),
    /// The application raises its events for key `0`'s stroke.
    Raised(usize),
    /// The application moves its focus to the button.
    FocusMoved,
    /// The user presses key `0`.
    Press(usize),
    /// The listener tells Core of the button's focus.
    FocusTold,
}

/// One case's run.
struct Run<'s> {
    script: &'s Script,
    expected: Expected,
    held: Vec<bool>,
    seam: Seam,
    app: App,
    input: Input,
    field_focused: bool,
    /// Whether each key's change altered the text.
    edited: Vec<bool>,
    agenda: Vec<(Us, Happening)>,
    heard: Vec<(Us, Heard)>,
}

/// What startup says before the first key: the window's title and the
/// field, which this test does not judge.
fn startup_heard(heard: &[Heard]) -> Result<(), String> {
    if heard
        .iter()
        .any(|heard| matches!(heard, Heard::Char(_) | Heard::Blank))
    {
        return Err(format!("startup said a key's words: {heard:?}"));
    }
    Ok(())
}

impl<'s> Run<'s> {
    fn new(script: &'s Script, expected: Expected, held: Vec<bool>) -> Result<Self, String> {
        let app = App {
            text: script.text.clone(),
            caret: script.caret,
        };
        let read_us = script.presses.iter().map(|press| press.read_us).collect();
        let focus_us = script
            .focus
            .map_or((0, 0), |focus| (focus.listener_us, focus.completion_us));
        let (seam, startup) = Seam::new(script.kind, &app, read_us, focus_us, FIRST - 500 * MS);
        startup_heard(&startup)?;
        let mut agenda: Vec<(Us, Happening)> = script
            .presses
            .iter()
            .enumerate()
            .map(|(key, press)| (press.at, Happening::Press(key)))
            .collect();
        if let Some(focus) = script.focus {
            agenda.push((focus.at, Happening::FocusMoved));
            agenda.push((focus.told, Happening::FocusTold));
        }
        Ok(Self {
            script,
            expected,
            held,
            seam,
            app,
            input: Input::default(),
            field_focused: true,
            edited: vec![false; script.presses.len()],
            agenda,
            heard: Vec::new(),
        })
    }

    /// Runs the case to its end, comparing after every step.
    fn run(mut self) -> Result<(), String> {
        let last = self
            .script
            .presses
            .iter()
            .map(|press| press.at)
            .chain(self.script.focus.map(|focus| focus.told))
            .max()
            .unwrap_or(FIRST);
        let end = last + 1_000 * MS;
        loop {
            self.agenda.sort_unstable();
            let next = [self.agenda.first().map(|&(at, _)| at), self.seam.next_due()]
                .into_iter()
                .flatten()
                .min();
            let Some(at) = next.filter(|&at| at <= end) else {
                break;
            };
            while let Some(&(when, happening)) = self.agenda.first() {
                if when != at {
                    break;
                }
                self.agenda.remove(0);
                let seen = self.happen(at, happening);
                self.take(at, seen);
            }
            let seen = self.seam.advance(at, &self.app);
            self.take(at, seen);
            self.compare(at)?;
        }
        if self.heard != self.expected {
            return Err(self.mismatch("at the end", end));
        }
        Ok(())
    }

    fn happen(&mut self, at: Us, happening: Happening) -> Vec<Seen> {
        match happening {
            Happening::Change(key) => {
                let before = self.app.text.clone();
                self.app.apply(self.script.presses[key].key);
                self.edited[key] = self.app.text != before;
                Vec::new()
            }
            Happening::Raised(key) => self.seam.raised(at, self.edited[key], &self.app),
            Happening::FocusMoved => {
                self.field_focused = false;
                Vec::new()
            }
            Happening::Press(key) => {
                let press = self.script.presses[key];
                self.seam
                    .press(key, press.key, self.held[key], at, &self.app)
            }
            Happening::FocusTold => self.seam.focus_told(at, &self.app),
        }
    }

    /// Takes what crossing the seam showed: speech heard, and strokes
    /// reaching the application.
    fn take(&mut self, at: Us, seen: Vec<Seen>) {
        for seen in seen {
            match seen {
                Seen::Heard(heard) => self.heard.push((at, heard)),
                Seen::Sent(key) => {
                    if !self.field_focused {
                        continue;
                    }
                    let handling: Option<Handling> = self.script.presses[key].handling;
                    if let Some((change, event)) = self.input.stroke(at, handling, self.script.kind)
                    {
                        self.agenda.push((change, Happening::Change(key)));
                        if let Some(event) = event {
                            self.agenda.push((event, Happening::Raised(key)));
                        }
                    }
                }
            }
        }
    }

    /// After the step at `at`: what was heard is exactly what the model says
    /// is heard by then.
    fn compare(&self, at: Us) -> Result<(), String> {
        let due: Vec<(Us, Heard)> = self
            .expected
            .iter()
            .filter(|(when, _)| *when <= at)
            .cloned()
            .collect();
        if self.heard != due {
            return Err(self.mismatch("after a step", at));
        }
        Ok(())
    }

    fn mismatch(&self, when: &str, at: Us) -> String {
        format!(
            "{when} at {} ms: heard {:?}, the model {:?}
what crossed the seam:
{}",
            ms(at),
            relative(&self.heard),
            relative(&self.expected),
            self.seam.log.join(
                "
"
            ),
        )
    }
}

/// A time as milliseconds from the first key, for reading.
fn ms(at: Us) -> String {
    let after = at.saturating_sub(FIRST);
    format!("{}.{:03}", after / 1_000, after % 1_000)
}

/// Times from the first key, in milliseconds, for reading.
fn relative(heard: &[(Us, Heard)]) -> Vec<(String, &Heard)> {
    heard.iter().map(|(at, heard)| (ms(*at), heard)).collect()
}

/// Runs `script`: `Ok(false)` when the model leaves its outcome open.
fn check(script: &Script) -> Result<bool, String> {
    let Ok(prediction) = Model::new(script).run() else {
        return Ok(false);
    };
    let run = Run::new(script, prediction.expected, prediction.held)?;
    run.run()?;
    Ok(true)
}

/// The configuration: a fixed seed, no failure files, no fork and no
/// timeout, every field written out.
fn config(seed: u64, cases: u32) -> Config {
    Config {
        cases,
        max_local_rejects: 1_000_000,
        max_global_rejects: 1_000_000,
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
        rng_seed: RngSeed::Fixed(seed),
        _non_exhaustive: (),
    }
}

/// `script` as the line that makes it, for `found()`.
fn code(script: &Script) -> String {
    let mut out = format!(
        "Script {{ kind: Kind::{:?}, text: {:?}.to_owned(), caret: {}, presses: vec![",
        script.kind, script.text, script.caret
    );
    for press in &script.presses {
        let handling = press.handling.map_or("None".to_owned(), |handling| {
            format!(
                "Some(Handling {{ change_us: {}, event_us: {} }})",
                handling.change_us, handling.event_us
            )
        });
        let _ = write!(
            out,
            "Press {{ at: {}, key: Key::{:?}, handling: {handling}, read_us: {} }}, ",
            press.at, press.key, press.read_us
        );
    }
    out.push_str("], focus: ");
    match script.focus {
        Some(focus) => {
            let _ = write!(
                out,
                "Some(FocusMove {{ at: {}, told: {}, listener_us: {}, completion_us: {} }})",
                focus.at, focus.told, focus.listener_us, focus.completion_us
            );
        }
        None => out.push_str("None"),
    }
    out.push_str(" },");
    out
}

#[test]
fn every_generated_case_keeps_nvdas_caret_wait() {
    let mut runner = TestRunner::new(config(20_261_116, 2048));
    let result = runner.run(&world::script(), |script| match check(&script) {
        Ok(true) => Ok(()),
        Ok(false) => Err(TestCaseError::reject("the model leaves it open")),
        Err(failure) => Err(TestCaseError::fail(failure)),
    });
    if let Err(TestError::Fail(reason, script)) = result {
        eprintln!(
            "the smallest failing case, as a line for `found()` in crates/verbatim-outpost-logic/tests/caret_watch.rs:\n        {}\n\
             the found cases alone: cargo test -p verbatim-outpost-logic --test caret_watch -- --exact found_cases_keep_nvdas_caret_wait\n\
             this test again: cargo test -p verbatim-outpost-logic --test caret_watch -- --exact every_generated_case_keeps_nvdas_caret_wait",
            code(&script)
        );
        panic!("{reason}");
    } else if let Err(error) = result {
        panic!("{error}");
    }
}

/// A key pressed at `at_ms` after the first key's time.
fn press(at_ms: Us, key: Key, handling: Option<(Us, Us)>, read_ms: Us) -> Press {
    Press {
        at: FIRST + at_ms * MS,
        key,
        handling: handling.map(|(change_ms, event_ms)| Handling {
            change_us: change_ms * MS,
            event_us: event_ms * MS,
        }),
        read_us: read_ms * MS,
    }
}

fn script(kind: Kind, caret: usize, presses: Vec<Press>, focus: Option<FocusMove>) -> Script {
    Script {
        kind,
        text: "ab cde".to_owned(),
        caret,
        presses,
        focus,
    }
}

/// The application moves its focus to its button at `at_ms` and the
/// listener tells Core 1 ms later; each of its reads takes 5 ms.
fn focus_at(at_ms: i64) -> FocusMove {
    let at = FIRST
        .checked_add_signed(at_ms * 1_000)
        .expect("after the clock's start");
    FocusMove {
        at,
        told: at + MS,
        listener_us: 5 * MS,
        completion_us: 5 * MS,
    }
}

/// The model's rules, each in one script on "ab cde" with what it says,
/// written by hand from the rule, the time each is said in ms after the
/// first key.
/// A rule: what it says, its script, and what is heard.
type Rule = (&'static str, Script, Vec<(Us, Heard)>);

#[expect(
    clippy::too_many_lines,
    reason = "one list of the rules, each a few lines, read in order"
)]
fn rules() -> Vec<Rule> {
    // Heard `us` microseconds after the first key.
    let at = |us: Us, heard: Heard| (FIRST + us, heard);
    vec![
        (
            "Notepad's event 3 ms after the key brings a check forward, which finds the move: the character at the caret",
            script(
                Kind::Notepad,
                0,
                vec![press(0, Key::Right, Some((2, 1)), 5)],
                None,
            ),
            vec![at(8_000, Heard::Char('b'))],
        ),
        (
            "in the text box the check 10 ms after the key finds the move",
            script(
                Kind::TextBox,
                0,
                vec![press(0, Key::Right, Some((2, 1)), 5)],
                None,
            ),
            vec![at(15_000, Heard::Char('b'))],
        ),
        (
            "a key the application does not handle says its unit at 100 ms",
            script(Kind::TextBox, 0, vec![press(0, Key::Right, None, 5)], None),
            vec![at(105_000, Heard::Char('a'))],
        ),
        (
            "Backspace the application does not handle says nothing",
            script(
                Kind::TextBox,
                2,
                vec![press(0, Key::Backspace, None, 5)],
                None,
            ),
            vec![],
        ),
        (
            "Delete says the character it leaves at the caret",
            script(
                Kind::TextBox,
                0,
                vec![press(0, Key::Delete, Some((2, 1)), 5)],
                None,
            ),
            vec![at(15_000, Heard::Char('b'))],
        ),
        (
            "Control+Right Arrow says the word at the caret",
            script(
                Kind::Notepad,
                0,
                vec![press(0, Key::WordRight, Some((2, 1)), 5)],
                None,
            ),
            vec![at(8_000, Heard::Text("cde".to_owned()))],
        ),
        (
            "Backspace with a focus pending says what it deleted, then the focus",
            script(
                Kind::Notepad,
                2,
                vec![press(0, Key::Backspace, Some((30, 1)), 5)],
                Some(focus_at(20)),
            ),
            vec![
                at(21_000, Heard::Char('b')),
                at(31_000, Heard::Focus("OK".to_owned())),
            ],
        ),
        (
            "a movement key with a focus pending says nothing, then the focus",
            script(
                Kind::Notepad,
                0,
                vec![press(0, Key::Right, Some((30, 1)), 5)],
                Some(focus_at(20)),
            ),
            vec![at(31_000, Heard::Focus("OK".to_owned()))],
        ),
        (
            "a key whose read before it passes 150 ms is released and says nothing",
            script(
                Kind::TextBox,
                0,
                vec![press(0, Key::Right, Some((2, 1)), 200)],
                None,
            ),
            vec![],
        ),
        (
            "a later key ends the wait silently, and reads at its turn; its check finds both keys' moves",
            script(
                Kind::TextBox,
                0,
                vec![
                    press(0, Key::Right, Some((50, 1)), 5),
                    press(30, Key::Right, Some((2, 1)), 4),
                ],
                None,
            ),
            vec![at(64_000, Heard::Char(' '))],
        ),
        (
            "a key whose field lost the focus before its turn runs on that field, after the focus, its stroke reaching the button",
            script(
                Kind::Notepad,
                0,
                vec![press(0, Key::Right, Some((2, 1)), 5)],
                Some(focus_at(-2)),
            ),
            vec![
                at(9_000, Heard::Focus("OK".to_owned())),
                at(114_000, Heard::Char('a')),
            ],
        ),
        (
            "a change first seen at the bound is the key's effect: Backspace says what it deleted",
            script(
                Kind::TextBox,
                2,
                vec![press(0, Key::Backspace, Some((95, 1)), 5)],
                None,
            ),
            vec![at(105_000, Heard::Char('b'))],
        ),
        (
            "an event heard while a key's read before it is out is nothing special: the key's own move ends its wait",
            script(
                Kind::Notepad,
                0,
                vec![
                    press(0, Key::Right, Some((32, 1)), 5),
                    press(30, Key::Left, Some((2, 1)), 10),
                ],
                None,
            ),
            vec![at(43_000, Heard::Char('a'))],
        ),
    ]
}

/// The model says what each rule says: a test of the model alone.
#[test]
fn the_model_keeps_each_rule() {
    for (rule, script, heard) in rules() {
        let prediction = Model::new(&script)
            .run()
            .map(|prediction| prediction.expected);
        assert_eq!(prediction, Ok(heard), "{rule}");
    }
}

/// Each rule's script against the code.
#[test]
fn each_rule_keeps_nvdas_caret_wait() {
    let failures: Vec<String> = rules()
        .into_iter()
        .filter_map(|(rule, script, _)| {
            check(&script)
                .err()
                .map(|failure| format!("{rule}: {}", failure.lines().next().unwrap_or_default()))
        })
        .collect();
    assert_eq!(failures, Vec::<String>::new());
}

/// The cases the generated test failed on, each kept whole.
fn found() -> Vec<Script> {
    vec![
        // A focus told while the read before Left waits in the outpost
        // behind a read the hook's release left running: the read is to be
        // withdrawn and the focus taken at once (`books/dev/src/reference/crates/verbatim-core.md`, "The caret key on the event queue"), and is not.
        Script {
            kind: Kind::Notepad,
            text: "ab".to_owned(),
            caret: 0,
            presses: vec![
                Press {
                    at: 1_000_000,
                    key: Key::Left,
                    handling: Some(Handling {
                        change_us: 120_000,
                        event_us: 1,
                    }),
                    read_us: 150_001,
                },
                Press {
                    at: 1_030_000,
                    key: Key::Left,
                    handling: Some(Handling {
                        change_us: 1_000,
                        event_us: 1,
                    }),
                    read_us: 1_000,
                },
            ],
            focus: Some(FocusMove {
                at: 1_146_122,
                told: 1_150_002,
                listener_us: 1_000,
                completion_us: 1_000,
            }),
        },
    ]
}

/// Every found case keeps the model, and the model decides each.
#[test]
fn found_cases_keep_nvdas_caret_wait() {
    for script in found() {
        assert_eq!(check(&script), Ok(true), "{}", code(&script));
    }
}
