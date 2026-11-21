//! Core's plain reader, and the decision a read of the focus NVDA makes
//! itself takes when the window it names is hung (Dickson, 2026-11-19).
//!
//! NVDA reads the focus itself, through `objectWithFocus()`, at the fake
//! focus after a menu end (`IAccessibleHandler/__init__.py` 976 to 1002,
//! 1082 to 1103), at the return from the secure desktop (809 to 813) and
//! at its start (`core.py` 422 to 435). Each takes the window in front,
//! `GetForegroundWindow` then its thread's focus window
//! (`NVDAObjects/window/__init__.py` 162 to 181). When that window is a
//! ghost, which Windows shows in place of a hung top-level window, NVDA
//! finds no API class for a hung window (83 to 98) and no overlay for the
//! class `Ghost` (100 to 140), so it makes a plain `Window` of it: its
//! window text as its name, the role window, only the unavailable state
//! from its style (299 to 306), no caret and no children reached, all with
//! no call into any application. Being no `IAccessible` object it is queued
//! with no duplicate test (`IAccessibleHandler/__init__.py` 772 to 782),
//! so it is spoken.
//!
//! Verbatim's shell reads the same facts when it refuses such a read as
//! hung ([`verbatim_model::request::Front`]), and the read's entry decides
//! from them by [`decide`]. A ghost found by the menu or popup end's
//! check, the start or the return is committed as an ordinary focus, the
//! plain object, through the same commit as any focus. Core's plain reader
//! holds it, one at a time, and answers every request on it at once from
//! its facts, exactly as NVDA's plain `Window` does, so every command,
//! today's and any later one, treats it as any object (Dickson,
//! 2026-11-19). Its facts are read once, at the decision. The ghost's
//! destruction leaves the record held and marks it dead
//! ([`PlainKind::Destroyed`]): NVDA's focus stays on its plain `Window`,
//! whose dead handle has no text and the style 0, so Insert+Tab says
//! "window invisible" (Dickson, 2026-11-19). The record ends only when the
//! next plain commit replaces it.

use serde::{Deserialize, Serialize};
use verbatim_model::batch::{
    Batch, Descended, Done, Gate, KindMismatch, Missing, Op, ReaderKind, StepOutcome, StepRefusal,
    Subject, Tested, parse,
};
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    DesktopFront, FocusCaret, FocusNode, FocusRead, Front, GhostFront, NodeRead, NodeWindow,
    ObjectRead, ObjectRef, Outcome as WireOutcome, Question, Refusal, RelationFound, RelationQuery,
    Reply, Request as Wire,
};
use verbatim_model::{
    Issuer, NodeId, Pid, Role, State, StateSet, TextReply, WindowFacts, WindowHandle,
};

use super::execute::{Executed, Known, Origin, execute};
use super::versions::ReadBy;
use crate::entries::Entries;
use crate::event_queue::{Cx, RequestWindow};
use crate::keys::entry::Fence;
use crate::state::SrState;

/// The number of the plain object's node within its generation.
const WINDOW_NUMBER: u64 = 1;

/// Which plain window Core's plain reader holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PlainKind {
    /// A ghost window found in front, with the facts the shell read of it
    /// at the decision.
    Ghost {
        /// The hung window it stands for.
        stands_for: WindowHandle,
        /// Its window text.
        name: String,
        /// Whether it is disabled.
        unavailable: bool,
    },
    /// A ghost whose window was destroyed while it was held: NVDA's plain
    /// `Window` of a dead handle, whose name is its empty window text and
    /// whose only state is invisible, from its style of 0
    /// (`NVDAObjects/window/__init__.py` 190 to 194, 299 to 306;
    /// `controlTypes/processAndLabelStates.py` 62 to 63 keeps it for a
    /// query). Same node and generation as the ghost it was.
    Destroyed,
    /// The desktop window, found in front when no window is: NVDA's
    /// `Desktop`, a plain window named "Desktop" (`NVDAObjects/window/
    /// __init__.py` 409 to 413), which Core names itself as it names every
    /// desktop window (`docs/parity/review.md`, "Dead objects").
    Desktop,
}

