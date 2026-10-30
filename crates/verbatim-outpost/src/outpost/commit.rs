//! A program's commit (`docs/design/ownership-outpost-reducer.md`,
//! "Workers and call programs"): what one step of a program changes, built
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

use verbatim_model::{CaretReport, NodeId, NodeSnapshot, Role, StateSet, TraceId};
use verbatim_uia::{Agile, Kept};

use verbatim_model::protocol::{EventTiming, OutpostToCore};

use super::reducer::{
    Change, DialogMemo, FocusPath, GapNote, Group, Object, OwedWork, Report, TextChange,
    VerdictChange, WatchChange,
};
use super::store::TextWrite;

/// Where the focus-following subscription is to be moved for a focus,
/// computed before the commit.
pub(super) struct Target {
    /// What is followed: the node, its group, and the slider followed for
    /// its value changes; `None` for nothing.
    pub(super) wanted: Option<(NodeId, Group, Option<NodeId>)>,
    /// The node's element and the slider's, when they are kept.
    pub(super) element: Option<Agile<Kept>>,
    pub(super) slider: Option<Agile<Kept>>,
}

impl Target {
    /// The target with no element, as a program's own copy holds it: the
    /// shell gives its elements their tokens.
    fn pure(&self) -> super::reducer::Target {
        super::reducer::Target {
            wanted: self.wanted,
            element: None,
            slider: None,
        }
    }
}

/// The element kept for a UIA focus's node that the focus listener's
/// evidence says is no longer the node's element: the runtime id names a
/// new element, the one it named having lost the focus or gone
/// (`docs/design/focus-pipeline-rework.md`, "One object, one identity").
pub(super) struct Stale {
    /// The node.
    pub(super) node: NodeId,
    /// The element kept for it before the focus was read, evicted only if
    /// it is still the one kept, compared by agile reference.
    pub(super) element: Option<Agile<Kept>>,
    /// Whether the earlier element is gone: its terminal entry goes too.
    pub(super) gone: bool,
}

/// A focus becoming the followed one ([`Effects::follow`]).
#[expect(
    clippy::struct_excessive_bools,
    reason = "facts about one focus, each read before the commit"
)]
pub(super) struct Follow {
    /// The focus and its role.
    pub(super) node: NodeId,
    pub(super) role: Role,
    /// The object it is, as the intake records the focus.
    pub(super) object: Option<Object>,
    /// The window it was reported in.
    pub(super) window: Option<isize>,
    /// Its ancestors, outermost first, and itself.
    pub(super) chain: Vec<NodeSnapshot>,
    /// The batch its entry belongs to.
    pub(super) batch: u64,
    /// Whether it was reached by redirecting a control's own focus event
    /// to the control's focused child.
    pub(super) redirected: bool,
    /// Whether it is a terminal read through UI Automation, and whether its
    /// window is the console host's.
    pub(super) terminal: bool,
    pub(super) console: bool,
    /// For a terminal, the caret read with its report, noted as the caret
    /// last sent to Core.
    pub(super) sent_caret: Option<CaretReport>,
    /// Where the subscription is moved.
    pub(super) target: Target,
    /// What the focus owes once followed.
    pub(super) owed: Option<OwedWork>,
    /// The trace and observation of the event it came from.
    pub(super) trace: TraceId,
    pub(super) observed_at_us: u64,
    /// Whether it is adopted as the followed focus: false for a focus-now
    /// answer naming the focus already followed, under the same node,
    /// which keeps the tracking and the intake's marks as they are and is
    /// only followed again.
    pub(super) adopt: bool,
}

impl Follow {
    /// The reducer's part, its target's elements left to the shell.
    fn pure(&self) -> super::reducer::Follow {
        super::reducer::Follow {
            node: self.node,
            role: self.role,
            object: self.object.clone(),
            window: self.window,
            chain: self.chain.clone(),
            batch: self.batch,
            redirected: self.redirected,
            terminal: self.terminal,
            console: self.console,
            target: self.target.pure(),
            owed: self.owed.clone(),
            trace: self.trace,
            observed_at_us: self.observed_at_us,
            adopt: self.adopt,
        }
    }
}

