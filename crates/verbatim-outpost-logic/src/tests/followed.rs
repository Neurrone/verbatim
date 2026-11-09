//! The generated test of the outpost's followed focus: lists of actions, each
//! made valid when it is applied, given to the reducer through its entries,
//! its intake, its release and its programs' commits, and compared after
//! every step with a much simpler reference model.
//!
//! The model is written from `docs/parity/focus-events.md`,
//! `docs/parity/terminals.md`, ADR 0001 ("Focus-following subscriptions") and
//! Dickson's approved followed-focus rules (`docs/design/state-models.md`,
//! "The outpost's followed focus": its table, rules and limits), never from
//! the code's fields: a list of the focuses the outpost adopted in the order
//! it read them, the elements of every report not yet released, the work
//! owed, the caret key's watch and the held console. It checks what the
//! record exists to decide: a later event of the newest focus is planned and
//! read as that focus's and an event of an older focus is not; every
//! unreleased element is followed exactly once with its group; each token is
//! let go exactly once; owed work is done or dropped once; a held console's
//! updates are dropped only while the terminal is held and its first read is
//! not under way; a caret key's watch ends when the focus moves elsewhere.
//!
//! The world makes the real shapes: focus events and foreground changes in
//! the orders they arrive, a UIA focus's marker and read, a program whose
//! element is found after a newer focus was adopted, focus-now answers, a
//! release, owed work attempted, done or abandoned, an abandoned worker's
//! commit, and held consoles.
//!
//! Its limits. Programs live in `crates/verbatim-outpost/src/outpost/worker.rs`,
//! outside this crate, so the commit sites and the worker's choice between
//! adopting and only following again cannot be reached here: the scripted
//! programs build their commits at one seam, the module [`seam`], and a
//! hand-written test at the worker's level pins that choice. The caret key's
//! watch is opened waiting only, and no deadline jump reaches its bound: the
//! watch's own poll and bound are the examples of `watch.rs` and of
//! `tests/generated.rs`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestError, TestRunner};
use verbatim_core::focus_rules::HeldNode;
use verbatim_model::protocol::{AncestorsEnd, UiaAncestorFact, UiaFocusRead, UiaSnapshotFact};
use verbatim_model::{
    CaretMotion, KeyAnswer, KeyRequest, KeyStart, KeyTag, QueryId, TextOp, WatchKind,
};

use super::*;
use crate::intake::OBJID_CLIENT;

/// The adapter between this test's vocabulary and the reducer: the only code
/// here that names how a commit is built or what a program reads of its
/// copies, so that changing the follow state changes this module alone.
mod seam {
    use super::*;
    use crate::{
        FocusCommit, Followed as Record, LastReported, MarkChange, Origin as RecordOrigin,
        Subscribed,
    };

    /// How a focus came to be read.
    #[derive(Clone, Debug)]
    pub(super) enum Origin {
        /// The focus listener's read of a UI Automation focus.
        Listener(Box<UiaFocusRead>),
        /// An MSAA focus, at this event address.
        Msaa((isize, i32, i32)),
        /// A focus-now answer.
        Answer,
    }

    /// A focus becoming the followed one.
    #[derive(Clone, Debug)]
    #[expect(
        clippy::struct_excessive_bools,
        reason = "facts about one focus, each read before the commit"
    )]
    pub(super) struct Adoption {
        pub(super) node: NodeId,
        pub(super) role: Role,
        pub(super) object: Option<Object>,
        pub(super) window: isize,
        pub(super) chain: Vec<NodeId>,
        pub(super) batch: u64,
        pub(super) redirected: bool,
        pub(super) terminal: bool,
        pub(super) console: bool,
        pub(super) element: Option<(Group, u64)>,
        pub(super) slider: Option<(NodeId, u64)>,
        pub(super) owed: bool,
        pub(super) origin: Origin,
        pub(super) dialog: Option<DialogMemo>,
        pub(super) replaced: bool,
        pub(super) answer: Option<(RequestId, TraceId)>,
    }

    /// What one commit of a scripted program asks.
    #[derive(Clone, Debug)]
    pub(super) enum Commit {
        /// A focus adopted.
        Adopt(Box<Adoption>),
        /// A focus-now answer naming the focus already followed: only
        /// followed again.
        Again {
            node: NodeId,
            role: Role,
            element: Option<(Group, u64)>,
            slider: Option<(NodeId, u64)>,
            owed: bool,
            answer: (RequestId, TraceId),
        },
        /// A foreground report.
        Foreground {
            node: NodeId,
            role: Role,
            batch: u64,
            dialog: DialogMemo,
        },
        /// The element of `node` found after its report.
        Element {
            node: NodeId,
            group: Group,
            element: u64,
            slider: Option<(NodeId, u64)>,
        },
        /// The followed focus's owed work is done.
        OwedDone(NodeId),
        /// The console window's updates are absorbed.
        Absorb(isize),
        /// The system caret's events are dropped until the next focus.
        DropSystemCaret,
        /// The redirect mark is cleared.
        ClearRedirect,
        /// A caret key's first check found nothing: its watch opens, waiting,
        /// on `node`.
        Open {
            request: (RequestId, TraceId),
            node: NodeId,
        },
        /// A query answered, with nothing else committed.
        Answer((RequestId, TraceId)),
    }

    /// What a program reads of its copy of the reducer's facts.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) struct Reads {
        pub(super) focused: Option<Object>,
        pub(super) node: Option<NodeId>,
        pub(super) role: Option<Role>,
        pub(super) window: Option<isize>,
        pub(super) chain: Vec<NodeId>,
        pub(super) redirected: bool,
        pub(super) msaa: Option<(isize, i32, i32)>,
        pub(super) reported_role: Option<Role>,
        pub(super) reported_batch: Option<u64>,
        pub(super) dialog: DialogMemo,
    }

    /// What a program reads of `facts`.
    pub(super) fn reads(facts: &Facts) -> Reads {
        let followed = facts.follow.followed.as_ref();
        let marks = facts.marks();
        Reads {
            focused: marks.focused,
            node: followed.map(|focus| focus.node),
            role: followed.map(|focus| focus.role),
            window: followed.and_then(|focus| focus.window),
            chain: followed.map_or_else(Vec::new, |focus| {
                focus.chain.iter().map(|node| node.id).collect()
            }),
            redirected: marks.focus_redirected,
            msaa: followed
                .and_then(Record::msaa_report)
                .map(|report| report.event),
            reported_role: facts.follow.last_reported.map(|last| last.role),
            reported_batch: facts.follow.last_reported.map(|last| last.batch),
            dialog: facts.follow.dialog.clone(),
        }
    }

    /// Whether a program finds its own copy of the element `runtime_id` by
    /// the focus's chain.
    pub(super) fn route_found(facts: &Facts, runtime_id: &[i32]) -> bool {
        facts
            .follow
            .followed
            .as_ref()
            .and_then(Record::focus_path)
            .is_some_and(|path| path.route_to(runtime_id).is_some())
    }

    /// Applies `change`, a commit of `generation`'s, to a program's copy.
    pub(super) fn apply_local(facts: &mut Facts, generation: u64, change: &Change) {
        let applied = super::apply_local(facts, generation, change);
        assert_eq!(applied.logic_errors, [], "a commit that cannot occur");
    }

    /// Whether the mailbox's copy `marks` drops `item` on its arrival.
    pub(super) fn drops(marks: &Marks, item: &Item) -> bool {
        marks.drops_on_arrival(item)
    }

    /// The reply to the query `(request_id, trace)`.
    fn answered((request_id, trace): (RequestId, TraceId)) -> OutpostToCore {
        reply(
            request_id,
            trace,
            QueryOutcome::Done(QueryResult::Text(TextReply::Done)),
        )
    }

    /// The report recorded for Core's judgement for a focus.
    fn held_focus(node: NodeId, role: Role) -> Focus {
        Focus {
            node: HeldNode {
                id: node,
                backend: Backend::Msaa,
                role,
                name: None,
                states: StateSet::new(),
                window: None,
                above_boundary: false,
            },
            ancestors: Vec::new(),
            top_level: None,
            in_front: InFront::Read(None),
            entered_in_front: None,
            event: None,
            simple_child: false,
            dialog_texts: Vec::new(),
            value_from_selection: false,
        }
    }

    /// Where the subscription follows, by token.
    fn target(element: Option<(Group, u64)>, slider: Option<(NodeId, u64)>) -> Target {
        element.map(|(group, element)| Subscribed {
            group,
            element,
            slider,
        })
    }

    /// What the focus owes, when it does.
    fn owed_of(owed: bool) -> Option<Owed> {
        owed.then(|| {
            Owed::due(
                OwedWork::FirstRead(reported_caret()),
                TraceId::from_raw(1),
                0,
            )
        })
    }

    /// The commit `commit` as the reducer takes it.
    #[expect(
        clippy::too_many_lines,
        reason = "one match, each arm a commit's whole mapping to the reducer's shape"
    )]
    pub(super) fn to_change(commit: &Commit) -> Change {
        match commit {
            Commit::Adopt(a) => {
                let origin = match &a.origin {
                    Origin::Listener(read) => RecordOrigin::Listener(FocusPath::of(read)),
                    Origin::Msaa(event) => RecordOrigin::Msaa(MsaaReport {
                        trace: TraceId::from_raw(1),
                        event: *event,
                        simple_child: false,
                        observed_at_us: 0,
                    }),
                    Origin::Answer => RecordOrigin::Answer,
                };
                let followed = Record {
                    node: a.node,
                    role: a.role,
                    object: a.object.clone(),
                    window: Some(a.window),
                    chain: a.chain.iter().map(|id| node(id.number(), a.role)).collect(),
                    titles: Vec::new(),
                    origin,
                    terminal: Record::terminal_mark(a.terminal, Some(a.window)),
                    redirected: a.redirected,
                    system_caret_unused: a.terminal && a.console,
                    subscription: target(a.element, a.slider),
                    owed: owed_of(a.owed),
                };
                let report = match a.answer {
                    Some(_) => Report::Answered(held_focus(a.node, a.role)),
                    None => Report::Reported(held_focus(a.node, a.role), Evidence::Msaa),
                };
                Change {
                    result: a.answer.map(answered),
                    report: Some(report),
                    focus: Some(FocusCommit::Adopt {
                        followed: Box::new(followed),
                        reported: LastReported {
                            role: a.role,
                            batch: a.batch,
                        },
                        dialog: a.dialog.clone(),
                    }),
                    replaced: a.replaced.then_some(a.node),
                    ..Change::default()
                }
            }
            Commit::Again {
                node,
                role,
                element,
                slider,
                owed,
                answer,
            } => Change {
                result: Some(answered(*answer)),
                report: Some(Report::Answered(held_focus(*node, *role))),
                focus: Some(FocusCommit::Again {
                    target: target(*element, *slider),
                    owed: owed.then(|| OwedWork::FirstRead(reported_caret())),
                    trace: TraceId::from_raw(1),
                    observed_at_us: 0,
                }),
                ..Change::default()
            },
            Commit::Foreground {
                node,
                role,
                batch,
                dialog,
            } => Change {
                report: Some(Report::Reported(held_focus(*node, *role), Evidence::Msaa)),
                focus: Some(FocusCommit::Foreground {
                    node: *node,
                    reported: LastReported {
                        role: *role,
                        batch: *batch,
                    },
                    dialog: dialog.clone(),
                }),
                ..Change::default()
            },
            Commit::Element {
                node,
                group,
                element,
                slider,
            } => Change {
                element: Some((
                    *node,
                    Subscribed {
                        group: *group,
                        element: *element,
                        slider: *slider,
                    },
                )),
                ..Change::default()
            },
            Commit::OwedDone(node) => Change {
                owed_done: Some(*node),
                ..Change::default()
            },
            Commit::Absorb(window) => Change {
                mark: Some(MarkChange::Absorb(*window)),
                ..Change::default()
            },
            Commit::DropSystemCaret => Change {
                mark: Some(MarkChange::DropSystemCaret),
                ..Change::default()
            },
            Commit::ClearRedirect => Change {
                mark: Some(MarkChange::ClearRedirect),
                ..Change::default()
            },
            Commit::Open { request, node } => Change {
                watch: Some(WatchChange::Open {
                    key: key_watch(request, *node),
                    console: false,
                }),
                ..Change::default()
            },
            Commit::Answer(query) => Change {
                result: Some(answered(*query)),
                ..Change::default()
            },
        }
    }
}

