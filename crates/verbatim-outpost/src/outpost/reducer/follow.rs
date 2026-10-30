//! What the outpost follows, and what a program's commit changes of it
//! (`docs/design/ownership-outpost-reducer.md`, "The reducer and its
//! thread"): the follow state, the focus line's paths, the caret key's
//! watch and owed work, all reducer state with no application object. A
//! program is given a copy ([`Facts`]) and applies its own commits to it
//! with the same function the reducer applies them with ([`apply`]), so
//! what it reads after a commit is what the reducer will hold.

use std::collections::HashSet;
use std::time::Duration;

use verbatim_model::protocol::{EventTiming, OutpostToCore, UiaFocusRead};
use verbatim_model::{
    CallCounts, CaretReport, CaretWatch, NodeId, NodeSnapshot, Role, StateSet, TraceId,
};

use super::held::{self, Held};
use super::intake::{Marks, Object};

/// The longest a caret key's watch stays open without evidence. It only
/// frees the watch: ending it this way says nothing, as ending it any other
/// way does. A key whose application raises its evidence later than this
/// is not spoken. Ten seconds, the time a program allows an application
/// that is busy, or starting up, to answer one event's reads.
pub(in crate::outpost) const CARET_WATCH_BOUND: Duration = Duration::from_secs(10);

/// How long after one of the console poll's reads the next is made.
pub(in crate::outpost) const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long after the key was passed on the console poll's last read is
/// made; a watch that has found no evidence by then ends, as at its own
/// bound.
pub(in crate::outpost) const POLL_BOUND: Duration = Duration::from_millis(50);

/// The focus-following subscription's event handler groups, as the
/// reducer names them (`verbatim_uia::Group`, variant for variant).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::outpost) enum Group {
    #[default]
    Base,
    Extended,
    ExtendedWithoutTextEvents,
}

/// The dialog the last foreground report gathered text for, which the
/// focus that follows it reuses: its node and its text.
pub(in crate::outpost) type DialogMemo = Option<(NodeId, Option<String>)>;

/// What the outpost remembers of the focus it follows across programs and
/// worker replacements: the focus and the facts about it the menu rules
/// and the walks read. A cache of reads and of what was reported, which
/// judges nothing Core was told.
#[derive(Clone, Debug, Default)]
pub(in crate::outpost) struct Tracking {
    /// The role of the focus or foreground window this outpost last
    /// reported, which the menu rules compare with; the focus's own role is
    /// `focus_role`.
    pub(in crate::outpost) role: Option<Role>,
    /// The batch in which a focus was last reported.
    pub(in crate::outpost) batch: Option<u64>,
    /// The last focus this outpost reported and its ancestors, outermost
    /// first: where the next focus's ancestor walk can stop and reuse the
    /// rest, as NVDA's does.
    pub(in crate::outpost) chain: Vec<NodeSnapshot>,
    /// The window of the last focus this outpost reported, for events on
    /// that focus, which then need no call to find their window.
    pub(in crate::outpost) window: Option<isize>,
    /// The node of the last focus this outpost reported, which it follows:
    /// its caret, text and terminal output are read, and the
    /// focus-following subscription is on it.
    pub(in crate::outpost) focus: Option<NodeId>,
    /// That focus's role, as last read, which decides how its text is
    /// read.
    pub(in crate::outpost) focus_role: Option<Role>,
    /// The dialog the last foreground report gathered text for.
    pub(in crate::outpost) dialog: DialogMemo,
    /// The UIA focus whose value is its first selected item's name, as the
    /// listener read it (`UiaFocusRead::value_from_selection`).
    pub(in crate::outpost) value_from_selection: Option<NodeId>,
}

/// The node whose caret was last read in a committed read, and when that
/// read began, in microseconds: a caret event observed before then changes
/// nothing the read did not see, and one observed later (while the read
/// was in flight included) may.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::outpost) struct CaretRead(Option<(u64, u64)>);

impl CaretRead {
    /// Notes a read of node `node`'s caret that began at `started_us`.
    pub(in crate::outpost) fn note(&mut self, node: u64, started_us: u64) {
        self.0 = Some((node, started_us));
    }

    /// Whether the last read of node `node`'s caret began at or after
    /// `observed_us`, so it saw what an event observed then reports.
    pub(in crate::outpost) fn covers(self, node: u64, observed_us: u64) -> bool {
        self.0
            .is_some_and(|(read, at)| read == node && at >= observed_us)
    }
}

