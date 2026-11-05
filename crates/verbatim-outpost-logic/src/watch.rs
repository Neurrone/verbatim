//! The caret key's watch (`books/dev/src/explanation/design/key-holding.md`): the outpost's open
//! check for a key's evidence, held as one state. The reducer alone decides
//! its times, on the run's shared clock; each program's copy applies the
//! same transitions with no clock, its times left at zero, since no
//! program reads them.

use std::ops::AddAssign;
use std::time::Duration;

use verbatim_model::protocol::{EventTiming, RequestId};
use verbatim_model::{CallCounts, CaretWatch, NodeId, TextReply, TraceId};

/// The longest a caret key's watch stays open without evidence. It only
/// frees the watch: ending it this way says nothing, as ending it any other
/// way does. A key whose application raises its evidence later than this
/// is not spoken. Ten seconds, the time a program allows an application
/// that is busy, or starting up, to answer one event's reads.
pub const CARET_WATCH_BOUND: Duration = Duration::from_secs(10);

/// How long after one of the console poll's reads the next is made.
pub const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long after the key was passed on the console poll's last read is
/// made; a watch that has found no evidence by then ends, as at its own
/// bound.
pub const POLL_BOUND: Duration = Duration::from_millis(50);

/// What a caret key's watch is: what it answers and looks for, and what its
/// checks have cost so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenWatch {
    /// The request it answers.
    pub request_id: RequestId,
    /// The key's trace.
    pub trace: TraceId,
    /// The node whose caret the key moves.
    pub node_id: NodeId,
    /// What the watch looks for, and what it has seen.
    pub watch: CaretWatch,
    /// The request's timing, which its answer carries.
    pub timing: EventTiming,
    /// The calls its checks have made so far, none of which found evidence.
    pub calls: CallCounts,
}

/// The caret key's watch, in one of its four states. Times are
/// microseconds on the run's shared clock; zero in a program's copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Watch {
    /// No watch.
    #[default]
    None,
    /// Waiting for evidence, which its node's events check; it ends at
    /// `bound` ([`CARET_WATCH_BOUND`] after it opened).
    Waiting {
        /// The watch.
        open: OpenWatch,
        /// When it ends without evidence.
        bound: u64,
    },
    /// The console host's poll (`books/dev/src/explanation/design/key-holding.md`, "The console
    /// host's caret poll"): a read due at `next`, the last at `until`.
    Polling {
        /// The watch.
        open: OpenWatch,
        /// When the next read is due.
        next: u64,
        /// When the last read is due.
        until: u64,
    },
    /// The program in flight holds it, by a take or as a poll read the
    /// reducer dispatched; `resume` is what it goes back to.
    Checking {
        /// The watch.
        open: OpenWatch,
        /// What it goes back to when the check finds nothing.
        resume: Resume,
    },
}

/// What a watch being checked goes back to when the check finds nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    /// Waiting, until this bound.
    Bound(u64),
    /// Polling, as it stood.
    Poll {
        /// When the next read is due.
        next: u64,
        /// When the last read is due.
        until: u64,
    },
    /// The poll's next step after a poll read: its next read, or its end
    /// once `until` has passed.
    Read {
        /// When the last read is due.
        until: u64,
    },
}

/// What a program's check of the watch it holds found: the one result of
/// every check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checked {
    /// The key's evidence, as the key's reply, with the calls the check
    /// made and when the watch's waiting ended.
    Answered {
        /// The key's reply.
        reply: TextReply,
        /// The calls the check made.
        calls: CallCounts,
        /// When the watch's waiting ended, on the run's shared clock.
        awaited_at_us: u64,
    },
    /// No evidence, with the calls the check made.
    Watching {
        /// The calls the check made.
        calls: CallCounts,
    },
}

