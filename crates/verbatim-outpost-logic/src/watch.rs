//! The watches in the outpost's waiting table
//! (`docs/design/readers-tell-core-asks-build1.md`, "The watch", Tables 14
//! and 15): a caret key's watch enters with its first check due now and
//! leaves when answered.
//!
//! A check compares the application's caret with the key's starting point
//! by the effect test (`verbatim_model::had_effect`), the first at once and
//! then one about `poll_us` after the last polled check began, until
//! `bound_us` after the pass; the check due at the bound is the last, and a
//! change it first sees counts as the effect, as NVDA's last try does
//! (`editableText.py` 124 to 152). A check that shows the effect, and the
//! last check whatever it shows, makes the final read in the same program
//! and answers.
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
//! failed, which counts as no effect; a last check that ends so, its final
//! read failed, is answered failed.

use verbatim_model::request::{Answer, Cancel, Outcome, RequestNumber};
use verbatim_model::{NodeId, Watch, WindowHandle};

use crate::charge::Request;

/// How long after the pass a UI Automation event brings a check forward:
/// NVDA's `_useEvents_maxTimeoutSec` (`editableText.py` 57).
pub const EVENT_WINDOW_US: u64 = 60_000;

/// How long after the pass Delete's polled check compares the word at the
/// caret: NVDA's `_hasCaretMoved_minWordTimeoutSec` (`editableText.py` 54).
pub const WORD_AFTER_US: u64 = 30_000;

