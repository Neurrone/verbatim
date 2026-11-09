//! The caret key's watch (`docs/design/caret-selection-rework.md`): the
//! outpost's open check for a key's evidence, held as one state beside the
//! node's last read. A key's baseline is a copy of a last read, kept here
//! for the key. The reducer alone decides the times, on the run's shared
//! clock; each program's copy applies the same transitions with no clock,
//! its times left at zero, since no program reads them.

use std::time::Duration;

use verbatim_model::protocol::EventTiming;
use verbatim_model::{
    Backend, CallCounts, CaretMotion, KeyRead, KeyTag, NodeId, TextUnit, TraceId, WatchKind,
};

use super::text::{LastRead, RangeKey};

/// How long a terminal's watch stays open without evidence: its handling
/// deadline, [`HANDLING_DEADLINE`](verbatim_model::HANDLING_DEADLINE). A text
/// field's watch has no time of its own: Core ends it.
pub const TERMINAL_WATCH_BOUND: Duration = verbatim_model::HANDLING_DEADLINE;

/// How long after one of the console poll's reads the next is made.
pub const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long after the before read ended, or the key's request arrived, the
/// console poll's last read is made; a watch that has found no evidence by
/// then ends.
pub const POLL_BOUND: Duration = Duration::from_millis(50);

/// What a read compared with a key's baseline found, by the comparisons only
/// the provider can make.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one comparison the provider makes"
)]
pub struct Compared {
    /// The caret left where it was.
    pub caret_moved: bool,
    /// The selection changed.
    pub selection_moved: bool,
    /// The characters beside the caret changed.
    pub beside_changed: bool,
    /// The unit at the caret changed.
    pub unit_changed: bool,
    /// The caret's line changed.
    pub line_changed: bool,
}

/// Whether `compared` is evidence of what a key of `motion` did: the caret
/// having left where it was, the characters beside it changing, the unit at
/// the caret changing after Delete, the caret's line changing after a key
/// that deletes, or the selection changing.
#[must_use]
pub fn had_effect(compared: Compared, motion: CaretMotion) -> bool {
    compared.caret_moved
        || compared.selection_moved
        || compared.beside_changed
        || (motion.compares_unit() && compared.unit_changed)
        || (motion.deletes() && compared.line_changed)
}

/// A key's baseline: a copy of its node's last read, of its provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Baseline {
    /// Through UIA, its positions the store's keys.
    Uia(LastRead<RangeKey>),
    /// Through an edit control's messages, its positions offsets.
    Edit(LastRead<u32>),
}

impl Baseline {
    /// The backend it was read through.
    #[must_use]
    pub fn backend(&self) -> Backend {
        match self {
            Self::Uia(_) => Backend::Uia,
            Self::Edit(_) => Backend::Msaa,
        }
    }
}

/// What a key's watch is: what it answers, what it compares with, and what
/// its checks have cost so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyWatch {
    /// The key's tag.
    pub tag: KeyTag,
    /// The node whose caret the key moves.
    pub node: NodeId,
    /// What the key is judged from.
    pub baseline: Baseline,
    /// The unit to report at the caret.
    pub unit: TextUnit,
    /// What the key does, which says what counts as its evidence.
    pub motion: CaretMotion,
    /// The kind of watch Core named.
    pub kind: WatchKind,
    /// The key's unit at the caret before it, when its before read read it.
    pub before_unit: Option<verbatim_model::TextChunk>,
    /// The key's trace.
    pub trace: TraceId,
    /// The request's timing, which its answer carries.
    pub timing: EventTiming,
    /// The calls its checks have made so far, none of which found evidence.
    pub calls: CallCounts,
}

/// What an open watch is doing between checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// A text field: until evidence or Core's word.
    Awaiting,
    /// A text field Core has asked the final read of, while another program
    /// is in flight: the final read runs as soon as none is.
    FinalDue,
    /// A terminal: its handling deadline.
    Terminal {
        /// When it ends without evidence.
        ends: u64,
    },
    /// The console host's caret poll: a read due at `next`, the last at
    /// `until`.
    Poll {
        /// When the next read is due.
        next: u64,
        /// When the last read is due.
        until: u64,
    },
}

