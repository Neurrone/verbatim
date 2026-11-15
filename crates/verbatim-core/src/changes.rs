//! Changes on objects as event-queue entries (`docs/design/readers-tell-core-asks-build1.md`,
//! "Changes on objects"): MSAA name, value, state and description
//! changes, the selection kinds, and UI Automation property changes, each
//! judged at its turn by the state-change rule against the committed focus
//! and its ancestors.
//!
//! An MSAA change is dropped unless its window is the window of the
//! committed focus or of one of its ancestors; otherwise Core asks the
//! outpost that holds those nodes, naming the event's window for the hang
//! check, whether the event's object is one of them, and reads the changed
//! property. An object outside them answers "not there" and nothing is
//! said. A plain MSAA selection asks the same outpost for the item, with
//! its relation to the committed focus read live: an item the focus
//! controls, or inside one, is spoken as a focus; any other is a state
//! change of the selected object.
//!
//! A UI Automation property change on the committed focus's runtime id,
//! or an ancestor's, runs on that node with no read, speaking the value
//! its tell carries.
//!
//! What is spoken comes from today's announcement code
//! ([`crate::reduce::property_changed`]), which compares each property with
//! the one last spoken of the node.

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue, RuntimeId};
use verbatim_model::request::{
    FocusNode, MsaaIds, NodeRead, ObjectRead, ObjectRef, Purpose, ReadObject, RelationFound,
    RelationQuery, Reply, Request as Wire,
};
use verbatim_model::tell::{Event, Payload};
use verbatim_model::{Backend, NodeId, OutpostId, PropertyChange, Role, State, StateSet, TraceId};

use crate::entries::{Entries, window_facts};
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, Request, RequestWindow};
use crate::focus::acceptance;
use crate::focus::backend::{Judged, Judgement};
use crate::focus::versions::{Kept, ReadBy};
use crate::state::SrState;

/// An MSAA change tell, parsed: the ids its event named and the window its
/// request names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MsaaChange {
    /// The tell.
    pub(crate) tell: Event,
    /// The ids its event named.
    pub(crate) ids: MsaaIds,
    /// The window its request names: the event's.
    pub(crate) window: RequestWindow,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: crate::keys::entry::Fence,
}

/// A UI Automation property change tell, parsed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UiaChange {
    /// The tell.
    pub(crate) tell: Event,
    /// The element's runtime id.
    pub(crate) runtime_id: RuntimeId,
    /// The property.
    pub(crate) property: PropertyId,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: crate::keys::entry::Fence,
    /// Its new value, as the tell carries it.
    pub(crate) value: Option<PropertyValue>,
}

/// Whether `kind` is an MSAA change this module takes.
pub(crate) fn is_msaa_change(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::NameChange
            | EventKind::ValueChange
            | EventKind::StateChange
            | EventKind::DescriptionChange
            | EventKind::Selection
            | EventKind::SelectionAdd
            | EventKind::SelectionRemove
            | EventKind::SelectionWithin
    )
}

/// What an MSAA change kind changes: the selection add, remove and within
/// kinds are state changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Changed {
    Name,
    Value,
    States,
    Description,
}

impl Changed {
    fn of(kind: EventKind) -> Option<Self> {
        match kind {
            EventKind::NameChange => Some(Self::Name),
            EventKind::ValueChange => Some(Self::Value),
            EventKind::StateChange
            | EventKind::SelectionAdd
            | EventKind::SelectionRemove
            | EventKind::SelectionWithin => Some(Self::States),
            EventKind::DescriptionChange => Some(Self::Description),
            _ => None,
        }
    }

    fn property(self) -> PropertyId {
        match self {
            Self::Name => PropertyId::Name,
            Self::Value => PropertyId::Value,
            Self::States => PropertyId::States,
            Self::Description => PropertyId::Description,
        }
    }

    /// `value`, the property this change read, as today's announcement
    /// code takes it; `None` for a value of the wrong type, a reader's
    /// fault.
    fn change(self, value: Option<&PropertyValue>) -> Option<PropertyChange> {
        let text = || match value {
            Some(PropertyValue::Text(text)) => Some(Some(text.clone())),
            None => Some(None),
            Some(_) => None,
        };
        Some(match self {
            Self::Name => PropertyChange::Name(text()?),
            Self::Value => PropertyChange::Value(text()?),
            Self::Description => PropertyChange::Description(text()?),
            Self::States => match value {
                Some(PropertyValue::States(states)) => PropertyChange::States(*states),
                None => PropertyChange::States(StateSet::default()),
                Some(_) => return None,
            },
        })
    }
}