/// What the followed focus still needs after the commit that made it
/// followed: owed work, attempted once per worker generation as a program
/// of its own with a deadline of its own.
#[derive(Clone, Debug)]
pub(in crate::outpost) struct Owed {
    /// The focus that owes it.
    pub(in crate::outpost) node: NodeId,
    /// What is owed.
    pub(in crate::outpost) work: OwedWork,
    /// The trace and observation of the event the focus came from.
    pub(in crate::outpost) trace: TraceId,
    pub(in crate::outpost) observed_at_us: u64,
    /// The worker generation attempting it, once one has started to.
    pub(in crate::outpost) attempting: Option<u64>,
}

/// The work a followed focus owes ([`Owed`]).
#[derive(Clone, Debug)]
pub(in crate::outpost) enum OwedWork {
    /// A terminal's first read, with the caret read with its report, or
    /// `None` for a terminal published at its walk's bound, whose caret is
    /// read first.
    FirstRead(Option<CaretReport>),
    /// A `SensitiveSlider`'s lookup, for a thumb published at its walk's
    /// bound, whose slider's element this outpost does not keep.
    Slider,
}

/// A caret key's watch for evidence, kept open between programs
/// (`docs/design/key-holding.md`).
#[derive(Clone, Debug)]
pub(in crate::outpost) struct OpenWatch {
    /// The request it answers.
    pub(in crate::outpost) request_id: u64,
    pub(in crate::outpost) trace: TraceId,
    /// The node whose caret the key moves.
    pub(in crate::outpost) node_id: NodeId,
    pub(in crate::outpost) watch: CaretWatch,
    /// When it was opened, on the reducer's clock, for
    /// [`CARET_WATCH_BOUND`].
    pub(in crate::outpost) opened: Duration,
    /// The request's timing, which its answer carries.
    pub(in crate::outpost) timing: EventTiming,
    /// The calls its checks have made so far, none of which found evidence.
    pub(in crate::outpost) calls: CallCounts,
    /// Its re-reads of the console host's caret, while they last.
    pub(in crate::outpost) poll: Option<Poll>,
}

/// A caret key's watch's poll of the console host's caret, kept in the
/// watch it belongs to: a documented workaround to waiting only for
/// evidence (`docs/design/key-holding.md`, "The console host's caret
/// poll"), since the console host raises its caret events only on its
/// cursor's blink timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::outpost) struct Poll {
    /// When the next read is due, on the reducer's clock.
    pub(in crate::outpost) next: Duration,
    /// When the last read is due.
    pub(in crate::outpost) until: Duration,
}

impl Poll {
    /// The poll of a watch opened at `opened` for a key passed on at
    /// `passed_at_ms`, by the clock of `now_ms` (0 when unknown, taken as
    /// `opened`).
    pub(in crate::outpost) fn new(opened: Duration, passed_at_ms: u64, now_ms: u64) -> Self {
        let until = if passed_at_ms == 0 {
            opened + POLL_BOUND
        } else {
            let bound_ms = u64::try_from(POLL_BOUND.as_millis()).unwrap_or(u64::MAX);
            opened + Duration::from_millis((passed_at_ms + bound_ms).saturating_sub(now_ms))
        };
        Self {
            next: (opened + POLL_INTERVAL).min(until),
            until,
        }
    }

    /// The poll after a read made at `now` found no evidence, or `None`
    /// when that read was its last.
    pub(in crate::outpost) fn after_read(self, now: Duration) -> Option<Self> {
        (now < self.until).then(|| Self {
            next: (now + POLL_INTERVAL).min(self.until),
            until: self.until,
        })
    }
}

/// When the reducer is to wake for `open`: at its poll's next read, or at
/// its bound ([`CARET_WATCH_BOUND`]) when it does not poll.
pub(in crate::outpost) fn wake_for(open: &OpenWatch) -> Duration {
    let bound = open.opened + CARET_WATCH_BOUND;
    open.poll.map_or(bound, |poll| poll.next.min(bound))
}

/// What a poll read that found no evidence leaves of the watch it was made
/// for ([`after_read`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::outpost) enum AfterRead {
    /// That watch is no longer open, or no longer polls: answered, ended or
    /// replaced meanwhile. The watch open now, if any, is left alone.
    Gone,
    /// The watch's next read is set.
    Again,
    /// The read was the poll's last: the watch is to end.
    Last,
}

