//! The caret waits in the outpost's waiting table
//! (`docs/design/readers-tell-core-asks-build1.md`, "The watch", Tables 14
//! and 15): a batch whose first step is `WaitForCaret` (the request list's
//! design, section 3) enters with its first check due now and leaves when
//! answered.
//!
//! A check compares the application's caret with the key's starting point
//! by the effect test (`verbatim_model::had_effect`), the first at once and
//! then one about `poll_us` after the last polled check began, until
//! `bound_us` after the pass; the check due at the bound is the last, and a
//! change it first sees counts as the effect, as NVDA's last try does
//! (`editableText.py` 124 to 152). A check that shows the effect, and the
//! last check whatever it shows, runs the batch's later steps and answers.
//!
//! Where the request says the event rule applies, a caret or text-change
//! event of the watched node heard within 60 ms of the pass brings a check
//! forward: a waiting watch's check is due at once, and a check running
//! when it is heard is followed by another due at once. A check brought
//! forward compares the word at the caret too, whatever the time, and ends
//! the watch only with the effect, as every check before the bound does;
//! the polls after it keep their times. An event counts only if the caret
//! is no longer where it was when the key was pressed (Dickson,
//! 2026-11-17), because Notepad raised a key's event about 7 ms late,
//! inside the next key's watch. A later event changes nothing, as NVDA
//! looks at events only within those 60 ms (`editableText.py` 57).
//!
//! The watched node's events are the ones the outpost's `Follow`
//! subscription hears: it is registered on the followed node alone, and
//! Core names the event rule only for the committed focus, which it
//! follows.
//!
//! A check that ends without answering showed no effect, or its read
//! failed, which counts as no effect; a last check that ends so is
//! answered failed.

use verbatim_model::batch::{Batch, CaretWait, Malformed, Op, ReaderKind, Step, Subject};
use verbatim_model::request::{Answer, Cancel, Outcome, RequestNumber};
use verbatim_model::{NodeId, WindowHandle};

/// How long after the pass a UI Automation event brings a check forward:
/// NVDA's `_useEvents_maxTimeoutSec` (`editableText.py` 57).
pub const EVENT_WINDOW_US: u64 = 60_000;

/// How long after the pass Delete's polled check compares the word at the
/// caret: NVDA's `_hasCaretMoved_minWordTimeoutSec` (`editableText.py` 54).
pub const WORD_AFTER_US: u64 = 30_000;

/// What a watch's program does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchWork {
    /// Compare the caret with the starting point; with the effect, run the
    /// batch's later steps and answer it.
    Check {
        /// Whether to compare the word at the caret as well: a wait whose
        /// effect test counts a change of the unit at the caret (Delete
        /// and Control+Delete), once 30 ms have passed since the pass or
        /// when an event brought the check forward.
        word: bool,
        /// Whether this is the check at the bound: with no effect, it runs
        /// the batch's later steps and answers the wait's bound.
        last: bool,
    },
}

/// A watch's program, for the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchProgram {
    /// The watch's request number, which its answer names.
    pub number: RequestNumber,
    /// The watched node.
    pub node: NodeId,
    /// The wait, the batch's first step.
    pub wait: CaretWait,
    /// The batch.
    pub batch: Batch,
    /// What the program does.
    pub work: WatchWork,
}

/// The node and the wait of `batch`'s first step, when it is a
/// `WaitForCaret` on a node: the one shape the waiting table takes.
#[must_use]
pub fn caret_wait(batch: &Batch) -> Option<(NodeId, &CaretWait)> {
    match &batch.steps.first()?.op {
        Op::WaitForCaret {
            subject: Subject::Node(node),
            wait,
        } => Some((*node, wait)),
        _ => None,
    }
}

/// What follows a check that shows no effect (Table 15, W2, A).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    /// The next poll.
    Poll,
    /// Another check at once: an event, heard at the time it carries while
    /// the check ran, brought it forward.
    Again(u64),
}