/// The nodes a change of `changed` can be spoken for, which the request
/// names: the committed focus; for a value change its parent too, a
/// slider that focuses its thumb speaking its own; for a state change
/// every ancestor but the simple children.
fn candidates(kept: &Kept, changed: Option<Changed>) -> Vec<NodeId> {
    let ancestors = kept.ancestors.iter();
    std::iter::once(kept.node)
        .chain(match changed {
            Some(Changed::Name | Changed::Description) => Vec::new(),
            Some(Changed::Value) => ancestors.take(1).map(|ancestor| ancestor.node).collect(),
            Some(Changed::States) | None => ancestors
                .filter(|ancestor| !ancestor.simple_child)
                .map(|ancestor| ancestor.node)
                .collect(),
        })
        .collect()
}

/// The generic read of an MSAA change's object, asked of the outpost that
/// holds the committed focus's nodes.
#[derive(Debug)]
pub(crate) struct ChangeRead {
    /// The application whose outpost holds the nodes.
    pub(crate) process: verbatim_model::Pid,
    /// The event's window.
    pub(crate) window: RequestWindow,
    /// The read.
    pub(crate) read: ReadObject,
}

impl Request for ChangeRead {
    type Reply = Box<ObjectRead>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::ReadObject(self.read.clone())
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Object(read) => Some(read),
            _ => None,
        }
    }
}

/// A node's children, read to count them.
#[derive(Debug)]
struct ChildrenRead {
    process: verbatim_model::Pid,
    window: RequestWindow,
    node: NodeId,
}

impl Request for ChildrenRead {
    type Reply = Vec<NodeRead>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::Children {
            node: self.node,
            set: Vec::new(),
        }
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Children(children) => Some(children),
            _ => None,
        }
    }
}

/// The committed focus, when it is a reader's focus held by `generation`
/// of its outpost: what Core keeps of it.
fn held(facts: &SrState, generation: Option<OutpostId>) -> Option<Kept> {
    facts
        .versions
        .committed_read()
        .filter(|kept| Some(kept.outpost) == generation)
        .cloned()
}

/// Whether `change` passes acceptance and the backend verdict on its
/// tell's facts.
fn passes(cx: &Cx<Entries>, change: &MsaaChange) -> bool {
    let background = cx.facts().settings.report_background_progress_bars;
    if acceptance::accept(change.tell.kind, window_facts(&change.tell), background).is_err() {
        return false;
    }
    cx.backend().judge(
        Judged::Msaa,
        &change.window.judged(),
        change.tell.at_us,
        cx.now_us(),
    ) != Judgement::Drop
}

