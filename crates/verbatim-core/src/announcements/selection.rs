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
//! - a list item without the keyboard focus whose parent is the committed
//!   focus gives that focus's value change, its value read live, as NVDA's
//!   `ListItem.event_stateChange` fires it (`NVDAObjects/UIA/__init__.py`
//!   2786 to 2800), when the focus is a combo box whose value is its
//!   selected item's, or has the `Value` pattern in a window whose class
//!   begins with `Windows.UI.Core`, both read with its value in one
//!   request;
//! - a list item in a `ComboLBox` window of the committed focus's process,
//!   while the focus is a combo box, is taken whatever the acceptance test
//!   says once Core has asked the focus for its `ComboBoxList`, present
//!   only for NVDA's classic .NET Framework Windows Forms combo box, which
//!   asks for these events (`requestEvents`, 2734 to 2744); after the
//!   selection's own handling, an item in that list gives the focus's value
//!   change (`ListItem.event_UIA_elementSelected`, 2770 to 2784);
//! - anything else is a state change of the item, which says nothing,
//!   since it is neither the focus nor one of its ancestors.

use std::iter;

use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    CompleteUia, CompletionFor, ObjectRead, ObjectRef, Purpose, ReadObject, RelationFound, Reply,
    Request as Wire, RouteEnd,
};
use verbatim_model::{
    Backend, NodeId, OutpostId, PropertyChange, Role, State, StateSet, WindowHandle,
};

use super::{Announcement, Kind, accepted, native, text};
use crate::entries::Entries;
use crate::entries::window_facts;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, Request, RequestWindow};
use crate::focus::FOCUS_SET;
use crate::focus::execute::Known;
use crate::focus::uia_focus::ListenerRead;
use crate::focus::versions::{Kept as CommittedFocus, ReadBy};

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
    if !native(tell) {
        return;
    }
    let Some(committed) = cx.facts().versions.committed_read().cloned() else {
        return;
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let list = if in_combo_box_list_window(tell, &committed) {
        combo_box_list(&mut cx, &committed, due_us).await
    } else {
        None
    };
    // The Windows Forms combo box's request takes its list's events
    // whatever the foreground.
    if !accepted(tell) && list.is_none() {
        return;
    }
    selected(&mut cx, &announcement, (kept, window, &committed), due_us).await;
    if list == Some(window.handle) && cached_role(tell) == Some(Role::ListItem) {
        combo_box_value(&mut cx, tell.trace_id, &committed, due_us).await;
    }
}

/// The selection's own handling (`event_selection`): the committed
/// focus's state change, an item the focus controls spoken as a focus, or
/// the value change of the combo box that is the item's parent.
async fn selected(
    cx: &mut Cx<Entries>,
    announcement: &Announcement,
    (kept, window, committed): (&super::Kept, RequestWindow, &CommittedFocus),
    due_us: u64,
) {
    let tell = &announcement.tell;
    let trace_id = tell.trace_id;
    // A selection of the committed focus is its state change, judged on the
    // states its tell carries.
    if matches!(&committed.read_by, ReadBy::Uia { runtime_id, .. } if runtime_id == &kept.runtime_id)
    {
        let Some(states) = cached_states(tell) else {
            return;
        };
        crate::changes::speak_change(
            cx,
            trace_id,
            committed.node,
            &PropertyChange::States(states),
            None,
        );
        return;
    }
    let read = ListenerRead {
        window,
        read: ReadObject {
            object: ObjectRef::Kept {
                listener: kept.listener,
                token: kept.token,
            },
            set: crate::details::walk_set(&cx.facts(), FOCUS_SET),
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
            set: crate::details::node_set(&cx.facts(), FOCUS_SET),
            known: known.sent.clone(),
            completes: CompletionFor::SelectionItem {
                focus: committed.node,
            },
            details: crate::details::focus_details(&cx.facts()),
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
                cx,
                trace_id,
                committed.node,
                (item.object, Backend::Uia),
            );
        }
        _ if parent_is_focus(&item, &known)
            && !focused(&item)
            && read_role(&item) == Some(Role::ListItem) =>
        {
            if committed.value_from_selection {
                combo_box_value(cx, trace_id, committed, due_us).await;
            } else {
                core_window_value(cx, trace_id, committed, due_us).await;
            }
        }
        _ => {}
    }
}

