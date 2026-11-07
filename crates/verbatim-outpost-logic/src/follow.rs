//! What the outpost follows, and what a program's commit changes of it
//! (`books/dev/src/explanation/design/ownership.md`): the follow state, the focus line's paths, the caret key's
//! watch and owed work, all reducer state with no application object. A
//! program is given a copy ([`Facts`]) and applies its own commits to it
//! with the same function the reducer applies them with ([`apply`]), so
//! what it reads after a commit is what the reducer will hold.

use std::collections::BTreeSet;
use std::sync::Arc;
use verbatim_model::protocol::{EventTiming, OutpostToCore, UiaFocusRead};
use verbatim_model::{CaretReport, NodeId, NodeSnapshot, Role, StateSet, TraceId};

use verbatim_reader_rules::Window;

use super::held::{self, Held};
use super::intake::{Marks, Object, TerminalMark};
use super::text::{RangeKey, TextChange, TextFacts};
use super::verdicts::Verdicts;
use super::watch::{Answer, Checked, Clock, OpenWatch, Watch};

/// The focus-following subscription's event handler groups, as the
/// reducer names them (`verbatim_uia::Group`, variant for variant).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Group {
    #[default]
    /// The events every focus is followed for.
    Base,
    /// The base events and a text control's text events.
    Extended,
    /// The extended events without the text events.
    ExtendedWithoutTextEvents,
}

/// The dialog the last foreground report gathered text for, which the
/// focus that follows it reuses: its node and its text.
pub type DialogMemo = Option<(NodeId, Option<String>)>;

/// What the outpost remembers of the focus it follows across programs and
/// worker replacements: the focus and the facts about it the menu rules
/// and the walks read. A cache of reads and of what was reported, which
/// judges nothing Core was told.
#[derive(Clone, Debug, Default)]
pub struct Tracking {
    /// The role of the focus or foreground window this outpost last
    /// reported, which the menu rules compare with; the focus's own role is
    /// `focus_role`.
    pub role: Option<Role>,
    /// The batch in which a focus was last reported.
    pub batch: Option<u64>,
    /// The last focus this outpost reported and its ancestors, outermost
    /// first: where the next focus's ancestor walk can stop and reuse the
    /// rest, as NVDA's does.
    pub chain: Vec<NodeSnapshot>,
    /// The window of the last focus this outpost reported, for events on
    /// that focus, which then need no call to find their window.
    pub window: Option<isize>,
    /// The node of the last focus this outpost reported, which it follows:
    /// its caret, text and terminal output are read, and the
    /// focus-following subscription is on it.
    pub focus: Option<NodeId>,
    /// That focus's role, as last read, which decides how its text is
    /// read.
    pub focus_role: Option<Role>,
    /// The dialog the last foreground report gathered text for.
    pub dialog: DialogMemo,
    /// The UIA focus whose value is its first selected item's name, as the
    /// listener read it (`UiaFocusRead::value_from_selection`).
    pub value_from_selection: Option<NodeId>,
}

/// The node whose caret was last read in a committed read, and when that
/// read began, in microseconds: a caret event observed before then changes
/// nothing the read did not see, and one observed later (while the read
/// was in flight included) may.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaretRead(Option<(u64, u64)>);

impl CaretRead {
    /// Notes a read of node `node`'s caret that began at `started_us`.
    pub fn note(&mut self, node: u64, started_us: u64) {
        self.0 = Some((node, started_us));
    }

    /// Whether the last read of node `node`'s caret began at or after
    /// `observed_us`, so it saw what an event observed then reports.
    pub fn covers(self, node: u64, observed_us: u64) -> bool {
        self.0
            .is_some_and(|(read, at)| read == node && at >= observed_us)
    }
}

/// What the followed focus still needs after the commit that made it
/// followed: owed work, attempted once per worker generation as a program
/// of its own with a deadline of its own.
#[derive(Clone, Debug)]
pub struct Owed {
    /// The focus that owes it.
    pub node: NodeId,
    /// What is owed.
    pub work: OwedWork,
    /// The trace and observation of the event the focus came from.
    pub trace: TraceId,
    /// The event's observation, in microseconds on the run's shared clock.
    pub observed_at_us: u64,
    /// The worker generation attempting it, once one has started to.
    pub attempting: Option<u64>,
}