/// A watch's state (Table 15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// W1: its next check, a poll, due at `due_us`, on the run's shared
    /// clock.
    Waiting { due_us: u64 },
    /// W1, brought forward: an event heard at `heard_us` brought a check
    /// forward, due at once; the poll after it is due at `poll_us`.
    Brought { heard_us: u64, poll_us: u64 },
    /// W2: its check runs, the program in flight; with no effect the next
    /// poll is due at `poll_us`. `gone`: its window was destroyed while the
    /// check ran, so it is answered not there when the check ends (Table
    /// 15, W2, H).
    Checking {
        last: bool,
        poll_us: u64,
        then: Then,
        gone: bool,
    },
}

/// One watch in the table.
#[derive(Clone, Debug)]
struct Watching {
    number: RequestNumber,
    window: WindowHandle,
    /// The watched node, the subject of its batch's first step.
    node: NodeId,
    /// The wait, its batch's first step.
    wait: CaretWait,
    /// Its batch with the first step taken out: the steps after the wait.
    after: Batch,
    phase: Phase,
}

impl Watching {
    /// `batch`, whose first step is a caret wait on a node, as a watch
    /// numbered `number` in `window`, its first check due at `due_us`;
    /// `None` for any other batch.
    fn new(
        (number, window): (RequestNumber, WindowHandle),
        mut batch: Batch,
        due_us: u64,
    ) -> Option<Self> {
        let mut steps = std::mem::take(&mut batch.steps).into_iter();
        let Some(Step {
            op:
                Op::WaitForCaret {
                    subject: Subject::Node(node),
                    wait,
                },
            when: None,
        }) = steps.next()
        else {
            return None;
        };
        batch.steps = steps.collect();
        Some(Self {
            number,
            window,
            node,
            wait: *wait,
            after: batch,
            phase: Phase::Waiting { due_us },
        })
    }

    /// Its wait, the batch's first step.
    fn wait(&self) -> &CaretWait {
        &self.wait
    }

    /// Its batch whole, the wait first.
    fn batch(&self) -> Batch {
        let wait = Step {
            op: Op::WaitForCaret {
                subject: Subject::Node(self.node),
                wait: Box::new(self.wait.clone()),
            },
            when: None,
        };
        Batch {
            steps: std::iter::once(wait)
                .chain(self.after.steps.iter().cloned())
                .collect(),
            placement: self.after.placement,
            spare: self.after.spare,
        }
    }

    /// When the bound passes, on the run's shared clock.
    fn bound_at(&self) -> u64 {
        let wait = self.wait();
        wait.passed_at_us.saturating_add(u64::from(wait.bound_us))
    }

    /// Whether an event heard at `at_us` brings a check forward.
    fn event_counts(&self, at_us: u64) -> bool {
        let wait = self.wait();
        wait.events && at_us <= wait.passed_at_us.saturating_add(EVENT_WINDOW_US)
    }

    /// The next poll's due time after a polled check begun at `began_us`:
    /// a poll later, but no later than the bound.
    fn next_poll(&self, began_us: u64) -> u64 {
        began_us
            .saturating_add(u64::from(self.wait().poll_us))
            .min(self.bound_at())
    }

    /// When its next check is due, if one waits: one brought forward at
    /// the event's time.
    fn due_at(&self) -> Option<u64> {
        match self.phase {
            Phase::Waiting { due_us } => Some(due_us),
            Phase::Brought { heard_us, poll_us } => Some(heard_us.min(poll_us)),
            Phase::Checking { .. } => None,
        }
    }
}

/// What a cancel of a watch does.
#[derive(Debug, PartialEq, Eq)]
pub enum WatchCancelled {
    /// The cancel names no watch here.
    NotHere,
    /// The watch was removed and is answered so.
    Answered(Answer),
    /// Its check runs and the foreground moved away: the worker is
    /// abandoned and replaced, and the watch answered so.
    Abandon(Answer),
}

/// The watches waiting.
#[derive(Clone, Debug, Default)]
pub struct Watches {
    table: Vec<Watching>,
}