/// A watch's one answer, which the reducer sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Ended without evidence, `WatchEnded`; `why` is logged.
    Ended {
        /// The watch.
        open: OpenWatch,
        /// Why it ended.
        why: &'static str,
    },
    /// Answered with the key's evidence.
    Evidence {
        /// The watch.
        open: OpenWatch,
        /// The key's reply.
        reply: TextReply,
        /// The calls the answering check made.
        calls: CallCounts,
        /// When the watch's waiting ended.
        awaited_at_us: u64,
    },
    /// The program holding it was abandoned.
    Abandoned {
        /// The watch.
        open: OpenWatch,
    },
}

/// What a due time reached did ([`Watch::reach`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Reached {
    /// Nothing is due.
    Nothing,
    /// The watch ended at its bound.
    Ended(Box<Answer>),
    /// A poll read is due: the watch is now held for it.
    Read,
}

/// The clock a transition reads: the run's shared clock, in microseconds,
/// for the reducer; `None` for a program's copy, which reads no clock.
pub type Clock = Option<u64>;

/// `duration` in microseconds.
fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

impl Watch {
    /// The watch, in any state but [`Watch::None`].
    #[must_use]
    pub fn open(&self) -> Option<&OpenWatch> {
        match self {
            Self::None => None,
            Self::Waiting { open, .. }
            | Self::Polling { open, .. }
            | Self::Checking { open, .. } => Some(open),
        }
    }

    /// The watch when it is waiting or polling, so a take would take it.
    #[must_use]
    pub fn kept(&self) -> Option<&OpenWatch> {
        match self {
            Self::Waiting { open, .. } | Self::Polling { open, .. } => Some(open),
            Self::None | Self::Checking { .. } => None,
        }
    }

    /// Whether there is no watch.
    #[must_use]
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// When the reducer next acts on the watch: its bound while waiting,
    /// its next read while polling.
    #[must_use]
    pub fn due(&self) -> Option<u64> {
        match self {
            Self::Waiting { bound, .. } => Some(*bound),
            Self::Polling { next, .. } => Some(*next),
            Self::None | Self::Checking { .. } => None,
        }
    }

    /// Opens `open` from no watch, polling when its node is the focused
    /// console host's text area (`console`): the bound is
    /// [`CARET_WATCH_BOUND`] from now; the poll's last read
    /// [`POLL_BOUND`] after the key was pressed (after now when the key has
    /// no time), its next [`POLL_INTERVAL`] from now or at the last,
    /// whichever is first. Whether it opened: a watch already there is
    /// left as it is.
    pub fn opened(&mut self, open: OpenWatch, console: bool, clock: Clock) -> bool {
        if !self.is_none() {
            return false;
        }
        *self = if console {
            let (next, until) = clock.map_or((0, 0), |now| {
                let pressed_us = open.watch.pressed_at_ms.saturating_mul(1_000);
                let from = if pressed_us == 0 { now } else { pressed_us };
                let until = from.saturating_add(micros(POLL_BOUND));
                (now.saturating_add(micros(POLL_INTERVAL)).min(until), until)
            });
            Self::Polling { open, next, until }
        } else {
            let bound = clock.map_or(0, |now| now.saturating_add(micros(CARET_WATCH_BOUND)));
            Self::Waiting { open, bound }
        };
        true
    }

    /// Takes a waiting or polling watch on `node` for the program in
    /// flight to check: it is held, to go back as it stood.
    pub fn take(&mut self, node: NodeId) -> Option<OpenWatch> {
        let resume = match self {
            Self::Waiting { open, bound } if open.node_id == node => Resume::Bound(*bound),
            Self::Polling { open, next, until } if open.node_id == node => Resume::Poll {
                next: *next,
                until: *until,
            },
            _ => return None,
        };
        let (Self::Waiting { open, .. } | Self::Polling { open, .. }) = std::mem::take(self) else {
            return None;
        };
        *self = Self::Checking {
            open: open.clone(),
            resume,
        };
        Some(open)
    }

