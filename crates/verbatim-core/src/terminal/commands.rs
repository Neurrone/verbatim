//! Review commands on the focused terminal, as event-queue entries
//! (`docs/design/readers-tell-core-asks-build3.md`, "Review commands as
//! chained requests", Tables 5 and 6): a command sends a fresh full read,
//! answers from Core's memory of the terminal, and asks the history read
//! or a command mark's search, at most three a press, as follow-ups under
//! the command's one due time. The answer from memory makes no request.
//!
//! A history read or a search counts from the start anchor Core names; as
//! it is sent, Core puts a place in its memory at that start's row, which
//! moves with each read the memory takes as the review position does, so
//! a read taken while it is out does not end the command; its rows are
//! placed from that place's row, and a place whose row left the memory
//! ends the command silently, marks kept.

use std::collections::BTreeSet;

use verbatim_model::batch::{Batch, StepOutcome};
use verbatim_model::request::{Reply, Request as Wire};
use verbatim_model::{
    Effect, Message, NodeId, TextAnchor, TextFailure, TextOp, TextPoint, TextReply, TraceId,
    UtteranceSegment,
};

use crate::entries::Entries;
use crate::event_queue::{Cx, Outcome, Reader, Request, RequestWindow};
use crate::state::SrState;
use crate::terminal::reading;
use crate::terminal_model::{self, review};
use crate::text_request::TextAnswer;

/// A history read or a search on the focused terminal, a follow-up of a
/// review command.
#[derive(Debug)]
struct FollowUp {
    window: RequestWindow,
    node: NodeId,
    op: TextOp,
}

impl Request for FollowUp {
    type Reply = Result<TextReply, TextFailure>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> verbatim_model::request::Question {
        verbatim_model::request::Question::Legacy(Wire::Text {
            node: self.node,
            op: self.op.clone(),
        })
    }

    fn decode(&self, reply: Reply) -> Option<Self::Reply> {
        crate::text_request::decode_text(reply, |reply| match reply {
            reply @ (TextReply::TerminalHistory { .. } | TextReply::TerminalFound { .. }) => {
                Some(reply)
            }
            _ => None,
        })
    }
}

/// Whether Core answers a review command's request on `node` naming
/// `points` from the focused terminal's memory: `node` is the modelled
/// focused terminal and its memory knows each point.
pub(crate) fn answers(state: &SrState, node: NodeId, points: &[TextPoint]) -> bool {
    reading::modelled(state) == Some(node)
        && terminal_model::model_of(state, node).is_some_and(|model| review::answers(model, points))
}

/// The anchors Core holds of terminal `node`'s outpost.
fn held_anchors(state: &SrState, node: NodeId) -> BTreeSet<TextAnchor> {
    node.outpost()
        .and_then(|outpost| state.held_anchors().remove(&outpost))
        .unwrap_or_default()
}

/// The command's fresh full read of the focused terminal `node`, once a
/// command (Table 6, column A): its answer is taken by the model, and the
/// terminal's speech waiting ahead of the first focus is said, then the
/// read's own. A read that brings nothing leaves the memory as it stands.
pub(crate) async fn fresh(cx: &mut Cx<Entries>, trace_id: TraceId, node: NodeId, due_us: u64) {
    let now_us = cx.now_us();
    let Some(asked) = reading::entry_read(&mut cx.facts_mut(), node, None, now_us) else {
        return;
    };
    let reply = match cx.ask(asked, due_us).await {
        // A read that fails, the terminal's text unread included, brings
        // nothing.
        Outcome::Result(Ok(reply)) => Some(reply),
        Outcome::Result(Err(_))
        | Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::ReaderEnded
        | Outcome::NoLongerAwaited(_) => None,
    };
    if let Some(reply) = reply.as_deref() {
        cx.emit(reading::observed(&asked.terminal, reply, trace_id));
    }
    let now_us = cx.now_us();
    let entry = reading::entry_read_ended(
        &mut cx.facts_mut(),
        &asked,
        reply.as_deref(),
        (now_us, trace_id),
    );
    reading::speak_read(cx, entry);
}

/// The place Core puts in terminal `node`'s memory at the row of the start
/// a follow-up counts from, with that start (Table 5, column A); `None`
/// when the model names no start or keeps no row there.
fn place_at_start(state: &mut SrState, node: NodeId) -> Option<(u64, TextAnchor)> {
    let model = terminal_model::model_mut(state, node)?;
    let start = model.start?;
    let row = usize::try_from(review::start_row(model)).ok()?;
    let anchor = model.reader.memory().anchor(row, 0)?;
    let place = model.rules().memory_mut().add_place(anchor);
    Some((place, start))
}

