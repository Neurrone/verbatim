//! A program's commit (`books/dev/src/explanation/design/ownership.md`): what one step of a program changes, built
//! with every call it needs already made, and posted to the mailbox. The
//! reducer applies it only while the program's request is live; until
//! then the program applies its pure part to its own copies
//! ([`super::reducer::Facts`]), so what it reads after a commit is what the
//! reducer will hold.
//!
//! [`Effects`] carries, besides the reducer's [`Change`], what only the
//! shell can apply: the elements a focus's subscription target names, the
//! element a UIA focus's evidence says is stale, a terminal's entry, the
//! caret last sent to Core, and the program's text writes since its last
//! commit, all writes to the store or the shell's table of followed
//! elements, made on the reducer's thread under the store's lock alone,
//! with no call into the application.

#![forbid(unsafe_code)]

use verbatim_model::{CaretReport, NodeId, StateSet, TraceId};
use verbatim_uia::{Agile, Kept};

use verbatim_model::protocol::{EventTiming, OutpostToCore};

use super::reducer::{
    Change, DialogMemo, FocusCommit, Followed, Group, LastReported, MarkChange, OwedWork, Report,
    Subscribed, WatchChange,
};
use super::store::TextWrite;

/// Where the focus-following subscription follows a focus, computed before
/// the commit: the focus's element and group, and the slider followed for
/// its value changes, when it is kept.
pub(super) struct KeptTarget {
    /// The group.
    pub(super) group: Group,
    /// The node's element.
    pub(super) element: Agile<Kept>,
    /// The slider's node and element.
    pub(super) slider: Option<(NodeId, Agile<Kept>)>,
}

/// Where the subscription is to be moved for a focus, `None` for nothing.
pub(super) type Target = Option<KeptTarget>;

/// The element kept for a UIA focus's node that the focus listener's
/// evidence says is no longer the node's element: the runtime id names a
/// new element, the one it named having lost the focus or gone
/// (`books/dev/src/reference/adr/0001-focus.md`, "Object identity").
/// The node's terminal entry goes with it, and the caret key's watch on the
/// node ends (`Change::replaced`).
pub(super) struct Stale {
    /// The node.
    pub(super) node: NodeId,
    /// The element kept for it before the focus was read, evicted only if
    /// it is still the one kept, compared by agile reference.
    pub(super) element: Option<Agile<Kept>>,
}

/// What of a focus's commit only the shell applies ([`FocusEffects`]).
pub(super) struct FollowWrites {
    /// The focus's node.
    pub(super) node: NodeId,
    /// Whether it is a terminal read through UI Automation.
    pub(super) terminal: bool,
    /// For a terminal, the caret read with its report, noted as the caret
    /// last sent to Core.
    pub(super) sent_caret: Option<CaretReport>,
    /// Where the subscription is moved.
    pub(super) target: Target,
}

/// A commit's change of what is followed ([`Effects::focus`]): the
/// reducer's [`FocusCommit`] with what only the shell can apply beside it,
/// so that the record's subscription is filled in only where the shell gives
/// its elements their tokens.
pub(super) enum FocusEffects {
    /// A focus adopted: its record, with no subscription yet.
    Adopt {
        /// The record.
        followed: Box<Followed>,
        /// Its report's role and batch.
        reported: LastReported,
        /// The dialog memo it leaves.
        dialog: Option<DialogMemo>,
        /// What only the shell applies.
        writes: FollowWrites,
    },
    /// A focus-now answer naming the focus already followed.
    Again {
        /// What it owes now.
        owed: Option<OwedWork>,
        /// The trace of the event it came from.
        trace: TraceId,
        /// The event's observation, on the run's shared clock.
        observed_at_us: u64,
        /// What only the shell applies.
        writes: FollowWrites,
    },
    /// A foreground report, which follows nothing.
    Foreground {
        /// The window's node.
        node: NodeId,
        /// Its role and batch.
        reported: LastReported,
        /// The dialog memo it leaves.
        dialog: DialogMemo,
    },
}

impl FocusEffects {
    /// What only the shell applies, for a focus followed.
    pub(super) fn writes_mut(&mut self) -> Option<&mut FollowWrites> {
        match self {
            Self::Adopt { writes, .. } | Self::Again { writes, .. } => Some(writes),
            Self::Foreground { .. } => None,
        }
    }

    /// The commit a program applies to its own copies: no tokens, so no
    /// subscription.
    fn local(&self) -> FocusCommit {
        match self {
            Self::Adopt {
                followed,
                reported,
                dialog,
                ..
            } => FocusCommit::Adopt {
                followed: followed.clone(),
                reported: *reported,
                dialog: dialog.clone(),
            },
            Self::Again {
                owed,
                trace,
                observed_at_us,
                ..
            } => FocusCommit::Again {
                target: None,
                owed: owed.clone(),
                trace: *trace,
                observed_at_us: *observed_at_us,
            },
            Self::Foreground {
                node,
                reported,
                dialog,
            } => FocusCommit::Foreground {
                node: *node,
                reported: *reported,
                dialog: dialog.clone(),
            },
        }
    }

    /// What of this only the shell applies, for a refused commit to let go
    /// of.
    pub(super) fn into_writes(self) -> Option<FollowWrites> {
        match self {
            Self::Adopt { writes, .. } | Self::Again { writes, .. } => Some(writes),
            Self::Foreground { .. } => None,
        }
    }

