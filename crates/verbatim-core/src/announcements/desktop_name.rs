//! The virtual desktop's name at its turn
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and
//! objects", its announcements).

use verbatim_model::property::{PropertyId, PropertyValue};

use super::Announcement;
use crate::batch::{On, Single};
use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader};
use verbatim_model::request::MsaaIds;

/// How long after its read a name no focus said is said alone.
const NAME_DUE_US: u64 = 250_000;

/// The function of a desktop name entry, naming `ids`.
pub(crate) async fn run(mut cx: Cx<Entries>, announcement: Announcement, ids: MsaaIds) {
    let Some(window) = announcement.window() else {
        return;
    };
    let mut batch = cx.batch(|| ());
    let read = batch.read(On::msaa(ids, false), &[PropertyId::Name]);
    let request = batch.finish().of(Reader::Listener, window).one(read);
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(Single::Done(read)) = cx.ask(request, due_us).await else {
        return;
    };
    let Some(PropertyValue::Text(name)) = read
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