/// Sets the next read of the watch for request `request_id`, if it is the
/// one in `open`, after a read of it at `now` found no evidence. The watch
/// is matched by its request, not its node: a newer watch on the same node
/// is another key's.
pub(in crate::outpost) fn after_read(
    open: &mut Option<OpenWatch>,
    request_id: u64,
    now: Duration,
) -> AfterRead {
    let Some(open) = open.as_mut().filter(|open| open.request_id == request_id) else {
        return AfterRead::Gone;
    };
    let Some(poll) = open.poll else {
        return AfterRead::Gone;
    };
    open.poll = poll.after_read(now);
    if open.poll.is_some() {
        AfterRead::Again
    } else {
        AfterRead::Last
    }
}

/// The raw-view chain of the UIA focus the outpost last reported from the
/// listener's read: the focus, then its ancestors, nearest first, each with
/// its runtime id and its own window handle (0 for none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::outpost) struct FocusPath {
    chain: Vec<(Vec<i32>, isize)>,
    /// Whether the listener's walk up from the focus did not reach the
    /// top: its read failed, or the walk was cut or ended at a failed step.
    cut: bool,
}

/// Where an element of a [`FocusPath`] is found from: the window whose
/// element the walk starts at, and the runtime ids below it, outermost
/// first, the last the element's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    /// The window whose element the walk starts at.
    pub(crate) window: isize,
    /// The runtime ids below it, outermost first.
    pub(crate) path: Vec<Vec<i32>>,
    /// The runtime id of the element wanted.
    pub(crate) runtime_id: Vec<i32>,
}

impl FocusPath {
    /// The chain of the focus `read` names.
    pub(in crate::outpost) fn of(read: &UiaFocusRead) -> Self {
        let chain = std::iter::once((read.snapshot.runtime_id.clone(), read.hwnd))
            .chain(
                read.ancestors
                    .iter()
                    .map(|ancestor| (ancestor.snapshot.runtime_id.clone(), ancestor.hwnd)),
            )
            .collect();
        let cut = read.ancestors_end != verbatim_model::protocol::AncestorsEnd::Top;
        Self { chain, cut }
    }

    /// Whether the chain begins with the focus `runtime_id` names, holds no
    /// window of its own anywhere, and the listener's walk up from it did
    /// not reach the top: the walk was cut below the focus's nearest
    /// window, so no route leads to its element ([`Self::route_to`]).
    pub(in crate::outpost) fn is_cut_at(&self, runtime_id: &[i32]) -> bool {
        self.cut
            && self.chain.first().is_some_and(|(id, _)| id == runtime_id)
            && self.chain.iter().all(|&(_, hwnd)| hwnd == 0)
    }

    /// The route to the element of the chain whose runtime id is
    /// `runtime_id`: from the nearest element at or above it that has a
    /// window of its own. `None` when no element of the chain has that
    /// runtime id, or none at or above it has a window.
    pub(in crate::outpost) fn route_to(&self, runtime_id: &[i32]) -> Option<Route> {
        let index = self.chain.iter().position(|(id, _)| id == runtime_id)?;
        let above = self.chain[index..]
            .iter()
            .position(|&(_, hwnd)| hwnd != 0)?;
        let root = index + above;
        let path = self.chain[index..root]
            .iter()
            .rev()
            .map(|(id, _)| id.clone())
            .collect();
        Some(Route {
            window: self.chain[root].1,
            path,
            runtime_id: runtime_id.to_vec(),
        })
    }
}

/// Where the focus-following subscription is to be moved for a focus: what
/// is followed, the node, its group and the slider followed for its value
/// changes, `None` for nothing; and the node's element and the slider's,
/// when they are kept, by the shell's tokens for them, so the reducer holds
/// no element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::outpost) struct Target {
    pub(in crate::outpost) wanted: Option<(NodeId, Group, Option<NodeId>)>,
    pub(in crate::outpost) element: Option<u64>,
    pub(in crate::outpost) slider: Option<u64>,
}