use seam::{Adoption, Commit, Origin, Reads};

// ----- The reference model -----

/// A focus the outpost adopted.
#[derive(Clone, Debug)]
struct Adopted {
    node: NodeId,
    role: Role,
    object: Object,
    window: isize,
    chain: Vec<NodeId>,
    terminal: bool,
    redirected: bool,
    /// The runtime ids of its chain, when the focus listener read it: what a
    /// route can be found to.
    route: Vec<Vec<i32>>,
    msaa: Option<(isize, i32, i32)>,
    /// The slider followed with it for its value changes.
    slider: Option<u64>,
}

/// The work the followed focus owes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OwedModel {
    /// Owed, to be attempted as the next program.
    Due,
    /// A worker attempted it; it is not attempted again.
    Attempted,
}

/// What the outpost's followed focus should be doing, by the parity documents
/// and the recorded decisions.
#[derive(Clone, Debug, Default)]
struct Model {
    /// Every adopted focus, in the order the outpost read them, newest last.
    focuses: Vec<Adopted>,
    /// The element and group of every report not yet released, by node.
    elements: BTreeMap<u64, (u64, Group)>,
    /// The role and batch of the last report of a focus or foreground window.
    reported: Option<(Role, u64)>,
    /// The dialog memo the last report left.
    dialog: DialogMemo,
    owed: Option<OwedModel>,
    /// The caret key's watch: its node and its query.
    watch: Option<(NodeId, RequestId)>,
    /// The newest key's request: only its program opens a watch.
    reading: Option<RequestId>,
    /// The queries whose watch ended in this step.
    ended_now: Vec<RequestId>,
    /// The console window whose updates are dropped on arrival.
    absorbed: Option<isize>,
    /// Whether the system caret's events are dropped on arrival.
    system_caret_unused: bool,
    released: BTreeSet<u64>,
    /// Tokens offered this step that the model does not follow.
    offered: BTreeSet<u64>,
}

/// The subscription, as the model and the world compare it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Followed {
    newest: Option<(u64, Group)>,
    slider: Option<u64>,
    kept: BTreeMap<u64, Group>,
}

impl Followed {
    /// Every token followed, by any route.
    fn tokens(&self) -> BTreeSet<u64> {
        self.newest
            .iter()
            .map(|&(token, _)| token)
            .chain(self.slider)
            .chain(self.kept.keys().copied())
            .collect()
    }
}

impl Model {
    fn newest(&self) -> Option<&Adopted> {
        self.focuses.last()
    }

    fn is_released(&self, node: NodeId) -> bool {
        self.released.contains(&node.number())
    }

    /// Where the subscription should be: the newest focus's element with its
    /// group and the slider of its value changes, and the element of every
    /// other report not released, by its group.
    fn followed(&self) -> Followed {
        let newest_node = self
            .newest()
            .map(|focus| focus.node.number())
            .filter(|node| !self.released.contains(node));
        let newest = newest_node.and_then(|node| {
            self.elements
                .get(&node)
                .map(|&(token, group)| (token, group))
        });
        let slider = newest.and(self.newest().and_then(|focus| focus.slider));
        let kept = self
            .elements
            .iter()
            .filter(|(node, _)| Some(**node) != newest_node)
            .map(|(_, &(token, group))| (token, group))
            .collect();
        Followed {
            newest,
            slider,
            kept,
        }
    }

    /// What a program reads of the focus.
    fn reads(&self) -> Reads {
        let focus = self.newest();
        Reads {
            focused: focus.map(|focus| focus.object.clone()),
            node: focus.map(|focus| focus.node),
            role: focus.map(|focus| focus.role),
            window: focus.map(|focus| focus.window),
            chain: focus.map(|focus| focus.chain.clone()).unwrap_or_default(),
            redirected: focus.is_some_and(|focus| focus.redirected),
            msaa: focus.and_then(|focus| focus.msaa),
            reported_role: self.reported.map(|(role, _)| role),
            reported_batch: self.reported.map(|(_, batch)| batch),
            dialog: self.dialog.clone(),
        }
    }

    /// Whether a program finds the element `id` by the newest focus's chain:
    /// a focus the listener read has its chain's elements, and adopting
    /// anything else drops the older focus's route.
    fn route(&self, id: &[i32]) -> bool {
        self.newest()
            .is_some_and(|focus| focus.route.iter().any(|known| known == id))
    }

    /// Whether `object` is the focus the outpost read last.
    fn is_newest(&self, object: &Object) -> bool {
        self.newest().is_some_and(|focus| focus.object == *object)
    }

    /// Ends the watch when its node is not `node` (or `node` is `None`).
    fn end_watch_elsewhere(&mut self, node: Option<NodeId>) {
        if let Some((watch_node, request)) = self.watch
            && Some(watch_node) != node
        {
            self.watch = None;
            self.ended_now.push(request);
        }
    }

    /// A program's commit opens the watch of `request`, the newest key's;
    /// an older key's, which a newer request replaced, ends.
    fn open_watch(&mut self, node: NodeId, request: RequestId) {
        if self.reading == Some(request) {
            self.reading = None;
            self.watch = Some((node, request));
        } else {
            self.ended_now.push(request);
        }
    }

    /// Ends the watch.
    fn end_watch(&mut self) {
        if let Some((_, request)) = self.watch.take() {
            self.ended_now.push(request);
        }
    }

