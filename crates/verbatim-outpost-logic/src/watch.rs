//! The watches in the outpost's waiting table
//! (`docs/design/readers-tell-core-asks-build1.md`, "The watch", Tables 14
//! and 15): a caret key's watch enters with its first check due now and
//! leaves when answered.
//!
//! A check compares the application's caret with the key's starting point
//! by the effect test (`verbatim_model::had_effect`), the first at once and
//! then one about `poll_us` after the last began, until `bound_us` after
//! the pass; the check due at the bound is the last, and a change it first
//! sees counts as the effect, as NVDA's last try does (`editableText.py`
//! 124 to 152). A check that shows the effect, and the last check
//! whatever it shows, makes the final read in the same program and
//! answers. Where the request says the event rule applies, a caret or
//! text-change event of the watched node heard within 60 ms of the pass,
//! or while the key's starting read was out, is the effect: the final read
//! is made at once. A later event changes nothing: NVDA looks at events
//! only within those 60 ms and otherwise checks on its 10 ms poll
//! (`editableText.py` 104 to 161).
//!
//! The watched node's events are the ones the outpost's `Follow`
//! subscription hears: it is registered on the followed node alone, and
//! Core names the event rule only for the committed focus, which it
//! follows.
//!
//! A program that ends without answering is a check whose read failed,
//! which counts as no effect, or a final read that failed, answered
//! failed.

use verbatim_model::request::{Answer, Cancel, Outcome, RequestNumber};
use verbatim_model::{NodeId, TextAnchor, Watch, WindowHandle};

use crate::charge::Request;

/// How long after the pass a UI Automation event is the key's effect: NVDA's
/// `_useEvents_maxTimeoutSec` (`editableText.py` 57).
pub const EVENT_WINDOW_US: u64 = 60_000;

/// How long after the pass Delete's check compares the word at the caret:
/// NVDA's `_hasCaretMoved_minWordTimeoutSec` (`editableText.py` 54).
pub const WORD_AFTER_US: u64 = 30_000;

/// What a watch's program does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchWork {
    /// Compare the caret with the starting point; with the effect, make
    /// the final read and answer [`verbatim_model::Watched::Effect`].
    Check {
        /// Whether to compare the word at the caret as well: Delete and
        /// Control+Delete, once 30 ms have passed since the pass.
        word: bool,
        /// Whether this is the check at the bound: with no effect, it
        /// makes the final read and answers
        /// [`verbatim_model::Watched::Bound`], or
        /// [`verbatim_model::Watched::BoundUnread`] for a final read only
        /// at the effect.
        last: bool,
    },
    /// The effect is known, from an event: make the final read and answer
    /// [`verbatim_model::Watched::Effect`].
    Final,
}

/// A watch's program, for the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchProgram {
    /// The watch's request number, which its answer names.
    pub number: RequestNumber,
    /// The watched node.
    pub node: NodeId,
    /// The watch.
    pub watch: Watch,
    /// What the program does.
    pub work: WatchWork,
}

/// What an event heard during a check means for it (Table 15, W2, A).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Noted {
    /// Nothing heard.
    Nothing,
    /// An event within 60 ms of the pass: the key's effect, whatever the
    /// check shows.
    Effect,
}

/// A watch's state (Table 15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// W1: its next check due at `due_us`, on the run's shared clock.
    Waiting { due_us: u64 },
    /// W2: a check runs as `request`, begun at `began_us`.
    Checking {
        request: Request,
        began_us: u64,
        last: bool,
        noted: Noted,
    },
    /// W3: its final read at the effect, queued, or running as `request`.
    Final { running: Option<Request> },
}

/// One watch in the table.
#[derive(Clone, Debug)]
struct Watching {
    number: RequestNumber,
    window: WindowHandle,
    node: NodeId,
    watch: Watch,
    phase: Phase,
    /// Its window was destroyed while its check ran: answered not there
    /// when the check ends (Table 15, W2, H).
    gone: bool,
}

impl Watching {
    /// When the bound passes, on the run's shared clock.
    fn bound_at(&self) -> u64 {
        self.watch
            .passed_at_us
            .saturating_add(u64::from(self.watch.bound_us))
    }

