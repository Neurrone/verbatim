//! The virtual desktop's name at its turn (`docs/design/readers-tell-core-asks-build1.md`,
//! "Focus and foreground"): an MSAA name change on the desktop window is
//! read by the listener, the name alone, and kept until the next focus
//! Core takes says it before its entered containers, or until 250 ms
//! later, when it is said alone (`crate::desktop`). Any outcome but a
//! result keeps nothing.

use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::{ObjectRef, Purpose, ReadObject};

use super::Announcement;
use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome};
use crate::focus::uia_focus::ListenerRead;
use verbatim_model::request::MsaaIds;

/// How long after its read a name no focus said is said alone.
const NAME_DUE_US: u64 = 250_000;

/// The function of a desktop name entry, naming `ids`.
pub(crate) async fn run(mut cx: Cx<Entries>, announcement: Announcement, ids: MsaaIds) {
    let Some(window) = announcement.window() else {
        return;
    };
    let read = ListenerRead {
        window,
        read: ReadObject {
            object: ObjectRef::Msaa {
                window: ids.window,
                object: ids.object,
                child: ids.child,
            },
            set: vec![PropertyId::Name],
            relation: None,
            descendants: false,
            walk: None,
            purpose: Purpose::Other,
        },
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(read) = cx.ask(read, due_us).await else {
        return;
    };
    let Some(PropertyValue::Text(name)) = read
        .object
        .values
        .answer(PropertyId::Name)
        .and_then(|answer| answer.value.clone())
    else {
        return;
    };
    let said_at_us = cx.due_in(NAME_DUE_US);
    crate::desktop::named(
        &mut cx.facts_mut(),
        announcement.tell.trace_id,
        name,
        said_at_us,
    );
}