    /// Applies an accepted commit.
    fn commit(&mut self, commit: &Commit) {
        match commit {
            Commit::Adopt(a) => {
                let route = match &a.origin {
                    Origin::Listener(read) => std::iter::once(read.snapshot.runtime_id.clone())
                        .chain(
                            read.ancestors
                                .iter()
                                .map(|ancestor| ancestor.snapshot.runtime_id.clone()),
                        )
                        .collect(),
                    _ => Vec::new(),
                };
                let msaa = match a.origin {
                    Origin::Msaa(event) => Some(event),
                    _ => None,
                };
                self.focuses.push(Adopted {
                    node: a.node,
                    role: a.role,
                    object: a.object.clone().expect("every adopted focus has an object"),
                    window: a.window,
                    chain: a.chain.clone(),
                    terminal: a.terminal,
                    redirected: a.redirected,
                    route,
                    msaa,
                    slider: a.slider.map(|(_, token)| token),
                });
                if let Some((group, token)) = a.element {
                    self.elements.insert(a.node.number(), (token, group));
                }
                self.owed = a.owed.then_some(OwedModel::Due);
                self.reported = Some((a.role, a.batch));
                if let Some(dialog) = &a.dialog {
                    self.dialog.clone_from(dialog);
                }
                self.end_watch_elsewhere(Some(a.node));
                if a.replaced {
                    self.end_watch_elsewhere(None);
                }
                self.absorbed = None;
                self.system_caret_unused = a.terminal && a.console;
            }
            Commit::Again {
                node,
                element,
                slider,
                owed,
                ..
            } => {
                if let Some((group, token)) = *element {
                    self.elements.insert(node.number(), (token, group));
                }
                if let Some(focus) = self.focuses.last_mut() {
                    focus.slider = slider.map(|(_, token)| token);
                }
                self.owed = owed.then_some(OwedModel::Due);
            }
            Commit::Foreground {
                node,
                role,
                batch,
                dialog,
            } => {
                self.reported = Some((*role, *batch));
                self.dialog.clone_from(dialog);
                self.end_watch_elsewhere(Some(*node));
            }
            Commit::Element {
                node,
                group,
                element,
                slider,
            } => {
                if self.newest().is_some_and(|focus| focus.node == *node)
                    && !self.is_released(*node)
                {
                    self.elements.insert(node.number(), (*element, *group));
                    if let Some(focus) = self.focuses.last_mut() {
                        focus.slider = slider.map(|(_, token)| token);
                    }
                } else {
                    self.offered
                        .extend(std::iter::once(*element).chain(slider.map(|(_, token)| token)));
                }
            }
            Commit::OwedDone(_) => self.owed = None,
            Commit::Absorb(window) => self.absorbed = Some(*window),
            Commit::DropSystemCaret => self.system_caret_unused = true,
            Commit::ClearRedirect => {
                if let Some(focus) = self.focuses.last_mut() {
                    focus.redirected = false;
                }
            }
            Commit::Open { request, node } => self.open_watch(*node, request.0),
            Commit::Answer(_) => {}
        }
    }

    /// A program was dispatched for `kind`.
    fn dispatched(&mut self, kind: &Kind) {
        match kind {
            Kind::Owed => {
                assert_eq!(
                    self.owed,
                    Some(OwedModel::Due),
                    "owed work dispatched when nothing is due"
                );
                self.owed = Some(OwedModel::Attempted);
                self.absorbed = None;
            }
            Kind::Key(..) | Kind::Terminal(..) => self.absorbed = None,
            _ => {}
        }
        if !matches!(kind, Kind::Owed) {
            assert_ne!(
                self.owed,
                Some(OwedModel::Due),
                "owed work is attempted before anything else"
            );
        }
    }

    /// The worker attempting the owed work was abandoned, or a release let
    /// go of `released`.
    fn abandoned(&mut self, kind: &Kind) {
        if matches!(kind, Kind::Owed) {
            self.owed = None;
        }
    }

    fn release(&mut self, released: &BTreeSet<u64>) {
        self.released.extend(released);
        self.elements.retain(|node, _| !released.contains(node));
        if self
            .newest()
            .is_some_and(|focus| released.contains(&focus.node.number()))
            && let Some(focus) = self.focuses.last_mut()
        {
            focus.slider = None;
        }
        if self
            .watch
            .is_some_and(|(node, _)| released.contains(&node.number()))
        {
            self.end_watch();
        }
    }
}

// ----- The world -----

/// One kind of accessible object the world's application has.
#[derive(Clone, Copy, Debug)]
struct Spec {
    /// The MSAA child id, for an MSAA object.
    child: Option<i32>,
    /// Its id, the last part of its runtime id.
    id: i32,
    window: isize,
    role: Role,
    group: Group,
    terminal: bool,
    console: bool,
    thumb: bool,
}

const fn msaa_spec(child: i32, role: Role) -> Spec {
    Spec {
        child: Some(child),
        id: 0,
        window: 9,
        role,
        group: Group::Base,
        terminal: false,
        console: false,
        thumb: false,
    }
}

const fn uia_spec(id: i32, window: isize, role: Role, group: Group) -> Spec {
    Spec {
        child: None,
        id,
        window,
        role,
        group,
        terminal: false,
        console: false,
        thumb: false,
    }
}

/// The application's objects, the first two MSAA, the rest UIA: a button and
/// a check box; a button and an edit; a Windows Terminal; a console; a thumb
/// and its slider.
const OBJECTS: [Spec; 8] = [
    msaa_spec(1, Role::Button),
    msaa_spec(2, Role::CheckBox),
    uia_spec(1, 20, Role::Button, Group::Base),
    uia_spec(2, 20, Role::EditableText, Group::Extended),
    Spec {
        terminal: true,
        ..uia_spec(3, 21, Role::Terminal, Group::Extended)
    },
    Spec {
        terminal: true,
        console: true,
        ..uia_spec(4, 22, Role::Terminal, Group::ExtendedWithoutTextEvents)
    },
    Spec {
        thumb: true,
        ..uia_spec(5, 20, Role::Button, Group::Base)
    },
    uia_spec(6, 20, Role::Slider, Group::Base),
];

const FIRST_UIA: usize = 2;
const CONSOLE: usize = 5;
const SLIDER: usize = 7;
const CONSOLE_WINDOW: isize = 22;

/// The windows foreground changes bring in front, each with its own node.
const FOREGROUND: [isize; 2] = [30, 31];

/// The first node number the world mints; ids below it are a foreground
/// window's or an ancestor's.
const FIRST_NODE: u64 = 1_000;

/// An object's runtime id.
fn runtime_id(spec: &Spec) -> Vec<i32> {
    vec![42, spec.id]
}

/// The runtime id of an object's parent in its chain: the slider for a
/// thumb, otherwise a parent of its own.
fn parent_id(spec: &Spec) -> Vec<i32> {
    if spec.thumb {
        runtime_id(&OBJECTS[SLIDER])
    } else {
        vec![42, 100 + spec.id]
    }
}

/// The object `spec` as the intake records it.
fn object_of(spec: &Spec) -> Object {
    match spec.child {
        Some(child) => Object::Msaa(spec.window, OBJID_CLIENT, child),
        None => Object::Uia(runtime_id(spec)),
    }
}

/// The index in [`OBJECTS`] of the UIA object with `id`, a runtime id.
fn spec_by_id(id: &[i32]) -> Option<usize> {
    OBJECTS
        .iter()
        .position(|spec| spec.child.is_none() && runtime_id(spec) == id)
}

/// A snapshot of an element's cached parts.
fn snapshot(runtime_id: Vec<i32>, role: Role) -> UiaSnapshotFact {
    UiaSnapshotFact {
        runtime_id,
        role,
        name: None,
        value: None,
        states: StateSet::new(),
        details: verbatim_model::NodeDetails::default(),
    }
}

/// A focus listener's read of `spec`, whose own window and parent's window
/// are given, so that a route to it starts at a window.
fn listener_read(spec: &Spec) -> UiaFocusRead {
    UiaFocusRead {
        hwnd: spec.window,
        snapshot: snapshot(runtime_id(spec), spec.role),
        ancestors: vec![UiaAncestorFact {
            snapshot: snapshot(parent_id(spec), Role::Group),
            hwnd: spec.window,
            content: true,
        }],
        ancestors_end: AncestorsEnd::Top,
        selected_child: None,
        value_from_selection: false,
        window: Some(spec.window),
        same_as_last: None,
        answers: None,
        cached_from: None,
    }
}

/// How a terminal's reading stands, the world's truth a program reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Terminal {
    Live,
    Held { changed: bool },
}

/// What a program does, as the world follows it.
#[derive(Clone, Debug)]
enum Kind {
    /// A focus of the object at this index, by an MSAA event.
    MsaaFocus(usize),
    /// The listener's read of a focus of the object at this index.
    UiaRead(usize, Box<UiaFocusRead>),
    /// A foreground change of the window at this index.
    Foreground(usize),
    /// An event of the object at this index.
    Event(usize),
    /// A caret event of the UIA object at this index.
    UiaCaret(usize),
    /// An update of the console window.
    Console,
    /// A focus-now query.
    FocusNow((RequestId, TraceId)),
    /// A caret key's query.
    Key((RequestId, TraceId)),
    /// One of the terminal's on-demand reading requests.
    Terminal((RequestId, TraceId), TextOpKind),
    /// The followed focus's owed work.
    Owed,
    /// Anything else: an event of another object.
    Other,
}

/// A terminal request, as the world follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextOpKind {
    Hold,
    ReadHold,
    ReadOnce,
    Cancel,
}

/// The program in flight.
#[derive(Clone, Debug)]
struct Flight {
    request: Request,
    kind: Kind,
    /// Its copy of the facts, with its own commits applied.
    copy: Facts,
    batch: u64,
    committed: bool,
    /// Whether it reported a focus.
    reported: bool,
}