    /// Whether an event heard at `at_us` is the key's effect.
    fn event_is_effect(&self, at_us: u64) -> bool {
        at_us <= self.watch.passed_at_us.saturating_add(EVENT_WINDOW_US)
    }

    /// The next check's due time after a check begun at `began_us` that
    /// showed none: a poll later, but no later than the bound.
    fn next_due(&self, began_us: u64) -> u64 {
        began_us
            .saturating_add(u64::from(self.watch.poll_us))
            .min(self.bound_at())
    }
}

/// The latest starting read asked: its number, when it was asked, and the
/// starting point its answer minted, for an event heard while it was out
/// (Table 15, A: "including one heard during the key's starting read").
#[derive(Clone, Copy, Debug)]
struct Starting {
    number: RequestNumber,
    asked_us: u64,
    point: Option<TextAnchor>,
}

/// What a cancel of a watch does.
#[derive(Debug, PartialEq, Eq)]
pub enum WatchCancelled {
    /// The cancel names no watch here.
    NotHere,
    /// The watch was removed and is answered so.
    Answered(Answer),
    /// Its program runs and the foreground moved away: the worker is
    /// abandoned and replaced, and the watch answered so.
    Abandon(Answer),
    /// Its final read runs on, its answer sent when it ends.
    Nothing,
}

/// The watches waiting, and what their event rule reads.
#[derive(Clone, Debug, Default)]
pub struct Watches {
    table: Vec<Watching>,
    starting: Option<Starting>,
    /// When an event of the followed node was last heard.
    heard_us: Option<u64>,
}

impl Watches {
    /// Whether no watch waits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// A starting read, request `number`, was asked at `at_us`.
    pub fn starting_asked(&mut self, number: RequestNumber, at_us: u64) {
        self.starting = Some(Starting {
            number,
            asked_us: at_us,
            point: None,
        });
    }

    /// Starting read `number` minted `point`.
    pub fn starting_answered(&mut self, number: RequestNumber, point: TextAnchor) {
        if let Some(starting) = self.starting.as_mut().filter(|s| s.number == number) {
            starting.point = Some(point);
        }
    }

    /// Watch `number` on `node` in `window` arrives at `now_us`: its first
    /// check due now, or, where the event rule applies and an event was
    /// heard since its key's starting read was asked, its final read.
    pub fn arrive(
        &mut self,
        (number, window, node): (RequestNumber, WindowHandle, NodeId),
        watch: Watch,
        now_us: u64,
    ) {
        let heard_while_starting = watch.events
            && self.starting.is_some_and(|starting| {
                starting.point == Some(watch.point)
                    && self.heard_us.is_some_and(|heard| heard > starting.asked_us)
            });
        let phase = if heard_while_starting {
            Phase::Final { running: None }
        } else {
            Phase::Waiting { due_us: now_us }
        };
        self.table.push(Watching {
            number,
            window,
            node,
            watch,
            phase,
            gone: false,
        });
    }

    /// A caret or text-change event of the followed node was heard at
    /// `at_us` (Table 15, A).
    pub fn heard(&mut self, at_us: u64) {
        self.heard_us = Some(at_us);
        for watching in self
            .table
            .iter_mut()
            .filter(|w| w.watch.events && w.event_is_effect(at_us))
        {
            watching.phase = match watching.phase {
                Phase::Waiting { .. } => Phase::Final { running: None },
                Phase::Checking {
                    request,
                    began_us,
                    last,
                    ..
                } => Phase::Checking {
                    request,
                    began_us,
                    last,
                    noted: Noted::Effect,
                },
                final_read @ Phase::Final { .. } => final_read,
            };
        }
    }

    /// The program due at `now_us`, if one is: a final read ahead of a
    /// check. The caller dispatches it and names its request with
    /// [`running`](Self::running).
    #[must_use]
    pub fn due(&self, now_us: u64) -> Option<WatchProgram> {
        let program = |watching: &Watching, work| WatchProgram {
            number: watching.number,
            node: watching.node,
            watch: watching.watch,
            work,
        };
        if let Some(watching) = self
            .table
            .iter()
            .find(|w| w.phase == Phase::Final { running: None })
        {
            return Some(program(watching, WatchWork::Final));
        }
        let watching = self
            .table
            .iter()
            .filter_map(|w| match w.phase {
                Phase::Waiting { due_us } if due_us <= now_us => Some((due_us, w)),
                _ => None,
            })
            .min_by_key(|(due_us, _)| *due_us)
            .map(|(_, w)| w)?;
        let word = watching.watch.motion.compares_unit()
            && now_us >= watching.watch.passed_at_us.saturating_add(WORD_AFTER_US);
        let last = now_us >= watching.bound_at();
        Some(program(watching, WatchWork::Check { word, last }))
    }