/// The work a followed focus owes ([`Owed`]).
#[derive(Clone, Debug)]
pub enum OwedWork {
    /// A terminal's first read, with the caret read with its report.
    FirstRead(CaretReport),
}

/// The raw-view chain of the UIA focus the outpost last reported from the
/// listener's read: the focus, then its ancestors, nearest first, each with
/// its runtime id and its own window, if it has one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusPath {
    chain: Vec<(Vec<i32>, Option<Window>)>,
    /// Whether the listener's walk up from the focus did not reach the
    /// top: its read failed, or the walk was cut or ended at a failed step.
    cut: bool,
    /// The focus's nearest window, as the read carries it, for a chain that
    /// was cut: where a route starts when no element of the chain at or
    /// above the one wanted has a window of its own.
    nearest: Option<Window>,
}

/// Where an element of a [`FocusPath`] is found from: the window whose
/// element the walk starts at, and the runtime ids below it, outermost
/// first, the last the element's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// The window whose element the walk starts at.
    pub window: Window,
    /// The runtime ids below it, outermost first.
    pub path: Vec<Vec<i32>>,
    /// The runtime id of the element wanted.
    pub runtime_id: Vec<i32>,
}

impl FocusPath {
    /// The chain of the focus `read` names.
    #[must_use]
    pub fn of(read: &UiaFocusRead) -> Self {
        let chain = std::iter::once((read.snapshot.runtime_id.clone(), Window::new(read.hwnd)))
            .chain(read.ancestors.iter().map(|ancestor| {
                (
                    ancestor.snapshot.runtime_id.clone(),
                    Window::new(ancestor.hwnd),
                )
            }))
            .collect();
        let cut = read.ancestors_end != verbatim_model::protocol::AncestorsEnd::Top;
        let nearest = read.window.and_then(Window::new).filter(|_| cut);
        Self {
            chain,
            cut,
            nearest,
        }
    }

    /// Whether the chain begins with the focus `runtime_id` names, holds no
    /// window of its own anywhere, and the listener's walk up from it did
    /// not reach the top: the walk was cut below the focus's nearest
    /// window, so no route leads to its element ([`Self::route_to`]).
    #[must_use]
    pub fn is_cut_at(&self, runtime_id: &[i32]) -> bool {
        self.cut
            && self.chain.first().is_some_and(|(id, _)| id == runtime_id)
            && self.chain.iter().all(|(_, window)| window.is_none())
    }