/// A plain window Core's plain reader holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlainObject {
    /// Its generation: its node's issuer, never reused.
    pub(crate) generation: u64,
    /// Its window, the ghost or the desktop window: a ghost's destruction
    /// marks the record [`PlainKind::Destroyed`].
    pub(crate) window: WindowHandle,
    /// Which plain window it is.
    pub(crate) kind: PlainKind,
    /// For a ghost, the application of the hung window it stands for, never
    /// the ghost's own (the desktop window manager's); for the desktop
    /// window, its owner. No rule naming the process starts an outpost for
    /// it, since every request on its node is the plain reader's.
    pub(crate) process: Pid,
    /// The thread of that window.
    pub(crate) thread: u32,
}

impl PlainObject {
    /// Its node.
    pub(crate) fn node(&self) -> NodeId {
        NodeId::plain(self.generation, WINDOW_NUMBER)
    }

    /// Its window as an answer names a node's nearest window: a top-level
    /// window of its own, marked as the desktop's for the desktop window.
    fn node_window(&self) -> NodeWindow {
        NodeWindow {
            handle: self.window,
            top_level: self.window,
            root_owner: self.window,
            process: self.process,
            thread: self.thread,
            desktop: self.kind == PlainKind::Desktop,
        }
    }

    /// Its properties of `set`, each once: a ghost's name, the role window,
    /// the unavailable state when disabled, the invisible state alone for a
    /// destroyed ghost, its own window; nothing else
    /// (`NVDAObjects/window/__init__.py` 299 to 306; `winUser.py` 652 to
    /// 655). The desktop window's name is Core's, as every desktop
    /// window's is.
    fn values(&self, set: &[PropertyId]) -> PropertyValues {
        let (name, states) = match &self.kind {
            PlainKind::Ghost {
                name, unavailable, ..
            } => {
                let mut states = StateSet::default();
                if *unavailable {
                    states.insert(State::Disabled);
                }
                (name.as_str(), states)
            }
            PlainKind::Destroyed => ("", StateSet::from_iter([State::Invisible])),
            PlainKind::Desktop => ("", StateSet::default()),
        };
        PropertyValues(
            set.iter()
                .map(|&id| PropertyAnswer {
                    id,
                    value: match id {
                        PropertyId::Name => {
                            (!name.is_empty()).then(|| PropertyValue::Text(name.to_owned()))
                        }
                        PropertyId::Role => Some(PropertyValue::Role(Role::Window)),
                        PropertyId::States => Some(PropertyValue::States(states)),
                        PropertyId::NativeWindow => Some(PropertyValue::Window(self.window)),
                        _ => None,
                    },
                })
                .collect(),
        )
    }

    /// Its answer to `asked`, as NVDA's plain `Window` gives it.
    fn answer(&self, asked: Asked<'_>) -> WireOutcome {
        match asked {
            Asked::Properties(set) => WireOutcome::Result(Reply::Properties(self.values(set))),
            // No children are reached; its parent and siblings, the
            // desktop and the neighbouring top-level windows, are not read
            // by Core, so those moves meet the edge (a recorded limit).
            Asked::Navigate => WireOutcome::Result(Reply::Navigated(None)),
            Asked::Find => WireOutcome::Result(Reply::Found(None)),
            Asked::Children => WireOutcome::Result(Reply::Children(Vec::new())),
            // No caret and no text pattern: Core reviews its flat text,
            // its name, as NVDA's `NVDAObjectTextInfo` reads `basicText`.
            Asked::Text => WireOutcome::Result(Reply::Text(TextReply::NoText)),
            // NVDA's `doAction` raises on a plain window, and the script
            // says "No action" (`NVDAObjects/__init__.py` 612 to 616;
            // `globalCommands.py` 1862 to 1903).
            Asked::Activate => WireOutcome::Refused(Refusal::Unsupported),
            // A plain window raises nothing to follow.
            Asked::Follow => WireOutcome::Result(Reply::Followed),
            Asked::Object { set, relation } => {
                WireOutcome::Result(Reply::Object(Box::new(ObjectRead {
                    object: NodeRead {
                        node: Some(self.node()),
                        window: self.node_window(),
                        values: self.values(set),
                    },
                    relation: relation.map(|query| self.relation(query)),
                    descendants: Vec::new(),
                    ancestors: Vec::new(),
                    met: None,
                    route: None,
                })))
            }
        }
    }