    /// `program`, from [`due`](Self::due), runs as `request` from `now_us`.
    pub fn running(&mut self, program: &WatchProgram, request: Request, now_us: u64) {
        let Some(watching) = self.find(program.number) else {
            return;
        };
        watching.phase = match program.work {
            WatchWork::Final => Phase::Final {
                running: Some(request),
            },
            WatchWork::Check { last, .. } => Phase::Checking {
                request,
                began_us: now_us,
                last,
                noted: Noted::Nothing,
            },
        };
    }

    /// When the next check comes due, on the run's shared clock: `None`
    /// with none waiting for a time.
    #[must_use]
    pub fn next_due(&self) -> Option<u64> {
        self.table
            .iter()
            .filter_map(|w| match w.phase {
                Phase::Waiting { due_us } => Some(due_us),
                Phase::Final { running: None } => Some(0),
                _ => None,
            })
            .min()
    }

    /// Whether `request` is a watch's program.
    #[must_use]
    pub fn runs(&self, request: Request) -> bool {
        self.table
            .iter()
            .any(|w| running_of(w.phase) == Some(request))
    }

    /// Program `request` answers `answer`: the watch it runs leaves the
    /// table, and the answer to send is returned. A check whose window was
    /// destroyed while it ran answers not there.
    pub fn answered(&mut self, request: Request, answer: Answer) -> Option<Answer> {
        let place = self
            .table
            .iter()
            .position(|w| running_of(w.phase) == Some(request) && w.number == answer.number)?;
        let watching = self.table.remove(place);
        if watching.gone && matches!(watching.phase, Phase::Checking { .. }) {
            return Some(not_there(watching.number));
        }
        Some(answer)
    }

    /// Program `request` ended without answering: a check
    /// showed no effect, or its read failed, and the watch goes on, its
    /// final read next after an event within 60 ms; a last check or a
    /// final read that answered nothing is answered failed.
    pub fn ended(&mut self, request: Request) -> Option<Answer> {
        let place = self
            .table
            .iter()
            .position(|w| running_of(w.phase) == Some(request))?;
        let watching = self.table.get_mut(place)?;
        match watching.phase {
            Phase::Checking {
                began_us,
                last,
                noted,
                ..
            } if !watching.gone => {
                watching.phase = match noted {
                    Noted::Effect => Phase::Final { running: None },
                    Noted::Nothing if last => {
                        let number = watching.number;
                        self.table.remove(place);
                        return Some(failed(number));
                    }
                    Noted::Nothing => Phase::Waiting {
                        due_us: watching.next_due(began_us),
                    },
                };
                None
            }
            Phase::Checking { .. } => {
                let number = watching.number;
                self.table.remove(place);
                Some(not_there(number))
            }
            Phase::Final { .. } | Phase::Waiting { .. } => {
                let number = watching.number;
                self.table.remove(place);
                Some(failed(number))
            }
        }
    }

    /// Core's `cancel` of a watch (Table 15, F).
    pub fn cancel(&mut self, cancel: Cancel) -> WatchCancelled {
        let Some(place) = self.table.iter().position(|w| w.number == cancel.number) else {
            return WatchCancelled::NotHere;
        };
        let Some(watching) = self.table.get(place) else {
            return WatchCancelled::NotHere;
        };
        match watching.phase {
            Phase::Checking { .. } | Phase::Final { running: Some(_) }
                if cancel.reason.abandons_call() =>
            {
                self.table.remove(place);
                WatchCancelled::Abandon(ended(cancel.number, "abandoned"))
            }
            // A check running commits nothing once its watch is gone.
            Phase::Waiting { .. } | Phase::Final { running: None } | Phase::Checking { .. } => {
                self.table.remove(place);
                WatchCancelled::Answered(ended(cancel.number, "cancelled"))
            }
            Phase::Final { running: Some(_) } => WatchCancelled::Nothing,
        }
    }

