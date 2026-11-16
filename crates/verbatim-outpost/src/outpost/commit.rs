//! A program's commit (`books/dev/src/explanation/design/ownership.md`): what one step of a program changes, built
//! with every call it needs already made, and posted to the mailbox. The
//! reducer applies it only while the program's request is live; until
//! then the program applies its pure part to its own copies
//! ([`super::reducer::Facts`]), so what it reads after a commit is what the
//! reducer will hold.
//!
//! [`Effects`] carries, besides the reducer's [`Change`], what only the
//! shell can apply: the elements a focus's subscription target names, the
//! element a UIA focus's evidence says is stale, and the program's text
//! writes since its last commit, all writes to the store or the shell's table of followed
//! elements, made on the reducer's thread under the store's lock alone,
//! with no call into the application.

#![forbid(unsafe_code)]

use verbatim_model::NodeId;
use verbatim_uia::{Agile, Kept};

use verbatim_model::protocol::OutpostToCore;

use super::reducer::{Change, FocusCommit, Followed, Group, LastReported, MarkChange, Subscribed};
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
/// Any key's note on the node ends (`Change::replaced`).
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
        /// What only the shell applies.
        writes: FollowWrites,
    },
    /// A focus-now answer naming the focus already followed.
    Again {
        /// What only the shell applies.
        writes: FollowWrites,
    },
}

impl FocusEffects {
    /// What only the shell applies, for a focus followed.
    pub(super) fn writes_mut(&mut self) -> &mut FollowWrites {
        match self {
            Self::Adopt { writes, .. } | Self::Again { writes, .. } => writes,
        }
    }

    /// The commit a program applies to its own copies: no tokens, so no
    /// subscription.
    fn local(&self) -> FocusCommit {
        match self {
            Self::Adopt {
                followed, reported, ..
            } => FocusCommit::Adopt {
                followed: followed.clone(),
                reported: *reported,
            },
            Self::Again { .. } => FocusCommit::Again { target: None },
        }
    }

    /// What of this only the shell applies, for a refused commit to let go
    /// of.
    pub(super) fn into_writes(self) -> FollowWrites {
        match self {
            Self::Adopt { writes, .. } | Self::Again { writes, .. } => writes,
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
                writes,
            } => {
                followed.subscription = subscribe(writes);
                FocusCommit::Adopt { followed, reported }
            }
            Self::Again { writes } => FocusCommit::Again {
                target: subscribe(writes),
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
    /// A focus adopted or followed again, or a foreground report.
    pub(super) focus: Option<FocusEffects>,
    /// The focus's element found after its report, with its slider's when
    /// found: the subscription is moved to them only while that node is
    /// the followed focus.
    pub(super) element: Option<(NodeId, KeptTarget)>,
    /// A read of this node's caret began at the instant given, in
    /// microseconds.
    pub(super) caret_read: Option<(NodeId, u64)>,
    /// A change of the marks on the followed focus.
    pub(super) mark: Option<MarkChange>,
    /// A UIA focus's stale element, evicted before the focus is followed.
    pub(super) stale: Option<Stale>,
    /// The program's text writes since its last commit, in order: its
    /// text facts' changes, which it applied to its own copies as it made
    /// them, with the ranges they name, and its patterns and support.
    pub(super) text: Vec<TextWrite>,
}

impl Effects {
    /// The reducer's part of these effects but for their messages, with no
    /// element token: what a program applies to its own copies. An element
    /// found after its report is left out: it has no token, and the
    /// subscription in a program's copy is not read.
    pub(super) fn local(&self) -> Change {
        Change {
            result: None,
            focus: self.focus.as_ref().map(FocusEffects::local),
            element: None,
            caret_read: self.caret_read,
            mark: self.mark,
            replaced: self.stale.as_ref().map(|stale| stale.node),
            text: Vec::new(),
        }
    }

    /// The reducer's part, with no focus or element until the shell has
    /// given them their tokens, and what only the shell applies.
    pub(super) fn split(self) -> (Change, Writes) {
        let change = Change {
            focus: None,
            element: None,
            result: self.result,
            caret_read: self.caret_read,
            mark: self.mark,
            replaced: self.stale.as_ref().map(|stale| stale.node),
            text: Vec::new(),
        };
        let writes = Writes {
            focus: self.focus,
            element: self.element,
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
