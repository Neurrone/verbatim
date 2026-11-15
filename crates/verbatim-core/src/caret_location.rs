//! Report caret location, an event-queue entry
//! (`docs/design/readers-tell-core-asks-build1.md`, "Keys and commands in
//! this build"): at its turn it asks the committed focus's outpost where
//! the caret is, and holds the queue until the answer, as NVDA's script
//! runs on its queue (`docs/parity/review.md`, "Review cursor
//! line/word/character over object text"). Any outcome but a result says
//! nothing; a hung window has played the not-responding sound in the
//! queue.

use verbatim_model::{Message, TextOp, TextPoint, TraceId};

use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome};
use crate::text_request::CaretTarget;
use crate::{editing, review_text};

/// A press of report caret location under `trace_id`, at its turn.
pub(crate) async fn run(mut cx: Cx<Entries>, trace_id: TraceId) {
    let target = CaretTarget::of(&cx.facts());
    let text = match target {
        CaretTarget::NoCaret => {
            cx.emit(editing::speak(
                trace_id,
                review_text::message(Message::NoCaret),
            ));
            return;
        }
        CaretTarget::NoText => {
            cx.emit(editing::speak(
                trace_id,
                review_text::message(Message::NotSupported),
            ));
            return;
        }
        CaretTarget::Text(text) => text,
    };
    let request = text.request(TextOp::Location(TextPoint::Caret));
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(reply) = cx.ask(request, due_us).await else {
        return;
    };
    for effect in review_text::location(trace_id, &reply) {
        cx.emit(effect);
    }
}

#[cfg(test)]
#[path = "caret_location_tests.rs"]
mod tests;