    /// Window `window` was destroyed: a watch on it that waits is answered
    /// not there; one whose check runs is answered so when the check ends;
    /// a final read running answers what it found (Table 15, H).
    pub fn destroyed(&mut self, window: WindowHandle) -> Vec<Answer> {
        let mut gone = Vec::new();
        self.table.retain_mut(|watching| {
            if watching.window != window {
                return true;
            }
            match watching.phase {
                Phase::Waiting { .. } | Phase::Final { running: None } => {
                    gone.push(not_there(watching.number));
                    false
                }
                Phase::Checking { .. } => {
                    watching.gone = true;
                    true
                }
                Phase::Final { running: Some(_) } => true,
            }
        });
        gone
    }

    /// The anchors the waiting watches name, which a release spares
    /// (Table 15, I).
    pub fn anchors(&self) -> impl Iterator<Item = u64> + '_ {
        self.table.iter().map(|w| w.watch.point.0)
    }

    /// The outpost shuts down: every watch is dropped unanswered (Table
    /// 15, J).
    pub fn close(&mut self) {
        self.table.clear();
    }

    fn find(&mut self, number: RequestNumber) -> Option<&mut Watching> {
        self.table.iter_mut().find(|w| w.number == number)
    }
}

/// The program a watch in `phase` runs, if one runs.
fn running_of(phase: Phase) -> Option<Request> {
    match phase {
        Phase::Checking { request, .. }
        | Phase::Final {
            running: Some(request),
        } => Some(request),
        Phase::Waiting { .. } | Phase::Final { running: None } => None,
    }
}

/// The answer "ended or stopped" to watch `number`, for the log `detail`.
fn ended(number: RequestNumber, detail: &str) -> Answer {
    Answer {
        number,
        outcome: Outcome::Ended,
        probes: Vec::new(),
        detail: detail.to_owned(),
    }
}