    /// The route to the element of the chain whose runtime id is
    /// `runtime_id`: from the nearest element at or above it that has a
    /// window of its own; or, for a chain whose walk up was cut below the
    /// focus's nearest window, so that none at or above it has one, from
    /// that nearest window, down by every runtime id of the chain from its
    /// outermost element, which finds the element when the outermost is
    /// the window's child, as a walk whose first step failed leaves a
    /// child of its window. `None` when no element of the chain has that
    /// runtime id, or none at or above it has a window and the chain was
    /// not cut.
    #[must_use]
    pub fn route_to(&self, runtime_id: &[i32]) -> Option<Route> {
        let index = self.chain.iter().position(|(id, _)| id == runtime_id)?;
        let from = self.chain.get(index..)?;
        let (above, window) = from
            .iter()
            .enumerate()
            .find_map(|(above, &(_, window))| Some((above, window?)))
            .or_else(|| Some((from.len(), self.nearest?)))?;
        let path = from
            .get(..above)?
            .iter()
            .rev()
            .map(|(id, _)| id.clone())
            .collect();
        Some(Route {
            window,
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
pub struct Target {
    /// The node followed, its group, and the slider followed for its value
    /// changes; `None` for nothing.
    pub wanted: Option<(NodeId, Group, Option<NodeId>)>,
    /// The node's element, by the shell's token for it, when it is kept.
    pub element: Option<u64>,
    /// The slider's element, by the shell's token for it, when it is kept.
    pub slider: Option<u64>,
}

/// A focus becoming the followed one ([`Change::follow`]).
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "facts about one focus, each read before the commit"
)]
pub struct Follow {
    /// The focus and its role.
    pub node: NodeId,
    /// The focus's role.
    pub role: Role,
    /// The object it is, as the intake records the focus.
    pub object: Option<Object>,
    /// The window it was reported in.
    pub window: Option<isize>,
    /// Its ancestors, outermost first, and itself.
    pub chain: Vec<NodeSnapshot>,
    /// The batch its entry belongs to.
    pub batch: u64,
    /// Whether it was reached by redirecting a control's own focus event
    /// to the control's focused child.
    pub redirected: bool,
    /// Whether it is a terminal read through UI Automation, and whether its
    /// window is the console host's.
    pub terminal: bool,
    /// Whether its window is the console host's.
    pub console: bool,
    /// Where the subscription is moved.
    pub target: Target,
    /// What the focus owes once followed.
    pub owed: Option<OwedWork>,
    /// The trace and observation of the event it came from.
    pub trace: TraceId,
    /// The event's observation, in microseconds on the run's shared clock.
    pub observed_at_us: u64,
    /// Whether it is adopted as the followed focus: false for a focus-now
    /// answer naming the focus already followed, under the same node,
    /// which keeps the tracking and the intake's marks as they are and is
    /// only followed again.
    pub adopt: bool,
}

/// A change of the caret key's watch ([`Change::watch`]), a program's
/// commit: the reducer decides every time.
#[derive(Clone, Debug)]
pub enum WatchChange {
    /// A caret key's first check found nothing: the watch opens, polling
    /// when its node is the focused console host's text area (`console`),
    /// and its query is no longer the program's.
    Open {
        /// The watch.
        open: OpenWatch,
        /// Whether its node is the focused console host's text area.
        console: bool,
    },
    /// The watch on this node taken by the program to be checked.
    Take(NodeId),
    /// The held watch's check found this.
    Checked(Checked),
}

/// The report to record among those the outpost reads changes for
/// ([`Change::report`]).
#[derive(Clone, Debug)]
pub enum Report {
    /// A focus or foreground change reported, with its own evidence of
    /// whether it repeats the newest report.
    Reported(held::Focus, held::Evidence),
    /// A focus-now answer's control, which keeps the event of the same
    /// focus judged against before it.
    Answered(held::Focus),
}

/// What one commit of a program changes, its pure part: built by the
/// program with every call it needs already made, and applied by the
/// reducer, after the shell has made its store writes, in the order today's
/// commit applies them: its report recorded for Core's judgement, then its
/// messages sent, then the rest.
#[derive(Clone, Debug, Default)]
pub struct Change {
    /// The program's result, which ends its query.
    pub result: Option<OutpostToCore>,
    /// The report to record for Core's judgement.
    pub report: Option<Report>,
    /// A focus becoming the followed one.
    pub follow: Option<Follow>,
    /// A foreground report's role and batch, with a caret key's watch on
    /// another node than this window's ended.
    pub foreground: Option<(NodeId, Role, u64)>,
    /// The dialog memo the report leaves.
    pub dialog: Option<DialogMemo>,
    /// The focus listener's mark of a UIA focus whose value is its first
    /// selected item; `None` leaves the mark as it was.
    pub value_from_selection: Option<Option<NodeId>>,
    /// The focus path of a UIA focus.
    pub focus_path: Option<FocusPath>,
    /// The focus's element found after its report, with its slider's when
    /// found: the subscription is moved to them only while that node is
    /// the followed focus.
    pub element: Option<(NodeId, Target)>,
    /// The owed work of this node is done, read or failed.
    pub owed_done: Option<NodeId>,
    /// A read of this node's caret began at the instant given, in
    /// microseconds.
    pub caret_read: Option<(NodeId, u64)>,
    /// A change of the caret key's watch.
    pub watch: Option<WatchChange>,
    /// The intake's marks: the system caret's events dropped until the
    /// next focus; the console window's updates absorbed; the redirect mark
    /// cleared.
    pub drop_system_caret: bool,
    /// The console window whose updates are absorbed.
    pub absorb: Option<isize>,
    /// Whether the redirect mark is cleared.
    pub clear_redirect: bool,
    /// A held node's states, as an MSAA state change read them.
    pub states: Option<(NodeId, StateSet)>,
    /// A held terminal's caret event, noted to be read once nothing else
    /// is waiting, with its event's timing, which the read of the caret
    /// judges the caret reads since the event by.
    pub noted_caret: Option<(NodeId, TraceId, u64, EventTiming)>,

    /// The node of a UIA focus whose earlier element, by the focus
    /// listener's evidence, lost the focus or is gone, so a new element
    /// stands under it: a caret key's watch on it ends, as Core forgets
    /// the key's query (`docs/parity/focus-events.md`, "Duplicate focus
    /// suppression").
    pub replaced: Option<NodeId>,
    /// The program's changes of text facts, in order, applied before the
    /// rest: a program applies each to its own copies as its text call
    /// makes it, so its own commit carries them only for the reducer.
    pub text: Vec<TextChange>,
    /// The program's window and what the program found there, when it
    /// found anything: the evidence the reducer's record of hung windows
    /// is kept by.
    pub found: Option<(Window, Found)>,
}

/// What a program found of its window, which its commits name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// Its check before a call found the window hung or ghosted, and no
    /// call was made into it.
    Hung,
    /// A counted call into it returned.
    Answered,
}

/// What the outpost follows: changed only by a commit the reducer applies,
/// and copied into each program.
#[derive(Clone, Debug, Default)]
pub struct FollowState {
    /// The focus this outpost follows.
    pub tracking: Tracking,
    /// The focus's node and group the focus-following subscription was
    /// last moved to, with the focus's parent followed for its value
    /// changes, if one is: `None` for nothing.
    pub followed: Option<(NodeId, Group, Option<NodeId>)>,
    /// The token of the element the subscription was last moved to, so a
    /// node whose element was replaced is followed again.
    pub followed_element: Option<u64>,
    /// The token of the slider followed with it for its value changes, so
    /// a slider whose element was replaced is followed again too.
    pub followed_slider: Option<u64>,
    /// The elements of the outpost's older UIA reports, each with its node
    /// and the group it was given, by token, which keep their groups until
    /// the store releases their node, so the outpost hears the changes of
    /// every node it has not released (`books/dev/src/explanation/design/ownership.md`).
    pub kept_groups: Vec<(NodeId, Group, u64)>,
    /// The caret key's watch for evidence.
    pub watch: Watch,
    /// The last committed read of a caret.
    pub caret_read: CaretRead,
    /// The raw-view chain the focus listener read for the UIA focus this
    /// outpost last reported, by which it finds its own copies of that
    /// focus's elements.
    pub focus_path: Option<FocusPath>,
    /// What the followed focus still owes.
    pub owed: Option<Owed>,
}

/// What a program is given a copy of: everything it reads of what the
/// outpost remembers.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// The reports whose focus node the outpost has not released.
    pub held: Held,
    /// What the outpost follows.
    pub follow: FollowState,
    /// What the intake reads of the focus.
    pub marks: Marks,
    /// The arbitrator's kept verdicts, a snapshot the reducer replaces on
    /// change ([`super::verdicts`]).
    pub verdicts: Arc<Verdicts>,
    /// Each node's text facts, shared until a change ([`super::text`]).
    pub text: Arc<TextFacts>,
}