/// The function of an MSAA change entry.
pub(crate) async fn msaa(mut cx: Cx<Entries>, change: MsaaChange) {
    if !passes(&cx, &change) {
        return;
    }
    let Some(process) = cx
        .facts()
        .versions
        .committed_read()
        .map(|kept| kept.window.process)
    else {
        return;
    };
    let generation = cx
        .instance(Reader::Outpost(process))
        .map(|instance| OutpostId(instance.generation));
    let Some(kept) = held(&cx.facts(), generation) else {
        return;
    };
    let trace_id = change.tell.trace_id;
    let changed = Changed::of(change.tell.kind);
    // A change outside the windows of the committed focus and its
    // ancestors cannot be on them; a plain selection's item may be in
    // another window, inside an object the focus controls.
    if changed.is_some() && !kept.place.holds_window(change.ids.window) {
        return;
    }
    let nodes = candidates(&kept, changed);
    let (set, relation) = match changed {
        Some(changed) => (
            vec![changed.property()],
            RelationQuery::OneOf(nodes.clone()),
        ),
        None => (
            crate::focus::FOCUS_SET.to_vec(),
            RelationQuery::Selection {
                focus: kept.node,
                nodes: nodes.clone(),
            },
        ),
    };
    let request = ChangeRead {
        process,
        window: change.window,
        read: ReadObject {
            object: ObjectRef::Msaa {
                window: change.ids.window,
                object: change.ids.object,
                child: change.ids.child,
            },
            set,
            relation: Some(relation),
            descendants: false,
            walk: None,
            purpose: Purpose::Other,
        },
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(read) = cx.ask(request, due_us).await else {
        return;
    };
    match (changed, read.relation) {
        (_, Some(RelationFound::Is(place))) => {
            let Some(node) = usize::try_from(place)
                .ok()
                .and_then(|place| nodes.get(place))
                .copied()
            else {
                return;
            };
            let changed = changed.unwrap_or(Changed::States);
            let value = read
                .object
                .values
                .answer(changed.property())
                .and_then(|answer| answer.value.as_ref());
            let Some(property) = changed.change(value) else {
                return;
            };
            let child_count = match &property {
                PropertyChange::States(states)
                    if newly_expanded_tree_item(&cx.facts(), node, *states) =>
                {
                    let request = ChildrenRead {
                        process,
                        window: change.window,
                        node,
                    };
                    match cx.ask(request, due_us).await {
                        Outcome::Result(children) => u32::try_from(children.len()).ok(),
                        _ => None,
                    }
                }
                _ => None,
            };
            speak_change(&mut cx, trace_id, node, &property, child_count);
        }
        (None, Some(RelationFound::Controlled | RelationFound::InsideControlled)) => {
            controlled_selection(&mut cx, trace_id, kept.node, read.object);
        }
        _ => {}
    }
}

/// A plain selection's item, `item`, is an object the committed focus
/// `focus` controls or inside one: it is spoken as a focus, the navigator
/// moved to it.
fn controlled_selection(cx: &mut Cx<Entries>, trace_id: TraceId, focus: NodeId, item: NodeRead) {
    let window = RequestWindow::of_node(&item.window);
    let item = FocusNode {
        read: item,
        runtime_id: None,
    };
    let Some(item) = crate::focus::execute::snapshot(&item, Backend::Msaa) else {
        return;
    };
    let said = {
        let mut facts = cx.facts_mut();
        let said = crate::reduce::reduce_controlled_selection(&mut facts, trace_id, focus, &item);
        // The navigator moved to the item names the window its read
        // found, so the navigator's commands can ask it.
        if let Some(navigator) = facts
            .navigator
            .as_mut()
            .filter(|navigator| navigator.object.id == item.id)
        {
            navigator.window = Some(window);
        }
        said
    };
    for said in said {
        cx.emit(said);
    }
}

/// Whether a state change of `node` to `states` newly expands the
/// committed focus, a tree item, judged against the states last spoken of
/// it: Core then asks its number of children.
fn newly_expanded_tree_item(facts: &SrState, node: NodeId, states: StateSet) -> bool {
    facts.focus.object().is_some_and(|focus| {
        focus.snapshot.id == node
            && focus.snapshot.role == Role::TreeItem
            && states.contains(State::Expanded)
            && focus
                .spoken_states(node)
                .is_some_and(|before| !before.contains(State::Expanded))
    })
}

/// Speaks a change of `node` by today's announcement code.
fn speak_change(
    cx: &mut Cx<Entries>,
    trace_id: TraceId,
    node: NodeId,
    change: &PropertyChange,
    child_count: Option<u32>,
) {
    let said =
        crate::reduce::property_changed(&mut cx.facts_mut(), trace_id, node, change, child_count);
    for said in said {
        cx.emit(said);
    }
}

/// The node of the committed focus, or of one of its ancestors, whose
/// runtime id is `runtime_id`.
fn node_of(kept: &Kept, runtime_id: &RuntimeId) -> Option<NodeId> {
    if matches!(&kept.read_by, ReadBy::Uia { runtime_id: focus, .. } if focus == runtime_id) {
        return Some(kept.node);
    }
    kept.ancestors
        .iter()
        .find(|ancestor| ancestor.runtime_id.as_ref() == Some(runtime_id))
        .map(|ancestor| ancestor.node)
}

/// What a UI Automation property change carries, as today's announcement
/// code takes it; `None` for a property this build does not speak from
/// the tell.
fn uia_change(property: PropertyId, value: Option<&PropertyValue>) -> Option<PropertyChange> {
    let changed = match property {
        PropertyId::Name => Changed::Name,
        PropertyId::Value => Changed::Value,
        PropertyId::States => Changed::States,
        PropertyId::Description => Changed::Description,
        _ => return None,
    };
    changed.change(value)
}

/// Judges a UI Automation property change on the committed focus, or on
/// one of its ancestors, from the value its tell carries, with no read.
pub(crate) fn uia_changed(facts: &mut SrState, change: &UiaChange) -> Vec<verbatim_model::Effect> {
    let Some(kept) = facts.versions.committed_read() else {
        return Vec::new();
    };
    let Some(node) = node_of(kept, &change.runtime_id) else {
        return Vec::new();
    };
    let Some(property) = uia_change(change.property, change.value.as_ref()) else {
        return Vec::new();
    };
    crate::reduce::property_changed(facts, change.tell.trace_id, node, &property, None)
}

/// The function of a UI Automation property change entry, which asks
/// nothing.
pub(crate) fn uia(cx: &mut Cx<Entries>, change: &UiaChange) {
    let said = uia_changed(&mut cx.facts_mut(), change);
    for said in said {
        cx.emit(said);
    }
}

/// The UI Automation property change `tell` names, parsed; `None` for a
/// tell with no property change.
pub(crate) fn uia_of_tell(tell: Event, runtime_id: RuntimeId) -> Option<UiaChange> {
    let Payload::PropertyChanged { property, value } = &tell.payload else {
        return None;
    };
    let (property, value) = (*property, value.clone());
    Some(UiaChange {
        tell,
        runtime_id,
        property,
        value,
        fence: crate::keys::entry::Fence::default(),
    })
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
