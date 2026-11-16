//! A UI Automation selection at its turn (Table 12), NVDA's rule
//! (`NVDAObjects/UIA/__init__.py` 2508 to 2509 routing it to
//! `event_selection`, `NVDAObjects/__init__.py` 1314 to 1340):
//!
//! - on the committed focus, it is the focus's state change, judged on the
//!   states its tell carries, with no request;
//! - otherwise Core asks the listener to read the item and the item's
//!   outpost to complete it, under one due time; the completion reads the
//!   committed focus's controller-for relation live, as NVDA reads
//!   `focus.controllerFor` at the selection. An item that is an object the
//!   focus controls, or inside one, is spoken as a focus;
//! - an item without the keyboard focus whose parent is the committed
//!   focus, a combo box whose value is its selected item's, gives that combo
//!   box's value change, its value read live, as NVDA's
//!   `ListItem.event_stateChange` fires it (`NVDAObjects/UIA/__init__.py`
//!   2785 to 2800);
//! - anything else is a state change of the item, which says nothing,
//!   since it is neither the focus nor one of its ancestors.
//!
//! Not built: NVDA's two other combo box cases, a `Windows.UI.Core` combo
//! box with the Value pattern and the Windows Forms list of a `ComboLBox`,
//! whose facts no focus answer carries.

use std::iter;

use verbatim_model::property::{PropertyId, PropertyValues};
use verbatim_model::request::{
    CompleteUia, CompletionFor, ObjectRead, ObjectRef, Purpose, ReadObject, RelationFound, Reply,
    Request as Wire, RouteEnd,
};
use verbatim_model::{Backend, NodeId, OutpostId, PropertyChange, State, StateSet};

use super::{Announcement, Kind, accepted, native, text};
use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, Request, RequestWindow};
use crate::focus::FOCUS_SET;
use crate::focus::execute::Known;
use crate::focus::uia_focus::ListenerRead;
use crate::focus::versions::ReadBy;

/// The item's outpost completing the listener's read of a selected item.
#[derive(Debug)]
struct ItemCompletion {
    window: RequestWindow,
    complete: CompleteUia,
}

impl Request for ItemCompletion {
    type Reply = Box<ObjectRead>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::CompleteUia(Box::new(self.complete.clone()))
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Object(read) => Some(read),
            _ => None,
        }
    }
}

/// The properties of a node by set.
#[derive(Debug)]
pub(crate) struct NodeProperties {
    /// The node's window.
    pub(crate) window: RequestWindow,
    /// The node.
    pub(crate) node: NodeId,
    /// The set.
    pub(crate) set: Vec<PropertyId>,
}

impl Request for NodeProperties {
    type Reply = PropertyValues;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::Properties {
            node: self.node,
            set: self.set.clone(),
        }
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Properties(values) => Some(values),
            _ => None,
        }
    }
}

