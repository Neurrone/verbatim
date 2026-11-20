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
//! A UI Automation property change, told by the outpost following the
//! committed focus, on the committed focus's runtime id or an ancestor's,
//! runs on that node with no read, speaking the value its tell carries;
//! but a description (help text) change and a suggestions (controller-for)
//! change, which Core asks one read for. Its state and description speech
//! is given only when its element was on the focus line as its tell
//! joined the queue ([`AtArrival`]), as NVDA judges a state change by its
//! focus when the event was delivered and its focus line when it is
//! handled (`docs/parity/changes-and-selection.md`, "State changes and the
//! focus line").
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
    /// Whether its element was on the focus line when it joined the
    /// queue, set then.
    pub(crate) at_arrival: AtArrival,
}

/// Whether the element of a UI Automation change or selection was on the
/// focus line when its tell joined the queue, NVDA's focus when the event
/// was delivered (`docs/parity/changes-and-selection.md`, "State changes
/// and the focus line"). For an outpost's change the line is the committed
/// focus, a kept ancestor of it, or the UI Automation focus whose
/// completion is out; for the listener's selection it is the committed
/// focus alone. Its state and description speech is given only for an
/// element on the line at arrival that is, at its turn, still the
/// committed focus or a kept ancestor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AtArrival {
    /// On the line.
    OnLine,
    /// Not on the line; a tell's value from its parse until it joins.
    #[default]
    OffLine,
}

impl AtArrival {
    fn of(on_line: bool) -> Self {
        if on_line { Self::OnLine } else { Self::OffLine }
    }
}

/// The runtime id of the UI Automation focus in flight: the front, a UI
/// Automation focus whose completion is waiting or out. The outpost
/// registers its element as the completion reaches it, so a change it
/// tells of the element was raised after that, when NVDA hears it too.
fn in_flight<'a>(
    front: Option<(&'a crate::entries::Entry, Option<&Wire>)>,
) -> Option<&'a RuntimeId> {
    match front? {
        (crate::entries::Entry::UiaFocus(focus), Some(Wire::CompleteUia(complete)))
            if complete.completes == verbatim_model::request::CompletionFor::Focus =>
        {
            Some(&focus.runtime_id)
        }
        _ => None,
    }
}

/// Sets `change`'s arrival fact as it joins the queue behind `front`: on
/// the line when its element is the committed focus, a kept ancestor or
/// the focus in flight.
pub(crate) fn arrived(
    facts: &SrState,
    front: Option<(&crate::entries::Entry, Option<&Wire>)>,
    change: &mut UiaChange,
) {
    let committed = facts
        .versions
        .committed_read()
        .is_some_and(|kept| node_of(kept, &change.runtime_id).is_some());
    change.at_arrival = AtArrival::of(committed || in_flight(front) == Some(&change.runtime_id));
}