/// What a watch's program does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchWork {
    /// Compare the caret with the starting point; with the effect, make
    /// the final read and answer [`verbatim_model::Watched::Effect`].
    Check {
        /// Whether to compare the word at the caret as well: Delete and
        /// Control+Delete, once 30 ms have passed since the pass or when
        /// an event brought the check forward.
        word: bool,
        /// Whether this is the check at the bound: with no effect, it
        /// makes the final read and answers
        /// [`verbatim_model::Watched::Bound`], or
        /// [`verbatim_model::Watched::BoundUnread`] for a final read only
        /// at the effect.
        last: bool,
    },
    /// Make the final read and answer [`verbatim_model::Watched::Effect`].
    /// The watch table never asks for it, as no event is the effect on its
    /// own.
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
    /// W2: a check runs as `request`; with no effect the next poll is due
    /// at `poll_us`.
    Checking {
        request: Request,
        last: bool,
        poll_us: u64,
        then: Then,
    },
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

    /// Whether an event heard at `at_us` brings a check forward.
    fn event_counts(&self, at_us: u64) -> bool {
        self.watch.events && at_us <= self.watch.passed_at_us.saturating_add(EVENT_WINDOW_US)
    }

    /// The next poll's due time after a polled check begun at `began_us`:
    /// a poll later, but no later than the bound.
    fn next_poll(&self, began_us: u64) -> u64 {
        began_us
            .saturating_add(u64::from(self.watch.poll_us))
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

    /// Watch `number` on `node` in `window` arrives at `now_us`: its first
    /// check due now.
    pub fn arrive(
        &mut self,
        (number, window, node): (RequestNumber, WindowHandle, NodeId),
        watch: Watch,
        now_us: u64,
    ) {
        self.table.push(Watching {
            number,
            window,
            node,
            watch,
            phase: Phase::Waiting { due_us: now_us },
            gone: false,
        });
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
                    request,
                    last,
                    poll_us,
                    then: Then::Poll,
                } => Phase::Checking {
                    request,
                    last,
                    poll_us,
                    then: Then::Again(at_us),
                },
                kept @ (Phase::Brought { .. } | Phase::Checking { .. }) => kept,
            };
        }
    }

    /// The check due at `now_us`, if one is: the earliest due. The caller
    /// dispatches it and names its request with [`running`](Self::running).
    #[must_use]
    pub fn due(&self, now_us: u64) -> Option<WatchProgram> {
        let watching = self
            .table
            .iter()
            .filter_map(|w| w.due_at().filter(|due| *due <= now_us).map(|due| (due, w)))
            .min_by_key(|(due_us, _)| *due_us)
            .map(|(_, w)| w)?;
        let brought = matches!(watching.phase, Phase::Brought { .. });
        let word = watching.watch.motion.compares_unit()
            && (brought || now_us >= watching.watch.passed_at_us.saturating_add(WORD_AFTER_US));
        let last = now_us >= watching.bound_at();
        Some(WatchProgram {
            number: watching.number,
            node: watching.node,
            watch: watching.watch,
            work: WatchWork::Check { word, last },
        })
    }

    /// `program`, from [`due`](Self::due), runs as `request` from `now_us`.
    pub fn running(&mut self, program: &WatchProgram, request: Request, now_us: u64) {
        let Some(watching) = self.table.iter_mut().find(|w| w.number == program.number) else {
            return;
        };
        let poll_us = match watching.phase {
            Phase::Waiting { .. } => watching.next_poll(now_us),
            Phase::Brought { poll_us, .. } => poll_us,
            Phase::Checking { .. } => return,
        };
        watching.phase = Phase::Checking {
            request,
            last: matches!(program.work, WatchWork::Check { last: true, .. }),
            poll_us,
            then: Then::Poll,
        };
    }

    /// When the next check comes due, on the run's shared clock: `None`
    /// with none waiting for a time.
    #[must_use]
    pub fn next_due(&self) -> Option<u64> {
        self.table.iter().filter_map(Watching::due_at).min()
    }

    /// Whether `request` is a watch's check.
    #[must_use]
    pub fn runs(&self, request: Request) -> bool {
        self.table
            .iter()
            .any(|w| running_of(w.phase) == Some(request))
    }

    /// Check `request` answers `answer`: the watch it runs leaves the
    /// table, and the answer to send is returned. A check whose window was
    /// destroyed while it ran answers not there.
    pub fn answered(&mut self, request: Request, answer: Answer) -> Option<Answer> {
        let place = self
            .table
            .iter()
            .position(|w| running_of(w.phase) == Some(request) && w.number == answer.number)?;
        let watching = self.table.remove(place);
        if watching.gone {
            return Some(not_there(watching.number));
        }
        Some(answer)
    }

    /// Check `request` ended without answering: it showed no effect, or
    /// its read failed, and the watch goes on, its next check brought
    /// forward by an event heard while it ran, or else the next poll; a
    /// last check that answered nothing is answered failed.
    pub fn ended(&mut self, request: Request) -> Option<Answer> {
        let (place, last, poll_us, then) =
            self.table
                .iter()
                .enumerate()
                .find_map(|(place, w)| match w.phase {
                    Phase::Checking {
                        request: running,
                        last,
                        poll_us,
                        then,
                    } if running == request => Some((place, last, poll_us, then)),
                    _ => None,
                })?;
        let watching = self.table.get_mut(place)?;
        let number = watching.number;
        if watching.gone {
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
                Phase::Checking { .. } => {
                    watching.gone = true;
                    true
                }
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
}

/// The check a watch in `phase` runs, if one runs.
fn running_of(phase: Phase) -> Option<Request> {
    match phase {
        Phase::Checking { request, .. } => Some(request),
        Phase::Waiting { .. } | Phase::Brought { .. } => None,
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
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::request::{CancelReason, Reply};
    use verbatim_model::{
        CaretMotion, TextAnchor, TextDetails, TextReply, TextUnit, WatchFinalRead, Watched,
    };

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

    /// A table with watch 4 arrived at the pass, its first check run as
    /// request 1 and ended with no effect, so it waits for its poll at
    /// 10 ms.
    fn polled(motion: CaretMotion, events: bool) -> Watches {
        let mut watches = arrived(motion, events);
        let _ = run(&mut watches, PASS, 1);
        assert_eq!(watches.ended(request(1)), None);
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
        let _ = run(&mut watches, PASS, 1);
        assert!(watches.runs(request(1)));
        assert_eq!(watches.answered(request(1), effect(4)), Some(effect(4)));
        assert!(watches.is_empty());
        assert_eq!(watches.ended(request(1)), None);
    }

    /// W1, A: an event within 60 ms of the pass brings a check forward,
    /// due at once, and the poll after it keeps its time.
    #[test]
    fn an_event_brings_a_check_forward() {
        let mut watches = polled(CaretMotion::NextCharacter, true);
        watches.heard(PASS + 3_000);
        assert_eq!(watches.next_due(), Some(PASS + 3_000));
        assert_eq!(run(&mut watches, PASS + 3_000, 2), Some(CHECK));
        assert_eq!(watches.ended(request(2)), None, "no effect: it waits");
        assert_eq!(watches.next_due(), Some(PASS + 10_000));
    }

    /// W2, A: an event within 60 ms heard while a check runs brings
    /// another check, due at once when it ends.
    #[test]
    fn an_event_during_a_check_brings_another_check_at_once() {
        let mut watches = arrived(CaretMotion::NextCharacter, true);
        let _ = run(&mut watches, PASS, 1);
        watches.heard(PASS + 1);
        assert_eq!(watches.ended(request(1)), None);
        assert_eq!(watches.next_due(), Some(PASS + 1));
        assert_eq!(run(&mut watches, PASS + 2, 2), Some(CHECK));
        assert_eq!(watches.ended(request(2)), None);
        assert_eq!(watches.next_due(), Some(PASS + 10_000));
    }

    /// A check an event brought forward compares the word at the caret,
    /// for Delete, before 30 ms have passed.
    #[test]
    fn an_events_check_compares_the_word() {
        let mut watches = polled(CaretMotion::Delete, true);
        watches.heard(PASS + 5_000);
        assert_eq!(
            run(&mut watches, PASS + 5_000, 2),
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
        assert_eq!(run(&mut watches, PASS + 3_000, 2), Some(CHECK));
        assert_eq!(watches.ended(request(2)), None);
        let mut began = Vec::new();
        let mut program = 3;
        while let Some(due) = watches.next_due() {
            let work = run(&mut watches, due, program);
            began.push(due - PASS);
            if work
                == Some(WatchWork::Check {
                    word: false,
                    last: true,
                })
            {
                break;
            }
            assert_eq!(watches.ended(request(program)), None);
            program += 1;
        }
        assert_eq!(
            began,
            [
                10_000, 20_000, 30_000, 40_000, 50_000, 60_000, 70_000, 80_000, 90_000, 100_000
            ]
        );
        assert_eq!(
            watches.answered(request(program), effect(4)),
            Some(effect(4))
        );
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
        let _ = run(&mut checking, PASS + 60_000, 2);
        checking.heard(PASS + 60_001);
        assert_eq!(checking.ended(request(2)), None);
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