/// Where place `place` of terminal `node`'s memory is now, its row, and
/// lets it go (Table 5, columns D and E); `None` once its row left the
/// memory.
fn place_row(state: &mut SrState, node: NodeId, place: u64) -> Option<isize> {
    let model = terminal_model::model_mut(state, node)?;
    let row = model.reader.memory().place(place).map(|anchor| anchor.row);
    model.rules().memory_mut().drop_place(place);
    row
}

/// Answers `batch`, a review command's request on the focused terminal
/// `node`, from Core's memory, one outcome a step, reading the rows above
/// it the answer needs as a history read from the start Core names, a
/// screen more than needed; `None` when the command ends saying nothing:
/// its model is gone, the history read failed, or it does not hold the
/// row needed.
pub(crate) async fn answer(
    cx: &mut Cx<Entries>,
    (window, node): (RequestWindow, NodeId),
    batch: &Batch,
    due_us: u64,
) -> Option<Vec<StepOutcome>> {
    let mut history = None;
    loop {
        let need = {
            let mut facts = cx.facts_mut();
            let held = held_anchors(&facts, node);
            let mut mint = facts.next_terminal_anchor;
            let model = terminal_model::model_mut(&mut facts, node)?;
            let answer = review::answer(model, (batch, history.as_ref()), &mut mint, &held);
            let height = model.height;
            facts.next_terminal_anchor = mint;
            match answer {
                Ok(outcomes) => return Some(outcomes),
                // The history read for this request does not hold the row
                // it needs, as when more than a screen of output scrolled
                // while it was out: the command ends, saying nothing.
                Err(_) if history.is_some() => return None,
                Err(need) => (need, height),
            }
        };
        let (review::Need { above, whole }, height) = need;
        // A screen more than the answer needs, for output that scrolls
        // while the request is out.
        let above = if whole {
            0
        } else {
            above.saturating_add(height)
        };
        let (place, start) = place_at_start(&mut cx.facts_mut(), node)?;
        let request = FollowUp {
            window,
            node,
            op: TextOp::TerminalHistory {
                above,
                whole,
                start,
                height,
            },
        };
        let outcome = cx.ask(request, due_us).await;
        let row = place_row(&mut cx.facts_mut(), node, place);
        // The terminal's output read meanwhile is heard before the
        // command's words (build 3, "Answers and speech").
        reading::speak_read(cx, None);
        // A history read that fails, the terminal's text unread included,
        // ends the command saying nothing.
        let Outcome::Result(Ok(TextReply::TerminalHistory {
            text,
            above: rows_above,
            ..
        })) = outcome
        else {
            return None;
        };
        let facts = cx.facts();
        let model = terminal_model::model_of(&facts, node)?;
        let asked = review::HistoryAsked { above, whole };
        let start_row = if whole { 0 } else { row? };
        history = Some(review::placed_history(
            model,
            asked,
            &text,
            (rows_above, start_row),
        ));
    }
}

/// A move to the previous or next command mark (`forward`) of the focused
/// terminal `node` from the review position at `from`, after the fresh
/// read (`books/dev/src/explanation/design/terminal-model.md`, "Features
/// and settings"): the line move's answer, the review position at the
/// mark's row; `None` when no mark is left that way, a search failed or
/// could not be placed, or three searches were made, the marks kept.
pub(crate) async fn command_mark(
    cx: &mut Cx<Entries>,
    (window, node): (RequestWindow, NodeId),
    (forward, from): (bool, TextPoint),
    due_us: u64,
) -> Option<TextAnswer> {
    let mut found = None;
    let mut searches = 0;
    loop {
        let search = {
            let mut facts = cx.facts_mut();
            let held = held_anchors(&facts, node);
            let mut mint = facts.next_terminal_anchor;
            let model = terminal_model::model_mut(&mut facts, node)?;
            let moved = review::mark(
                model,
                (forward, &from),
                (&mut found, &mut searches),
                &mut mint,
                &held,
            );
            facts.next_terminal_anchor = mint;
            match moved {
                Ok(outcome) => return outcome.map(TextAnswer::of_line),
                Err(search) => search,
            }
        };
        let (place, _) = place_at_start(&mut cx.facts_mut(), node)?;
        let outcome = cx
            .ask(
                FollowUp {
                    window,
                    node,
                    op: search.op,
                },
                due_us,
            )
            .await;
        let row = place_row(&mut cx.facts_mut(), node, place);
        reading::speak_read(cx, None);
        // A search that never ran ends the move, saying nothing, every mark
        // kept.
        let Outcome::Result(Ok(TextReply::TerminalFound { matches, .. })) = outcome else {
            return None;
        };
        let facts = cx.facts();
        let model = terminal_model::model_of(&facts, node)?;
        found = Some(review::placed(model, search.hash, &matches, row));
    }
}