    /// Its relation to the nodes `query` names: it is one of them when
    /// its node is among them; it controls nothing and nothing controls
    /// it.
    fn relation(&self, query: &RelationQuery) -> RelationFound {
        let nodes = match query {
            RelationQuery::OneOf(nodes) | RelationQuery::Selection { nodes, .. } => nodes,
            RelationQuery::ControlledBy(_) => return RelationFound::Unrelated,
        };
        nodes
            .iter()
            .position(|node| *node == self.node())
            .and_then(|place| u32::try_from(place).ok())
            .map_or(RelationFound::Unrelated, RelationFound::Is)
    }
}

/// Core's plain reader: the plain object it holds, one at a time, and the
/// generations it has given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlainReader {
    /// The last generation given; each plain commit takes the next.
    generations: u64,
    /// The plain object held, `None` before the first plain commit.
    pub(crate) object: Option<PlainObject>,
}

impl PlainReader {
    /// Holds a new plain object of `window`, owned by `process` and
    /// `thread`, replacing the one held: a new generation, so a request on
    /// an older one's node is answered "not there".
    fn hold(
        &mut self,
        window: WindowHandle,
        kind: PlainKind,
        (process, thread): (Pid, u32),
    ) -> &PlainObject {
        self.generations = self.generations.saturating_add(1);
        self.object.insert(PlainObject {
            generation: self.generations,
            window,
            kind,
            process,
            thread,
        })
    }

    /// `window` was destroyed: a ghost held at it becomes
    /// [`PlainKind::Destroyed`], keeping its node, as NVDA's focus stays on
    /// the plain `Window` of the dead handle (Dickson, 2026-11-19). The
    /// desktop's plain object is never destroyed.
    pub(crate) fn window_destroyed(&mut self, window: WindowHandle) {
        if let Some(object) = self.object.as_mut()
            && object.window == window
            && matches!(object.kind, PlainKind::Ghost { .. })
        {
            object.kind = PlainKind::Destroyed;
        }
    }
}

/// A request the plain reader answers, as its kind asks.
#[derive(Clone, Copy, Debug)]
enum Asked<'a> {
    /// Properties, by set.
    Properties(&'a [PropertyId]),
    /// One navigation step.
    Navigate,
    /// A find among its children or descendants.
    Find,
    /// Its children.
    Children,
    /// A read or move in its text.
    Text,
    /// Its default action.
    Activate,
    /// A `Follow` naming it.
    Follow,
    /// The generic read of it, by set, with its relation when asked.
    Object {
        set: &'a [PropertyId],
        relation: Option<&'a RelationQuery>,
    },
}

/// The node `request` is read on, with what it asks of it; `None` for a
/// kind that names no node a reader holds. Every kind is classed here, so
/// a new kind does not build until the plain reader answers it.
fn subject(request: &Wire) -> Option<(NodeId, Asked<'_>)> {
    match request {
        Wire::Properties { node, set } => Some((*node, Asked::Properties(set))),
        Wire::Navigate { node, .. } => Some((*node, Asked::Navigate)),
        Wire::Find { node, .. } => Some((*node, Asked::Find)),
        Wire::Children { node, .. } => Some((*node, Asked::Children)),
        Wire::Text { node, .. } => Some((*node, Asked::Text)),
        Wire::Activate { node } => Some((*node, Asked::Activate)),
        Wire::Follow(follow) => follow
            .node
            .as_ref()
            .map(|followed| (followed.node, Asked::Follow)),
        Wire::ReadObject(read) => match &read.object {
            ObjectRef::Node(node) => Some((
                *node,
                Asked::Object {
                    set: &read.set,
                    relation: read.relation.as_ref(),
                },
            )),
            ObjectRef::Kept { .. } | ObjectRef::Msaa { .. } => None,
        },
        Wire::CompleteUia(_)
        | Wire::MsaaRun(_)
        | Wire::CurrentFocus { .. }
        | Wire::DumpTree { .. }
        | Wire::ShellItems { .. }
        | Wire::ShellItemPosition { .. } => None,
    }
}