/// One step of a case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// An MSAA focus event of the object at this index among the first two.
    MsaaFocus(u8),
    /// A foreground change of the window at this index.
    Foreground(u8),
    /// A UIA focus's marker, for the UIA object at this index among the rest.
    Marker(u8),
    /// The listener's read taking the first marker's place.
    Read,
    /// The listener dropping the first marker.
    Dropped,
    /// An event of the object at this index, a caret event when `caret`.
    Event { object: u8, caret: bool },
    /// A console update of the console window.
    ConsoleUpdate,
    /// A focus-now query.
    FocusNow,
    /// A caret key on the node the world's application has as focus.
    Key,
    /// A terminal reading request.
    Terminal(u8),
    /// The program in flight does what it is scripted to: each flag's
    /// meaning depends on the program.
    Commit { a: bool, b: bool, c: bool, d: bool },
    /// The program in flight finds the element of the newest focus, or of an
    /// older one.
    Element { older: bool },
    /// The program in flight ends.
    Finish,
    /// The program in flight panics.
    Panic,
    /// The program in flight's deadline passes.
    Deadline,
    /// An abandoned worker's commit arrives.
    DeadCommit,
    /// Core's held nodes arrive: the objects at the mask's bits are released.
    Release(u8),
}

/// One case's state.
struct World {
    reducer: Reducer,
    answers: Answers,
    model: Model,
    at: u64,
    log: Vec<String>,
    trail: Vec<Action>,
    /// The node of each object the world has reported, by object index.
    nodes: BTreeMap<usize, u64>,
    next_node: u64,
    /// The token of each node's element, as the store keeps them.
    elements: BTreeMap<u64, u64>,
    next_token: u64,
    next_request: u64,
    /// The mailbox's copy of the marks.
    mailbox: Marks,
    /// The subscription, as last moved.
    subscription: Followed,
    /// Tokens let go this run.
    let_go: BTreeSet<u64>,
    /// Tokens let go in this step.
    let_go_now: BTreeSet<u64>,
    watch_ended_now: Vec<RequestId>,
    replies: BTreeMap<RequestId, u32>,
    flight: Option<Flight>,
    dead: Option<Request>,
    markers: VecDeque<(u64, usize)>,
    next_marker: u64,
    pending_release: Option<u8>,
    /// The last focus the application raised.
    application_focus: Option<usize>,
    /// The console terminal's reading.
    terminal: Terminal,
    /// When the caret key's watch opened, on the clock in milliseconds.
    watch_opened_at: u64,
    /// The shapes this case has reached.
    reached: BTreeSet<&'static str>,
}

impl World {
    fn new() -> Self {
        let mut reducer = Reducer::new();
        let _ = reducer.input(Input::HandlingDeadline(Duration::from_millis(1)), now(1));
        Self {
            reducer,
            answers: Answers::default(),
            model: Model::default(),
            at: 1_000,
            log: Vec::new(),
            trail: Vec::new(),
            nodes: BTreeMap::new(),
            next_node: FIRST_NODE,
            elements: BTreeMap::new(),
            next_token: 1,
            next_request: 0,
            mailbox: Marks::default(),
            subscription: Followed::default(),
            let_go: BTreeSet::new(),
            let_go_now: BTreeSet::new(),
            watch_ended_now: Vec::new(),
            replies: BTreeMap::new(),
            flight: None,
            dead: None,
            markers: VecDeque::new(),
            next_marker: 1,
            pending_release: None,
            application_focus: None,
            terminal: Terminal::Live,
            watch_opened_at: 0,
            reached: BTreeSet::new(),
        }
    }

    fn node_of(&mut self, object: usize) -> NodeId {
        let next = &mut self.next_node;
        let node = *self.nodes.entry(object).or_insert_with(|| {
            *next += 1;
            *next
        });
        NodeId::new(node)
    }

    /// The element of `node`, by its token: the store's, or a new one when
    /// `found`.
    fn element_of(&mut self, node: NodeId, found: bool) -> Option<u64> {
        if let Some(&token) = self.elements.get(&node.number()) {
            return Some(token);
        }
        found.then(|| {
            let token = self.next_token;
            self.next_token += 1;
            self.elements.insert(node.number(), token);
            token
        })
    }

    /// The node's element replaced by a new one, by a new token.
    fn replace_element(&mut self, node: NodeId) -> u64 {
        let token = self.next_token;
        self.next_token += 1;
        self.elements.insert(node.number(), token);
        token
    }

    /// A UIA object's slider, by its node and token, when it is a thumb and
    /// the slider's element is known or `found`.
    fn slider_of(&mut self, spec: &Spec, found: bool) -> Option<(NodeId, u64)> {
        if !spec.thumb {
            return None;
        }
        let slider = self.node_of(SLIDER);
        self.element_of(slider, found).map(|token| (slider, token))
    }

    fn input(&mut self, input: Input) -> Vec<Effect> {
        let at = now(self.at);
        let effects = self.reducer.input(input, at);
        with_answers(&mut self.reducer, &mut self.answers, effects, at)
    }

    /// Delivers `item` as the mailbox does: dropped as it is pushed when its
    /// copy of the marks says so.
    fn push(&mut self, item: Item) {
        if seam::drops(&self.mailbox, &item) {
            self.reached.insert(match item {
                Item::Msaa {
                    kind: WinKind::ConsoleUpdate,
                    ..
                } => "a held console's update dropped",
                _ => "a system caret event dropped",
            });
            return;
        }
        let entry = entry(item, self.at);
        let effects = self.input(Input::Entry {
            entry,
            source: Source::Other,
        });
        self.process(effects);
    }

    /// The kind of `program`.
    fn kind_of(program: &Program) -> Kind {
        let entry = match &program.work {
            Work::Entry { entry, .. } => entry,
            Work::Owed(_) => return Kind::Owed,
            Work::Drop | Work::WatchRead | Work::Finish { .. } | Work::FindBar(_) => {
                return Kind::Other;
            }
        };
        match &entry.item {
            Item::Fact(DeliveredFact::MsaaFocus { id_child, .. }, _) => OBJECTS
                .iter()
                .position(|spec| spec.child == Some(*id_child))
                .map_or(Kind::Other, Kind::MsaaFocus),
            Item::Fact(DeliveredFact::Foreground { hwnd }, _) => FOREGROUND
                .iter()
                .position(|window| window == hwnd)
                .map_or(Kind::Other, Kind::Foreground),
            Item::Fact(DeliveredFact::UiaFocusRead { read, .. }, _) => {
                spec_by_id(&read.snapshot.runtime_id)
                    .map_or(Kind::Other, |at| Kind::UiaRead(at, read.clone()))
            }
            Item::Uia(event) => {
                let at = spec_by_id(&event.parts.runtime_id).unwrap_or(0);
                if matches!(event.kind, UiaKind::TextSelection) {
                    Kind::UiaCaret(at)
                } else {
                    Kind::Event(at)
                }
            }
            Item::Msaa {
                kind: WinKind::ConsoleUpdate,
                ..
            } => Kind::Console,
            Item::Msaa { id_child, .. } => OBJECTS
                .iter()
                .position(|spec| spec.child == Some(*id_child))
                .map_or(Kind::Other, Kind::Event),
            Item::Query { request_id, query } => {
                let query_id = (*request_id, entry.trace);
                match query {
                    Query::ReadFocus { .. } => Kind::FocusNow(query_id),
                    Query::Text { op, .. } => match op {
                        TextOp::Key(_) => Kind::Key(query_id),
                        TextOp::TerminalHold => Kind::Terminal(query_id, TextOpKind::Hold),
                        TextOp::TerminalRead { hold: true } => {
                            Kind::Terminal(query_id, TextOpKind::ReadHold)
                        }
                        TextOp::TerminalRead { hold: false } => {
                            Kind::Terminal(query_id, TextOpKind::ReadOnce)
                        }
                        TextOp::TerminalCancel => Kind::Terminal(query_id, TextOpKind::Cancel),
                        _ => Kind::Other,
                    },
                    _ => Kind::Other,
                }
            }
            _ => Kind::Other,
        }
    }

    /// A program was dispatched: it starts, and what it reads of its copy is
    /// checked against the model.
    fn dispatched(&mut self, program: Program) {
        let kind = Self::kind_of(&program);
        if matches!(kind, Kind::Owed) {
            self.reached.insert("owed work attempted");
        }
        self.model.dispatched(&kind);
        self.compare_reads("a program's copy at dispatch", &program.facts);
        self.check_mailbox("a program's dispatch");
        let batch = match program.work {
            Work::Entry { batch, .. } => batch,
            _ => 0,
        };
        self.flight = Some(Flight {
            request: program.request,
            kind,
            copy: program.facts,
            batch,
            committed: false,
            reported: false,
        });
    }

    fn compare_reads(&self, what: &str, facts: &Facts) {
        assert_eq!(
            seam::reads(facts),
            self.model.reads(),
            "{what} after {:?}",
            self.trail
        );
        for spec in &OBJECTS {
            for id in [runtime_id(spec), parent_id(spec)] {
                assert_eq!(
                    seam::route_found(facts, &id),
                    self.model.route(&id),
                    "{what}: the route to {id:?} after {:?}",
                    self.trail
                );
            }
        }
    }

