//! A caret tell as an event-queue entry (`docs/design/readers-tell-core-asks-build1.md`,
//! "Selection and the caret's text facts", Tables 3 and 17): the system
//! caret's location change or show from the listener, or a UI Automation
//! text-selection change from the outpost following the focus.
//!
//! At its turn on the committed focus, with no focus pending, it asks one
//! caret read naming the selection baseline; the outpost compares the
//! selection now with the baseline's range as it now is and mints the next
//! baseline. Core speaks the difference in general terms when the
//! text-changed flag is set, or, in an edit control, when a value change of
//! the focus waits in the queue at the answer, which it takes as counted;
//! otherwise in specific terms. NVDA: `NVDAObjects/behaviors.py` 346 to
//! 349; `editableText.py` 429 to 451; `speech/speech.py` 1340 to 1418;
//! `NVDAObjects/window/edit.py` 1068 to 1080.
//!
//! The system caret's tell is for the readers' last focus, whatever its
//! window, as NVDA handles the MSAA caret's events as the last queued
//! focus's (`IAccessibleHandler/__init__.py` 634 to 666), and is said only
//! when that is the committed focus; a UI Automation tell is for the
//! element its runtime id names, said only when that is the committed
//! focus (`UIAHandler/__init__.py`).

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::RuntimeId;
use verbatim_model::request::{MsaaIds, Reply, Request as Wire};
use verbatim_model::tell::{ElementToken, Event, EventObject};
use verbatim_model::{
    NodeId, OutpostId, SelectionRead, SelectionReadReply, TextOp, TextProvider, TextReply,
};

use crate::changes::MsaaChange;
use crate::entries::{Entries, Entry, request_window, window_facts};
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, Request, RequestWindow};
use crate::focus::acceptance;
use crate::focus::versions::{Committed, Kept, ReadBy};
use crate::keys::entry::Fence;

/// The object a caret tell is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Target {
    /// The system caret's: the readers' last focus.
    System(MsaaIds),
    /// A UI Automation element, by its runtime id.
    Uia(RuntimeId),
}

/// A caret tell, parsed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CaretTell {
    /// The tell.
    pub(crate) tell: Event,
    /// The object it is for.
    pub(crate) target: Target,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: Fence,
}

impl CaretTell {
    /// The caret tell `tell` is, `None` for a tell of another kind, or one
    /// whose object does not fit its kind, a reader's fault.
    pub(crate) fn of_tell(tell: Event) -> Option<Self> {
        let target = match (tell.kind, &tell.object) {
            (
                EventKind::SystemCaret,
                EventObject::Msaa {
                    window,
                    object,
                    child,
                },
            ) => Target::System(MsaaIds {
                window: *window,
                object: *object,
                child: *child,
            }),
            (EventKind::UiaTextSelectionChanged, EventObject::Uia { runtime_id, .. }) => {
                Target::Uia(runtime_id.clone())
            }
            _ => return None,
        };
        Some(Self {
            tell,
            target,
            fence: Fence::default(),
        })
    }

    /// The element the outpost adopts as the node's when the node's own
    /// has no text pattern (design, "Selection and the caret's text
    /// facts"): a UI Automation tell's, from the outpost following the
    /// focus; the system caret's names none.
    pub(crate) fn adopted(&self) -> Option<ElementToken> {
        match (&self.target, &self.tell.object) {
            (Target::Uia(_), EventObject::Uia { token, .. }) => Some(*token),
            _ => None,
        }
    }

    /// The window its request names, for the foreground test.
    pub(crate) fn window(&self) -> Option<RequestWindow> {
        request_window(&self.tell)
    }
}

/// Whether two caret tells waiting for one object merge into one entry:
/// UI Automation tells of one element from one thread. The system caret's
/// merge by the MSAA limiter's place (NVDA:
/// `IAccessibleHandler/orderedWinEventLimiter.py` 79).
pub(crate) fn merges(waiting: &CaretTell, arriving: &CaretTell) -> bool {
    matches!((&waiting.target, &arriving.target), (Target::Uia(a), Target::Uia(b)) if a == b)
        && waiting.tell.thread == arriving.tell.thread
}

/// A caret tell's read on the committed focus's node.
#[derive(Debug)]
struct SelectionReadRequest {
    window: RequestWindow,
    node: NodeId,
    read: SelectionRead,
}

impl Request for SelectionReadRequest {
    type Reply = SelectionReadReply;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::Text {
            node: self.node,
            op: TextOp::ReadSelection(self.read),
        }
    }

    fn decode(reply: Reply) -> Option<SelectionReadReply> {
        match reply {
            Reply::Text(TextReply::SelectionRead(read)) => Some(*read),
            _ => None,
        }
    }
}