/// The plain reader's answer to `request` when it names a plain node,
/// given at once from `reader`'s facts: "not there" for a node of a
/// generation no longer held. `None` for a request a reader process
/// answers.
pub(crate) fn answer(reader: &PlainReader, request: &Question) -> Option<WireOutcome> {
    let request = match request {
        Question::Legacy(request) => request,
        Question::Batch(batch) => return answer_batch(reader, batch),
    };
    let (node, asked) = subject(request)?;
    let Issuer::Plain(_) = node.issuer() else {
        return None;
    };
    Some(match &reader.object {
        Some(object) if object.node() == node => object.answer(asked),
        Some(_) | None => WireOutcome::NotThere,
    })
}

/// The plain reader's answer to `batch` when a step names a plain node,
/// every operation answered at once from `reader`'s facts as NVDA's plain
/// `Window` answers it (the design's section 7): a read and a test from
/// its facts, no relation, ancestor or descendant reached, its place
/// among the nodes Core names by its node, a `Follow` applied, no text,
/// and no action. A batch naming a plain node of a generation no longer
/// held is not there; one that does not parse is refused whole. `None`
/// for a batch naming no plain node, which a reader process answers.
fn answer_batch(reader: &PlainReader, batch: &Batch) -> Option<WireOutcome> {
    let plain = batch
        .steps
        .iter()
        .find_map(|step| match step.op.subject() {
            Some(Subject::Node(node)) if matches!(node.issuer(), Issuer::Plain(_)) => Some(*node),
            _ => None,
        })?;
    let parsed = match parse(batch.clone(), ReaderKind::Plain) {
        Ok(parsed) => parsed,
        Err(malformed) => return Some(WireOutcome::Refused(Refusal::Malformed(malformed))),
    };
    let Some(object) = reader
        .object
        .as_ref()
        .filter(|object| object.node() == plain)
    else {
        return Some(WireOutcome::NotThere);
    };
    let mut outcomes = Vec::with_capacity(parsed.steps().len());
    for step in parsed.steps() {
        let outcome = match step.gate(&outcomes) {
            Ok(Gate::Run) => object.step(&step.op, &outcomes),
            Ok(Gate::Skip) => StepOutcome::Skipped,
            // Every result here is of its own operation's kind and the
            // parse checked each condition's kind, so this cannot arise;
            // the step is answered failed, which Core judges.
            Err(KindMismatch) => StepOutcome::Failed,
        };
        outcomes.push(outcome);
    }
    Some(WireOutcome::Result(Reply::Steps(outcomes)))
}

impl PlainObject {
    /// Its node as a result carries it, with its properties of `set`.
    fn node_read(&self, set: &[PropertyId]) -> NodeRead {
        NodeRead {
            node: Some(self.node()),
            window: self.node_window(),
            values: self.values(set),
        }
    }

