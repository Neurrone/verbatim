//! Commands on the focused terminal (`books/dev/src/explanation/design/terminal-model.md`,
//! "Moving by command" and "Report status bar"; build 9's design, 5.6):
//! each reads the window first, as a key's read of the watch's, then acts
//! on live reads. A command mark is a place counted from the document's
//! end with its line's text: tried at its place by one line read, and
//! otherwise found by a search outward from it, the request list's
//! `FindText` each way from the mark's row.

use verbatim_model::batch::Chunks;
use verbatim_model::{
    Effect, Message, NodeId, TextAnchor, TextDetails, TextMovement, TextPoint, TextPosition,
    TextUnit, TraceId, UtteranceSegment,
};

use crate::batch::Got;
use crate::entries::Entries;
use crate::event_queue::{Cx, Outcome, Reader, RequestWindow};
use crate::terminal::places::RowPlace;
use crate::terminal::{ReadSpeech, Watch, focus_of, focused, key_read, places_mut, places_of};
use crate::terminal_window::{Match, add_search, matches_of};

/// How many searches a press makes at most; the marks not yet tried are
/// kept once they are spent.
const SEARCHES: u32 = 3;

/// The search for `needle` in terminal `node` outward from the row `from`
/// rows above `start`, its end mark ([`crate::terminal_window::add_search`]):
/// the matches, counted from the mark; `None` when it could not run.
async fn search(
    cx: &mut Cx<Entries>,
    (window, node): (RequestWindow, NodeId),
    (start, from): (TextAnchor, i32),
    needle: &str,
    due_us: u64,
) -> Option<Vec<Match>> {
    let mut batch = cx.batch(|| ());
    let steps = add_search(&mut batch, node, (start, from), needle)?;
    let request = batch.finish().of(Reader::Outpost(window.process), window);
    let Outcome::Result(answers) = cx.ask(request, due_us).await else {
        return None;
    };
    matches_of(&answers, &steps)
}

/// The line `above` rows above `start`, terminal `node`'s end mark, read
/// live: its answer as a line move's. Counted from the mark Core names,
/// not from the document's end, so output written since the last read
/// moves nothing.
async fn read_row(
    cx: &mut Cx<Entries>,
    (window, node): (RequestWindow, NodeId),
    (start, above): (TextAnchor, u32),
    due_us: u64,
) -> Option<Chunks> {
    let mut batch = cx.batch(|| ());
    let movement = TextMovement {
        unit: TextUnit::Line,
        count: 0i32.checked_sub(i32::try_from(above).ok()?)?,
    };
    let line = batch.read_text(
        node,
        TextPoint::At(TextPosition::at(start)).into(),
        (above > 0).then_some(movement),
        TextUnit::Line,
        1,
        TextDetails::NONE,
    );
    let request = batch.finish().of(Reader::Outpost(window.process), window);
    let Outcome::Result(answers) = cx.ask(request, due_us).await else {
        return None;
    };
    match answers.get(line) {
        Got::Done(read) => Some(read.clone()),
        Got::Refused(_) | Got::NotThere(_) | Got::Failed | Got::Skipped => None,
    }
}

/// Terminal `node`'s end mark and the rows its places have moved by so
/// far, the frame a place's rows are counted in when a request is sent.
fn frame(cx: &Cx<Entries>, node: NodeId) -> Option<(TextAnchor, u64)> {
    let facts = cx.facts();
    let places = places_of(&facts, node)?;
    Some((places.end_anchor()?, places.written()))
}

/// `above`, counted in the frame `written`, in terminal `node`'s frame now:
/// moved up by every row the reads since moved the places, `None` once
/// that is past what a count can hold.
fn moved_since(cx: &Cx<Entries>, node: NodeId, (above, written): (u32, u64)) -> Option<u32> {
    let facts = cx.facts();
    let now = places_of(&facts, node)?.written();
    let by = u32::try_from(now.saturating_sub(written)).ok()?;
    above.checked_add(by).filter(|&above| above < u32::MAX)
}

/// Whether `answer`'s line reads `text`.
fn reads(answer: &Chunks, text: &str) -> bool {
    answer
        .chunks
        .first()
        .is_some_and(|line| line.text.trim_end() == text)
}

/// The place a move by command counts from: the review position a mark
/// move left, or the caret's line.
fn from_place(cx: &Cx<Entries>, node: NodeId) -> Option<u32> {
    let facts = cx.facts();
    if let Some(review) = places_of(&facts, node).and_then(|places| places.review.as_ref()) {
        return Some(review.above);
    }
    match &focus_of(&facts)?.watch {
        Watch::Live { old, .. } => old.caret_line().map(|(_, above)| above),
        Watch::Unread { .. } | Watch::Baselining { .. } | Watch::Gone => None,
    }
}