/// What one commit does, built on the stack before the commit with every
/// call it needs already made.
#[derive(Default)]
pub(super) struct Effects {
    /// The program's result, which ends its query.
    pub(super) result: Option<OutpostToCore>,
    /// An answer given outside its query's own program (a caret key's
    /// watch), with its request.
    pub(super) aside: Option<(u64, OutpostToCore)>,
    /// The report to record for Core's judgement.
    pub(super) report: Option<Report>,
    /// A focus becoming the followed one.
    pub(super) follow: Option<Follow>,
    /// A foreground report's role and batch, with a caret key's watch on
    /// another node than this window's ended.
    pub(super) foreground: Option<(NodeId, Role, u64)>,
    /// The dialog memo the report leaves.
    pub(super) dialog: Option<DialogMemo>,
    /// The focus listener's mark of a UIA focus whose value is its first
    /// selected item; `None` leaves the mark as it was.
    #[expect(
        clippy::option_option,
        reason = "a mark to set, to clear, or to leave as it was"
    )]
    pub(super) value_from_selection: Option<Option<NodeId>>,
    /// The focus path of a UIA focus.
    pub(super) focus_path: Option<FocusPath>,
    /// The focus's element found after its report, with its slider's when
    /// found: the subscription is moved to them only while that node is
    /// the followed focus.
    pub(super) element: Option<(NodeId, Target)>,
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
    /// The intake's marks: the system caret's events dropped until the
    /// next focus; the console window's updates absorbed; the redirect mark
    /// cleared.
    pub(super) drop_system_caret: bool,
    pub(super) absorb: Option<isize>,
    pub(super) clear_redirect: bool,
    /// A held node's states, as an MSAA state change read them.
    pub(super) states: Option<(NodeId, StateSet)>,
    /// A UIA focus's stale element, evicted before the focus is followed.
    pub(super) stale: Option<Stale>,
    /// The program's judgements for the gap record.
    pub(super) gap: Vec<GapNote>,
    /// A held terminal's caret event, noted to be read once nothing else is
    /// waiting, with its event's timing.
    pub(super) noted_caret: Option<(NodeId, TraceId, u64, EventTiming)>,
    /// The verdicts probed, and the windows found destroyed.
    pub(super) verdicts: Vec<VerdictChange>,
    /// The program's text writes since its last commit, in order: its
    /// text facts' changes, which it applied to its own copies as it made
    /// them, with the ranges they name, and its patterns and support.
    pub(super) text: Vec<TextWrite>,
}

impl Effects {
    /// The reducer's part of these effects but for their messages, with no
    /// element token: what a program applies to its own copies.
    pub(super) fn local(&self) -> Change {
        Change {
            result: None,
            aside: None,
            report: self.report.clone(),
            follow: self.follow.as_ref().map(Follow::pure),
            foreground: self.foreground,
            dialog: self.dialog.clone(),
            value_from_selection: self.value_from_selection,
            focus_path: self.focus_path.clone(),
            element: self
                .element
                .as_ref()
                .map(|(node, target)| (*node, target.pure())),
            owed_done: self.owed_done,
            caret_read: self.caret_read,
            watch: self.watch.clone(),
            drop_system_caret: self.drop_system_caret,
            absorb: self.absorb,
            clear_redirect: self.clear_redirect,
            states: self.states,
            gap: self.gap.clone(),
            noted_caret: self.noted_caret,
            verdicts: self.verdicts.clone(),
            text: Vec::new(),
        }
    }

    /// [`local`](Self::local), with the changes of text facts these
    /// carry, in order: what the program the worker runs after the console
    /// poll applies of the poll's commit to its copies, which the poll had
    /// applied to its own as its text calls made them.
    pub(super) fn carried(&self) -> Change {
        Change {
            text: self
                .text
                .iter()
                .filter_map(|write| match write {
                    TextWrite::Facts { change, .. } => Some(change.clone()),
                    _ => None,
                })
                .collect::<Vec<TextChange>>(),
            ..self.local()
        }
    }

    /// The reducer's part, whose targets the shell gives their elements'
    /// tokens, and what only the shell applies.
    pub(super) fn split(self) -> (Change, Writes) {
        let change = Change {
            follow: self.follow.as_ref().map(Follow::pure),
            element: self
                .element
                .as_ref()
                .map(|(node, target)| (*node, target.pure())),
            report: self.report,
            result: self.result,
            aside: self.aside,
            foreground: self.foreground,
            dialog: self.dialog,
            value_from_selection: self.value_from_selection,
            focus_path: self.focus_path,
            owed_done: self.owed_done,
            caret_read: self.caret_read,
            watch: self.watch,
            drop_system_caret: self.drop_system_caret,
            absorb: self.absorb,
            clear_redirect: self.clear_redirect,
            states: self.states,
            gap: self.gap,
            noted_caret: self.noted_caret,
            verdicts: self.verdicts,
            text: Vec::new(),
        };
        let writes = Writes {
            follow: self.follow.map(|follow| {
                (
                    follow.node,
                    follow.terminal,
                    follow.sent_caret,
                    follow.target,
                )
            }),
            element: self.element.map(|(_, target)| target),
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
    /// The followed focus's node, whether it is a terminal, the caret sent
    /// with it, and its subscription target's elements.
    pub(super) follow: Option<(NodeId, bool, Option<CaretReport>, Target)>,
    /// The elements of a focus's target found after its report.
    pub(super) element: Option<Target>,
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