    /// One operation's outcome on it, the outcomes of the steps before
    /// in `earlier`. A subject is the plain node or an earlier result
    /// naming it; another node is not there.
    fn step(&self, op: &Op, earlier: &[StepOutcome]) -> StepOutcome {
        let named = match op.subject() {
            Some(Subject::Node(node)) => *node == self.node(),
            Some(subject) => subject.earlier_nodes(earlier).is_some_and(|nodes| {
                !nodes.is_empty() && nodes.iter().all(|read| read.node == Some(self.node()))
            }),
            None => true,
        };
        if !named {
            return StepOutcome::NotThere(match op.subject() {
                Some(Subject::Node(node)) => Missing::Node(*node),
                _ => Missing::Object,
            });
        }
        StepOutcome::Done(match op {
            Op::Read { set, .. } => Done::Read(self.node_read(set)),
            Op::Test { matcher, .. } => {
                let mut read = Vec::new();
                let matched = matcher
                    .evaluate::<std::convert::Infallible>(&mut |id| {
                        read.push(id);
                        Ok(self.values(&[id]).0.pop().and_then(|answer| answer.value))
                    })
                    .unwrap_or_else(|never| match never {});
                Done::Test(Tested {
                    matched,
                    node: self.node_read(&read),
                })
            }
            // No children are reached; its parent and siblings are not
            // read by Core, so those moves meet the edge (a recorded
            // limit).
            Op::Related { .. } => Done::Related(Vec::new()),
            Op::Descendants { .. } => Done::Descendants(Descended {
                nodes: Vec::new(),
                complete: true,
            }),
            Op::IsOneOf {
                equal, at_address, ..
            } => Done::IsOneOf(
                equal
                    .iter()
                    .chain(at_address)
                    .position(|node| *node == self.node())
                    .and_then(|place| u32::try_from(place).ok()),
            ),
            // A plain window raises nothing to follow.
            Op::Follow(_) => Done::Follow,
            // NVDA's `doAction` raises on a plain window, and the script
            // says "No action" (`NVDAObjects/__init__.py` 612 to 616).
            Op::Activate { .. } => return StepOutcome::Refused(StepRefusal::Unsupported),
            // No caret and no text pattern: Core reviews its flat text.
            Op::Caret { .. }
            | Op::ReadText { .. }
            | Op::ReadRange { .. }
            | Op::CompareSelection { .. }
            | Op::Select { .. }
            | Op::Locate { .. }
            | Op::MoveBySentence { .. }
            | Op::WaitForCaret { .. } => return StepOutcome::Refused(StepRefusal::NoText),
            // Not run by the plain reader, whose parse refuses both.
            Op::Ancestors { .. } | Op::FocusRead { .. } => {
                return StepOutcome::Refused(StepRefusal::Unsupported);
            }
        })
    }
}

/// Which read of the focus Core made itself the shell answered with what
/// is in front: the rows of the design's decision table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HungRead {
    /// The check after a menu or popup end, NVDA's fake focus.
    MenuCheck,
    /// The read at startup.
    Startup,
    /// The read after the return from another input desktop.
    Returned,
    /// The check after the task switcher ends.
    SwitchCheck,
    /// The re-take after a reader's replacement.
    Retake,
    /// The read after a window Core watched as hung answers again.
    Recovered,
    /// A UI Automation focus's read afresh, of its application's current
    /// focus, when the ancestor its listener read met left the committed
    /// focus's line: NVDA reads the event's own element here, never the
    /// window in front, so it is left as it was.
    Afresh,
}

impl HungRead {
    /// Whether NVDA reads the focus here itself, through `objectWithFocus`,
    /// and so makes a plain window of a ghost in front. The switcher's end
    /// is left as it was over a ghost (a recorded difference), and NVDA has
    /// no re-take or recovery read.
    const fn reads_like_nvda(self) -> bool {
        matches!(self, Self::MenuCheck | Self::Startup | Self::Returned)
    }

    /// Whether NVDA reads the focus here itself and, with no window in
    /// front, focuses the desktop: every read of `reads_like_nvda`, and the
    /// switcher's end, whose fake focus is the menu end's
    /// (`IAccessibleHandler/__init__.py` 989 to 1002; Dickson,
    /// 2026-11-19).
    const fn reads_the_desktop(self) -> bool {
        self.reads_like_nvda() || matches!(self, Self::SwitchCheck)
    }
}

/// The read `read` of the focus, naming the window `window`, was answered
/// with what is in front, `front`: its entry decides (the design's decision
/// table). The desktop window, for a read NVDA makes itself, the
/// switcher's check among them, is committed as the plain desktop, spoken,
/// with nothing watched and no sound. A ghost, for a read NVDA makes itself
/// but the switcher's check, is committed as the plain object, spoken, its
/// hung window watched with no sound; with nothing hung in front such a
/// read says nothing and watches nothing, the window in front's own tells
/// speaking it. Every other answer, and every other read, plays the
/// not-responding sound and watches the window, while its application
/// holds attention, and clears Core's caret baseline as a failed read
/// does.
pub(crate) fn decide(
    cx: &mut Cx<Entries>,
    read: HungRead,
    window: RequestWindow,
    front: &Front,
    (trace_id, fence): (verbatim_model::TraceId, Fence),
) {
    match (front, read.reads_like_nvda(), read.reads_the_desktop()) {
        (Front::Desktop(desktop), _, true) => {
            let executed = commit_desktop(&mut cx.facts_mut(), desktop, (trace_id, fence));
            executed.deliver(cx, fence);
        }
        (Front::Ghost(ghost), true, _) => {
            let executed = commit_ghost(&mut cx.facts_mut(), ghost, (trace_id, fence));
            cx.watch_if_attended(&stood_for(ghost, window));
            executed.deliver(cx, fence);
        }
        (Front::Responsive | Front::Gone, true, _) => cx.facts_mut().focus_read_failed(),
        (Front::Desktop(_), _, false)
        | (Front::Ghost(_) | Front::Hung | Front::Responsive | Front::Gone, false, _)
        | (Front::Hung, true, _) => {
            cx.hung_if_attended(&window);
            cx.facts_mut().focus_read_failed();
        }
    }
}