/// A focus becoming the followed one ([`Change::follow`]).
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "facts about one focus, each read before the commit"
)]
pub(in crate::outpost) struct Follow {
    /// The focus and its role.
    pub(in crate::outpost) node: NodeId,
    pub(in crate::outpost) role: Role,
    /// The object it is, as the intake records the focus.
    pub(in crate::outpost) object: Option<Object>,
    /// The window it was reported in.
    pub(in crate::outpost) window: Option<isize>,
    /// Its ancestors, outermost first, and itself.
    pub(in crate::outpost) chain: Vec<NodeSnapshot>,
    /// The batch its entry belongs to.
    pub(in crate::outpost) batch: u64,
    /// Whether it was reached by redirecting a control's own focus event
    /// to the control's focused child.
    pub(in crate::outpost) redirected: bool,
    /// Whether it is a terminal read through UI Automation, and whether its
    /// window is the console host's.
    pub(in crate::outpost) terminal: bool,
    pub(in crate::outpost) console: bool,
    /// Where the subscription is moved.
    pub(in crate::outpost) target: Target,
    /// What the focus owes once followed.
    pub(in crate::outpost) owed: Option<OwedWork>,
    /// The trace and observation of the event it came from.
    pub(in crate::outpost) trace: TraceId,
    pub(in crate::outpost) observed_at_us: u64,
    /// Whether it is adopted as the followed focus: false for a focus-now
    /// answer naming the focus already followed, under the same node,
    /// which keeps the tracking and the intake's marks as they are and is
    /// only followed again.
    pub(in crate::outpost) adopt: bool,
}

/// A change of the caret key's watch ([`Change::watch`]).
#[derive(Clone, Debug)]
pub(in crate::outpost) enum WatchChange {
    /// A watch opened, the reducer to wake at the time given; the query it
    /// answers is no longer the program's.
    Open(OpenWatch, Duration),
    /// A watch taken out to be checked, put back with the query no longer
    /// the program's.
    PutBack(OpenWatch),
    /// The watch on this node taken out to be checked, its query made the
    /// one an abandonment answers.
    Take(NodeId),
    /// The open watch ended with no evidence, answered `WatchEnded`; the
    /// reason is logged.
    End(&'static str),
    /// The open watch for this request had a poll read at the time given
    /// that found no evidence.
    Polled(u64, Duration),
    /// The reducer is to wake at the time given for the open watch, or not
    /// at all.
    Wake(Option<Duration>),
}

/// The report to record among those the outpost reads changes for
/// ([`Change::report`]).
#[derive(Clone, Debug)]
pub(in crate::outpost) enum Report {
    /// A focus or foreground change reported, with its own evidence of
    /// whether it repeats the newest report.
    Reported(held::Focus, held::Evidence),
    /// A focus-now answer's control, which keeps the event of the same
    /// focus judged against before it.
    Answered(held::Focus),
}

/// A program's judgement the gap record takes (`super::gap_record`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::outpost) enum GapNote {
    /// A copy in `window`, of top-level window `top_level`, observed at
    /// `at_us`, dropped because its window is judged UIA.
    CopyDropped {
        window: isize,
        top_level: isize,
        at_us: u64,
    },
    /// An event of `window` accepted: a foreground change, which
    /// `restamps`, or a copy handled through MSAA.
    Accepted {
        window: isize,
        top_level: isize,
        at_us: u64,
        restamps: bool,
    },
    /// `window` was destroyed.
    WindowDestroyed(isize),
}

/// What one commit of a program changes, its pure part: built by the
/// program with every call it needs already made, and applied by the
/// reducer, after the shell has made its store writes, in the order today's
/// commit applies them: its report recorded for Core's judgement, then its
/// messages sent, then the rest.
#[derive(Clone, Debug, Default)]
pub(in crate::outpost) struct Change {
    /// The program's result, which ends its query.
    pub(in crate::outpost) result: Option<OutpostToCore>,
    /// An answer given outside its query's own program (a caret key's
    /// watch), with its request.
    pub(in crate::outpost) aside: Option<(u64, OutpostToCore)>,
    /// The report to record for Core's judgement.
    pub(in crate::outpost) report: Option<Report>,
    /// A focus becoming the followed one.
    pub(in crate::outpost) follow: Option<Follow>,
    /// A foreground report's role and batch, with a caret key's watch on
    /// another node than this window's ended.
    pub(in crate::outpost) foreground: Option<(NodeId, Role, u64)>,
    /// The dialog memo the report leaves.
    pub(in crate::outpost) dialog: Option<DialogMemo>,
    /// The focus listener's mark of a UIA focus whose value is its first
    /// selected item; `None` leaves the mark as it was.
    #[expect(
        clippy::option_option,
        reason = "a mark to set, to clear, or to leave as it was"
    )]
    pub(in crate::outpost) value_from_selection: Option<Option<NodeId>>,
    /// The focus path of a UIA focus.
    pub(in crate::outpost) focus_path: Option<FocusPath>,
    /// The focus's element found after its report, with its slider's when
    /// found: the subscription is moved to them only while that node is
    /// the followed focus.
    pub(in crate::outpost) element: Option<(NodeId, Target)>,
    /// The owed work of this node is done, read or failed.
    pub(in crate::outpost) owed_done: Option<NodeId>,
    /// A read of this node's caret began at the instant given, in
    /// microseconds.
    pub(in crate::outpost) caret_read: Option<(NodeId, u64)>,
    /// A change of the caret key's watch.
    pub(in crate::outpost) watch: Option<WatchChange>,
    /// The intake's marks: the system caret's events dropped until the
    /// next focus; the console window's updates absorbed; the redirect mark
    /// cleared.
    pub(in crate::outpost) drop_system_caret: bool,
    pub(in crate::outpost) absorb: Option<isize>,
    pub(in crate::outpost) clear_redirect: bool,
    /// A held node's states, as an MSAA state change read them.
    pub(in crate::outpost) states: Option<(NodeId, StateSet)>,
    /// The program's judgements for the gap record, in order.
    pub(in crate::outpost) gap: Vec<GapNote>,
    /// A held terminal's caret event, noted to be read once nothing else
    /// is waiting.
    pub(in crate::outpost) noted_caret: Option<(NodeId, TraceId, u64)>,
}