/// What a check does when it finds nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterCheck {
    /// Goes back to this stage.
    Resume(Stage),
    /// Answers: the check is the final read.
    Answer,
    /// The poll's next step after a poll read: its next read, or its end
    /// once `until` has passed.
    PollRead {
        /// When the last read is due.
        until: u64,
    },
}

/// The caret key's watch, in one of its states. Times are microseconds on
/// the run's shared clock; zero in a program's copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Watch {
    /// No watch.
    #[default]
    None,
    /// A key's request whose program opens the watch has arrived: a commit
    /// opens a watch only from here, and only the watch of this tag, so a
    /// program whose key was given up or replaced opens none.
    Reading {
        /// The key's tag.
        tag: KeyTag,
    },
    /// Between checks.
    Open {
        /// The watch.
        key: Box<KeyWatch>,
        /// What it is doing.
        stage: Stage,
    },
    /// The program in flight holds it.
    Checking {
        /// The watch.
        key: Box<KeyWatch>,
        /// What it does when the check finds nothing.
        after: AfterCheck,
    },
}

/// A read a check made, with the judgement of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The node read.
    pub node: NodeId,
    /// The trace its program ran under, which an untagged report carries.
    pub trace: TraceId,
    /// The backend read, which the report carries.
    pub backend: Backend,
    /// The read, as a key's result or an untagged report.
    pub read: Box<KeyRead>,
    /// Whether the read shows the key had an effect ([`had_effect`]).
    pub effect: bool,
    /// Whether a read that ends no watch goes as an untagged caret report:
    /// a terminal's reading goes by its own path.
    pub untagged: bool,
    /// The calls the check made.
    pub calls: CallCounts,
    /// When the watch's waiting ended, on the run's shared clock.
    pub awaited_at_us: u64,
}

/// What a program's check of the watch it holds found: the one result of
/// every check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checked {
    /// The read.
    Found(Found),
    /// The caret moved while it was read, and the event that follows the
    /// move checks again.
    Moving {
        /// The calls the check made.
        calls: CallCounts,
    },
    /// The read could not be made.
    Failed {
        /// The calls the check made.
        calls: CallCounts,
    },
}

/// A watch's one answer, which the reducer sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Ended with no read to answer from; `why` is logged.
    Ended {
        /// The watch.
        key: Box<KeyWatch>,
        /// Why it ended.
        why: &'static str,
    },
    /// A read found evidence.
    Evidence {
        /// The watch.
        key: Box<KeyWatch>,
        /// The read.
        found: Found,
    },
    /// The final read found no evidence.
    Bound {
        /// The watch.
        key: Box<KeyWatch>,
        /// The read.
        found: Found,
    },
}

/// A check's read that ends no watch, sent as an untagged caret report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Untagged {
    /// The node read.
    pub node: NodeId,
    /// The read.
    pub found: Found,
}

/// What a due time reached did ([`Watch::reach`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Reached {
    /// Nothing is due.
    Nothing,
    /// The watch ended at its handling deadline.
    Ended(Box<Answer>),
    /// A read is due: the watch is now held for it.
    Read,
}

/// What Core's final read found ([`Watch::final_read`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalRead {
    /// The order is for no open watch, and is ignored.
    Ignored,
    /// The next idle moment runs the final read.
    Noted,
    /// A read is to be dispatched now: the watch is held for it.
    Dispatch,
}

/// The clock a transition reads: the run's shared clock, in microseconds,
/// for the reducer; `None` for a program's copy, which reads no clock.
pub type Clock = Option<u64>;

/// `duration` in microseconds.
fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

impl Watch {
    /// The watch, when one is open.
    #[must_use]
    pub fn open(&self) -> Option<&KeyWatch> {
        match self {
            Self::None | Self::Reading { .. } => None,
            Self::Open { key, .. } | Self::Checking { key, .. } => Some(key),
        }
    }

    /// The watch when it is between checks, so a take would take it.
    #[must_use]
    pub fn kept(&self) -> Option<&KeyWatch> {
        match self {
            Self::Open { key, .. } => Some(key),
            Self::None | Self::Reading { .. } | Self::Checking { .. } => None,
        }
    }