/// Where the subscription is to follow, by token: the newest focus's
/// element with its group, the slider's with the value group, and every
/// kept element with its own group (`Followed::Subscribe`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    /// The newest focus's element, by token, with its group.
    pub newest: Option<(u64, Group)>,
    /// The slider's element, by token, followed for its value changes.
    pub slider: Option<u64>,
    /// Every kept element, by token, with its own group.
    pub kept: Vec<(u64, Group)>,
}

/// What applying a commit asks of the shell besides the follow state
/// itself.
#[derive(Debug, Default)]
pub struct Applied {
    /// The caret key's watch taken by [`WatchChange::Take`].
    pub taken: Option<OpenWatch>,
    /// Whether [`WatchChange::Open`] opened the watch.
    pub opened: bool,
    /// Whether [`WatchChange::Checked`] found the watch held and found no
    /// evidence.
    pub resumed: bool,
    /// The watch's answers to send.
    pub answers: Vec<Answer>,
    /// The subscription's new targets, when it is to move.
    pub subscribe: Option<Subscription>,
    /// The element tokens no longer followed, for the shell to drop.
    pub unfollowed: Vec<u64>,
    /// The UIA ranges no text fact names any more, by node and key, for
    /// the store to let go of.
    pub ranges: Vec<(u64, RangeKey)>,
}

