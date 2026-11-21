//! A toast at its turn
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and
//! objects", its announcements).

use verbatim_model::{Backend, NodeId, SpeechPriority, ToastId};

use super::{Announcement, Kept, native};
use crate::batch::{On, Single};
use crate::entries::{Entries, window_facts};
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader};
use crate::focus::FOCUS_SET;
use crate::state::LastToast;

/// A toast's nearest window class.
const TOAST_WINDOW: &str = "Windows.UI.Core.CoreWindow";

/// How long after a toast the same toast is dropped as a repeat.
const REPEAT_US: u64 = 1_000_000;

/// The function of a toast entry, its element `kept` by the listener.
pub(crate) async fn run(mut cx: Cx<Entries>, announcement: Announcement, kept: Kept) {
    let tell = &announcement.tell;
    let Some(facts) = window_facts(tell) else {
        return;
    };
    if facts.window.class != TOAST_WINDOW || !native(tell) {
        return;
    }
    let id = ToastId(kept.runtime_id.clone());
    if cx
        .facts()
        .last_toast
        .as_ref()
        .is_some_and(|last| last.id == id && tell.at_us.saturating_sub(last.at_us) < REPEAT_US)
    {
        return;
    }
    let Some(window) = announcement.window() else {
        return;
    };
    let mut batch = cx.batch(|| ());
    let read = batch.read(
        On::kept(kept.listener, kept.token, false),
        &crate::details::node_set(&cx.facts(), FOCUS_SET),
    );
    let request = batch.finish().of(Reader::Listener, window).one(read);
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(Single::Done(mut node)) = cx.ask(request, due_us).await else {
        return;
    };
    // The listener mints no node; a toast is announced, never followed.
    node.node = Some(NodeId::new(0));
    let Some(snapshot) = crate::focus::execute::snapshot(
        &verbatim_model::request::FocusNode {
            read: node,
            runtime_id: None,
        },
        Backend::Uia,
    ) else {
        return;
    };
    cx.facts_mut().last_toast = Some(LastToast {
        id,
        at_us: tell.at_us,
    });
    cx.emit(crate::reduce::announce_object(
        tell.trace_id,
        &snapshot,
        SpeechPriority::Queued,
    ));
}