    /// Whether no watch is open.
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.open().is_none()
    }

    /// The UIA ranges the baseline names when the watch is `node`'s.
    pub fn baseline_keys(&self, node: u64) -> impl Iterator<Item = RangeKey> + '_ {
        self.open()
            .filter(|key| key.node.number() == node)
            .and_then(|key| match &key.baseline {
                Baseline::Uia(last) => Some(last.positions().copied()),
                Baseline::Edit(_) => None,
            })
            .into_iter()
            .flatten()
    }

    /// Whether a final read is due, which the next idle moment runs.
    #[must_use]
    pub fn final_due(&self) -> bool {
        matches!(
            self,
            Self::Open {
                stage: Stage::FinalDue,
                ..
            }
        )
    }

    /// When the reducer next acts on the watch: a terminal's deadline or a
    /// poll's next read.
    #[must_use]
    pub fn due(&self) -> Option<u64> {
        match self {
            Self::Open { stage, .. } => match stage {
                Stage::Awaiting | Stage::FinalDue => None,
                Stage::Terminal { ends } => Some(*ends),
                Stage::Poll { next, .. } => Some(*next),
            },
            Self::None | Self::Reading { .. } | Self::Checking { .. } => None,
        }
    }

    /// A key's request whose program opens the watch has arrived.
    pub fn reading(&mut self, tag: KeyTag) {
        *self = Self::Reading { tag };
    }

    /// The request of key `tag` was cancelled, or its program ended, and no
    /// watch will open for it.
    pub fn reading_ended(&mut self, tag: KeyTag) {
        if *self == (Self::Reading { tag }) {
            *self = Self::None;
        }
    }

    /// A program's commit opens `key`, from [`Watch::Reading`] of its tag.
    /// Whether it opened: anything else is left as it is.
    pub fn opened(&mut self, key: Box<KeyWatch>, console: bool, clock: Clock) -> bool {
        if *self != (Self::Reading { tag: key.tag }) {
            return false;
        }
        self.start(key, console, clock);
        true
    }

    /// Opens `key`, its stage by its kind and whether its node is the
    /// focused console host's text area (`console`).
    pub fn start(&mut self, key: Box<KeyWatch>, console: bool, clock: Clock) {
        let stage = match key.kind {
            WatchKind::TextField => Stage::Awaiting,
            WatchKind::Terminal if console => {
                let (next, until) = clock.map_or((0, 0), |now| {
                    let until = now.saturating_add(micros(POLL_BOUND));
                    (now.saturating_add(micros(POLL_INTERVAL)).min(until), until)
                });
                Stage::Poll { next, until }
            }
            WatchKind::Terminal => Stage::Terminal {
                ends: clock.map_or(0, |now| now.saturating_add(micros(TERMINAL_WATCH_BOUND))),
            },
        };
        *self = Self::Open { key, stage };
    }

    /// Takes the open watch of `tag` on `node` for the program in flight to
    /// check, so a check made from an older copy cannot answer a newer
    /// watch; a final read due makes the check the final read.
    pub fn take(&mut self, node: NodeId, tag: KeyTag) -> Option<KeyWatch> {
        if !matches!(self, Self::Open { key, .. } if key.node == node && key.tag == tag) {
            return None;
        }
        let Self::Open { key, stage } = std::mem::take(self) else {
            return None;
        };
        let after = if stage == Stage::FinalDue {
            AfterCheck::Answer
        } else {
            AfterCheck::Resume(stage)
        };
        let taken = (*key).clone();
        *self = Self::Checking { key, after };
        Some(taken)
    }

    /// The held watch's check found `checked`. A found read with evidence
    /// answers as evidence; with none, it goes back to its stage, or as
    /// the bound answer when it is the final read. A check with no watch
    /// held, or one that failed, gives what it gave. The calls are added to
    /// the watch's when it stays open.
    pub fn checked(&mut self, checked: Checked, clock: Clock) -> CheckResult {
        if !matches!(self, Self::Checking { .. }) {
            return match checked {
                Checked::Found(found) => CheckResult::NoWatch(found),
                Checked::Moving { .. } | Checked::Failed { .. } => CheckResult::Nothing,
            };
        }
        let Self::Checking { mut key, after } = std::mem::take(self) else {
            return CheckResult::Nothing;
        };
        let found = match checked {
            Checked::Found(found) => found,
            Checked::Moving { calls } => {
                std::ops::AddAssign::add_assign(&mut key.calls, calls);
                return match after {
                    AfterCheck::Resume(stage) => {
                        *self = Self::Open { key, stage };
                        CheckResult::Resumed(None)
                    }
                    AfterCheck::PollRead { until } => {
                        let next = clock.map_or(0, |now| {
                            now.saturating_add(micros(POLL_INTERVAL)).min(until)
                        });
                        *self = Self::Open {
                            key,
                            stage: Stage::Poll { next, until },
                        };
                        CheckResult::Resumed(None)
                    }
                    // The final read has no event to wait for, so it never
                    // reports the caret moving ([`Checked::Moving`]).
                    AfterCheck::Answer => CheckResult::Answered(Answer::Ended {
                        key,
                        why: "the caret moved during its final read",
                    }),
                };
            }
            Checked::Failed { .. } => {
                return CheckResult::Answered(Answer::Ended {
                    key,
                    why: "its check failed",
                });
            }
        };
        if found.effect {
            return CheckResult::Answered(Answer::Evidence { key, found });
        }
        // Call counts add saturating.
        std::ops::AddAssign::add_assign(&mut key.calls, found.calls);
        let found = match after {
            AfterCheck::Answer => {
                return CheckResult::Answered(Answer::Bound { key, found });
            }
            AfterCheck::Resume(stage) => {
                *self = Self::Open { key, stage };
                found
            }
            AfterCheck::PollRead { until } => {
                match clock {
                    Some(now) if now >= until => {
                        return CheckResult::Answered(Answer::Ended {
                            key,
                            why: "its poll's bound passed",
                        });
                    }
                    Some(now) => {
                        *self = Self::Open {
                            key,
                            stage: Stage::Poll {
                                next: now.saturating_add(micros(POLL_INTERVAL)).min(until),
                                until,
                            },
                        };
                    }
                    None => {
                        *self = Self::Open {
                            key,
                            stage: Stage::Poll { next: 0, until },
                        };
                    }
                }
                found
            }
        };
        CheckResult::Resumed(Some(found))
    }

    /// Ends the watch, in any state, for `why`.
    pub fn end(&mut self, why: &'static str) -> Option<Answer> {
        let key = match std::mem::take(self) {
            Self::None | Self::Reading { .. } => return None,
            Self::Open { key, .. } | Self::Checking { key, .. } => key,
        };
        Some(Answer::Ended { key, why })
    }

    /// Ends an open watch for `why`; a held one is left to the program
    /// holding it.
    pub fn end_kept(&mut self, why: &'static str) -> Option<Answer> {
        if self.kept().is_some() {
            self.end(why)
        } else {
            None
        }
    }

    /// Core's end of the watch `tag`: it stops, and sends nothing. Ignored
    /// for no watch or another's.
    pub fn end_order(&mut self, tag: KeyTag) {
        if self.open().is_some_and(|key| key.tag == tag) {
            *self = Self::None;
        }
        self.reading_ended(tag);
    }

    /// Core's final read of the watch `tag`: an open watch of a text field
    /// is held for a read to be dispatched now, or, with a program in flight
    /// (`busy`), notes the read as due; a check already running, whose read
    /// began before the order, is followed by the final read when it finds
    /// nothing. Any other is ignored.
    pub fn final_read(&mut self, tag: KeyTag, busy: bool) -> FinalRead {
        match self {
            Self::Open { key, stage } if key.tag == tag && *stage == Stage::Awaiting => {
                if busy {
                    *stage = Stage::FinalDue;
                    FinalRead::Noted
                } else {
                    let Self::Open { key, .. } = std::mem::take(self) else {
                        return FinalRead::Ignored;
                    };
                    *self = Self::Checking {
                        key,
                        after: AfterCheck::Answer,
                    };
                    FinalRead::Dispatch
                }
            }
            Self::Checking { key, after }
                if key.tag == tag && *after == AfterCheck::Resume(Stage::Awaiting) =>
            {
                *after = AfterCheck::Resume(Stage::FinalDue);
                FinalRead::Noted
            }
            _ => FinalRead::Ignored,
        }
    }

    /// Acts on the due time if it has come by `now_us`: a final read due,
    /// or a poll's next read, holds the watch for a read; a terminal's
    /// deadline ends it.
    pub fn reach(&mut self, now_us: u64) -> Reached {
        let Self::Open { stage, .. } = self else {
            return Reached::Nothing;
        };
        match *stage {
            Stage::FinalDue => {
                let Self::Open { key, .. } = std::mem::take(self) else {
                    return Reached::Nothing;
                };
                *self = Self::Checking {
                    key,
                    after: AfterCheck::Answer,
                };
                Reached::Read
            }
            Stage::Terminal { ends } if now_us >= ends => match self.end("its deadline passed") {
                Some(answer) => Reached::Ended(Box::new(answer)),
                None => Reached::Nothing,
            },
            Stage::Poll { next, until } if now_us >= next => {
                let Self::Open { key, .. } = std::mem::take(self) else {
                    return Reached::Nothing;
                };
                *self = Self::Checking {
                    key,
                    after: AfterCheck::PollRead { until },
                };
                Reached::Read
            }
            Stage::Awaiting | Stage::Terminal { .. } | Stage::Poll { .. } => Reached::Nothing,
        }
    }

    /// The program holding the watch was abandoned or ended without its
    /// check's result: the watch ends.
    pub fn program_ended(&mut self) -> Option<Answer> {
        if matches!(self, Self::Checking { .. }) {
            self.end("its check failed")
        } else {
            None
        }
    }
}

