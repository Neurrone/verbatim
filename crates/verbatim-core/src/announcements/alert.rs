//! An MSAA alert at its turn
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and
//! objects", its announcements).

use verbatim_model::request::{
    FocusNode, NodeRead, ObjectRef, Purpose, ReadObject, RelationFound, RelationQuery,
};
use verbatim_model::{Backend, NodeId, NodeSnapshot, Role, SpeechPriority, State};

use super::{Announcement, Kind};
use crate::changes::ChangeRead;
use crate::entries::Entries;
use crate::event_queue::RequestWindow;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader};
use crate::focus::FOCUS_SET;
use crate::focus::acceptance;
use verbatim_model::request::MsaaIds;

/// The function of an MSAA alert entry, naming `ids` in `window`, moved
/// behind the pending entries once when `moved`.
pub(crate) async fn run(
    mut cx: Cx<Entries>,
    announcement: Announcement,
    (ids, window): (MsaaIds, RequestWindow),
    moved: bool,
) {
    let tell = &announcement.tell;
    if !moved && focus_pending_behind(&cx) {
        let mut moved = announcement.clone();
        moved.kind = Kind::Alert {
            ids,
            window,
            moved: true,
        };
        cx.requeue(crate::entries::Entry::Announcement(Box::new(moved)));
        return;
    }
    if !crate::changes::passes(&cx, acceptance::accepted(tell), tell, &window) {
        return;
    }
    // The committed focus's line, when the alert's outpost holds it: the
    // focus first, met by NVDA's equality, then its ancestors.
    let generation = cx
        .instance(Reader::Outpost(window.process))
        .map(verbatim_model::request::Instance::outpost_id);
    let line: Vec<NodeId> = cx
        .facts()
        .versions
        .live()
        .filter(|focus| {
            generation
                .is_some_and(|generation| focus.context.snapshot.id.outpost() == Some(generation))
        })
        .map(|focus| {
            std::iter::once(focus.context.snapshot.id)
                .chain(focus.line.iter().rev().map(|ancestor| ancestor.snapshot.id))
                .collect()
        })
        .unwrap_or_default();
    let request = ChangeRead {
        process: window.process,
        window,
        read: ReadObject {
            object: ObjectRef::Msaa {
                window: ids.window,
                object: ids.object,
                child: ids.child,
            },
            set: crate::details::node_set(&cx.facts(), FOCUS_SET),
            relation: (!line.is_empty()).then_some(RelationQuery::OneOf(line)),
            descendants: true,
            purpose: Purpose::Other,
        },
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(read) = cx.ask(request, due_us).await else {
        return;
    };
    // Place 0 is the focus itself, which is no ancestor of its own.
    if matches!(read.relation, Some(RelationFound::Is(place)) if place > 0) {
        return;
    }
    let Some(alert) = snapshot(read.object) else {
        return;
    };
    let description = alert
        .details
        .description
        .as_deref()
        .is_some_and(|description| !description.is_empty());
    let named = alert.name.as_deref().is_some_and(|name| !name.is_empty());
    if alert.role != Role::Alert || (!named && !description && read.descendants.is_empty()) {
        return;
    }
    let trace_id = tell.trace_id;
    cx.emit(crate::reduce::announce_object(
        trace_id,
        &alert,
        SpeechPriority::Queued,
    ));
    for child in read.descendants.into_iter().filter_map(snapshot) {
        if spoken_child(&child, description) {
            cx.emit(crate::reduce::announce_object(
                trace_id,
                &child,
                SpeechPriority::Queued,
            ));
        }
    }
}

/// Whether an entry waiting behind the alert is a focus that counts as
/// pending by the one definition: an unconfirmed MSAA focus does not, and
/// is not read, as NVDA's events not yet pumped are not pending.
fn focus_pending_behind(cx: &Cx<Entries>) -> bool {
    crate::focus::validity::pending_behind_front(cx, crate::focus::validity::Within::ALL)
        == crate::focus::validity::Pending::Counts
}

/// Whether NVDA speaks `child`, a descendant of an alert that has a
/// description or not.
fn spoken_child(child: &NodeSnapshot, alert_described: bool) -> bool {
    let focusable = child.states.contains(State::Focusable);
    if alert_described {
        return focusable;
    }
    focusable || matches!(child.role, Role::List | Role::ListItem)
}

/// The snapshot of an MSAA node the read minted.
fn snapshot(read: NodeRead) -> Option<NodeSnapshot> {
    crate::focus::execute::snapshot(
        &FocusNode {
            read,
            runtime_id: None,
        },
        Backend::Msaa,
    )
}