    /// Checks the mailbox's copy of the marks against the model: what it
    /// drops of a console's updates and of the system caret's events.
    fn check_mailbox(&self, what: &str) {
        let update = Item::Msaa {
            kind: WinKind::ConsoleUpdate,
            hwnd: CONSOLE_WINDOW,
            id_object: 0,
            id_child: 0,
            raiser: 5,
        };
        assert_eq!(
            seam::drops(&self.mailbox, &update),
            self.model.absorbed == Some(CONSOLE_WINDOW),
            "a console update's drop at {what} after {:?}",
            self.trail
        );
        let caret = Item::Msaa {
            kind: WinKind::Caret,
            hwnd: 9,
            id_object: 0,
            id_child: 0,
            raiser: 6,
        };
        assert_eq!(
            seam::drops(&self.mailbox, &caret),
            self.model.system_caret_unused,
            "the system caret's drop at {what} after {:?}",
            self.trail
        );
    }

    /// Carries out `effects`, following what the world keeps of them.
    fn process(&mut self, effects: Vec<Effect>) {
        self.log
            .extend(effects.iter().map(|effect| format!("{effect:?}")));
        let mut queue: VecDeque<Effect> = effects.into();
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::Dispatch(program) => self.dispatched(*program),
                Effect::Send(message) => self.sent(&message),
                Effect::Subscribe(subscription) => self.moved(&subscription),
                Effect::Unfollow(tokens) => self.let_go(&tokens),
                Effect::Marks(marks) => self.mailbox = marks,
                Effect::Release { .. } => {
                    let more = self.released();
                    for effect in more.into_iter().rev() {
                        queue.push_front(effect);
                    }
                }
                Effect::Abandon { .. } => {
                    if let Some(flight) = self.flight.take() {
                        if matches!(flight.kind, Kind::Owed) {
                            self.reached.insert("owed work abandoned");
                        }
                        self.model.abandoned(&flight.kind);
                        self.dead = Some(flight.request);
                    }
                }
                _ => {}
            }
        }
    }

    /// The release asked for: the objects at the pending mask's bits let go.
    fn released(&mut self) -> Vec<Effect> {
        let mask = self.pending_release.take().unwrap_or(0);
        let chosen: Vec<usize> = (0..OBJECTS.len())
            .filter(|at| mask & (1 << at) != 0 && self.nodes.contains_key(at))
            .collect();
        let mut nodes = BTreeSet::new();
        if chosen.is_empty() {
            self.reached.insert("a release of nothing");
        }
        for object in chosen {
            let node = self
                .nodes
                .remove(&object)
                .expect("a chosen object has a node");
            nodes.insert(node);
            self.elements.remove(&node);
        }
        self.model.release(&nodes);
        self.input(Input::Released {
            nodes,
            anchors: BTreeSet::new(),
            objects: false,
        })
    }

    fn sent(&mut self, message: &OutpostToCore) {
        if let OutpostToCore::Event {
            event:
                NormalizedEvent::KeyAnswered {
                    tag,
                    answer: KeyAnswer::Ended,
                    ..
                },
            ..
        } = message
        {
            self.watch_ended_now.push(RequestId::shell(tag.0.0));
        }
        if let OutpostToCore::Reply { request_id, .. } = message {
            let count = self.replies.entry(*request_id).or_default();
            *count += 1;
            assert_eq!(
                *count, 1,
                "a second reply to {request_id:?} after {:?}",
                self.trail
            );
        }
    }

    /// The subscription was moved: it must differ from where it was, name
    /// no element twice.
    fn moved(&mut self, subscription: &Subscription) {
        let mut kept = BTreeMap::new();
        for &(token, group) in &subscription.kept {
            assert!(
                kept.insert(token, group).is_none(),
                "token {token} kept twice after {:?}",
                self.trail
            );
        }
        assert!(
            subscription
                .newest
                .is_none_or(|(token, _)| !kept.contains_key(&token)),
            "the newest element kept as well after {:?}",
            self.trail
        );
        let moved = Followed {
            newest: subscription.newest,
            slider: subscription.slider,
            kept,
        };
        assert_ne!(
            moved, self.subscription,
            "a move to where it was after {:?}",
            self.trail
        );
        self.subscription = moved;
    }

    fn let_go(&mut self, tokens: &[u64]) {
        for &token in tokens {
            assert!(
                self.let_go.insert(token),
                "token {token} let go twice after {:?}",
                self.trail
            );
            self.let_go_now.insert(token);
            self.elements.retain(|_, kept| *kept != token);
        }
    }

    /// Whether the program in flight has not yet committed.
    fn uncommitted(&self) -> bool {
        self.flight.as_ref().is_some_and(|flight| !flight.committed)
    }

    /// Notes the shape `commit` gives.
    fn note(&mut self, commit: &Commit) {
        let shape = match commit {
            Commit::Adopt(adoption) => match adoption.origin {
                Origin::Listener(_) if adoption.slider.is_some() => {
                    "a thumb adopted with its slider"
                }
                Origin::Listener(_) if adoption.replaced => "an element replaced under its node",
                Origin::Listener(_) if adoption.owed => "a terminal adopted, owing its first read",
                Origin::Listener(_) => "a focus the listener read adopted",
                Origin::Msaa(_) if adoption.redirected => "a redirected MSAA focus adopted",
                Origin::Msaa(_) => "an MSAA focus adopted",
                Origin::Answer => "a focus-now answer adopted",
            },
            Commit::Again { .. } => "a focus-now answer followed again",
            Commit::Foreground { .. } => "a foreground report",
            Commit::Element { node, .. } => {
                if self.model.newest().is_some_and(|focus| focus.node == *node) {
                    "the newest focus's element found"
                } else {
                    "an older focus's element found"
                }
            }
            Commit::OwedDone(_) => "owed work done",
            Commit::Absorb(_) => "a held console absorbed",
            Commit::DropSystemCaret => "the system caret dropped",
            Commit::ClearRedirect => "the redirect mark cleared",
            Commit::Open { .. } => "a caret key's watch opened",
            Commit::Answer(_) => "a query answered",
        };
        self.reached.insert(shape);
    }

    /// Commits `commit` for the program in flight.
    fn commit(&mut self, commit: &Commit) {
        self.note(commit);
        let flight = self.flight.as_mut().expect("a program in flight");
        flight.committed = true;
        let request = flight.request;
        let change = seam::to_change(commit);
        seam::apply_local(&mut flight.copy, request.generation, &change);
        self.model.commit(commit);
        let effects = self.input(Input::Commit {
            request,
            change: Box::new(change),
        });
        self.process(effects);
        // What the program reads after its commit is what the model holds.
        let copy = self
            .flight
            .as_ref()
            .expect("a commit does not end its program")
            .copy
            .clone();
        self.compare_reads("a program's copy after its commit", &copy);
    }

    fn finish(&mut self, panicked: bool) {
        let flight = self.flight.take().expect("a program in flight");
        let effects = self.input(Input::Finished {
            request: flight.request,
            reported: flight.reported,
            panicked,
            cut: None,
        });
        self.process(effects);
    }

    /// Takes `action`, then compares.
    #[expect(
        clippy::too_many_lines,
        reason = "one match over the world's actions, each a few lines"
    )]
    fn step(&mut self, action: Action) {
        self.trail.push(action);
        self.at += 1;
        let before = self.model.followed().tokens();
        self.watch_ended_now.clear();
        self.model.ended_now.clear();
        self.model.offered.clear();
        self.let_go_now.clear();
        match action {
            Action::MsaaFocus(at) => {
                let object = usize::from(at) % FIRST_UIA;
                self.application_focus = Some(object);
                let child = OBJECTS[object].child.expect("an MSAA object");
                let time = u32::try_from(self.at).expect("the clock fits");
                self.push(Item::Fact(
                    DeliveredFact::MsaaFocus {
                        hwnd: OBJECTS[object].window,
                        id_object: OBJID_CLIENT,
                        id_child: child,
                    },
                    Some(WinEventStamp { thread: 3, time }),
                ));
            }
            Action::Foreground(at) => {
                let time = u32::try_from(self.at).expect("the clock fits");
                self.push(Item::Fact(
                    DeliveredFact::Foreground {
                        hwnd: FOREGROUND[usize::from(at) % FOREGROUND.len()],
                    },
                    Some(WinEventStamp { thread: 3, time }),
                ));
            }
            Action::Marker(at) => {
                let object = FIRST_UIA + usize::from(at) % (OBJECTS.len() - FIRST_UIA);
                self.application_focus = Some(object);
                let marker = self.next_marker;
                self.next_marker += 1;
                self.markers.push_back((marker, object));
                self.push(Item::Fact(DeliveredFact::UiaFocusMarker { marker }, None));
            }
            Action::Read => {
                if let Some((marker, object)) = self.markers.pop_front() {
                    let read = listener_read(&OBJECTS[object]);
                    self.push(Item::Fact(
                        DeliveredFact::UiaFocusRead {
                            marker,
                            read: Box::new(read),
                            focus: true,
                        },
                        None,
                    ));
                }
            }
            Action::Dropped => {
                if let Some((marker, _)) = self.markers.pop_front() {
                    self.push(Item::Fact(
                        DeliveredFact::UiaFocusDropped {
                            marker,
                            focus: true,
                        },
                        None,
                    ));
                }
            }
            Action::Event { object, caret } => self.event(object, caret),
            Action::ConsoleUpdate => self.push(Item::Msaa {
                kind: WinKind::ConsoleUpdate,
                hwnd: CONSOLE_WINDOW,
                id_object: 0,
                id_child: 0,
                raiser: 5,
            }),
            Action::FocusNow => {
                let id = self.fresh_request();
                self.push(focus_now(id));
            }
            Action::Key => {
                let node = self.key_node();
                if let Some(node) = node {
                    let id = self.fresh_request();
                    // A key's request replaces the watch as it arrives.
                    self.model.end_watch();
                    self.model.reading = Some(RequestId::shell(id));
                    self.push(text_query(id, node, key_op(id)));
                }
            }
            Action::Terminal(op) => {
                if let Some(node) = self.console_node() {
                    let id = self.fresh_request();
                    self.push(text_query(id, node, terminal_op(op)));
                }
            }
            Action::Commit { a, b, c, d } => self.scripted(a, b, c, d),
            Action::Element { older } => self.element(older),
            Action::Finish => {
                if self.flight.is_some() {
                    self.finish(false);
                }
            }
            Action::Panic => {
                if self.flight.is_some() {
                    self.finish(true);
                }
            }
            Action::Deadline => self.deadline(),
            Action::DeadCommit => self.dead_commit(),
            Action::Release(mask) => {
                self.pending_release = Some(mask);
                self.push(Item::NodesHeld {
                    nodes: Vec::new(),
                    anchors: Vec::new(),
                    acknowledged: 0,
                });
            }
        }
        self.check(&before);
    }

    fn fresh_request(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    /// The node a caret key names: the newest focus's or the one under the
    /// application's own focus.
    fn key_node(&mut self) -> Option<NodeId> {
        let object = self.application_focus?;
        Some(self.node_of(object))
    }

    /// The node of the console terminal, when the world has reported it.
    fn console_node(&mut self) -> Option<NodeId> {
        self.nodes.get(&CONSOLE).map(|&node| NodeId::new(node))
    }

    fn event(&mut self, object: u8, caret: bool) {
        let at = usize::from(object) % OBJECTS.len();
        let spec = OBJECTS[at];
        let item = match spec.child {
            Some(child) => Item::Msaa {
                kind: if caret {
                    WinKind::Caret
                } else {
                    WinKind::NameChange
                },
                hwnd: spec.window,
                id_object: OBJID_CLIENT,
                id_child: child,
                raiser: 4,
            },
            None => Item::Uia(UiaEvent {
                kind: if caret {
                    UiaKind::TextSelection
                } else {
                    UiaKind::Property(entry::property::NAME)
                },
                parts: snapshot(runtime_id(&spec), spec.role),
                hwnd: spec.window,
                focus_window: 0,
                element: None,
            }),
        };
        self.push(item);
    }

    /// The program in flight does what it is scripted to, each flag's
    /// meaning its kind's.
    #[expect(
        clippy::too_many_lines,
        clippy::fn_params_excessive_bools,
        reason = "one match over the kinds of program, whose flags select its scripted variation"
    )]
    fn scripted(&mut self, a: bool, b: bool, c: bool, d: bool) {
        if !self.uncommitted() {
            return;
        }
        let flight = self.flight.as_ref().expect("a program in flight");
        let batch = flight.batch;
        match flight.kind.clone() {
            Kind::MsaaFocus(object) => {
                let spec = OBJECTS[object];
                let node = self.node_of(object);
                let dialog = Some(d.then(|| (node, Some("a dialog".to_owned()))));
                let event = (
                    spec.window,
                    OBJID_CLIENT,
                    spec.child.expect("an MSAA object"),
                );
                self.adopt(Box::new(Adoption {
                    node,
                    role: spec.role,
                    object: Some(object_of(&spec)),
                    window: spec.window,
                    chain: vec![node],
                    batch,
                    redirected: b,
                    terminal: false,
                    console: false,
                    element: None,
                    slider: None,
                    owed: false,
                    origin: Origin::Msaa(event),
                    dialog,
                    replaced: false,
                    answer: None,
                }));
            }
            Kind::UiaRead(object, read) => {
                let spec = OBJECTS[object];
                let node = self.node_of(object);
                let replaced = b && self.elements.contains_key(&node.number());
                let token = if replaced {
                    Some(self.replace_element(node))
                } else {
                    self.element_of(node, a)
                };
                let element = token.map(|token| (spec.group, token));
                let slider = if token.is_some() {
                    self.slider_of(&spec, false)
                } else {
                    None
                };
                let dialog = Some(d.then(|| (node, Some("a dialog".to_owned()))));
                self.adopt(Box::new(Adoption {
                    node,
                    role: spec.role,
                    object: Some(object_of(&spec)),
                    window: spec.window,
                    chain: vec![NodeId::new(9_000), node],
                    batch,
                    redirected: false,
                    terminal: spec.terminal,
                    console: spec.console,
                    element,
                    slider,
                    owed: spec.terminal && c,
                    origin: Origin::Listener(read),
                    dialog,
                    replaced,
                    answer: None,
                }));
            }
            Kind::Foreground(window) => {
                let node = NodeId::new(100 + u64::try_from(window).expect("a small index"));
                let dialog = a.then(|| (node, Some("a dialog".to_owned())));
                self.commit(&Commit::Foreground {
                    node,
                    role: Role::Window,
                    batch,
                    dialog,
                });
                self.flight.as_mut().expect("in flight").reported = true;
            }
            Kind::FocusNow(query) => self.answer_focus_now(query, a, c, batch),
            Kind::Key(query) => {
                if b {
                    self.commit(&Commit::Answer(query));
                } else {
                    let node = if a {
                        self.older_node()
                            .or_else(|| self.model.newest().map(|focus| focus.node))
                    } else {
                        self.model.newest().map(|focus| focus.node)
                    };
                    if let Some(node) = node {
                        self.watch_opened_at = self.at;
                        self.commit(&Commit::Open {
                            request: query,
                            node,
                        });
                    }
                }
            }
            Kind::Terminal(query, op) => {
                self.terminal = match (op, self.terminal) {
                    (TextOpKind::Hold, Terminal::Live) | (TextOpKind::ReadHold, _) => {
                        Terminal::Held { changed: false }
                    }
                    (TextOpKind::Hold, held) => held,
                    (TextOpKind::ReadOnce | TextOpKind::Cancel, _) => Terminal::Live,
                };
                self.commit(&Commit::Answer(query));
            }
            Kind::Console => {
                if let Terminal::Held { .. } = self.terminal {
                    self.terminal = Terminal::Held { changed: true };
                }
                let held_changed = self.terminal == Terminal::Held { changed: true };
                let console_focus = self
                    .model
                    .newest()
                    .is_some_and(|focus| focus.window == CONSOLE_WINDOW && focus.terminal);
                if held_changed && console_focus && self.model.watch.is_none() {
                    self.commit(&Commit::Absorb(CONSOLE_WINDOW));
                }
            }
            Kind::UiaCaret(object) => {
                if a && self.model.is_newest(&object_of(&OBJECTS[object])) {
                    self.commit(&Commit::DropSystemCaret);
                }
            }
            Kind::Event(object) => {
                let redirected = self.model.newest().is_some_and(|focus| focus.redirected);
                if b && redirected && self.model.is_newest(&object_of(&OBJECTS[object])) {
                    self.commit(&Commit::ClearRedirect);
                }
            }
            Kind::Owed => {
                if a {
                    let node = self
                        .model
                        .newest()
                        .expect("owed work belongs to a focus")
                        .node;
                    self.commit(&Commit::OwedDone(node));
                }
            }
            Kind::Other => {}
        }
    }

    /// Adopts `adoption` as a focus program's commit.
    fn adopt(&mut self, adoption: Box<Adoption>) {
        if adoption.node.number() != self.model.newest().map_or(0, |focus| focus.node.number()) {
            self.terminal = Terminal::Live;
        }
        self.commit(&Commit::Adopt(adoption));
        if let Some(flight) = self.flight.as_mut() {
            flight.reported = true;
        }
    }

    /// A focus-now answer: only followed again when it names the focus
    /// already followed under the same node, adopted otherwise.
    fn answer_focus_now(&mut self, query: (RequestId, TraceId), a: bool, c: bool, batch: u64) {
        let Some(object) = self.application_focus else {
            self.commit(&Commit::Answer(query));
            return;
        };
        let spec = OBJECTS[object];
        let node = self.node_of(object);
        // An MSAA focus has no element to follow.
        let token = if spec.child.is_some() {
            None
        } else {
            self.element_of(node, a)
        };
        let element = token.map(|token| (spec.group, token));
        let slider = if token.is_some() {
            self.slider_of(&spec, false)
        } else {
            None
        };
        let owed = spec.terminal && c;
        if self.model.newest().is_some_and(|focus| focus.node == node)
            && !self.model.is_released(node)
        {
            self.commit(&Commit::Again {
                node,
                role: spec.role,
                element,
                slider,
                owed,
                answer: query,
            });
            return;
        }
        self.terminal = Terminal::Live;
        self.commit(&Commit::Adopt(Box::new(Adoption {
            node,
            role: spec.role,
            object: Some(object_of(&spec)),
            window: spec.window,
            chain: vec![node],
            batch,
            redirected: false,
            terminal: spec.terminal,
            console: spec.console,
            element,
            slider,
            owed,
            origin: Origin::Answer,
            dialog: None,
            replaced: false,
            answer: Some(query),
        })));
        if let Some(flight) = self.flight.as_mut() {
            flight.reported = true;
        }
    }

    /// The node of an older focus, not the newest and not released.
    fn older_node(&self) -> Option<NodeId> {
        let newest = self.model.newest()?.node;
        self.model
            .focuses
            .iter()
            .rev()
            .map(|focus| focus.node)
            .find(|node| *node != newest && !self.model.is_released(*node))
    }

    /// The program in flight finds the element of the newest focus, or an
    /// older one.
    fn element(&mut self, older: bool) {
        if self.flight.is_none() {
            return;
        }
        let node = if older {
            self.older_node()
        } else {
            self.model
                .newest()
                .map(|focus| focus.node)
                .filter(|node| !self.model.is_released(*node))
        };
        let Some(node) = node else {
            return;
        };
        let object = self
            .nodes
            .iter()
            .find(|(_, number)| **number == node.number())
            .map(|(object, _)| *object);
        let Some(object) = object else {
            return;
        };
        let spec = OBJECTS[object];
        if spec.child.is_some() {
            return;
        }
        let element = self.element_of(node, true).expect("found");
        let slider = self.slider_of(&spec, true);
        let flight = self.flight.as_mut().expect("in flight");
        let request = flight.request;
        let change = seam::to_change(&Commit::Element {
            node,
            group: spec.group,
            element,
            slider,
        });
        seam::apply_local(&mut flight.copy, request.generation, &change);
        self.note(&Commit::Element {
            node,
            group: spec.group,
            element,
            slider,
        });
        self.model.commit(&Commit::Element {
            node,
            group: spec.group,
            element,
            slider,
        });
        let effects = self.input(Input::Commit {
            request,
            change: Box::new(change),
        });
        self.process(effects);
    }

    fn deadline(&mut self) {
        let Some(flight) = self.flight.as_ref() else {
            return;
        };
        // The focus read deadline adds two seconds; no jump reaches the
        // caret key's bound, whose own expiry the watch's examples cover.
        let slow = matches!(flight.kind, Kind::UiaRead(..) | Kind::FocusNow(_));
        if self.model.watch.is_some() && (slow || self.watch_opened_at + 10_000 < self.at + 3_000) {
            return;
        }
        let request = flight.request;
        loop {
            let due = self
                .reducer
                .next_timer(now(self.at))
                .expect("a program in flight has a time");
            self.at = self
                .at
                .max(u64::try_from(due.as_millis()).expect("fits") + 1);
            let at = now(self.at);
            let effects = self.reducer.tick(at);
            let effects = with_answers(&mut self.reducer, &mut self.answers, effects, at);
            let abandoned = effects
                .iter()
                .any(|effect| matches!(effect, Effect::Abandon { .. }));
            self.process(effects);
            if abandoned || !self.reducer.accepts(request) {
                return;
            }
        }
    }

    /// An abandoned worker's commit arrives, as a new focus; the reducer
    /// refuses it.
    fn dead_commit(&mut self) {
        let Some(request) = self.dead else {
            return;
        };
        self.reached.insert("an abandoned worker's commit refused");
        let spec = OBJECTS[0];
        let node = NodeId::new(99);
        let change = seam::to_change(&Commit::Adopt(Box::new(Adoption {
            node,
            role: spec.role,
            object: Some(object_of(&spec)),
            window: spec.window,
            chain: vec![node],
            batch: 0,
            redirected: false,
            terminal: false,
            console: false,
            element: None,
            slider: None,
            owed: false,
            origin: Origin::Answer,
            dialog: None,
            replaced: false,
            answer: None,
        })));
        let effects = self.input(Input::Commit {
            request,
            change: Box::new(change),
        });
        self.process(effects);
    }

    /// Compares the step's outcome with the model, then the probes.
    fn check(&mut self, before: &BTreeSet<u64>) {
        let trail = &self.trail;
        let expected = self.model.followed();
        assert_eq!(
            self.subscription, expected,
            "the subscription after {trail:?}"
        );
        let after = expected.tokens();
        let let_go: BTreeSet<u64> = before
            .difference(&after)
            .copied()
            .chain(
                self.model
                    .offered
                    .iter()
                    .copied()
                    .filter(|token| !after.contains(token)),
            )
            .collect();
        assert_eq!(self.let_go_now, let_go, "the tokens let go after {trail:?}");
        assert_eq!(
            self.watch_ended_now, self.model.ended_now,
            "the watches ended after {trail:?}"
        );
        self.check_mailbox("the end of the step");
        if !self.let_go_now.is_empty() {
            self.reached.insert("a token let go");
        }
        if !self.watch_ended_now.is_empty() {
            self.reached.insert("a caret key's watch ended");
        }
        if !expected.kept.is_empty() {
            self.reached.insert("an older focus's element kept");
        }
        // Owed work is attempted before anything else.
        if self.model.owed == Some(OwedModel::Due) {
            assert!(
                self.reducer.busy(),
                "owed work not attempted after {trail:?}"
            );
        }
        self.probe_events();
    }

    /// Asks what the planning does with later events, on a copy of the
    /// reducer so the case goes on undisturbed: a later event of the newest
    /// focus is planned as that focus's, and an event of an older focus is
    /// not.
    fn probe_events(&self) {
        let older = self
            .model
            .focuses
            .iter()
            .rev()
            .map(|focus| &focus.object)
            .find(|object| !self.model.is_newest(object));
        for object in self
            .model
            .newest()
            .map(|focus| &focus.object)
            .into_iter()
            .chain(older)
        {
            match object {
                Object::Msaa(_, _, child) => self.probe_limit(*child, object),
                Object::Uia(id) => self.probe_uia(id, object),
            }
        }
    }

    /// Runs `items` as one batch on a copy of the reducer, with the program
    /// in flight ended first, and gives back the traces of those handled,
    /// in order.
    fn run_batch(&self, items: Vec<Item>) -> Vec<u64> {
        let mut reducer = self.reducer.clone();
        let mut answers = self.answers.clone();
        let at = now(self.at + 1);
        let probe_trace = 1_000_000;
        let mut effects = reducer.input(
            Input::Entry {
                entry: Entry::own(
                    Item::Fact(DeliveredFact::UiaFocusReadsEnded, None),
                    TraceId::from_raw(probe_trace),
                    0,
                ),
                source: Source::Other,
            },
            at,
        );
        let sentinel = Item::Msaa {
            kind: WinKind::NameChange,
            hwnd: 77,
            id_object: OBJID_CLIENT,
            id_child: 77,
            raiser: 88,
        };
        for (offset, item) in std::iter::once(sentinel).chain(items).enumerate() {
            let trace = probe_trace + 1 + u64::try_from(offset).expect("small");
            effects.extend(reducer.input(
                Input::Entry {
                    entry: Entry::own(item, TraceId::from_raw(trace), trace),
                    source: Source::Other,
                },
                at,
            ));
        }
        let mut queue: VecDeque<Effect> =
            with_answers(&mut reducer, &mut answers, effects, at).into();
        if let Some(flight) = &self.flight {
            queue.extend(reducer.input(
                Input::Finished {
                    request: flight.request,
                    reported: false,
                    panicked: false,
                    cut: None,
                },
                at,
            ));
        }
        let mut handled = Vec::new();
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::Dispatch(program) => {
                    if let Work::Entry { entry, .. } = &program.work {
                        let trace = entry.observed_at_us;
                        if trace > probe_trace {
                            handled.push(trace);
                        }
                    }
                    queue.extend(reducer.input(
                        Input::Finished {
                            request: program.request,
                            reported: false,
                            panicked: false,
                            cut: None,
                        },
                        at,
                    ));
                }
                Effect::Release { .. } => queue.extend(reducer.input(
                    Input::Released {
                        nodes: BTreeSet::new(),
                        anchors: BTreeSet::new(),
                        objects: false,
                    },
                    at,
                )),
                _ => {}
            }
        }
        handled
    }

    /// An MSAA event of `object`, past the batch's limit of other events: it
    /// is kept only as the focus's.
    fn probe_limit(&self, child: i32, object: &Object) {
        let probe = 900_000;
        let mut items = vec![Item::Msaa {
            kind: WinKind::NameChange,
            hwnd: 9,
            id_object: OBJID_CLIENT,
            id_child: child,
            raiser: 77,
        }];
        for extra in 0..10_i32 {
            items.push(Item::Msaa {
                kind: WinKind::NameChange,
                hwnd: 66,
                id_object: OBJID_CLIENT,
                id_child: extra + probe,
                raiser: 77,
            });
        }
        let handled = self.run_batch(items);
        let kept = handled.contains(&(1_000_000 + 2));
        assert_eq!(
            kept,
            self.model.is_newest(object),
            "the batch limit's keep of {object:?} after {:?}",
            self.trail
        );
    }

    /// A UIA object's events: a state change of it behind a marker whose read
    /// is a focus inside it is planned ahead of the read only as the focus's;
    /// a terminal's caret event is planned after its text change only as the
    /// focus's terminal.
    fn probe_uia(&self, id: &[i32], object: &Object) {
        let marker = 777_777;
        let parts = |kind| {
            Item::Uia(UiaEvent {
                kind,
                parts: snapshot(id.to_vec(), Role::Group),
                hwnd: 20,
                focus_window: 0,
                element: None,
            })
        };
        let read = UiaFocusRead {
            hwnd: 66,
            snapshot: snapshot(vec![77, 77], Role::Button),
            ancestors: vec![UiaAncestorFact {
                snapshot: snapshot(id.to_vec(), Role::Group),
                hwnd: 66,
                content: true,
            }],
            ancestors_end: AncestorsEnd::Top,
            selected_child: None,
            value_from_selection: false,
            window: Some(66),
            same_as_last: None,
            answers: None,
            cached_from: None,
        };
        // The sentinel is first, then marker, the change, the read.
        let handled = self.run_batch(vec![
            Item::Fact(DeliveredFact::UiaFocusMarker { marker }, None),
            parts(UiaKind::Property(30_070)),
            Item::Fact(
                DeliveredFact::UiaFocusRead {
                    marker,
                    read: Box::new(read),
                    focus: true,
                },
                None,
            ),
        ]);
        // Offsets: the sentinel 0, marker 1, the change 2, the read 3.
        let change = 1_000_000 + 3;
        let moved = handled.iter().position(|trace| *trace == change)
            < handled.iter().position(|trace| *trace == 1_000_000 + 4);
        assert_eq!(
            moved,
            self.model.is_newest(object),
            "the old focus's state change placed behind its marker's read of {object:?} after {:?}",
            self.trail
        );
        // A terminal's caret event, then its text change: moved after the
        // change only for the focus's terminal.
        let handled = self.run_batch(vec![
            parts(UiaKind::TextSelection),
            parts(UiaKind::TextChanged),
        ]);
        let terminal = self
            .model
            .newest()
            .is_some_and(|focus| focus.terminal && focus.object == *object);
        // Offsets: the sentinel 0, the caret 1, the change 2.
        let caret_after = handled.iter().position(|trace| *trace == 1_000_000 + 2)
            > handled.iter().position(|trace| *trace == 1_000_000 + 3);
        assert_eq!(
            caret_after, terminal,
            "the terminal's caret after its text change of {object:?} after {:?}",
            self.trail
        );
    }

    /// Ends the case: the program in flight ends, a held console reads
    /// nothing more, and the replies are counted.
    fn drain(&mut self) {
        while self.flight.is_some() {
            self.step(Action::Finish);
        }
    }

    /// One case: its actions, then the drain.
    fn case(actions: &[Action]) -> Self {
        let mut world = Self::new();
        for &action in actions {
            world.step(action);
        }
        world.drain();
        world
    }
}