/// What a check's result did ([`Watch::checked`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckResult {
    /// The key is answered.
    Answered(Answer),
    /// It found nothing and the watch stays open; a read goes untagged.
    Resumed(Option<Found>),
    /// No watch is held: the read goes untagged.
    NoWatch(Found),
    /// Nothing to do.
    Nothing,
}

#[cfg(test)]
mod tests {
    use verbatim_model::protocol::EventTiming;
    use verbatim_model::{QueryId, TextAnchor, TextChunk, TraceId};

    use super::*;

    fn line() -> TextChunk {
        TextChunk {
            unit: TextUnit::Line,
            text: String::new(),
            start: TextAnchor(1),
            offset: 0,
            languages: Vec::new(),
            first: false,
            last: false,
            truncated: false,
            formats: Vec::new(),
        }
    }

    fn key(kind: WatchKind) -> Box<KeyWatch> {
        Box::new(KeyWatch {
            tag: KeyTag(QueryId(7)),
            node: NodeId::new(1),
            baseline: Baseline::Edit(LastRead {
                caret: 0,
                selection: None,
                line: line(),
                text_changed: false,
                read_at_us: 0,
            }),
            unit: TextUnit::Character,
            motion: CaretMotion::NextCharacter,
            kind,
            before_unit: None,
            trace: TraceId::from_raw(1),
            timing: EventTiming::default(),
            calls: CallCounts::default(),
        })
    }