    /// The held watch's check found `checked`: answered, it ends with the
    /// evidence; watching, it goes back as it stood after a take, and after
    /// a poll read polls again [`POLL_INTERVAL`] from now or at its last
    /// read, whichever is first, or ends once its last read has passed. The
    /// calls are added to the watch's. `None` when no watch is held, and
    /// the result is dropped; otherwise the answer it gives, if any.
    pub fn checked(&mut self, checked: &Checked, clock: Clock) -> Option<Option<Answer>> {
        if !matches!(self, Self::Checking { .. }) {
            return None;
        }
        let Self::Checking { mut open, resume } = std::mem::take(self) else {
            return None;
        };
        match checked {
            Checked::Answered {
                reply,
                calls,
                awaited_at_us,
            } => Some(Some(Answer::Evidence {
                open,
                reply: reply.clone(),
                calls: *calls,
                awaited_at_us: *awaited_at_us,
            })),
            Checked::Watching { calls } => {
                open.calls.add_assign(*calls);
                *self = match resume {
                    Resume::Bound(bound) => Self::Waiting { open, bound },
                    Resume::Poll { next, until } => Self::Polling { open, next, until },
                    Resume::Read { until } => match clock {
                        Some(now) if now >= until => {
                            return Some(Some(Answer::Ended {
                                open,
                                why: "its poll's bound passed",
                            }));
                        }
                        Some(now) => Self::Polling {
                            open,
                            next: now.saturating_add(micros(POLL_INTERVAL)).min(until),
                            until,
                        },
                        None => Self::Polling {
                            open,
                            next: 0,
                            until,
                        },
                    },
                };
                Some(None)
            }
        }
    }

    /// Ends the watch, in any state, for `why`.
    pub fn end(&mut self, why: &'static str) -> Option<Answer> {
        let open = match std::mem::take(self) {
            Self::None => return None,
            Self::Waiting { open, .. }
            | Self::Polling { open, .. }
            | Self::Checking { open, .. } => open,
        };
        Some(Answer::Ended { open, why })
    }

    /// Ends a waiting or polling watch for `why`; a held one is left to
    /// the program holding it.
    pub fn end_kept(&mut self, why: &'static str) -> Option<Answer> {
        if self.kept().is_some() {
            self.end(why)
        } else {
            None
        }
    }

    /// Acts on the due time if it has come by `now_us`: a waiting watch
    /// ends at its bound; a polling watch is held for its next read.
    pub fn reach(&mut self, now_us: u64) -> Reached {
        match self {
            Self::Waiting { bound, .. } if now_us >= *bound => match self.end("its bound passed") {
                Some(answer) => Reached::Ended(Box::new(answer)),
                None => Reached::Nothing,
            },
            Self::Polling { next, until, .. } if now_us >= *next => {
                let until = *until;
                let Self::Polling { open, .. } = std::mem::take(self) else {
                    return Reached::Nothing;
                };
                *self = Self::Checking {
                    open,
                    resume: Resume::Read { until },
                };
                Reached::Read
            }
            _ => Reached::Nothing,
        }
    }

    /// The program holding the watch was abandoned: it ends, answered
    /// `Abandoned`.
    pub fn abandoned(&mut self) -> Option<Answer> {
        if !matches!(self, Self::Checking { .. }) {
            return None;
        }
        let Self::Checking { open, .. } = std::mem::take(self) else {
            return None;
        };
        Some(Answer::Abandoned { open })
    }