impl FollowState {
    /// Ends a caret key's watch, in any state, on a node other than
    /// `node`, the new focus or foreground window, as NVDA's wait gives way
    /// to a focus change.
    fn end_watch_elsewhere(&mut self, node: NodeId, applied: &mut Applied) {
        if self.watch.open().is_some_and(|open| open.node_id != node) {
            applied.answers.extend(self.watch.end("the focus moved"));
        }
    }

    /// Moves the focus-following subscription's newest focus to `target`,
    /// unless it is already there, on the same elements. The focus it
    /// followed before keeps its group on its element, among
    /// [`kept_groups`](Self::kept_groups), until its node is released; a
    /// focus that returns to an element that kept its group is not given
    /// it again.
    fn retarget(&mut self, target: &Target, applied: &mut Applied) {
        if self.followed == target.wanted
            && self.followed_element == target.element
            && self.followed_slider == target.slider
        {
            return;
        }
        let before = std::mem::replace(&mut self.followed, target.wanted);
        let before_element = std::mem::replace(&mut self.followed_element, target.element);
        let mut dropped: Vec<u64> = std::mem::replace(&mut self.followed_slider, target.slider)
            .into_iter()
            .collect();
        if let (Some((node, group, _)), Some(element)) = (before, before_element) {
            let kept = self
                .kept_groups
                .iter_mut()
                .find(|(kept, _, _)| *kept == node);
            if let Some(kept) = kept {
                let (_, _, replaced) = std::mem::replace(kept, (node, group, element));
                dropped.push(replaced);
            } else {
                self.kept_groups.push((node, group, element));
            }
        }
        // The newest focus's element is followed as the newest, not kept.
        if let Some((node, _, _)) = target.wanted {
            dropped.extend(
                self.kept_groups
                    .extract_if(.., |(kept, _, _)| *kept == node)
                    .map(|(_, _, element)| element),
            );
        }
        self.unfollow(dropped, applied);
        applied.subscribe = Some(self.subscription());
    }

    /// Whether `token` names an element the subscription still follows.
    fn follows(&self, token: u64) -> bool {
        self.followed_element == Some(token)
            || self.followed_slider == Some(token)
            || self.kept_groups.iter().any(|&(_, _, kept)| kept == token)
    }

    /// Lets go of `tokens`, each once, but those the subscription still
    /// follows: the shell gives one element one token, whatever it is
    /// followed as.
    fn unfollow(&self, tokens: impl IntoIterator<Item = u64>, applied: &mut Applied) {
        for token in tokens {
            if !self.follows(token) && !applied.unfollowed.contains(&token) {
                applied.unfollowed.push(token);
            }
        }
    }

