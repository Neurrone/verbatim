//! Report caret location, an event-queue entry
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Text"): at its turn it asks the committed
//! focus's outpost where
//! the caret is, and holds the queue until the answer, as NVDA's script
//! runs on its queue (`docs/parity/review.md`, "Review cursor
//! line/word/character over object text"). Any outcome but a result says
//! nothing; a hung window has played the not-responding sound in the
//! queue.

use verbatim_model::{Message, TextPoint, TraceId};

use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US};
use crate::text_request::{Answerer, CaretTarget};
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
    let due_us = cx.due_in(OTHER_DUE_US);
    let mut sent = crate::batch::batch(|| ());
    let located = sent.locate(text.node, TextPoint::Caret.into());
    let by = Answerer::Outpost(text.window);
    let Some(answers) = crate::text_request::ask(&mut cx, by, sent.finish(), due_us).await else {
        return;
    };
    let located = crate::text_request::got(&answers, located);
    for effect in review_text::location(trace_id, located) {
        cx.emit(effect);
    }
}

#[cfg(test)]
#[path = "caret_location_tests.rs"]
mod tests;
