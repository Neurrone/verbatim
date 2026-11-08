//! What the outpost follows, and what a program's commit changes of it
//! (`books/dev/src/explanation/design/ownership.md`): the follow state, the focus line's paths, the caret key's
//! watch and owed work, all reducer state with no application object. A
//! program is given a copy ([`Facts`]) and applies its own commits to it
//! with the same function the reducer applies them with ([`apply`]), so
//! what it reads after a commit is what the reducer will hold.

use std::collections::BTreeSet;
use std::sync::Arc;
use verbatim_model::protocol::{EventTiming, FrontWindow, OutpostToCore, UiaFocusRead};
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

/// The foreground window's progress bars subscription: one value in the
/// reducer, ended with its generation
/// (`docs/parity/changes-and-selection.md`, "Progress bars that are not
/// the focus").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BarWatch {
    /// No window is watched.
    #[default]
    None,
    /// The element of `window` is being found, by a program started
    /// (`started`) or still to start.
    Finding {
        /// The foreground window.
        window: isize,
        /// Whether a program has been started for it.
        started: bool,
    },
    /// The subtree of `window`'s element is subscribed, by its token.
    Subscribed {
        /// The foreground window.
        window: isize,
        /// Its element's token.
        element: u64,
    },
}

impl BarWatch {
    /// The window watched, while it is subscribed or being found.
    #[must_use]
    pub fn window(self) -> Option<isize> {
        match self {
            BarWatch::None => None,
            BarWatch::Finding { window, .. } | BarWatch::Subscribed { window, .. } => Some(window),
        }
    }

    /// The window whose element a program is to start finding, if one is due.
    pub(crate) fn due(self) -> Option<isize> {
        match self {
            BarWatch::Finding {
                window,
                started: false,
            } => Some(window),
            _ => None,
        }
    }
}

/// The dialog the last foreground report gathered text for, which the
/// focus that follows it reuses: its node and its text.
pub type DialogMemo = Option<(NodeId, Option<String>)>;

/// The last focus this outpost reported and adopted: the one whose caret,
/// text and subscription it tracks. Replaced whole when a focus is adopted,
/// so that no fact of an older focus survives beside a newer one. A cache of
/// reads and of what was reported, which judges nothing Core was told.
#[derive(Clone, Debug)]
pub struct Followed {
    /// The focus's node.
    pub node: NodeId,
    /// The focus's role, as last read, which decides how its text is read.
    pub role: Role,
    /// The object it is, as the intake records the focus.
    pub object: Option<Object>,
    /// The window it was reported in, for events on the focus, which then
    /// need no call to find their window.
    pub window: Option<isize>,
    /// Its ancestors, outermost first, and itself: where the next focus's
    /// ancestor walk can stop and reuse the rest, as NVDA's does.
    pub chain: Vec<NodeSnapshot>,
    /// The window text of each window entry of `chain`, the outpost's
    /// ancestry cache, read when it was reported, which the title check
    /// reads again before a walk reuses it
    /// (`verbatim_reader_rules::ancestry_cache`).
    pub titles: Vec<verbatim_reader_rules::ancestry_cache::SavedTitle>,
    /// How the focus was read.
    pub origin: Origin,
    /// Whether it is a terminal read through UIA, and its window when
    /// known: its caret event is planned after its text changes waiting
    /// with it.
    pub terminal: TerminalMark,
    /// Whether it was reached by redirecting a control's own focus event to
    /// the control's focused child, whose own event is still to come.
    pub redirected: bool,
    /// Whether the system caret's events are dropped as they are pushed:
    /// the focus has caret events of its own.
    pub system_caret_unused: bool,
    /// Where the focus-following subscription follows it, `None` for
    /// nothing.
    pub subscription: Option<Subscribed>,
    /// What the focus still owes.
    pub owed: Option<Owed>,
}

impl Followed {
    /// The raw-view chain the focus listener read for this focus, by which
    /// the outpost finds its own copies of its elements.
    #[must_use]
    pub fn focus_path(&self) -> Option<&FocusPath> {
        match &self.origin {
            Origin::Listener(path) => Some(path),
            Origin::Msaa(_) | Origin::Answer | Origin::MenuOpening => None,
        }
    }