/// The committed focus `tell` is for, when its reader lives: the system
/// caret's, when the readers' last focus is the committed focus (Table 3,
/// column G); a UI Automation tell's, when its runtime id is the
/// committed focus's.
fn target(cx: &Cx<Entries>, tell: &CaretTell) -> Option<Kept> {
    let kept = {
        let facts = cx.facts();
        match &tell.target {
            Target::System(_) => facts.versions.caret_target().cloned(),
            Target::Uia(runtime_id) => match &facts.versions.committed {
                Committed::Read(kept) if matches!(&kept.read_by, ReadBy::Uia { runtime_id: focus, .. } if focus == runtime_id) => {
                    Some((**kept).clone())
                }
                _ => None,
            },
        }
    }?;
    let live = cx
        .instance(Reader::Outpost(kept.window.process))
        .is_some_and(|instance| OutpostId(instance.generation) == kept.outpost);
    live.then_some(kept)
}

/// Whether `change` is an MSAA value change of the committed focus
/// `kept`'s own window object, whose value is its text.
fn value_change_of(kept: &Kept, change: &MsaaChange) -> bool {
    /// The object id of a window's client area.
    const OBJID_CLIENT: i32 = -4;
    change.tell.kind == EventKind::ValueChange
        && change.ids.window == kept.place.window
        && change.ids.object == OBJID_CLIENT
        && change.ids.child == 0
}

/// Whether `change`, an MSAA change at its turn, is a value change of the
/// committed focus `kept` that sets its text-changed flag (Table 17,
/// column G): `kept` is editable text with automatic selection detection
/// read through an edit control's messages.
pub(crate) fn sets_text_changed(kept: &Kept, change: &MsaaChange) -> bool {
    crate::text_facts::provider(kept) == TextProvider::EditMessages
        && crate::text_facts::detects_selection(kept)
        && value_change_of(kept, change)
}

/// The caret tell's function at its turn.
pub(crate) async fn run(mut cx: Cx<Entries>, tell: CaretTell) {
    if acceptance::accept(tell.tell.kind, window_facts(&tell.tell), false).is_err() {
        return;
    }
    if crate::keys::caret_key::focus_pending(&cx) {
        return;
    }
    let Some(kept) = target(&cx, &tell) else {
        return;
    };
    let provider = crate::text_facts::provider(&kept);
    let request = SelectionReadRequest {
        window: kept.window,
        node: kept.node,
        read: SelectionRead {
            provider,
            baseline: kept.selection.baseline,
            adopt: tell.adopted(),
        },
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    // A read that fails says nothing and keeps the baseline and the flag
    // (Table 17, column I). NVDA: `editableText.py` 433 to 436.
    let Outcome::Result(read) = cx.ask(request, due_us).await else {
        return;
    };
    answered(&mut cx, &tell, (&kept, provider), read);
}

/// Judges the caret tell's answer `read` on the committed focus `kept`
/// (Table 17, columns D and E).
fn answered(
    cx: &mut Cx<Entries>,
    tell: &CaretTell,
    (kept, provider): (&Kept, TextProvider),
    read: SelectionReadReply,
) {
    if let Some(offsets) = read.edit_selection
        && crate::text_facts::edit_selection_repeated(&mut cx.facts_mut(), offsets)
    {
        return;
    }
    let collapsed = read.caret.selection.is_none();
    crate::editing::take_caret(&mut cx.facts_mut(), kept.node, read.caret);
    let Some(difference) = read.difference else {
        // No baseline to compare with: nothing said, the next one minted,
        // the flag kept (column E).
        crate::text_facts::minted(&mut cx.facts_mut(), kept.node, read.baseline);
        return;
    };
    // In an edit control, a value change of the focus waiting now makes
    // the terms general, and is taken as counted, as NVDA runs it inside
    // its caret event (`NVDAObjects/window/edit.py` 1075 to 1080).
    let value_waiting = provider == TextProvider::EditMessages
        && !cx
            .take_waiting(
                |entry| matches!(entry, Entry::MsaaChange(change) if value_change_of(kept, change)),
            )
            .is_empty();
    let general = {
        let mut facts = cx.facts_mut();
        let Committed::Read(committed) = &mut facts.versions.committed else {
            return;
        };
        if committed.node != kept.node {
            return;
        }
        let general = committed.selection.text_changed || value_waiting;
        committed.selection.text_changed = false;
        committed.selection.baseline = Some(read.baseline);
        general
    };
    let segments = crate::editing::selection_segments(
        &difference.changes,
        general.then_some(crate::editing::General {
            selection: difference.new_selection.as_ref(),
            collapsed,
        }),
    );
    if !segments.is_empty() {
        cx.emit(crate::editing::speak(tell.tell.trace_id, segments));
    }
}

#[cfg(test)]
#[path = "caret_tell_tests.rs"]
mod tests;