    fn found(effect: bool) -> Found {
        Found {
            node: NodeId::new(1),
            trace: TraceId::from_raw(1),
            backend: Backend::Msaa,
            read: Box::new(KeyRead {
                caret: verbatim_model::CaretReport {
                    line: line(),
                    selection: None,
                    read_at_us: 0,
                },
                selection_changes: Vec::new(),
                content_changed: false,
                new_selection: None,
                before_line: line(),
                before_selection: None,
                before_unit: line(),
                unit: line(),
                on_baseline_line: true,
            }),
            effect,
            untagged: true,
            calls: CallCounts::default(),
            awaited_at_us: 0,
        }
    }

    #[test]
    fn the_judgement_counts_what_the_decisions_define() {
        let caret = Compared {
            caret_moved: true,
            ..Compared::default()
        };
        assert!(had_effect(caret, CaretMotion::NextCharacter));
        let unit = Compared {
            unit_changed: true,
            ..Compared::default()
        };
        assert!(!had_effect(unit, CaretMotion::NextCharacter));
        assert!(!had_effect(unit, CaretMotion::Backspace));
        assert!(had_effect(unit, CaretMotion::Delete));
        let line = Compared {
            line_changed: true,
            ..Compared::default()
        };
        assert!(!had_effect(line, CaretMotion::NextCharacter));
        assert!(had_effect(line, CaretMotion::Backspace));
    }