    /// What a live re-read of this focus needs, when it is an MSAA focus.
    #[must_use]
    pub fn msaa_report(&self) -> Option<MsaaReport> {
        match &self.origin {
            Origin::Msaa(report) => Some(*report),
            Origin::Listener(_) | Origin::Answer | Origin::MenuOpening => None,
        }
    }

    /// The marks the mailbox and the intake read of this focus, with the
    /// held terminal's window `absorbed`, which is not a fact of it.
    fn marks(&self, absorbed: Option<isize>) -> Marks {
        Marks {
            focused: self.object.clone(),
            terminal: self.terminal,
            focus_redirected: self.redirected,
            absorbed,
            system_caret_unused: self.system_caret_unused,
        }
    }

    /// The terminal mark of a focus that is a terminal read through UIA
    /// (`terminal`) or not, in `window`.
    #[must_use]
    pub fn terminal_mark(terminal: bool, window: Option<isize>) -> TerminalMark {
        if terminal {
            TerminalMark::of(window.and_then(Window::new))
        } else {
            TerminalMark::None
        }
    }
}

/// How a followed focus was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The listener's read of a UI Automation focus.
    Listener(FocusPath),
    /// An MSAA focus, which a live re-read finds again.
    Msaa(MsaaReport),
    /// A focus-now answer.
    Answer,
    /// An MSAA menu opening, whose chain is at most Core's hidden frame.
    MenuOpening,
}

/// Where the focus-following subscription follows a focus: its group, its
/// element by the shell's token, and the slider followed with it for its
/// value changes, by node and token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subscribed {
    /// The element's group.
    pub group: Group,
    /// The element's token.
    pub element: u64,
    /// The slider followed for its value changes.
    pub slider: Option<(NodeId, u64)>,
}

/// The role and batch of the focus or foreground window this outpost last
/// reported, which the menu rules compare with. Outside [`Followed`]: a
/// foreground report writes them and follows nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastReported {
    /// The role.
    pub role: Role,
    /// The batch.
    pub batch: u64,
}

/// An MSAA focus report of the outpost's, as a live re-read finds it again
/// ([`Origin::Msaa`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MsaaReport {
    /// The report's trace.
    pub trace: TraceId,
    /// Its event's address: window, object id and child id.
    pub event: (isize, i32, i32),
    /// Whether its object was acquired as a simple child.
    pub simple_child: bool,
    /// When its event was observed, on the run's shared clock.
    pub observed_at_us: u64,
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
    /// What is owed.
    pub work: OwedWork,
    /// The trace and observation of the event the focus came from.
    pub trace: TraceId,
    /// The event's observation, in microseconds on the run's shared clock.
    pub observed_at_us: u64,
    /// The worker generation attempting it, once one has started to.
    pub attempting: Option<u64>,
}

impl Owed {
    /// `work`, owed for the event `trace` observed at `observed_at_us`,
    /// which no worker has attempted.
    #[must_use]
    pub fn due(work: OwedWork, trace: TraceId, observed_at_us: u64) -> Self {
        Self {
            work,
            trace,
            observed_at_us,
            attempting: None,
        }
    }
}

/// The owed work a program is given: the followed focus's node and what it
/// owes.
#[derive(Clone, Debug)]
pub struct OwedProgram {
    /// The focus that owes it.
    pub node: NodeId,
    /// What is owed.
    pub owed: Owed,
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

/// Where the focus-following subscription is to be moved for a focus,
/// `None` for nothing: the element and the slider by the shell's tokens, so
/// the reducer holds no element.
pub type Target = Option<Subscribed>;

/// A focus commit's one change of what is followed ([`Change::focus`]).
#[derive(Clone, Debug)]
pub enum FocusCommit {
    /// A focus becoming the followed one, replacing the record whole, with
    /// its report's role and batch and, but for a focus-now answer, the
    /// dialog memo it leaves.
    Adopt {
        /// The record.
        followed: Box<Followed>,
        /// Its report's role and batch.
        reported: LastReported,
        /// The dialog memo the report leaves.
        dialog: Option<DialogMemo>,
    },
    /// A focus-now answer naming the focus already followed: its
    /// subscription and owed work change, and nothing else.
    Again {
        /// Where the subscription is moved.
        target: Target,
        /// What the focus owes now.
        owed: Option<OwedWork>,
        /// The trace of the event it came from.
        trace: TraceId,
        /// The event's observation, in microseconds on the run's shared
        /// clock.
        observed_at_us: u64,
    },
    /// A foreground report: it follows nothing.
    Foreground {
        /// The window's node.
        node: NodeId,
        /// Its role and batch.
        reported: LastReported,
        /// The dialog memo it leaves for the focus that comes after.
        dialog: DialogMemo,
    },
}

/// A commit's one change of the marks on the followed focus
/// ([`Change::mark`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkChange {
    /// The console window's updates are absorbed.
    Absorb(isize),
    /// The system caret's events are dropped until the next focus.
    DropSystemCaret,
    /// The redirect mark is cleared.
    ClearRedirect,
}

