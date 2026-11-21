//! Report focus, Verbatim+Tab (`books/dev/src/reference/crates/verbatim-core.md`, "Verbatim+Tab"):
//! an event-queue entry that, at its turn, reads the
//! committed focus node's properties live in one request and speaks them
//! as NVDA's script speaks its focus object, its text summary included.
//! The answer is only spoken: it never becomes Core's focus.
//!
//! With no committed focus it says "No focus", asking nothing. Any outcome
//! but a result says nothing; a request refused unsent, or stopped,
//! because the window is hung has already played the not-responding sound
//! in the queue. Each press waits its turn, so two presses give two
//! announcements, and a press ahead of a focus reads the old focus.

use verbatim_model::property::{PropertyId, PropertyValue, TextSummary};
use verbatim_model::request::{FocusNode, NodeRead};
use verbatim_model::{
    Backend, Effect, Message, NodeId, NodeSnapshot, Phrase, Pid, SegmentContent, SelectionText,
    SpeechPriority, TraceId, Utterance, UtteranceSegment,
};

use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, RequestWindow};
use crate::focus::versions::Committed;
use crate::{editing, review_text, text};

/// The properties a node's announcement speaks, which a read of a node
/// for its announcement asks.
pub(crate) const ANNOUNCED_SET: [PropertyId; 15] = [
    PropertyId::Name,
    PropertyId::Role,
    PropertyId::Value,
    PropertyId::States,
    PropertyId::Description,
    PropertyId::KeyboardShortcut,
    PropertyId::PositionInSet,
    PropertyId::SetSize,
    PropertyId::Level,
    PropertyId::Rect,
    PropertyId::ClassName,
    PropertyId::NativeWindow,
    PropertyId::RangeValue,
    PropertyId::Cell,
    PropertyId::TableCounts,
];

/// The properties report focus reads: everything the focus announcement
/// speaks that the settings report, and the text summary.
fn report_set(state: &crate::state::SrState) -> Vec<PropertyId> {
    let mut set = crate::details::node_set(state, &ANNOUNCED_SET);
    set.push(PropertyId::TextSummary);
    set
}

/// The read of the committed focus's node: its application's outpost,
/// the window named, the node and the set.
type FocusProperties = (Pid, RequestWindow, NodeId, Vec<PropertyId>);

/// What a press finds committed.
enum Target {
    /// Nothing: "No focus".
    Nothing,
    /// A node to read, through `backend`.
    Read(FocusProperties, Backend),
    /// A focus with no node to read: the committed focus's reader ended,
    /// so its node is not there, or Core's own focus.
    Unread,
}

/// A press of report focus under `trace_id`, `repeat` extra times in
/// quick succession, at its turn.
pub(crate) async fn run(mut cx: Cx<Entries>, trace_id: TraceId, repeat: u8) {
    let target = {
        let facts = cx.facts();
        match &facts.versions.committed {
            Committed::Nothing => Target::Nothing,
            Committed::Read(focus) => Target::Read(
                (
                    focus.context.source,
                    focus.kept.asked,
                    focus.context.snapshot.id,
                    report_set(&facts),
                ),
                focus.kept.read_by.backend(),
            ),
            Committed::Ended(_) | Committed::Own => Target::Unread,
        }
    };
    let (request, backend) = match target {
        Target::Nothing => {
            cx.emit(editing::speak(
                trace_id,
                review_text::message(Message::NoFocus),
            ));
            return;
        }
        Target::Unread => return,
        Target::Read(request, backend) => (request, backend),
    };
    let (process, asked, node, set) = request;
    let window = asked.node_window();
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(values) =
        crate::navigator::read_values(&mut cx, (process, asked, node), &set, due_us).await
    else {
        return;
    };
    let summary = match values
        .answer(PropertyId::TextSummary)
        .and_then(|answer| answer.value.clone())
    {
        Some(PropertyValue::TextSummary(summary)) => Some(summary),
        _ => None,
    };
    let read = FocusNode {
        read: NodeRead {
            node: Some(node),
            window,
            values,
        },
        runtime_id: None,
    };
    let Some(snapshot) = crate::focus::execute::snapshot(&read, backend) else {
        return;
    };
    let said = {
        let mut facts = cx.facts_mut();
        speak(&mut facts, trace_id, (&snapshot, summary), repeat)
    };
    for effect in said {
        cx.emit(effect);
    }
}

/// What one press says of `node`, with its text summary, for its `repeat`
/// count: the object, then its text, for the first press; its name
/// spelled for the second; spelled with character descriptions after.
fn speak(
    state: &mut crate::state::SrState,
    trace_id: TraceId,
    (node, summary): (&NodeSnapshot, Option<TextSummary>),
    repeat: u8,
) -> Vec<Effect> {
    if repeat > 0 {
        let segments =
            review_text::spell_or_blank(node.name.as_deref().unwrap_or_default(), repeat > 1, None);
        return vec![about(trace_id, node, segments)];
    }
    let mut effects = vec![Effect::Speak(
        crate::reduce::announce_query(state, trace_id, node),
        None,
    )];
    if editing::may_have_text(node.role) {
        let segments = match summary {
            Some(TextSummary::Selected(selected)) => vec![UtteranceSegment::new(
                SegmentContent::Phrase(Phrase::Preselected(SelectionText::Text(selected))),
            )],
            Some(TextSummary::Line(line)) => {
                text::text_segments(text::line_content(&line, editing::is_grid(node.role)), None)
            }
            None => Vec::new(),
        };
        if !segments.is_empty() {
            effects.push(about(trace_id, node, segments));
        }
    }
    effects
}

/// `segments`, queued, spoken about `node`.
fn about(trace_id: TraceId, node: &NodeSnapshot, segments: Vec<UtteranceSegment>) -> Effect {
    Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(crate::reduce::source_of(node)),
            say_all: false,
            validity: None,
        },
        None,
    )
}

#[cfg(test)]
#[path = "report_focus_tests.rs"]
mod tests;