/// The hung window `ghost` stands for, which is watched: `asked`, the window
/// the read named, when it is that window; otherwise the window of another
/// application whose ghost came in front, as a click can bring it, named
/// by its own application and thread, so it, not the asked application,
/// is watched and read again when it answers.
fn stood_for(ghost: &GhostFront, asked: RequestWindow) -> RequestWindow {
    if ghost.stands_for == asked.handle {
        return asked;
    }
    RequestWindow {
        handle: ghost.stands_for,
        top_level: ghost.stands_for,
        root_owner: ghost.stands_for,
        process: ghost.process,
        thread: ghost.thread,
        by_class: None,
    }
}

/// Commits the plain object of `ghost` as the focus, named by the
/// application of the hung window it stands for.
fn commit_ghost(
    state: &mut SrState,
    ghost: &GhostFront,
    trace: (verbatim_model::TraceId, Fence),
) -> Executed {
    let kind = PlainKind::Ghost {
        stands_for: ghost.stands_for,
        name: ghost.name.clone(),
        unavailable: ghost.unavailable,
    };
    let object = state
        .plain
        .hold(ghost.ghost, kind, (ghost.process, ghost.thread))
        .clone();
    commit(state, &object, trace)
}

/// Commits the plain desktop of `desktop` as the focus, as NVDA's fake
/// focus focuses its `Desktop` with no window in front.
fn commit_desktop(
    state: &mut SrState,
    desktop: &DesktopFront,
    trace: (verbatim_model::TraceId, Fence),
) -> Executed {
    let object = state
        .plain
        .hold(
            desktop.desktop,
            PlainKind::Desktop,
            (desktop.process, desktop.thread),
        )
        .clone();
    commit(state, &object, trace)
}

/// Commits `object`, just held, as the focus, executed through the same
/// commit as any focus. Its read is one node, whose window facts name its
/// window as its top-level window and root owner, with no ancestors below
/// the desktop, no text, and itself as the foreground object.
fn commit(
    state: &mut SrState,
    object: &PlainObject,
    (trace_id, fence): (verbatim_model::TraceId, Fence),
) -> Executed {
    let focus = FocusNode {
        read: NodeRead {
            node: Some(object.node()),
            window: object.node_window(),
            values: object.values(&[
                PropertyId::Name,
                PropertyId::Role,
                PropertyId::States,
                PropertyId::NativeWindow,
            ]),
        },
        runtime_id: None,
    };
    let read = FocusRead {
        focus: focus.clone(),
        msaa: None,
        ancestors: Vec::new(),
        met: None,
        foreground: Some(focus),
        value_from_selection: false,
        runtime_id_reused: false,
        caret: FocusCaret::NoText,
    };
    let own_window = RequestWindow {
        handle: object.window,
        top_level: object.window,
        root_owner: object.window,
        process: object.process,
        thread: object.thread,
        by_class: None,
    };
    let mut origin = Origin::of_read(trace_id, own_window, fence);
    origin.window = Some(WindowFacts {
        top_level: object.window,
        root_owner: object.window,
        topmost: false,
        under_active_window: None,
        in_foreground: true,
        foreground_window: object.window,
        class: String::new(),
        application: String::new(),
        alt_held: false,
        foreground_is_shell: false,
    });
    execute(state, &origin, &read, &Known::default(), ReadBy::Plain)
}

#[cfg(test)]
#[path = "plain_tests.rs"]
mod tests;
