//! A toast at its turn (Table 12): a UI Automation window opened whose
//! nearest window is a Windows Runtime core window, NVDA's toast test
//! (`NVDAObjects/UIA/__init__.py` 1282 to 1287), not a repeat of the toast
//! spoken last within a second (ruled 2026-11-12), is read by the listener,
//! its name, value, states, description and position, and announced as
//! NVDA's `Notification.event_alert` speaks the object
//! (`NVDAObjects/behaviors.py` 1008 to 1013), queued behind current
//! speech, as today. Any outcome but a result drops it unspoken.

use verbatim_model::request::{ObjectRef, Purpose, ReadObject};
use verbatim_model::{Backend, NodeId, SpeechPriority, ToastId};

use super::{Announcement, Kept, native};
use crate::entries::{Entries, window_facts};
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome};
use crate::focus::FOCUS_SET;
use crate::focus::uia_focus::ListenerRead;
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
    let read = ListenerRead {
        window,
        read: ReadObject {
            object: ObjectRef::Kept {
                listener: kept.listener,
                token: kept.token,
            },
            set: crate::details::node_set(&cx.facts(), FOCUS_SET),
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
    let mut node = read.object;
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