    /// Where the subscription follows now.
    fn subscription(&self) -> Subscription {
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
            slider: self.followed_slider.filter(|_| newest.is_some()),
            kept,
        }
    }

    /// The store released `released`, by node number: the elements of
    /// those nodes lose their groups, the newest focus's too, with its
    /// slider's, when its node is released, and the subscription is moved
    /// once if anything changed.
    pub fn release_groups(&mut self, released: &BTreeSet<u64>, applied: &mut Applied) {
        let mut dropped: Vec<_> = self
            .kept_groups
            .extract_if(.., |(node, _, _)| released.contains(&node.number()))
            .map(|(_, _, element)| element)
            .collect();
        if self
            .followed
            .is_some_and(|(node, _, _)| released.contains(&node.number()))
        {
            self.followed = None;
            dropped.extend(self.followed_element.take());
            dropped.extend(self.followed_slider.take());
        }
        if !dropped.is_empty() {
            self.unfollow(dropped, applied);
            applied.subscribe = Some(self.subscription());
        }
    }
}

/// Applies the pure part of a commit of `generation`'s, `change`, but for
/// its report and messages, which the caller records and sends first: to
/// the reducer's follow state, reports and marks, by the shared clock
/// (`clock`), or to a program's copies of them, with no clock.
pub fn apply(facts: &mut Facts, generation: u64, change: &Change, clock: Clock) -> Applied {
    let mut applied = Applied::default();
    if !change.text.is_empty() {
        let text = Arc::make_mut(&mut facts.text);
        for text_change in &change.text {
            applied.ranges.extend(text.apply(text_change));
        }
    }
    let follow_state = &mut facts.follow;
    if let Some(node) = change.replaced
        && follow_state
            .watch
            .open()
            .is_some_and(|open| open.node_id == node)
    {
        applied
            .answers
            .extend(follow_state.watch.end("the focus's element was replaced"));
    }
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
            facts.marks = Marks {
                focused: follow.object.clone(),
                terminal: if follow.terminal {
                    TerminalMark::of(follow.window.and_then(Window::new))
                } else {
                    TerminalMark::None
                },
                focus_redirected: follow.redirected,
                absorbed: None,
                system_caret_unused: follow.terminal && follow.console,
            };
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
            follow_state.unfollow(
                target.element.into_iter().chain(target.slider),
                &mut applied,
            );
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
    if let Some(watch) = &change.watch {
        apply_watch(&mut follow_state.watch, watch, clock, &mut applied);
    }
    applied
}

/// Applies `change`, a commit's change of the caret key's watch, to
/// `watch` by `clock` ([`apply`]).
fn apply_watch(watch: &mut Watch, change: &WatchChange, clock: Clock, applied: &mut Applied) {
    match change {
        WatchChange::Open { open, console } => {
            applied.opened = watch.opened(open.clone(), *console, clock);
        }
        WatchChange::Take(node) => applied.taken = watch.take(*node),
        WatchChange::Checked(checked) => {
            if let Some(answer) = watch.checked(checked, clock) {
                applied.resumed = answer.is_none() && matches!(checked, Checked::Watching { .. });
                applied.answers.extend(answer);
            }
        }
    }
}

/// Applies `change`, a program's commit of `generation`, to a program's
/// copies `facts`, as the program's own commit does: its report recorded,
/// then [`apply`], with no clock.
pub fn apply_local(facts: &mut Facts, generation: u64, change: &Change) -> Applied {
    if let Some(report) = &change.report {
        record(&mut facts.held, report);
    }
    apply(facts, generation, change, None)
}