/// What the outpost follows: changed only by a commit the reducer applies,
/// and copied into each program.
#[derive(Clone, Debug, Default)]
pub(in crate::outpost) struct FollowState {
    /// The focus this outpost follows.
    pub(in crate::outpost) tracking: Tracking,
    /// The focus's node and group the focus-following subscription was
    /// last moved to, with the focus's parent followed for its value
    /// changes, if one is: `None` for nothing.
    pub(in crate::outpost) followed: Option<(NodeId, Group, Option<NodeId>)>,
    /// The token of the element the subscription was last moved to, so a
    /// node whose element was replaced is followed again.
    pub(in crate::outpost) followed_element: Option<u64>,
    /// The elements of the outpost's older UIA reports, each with its node
    /// and the group it was given, by token, which keep their groups until
    /// the store releases their node, so the outpost hears the changes of
    /// every node it has not released (`docs/design/ownership-one-judge.md`,
    /// "UIA: one focus-following group per unreleased report").
    pub(in crate::outpost) kept_groups: Vec<(NodeId, Group, u64)>,
    /// The caret key's watch for evidence kept open between programs.
    pub(in crate::outpost) caret_watch: Option<OpenWatch>,
    /// The last committed read of a caret.
    pub(in crate::outpost) caret_read: CaretRead,
    /// The raw-view chain the focus listener read for the UIA focus this
    /// outpost last reported, by which it finds its own copies of that
    /// focus's elements.
    pub(in crate::outpost) focus_path: Option<FocusPath>,
    /// What the followed focus still owes.
    pub(in crate::outpost) owed: Option<Owed>,
}

/// What a program is given a copy of: everything it reads of what the
/// outpost remembers.
#[derive(Clone, Debug, Default)]
pub(in crate::outpost) struct Facts {
    /// The reports whose focus node the outpost has not released.
    pub(in crate::outpost) held: Held,
    /// What the outpost follows.
    pub(in crate::outpost) follow: FollowState,
    /// What the intake reads of the focus.
    pub(in crate::outpost) marks: Marks,
}

/// Where the subscription is to follow, by token: the newest focus's
/// element with its group, the slider's with the value group, and every
/// kept element with its own group ([`Followed::Subscribe`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::outpost) struct Subscription {
    pub(in crate::outpost) newest: Option<(u64, Group)>,
    pub(in crate::outpost) slider: Option<u64>,
    pub(in crate::outpost) kept: Vec<(u64, Group)>,
}

/// What applying a commit asks of the shell besides the follow state
/// itself.
#[derive(Debug, Default)]
pub(in crate::outpost) struct Applied {
    /// The caret key's watch taken out by [`WatchChange::Take`].
    pub(in crate::outpost) taken: Option<OpenWatch>,
    /// Answers to send: an ended watch's `WatchEnded`, with why it ended.
    pub(in crate::outpost) ended: Vec<(OpenWatch, &'static str)>,
    /// The subscription's new targets, when it is to move.
    pub(in crate::outpost) subscribe: Option<Subscription>,
    /// The element tokens no longer followed, for the shell to drop.
    pub(in crate::outpost) unfollowed: Vec<u64>,
    /// The intake's marks for the focus adopted.
    pub(in crate::outpost) adopted: Option<Marks>,
    /// The watch's wake to set, or clear with `Some(None)`.
    #[expect(
        clippy::option_option,
        reason = "a wake to set, to clear, or to leave as it was"
    )]
    pub(in crate::outpost) wake: Option<Option<Duration>>,
}