/// The watch of a key on `node`, for a commit that opens it.
fn key_watch(request: &(RequestId, TraceId), node: NodeId) -> Box<KeyWatch> {
    Box::new(KeyWatch {
        tag: KeyTag(QueryId(request.0.number)),
        node,
        baseline: Baseline::Edit(LastRead {
            caret: 0,
            selection: None,
            line: verbatim_model::TextChunk {
                unit: TextUnit::Line,
                text: String::new(),
                start: verbatim_model::TextAnchor(1),
                offset: 0,
                languages: Vec::new(),
                first: false,
                last: false,
                truncated: false,
                formats: Vec::new(),
            },
            text_changed: false,
            read_at_us: 0,
        }),
        unit: TextUnit::Character,
        motion: CaretMotion::NextCharacter,
        // A terminal's, whose watch the outpost ends on a focus elsewhere.
        kind: WatchKind::Terminal,
        before_unit: None,
        trace: request.1,
        timing: EventTiming::default(),
        calls: CallCounts::default(),
    })
}

/// A focus-now query, request `request_id`.
fn text_query(request_id: u64, node_id: NodeId, op: TextOp) -> Item {
    Item::Query {
        request_id: RequestId::shell(request_id),
        query: Query::Text { node_id, op },
    }
}

/// A caret key's operation.
fn key_op(id: u64) -> TextOp {
    TextOp::Key(KeyRequest {
        tag: KeyTag(QueryId(id)),
        kind: WatchKind::Terminal,
        unit: TextUnit::Character,
        motion: CaretMotion::NextCharacter,
        start: KeyStart::ReadFirst,
    })
}