impl Watches {
    /// Whether no watch waits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Watch `number` in `window` arrives at `now_us`, its first check due
    /// now, when `batch`'s first step is a caret wait on a node
    /// ([`caret_wait`]) and it parses; `None` for any other batch, which
    /// the caller runs as any batch, and `Some(Err)` for a wait that does
    /// not parse, which is refused whole, as the worker refuses one.
    pub fn arrive(
        &mut self,
        (number, window): (RequestNumber, WindowHandle),
        batch: &Batch,
        now_us: u64,
    ) -> Option<Result<(), Malformed>> {
        caret_wait(batch)?;
        let parsed = match verbatim_model::batch::parse(batch.clone(), ReaderKind::Outpost) {
            Ok(parsed) => parsed,
            Err(malformed) => return Some(Err(malformed)),
        };
        // A parsed batch's first step has no condition, having no step
        // before it to name.
        let watching = Watching::new((number, window), parsed.into_batch(), now_us)?;
        self.table.push(watching);
        Some(Ok(()))
    }

    /// A caret or text-change event of the followed node was heard at
    /// `at_us`: within 60 ms of a watch's pass, where its event rule
    /// applies, it brings a check forward (Table 15, A).
    pub fn heard(&mut self, at_us: u64) {
        for watching in self.table.iter_mut().filter(|w| w.event_counts(at_us)) {
            watching.phase = match watching.phase {
                Phase::Waiting { due_us } => Phase::Brought {
                    heard_us: at_us,
                    poll_us: due_us,
                },
                Phase::Checking {
                    last,
                    poll_us,
                    then: Then::Poll,
                    gone,
                } => Phase::Checking {
                    last,
                    poll_us,
                    then: Then::Again(at_us),
                    gone,
                },
                kept @ (Phase::Brought { .. } | Phase::Checking { .. }) => kept,
            };
        }
    }

    /// The check due at `now_us`, if one is: the earliest due. The caller
    /// marks it running with [`running`](Self::running) and dispatches it.
    #[must_use]
    pub fn due(&self, now_us: u64) -> Option<WatchProgram> {
        let watching = self
            .table
            .iter()
            .filter_map(|w| w.due_at().filter(|due| *due <= now_us).map(|due| (due, w)))
            .min_by_key(|(due_us, _)| *due_us)
            .map(|(_, w)| w)?;
        let brought = matches!(watching.phase, Phase::Brought { .. });
        let wait = watching.wait();
        let word = wait.effect.unit
            && (brought || now_us >= wait.passed_at_us.saturating_add(WORD_AFTER_US));
        let last = now_us >= watching.bound_at();
        Some(WatchProgram {
            number: watching.number,
            node: watching.node,
            wait: wait.clone(),
            batch: watching.batch(),
            work: WatchWork::Check { word, last },
        })
    }

    /// `program`, from [`due`](Self::due), runs from `now_us`.
    pub fn running(&mut self, program: &WatchProgram, now_us: u64) {
        let Some(watching) = self.table.iter_mut().find(|w| w.number == program.number) else {
            return;
        };
        let poll_us = match watching.phase {
            Phase::Waiting { .. } => watching.next_poll(now_us),
            Phase::Brought { poll_us, .. } => poll_us,
            Phase::Checking { .. } => return,
        };
        watching.phase = Phase::Checking {
            last: matches!(program.work, WatchWork::Check { last: true, .. }),
            poll_us,
            then: Then::Poll,
            gone: false,
        };
    }

    /// When the next check comes due, on the run's shared clock: `None`
    /// with none waiting for a time.
    #[must_use]
    pub fn next_due(&self) -> Option<u64> {
        self.table.iter().filter_map(Watching::due_at).min()
    }

    /// The check of watch `number`, the program in flight, answers
    /// `answer`: the watch leaves the table, and the answer to send is
    /// returned. A check whose window was destroyed while it ran answers
    /// not there. `None` when the watch is no longer here, or the answer
    /// is another's.
    pub fn answered(&mut self, number: RequestNumber, answer: Answer) -> Option<Answer> {
        let place = self.table.iter().position(|w| {
            w.number == number
                && answer.number == number
                && matches!(w.phase, Phase::Checking { .. })
        })?;
        let watching = self.table.remove(place);
        if let Phase::Checking { gone: true, .. } = watching.phase {
            return Some(not_there(watching.number));
        }
        Some(answer)
    }