impl FollowState {
    /// Ends the open caret key's watch, if there is one, with its
    /// `WatchEnded` answer and its wake cancelled.
    fn end_watch(&mut self, why: &'static str, applied: &mut Applied) {
        if let Some(open) = self.caret_watch.take() {
            applied.wake = Some(None);
            applied.ended.push((open, why));
        }
    }

    /// Ends a caret key's watch on a node other than `node`, the new focus
    /// or foreground window, as NVDA's wait gives way to a focus change.
    fn end_watch_elsewhere(&mut self, node: NodeId, applied: &mut Applied) {
        if self
            .caret_watch
            .as_ref()
            .is_some_and(|open| open.node_id != node)
        {
            self.end_watch("the focus moved", applied);
        }
    }

    /// Moves the focus-following subscription's newest focus to `target`,
    /// unless it is already there. The focus it followed before keeps its
    /// group on its element, among [`kept_groups`](Self::kept_groups),
    /// until its node is released; a focus that returns to an element
    /// that kept its group is not given it again.
    fn retarget(&mut self, target: &Target, applied: &mut Applied) {
        if self.followed == target.wanted && self.followed_element == target.element {
            applied.unfollowed.extend(target.slider);
            return;
        }
        let before = std::mem::replace(&mut self.followed, target.wanted);
        let before_element = std::mem::replace(&mut self.followed_element, target.element);
        if let (Some((node, group, _)), Some(element)) = (before, before_element) {
            let kept = self
                .kept_groups
                .iter()
                .position(|(kept, _, _)| *kept == node);
            if let Some(kept) = kept {
                let (_, _, replaced) =
                    std::mem::replace(&mut self.kept_groups[kept], (node, group, element));
                if replaced != element {
                    applied.unfollowed.push(replaced);
                }
            } else {
                self.kept_groups.push((node, group, element));
            }
        }
        // The newest focus's element is followed as the newest, not kept.
        if let Some((node, _, _)) = target.wanted {
            let mut index = 0;
            while index < self.kept_groups.len() {
                if self.kept_groups[index].0 == node {
                    let (_, _, removed) = self.kept_groups.remove(index);
                    if Some(removed) != self.followed_element {
                        applied.unfollowed.push(removed);
                    }
                } else {
                    index += 1;
                }
            }
        }
        applied.subscribe = Some(self.subscription(target.slider));
    }

    /// Where the subscription follows now, with `slider` followed for its
    /// value changes.
    fn subscription(&self, slider: Option<u64>) -> Subscription {
        let kept = self
            .kept_groups
            .iter()
            .map(|&(_, group, element)| (element, group))
            .collect();
        let newest = match (self.followed, self.followed_element) {
            (Some((_, group, _)), Some(element)) => Some((element, group)),
            _ => None,
        };
        Subscription {
            newest,
            slider: slider.filter(|_| newest.is_some()),
            kept,
        }
    }

    /// The store released `released`, by node number: the elements of
    /// those nodes lose their groups, the newest focus's too when its node
    /// is released, and the subscription is moved once if anything
    /// changed.
    pub(in crate::outpost) fn release_groups(
        &mut self,
        released: &HashSet<u64>,
        applied: &mut Applied,
    ) {
        let mut changed = false;
        let mut index = 0;
        while index < self.kept_groups.len() {
            if released.contains(&self.kept_groups[index].0.number()) {
                applied.unfollowed.push(self.kept_groups.remove(index).2);
                changed = true;
            } else {
                index += 1;
            }
        }
        if self
            .followed
            .is_some_and(|(node, _, _)| released.contains(&node.number()))
        {
            self.followed = None;
            applied.unfollowed.extend(self.followed_element.take());
            changed = true;
        }
        if changed {
            applied.subscribe = Some(self.subscription(None));
        }
    }
}