    /// The reducer's commit, `subscribe` making the follow's writes and
    /// giving where the subscription follows, by token.
    pub(super) fn into_commit(
        self,
        subscribe: impl FnOnce(FollowWrites) -> Option<Subscribed>,
    ) -> FocusCommit {
        match self {
            Self::Adopt {
                mut followed,
                reported,
                dialog,
                writes,
            } => {
                followed.subscription = subscribe(writes);
                FocusCommit::Adopt {
                    followed,
                    reported,
                    dialog,
                }
            }
            Self::Again {
                owed,
                trace,
                observed_at_us,
                writes,
            } => FocusCommit::Again {
                target: subscribe(writes),
                owed,
                trace,
                observed_at_us,
            },
            Self::Foreground {
                node,
                reported,
                dialog,
            } => FocusCommit::Foreground {
                node,
                reported,
                dialog,
            },
        }
    }
}

/// What one commit does, built on the stack before the commit with every
/// call it needs already made.
#[derive(Default)]
pub(super) struct Effects {
    /// The program's result, which ends its query.
    pub(super) result: Option<OutpostToCore>,
    /// The report to record for Core's judgement.
    pub(super) report: Option<Report>,
    /// A focus adopted or followed again, or a foreground report.
    pub(super) focus: Option<FocusEffects>,
    /// The focus's element found after its report, with its slider's when
    /// found: the subscription is moved to them only while that node is
    /// the followed focus.
    pub(super) element: Option<(NodeId, KeptTarget)>,
    /// A terminal's new entry, the copy its read worked on.
    pub(super) terminal: Option<(NodeId, crate::terminal::Terminal)>,
    /// The caret last sent to Core for a focused terminal.
    pub(super) sent_caret: Option<(NodeId, CaretReport)>,
    /// The owed work of this node is done, read or failed.
    pub(super) owed_done: Option<NodeId>,
    /// A read of this node's caret began at the instant given, in
    /// microseconds.
    pub(super) caret_read: Option<(NodeId, u64)>,
    /// A change of the caret key's watch.
    pub(super) watch: Option<WatchChange>,
    /// A change of the marks on the followed focus.
    pub(super) mark: Option<MarkChange>,
    /// A held node's states, as an MSAA state change read them.
    pub(super) states: Option<(NodeId, StateSet)>,
    /// A UIA focus's stale element, evicted before the focus is followed.
    pub(super) stale: Option<Stale>,
    /// A held terminal's caret event, noted to be read once nothing else is
    /// waiting, with its event's timing.
    pub(super) noted_caret: Option<(NodeId, TraceId, u64, EventTiming)>,
    /// The program's text writes since its last commit, in order: its
    /// text facts' changes, which it applied to its own copies as it made
    /// them, with the ranges they name, and its patterns and support.
    pub(super) text: Vec<TextWrite>,
    /// The window the program found hung, from its charge, set as the
    /// commit is posted (`RequestContext::commit`).
    pub(super) found: Option<verbatim_reader_rules::Window>,
}

impl Effects {
    /// The reducer's part of these effects but for their messages, with no
    /// element token: what a program applies to its own copies. An element
    /// found after its report is left out: it has no token, and the
    /// subscription in a program's copy is not read.
    pub(super) fn local(&self) -> Change {
        Change {
            result: None,
            report: self.report.clone(),
            focus: self.focus.as_ref().map(FocusEffects::local),
            element: None,
            owed_done: self.owed_done,
            caret_read: self.caret_read,
            watch: self.watch.clone(),
            mark: self.mark,
            states: self.states,
            noted_caret: self.noted_caret,
            replaced: self.stale.as_ref().map(|stale| stale.node),
            text: Vec::new(),
            found: self.found,
        }
    }

    /// The reducer's part, with no focus or element until the shell has
    /// given them their tokens, and what only the shell applies.
    pub(super) fn split(self) -> (Change, Writes) {
        let change = Change {
            focus: None,
            element: None,
            report: self.report,
            result: self.result,
            owed_done: self.owed_done,
            caret_read: self.caret_read,
            watch: self.watch,
            mark: self.mark,
            states: self.states,
            found: self.found,
            noted_caret: self.noted_caret,
            replaced: self.stale.as_ref().map(|stale| stale.node),
            text: Vec::new(),
        };
        let writes = Writes {
            focus: self.focus,
            element: self.element,
            terminal: self.terminal,
            sent_caret: self.sent_caret,
            stale: self.stale,
            text: self.text,
        };
        (change, writes)
    }
}

/// What of a commit only the shell applies ([`Effects::split`]).
pub(super) struct Writes {
    /// The focus followed or the foreground reported.
    pub(super) focus: Option<FocusEffects>,
    /// The element of a focus found after its report, with its node.
    pub(super) element: Option<(NodeId, KeptTarget)>,
    /// A terminal's new entry.
    pub(super) terminal: Option<(NodeId, crate::terminal::Terminal)>,
    /// The caret last sent to Core for a focused terminal.
    pub(super) sent_caret: Option<(NodeId, CaretReport)>,
    /// A UIA focus's stale element.
    pub(super) stale: Option<Stale>,
    /// The program's text writes, which give the change its text facts'
    /// changes as they are made.
    pub(super) text: Vec<TextWrite>,
}

/// What [`Worker::commit`](super::worker) did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Committed {
    /// The effects were posted for the reducer.
    Applied,
    /// Nothing was posted: the worker was abandoned.
    Refused,
}
