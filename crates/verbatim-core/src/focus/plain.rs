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
//! 2026-11-19). Its facts are read once, at the decision, and its record
//! ends with the ghost's destruction.

use serde::{Deserialize, Serialize};
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    FocusCaret, FocusNode, FocusRead, Front, GhostFront, NodeRead, NodeWindow, ObjectClass,
    ObjectRead, ObjectRef, Outcome as WireOutcome, Refusal, RelationFound, RelationQuery, Reply,
    Request as Wire,
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

/// A plain window Core's plain reader holds: a ghost window found in
/// front, with the facts the shell read of it at the decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlainObject {
    /// Its generation: its node's issuer, never reused.
    pub(crate) generation: u64,
    /// The ghost window, which owns the record: its destruction ends it.
    pub(crate) ghost: WindowHandle,
    /// The hung window it stands for.
    pub(crate) stands_for: WindowHandle,
    /// Its window text.
    pub(crate) name: String,
    /// Whether it is disabled.
    pub(crate) unavailable: bool,
    /// The application of the hung window it stands for, never the
    /// ghost's own (the desktop window manager's), so that no rule naming
    /// the process starts an outpost for the ghost's.
    pub(crate) process: Pid,
    /// The hung window's thread.
    pub(crate) thread: u32,
}

impl PlainObject {
    /// Its node.
    pub(crate) fn node(&self) -> NodeId {
        NodeId::plain(self.generation, WINDOW_NUMBER)
    }

    /// Its window as an answer names a node's nearest window: the ghost,
    /// a top-level window of its own, named by the hung application's
    /// process.
    fn node_window(&self) -> NodeWindow {
        NodeWindow {
            handle: self.ghost,
            top_level: self.ghost,
            root_owner: self.ghost,
            process: self.process,
            thread: self.thread,
            desktop: false,
        }
    }

    /// Its properties of `set`, each once: its name, the role window, the
    /// unavailable state when disabled, its own window; nothing else
    /// (`NVDAObjects/window/__init__.py` 299 to 306; `winUser.py` 652 to
    /// 655).
    fn values(&self, set: &[PropertyId]) -> PropertyValues {
        PropertyValues(
            set.iter()
                .map(|&id| PropertyAnswer {
                    id,
                    value: match id {
                        PropertyId::Name => {
                            (!self.name.is_empty()).then(|| PropertyValue::Text(self.name.clone()))
                        }
                        PropertyId::Role => Some(PropertyValue::Role(Role::Window)),
                        PropertyId::States => {
                            let mut states = StateSet::default();
                            if self.unavailable {
                                states.insert(State::Disabled);
                            }
                            Some(PropertyValue::States(states))
                        }
                        PropertyId::NativeWindow => Some(PropertyValue::Window(self.ghost)),
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
    /// The plain object held, `None` before the first plain commit and
    /// after its ghost's destruction.
    pub(crate) object: Option<PlainObject>,
}

impl PlainReader {
    /// Holds a new plain object of `ghost`, named by the application of the
    /// hung window it stands for, replacing the one held: a new generation,
    /// so a request on an older one's node is answered "not there".
    fn hold(&mut self, ghost: &GhostFront) -> &PlainObject {
        self.generations = self.generations.saturating_add(1);
        self.object.insert(PlainObject {
            generation: self.generations,
            ghost: ghost.ghost,
            stands_for: ghost.stands_for,
            name: ghost.name.clone(),
            unavailable: ghost.unavailable,
            process: ghost.process,
            thread: ghost.thread,
        })
    }

    /// `window` was destroyed: the record ends when it is the ghost's.
    pub(crate) fn window_destroyed(&mut self, window: WindowHandle) {
        if self
            .object
            .as_ref()
            .is_some_and(|object| object.ghost == window)
        {
            self.object = None;
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
pub(crate) fn answer(reader: &PlainReader, request: &Wire) -> Option<WireOutcome> {
    let (node, asked) = subject(request)?;
    let Issuer::Plain(_) = node.issuer() else {
        return None;
    };
    Some(match &reader.object {
        Some(object) if object.node() == node => object.answer(asked),
        Some(_) | None => WireOutcome::NotThere,
    })
}

/// Which read of the focus Core made itself was answered hung: the rows
/// of the design's decision table.
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
    /// Whether NVDA reads the focus here itself, and so makes a plain
    /// window of a ghost in front. The switcher's end is left as it was
    /// (a recorded difference), and NVDA has no re-take or recovery read.
    const fn reads_like_nvda(self) -> bool {
        matches!(self, Self::MenuCheck | Self::Startup | Self::Returned)
    }
}

/// The read `read` of the focus, naming the hung application's window
/// `window`, was answered with what is in front, `front`: its entry
/// decides (the design's decision table). A ghost, for a read NVDA makes
/// itself, is committed as the plain object, spoken, its hung window
/// watched with no sound; with nothing hung in front such a read says
/// nothing and watches nothing, the window in front's own tells speaking
/// it. Every other answer, and every other read, plays the not-responding
/// sound and watches the window, while its application holds attention,
/// and clears Core's caret baseline as a failed read does.
pub(crate) fn decide(
    cx: &mut Cx<Entries>,
    read: HungRead,
    window: RequestWindow,
    front: &Front,
    (trace_id, fence): (verbatim_model::TraceId, Fence),
) {
    match (front, read.reads_like_nvda()) {
        (Front::Ghost(ghost), true) => {
            let executed = commit(&mut cx.facts_mut(), ghost, (trace_id, fence));
            cx.watch_if_attended(&stood_for(ghost, window));
            executed.deliver(cx);
        }
        (Front::Responsive | Front::Nothing, true) => cx.facts_mut().focus_read_failed(),
        (Front::Ghost(_) | Front::Hung | Front::Responsive | Front::Nothing, false)
        | (Front::Hung, true) => {
            cx.sound_if_attended(&window);
            cx.watch_if_attended(&window);
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
/// application of the hung window it stands for: a new plain object is
/// held, and executed through the same commit as any focus. Its read is
/// one node, whose window facts name the ghost as its top-level window
/// and root owner, with no ancestors below the desktop, no text, and
/// itself as the foreground object.
fn commit(
    state: &mut SrState,
    ghost: &GhostFront,
    (trace_id, fence): (verbatim_model::TraceId, Fence),
) -> Executed {
    let object = state.plain.hold(ghost).clone();
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
        class: ObjectClass::Plain,
        runtime_id_reused: false,
        caret: FocusCaret::NoText,
    };
    let ghost_window = RequestWindow {
        handle: object.ghost,
        top_level: object.ghost,
        root_owner: object.ghost,
        process: object.process,
        thread: object.thread,
        by_class: None,
    };
    let mut origin = Origin::of_read(trace_id, ghost_window, fence);
    origin.window = Some(WindowFacts {
        top_level: object.ghost,
        root_owner: object.ghost,
        topmost: false,
        under_active_window: None,
        in_foreground: true,
        foreground_window: object.ghost,
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