/// A watch whose program failed is answered failed.
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
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::request::{CancelReason, Reply};
    use verbatim_model::{CaretMotion, TextDetails, TextReply, TextUnit, WatchFinalRead, Watched};

    use super::*;

    const PASS: u64 = 1_000_000;
    const WINDOW: WindowHandle = WindowHandle(5);

    fn watch(motion: CaretMotion, events: bool) -> Watch {
        Watch {
            provider: verbatim_model::TextProvider::Uia,
            point: TextAnchor(9),
            motion,
            passed_at_us: PASS,
            poll_us: 10_000,
            events,
            bound_us: 100_000,
            final_read: WatchFinalRead::Unit {
                unit: TextUnit::Character,
                details: TextDetails::ALL,
            },
        }
    }

    fn request(number: u64) -> Request {
        Request {
            generation: 0,
            number,
        }
    }

    /// A table with watch 4 arrived at the pass.
    fn arrived(motion: CaretMotion, events: bool) -> Watches {
        let mut watches = Watches::default();
        watches.arrive(
            (RequestNumber(4), WINDOW, NodeId::new(2)),
            watch(motion, events),
            PASS,
        );
        watches
    }

    /// Runs the program due at `at` as `program`, returning its work.
    fn run(watches: &mut Watches, at: u64, program: u64) -> Option<WatchWork> {
        let due = watches.due(at)?;
        watches.running(&due, request(program), at);
        Some(due.work)
    }

    fn effect(number: u64) -> Answer {
        Answer {
            number: RequestNumber(number),
            outcome: Outcome::Result(Reply::Text(TextReply::Watched(Box::new(
                Watched::BoundUnread,
            )))),
            probes: Vec::new(),
            detail: String::new(),
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
        assert_eq!(run(&mut watches, PASS, 1), Some(CHECK));
        assert_eq!(watches.due(PASS), None, "one check at a time");
        assert_eq!(watches.ended(request(1)), None, "no effect: it waits");
        assert_eq!(watches.next_due(), Some(PASS + 10_000));
        assert_eq!(watches.due(PASS + 9_999), None);
        assert_eq!(run(&mut watches, PASS + 10_000, 2), Some(CHECK));
    }

    /// W1, E: the check due at the bound is the last, and a check begun
    /// late is due no later than the bound.
    #[test]
    fn the_check_at_the_bound_is_the_last() {
        let mut watches = arrived(CaretMotion::NextCharacter, false);
        assert_eq!(run(&mut watches, PASS + 95_000, 1), Some(CHECK));
        assert_eq!(watches.ended(request(1)), None);
        assert_eq!(watches.next_due(), Some(PASS + 100_000));
        assert_eq!(
            run(&mut watches, PASS + 100_000, 2),
            Some(WatchWork::Check {
                word: false,
                last: true
            })
        );
        assert_eq!(
            watches.ended(request(2)),
            Some(failed(RequestNumber(4))),
            "a last check that answered nothing failed"
        );
        assert!(watches.is_empty());
    }

    /// Delete's checks compare the word at the caret once 30 ms have
    /// passed since the pass.
    #[test]
    fn deletes_checks_compare_the_word_after_30_ms() {
        let mut watches = arrived(CaretMotion::Delete, false);
        assert_eq!(run(&mut watches, PASS, 1), Some(CHECK));
        assert_eq!(watches.ended(request(1)), None);
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
        let _ = run(&mut watches, PASS, 1);
        assert!(watches.runs(request(1)));
        assert_eq!(watches.answered(request(1), effect(4)), Some(effect(4)));
        assert!(watches.is_empty());
        assert_eq!(watches.ended(request(1)), None);
    }

    /// W1, A: an event within 60 ms of the pass is the effect, its final
    /// read next; one later changes nothing.
    #[test]
    fn an_event_within_60_ms_is_the_effect_and_a_later_one_nothing() {
        let mut watches = arrived(CaretMotion::NextCharacter, true);
        watches.heard(PASS + 60_000);
        assert_eq!(run(&mut watches, PASS + 60_000, 1), Some(WatchWork::Final));
        let mut late = arrived(CaretMotion::NextCharacter, true);
        let _ = run(&mut late, PASS, 1);
        assert_eq!(late.ended(request(1)), None);
        late.heard(PASS + 60_001);
        assert_eq!(late.next_due(), Some(PASS + 10_000));
    }

    /// A watch whose request names no event rule ignores events.
    #[test]
    fn a_watch_without_the_event_rule_ignores_events() {
        let mut watches = arrived(CaretMotion::NextCharacter, false);
        watches.heard(PASS + 1);
        assert_eq!(run(&mut watches, PASS + 1, 1), Some(CHECK));
    }

    /// W2, A: an event within 60 ms heard while a check runs is the
    /// effect whatever the check shows: its final read follows.
    #[test]
    fn an_event_during_a_check_brings_the_final_read_after_it() {
        let mut watches = arrived(CaretMotion::NextCharacter, true);
        let _ = run(&mut watches, PASS, 1);
        watches.heard(PASS + 1);
        assert_eq!(watches.ended(request(1)), None);
        assert_eq!(run(&mut watches, PASS + 2, 2), Some(WatchWork::Final));
    }

    /// An event heard after the key's starting read was asked, while it
    /// was out, is the effect as the watch arrives; one heard before it was
    /// asked is not.
    #[test]
    fn an_event_heard_while_the_starting_read_was_out_is_the_effect() {
        let mut watches = Watches::default();
        watches.heard(PASS - 2_000);
        watches.starting_asked(RequestNumber(3), PASS - 1_500);
        watches.heard(PASS - 1_000);
        watches.starting_answered(RequestNumber(3), TextAnchor(9));
        watches.arrive(
            (RequestNumber(4), WINDOW, NodeId::new(2)),
            watch(CaretMotion::NextCharacter, true),
            PASS,
        );
        assert_eq!(run(&mut watches, PASS, 1), Some(WatchWork::Final));

        let mut before = Watches::default();
        before.heard(PASS - 2_000);
        before.starting_asked(RequestNumber(3), PASS - 1_500);
        before.starting_answered(RequestNumber(3), TextAnchor(9));
        before.arrive(
            (RequestNumber(4), WINDOW, NodeId::new(2)),
            watch(CaretMotion::NextCharacter, true),
            PASS,
        );
        assert_eq!(run(&mut before, PASS, 1), Some(CHECK));
    }

    /// W1 and W3 queued, F: removed and answered cancelled, with no call.
    #[test]
    fn a_waiting_watch_cancelled_is_answered_cancelled() {
        for reason in [CancelReason::Withdrawn, CancelReason::Foreground] {
            let mut watches = arrived(CaretMotion::NextCharacter, true);
            assert_eq!(
                watches.cancel(cancel(reason)),
                WatchCancelled::Answered(ended(RequestNumber(4), "cancelled"))
            );
            assert!(watches.is_empty());
            let mut queued = arrived(CaretMotion::NextCharacter, true);
            queued.heard(PASS + 1);
            assert_eq!(
                queued.cancel(cancel(reason)),
                WatchCancelled::Answered(ended(RequestNumber(4), "cancelled"))
            );
        }
    }

    /// W2, F: a check running is answered cancelled and commits nothing,
    /// or, as the foreground moved away, abandoned with its worker.
    #[test]
    fn a_check_running_cancelled_commits_nothing() {
        let mut watches = arrived(CaretMotion::NextCharacter, false);
        let _ = run(&mut watches, PASS, 1);
        assert_eq!(
            watches.cancel(cancel(CancelReason::Withdrawn)),
            WatchCancelled::Answered(ended(RequestNumber(4), "cancelled"))
        );
        assert_eq!(watches.answered(request(1), effect(4)), None);
        let mut foreground = arrived(CaretMotion::NextCharacter, false);
        let _ = run(&mut foreground, PASS, 1);
        assert_eq!(
            foreground.cancel(cancel(CancelReason::Foreground)),
            WatchCancelled::Abandon(ended(RequestNumber(4), "abandoned"))
        );
    }

    /// W3 running, F: the final read runs on, its answer sent when it
    /// ends; as the foreground moved away it is abandoned.
    #[test]
    fn a_final_read_running_cancelled_runs_on() {
        let mut watches = arrived(CaretMotion::NextCharacter, true);
        watches.heard(PASS + 1);
        let _ = run(&mut watches, PASS + 1, 1);
        assert_eq!(
            watches.cancel(cancel(CancelReason::Withdrawn)),
            WatchCancelled::Nothing
        );
        assert_eq!(watches.answered(request(1), effect(4)), Some(effect(4)));
        let mut foreground = arrived(CaretMotion::NextCharacter, true);
        foreground.heard(PASS + 1);
        let _ = run(&mut foreground, PASS + 1, 1);
        assert_eq!(
            foreground.cancel(cancel(CancelReason::Foreground)),
            WatchCancelled::Abandon(ended(RequestNumber(4), "abandoned"))
        );
    }

    /// A final read that ends without answering is answered failed.
    #[test]
    fn a_final_read_that_answers_nothing_fails() {
        let mut watches = arrived(CaretMotion::NextCharacter, true);
        watches.heard(PASS + 1);
        let _ = run(&mut watches, PASS + 1, 1);
        assert_eq!(watches.ended(request(1)), Some(failed(RequestNumber(4))));
    }

    /// H: a waiting watch whose window is destroyed is answered not there;
    /// one whose check runs is answered so when the check ends, whatever
    /// it found; a final read running answers what it found.
    #[test]
    fn a_destroyed_window_answers_its_watches_not_there() {
        let mut waiting = arrived(CaretMotion::NextCharacter, false);
        assert_eq!(waiting.destroyed(WINDOW), [not_there(RequestNumber(4))]);
        assert!(waiting.is_empty());
        let mut checking = arrived(CaretMotion::NextCharacter, false);
        let _ = run(&mut checking, PASS, 1);
        assert_eq!(checking.destroyed(WINDOW), []);
        assert_eq!(
            checking.answered(request(1), effect(4)),
            Some(not_there(RequestNumber(4)))
        );
        let mut nothing = arrived(CaretMotion::NextCharacter, false);
        let _ = run(&mut nothing, PASS, 1);
        let _ = nothing.destroyed(WINDOW);
        assert_eq!(nothing.ended(request(1)), Some(not_there(RequestNumber(4))));
        let mut reading = arrived(CaretMotion::NextCharacter, true);
        reading.heard(PASS + 1);
        let _ = run(&mut reading, PASS + 1, 1);
        assert_eq!(reading.destroyed(WINDOW), []);
        assert_eq!(reading.answered(request(1), effect(4)), Some(effect(4)));
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