/// "Report status bar" (`books/dev/src/explanation/design/terminal-model.md`, "Features and
/// settings"): in the focused terminal Core reads, its footer, after a
/// fresh read as every review command is, spelled on the second press and
/// copied on the third; elsewhere "No status line found", until milestone
/// M5's general status bar.
pub(crate) async fn status_bar(mut cx: Cx<Entries>, trace_id: TraceId, repeat: u8) {
    let Some(node) = reading::modelled(&cx.facts()) else {
        cx.emit(crate::editing::speak(
            trace_id,
            crate::review_text::message(Message::NoStatusLineFound),
        ));
        return;
    };
    let due_us = cx.due_in(crate::event_queue::OTHER_DUE_US);
    fresh(&mut cx, trace_id, node, due_us).await;
    let footer = {
        let facts = cx.facts();
        let Some(model) = terminal_model::model_of(&facts, node) else {
            return;
        };
        review::footer(model)
    };
    cx.emit(status_bar_said(trace_id, repeat, &footer));
}

/// What "Report status bar" says of `footer`, its lines, on press
/// `repeat`: the lines, one segment each, on the first; spelled on the
/// second, joined by a space; copied to the clipboard, joined by line
/// breaks, on the third and later; "no status bar information" when there
/// are none.
fn status_bar_said(trace_id: TraceId, repeat: u8, footer: &[String]) -> Effect {
    if footer.is_empty() {
        return crate::editing::speak(
            trace_id,
            crate::review_text::message(Message::NoStatusBarInformation),
        );
    }
    match repeat {
        0 => crate::editing::speak(
            trace_id,
            footer
                .iter()
                .map(|line| UtteranceSegment::text(line.clone()))
                .collect(),
        ),
        // Spelled as one text, its lines joined by a space as a status
        // bar's parts are (`docs/parity/terminals.md`, "Reading a
        // terminal on demand"), so the space is spelled between them.
        1 => crate::editing::speak(
            trace_id,
            crate::review_text::spell_or_blank(&footer.join(" "), false, None),
        ),
        _ => Effect::CopyToClipboard(footer.join("\r\n")),
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::TextUnit;

    use super::*;

    /// Every failure a history read or a search can be answered decodes as
    /// its answer, a terminal whose text was not read included; only a
    /// reply of another request's kind is malformed.
    #[test]
    fn a_follow_ups_failures_decode_and_another_reply_does_not() {
        let follow_up = FollowUp {
            window: crate::event_queue::RequestWindow {
                handle: verbatim_model::WindowHandle(1),
                top_level: verbatim_model::WindowHandle(1),
                root_owner: verbatim_model::WindowHandle(1),
                process: verbatim_model::Pid(1),
                thread: 1,
                by_class: None,
            },
            node: NodeId::new(1),
            op: TextOp::TerminalHistory {
                above: 0,
                whole: false,
                start: verbatim_model::TextAnchor(1),
                height: 1,
            },
        };
        for (reply, failure) in [
            (TextReply::NoText, TextFailure::NoText),
            (TextReply::Unsupported, TextFailure::Unsupported),
            (
                TextReply::UnsupportedUnit(TextUnit::Line),
                TextFailure::UnsupportedUnit(TextUnit::Line),
            ),
            (TextReply::AnchorLost, TextFailure::AnchorLost),
            (TextReply::Gone, TextFailure::Gone),
            (TextReply::Unanswered, TextFailure::Unanswered),
        ] {
            assert_eq!(
                follow_up.decode(Reply::Text(reply)),
                Some(Err(failure)),
                "{failure:?}"
            );
        }
        assert_eq!(follow_up.decode(Reply::Followed), None);
    }
}