/// Applies the pure part of a commit of `generation`'s, `change`, but for
/// its report and messages, which the caller records and sends first: to
/// the reducer's follow state, reports and marks, or to a program's copies
/// of them.
pub(in crate::outpost) fn apply(facts: &mut Facts, generation: u64, change: &Change) -> Applied {
    let mut applied = Applied::default();
    let follow_state = &mut facts.follow;
    if let Some(memo) = &change.dialog {
        follow_state.tracking.dialog.clone_from(memo);
    }
    if let Some(follow) = &change.follow {
        if follow.adopt {
            follow_state.end_watch_elsewhere(follow.node, &mut applied);
            // The intake's marks in one go: a terminal read through UIA
            // has its caret event planned after its text changes, and a
            // console host's terminal, whose console caret event is its
            // caret event, has the system caret's events dropped
            // (`docs/parity/text.md`).
            let marks = Marks {
                focused: follow.object.clone(),
                terminal: follow.terminal.then(|| follow.window.unwrap_or(0)),
                focus_redirected: follow.redirected,
                absorbed: None,
                system_caret_unused: follow.terminal && follow.console,
            };
            facts.marks = marks.clone();
            applied.adopted = Some(marks);
            let tracking = &mut follow_state.tracking;
            tracking.role = Some(follow.role);
            tracking.focus = Some(follow.node);
            tracking.focus_role = Some(follow.role);
            tracking.window = follow.window;
            tracking.chain.clone_from(&follow.chain);
            tracking.batch = Some(follow.batch);
        }
        follow_state.retarget(&follow.target, &mut applied);
        follow_state.owed = follow.owed.clone().map(|work| Owed {
            node: follow.node,
            work,
            trace: follow.trace,
            observed_at_us: follow.observed_at_us,
            attempting: None,
        });
    }
    if let Some(path) = &change.focus_path {
        follow_state.focus_path = Some(path.clone());
    }
    if let Some(value_from_selection) = change.value_from_selection {
        follow_state.tracking.value_from_selection = value_from_selection;
    }
    if let Some((node, role, batch)) = change.foreground {
        follow_state.end_watch_elsewhere(node, &mut applied);
        follow_state.tracking.role = Some(role);
        follow_state.tracking.batch = Some(batch);
    }
    if let Some((node, target)) = &change.element {
        if follow_state.tracking.focus == Some(*node) {
            follow_state.retarget(target, &mut applied);
        } else {
            applied
                .unfollowed
                .extend(target.element.into_iter().chain(target.slider));
        }
    }
    if let Some(node) = change.owed_done
        && follow_state
            .owed
            .as_ref()
            .is_some_and(|owed| owed.node == node && owed.attempting == Some(generation))
    {
        follow_state.owed = None;
    }
    if let Some((node, started_us)) = change.caret_read {
        follow_state.caret_read.note(node.number(), started_us);
    }
    if change.drop_system_caret {
        facts.marks.system_caret_unused = true;
    }
    if change.absorb.is_some() {
        facts.marks.absorbed = change.absorb;
    }
    if change.clear_redirect {
        facts.marks.focus_redirected = false;
    }
    if let Some((node, states)) = change.states {
        let _ = facts.held.replace_states(node, states);
    }
    match &change.watch {
        None => {}
        Some(WatchChange::Open(open, wake)) => {
            applied.wake = Some(Some(*wake));
            follow_state.caret_watch = Some(open.clone());
        }
        Some(WatchChange::PutBack(open)) => follow_state.caret_watch = Some(open.clone()),
        Some(WatchChange::Take(node)) => {
            applied.taken = follow_state
                .caret_watch
                .take_if(|open| open.node_id == *node);
        }
        Some(WatchChange::End(why)) => follow_state.end_watch(why, &mut applied),
        Some(WatchChange::Polled(request_id, at)) => {
            if after_read(&mut follow_state.caret_watch, *request_id, *at) == AfterRead::Last {
                follow_state.end_watch("its poll's bound passed", &mut applied);
            }
        }
        Some(WatchChange::Wake(at)) => applied.wake = Some(*at),
    }
    applied
}

/// Records `report` among the reports changes are read for.
pub(in crate::outpost) fn record(held: &mut Held, report: &Report) {
    match report {
        Report::Reported(focus, evidence) => held.reported(focus.clone(), *evidence),
        Report::Answered(focus) => {
            let focus = answer_to_judge(held.expected(), focus.clone());
            // An answer of the newest report's node is the same element
            // read again.
            held.reported(focus, held::Evidence::Uia(None));
        }
    }
}