/// The nearest mark of `node` past `from` going `forward` (down, nearer
/// the end) or back, taken out of the marks.
fn next_mark(cx: &mut Cx<Entries>, node: NodeId, (from, forward): (u32, bool)) -> Option<RowPlace> {
    let mut facts = cx.facts_mut();
    let places = places_mut(&mut facts, node);
    let at = places
        .marks
        .iter()
        .enumerate()
        .filter(|(_, mark)| {
            if forward {
                mark.above < from
            } else {
                mark.above > from
            }
        })
        .min_by_key(|(_, mark)| mark.above.abs_diff(from))
        .map(|(at, _)| at)?;
    places.marks.remove(at)
}

/// A move to the previous or next command mark (`forward`) of the focused
/// terminal `node`, under `trace`: the line read at the mark, the review
/// position left there; `None` when no mark is left that way, its window
/// read did not run, or every search was spent, the marks kept.
pub(crate) async fn command_mark(
    cx: &mut Cx<Entries>,
    (window, node): (RequestWindow, NodeId),
    (forward, trace): (bool, TraceId),
    due_us: u64,
) -> Option<Chunks> {
    key_read(cx, (node, window), (trace, ReadSpeech::Now), due_us).await?;
    let from = from_place(cx, node)?;
    let mut searches = 0;
    loop {
        let mut mark = next_mark(cx, node, (from, forward))?;
        // The mark's rows are counted in the frame it was taken in; each
        // request counts from the end mark of the frame it is sent in.
        let Some((start, taken)) = frame(cx, node) else {
            places_mut(&mut cx.facts_mut(), node).mark(mark);
            return None;
        };
        let mut sent = taken;
        let mut answer = read_row(cx, (window, node), (start, mark.above), due_us).await;
        if !answer
            .as_ref()
            .is_some_and(|answer| reads(answer, &mark.text))
        {
            let Some(above) = moved_since(cx, node, (mark.above, taken)) else {
                continue;
            };
            mark.above = above;
            if searches == SEARCHES {
                places_mut(&mut cx.facts_mut(), node).mark(mark);
                return None;
            }
            searches = searches.saturating_add(1);
            let Some((start, search_frame)) = frame(cx, node) else {
                places_mut(&mut cx.facts_mut(), node).mark(mark);
                return None;
            };
            let from = i32::try_from(mark.above).unwrap_or(i32::MAX);
            // A search that never ran ends the move, every mark kept.
            let Some(matches) = search(cx, (window, node), (start, from), &mark.text, due_us).await
            else {
                if let Some(above) = moved_since(cx, node, (mark.above, search_frame)) {
                    mark.above = above;
                    places_mut(&mut cx.facts_mut(), node).mark(mark);
                }
                return None;
            };
            let found = matches
                .iter()
                .filter(|found| found.text.trim_end().starts_with(mark.text.as_str()))
                .filter_map(|found| u32::try_from(found.above).ok())
                .min_by_key(|above| above.abs_diff(mark.above));
            // A mark not found is dropped, and the next tried.
            let Some(found) = found else {
                continue;
            };
            // The search counted from the end mark it named: placed in the
            // frame now.
            let Some(above) = moved_since(cx, node, (found, search_frame)) else {
                continue;
            };
            let (start, now) = frame(cx, node)?;
            mark.above = above;
            sent = now;
            answer = read_row(cx, (window, node), (start, above), due_us).await;
        }
        let Some(answer) = answer.filter(|answer| reads(answer, &mark.text)) else {
            continue;
        };
        mark.above = moved_since(cx, node, (mark.above, sent))?;
        let mut facts = cx.facts_mut();
        let places = places_mut(&mut facts, node);
        places.review = Some(mark.clone());
        places.mark(mark);
        return Some(answer);
    }
}

/// "Report status bar": in the focused terminal, its footer after the
/// window's read (`verbatim_text::terminal_window::Reading::footer`: the
/// lines below the caret, else the rows at the screen's bottom the program
/// rewrote, else the last line with text), spelled on the second press
/// and copied on the third;
/// elsewhere "No status line found", until milestone M5's general status
/// bar. A read that brings nothing says nothing.
pub(crate) async fn status_bar(mut cx: Cx<Entries>, trace_id: TraceId, repeat: u8) {
    let Some(terminal) = focused(&cx.facts()) else {
        cx.emit(crate::editing::speak(
            trace_id,
            crate::review_text::message(Message::NoStatusLineFound),
        ));
        return;
    };
    let due_us = cx.due_in(crate::event_queue::OTHER_DUE_US);
    let read = key_read(
        &mut cx,
        (terminal.node, terminal.window),
        (trace_id, ReadSpeech::Now),
        due_us,
    )
    .await;
    if !read.is_some_and(|read| read.answered) {
        return;
    }
    let footer = {
        let facts = cx.facts();
        let Some(focus) = focus_of(&facts) else {
            return;
        };
        let Watch::Live { old, .. } = &focus.watch else {
            return;
        };
        focus.reading.footer(old)
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
        // bar's parts are (`docs/parity/terminals.md`, "Reading a terminal
        // on demand"), so the space is spelled between them.
        1 => crate::editing::speak(
            trace_id,
            crate::review_text::spell_or_blank(&footer.join(" "), false, None),
        ),
        _ => Effect::CopyToClipboard(footer.join("\r\n")),
    }
}