    #[test]
    fn a_console_poll_counts_from_its_opening_and_ends_at_its_last_read() {
        let mut watch = Watch::None;
        watch.start(key(WatchKind::Terminal), true, Some(1_000));
        assert_eq!(watch.due(), Some(6_000));
        assert_eq!(watch.reach(5_999), Reached::Nothing);
        assert_eq!(watch.reach(6_000), Reached::Read);
        assert_eq!(
            watch.checked(Checked::Found(found(false)), Some(8_000)),
            CheckResult::Resumed(Some(found(false)))
        );
        assert_eq!(watch.due(), Some(13_000));
        assert_eq!(watch.reach(13_000), Reached::Read);
        assert!(matches!(
            watch.checked(Checked::Found(found(false)), Some(51_000)),
            CheckResult::Answered(Answer::Ended { .. })
        ));
        assert_eq!(watch, Watch::None);
    }

    #[test]
    fn a_terminals_deadline_ends_its_watch() {
        let mut watch = Watch::None;
        watch.start(key(WatchKind::Terminal), false, Some(0));
        let ends = micros(TERMINAL_WATCH_BOUND);
        assert_eq!(watch.reach(ends - 1), Reached::Nothing);
        assert!(matches!(watch.reach(ends), Reached::Ended(_)));
    }

    #[test]
    fn a_text_fields_watch_has_no_time_of_its_own() {
        let mut watch = Watch::None;
        watch.start(key(WatchKind::TextField), false, Some(0));
        assert_eq!(watch.due(), None);
        assert_eq!(watch.reach(u64::MAX), Reached::Nothing);
    }

    #[test]
    fn a_final_read_with_nothing_found_is_the_bound_answer() {
        let mut watch = Watch::None;
        watch.start(key(WatchKind::TextField), false, Some(0));
        assert_eq!(
            watch.final_read(KeyTag(QueryId(7)), false),
            FinalRead::Dispatch
        );
        assert!(matches!(
            watch.checked(Checked::Found(found(false)), Some(1)),
            CheckResult::Answered(Answer::Bound { .. })
        ));
    }

    #[test]
    fn a_final_read_with_a_program_in_flight_runs_after_it() {
        let mut watch = Watch::None;
        watch.start(key(WatchKind::TextField), false, Some(0));
        assert_eq!(watch.final_read(KeyTag(QueryId(7)), true), FinalRead::Noted);
        assert!(watch.final_due());
        assert_eq!(watch.reach(1), Reached::Read);
    }

    #[test]
    fn a_running_check_is_followed_by_the_final_read() {
        let mut watch = Watch::None;
        watch.start(key(WatchKind::TextField), false, Some(0));
        assert!(watch.take(NodeId::new(1), KeyTag(QueryId(7))).is_some());
        assert_eq!(watch.final_read(KeyTag(QueryId(7)), true), FinalRead::Noted);
        assert_eq!(
            watch.checked(Checked::Found(found(false)), Some(1)),
            CheckResult::Resumed(Some(found(false)))
        );
        assert!(watch.final_due());
    }

    #[test]
    fn a_commit_opens_a_watch_only_from_its_keys_request() {
        let mut watch = Watch::None;
        assert!(!watch.opened(key(WatchKind::TextField), false, Some(0)));
        watch.reading(KeyTag(QueryId(8)));
        assert!(!watch.opened(key(WatchKind::TextField), false, Some(0)));
        watch.reading(KeyTag(QueryId(7)));
        assert!(watch.opened(key(WatchKind::TextField), false, Some(0)));
    }

    #[test]
    fn a_take_of_an_older_tag_takes_nothing() {
        let mut watch = Watch::None;
        watch.start(key(WatchKind::TextField), false, Some(0));
        assert_eq!(watch.take(NodeId::new(1), KeyTag(QueryId(8))), None);
    }

    #[test]
    fn an_order_for_another_tag_is_ignored() {
        let mut watch = Watch::None;
        watch.start(key(WatchKind::TextField), false, Some(0));
        let open = watch.clone();
        assert_eq!(
            watch.final_read(KeyTag(QueryId(8)), false),
            FinalRead::Ignored
        );
        watch.end_order(KeyTag(QueryId(8)));
        assert_eq!(watch, open);
    }

    #[test]
    fn a_checks_read_with_no_watch_held_goes_untagged() {
        let mut watch = Watch::None;
        assert_eq!(
            watch.checked(Checked::Found(found(true)), Some(1)),
            CheckResult::NoWatch(found(true))
        );
    }
}