    /// The check of watch `number`, the program in flight, ended without
    /// answering: it showed no effect, or its read failed, and the watch
    /// goes on, its next check brought forward by an event heard while it
    /// ran, or else the next poll; a last check that answered nothing is
    /// answered failed.
    pub fn ended(&mut self, number: RequestNumber) -> Option<Answer> {
        let place = self.table.iter().position(|w| w.number == number)?;
        let watching = self.table.get_mut(place)?;
        let Phase::Checking {
            last,
            poll_us,
            then,
            gone,
        } = watching.phase
        else {
            return None;
        };
        if gone {
            self.table.remove(place);
            return Some(not_there(number));
        }
        if last {
            self.table.remove(place);
            return Some(failed(number));
        }
        watching.phase = match then {
            Then::Poll => Phase::Waiting { due_us: poll_us },
            Then::Again(heard_us) => Phase::Brought { heard_us, poll_us },
        };
        None
    }

    /// Core's `cancel` of a watch (Table 15, F).
    pub fn cancel(&mut self, cancel: Cancel) -> WatchCancelled {
        let Some(place) = self.table.iter().position(|w| w.number == cancel.number) else {
            return WatchCancelled::NotHere;
        };
        let watching = self.table.remove(place);
        match watching.phase {
            Phase::Checking { .. } if cancel.reason.abandons_call() => {
                WatchCancelled::Abandon(ended(cancel.number, "abandoned"))
            }
            // A check running commits nothing once its watch is gone.
            Phase::Waiting { .. } | Phase::Brought { .. } | Phase::Checking { .. } => {
                WatchCancelled::Answered(ended(cancel.number, "cancelled"))
            }
        }
    }

    /// Window `window` was destroyed: a watch on it that waits is answered
    /// not there; one whose check runs is answered so when the check ends
    /// (Table 15, H).
    pub fn destroyed(&mut self, window: WindowHandle) -> Vec<Answer> {
        let mut gone = Vec::new();
        self.table.retain_mut(|watching| {
            if watching.window != window {
                return true;
            }
            match watching.phase {
                Phase::Waiting { .. } | Phase::Brought { .. } => {
                    gone.push(not_there(watching.number));
                    false
                }
                Phase::Checking { ref mut gone, .. } => {
                    *gone = true;
                    true
                }
            }
        });
        gone
    }

    /// The anchors the waiting watches name, which a release spares
    /// (Table 15, I).
    pub fn anchors(&self) -> impl Iterator<Item = u64> + '_ {
        self.table.iter().map(|w| w.wait().from.0)
    }

    /// The outpost shuts down: every watch is dropped unanswered (Table
    /// 15, J).
    pub fn close(&mut self) {
        self.table.clear();
    }
}

/// The answer "ended or stopped" to watch `number`, for the log `detail`.
fn ended(number: RequestNumber, detail: &str) -> Answer {
    Answer {
        number,
        outcome: Outcome::Ended,
        probes: Vec::new(),
        detail: detail.to_owned(),
        began_us: 0,
        ended_us: 0,
    }
}

/// A watch whose last check failed is answered failed.
fn failed(number: RequestNumber) -> Answer {
    ended(number, "the outpost failed handling the request")
}