/// Records `report` among the reports changes are read for.
pub fn record(held: &mut Held, report: &Report) {
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
pub fn answer_to_judge(before: Option<&held::Focus>, answered: held::Focus) -> held::Focus {
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
    use verbatim_model::protocol::RequestId;
    use verbatim_model::{CallCounts, CaretWatch, TextUnit};

    use super::*;

    /// A watch for request `request_id` on node 1, waiting.
    fn waiting(request_id: u64) -> Watch {
        Watch::Waiting {
            open: OpenWatch {
                request_id: RequestId::shell(request_id),
                trace: TraceId::from_raw(1),
                node_id: NodeId::new(1),
                watch: CaretWatch {
                    since: None,
                    pressed_at_ms: 0,
                    unit: TextUnit::Character,
                    compare: None,
                    landing: false,
                    deletes: false,
                },
                timing: EventTiming::default(),
                calls: CallCounts::default(),
            },
            bound: 10_000,
        }
    }

    /// A focus, runtime id `[42, 1]`, in window `own` (0 for none), with
    /// ancestors `[42, 2]` and `[42, 3]`, nearest first, in the windows
    /// `windows` gives, its walk ending as `end` says.
    fn focus_read(
        own: isize,
        windows: [isize; 2],
        end: verbatim_model::protocol::AncestorsEnd,
    ) -> verbatim_model::protocol::UiaFocusRead {
        use verbatim_model::protocol::{UiaAncestorFact, UiaFocusRead, UiaSnapshotFact};
        let snapshot = |id| UiaSnapshotFact {
            runtime_id: vec![42, id],
            role: verbatim_model::Role::Group,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        };
        UiaFocusRead {
            hwnd: own,
            snapshot: snapshot(1),
            ancestors: (2..=3)
                .zip(windows)
                .map(|(id, hwnd)| UiaAncestorFact {
                    snapshot: snapshot(id),
                    hwnd,
                    content: true,
                })
                .collect(),
            ancestors_end: end,
            selected_child: None,
            value_from_selection: false,
            window: None,
            same_as_last: None,
        }
    }

    /// A chain is cut at its focus only when the walk up from it did not
    /// reach the top and no element of it has a window of its own; never
    /// at an ancestor.
    #[test]
    fn a_chain_is_cut_at_a_windowless_focus_whose_walk_did_not_reach_the_top() {
        use verbatim_model::protocol::AncestorsEnd;
        let focus = [42, 1];
        let cut = FocusPath::of(&focus_read(0, [0, 0], AncestorsEnd::PlainFrom(70)));
        assert!(cut.is_cut_at(&focus));
        assert!(!cut.is_cut_at(&[42, 2]), "an ancestor is not the focus");
        let failed = FocusPath::of(&focus_read(0, [0, 0], AncestorsEnd::ContinueFrom(70)));
        assert!(failed.is_cut_at(&focus));
        let whole = FocusPath::of(&focus_read(0, [0, 0], AncestorsEnd::Top));
        assert!(!whole.is_cut_at(&focus));
        let windowed_ancestor = FocusPath::of(&focus_read(0, [0, 80], AncestorsEnd::PlainFrom(70)));
        assert!(!windowed_ancestor.is_cut_at(&focus));
        let windowed_focus = FocusPath::of(&focus_read(60, [0, 0], AncestorsEnd::PlainFrom(70)));
        assert!(!windowed_focus.is_cut_at(&focus));
        assert!(!FocusPath::default().is_cut_at(&focus));
    }

    /// A windowless chain whose walk up failed below the focus's nearest
    /// window, which the read carries, is routed from that window down by
    /// every runtime id of the chain; a whole one, with no window of its
    /// own, has no route.
    #[test]
    fn a_cut_windowless_chain_is_routed_from_its_nearest_window() {
        use verbatim_model::protocol::AncestorsEnd;
        let nearest = |end| UiaFocusRead {
            window: Some(70),
            ..focus_read(0, [0, 0], end)
        };
        assert_eq!(
            FocusPath::of(&nearest(AncestorsEnd::ContinueFrom(90))).route_to(&[42, 1]),
            Some(Route {
                window: Window::new(70).expect("70 is not 0"),
                path: vec![vec![42, 3], vec![42, 2], vec![42, 1]],
                runtime_id: vec![42, 1],
            })
        );
        assert_eq!(
            FocusPath::of(&nearest(AncestorsEnd::Top)).route_to(&[42, 1]),
            None
        );
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

    /// A target following node 2, a slider's thumb, on element `element`
    /// with its slider, node 3, on element `slider`.
    fn thumb(element: u64, slider: u64) -> Target {
        Target {
            wanted: Some((NodeId::new(2), Group::Base, Some(NodeId::new(3)))),
            element: Some(element),
            slider: Some(slider),
        }
    }

    #[test]
    fn a_focus_whose_element_was_replaced_is_followed_again() {
        let mut facts = Facts::default();
        facts.follow.retarget(&thumb(5, 6), &mut Applied::default());
        // The same node, group and slider, the thumb's node now a new
        // element: it is followed, and the old one let go of.
        let mut replaced = Applied::default();
        facts.follow.retarget(&thumb(8, 6), &mut replaced);
        assert_eq!(
            (replaced.subscribe, replaced.unfollowed),
            (
                Some(Subscription {
                    newest: Some((8, Group::Base)),
                    slider: Some(6),
                    kept: Vec::new(),
                }),
                vec![5]
            )
        );
    }

    #[test]
    fn a_focus_whose_slider_element_was_replaced_is_followed_again() {
        let mut facts = Facts::default();
        let mut applied = Applied::default();
        facts.follow.retarget(&thumb(5, 6), &mut applied);
        // The same thumb and slider again: nothing moves.
        let mut same = Applied::default();
        facts.follow.retarget(&thumb(5, 6), &mut same);
        assert_eq!((same.subscribe, same.unfollowed), (None, Vec::new()));
        // The same thumb, its slider's node now a new element.
        let mut replaced = Applied::default();
        facts.follow.retarget(&thumb(5, 7), &mut replaced);
        assert_eq!(
            (replaced.subscribe, replaced.unfollowed),
            (
                Some(Subscription {
                    newest: Some((5, Group::Base)),
                    slider: Some(7),
                    kept: Vec::new(),
                }),
                vec![6]
            )
        );
    }

    #[test]
    fn the_followed_focus_released_lets_go_of_its_element_and_slider() {
        let mut facts = Facts::default();
        facts.follow.retarget(&thumb(5, 6), &mut Applied::default());
        let mut applied = Applied::default();
        facts
            .follow
            .release_groups(&BTreeSet::from([2]), &mut applied);
        assert_eq!(
            (
                facts.follow.followed,
                facts.follow.followed_element,
                facts.follow.followed_slider,
                applied.subscribe,
                applied.unfollowed
            ),
            (
                None,
                None,
                None,
                Some(Subscription {
                    newest: None,
                    slider: None,
                    kept: Vec::new(),
                }),
                vec![5, 6]
            )
        );
    }

    #[test]
    fn the_followed_focus_keeps_its_slider_when_other_nodes_are_released() {
        let mut facts = Facts::default();
        facts.follow.retarget(&thumb(5, 6), &mut Applied::default());
        facts
            .follow
            .kept_groups
            .push((NodeId::new(9), Group::Base, 4));
        let mut applied = Applied::default();
        facts
            .follow
            .release_groups(&BTreeSet::from([9]), &mut applied);
        assert_eq!(
            (
                facts.follow.followed_element,
                facts.follow.followed_slider,
                applied.subscribe,
                applied.unfollowed
            ),
            (
                Some(5),
                Some(6),
                Some(Subscription {
                    newest: Some((5, Group::Base)),
                    slider: Some(6),
                    kept: Vec::new(),
                }),
                vec![4]
            )
        );
    }

    #[test]
    fn a_focus_whose_element_was_replaced_ends_the_caret_keys_watch_on_its_node() {
        let mut facts = Facts::default();
        facts.follow.watch = waiting(7);
        // Another node replaced leaves the watch on node 1 alone.
        let elsewhere = Change {
            replaced: Some(NodeId::new(2)),
            ..Change::default()
        };
        let applied = apply(&mut facts, 0, &elsewhere, Some(1));
        assert_eq!(applied.answers, []);
        let replaced = Change {
            replaced: Some(NodeId::new(1)),
            ..Change::default()
        };
        let applied = apply(&mut facts, 0, &replaced, Some(1));
        let ended: Vec<(u64, &str)> = applied
            .answers
            .iter()
            .map(|answer| match answer {
                Answer::Ended { open, why } => (open.request_id.number, *why),
                answer => panic!("an end, not {answer:?}"),
            })
            .collect();
        assert_eq!(
            (ended, facts.follow.watch.is_none()),
            (vec![(7, "the focus's element was replaced")], true)
        );
    }
}