/// A focus-now answer's control, `answered`, as the outpost records it
/// among its reports, given `before`, the newest report: an answer for the
/// same node keeps the MSAA event address and the simple-child mark its
/// report recorded, as NVDA's fake focus leaves its last queued focus's
/// event as it was.
pub(in crate::outpost) fn answer_to_judge(
    before: Option<&held::Focus>,
    answered: held::Focus,
) -> held::Focus {
    match before.filter(|before| before.node.id == answered.node.id) {
        Some(before) => held::Focus {
            event: before.event,
            simple_child: before.simple_child,
            ..answered
        },
        None => answered,
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::TextUnit;

    use super::*;

    /// A watch for request `request_id` on node 1, opened at `opened`,
    /// polling as `poll` says.
    fn watch(request_id: u64, opened: Duration, poll: Option<Poll>) -> OpenWatch {
        OpenWatch {
            request_id,
            trace: TraceId::mint(),
            node_id: NodeId::new(1),
            watch: CaretWatch {
                since: None,
                pressed_at_ms: 0,
                unit: TextUnit::Character,
                compare: None,
                landing: false,
                deletes: false,
            },
            opened,
            timing: EventTiming::default(),
            calls: CallCounts::default(),
            poll,
        }
    }

    #[test]
    fn a_polling_watch_wakes_the_reducer_at_its_next_read() {
        let opened = Duration::from_secs(1);
        let poll = Poll::new(opened, 0, 0);
        assert_eq!(
            wake_for(&watch(7, opened, Some(poll))),
            opened + POLL_INTERVAL
        );
        assert_eq!(
            wake_for(&watch(7, opened, None)),
            opened + CARET_WATCH_BOUND
        );
    }

    #[test]
    fn a_read_for_a_replaced_watch_leaves_the_newer_one_on_its_node_alone() {
        let opened = Duration::from_secs(1);
        let newer = Poll::new(opened, 0, 0);
        let mut open = Some(watch(8, opened, Some(newer)));
        // The read for request 7, the older key's, returns after the newer
        // key's watch on the same node opened, at that watch's bound.
        assert_eq!(
            after_read(&mut open, 7, opened + POLL_BOUND),
            AfterRead::Gone
        );
        let open = open.expect("the newer watch stays open");
        assert_eq!(open.poll.map(|poll| poll.next), Some(newer.next));
    }

    #[test]
    fn a_read_at_the_bound_is_the_watchs_last() {
        let opened = Duration::from_secs(1);
        let mut open = Some(watch(7, opened, Some(Poll::new(opened, 0, 0))));
        assert_eq!(
            after_read(&mut open, 7, opened + POLL_INTERVAL),
            AfterRead::Again
        );
        assert_eq!(
            after_read(&mut open, 7, opened + POLL_BOUND),
            AfterRead::Last
        );
    }

    #[test]
    fn the_poll_ends_its_bound_after_the_key_was_passed_on() {
        let opened = Duration::from_secs(1);
        // Opened 3 ms after the key was passed on.
        let poll = Poll::new(opened, 1_000, 1_003);
        assert_eq!(poll.until, opened + Duration::from_millis(47));
        assert_eq!(poll.next, opened + POLL_INTERVAL);
    }

    #[test]
    fn a_poll_without_the_keys_time_ends_its_bound_after_it_opened() {
        let opened = Duration::from_secs(1);
        let poll = Poll::new(opened, 0, 1_003);
        assert_eq!(poll.until, opened + POLL_BOUND);
    }

    #[test]
    fn each_read_is_followed_by_the_next_until_the_bound() {
        let opened = Duration::from_secs(1);
        let poll = Poll::new(opened, 1_000, 1_000);
        let read_at = opened + Duration::from_millis(47);
        let next = poll.after_read(read_at).expect("a read before the bound");
        assert_eq!(next.next, opened + POLL_BOUND);
        assert!(next.after_read(opened + POLL_BOUND).is_none());
    }

    #[test]
    fn an_event_observed_while_a_read_was_in_flight_is_not_covered() {
        let mut read = CaretRead::default();
        // A read of node 1 begins at 100 and returns at 200; a caret event
        // is observed at 150, while it was in flight.
        read.note(1, 100);
        assert!(!read.covers(1, 150));
        // One observed before the read began is covered; another node's
        // never is.
        assert!(read.covers(1, 100));
        assert!(read.covers(1, 90));
        assert!(!read.covers(2, 90));
    }
}