/// The answer "not there" to watch `number`: its window was destroyed.
fn not_there(number: RequestNumber) -> Answer {
    Answer {
        number,
        outcome: Outcome::NotThere,
        probes: Vec::new(),
        detail: "its window was destroyed".to_owned(),
        began_us: 0,
        ended_us: 0,
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::TextAnchor;
    use verbatim_model::batch::{Done, EffectTest, Placement, Step, StepOutcome, Waited};
    use verbatim_model::request::{CancelReason, Reply};

    use super::*;

    const PASS: u64 = 1_000_000;
    const WINDOW: WindowHandle = WindowHandle(5);

    /// A key, by the effect test its wait takes.
    #[derive(Clone, Copy)]
    enum CaretMotion {
        /// A key that moves the caret.
        NextCharacter,
        /// Delete, whose wait compares the unit at the caret and the line.
        Delete,
    }

    /// The wait of a key of `motion` on node 2 from anchor 9.
    fn watch(motion: CaretMotion, events: bool) -> Batch {
        let compares = matches!(motion, CaretMotion::Delete);
        Batch {
            steps: vec![Step {
                op: Op::WaitForCaret {
                    subject: Subject::Node(NodeId::new(2)),
                    wait: Box::new(CaretWait {
                        from: TextAnchor(9),
                        line: "alpha".to_owned(),
                        offset: 0,
                        word: compares.then(|| "alpha".to_owned()),
                        effect: EffectTest {
                            unit: compares,
                            line: compares,
                        },
                        passed_at_us: PASS,
                        poll_us: 10_000,
                        bound_us: 100_000,
                        events,
                    }),
                },
                when: None,
            }],
            placement: Placement::Queue,
            spare: None,
        }
    }

    /// The watch every test's table holds.
    const WATCH: RequestNumber = RequestNumber(4);

    /// A table with watch 4 arrived at the pass.
    fn arrived(motion: CaretMotion, events: bool) -> Watches {
        let mut watches = Watches::default();
        let batch = watch(motion, events);
        watches
            .arrive((RequestNumber(4), WINDOW), &batch, PASS)
            .expect("a wait on a node")
            .expect("a wait parses");
        watches
    }

    /// A table with watch 4 arrived at the pass, its first check run and
    /// ended with no effect, so it waits for its poll at
    /// 10 ms.
    fn polled(motion: CaretMotion, events: bool) -> Watches {
        let mut watches = arrived(motion, events);
        let _ = run(&mut watches, PASS);
        assert_eq!(watches.ended(WATCH), None);
        watches
    }

    /// Runs the program due at `at`, returning its work.
    fn run(watches: &mut Watches, at: u64) -> Option<WatchWork> {
        let due = watches.due(at)?;
        watches.running(&due, at);
        Some(due.work)
    }

    fn effect(number: u64) -> Answer {
        Answer {
            number: RequestNumber(number),
            outcome: Outcome::Result(Reply::Steps(vec![StepOutcome::Done(Done::WaitForCaret(
                Waited::Bound,
            ))])),

            probes: Vec::new(),
            detail: String::new(),
            began_us: 0,
            ended_us: 0,
        }
    }

    fn cancel(reason: CancelReason) -> Cancel {
        Cancel {
            number: RequestNumber(4),
            reason,
        }
    }

    const CHECK: WatchWork = WatchWork::Check {
        word: false,
        last: false,
    };

    /// W1, B: the first check is due at once, then one a poll after each
    /// check began; a check that shows nothing leaves the watch waiting.
    #[test]
    fn checks_run_at_once_and_then_a_poll_after_each_began() {
        let mut watches = arrived(CaretMotion::NextCharacter, false);
        assert_eq!(run(&mut watches, PASS), Some(CHECK));
        assert_eq!(watches.due(PASS), None, "one check at a time");
        assert_eq!(watches.ended(WATCH), None, "no effect: it waits");
        assert_eq!(watches.next_due(), Some(PASS + 10_000));
        assert_eq!(watches.due(PASS + 9_999), None);
        assert_eq!(run(&mut watches, PASS + 10_000), Some(CHECK));
    }

    /// W1, E: the check due at the bound is the last, and a check begun
    /// late is due no later than the bound.
    #[test]
    fn the_check_at_the_bound_is_the_last() {
        let mut watches = arrived(CaretMotion::NextCharacter, false);
        assert_eq!(run(&mut watches, PASS + 95_000), Some(CHECK));
        assert_eq!(watches.ended(WATCH), None);
        assert_eq!(watches.next_due(), Some(PASS + 100_000));
        assert_eq!(
            run(&mut watches, PASS + 100_000),
            Some(WatchWork::Check {
                word: false,
                last: true
            })
        );
        assert_eq!(
            watches.ended(WATCH),
            Some(failed(RequestNumber(4))),
            "a last check that answered nothing failed"
        );
        assert!(watches.is_empty());
    }

    /// Delete's polled checks compare the word at the caret once 30 ms
    /// have passed since the pass.
    #[test]
    fn deletes_checks_compare_the_word_after_30_ms() {
        let watches = polled(CaretMotion::Delete, false);
        assert_eq!(
            watches.due(PASS + 30_000).map(|program| program.work),
            Some(WatchWork::Check {
                word: true,
                last: false
            })
        );
    }

    /// W2, C: a check that answers ends the watch with its answer.
    #[test]
    fn a_check_that_answers_ends_the_watch() {
        let mut watches = arrived(CaretMotion::NextCharacter, false);
        let _ = run(&mut watches, PASS);
        assert_eq!(watches.answered(WATCH, effect(4)), Some(effect(4)));
        assert!(watches.is_empty());
        assert_eq!(watches.ended(WATCH), None);
    }

    /// W1, A: an event within 60 ms of the pass brings a check forward,
    /// due at once, and the poll after it keeps its time.
    #[test]
    fn an_event_brings_a_check_forward() {
        let mut watches = polled(CaretMotion::NextCharacter, true);
        watches.heard(PASS + 3_000);
        assert_eq!(watches.next_due(), Some(PASS + 3_000));
        assert_eq!(run(&mut watches, PASS + 3_000), Some(CHECK));
        assert_eq!(watches.ended(WATCH), None, "no effect: it waits");
        assert_eq!(watches.next_due(), Some(PASS + 10_000));
    }

    /// W2, A: an event within 60 ms heard while a check runs brings
    /// another check, due at once when it ends.
    #[test]
    fn an_event_during_a_check_brings_another_check_at_once() {
        let mut watches = arrived(CaretMotion::NextCharacter, true);
        let _ = run(&mut watches, PASS);
        watches.heard(PASS + 1);
        assert_eq!(watches.ended(WATCH), None);
        assert_eq!(watches.next_due(), Some(PASS + 1));
        assert_eq!(run(&mut watches, PASS + 2), Some(CHECK));
        assert_eq!(watches.ended(WATCH), None);
        assert_eq!(watches.next_due(), Some(PASS + 10_000));
    }

    /// A check an event brought forward compares the word at the caret,
    /// for Delete, before 30 ms have passed.
    #[test]
    fn an_events_check_compares_the_word() {
        let mut watches = polled(CaretMotion::Delete, true);
        watches.heard(PASS + 5_000);
        assert_eq!(
            run(&mut watches, PASS + 5_000),
            Some(WatchWork::Check {
                word: true,
                last: false
            })
        );
    }

    /// An event whose check finds no effect leaves the watch polling to
    /// the bound, where its last check answers.
    #[test]
    fn an_event_with_no_move_leaves_the_watch_polling_to_the_bound() {
        let mut watches = polled(CaretMotion::NextCharacter, true);
        watches.heard(PASS + 3_000);
        assert_eq!(run(&mut watches, PASS + 3_000), Some(CHECK));
        assert_eq!(watches.ended(WATCH), None);
        let mut began = Vec::new();
        while let Some(due) = watches.next_due() {
            let work = run(&mut watches, due);
            began.push(due - PASS);
            if work
                == Some(WatchWork::Check {
                    word: false,
                    last: true,
                })
            {
                break;
            }
            assert_eq!(watches.ended(WATCH), None);
        }
        assert_eq!(
            began,
            [
                10_000, 20_000, 30_000, 40_000, 50_000, 60_000, 70_000, 80_000, 90_000, 100_000
            ]
        );
        assert_eq!(watches.answered(WATCH, effect(4)), Some(effect(4)));
        assert!(watches.is_empty());
    }

    /// An event later than 60 ms after the pass changes nothing, whether
    /// the watch waits or its check runs.
    #[test]
    fn an_event_after_60_ms_changes_nothing() {
        let mut waiting = polled(CaretMotion::NextCharacter, true);
        waiting.heard(PASS + 60_001);
        assert_eq!(waiting.next_due(), Some(PASS + 10_000));
        let mut checking = polled(CaretMotion::NextCharacter, true);
        let _ = run(&mut checking, PASS + 60_000);
        checking.heard(PASS + 60_001);
        assert_eq!(checking.ended(WATCH), None);
        assert_eq!(checking.next_due(), Some(PASS + 70_000));
    }

    /// A watch whose request names no event rule ignores events.
    #[test]
    fn a_watch_without_the_event_rule_ignores_events() {
        let mut watches = polled(CaretMotion::NextCharacter, false);
        watches.heard(PASS + 1);
        assert_eq!(watches.next_due(), Some(PASS + 10_000));
    }

    /// W1, waiting or brought forward, F: removed and answered cancelled,
    /// with no call.
    #[test]
    fn a_waiting_watch_cancelled_is_answered_cancelled() {
        for reason in [CancelReason::Withdrawn, CancelReason::Foreground] {
            let mut watches = arrived(CaretMotion::NextCharacter, true);
            assert_eq!(
                watches.cancel(cancel(reason)),
                WatchCancelled::Answered(ended(RequestNumber(4), "cancelled"))
            );
            assert!(watches.is_empty());
            let mut brought = arrived(CaretMotion::NextCharacter, true);
            brought.heard(PASS + 1);
            assert_eq!(
                brought.cancel(cancel(reason)),
                WatchCancelled::Answered(ended(RequestNumber(4), "cancelled"))
            );
            assert!(brought.is_empty());
        }
    }

    /// W2, F: a check running is answered cancelled and commits nothing,
    /// or, as the foreground moved away, abandoned with its worker.
    #[test]
    fn a_check_running_cancelled_commits_nothing() {
        let mut watches = arrived(CaretMotion::NextCharacter, false);
        let _ = run(&mut watches, PASS);
        assert_eq!(
            watches.cancel(cancel(CancelReason::Withdrawn)),
            WatchCancelled::Answered(ended(RequestNumber(4), "cancelled"))
        );
        assert_eq!(watches.answered(WATCH, effect(4)), None);
        let mut foreground = arrived(CaretMotion::NextCharacter, false);
        let _ = run(&mut foreground, PASS);
        assert_eq!(
            foreground.cancel(cancel(CancelReason::Foreground)),
            WatchCancelled::Abandon(ended(RequestNumber(4), "abandoned"))
        );
    }

    /// H: a waiting watch, or one brought forward, whose window is
    /// destroyed is answered not there; one whose check runs is answered
    /// so when the check ends, whatever it found.
    #[test]
    fn a_destroyed_window_answers_its_watches_not_there() {
        let mut waiting = arrived(CaretMotion::NextCharacter, false);
        assert_eq!(waiting.destroyed(WINDOW), [not_there(RequestNumber(4))]);
        assert!(waiting.is_empty());
        let mut brought = arrived(CaretMotion::NextCharacter, true);
        brought.heard(PASS + 1);
        assert_eq!(brought.destroyed(WINDOW), [not_there(RequestNumber(4))]);
        assert!(brought.is_empty());
        let mut checking = arrived(CaretMotion::NextCharacter, false);
        let _ = run(&mut checking, PASS);
        assert_eq!(checking.destroyed(WINDOW), []);
        assert_eq!(
            checking.answered(WATCH, effect(4)),
            Some(not_there(RequestNumber(4)))
        );
        let mut nothing = arrived(CaretMotion::NextCharacter, false);
        let _ = run(&mut nothing, PASS);
        let _ = nothing.destroyed(WINDOW);
        assert_eq!(nothing.ended(WATCH), Some(not_there(RequestNumber(4))));
        assert_eq!(
            arrived(CaretMotion::NextCharacter, false).destroyed(WindowHandle(6)),
            []
        );
    }

    /// I and J: the waiting watches' starting points are spared by a
    /// release, and shutdown drops every watch unanswered.
    #[test]
    fn anchors_are_spared_and_shutdown_drops_every_watch() {
        let mut watches = arrived(CaretMotion::NextCharacter, false);
        assert_eq!(watches.anchors().collect::<Vec<_>>(), [9]);
        watches.close();
        assert!(watches.is_empty());
        assert_eq!(watches.due(PASS), None);
    }
}