    /// The program in flight ended, having panicked, returned without its
    /// check's result, or been claimed: a watch it held ends.
    pub fn program_ended(&mut self) -> Option<Answer> {
        if matches!(self, Self::Checking { .. }) {
            self.end("its check failed")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::TextUnit;

    use super::*;

    /// A watch for request 7 on node 1, its key pressed at `pressed_ms` on
    /// the shared clock.
    fn open(pressed_ms: u64) -> OpenWatch {
        OpenWatch {
            request_id: RequestId::shell(7),
            trace: TraceId::from_raw(1),
            node_id: NodeId::new(1),
            watch: CaretWatch {
                since: None,
                pressed_at_ms: pressed_ms,
                unit: TextUnit::Character,
                compare: None,
                landing: false,
                deletes: false,
            },
            timing: EventTiming::default(),
            calls: CallCounts::default(),
        }
    }

    fn uia(calls: u32) -> CallCounts {
        CallCounts {
            uia: calls,
            ..CallCounts::default()
        }
    }

    #[test]
    fn the_poll_ends_its_bound_after_the_key_was_pressed() {
        let mut watch = Watch::None;
        // Opened 3 ms after the key was pressed, at 1 s.
        assert!(watch.opened(open(1_000), true, Some(1_003_000)));
        assert_eq!(
            watch,
            Watch::Polling {
                open: open(1_000),
                next: 1_008_000,
                until: 1_050_000,
            }
        );
    }

    #[test]
    fn a_poll_without_the_keys_time_ends_its_bound_after_it_opened() {
        let mut watch = Watch::None;
        assert!(watch.opened(open(0), true, Some(1_003_000)));
        assert_eq!(watch.due(), Some(1_008_000));
        let Watch::Polling { until, .. } = watch else {
            panic!("a polling watch, not {watch:?}");
        };
        assert_eq!(until, 1_053_000);
    }

    #[test]
    fn a_watch_opens_only_where_none_is() {
        let mut watch = Watch::None;
        assert!(watch.opened(open(0), false, Some(1_000)));
        assert_eq!(
            watch,
            Watch::Waiting {
                open: open(0),
                bound: 10_001_000,
            }
        );
        assert!(!watch.opened(open(5), true, Some(2_000)));
        assert_eq!(watch.due(), Some(10_001_000));
    }

    #[test]
    fn a_programs_copy_opens_with_its_times_at_zero() {
        let mut waiting = Watch::None;
        waiting.opened(open(1_000), false, None);
        assert_eq!(
            waiting,
            Watch::Waiting {
                open: open(1_000),
                bound: 0,
            }
        );
        let mut polling = Watch::None;
        polling.opened(open(1_000), true, None);
        assert_eq!(
            polling,
            Watch::Polling {
                open: open(1_000),
                next: 0,
                until: 0,
            }
        );
    }

    #[test]
    fn each_poll_read_is_followed_by_the_next_until_the_last() {
        let mut watch = Watch::Polling {
            open: open(1_000),
            next: 1_045_000,
            until: 1_050_000,
        };
        assert_eq!(watch.reach(1_044_999), Reached::Nothing);
        assert_eq!(watch.reach(1_046_000), Reached::Read);
        assert_eq!(watch.due(), None);
        // A read at 47 ms finds nothing: the next is the last, at 50 ms,
        // not 52.
        assert_eq!(
            watch.checked(&Checked::Watching { calls: uia(2) }, Some(1_047_000)),
            Some(None)
        );
        assert_eq!(
            watch,
            Watch::Polling {
                open: OpenWatch {
                    calls: uia(2),
                    ..open(1_000)
                },
                next: 1_050_000,
                until: 1_050_000,
            }
        );
        assert_eq!(watch.reach(1_050_000), Reached::Read);
        assert_eq!(
            watch.checked(&Checked::Watching { calls: uia(2) }, Some(1_050_000)),
            Some(Some(Answer::Ended {
                open: OpenWatch {
                    calls: uia(4),
                    ..open(1_000)
                },
                why: "its poll's bound passed",
            }))
        );
        assert_eq!(watch, Watch::None);
    }

    #[test]
    fn a_take_of_another_node_leaves_a_waiting_watch_as_it_is() {
        let waiting = Watch::Waiting {
            open: open(0),
            bound: 10_001_000,
        };
        let mut watch = waiting.clone();
        assert_eq!(watch.take(NodeId::new(2)), None);
        assert_eq!(watch, waiting);
    }

    #[test]
    fn a_take_found_nothing_puts_the_watch_back_as_it_stood() {
        let polling = Watch::Polling {
            open: open(1_000),
            next: 1_005_000,
            until: 1_050_000,
        };
        let mut watch = polling.clone();
        assert_eq!(watch.take(NodeId::new(2)), None);
        assert_eq!(watch, polling);
        assert_eq!(watch.take(NodeId::new(1)), Some(open(1_000)));
        assert_eq!(watch.due(), None);
        // Found nothing, even at a time past its next read.
        assert_eq!(
            watch.checked(&Checked::Watching { calls: uia(1) }, Some(1_040_000)),
            Some(None)
        );
        assert_eq!(
            watch,
            Watch::Polling {
                open: OpenWatch {
                    calls: uia(1),
                    ..open(1_000)
                },
                next: 1_005_000,
                until: 1_050_000,
            }
        );
    }

    #[test]
    fn a_waiting_watch_ends_at_its_bound() {
        let mut watch = Watch::Waiting {
            open: open(0),
            bound: 10_000,
        };
        assert_eq!(watch.reach(9_999), Reached::Nothing);
        assert_eq!(
            watch.reach(10_000),
            Reached::Ended(Box::new(Answer::Ended {
                open: open(0),
                why: "its bound passed",
            }))
        );
        assert_eq!(watch, Watch::None);
    }

    #[test]
    fn a_result_with_no_watch_held_is_dropped() {
        let waiting = Watch::Waiting {
            open: open(0),
            bound: 10_000,
        };
        let mut watch = waiting.clone();
        let answered = Checked::Answered {
            reply: TextReply::Done,
            calls: uia(1),
            awaited_at_us: 5,
        };
        assert_eq!(watch.checked(&answered, Some(1)), None);
        assert_eq!(watch, waiting);
        assert_eq!(watch.abandoned(), None);
        assert_eq!(watch.program_ended(), None);
        assert_eq!(watch, waiting);
    }

    /// Every transition from every state, enumerated: the module's own
    /// rules that a query is answered at most once, by a transition that
    /// leaves no watch, and that a held watch has no due time.
    #[test]
    fn every_transition_answers_only_as_it_leaves_no_watch() {
        type Step = fn(&mut Watch) -> Option<Answer>;
        let states = [
            Watch::None,
            Watch::Waiting {
                open: open(0),
                bound: 10_000,
            },
            Watch::Polling {
                open: open(0),
                next: 5_000,
                until: 50_000,
            },
            Watch::Checking {
                open: open(0),
                resume: Resume::Bound(10_000),
            },
            Watch::Checking {
                open: open(0),
                resume: Resume::Poll {
                    next: 5_000,
                    until: 50_000,
                },
            },
            Watch::Checking {
                open: open(0),
                resume: Resume::Read { until: 50_000 },
            },
        ];
        let steps: [(&str, Step); 11] = [
            ("open", |watch| {
                watch.opened(open(0), true, Some(1));
                None
            }),
            ("take", |watch| {
                watch.take(NodeId::new(1));
                None
            }),
            ("answered", |watch| {
                watch
                    .checked(
                        &Checked::Answered {
                            reply: TextReply::Done,
                            calls: uia(1),
                            awaited_at_us: 1,
                        },
                        Some(1),
                    )
                    .flatten()
            }),
            ("watching early", |watch| {
                watch
                    .checked(&Checked::Watching { calls: uia(1) }, Some(1))
                    .flatten()
            }),
            ("watching late", |watch| {
                watch
                    .checked(&Checked::Watching { calls: uia(1) }, Some(60_000))
                    .flatten()
            }),
            ("end", |watch| watch.end("why")),
            ("end kept", |watch| watch.end_kept("why")),
            ("abandoned", Watch::abandoned),
            ("program ended", Watch::program_ended),
            ("reached early", |watch| match watch.reach(1) {
                Reached::Ended(answer) => Some(*answer),
                _ => None,
            }),
            ("reached late", |watch| match watch.reach(60_000) {
                Reached::Ended(answer) => Some(*answer),
                _ => None,
            }),
        ];
        for state in &states {
            for (name, step) in &steps {
                let mut watch = state.clone();
                let answer = step(&mut watch);
                if answer.is_some() {
                    assert!(
                        !state.is_none() && watch.is_none(),
                        "{name} from {state:?} answered and left {watch:?}"
                    );
                }
                if state.is_none() {
                    assert!(answer.is_none(), "{name} from no watch answered");
                }
                if matches!(watch, Watch::Checking { .. }) {
                    assert_eq!(watch.due(), None, "{name} from {state:?}");
                }
            }
        }
    }
}