/// A terminal request.
fn terminal_op(op: u8) -> TextOp {
    match op % 4 {
        0 => TextOp::TerminalHold,
        1 => TextOp::TerminalRead { hold: true },
        2 => TextOp::TerminalRead { hold: false },
        _ => TextOp::TerminalCancel,
    }
}

fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    prop_oneof![
        5 => (0..4u8).prop_map(Action::MsaaFocus),
        2 => (0..4u8).prop_map(Action::Foreground),
        3 => (0..6u8).prop_map(Action::Marker),
        3 => Just(Action::Marker(3)),
        4 => Just(Action::Read),
        1 => Just(Action::Dropped),
        3 => (0..8u8, any::<bool>()).prop_map(|(object, caret)| Action::Event { object, caret }),
        5 => Just(Action::ConsoleUpdate),
        2 => Just(Action::FocusNow),
        3 => Just(Action::Key),
        2 => (0..4u8).prop_map(Action::Terminal),
        2 => Just(Action::Terminal(0)),
        10 => (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>())
            .prop_map(|(a, b, c, d)| Action::Commit { a, b, c, d }),
        3 => any::<bool>().prop_map(|older| Action::Element { older }),
        10 => Just(Action::Finish),
        1 => Just(Action::Panic),
        2 => Just(Action::Deadline),
        1 => Just(Action::DeadCommit),
        2 => any::<u8>().prop_map(Action::Release),
    ]
}