/// Whether `tell`, a selection, is from a `ComboLBox` window of the
/// committed focus's process while the focus is a combo box: the events a
/// Windows Forms combo box asks for, Core asking the focus whether it is
/// one ([`combo_box_list`]).
fn in_combo_box_list_window(
    tell: &verbatim_model::tell::Event,
    committed: &CommittedFocus,
) -> bool {
    committed.role == Role::ComboBox
        && tell.process == committed.source
        && window_facts(tell).is_some_and(|facts| facts.window.class == "ComboLBox")
}

/// The window of the committed focus's drop-down list when the focus is a
/// classic .NET Framework Windows Forms combo box, read by its outpost
/// (`ComboBoxList`); `None` for any other focus, or when it is not
/// answered.
async fn combo_box_list(
    cx: &mut Cx<Entries>,
    combo_box: &CommittedFocus,
    due_us: u64,
) -> Option<WindowHandle> {
    let read = NodeProperties {
        window: combo_box.window,
        node: combo_box.node,
        set: vec![PropertyId::ComboBoxList],
    };
    let Outcome::Result(values) = cx.ask(read, due_us).await else {
        return None;
    };
    match values
        .answer(PropertyId::ComboBoxList)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Window(list)) => Some(*list),
        _ => None,
    }
}

/// The value change of `combo_box`, the committed focus, whose selected
/// item changed, when it has the `Value` pattern and its window's class
/// begins with `Windows.UI.Core` (NVDA issue 6337): the three read in one
/// request, its value spoken as a value change.
async fn core_window_value(
    cx: &mut Cx<Entries>,
    trace_id: verbatim_model::TraceId,
    combo_box: &CommittedFocus,
    due_us: u64,
) {
    let read = NodeProperties {
        window: combo_box.window,
        node: combo_box.node,
        set: vec![
            PropertyId::HasValuePattern,
            PropertyId::WindowClass,
            PropertyId::Value,
        ],
    };
    let Outcome::Result(values) = cx.ask(read, due_us).await else {
        return;
    };
    let value_of = |id| values.answer(id).and_then(|answer| answer.value.as_ref());
    let has_value = matches!(
        value_of(PropertyId::HasValuePattern),
        Some(PropertyValue::Flag(true))
    );
    let core_window =
        text(value_of(PropertyId::WindowClass)).is_some_and(|class| class.starts_with(CORE_WINDOW));
    if !(has_value && core_window) {
        return;
    }
    let value = text(value_of(PropertyId::Value)).map(str::to_owned);
    crate::changes::speak_change(
        cx,
        trace_id,
        combo_box.node,
        &PropertyChange::Value(value),
        None,
    );
}

/// The class prefix of a Windows Runtime core window.
const CORE_WINDOW: &str = "Windows.UI.Core";

/// The value change of `combo_box`, the committed focus, whose selected
/// item changed: its value read live and spoken as a value change.
async fn combo_box_value(
    cx: &mut Cx<Entries>,
    trace_id: verbatim_model::TraceId,
    combo_box: &CommittedFocus,
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

/// The role a selection's tell carries from its event's cache.
fn cached_role(tell: &verbatim_model::tell::Event) -> Option<Role> {
    match tell
        .cached
        .answer(PropertyId::Role)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Role(role)) => Some(*role),
        _ => None,
    }
}

/// The item's role, as read.
fn read_role(item: &ObjectRead) -> Option<Role> {
    match item
        .object
        .values
        .answer(PropertyId::Role)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Role(role)) => Some(*role),
        _ => None,
    }
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