/// A commit asking for what the followed focus's state table says cannot
/// occur: a logic error in a program, applied as nothing and passed up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicError {
    /// A focus-now answer followed again with no focus followed: the
    /// program adopts instead.
    AgainWithNothingFollowed,
    /// A change of a mark on the followed focus with no focus followed:
    /// every site tests the focus first.
    MarkWithNothingFollowed(MarkChange),
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
    /// The change of what is followed: a focus adopted, followed again, or
    /// a foreground report.
    pub focus: Option<FocusCommit>,
    /// The focus's element found after its report, with its slider's when
    /// found: the subscription is moved to them only while that node is
    /// the followed focus.
    pub element: Option<(NodeId, Subscribed)>,
    /// The owed work of this node is done, read or failed.
    pub owed_done: Option<NodeId>,
    /// A read of this node's caret began at the instant given, in
    /// microseconds.
    pub caret_read: Option<(NodeId, u64)>,
    /// A change of the caret key's watch.
    pub watch: Option<WatchChange>,
    /// A change of the marks on the followed focus.
    pub mark: Option<MarkChange>,
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
    /// The window the program's check before a call found hung or ghosted,
    /// with no call made into it, when it found one: confirmed live before
    /// it counts ([`Effect::CheckHung`](crate::Effect::CheckHung)).
    pub found: Option<Window>,
    /// The foreground window's element found, by its window and the
    /// shell's token.
    pub bar_found: Option<(isize, u64)>,
}

/// What the outpost follows: changed only by a commit the reducer applies,
/// and copied into each program.
#[derive(Clone, Debug, Default)]
pub struct FollowState {
    /// The focus this outpost follows.
    pub followed: Option<Followed>,
    /// The role and batch of the last report of a focus or foreground
    /// window.
    pub last_reported: Option<LastReported>,
    /// The dialog the last foreground report gathered text for. A focus-now
    /// adoption has none and leaves it.
    pub dialog: DialogMemo,
    /// The elements of the outpost's older UIA reports, each with its node
    /// and the group it was given, by token, which keep their groups until
    /// the store releases their node, so the outpost hears the changes of
    /// every node it has not released (`books/dev/src/explanation/design/ownership.md`).
    pub kept_groups: Vec<(NodeId, Group, u64)>,
    /// The console window whose updates are dropped as they are pushed: a
    /// held terminal's window, which need not be the followed focus's, as
    /// a terminal stays held after the focus moves elsewhere in its console.
    /// A program marks it while the terminal's reading is held with its text
    /// changed and no caret key's watch is open, when it would do nothing
    /// for an update; an adoption clears it, as does anything that may end
    /// the hold before it is handled, so an update observed after the read
    /// that ends the hold begins is never dropped.
    pub absorbed: Option<isize>,
    /// The caret key's watch for evidence.
    pub watch: Watch,
    /// The last committed read of a caret.
    pub caret_read: CaretRead,
    /// The foreground window's progress bars subscription.
    pub bar: BarWatch,
}

/// What a program is given a copy of: everything it reads of what the
/// outpost remembers.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// The reports whose focus node the outpost has not released.
    pub held: Held,
    /// What the outpost follows.
    pub follow: FollowState,
    /// The arbitrator's kept verdicts, a snapshot the reducer replaces on
    /// change ([`super::verdicts`]).
    pub verdicts: Arc<Verdicts>,
    /// Each node's text facts, shared until a change ([`super::text`]).
    pub text: Arc<TextFacts>,
}

impl Facts {
    /// What the intake and the mailbox read of the followed focus, computed
    /// from its record.
    #[must_use]
    pub fn marks(&self) -> Marks {
        let absorbed = self.follow.absorbed;
        self.follow.followed.as_ref().map_or_else(
            || Marks {
                absorbed,
                ..Marks::default()
            },
            |followed| followed.marks(absorbed),
        )
    }
}