/// The shapes every run of the generated cases reaches.
const REACHED: [&str; 20] = [
    "a thumb adopted with its slider",
    "an element replaced under its node",
    "a terminal adopted, owing its first read",
    "a focus the listener read adopted",
    "a redirected MSAA focus adopted",
    "an MSAA focus adopted",
    "a focus-now answer adopted",
    "a focus-now answer followed again",
    "a foreground report",
    "the newest focus's element found",
    "an older focus's element found",
    "owed work attempted",
    "owed work done",
    "owed work abandoned",
    "a held console absorbed",
    "a held console's update dropped",
    "a token let go",
    "a caret key's watch ended",
    "an older focus's element kept",
    "an abandoned worker's commit refused",
];

/// The seed and case count of the gate's run.
const SEED: u64 = 20_261_109;
const CASES: u32 = 2_048;

/// Runs the generated cases `config` draws, each twice from a new reducer,
/// shrinking a failure to its smallest case.
///
/// # Panics
///
/// On a failing case, naming the smallest.
pub(super) fn run_generated(config: Config, again: &str) {
    let reached = RefCell::new(BTreeSet::new());
    let mut runner = TestRunner::new(config);
    let strategy = proptest::collection::vec(action(), 1..80);
    let result = runner.run(&strategy, |actions| {
        let first = World::case(&actions);
        let second = World::case(&actions);
        assert_eq!(first.log, second.log, "two runs of one case differ");
        reached.borrow_mut().extend(first.reached.iter().copied());
        Ok(())
    });
    match result {
        Ok(()) => {
            let reached = reached.into_inner();
            let missing: Vec<&&str> = REACHED
                .iter()
                .filter(|shape| !reached.contains(**shape))
                .collect();
            assert!(
                missing.is_empty(),
                "shapes the cases did not reach: {missing:?}"
            );
        }
        Err(TestError::Fail(reason, case)) => panic!("{reason}\n{}", fixed_report(&case, again)),
        Err(error) => panic!("{error}"),
    }
}

/// The report of a failing case: `case` as a line ready to paste into
/// [`fixed`], with the commands that run the fixed cases and the run that
/// found it.
fn fixed_report(case: &[Action], again: &str) -> String {
    let steps: Vec<String> = case
        .iter()
        .map(|action| format!("Action::{action:?}"))
        .collect();
    format!(
        "the smallest failing case, as a line for `fixed()` in crates/verbatim-outpost-logic/src/tests/followed.rs:\n        vec![{}],\n\
         the fixed cases alone: cargo test -p verbatim-outpost-logic --lib -- --exact tests::followed::fixed_cases_keep_the_followed_focus_rules\n\
         the run that found it again: {again}",
        steps.join(", ")
    )
}

/// Cases of a world of real applications' focus events, keys and terminals
/// keep what the followed focus exists to decide, compared with the
/// reference model after every step.
#[test]
fn generated_cases_keep_the_followed_focus_rules() {
    run_generated(
        super::explore::config(SEED, CASES),
        "cargo test -p verbatim-outpost-logic --lib -- --exact tests::followed::generated_cases_keep_the_followed_focus_rules",
    );
}

/// The cases the generated test failed on, each kept as its actions: an
/// MSAA focus adopted after a UIA focus keeps the older focus's route, where
/// an adoption replaces the whole record.
fn fixed() -> Vec<Vec<Action>> {
    vec![vec![
        Action::Marker(4),
        Action::MsaaFocus(0),
        Action::Read,
        Action::Commit {
            a: false,
            b: false,
            c: false,
            d: false,
        },
        Action::Finish,
        Action::Commit {
            a: false,
            b: false,
            c: false,
            d: false,
        },
    ]]
}

/// The found cases keep the followed focus's rules.
#[test]
fn fixed_cases_keep_the_followed_focus_rules() {
    for case in fixed() {
        let _ = World::case(&case);
    }
}