/// The arrival fact of the listener's selection of the element whose
/// runtime id is `runtime_id`: on the line when it is the committed focus.
pub(crate) fn selection_arrived(facts: &SrState, runtime_id: &RuntimeId) -> AtArrival {
    AtArrival::of(matches!(
        facts.versions.committed_read().map(|kept| &kept.read_by),
        Some(ReadBy::Uia { runtime_id: focus, .. }) if focus == runtime_id
    ))
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

/// How many children a node holds, read as one property
/// ([`PropertyId::ChildCount`]), with no read of a child.
#[derive(Debug)]
struct ChildCountRead {
    process: verbatim_model::Pid,
    window: RequestWindow,
    node: NodeId,
}

impl Request for ChildCountRead {
    type Reply = Option<u32>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::Properties {
            node: self.node,
            set: vec![PropertyId::ChildCount],
        }
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Properties(values) => Some(
                match values
                    .answer(PropertyId::ChildCount)
                    .and_then(|answer| answer.value.as_ref())
                {
                    Some(PropertyValue::Count(count)) => Some(*count),
                    _ => None,
                },
            ),
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
        .filter(|kept| generation.is_some_and(|generation| kept.node.outpost() == Some(generation)))
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

/// A value change of the committed focus, editable text with automatic
/// selection detection, sets its text-changed flag and says nothing
/// (Table 17, column G); `true` when `change` is one. One a caret tell's
/// answer took as counted no longer waits.
fn sets_text_changed(facts: &mut SrState, change: &MsaaChange) -> bool {
    let crate::focus::versions::Committed::Read(committed) = &mut facts.versions.committed else {
        return false;
    };
    if !crate::caret_tell::sets_text_changed(committed, change) {
        return false;
    }
    crate::text_facts::value_changed(committed);
    true
}

/// A change of the committed focus's text, a value change of an edit
/// control's client read through its messages, is the control plane's
/// text change, as the outpost reported it before Core judged its changes.
pub(crate) fn observe_text_change(cx: &mut Cx<Entries>, change: &MsaaChange) {
    let node = cx
        .facts()
        .versions
        .committed_read()
        .filter(|kept| crate::caret_tell::edit_text_change(kept, change))
        .map(|kept| kept.node);
    if let Some(node_id) = node {
        cx.emit(verbatim_model::Observed::effect(
            change.tell.trace_id,
            (change.tell.process, Backend::Msaa),
            None,
            verbatim_model::NormalizedEvent::TextChanged { node_id },
        ));
    }
}

/// Whether `change` goes on to be judged: it passes, and is no value
/// change that sets the text-changed flag. A change of the committed
/// field's text that passes is the control plane's text change.
fn judged(cx: &mut Cx<Entries>, change: &MsaaChange) -> bool {
    if !passes(cx, change) {
        return false;
    }
    observe_text_change(cx, change);
    !sets_text_changed(&mut cx.facts_mut(), change)
}

/// The function of an MSAA change entry.
pub(crate) async fn msaa(mut cx: Cx<Entries>, change: MsaaChange) {
    if !judged(&mut cx, &change) {
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
        .map(verbatim_model::request::Instance::outpost_id);
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
    // An MSAA object controls nothing, so a plain selection's item is
    // only ever one of the nodes, whose states alone are judged; neither
    // read goes further for an object that is none of them.
    let (set, relation) = match changed {
        Some(changed) => (
            vec![changed.property()],
            RelationQuery::OneOf(nodes.clone()),
        ),
        None => (
            vec![PropertyId::States],
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
            purpose: Purpose::Change,
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
            // The property asked must be answered, if only with no value:
            // a read without its answer is a reader's fault, refused
            // rather than taken as the property having no value.
            let Some(answer) = read.object.values.answer(changed.property()) else {
                return;
            };
            let Some(property) = changed.change(answer.value.as_ref()) else {
                return;
            };
            let child_count = match &property {
                PropertyChange::States(states)
                    if newly_expanded_tree_item(&cx.facts(), node, *states) =>
                {
                    let request = ChildCountRead {
                        process,
                        window: change.window,
                        node,
                    };
                    match cx.ask(request, due_us).await {
                        Outcome::Result(count) => count,
                        _ => None,
                    }
                }
                _ => None,
            };
            speak_change(&mut cx, trace_id, node, &property, child_count);
        }
        (None, Some(RelationFound::Controlled | RelationFound::InsideControlled)) => {
            controlled_selection(&mut cx, trace_id, kept.node, (read.object, Backend::Msaa));
        }
        _ => {}
    }
}

/// A plain selection's item, `item`, is an object the committed focus
/// `focus` controls or inside one: it is spoken as a focus, the navigator
/// moved to it.
pub(crate) fn controlled_selection(
    cx: &mut Cx<Entries>,
    trace_id: TraceId,
    focus: NodeId,
    (item, backend): (NodeRead, Backend),
) {
    let window = RequestWindow::of_node(&item.window);
    let item = FocusNode {
        read: item,
        runtime_id: None,
    };
    let Some(item) = crate::focus::execute::snapshot(&item, backend) else {
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
pub(crate) fn speak_change(
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
pub(crate) fn node_of(kept: &Kept, runtime_id: &RuntimeId) -> Option<NodeId> {
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
        _ => return None,
    };
    changed.change(value)
}

/// The node of the committed focus, or of one of its ancestors, that a UI
/// Automation property change `change` is on, with what Core keeps of the
/// committed focus; `None` for a change on anything else.
fn uia_target(facts: &SrState, change: &UiaChange) -> Option<(Kept, NodeId)> {
    let kept = facts.versions.committed_read()?;
    let node = node_of(kept, &change.runtime_id)?;
    Some((kept.clone(), node))
}

/// Judges a UI Automation property change on the committed focus, or on
/// one of its ancestors, from the value its tell carries, with no read: a
/// name, value or state change. A state change is spoken only when its
/// element was on the line at its arrival, and, on the focus, not when the
/// focus reported the focused state and the change's states, taken from
/// the event's cache as the application raised it, lack it: the focus had
/// moved before the change was raised (`docs/parity/changes-and-selection.md`,
/// "State changes after the focus has left"). A description or
/// suggestions change is asked ([`uia_entry`]) and says nothing here.
pub(crate) fn uia_changed(facts: &mut SrState, change: &UiaChange) -> Vec<verbatim_model::Effect> {
    let Some((_, node)) = uia_target(facts, change) else {
        return Vec::new();
    };
    // A value change of an element with a text pattern says nothing; only
    // Word's and a terminal's text changes set the text-changed flag.
    // NVDA: `NVDAObjects/UIA/__init__.py` 2514 to 2516.
    if change.property == PropertyId::Value
        && matches!(
            change
                .tell
                .cached
                .answer(PropertyId::HasTextPattern)
                .and_then(|answer| answer.value.as_ref()),
            Some(PropertyValue::Flag(true))
        )
    {
        return Vec::new();
    }
    let Some(property) = uia_change(change.property, change.value.as_ref()) else {
        return Vec::new();
    };
    match property {
        PropertyChange::States(states) => {
            if change.at_arrival == AtArrival::OffLine {
                return Vec::new();
            }
            crate::reduce::outpost_states_changed(facts, change.tell.trace_id, node, states)
        }
        property => {
            crate::reduce::property_changed(facts, change.tell.trace_id, node, &property, None)
        }
    }
}

/// Whether the UI Automation property change `change` passes acceptance
/// on its tell's facts: the listener's, or those of the followed node's
/// window an outpost gathered, as NVDA's property-change handler applies
/// it to every event (`docs/parity/focus-events.md`, "Event acceptance").
/// A value change, the outpost's tell of a value or range value change,
/// is judged as a value change, which background progress bars reported
/// accepts from any window, as NVDA maps both properties to its value
/// change event; every other property is judged as a property change.
fn accepted(facts: &SrState, change: &UiaChange) -> bool {
    let (kind, background) = if change.property == PropertyId::Value {
        (
            EventKind::ValueChange,
            facts.settings.report_background_progress_bars,
        )
    } else {
        (change.tell.kind, false)
    };
    acceptance::accept(kind, window_facts(&change.tell), background).is_ok()
}

/// The function of a UI Automation property change entry, which asks
/// nothing for a name, value or state change, once it passes acceptance.
pub(crate) fn uia(cx: &mut Cx<Entries>, change: &UiaChange) {
    if !accepted(&cx.facts(), change) {
        return;
    }
    let said = uia_changed(&mut cx.facts_mut(), change);
    for said in said {
        cx.emit(said);
    }
}

/// The function of a UI Automation property change entry at its turn,
/// which passes acceptance on its tell's facts first. A
/// help text change, told as a description change, and a controller-for
/// change, told as a suggestions change, are the two Core asks for: one
/// read of the node's description, read when the change is handled (the
/// full description, else the help text), and one of whether its
/// suggestions are open, read live (`docs/parity/changes-and-selection.md`,
/// "Suggestions appearing and going away"). Both are asked only on the
/// committed focus: a description change is spoken for no other node, nor
/// for one off the line at its arrival, and a suggestions change on any
/// other plays the suggestions-closed indication unread when its element
/// is a suggestions field. Any other change speaks the value its tell
/// carries ([`uia`]).
pub(crate) async fn uia_entry(mut cx: Cx<Entries>, change: UiaChange) {
    let asked = match change.property {
        PropertyId::Description => PropertyId::Description,
        PropertyId::Suggestions => PropertyId::Suggestions,
        _ => return uia(&mut cx, &change),
    };
    if !accepted(&cx.facts(), &change) {
        return;
    }
    let target = uia_target(&cx.facts(), &change);
    let Some((kept, node)) = target.filter(|(kept, node)| *node == kept.node) else {
        // A suggestions change on anything but the committed focus plays
        // the suggestions-closed indication with no read, for a
        // suggestions field alone, the opening's test applying only to the
        // focus.
        if asked == PropertyId::Suggestions && suggestions_field(&change.tell) {
            cx.emit(verbatim_model::Effect::PlayEarcon(
                verbatim_model::Earcon::SuggestionsClosed,
            ));
        }
        return;
    };
    if asked == PropertyId::Description && change.at_arrival == AtArrival::OffLine {
        return;
    }
    let read = crate::announcements::selection::NodeProperties {
        window: kept.window,
        node,
        set: vec![asked],
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(values) = cx.ask(read, due_us).await else {
        return;
    };
    let value = values
        .answer(asked)
        .and_then(|answer| answer.value.as_ref());
    let property = match (asked, value) {
        (PropertyId::Suggestions, Some(PropertyValue::Flag(open))) => {
            PropertyChange::Suggestions(*open)
        }
        // An object that reports no suggestions says nothing.
        (PropertyId::Suggestions, _) => return,
        (_, value) => match Changed::Description.change(value) {
            Some(property) => property,
            None => return,
        },
    };
    speak_change(&mut cx, change.tell.trace_id, node, &property, None);
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
        at_arrival: AtArrival::default(),
    })
}

/// Whether a suggestions change's element is a suggestions field, by the
/// two facts its outpost's tell carries from the event's cache: editable
/// text, which an element with a text pattern is taken to be, or a search
/// field known by its automation id `SearchTextBox` or `TextBox`
/// (`docs/parity/changes-and-selection.md`, "Suggestions appearing and
/// going away").
fn suggestions_field(tell: &Event) -> bool {
    let cached = |id| {
        tell.cached
            .answer(id)
            .and_then(|answer| answer.value.as_ref())
    };
    matches!(
        cached(PropertyId::HasTextPattern),
        Some(PropertyValue::Flag(true))
    ) || matches!(
        cached(PropertyId::AutomationId),
        Some(PropertyValue::Text(id)) if id == "SearchTextBox" || id == "TextBox"
    )
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