/// Where the subscription is to follow, by token: the newest focus's
/// element with its group, the slider's with the value group, every
/// kept element with its own group (`Followed::Subscribe`), and the
/// foreground window's element for its progress bars ([`BarWatch`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    /// The newest focus's element, by token, with its group.
    pub newest: Option<(u64, Group)>,
    /// The slider's element, by token, followed for its value changes.
    pub slider: Option<u64>,
    /// Every kept element, by token, with its own group.
    pub kept: Vec<(u64, Group)>,
    /// The foreground window's element, by token, whose subtree is
    /// followed for its progress bars.
    pub bar: Option<u64>,
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
    /// What the commit asked that its state table says cannot occur.
    pub logic_errors: Vec<LogicError>,
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

    /// The focus-following subscription moved from `previous`, its node and
    /// where it was, to `target` on the record of `node`, unless it is on
    /// the same elements. The focus it followed before keeps its group on
    /// its element, among [`kept_groups`](Self::kept_groups), until its
    /// node is released; a focus that returns to an element that kept its
    /// group is not given it again. The record already holds `target`.
    fn subscription_moved(
        &mut self,
        previous: Option<(NodeId, Subscribed)>,
        (node, target): (NodeId, Target),
        applied: &mut Applied,
    ) {
        if previous == target.map(|subscribed| (node, subscribed)) {
            return;
        }
        let mut dropped: Vec<u64> = Vec::new();
        if let Some((before, subscribed)) = previous {
            dropped.extend(subscribed.slider.map(|(_, slider)| slider));
            let kept = self
                .kept_groups
                .iter_mut()
                .find(|(kept, _, _)| *kept == before);
            if let Some(kept) = kept {
                let (_, _, replaced) =
                    std::mem::replace(kept, (before, subscribed.group, subscribed.element));
                dropped.push(replaced);
            } else {
                self.kept_groups
                    .push((before, subscribed.group, subscribed.element));
            }
        }
        // The newest focus's element is followed as the newest, not kept.
        if target.is_some() {
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
        self.followed
            .as_ref()
            .and_then(|followed| followed.subscription)
            .is_some_and(|subscribed| {
                subscribed.element == token
                    || subscribed.slider.is_some_and(|(_, slider)| slider == token)
            })
            || self.kept_groups.iter().any(|&(_, _, kept)| kept == token)
            || matches!(self.bar, BarWatch::Subscribed { element, .. } if element == token)
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
        let subscribed = self
            .followed
            .as_ref()
            .and_then(|followed| followed.subscription);
        Subscription {
            newest: subscribed.map(|subscribed| (subscribed.element, subscribed.group)),
            slider: subscribed.and_then(|subscribed| subscribed.slider.map(|(_, slider)| slider)),
            kept,
            bar: match self.bar {
                BarWatch::Subscribed { element, .. } => Some(element),
                BarWatch::None | BarWatch::Finding { .. } => None,
            },
        }
    }

    /// Core's word on where attention is, by the table of
    /// `docs/parity/changes-and-selection.md`, "Progress bars that are not
    /// the focus".
    pub fn front(&mut self, front: FrontWindow, applied: &mut Applied) {
        let finding = |window| BarWatch::Finding {
            window,
            started: false,
        };
        match (self.bar, front) {
            (BarWatch::None | BarWatch::Finding { .. }, FrontWindow::Left) => {
                self.bar = BarWatch::None;
            }
            (BarWatch::Finding { window: before, .. }, FrontWindow::In(window))
                if before == window => {}
            (BarWatch::None | BarWatch::Finding { .. }, FrontWindow::In(window)) => {
                self.bar = finding(window);
            }
            (BarWatch::Subscribed { window: before, .. }, FrontWindow::In(window))
                if before == window => {}
            (BarWatch::Subscribed { element, .. }, FrontWindow::In(window)) => {
                self.bar = finding(window);
                self.withdraw_bar(element, applied);
            }
            (BarWatch::Subscribed { element, .. }, FrontWindow::Left) => {
                self.bar = BarWatch::None;
                self.withdraw_bar(element, applied);
            }
        }
    }

    /// The subscription of the bar watch left: its element is let go of
    /// unless the focus subscription follows it, and the subscription moved.
    fn withdraw_bar(&self, element: u64, applied: &mut Applied) {
        self.unfollow(std::iter::once(element), applied);
        applied.subscribe = Some(self.subscription());
    }

    /// The element of `window` found by a program, by `element`'s token.
    fn bar_found(&mut self, (window, element): (isize, u64), applied: &mut Applied) {
        match self.bar {
            BarWatch::Finding {
                window: finding, ..
            } if finding == window => {
                self.bar = BarWatch::Subscribed { window, element };
                applied.subscribe = Some(self.subscription());
            }
            // An earlier lookup's answer, or a window no longer in front.
            BarWatch::None | BarWatch::Finding { .. } | BarWatch::Subscribed { .. } => {
                self.unfollow(std::iter::once(element), applied);
            }
        }
    }

    /// The program finding `window`'s element ended without finding it, or
    /// was abandoned: only a watch still waiting on that program ends, not
    /// one for which a new program is still to start.
    pub fn bar_find_failed(&mut self, window: isize) {
        if self.bar
            == (BarWatch::Finding {
                window,
                started: true,
            })
        {
            self.bar = BarWatch::None;
        }
    }

    /// The store released `released`, by node number: the elements of
    /// those nodes lose their groups, the newest focus's too, with its
    /// slider's, when its node is released, and the subscription is moved
    /// once if anything changed. The rest of the record stays for the
    /// walks.
    pub fn release_groups(&mut self, released: &BTreeSet<u64>, applied: &mut Applied) {
        let mut dropped: Vec<_> = self
            .kept_groups
            .extract_if(.., |(node, _, _)| released.contains(&node.number()))
            .map(|(_, _, element)| element)
            .collect();
        if let Some(subscribed) = self
            .followed
            .as_mut()
            .filter(|followed| released.contains(&followed.node.number()))
            .and_then(|followed| followed.subscription.take())
        {
            dropped.push(subscribed.element);
            dropped.extend(subscribed.slider.map(|(_, slider)| slider));
        }
        if !dropped.is_empty() {
            self.unfollow(dropped, applied);
            applied.subscribe = Some(self.subscription());
        }
    }

    /// Applies a commit's change of what is followed.
    fn apply_focus(&mut self, focus: &FocusCommit, applied: &mut Applied) {
        match focus {
            FocusCommit::Adopt {
                followed,
                reported,
                dialog,
            } => {
                let followed = followed.as_ref().clone();
                let node = followed.node;
                let target = followed.subscription;
                self.end_watch_elsewhere(node, applied);
                let previous = self
                    .followed
                    .replace(followed)
                    .and_then(|before| Some((before.node, before.subscription?)));
                self.subscription_moved(previous, (node, target), applied);
                self.absorbed = None;
                self.last_reported = Some(*reported);
                if let Some(dialog) = dialog {
                    self.dialog.clone_from(dialog);
                }
            }
            FocusCommit::Again {
                target,
                owed,
                trace,
                observed_at_us,
            } => {
                let Some(followed) = self.followed.as_mut() else {
                    applied
                        .logic_errors
                        .push(LogicError::AgainWithNothingFollowed);
                    return;
                };
                let node = followed.node;
                let previous = followed.subscription.map(|before| (node, before));
                followed.subscription = *target;
                followed.owed = owed
                    .clone()
                    .map(|work| Owed::due(work, *trace, *observed_at_us));
                self.subscription_moved(previous, (node, *target), applied);
            }
            FocusCommit::Foreground {
                node,
                reported,
                dialog,
            } => {
                self.end_watch_elsewhere(*node, applied);
                self.last_reported = Some(*reported);
                self.dialog.clone_from(dialog);
            }
        }
    }

    /// The focus's element found after its report: the subscription is
    /// moved to it only while `node` is followed, and its tokens are let go
    /// otherwise.
    fn element_found(&mut self, node: NodeId, target: Subscribed, applied: &mut Applied) {
        let Some(followed) = self
            .followed
            .as_mut()
            .filter(|followed| followed.node == node)
        else {
            self.unfollow(
                std::iter::once(target.element).chain(target.slider.map(|(_, slider)| slider)),
                applied,
            );
            return;
        };
        let previous = followed.subscription.map(|before| (node, before));
        followed.subscription = Some(target);
        self.subscription_moved(previous, (node, Some(target)), applied);
    }

    /// Applies a commit's change of the marks on the followed focus.
    fn apply_mark(&mut self, mark: MarkChange, applied: &mut Applied) {
        let set: fn(&mut Followed) = match mark {
            MarkChange::Absorb(window) => {
                self.absorbed = Some(window);
                return;
            }
            MarkChange::DropSystemCaret => |followed| followed.system_caret_unused = true,
            MarkChange::ClearRedirect => |followed| followed.redirected = false,
        };
        match self.followed.as_mut() {
            Some(followed) => set(followed),
            None => applied
                .logic_errors
                .push(LogicError::MarkWithNothingFollowed(mark)),
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
    if let Some(focus) = &change.focus {
        follow_state.apply_focus(focus, &mut applied);
    }
    if let Some((node, target)) = change.element {
        follow_state.element_found(node, target, &mut applied);
    }
    if let Some(node) = change.owed_done
        && let Some(followed) = follow_state
            .followed
            .as_mut()
            .filter(|followed| followed.node == node)
        && followed
            .owed
            .as_ref()
            .is_some_and(|owed| owed.attempting == Some(generation))
    {
        followed.owed = None;
    }
    if let Some((node, started_us)) = change.caret_read {
        follow_state.caret_read.note(node.number(), started_us);
    }
    if let Some(found) = change.bar_found {
        follow_state.bar_found(found, &mut applied);
    }
    if let Some(mark) = change.mark {
        follow_state.apply_mark(mark, &mut applied);
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

/// A record of `node`, of `role`, with nothing but its node and role, as
/// the tests start from.
#[cfg(test)]
pub(crate) fn plain_followed(node: NodeId, role: Role) -> Followed {
    Followed {
        node,
        role,
        object: None,
        window: None,
        chain: Vec::new(),
        titles: Vec::new(),
        origin: Origin::Answer,
        terminal: TerminalMark::None,
        redirected: false,
        system_caret_unused: false,
        subscription: None,
        owed: None,
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
            answers: None,
            cached_from: None,
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

    /// A record of node 2, a slider's thumb, followed with `subscription`.
    fn thumb_focused(subscription: Subscribed) -> Facts {
        let mut facts = Facts::default();
        facts.follow.followed = Some(Followed {
            subscription: Some(subscription),
            ..plain_followed(NodeId::new(2), Role::Button)
        });
        facts
    }

    /// Where a thumb is followed, on element `element` with its slider,
    /// node 3, on element `slider`.
    fn thumb(element: u64, slider: u64) -> Subscribed {
        Subscribed {
            group: Group::Base,
            element,
            slider: Some((NodeId::new(3), slider)),
        }
    }

    /// The thumb followed again at `target`.
    fn again(facts: &mut Facts, target: Subscribed) -> Applied {
        let change = Change {
            focus: Some(FocusCommit::Again {
                target: Some(target),
                owed: None,
                trace: TraceId::from_raw(1),
                observed_at_us: 0,
            }),
            ..Change::default()
        };
        apply(facts, 0, &change, None)
    }

    #[test]
    fn a_focus_whose_element_was_replaced_is_followed_again() {
        let mut facts = thumb_focused(thumb(5, 6));
        // The same node, group and slider, the thumb's node now a new
        // element: it is followed, and the old one let go of.
        let replaced = again(&mut facts, thumb(8, 6));
        assert_eq!(
            (replaced.subscribe, replaced.unfollowed),
            (
                Some(Subscription {
                    newest: Some((8, Group::Base)),
                    slider: Some(6),
                    kept: Vec::new(),
                    bar: None,
                }),
                vec![5]
            )
        );
    }

    #[test]
    fn a_focus_whose_slider_element_was_replaced_is_followed_again() {
        let mut facts = thumb_focused(thumb(5, 6));
        // The same thumb and slider again: nothing moves.
        let same = again(&mut facts, thumb(5, 6));
        assert_eq!((same.subscribe, same.unfollowed), (None, Vec::new()));
        // The same thumb, its slider's node now a new element.
        let replaced = again(&mut facts, thumb(5, 7));
        assert_eq!(
            (replaced.subscribe, replaced.unfollowed),
            (
                Some(Subscription {
                    newest: Some((5, Group::Base)),
                    slider: Some(7),
                    kept: Vec::new(),
                    bar: None,
                }),
                vec![6]
            )
        );
    }

    #[test]
    fn the_followed_focus_released_lets_go_of_its_element_and_slider() {
        let mut facts = thumb_focused(thumb(5, 6));
        let mut applied = Applied::default();
        facts
            .follow
            .release_groups(&BTreeSet::from([2]), &mut applied);
        assert_eq!(
            (
                facts.follow.followed.expect("still followed").subscription,
                applied.subscribe,
                applied.unfollowed
            ),
            (
                None,
                Some(Subscription {
                    newest: None,
                    slider: None,
                    kept: Vec::new(),
                    bar: None,
                }),
                vec![5, 6]
            )
        );
    }

    #[test]
    fn the_followed_focus_keeps_its_slider_when_other_nodes_are_released() {
        let mut facts = thumb_focused(thumb(5, 6));
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
                facts.follow.followed.expect("still followed").subscription,
                applied.subscribe,
                applied.unfollowed
            ),
            (
                Some(thumb(5, 6)),
                Some(Subscription {
                    newest: Some((5, Group::Base)),
                    slider: Some(6),
                    kept: Vec::new(),
                    bar: None,
                }),
                vec![4]
            )
        );
    }

    /// The commits the state table says cannot occur are returned as logic
    /// errors and change nothing.
    #[test]
    fn a_commit_that_cannot_occur_is_returned_as_a_logic_error() {
        let mut facts = Facts::default();
        let change = Change {
            focus: Some(FocusCommit::Again {
                target: None,
                owed: None,
                trace: TraceId::from_raw(1),
                observed_at_us: 0,
            }),
            mark: Some(MarkChange::DropSystemCaret),
            ..Change::default()
        };
        let applied = apply(&mut facts, 0, &change, None);
        assert_eq!(
            applied.logic_errors,
            vec![
                LogicError::AgainWithNothingFollowed,
                LogicError::MarkWithNothingFollowed(MarkChange::DropSystemCaret)
            ]
        );
        assert_eq!(facts.marks(), Marks::default());
    }

    /// A focus followed on element `element`, to share its token with the
    /// bar watch.
    fn focused_on(element: u64) -> FollowState {
        FollowState {
            followed: Some(Followed {
                subscription: Some(Subscribed {
                    group: Group::Base,
                    element,
                    slider: None,
                }),
                ..plain_followed(NodeId::new(2), Role::Button)
            }),
            ..FollowState::default()
        }
    }

    const FINDING: BarWatch = BarWatch::Finding {
        window: 10,
        started: true,
    };
    const SUBSCRIBED: BarWatch = BarWatch::Subscribed {
        window: 10,
        element: 7,
    };

    /// Whether the subscription moved, and where it then follows the
    /// window.
    #[derive(Debug, PartialEq, Eq)]
    enum Moved {
        Not,
        To(Option<u64>),
    }

    /// `bar` as the state, an input applied, and what is left: the state,
    /// where the subscription follows the window, and the tokens let go of.
    fn after(
        bar: BarWatch,
        input: impl FnOnce(&mut FollowState, &mut Applied),
    ) -> (BarWatch, Moved, Vec<u64>) {
        let mut follow = FollowState {
            bar,
            ..FollowState::default()
        };
        let mut applied = Applied::default();
        input(&mut follow, &mut applied);
        (
            follow.bar,
            applied
                .subscribe
                .map_or(Moved::Not, |subscribe| Moved::To(subscribe.bar)),
            applied.unfollowed,
        )
    }

    fn front(front: FrontWindow) -> impl FnOnce(&mut FollowState, &mut Applied) {
        move |follow, applied| follow.front(front, applied)
    }

    fn found(window: isize, token: u64) -> impl FnOnce(&mut FollowState, &mut Applied) {
        move |follow, applied| follow.bar_found((window, token), applied)
    }

    fn failed(window: isize) -> impl FnOnce(&mut FollowState, &mut Applied) {
        move |follow, _| follow.bar_find_failed(window)
    }

    #[test]
    fn a_bar_watch_none_finds_an_element_for_a_window_and_ignores_the_rest() {
        assert_eq!(
            after(BarWatch::None, front(FrontWindow::In(10))),
            (
                BarWatch::Finding {
                    window: 10,
                    started: false
                },
                Moved::Not,
                vec![]
            )
        );
        assert_eq!(
            after(BarWatch::None, front(FrontWindow::Left)),
            (BarWatch::None, Moved::Not, vec![])
        );
        assert_eq!(
            after(BarWatch::None, found(10, 7)),
            (BarWatch::None, Moved::Not, vec![7])
        );
        assert_eq!(
            after(BarWatch::None, failed(10)),
            (BarWatch::None, Moved::Not, vec![])
        );
    }

    #[test]
    fn a_bar_watch_finding_takes_only_its_own_windows_answer() {
        assert_eq!(
            after(FINDING, found(10, 7)),
            (SUBSCRIBED, Moved::To(Some(7)), vec![])
        );
        assert_eq!(after(FINDING, found(11, 8)), (FINDING, Moved::Not, vec![8]));
        assert_eq!(
            after(FINDING, failed(10)),
            (BarWatch::None, Moved::Not, vec![])
        );
        assert_eq!(after(FINDING, failed(11)), (FINDING, Moved::Not, vec![]));
        // A lookup asked for again and not yet started waits for its own
        // program's answer, not an earlier one's.
        let waiting = BarWatch::Finding {
            window: 10,
            started: false,
        };
        assert_eq!(after(waiting, failed(10)), (waiting, Moved::Not, vec![]));
    }

    #[test]
    fn a_bar_watch_finding_moves_with_the_window_and_ends_when_left() {
        assert_eq!(
            after(FINDING, front(FrontWindow::In(10))),
            (FINDING, Moved::Not, vec![])
        );
        assert_eq!(
            after(FINDING, front(FrontWindow::In(11))),
            (
                BarWatch::Finding {
                    window: 11,
                    started: false
                },
                Moved::Not,
                vec![]
            )
        );
        assert_eq!(
            after(FINDING, front(FrontWindow::Left)),
            (BarWatch::None, Moved::Not, vec![])
        );
    }

    #[test]
    fn a_bar_watch_subscribed_withdraws_when_the_window_changes_or_is_left() {
        assert_eq!(
            after(SUBSCRIBED, front(FrontWindow::In(10))),
            (SUBSCRIBED, Moved::Not, vec![])
        );
        assert_eq!(
            after(SUBSCRIBED, front(FrontWindow::In(11))),
            (
                BarWatch::Finding {
                    window: 11,
                    started: false
                },
                Moved::To(None),
                vec![7]
            )
        );
        assert_eq!(
            after(SUBSCRIBED, front(FrontWindow::Left)),
            (BarWatch::None, Moved::To(None), vec![7])
        );
        assert_eq!(
            after(SUBSCRIBED, found(10, 8)),
            (SUBSCRIBED, Moved::Not, vec![8])
        );
        assert_eq!(
            after(SUBSCRIBED, failed(10)),
            (SUBSCRIBED, Moved::Not, vec![])
        );
    }

    /// Letting go of the bar watch's element follows the same rule as
    /// letting go of the focus's: one token, kept while either follows it.
    #[test]
    fn a_token_shared_with_the_focus_is_kept_when_the_bar_watch_lets_it_go() {
        let mut follow = FollowState {
            bar: SUBSCRIBED,
            ..focused_on(7)
        };
        let mut applied = Applied::default();
        follow.front(FrontWindow::Left, &mut applied);
        assert_eq!(applied.unfollowed, Vec::<u64>::new());
        // A found element the focus already holds is kept likewise.
        let mut follow = focused_on(9);
        let mut applied = Applied::default();
        follow.bar_found((10, 9), &mut applied);
        assert_eq!(applied.unfollowed, Vec::<u64>::new());
        assert_eq!(follow.bar, BarWatch::None);
    }

    /// The token the bar watch holds is kept when the focus lets it go.
    #[test]
    fn a_token_shared_with_the_bar_watch_is_kept_when_the_focus_lets_it_go() {
        let mut follow = FollowState {
            bar: SUBSCRIBED,
            ..focused_on(7)
        };
        let mut applied = Applied::default();
        follow.release_groups(&BTreeSet::from([2]), &mut applied);
        assert_eq!(applied.unfollowed, Vec::<u64>::new());
        assert_eq!(
            applied.subscribe.expect("the subscription moved").bar,
            Some(7)
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