/// The function of a UI Automation selection entry.
pub(crate) async fn run(mut cx: Cx<Entries>, announcement: Announcement) {
    let Kind::UiaSelection(kept) = &announcement.kind else {
        return;
    };
    let tell = &announcement.tell;
    let Some(window) = announcement.window() else {
        return;
    };
    if !native(tell) || !accepted(tell) {
        return;
    }
    let Some(committed) = cx.facts().versions.committed_read().cloned() else {
        return;
    };
    let trace_id = tell.trace_id;
    // A selection of the committed focus is its state change, judged on the
    // states its tell carries.
    if matches!(&committed.read_by, ReadBy::Uia { runtime_id, .. } if *runtime_id == kept.runtime_id)
    {
        let Some(states) = cached_states(tell) else {
            return;
        };
        crate::changes::speak_change(
            &mut cx,
            trace_id,
            committed.node,
            &PropertyChange::States(states),
            None,
        );
        return;
    }
    let due_us = cx.due_in(OTHER_DUE_US);
    let read = ListenerRead {
        window,
        read: ReadObject {
            object: ObjectRef::Kept(kept.token),
            set: FOCUS_SET.to_vec(),
            relation: None,
            descendants: false,
            walk: Some(Known::of(&cx.facts(), None).sent),
            purpose: Purpose::Other,
        },
    };
    let Outcome::Result(object) = cx.ask(read, due_us).await else {
        return;
    };
    // An item read without its route is the listener's fault.
    let Some(route) = object.route else {
        return;
    };
    let generation = cx
        .instance(Reader::Outpost(window.process))
        .map(|instance| OutpostId(instance.generation));
    let known = Known::of(&cx.facts(), generation);
    if let RouteEnd::Known(met) = &route.end
        && !known.sent.contains(met)
    {
        // The committed focus changed between the two reads.
        return;
    }
    let complete = ItemCompletion {
        window,
        complete: CompleteUia {
            route,
            read: iter::once(object.object.values)
                .chain(object.ancestors.into_iter().map(|ancestor| ancestor.values))
                .collect(),
            set: FOCUS_SET.to_vec(),
            known: known.sent.clone(),
            completes: CompletionFor::SelectionItem {
                focus: committed.node,
            },
            baseline: None,
        },
    };
    let Outcome::Result(item) = cx.ask(complete, due_us).await else {
        return;
    };
    // The focus committed meanwhile is no longer the one asked about.
    if cx
        .facts()
        .versions
        .committed_read()
        .is_none_or(|now| now.node != committed.node)
    {
        return;
    }
    match item.relation {
        Some(RelationFound::Controlled | RelationFound::InsideControlled) => {
            crate::changes::controlled_selection(
                &mut cx,
                trace_id,
                committed.node,
                (item.object, Backend::Uia),
            );
        }
        _ if committed.value_from_selection
            && parent_is_focus(&item, &known)
            && !focused(&item) =>
        {
            combo_box_value(&mut cx, trace_id, &committed, due_us).await;
        }
        _ => {}
    }
}

/// The value change of `combo_box`, the committed focus, whose selected
/// item changed: its value read live and spoken as a value change.
async fn combo_box_value(
    cx: &mut Cx<Entries>,
    trace_id: verbatim_model::TraceId,
    combo_box: &crate::focus::versions::Kept,
    due_us: u64,
) {
    let read = NodeProperties {
        window: combo_box.window,
        node: combo_box.node,
        set: vec![PropertyId::Value],
    };
    let Outcome::Result(values) = cx.ask(read, due_us).await else {
        return;
    };
    let value = text(
        values
            .answer(PropertyId::Value)
            .and_then(|answer| answer.value.as_ref()),
    )
    .map(str::to_owned);
    crate::changes::speak_change(
        cx,
        trace_id,
        combo_box.node,
        &PropertyChange::Value(value),
        None,
    );
}

/// The states a selection's tell carries from its event's cache.
fn cached_states(tell: &verbatim_model::tell::Event) -> Option<StateSet> {
    match tell
        .cached
        .answer(PropertyId::States)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(verbatim_model::property::PropertyValue::States(states)) => Some(*states),
        _ => None,
    }
}

/// Whether the item's parent is the committed focus: its walk up met the
/// focus itself, the first of what Core knows, before any other ancestor.
fn parent_is_focus(item: &ObjectRead, known: &Known) -> bool {
    item.ancestors.is_empty()
        && item
            .met
            .and_then(|met| usize::try_from(met).ok())
            .and_then(|met| known.places.get(met))
            == Some(&0)
}

/// Whether the item has the keyboard focus, as read.
fn focused(item: &ObjectRead) -> bool {
    matches!(
        item.object
            .values
            .answer(PropertyId::States)
            .and_then(|answer| answer.value.as_ref()),
        Some(verbatim_model::property::PropertyValue::States(states)) if states.contains(State::Focused)
    )
}
