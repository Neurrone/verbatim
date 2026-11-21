//! A show at its turn
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and
//! objects", its announcements).

use verbatim_model::request::{FocusNode, ObjectRef, Purpose, ReadObject};
use verbatim_model::{Backend, Role, SpeechPriority};

use super::Announcement;
use crate::changes::ChangeRead;
use crate::entries::{Entries, window_facts};
use crate::event_queue::RequestWindow;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome};
use crate::focus::FOCUS_SET;
use crate::focus::acceptance;
use verbatim_model::request::MsaaIds;

/// The tooltip class, whose shows are read.
const TOOLTIP_CLASS: &str = "tooltips_class32";

/// The function of a show entry, naming `ids` in `window`.
pub(crate) async fn run(
    mut cx: Cx<Entries>,
    announcement: Announcement,
    ids: MsaaIds,
    window: RequestWindow,
) {
    let tell = &announcement.tell;
    if window_facts(tell).is_none_or(|facts| facts.window.class != TOOLTIP_CLASS)
        || !crate::changes::passes(&cx, acceptance::accepted(tell), tell, &window)
    {
        return;
    }
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
            relation: None,
            descendants: false,
            purpose: Purpose::Other,
        },
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(read) = cx.ask(request, due_us).await else {
        return;
    };
    let Some(node) = crate::focus::execute::snapshot(
        &FocusNode {
            read: read.object,
            runtime_id: None,
        },
        Backend::Msaa,
    ) else {
        return;
    };
    if node.role != Role::HelpBalloon {
        return;
    }
    cx.emit(crate::reduce::announce_object(
        tell.trace_id,
        &node,
        SpeechPriority::Queued,
    ));
}
