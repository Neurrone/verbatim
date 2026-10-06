//! Say-all (milestone M4; `docs/nvda/speech.md`, "Say-all"): continuous
//! reading from the caret or the review cursor, moving it as speech goes.
//!
//! Reading goes one chunk at a time, by the "Say all reads by" setting:
//! by sentence where the text can be split into sentences, by paragraph,
//! or by line. A provider that NVDA splits into sentences itself answers a
//! sentence read with the paragraph, which Core splits by Unicode's
//! sentence rules (`verbatim-text`); a provider with no sentence unit (UIA)
//! answers that it has none, and reading goes by line, as it always does in
//! a terminal. Each spoken piece starts with an index mark; when playback
//! reaches it, the caret or review cursor moves to the piece, and when
//! little is left queued the next chunk is read, so the lookahead stays
//! bounded. Any key stops it, leaving the cursor where reading stopped,
//! and the display is kept on while it reads.

use verbatim_model::{
    Effect, NodeId, SayAllUnit, SegmentContent, SpeechMark, TextChunk, TextMovement, TextOp,
    TextPoint, TextPosition, TextRead, TextReply, TextRequest, TextUnit, TraceId, UtteranceSegment,
};
use verbatim_text::Segmenter;

use crate::editing::{is_grid, speak};
use crate::state::{ReviewText, SayAll, SrState};
use crate::text;

/// The next chunk is read once no more than this many spoken pieces are
/// still waiting for their marks.
const LOOKAHEAD: usize = 1;

/// Starts reading `node` from `at`, moving the caret as it goes when
/// `moves_caret`, the review cursor otherwise. Stops a say-all already
/// running.
pub(crate) fn start(
    state: &mut SrState,
    node: NodeId,
    moves_caret: bool,
    at: TextPoint,
) -> Vec<Effect> {
    let mut effects = stop(state);
    let grid = state
        .navigator
        .as_ref()
        .filter(|navigator| navigator.object.id == node)
        .map(|navigator| navigator.object.role)
        .or_else(|| {
            state
                .focus
                .as_ref()
                .filter(|focus| focus.snapshot.id == node)
                .map(|focus| focus.snapshot.role)
        })
        .is_some_and(is_grid);
    let unit = match state.settings.say_all_unit {
        // Terminals' larger units span the whole buffer: line is the
        // largest used there.
        _ if grid => TextUnit::Line,
        SayAllUnit::Sentence => TextUnit::Sentence,
        SayAllUnit::Paragraph => TextUnit::Paragraph,
        SayAllUnit::Line => TextUnit::Line,
    };
    let display_held = state.settings.keep_display_on;
    if display_held {
        effects.push(Effect::KeepDisplayOn(true));
    }
    state.say_all = Some(SayAll {
        node,
        moves_caret,
        start: at,
        unit,
        pending: None,
        last_chunk: None,
        queued: std::collections::VecDeque::new(),
        finished: false,
        display_held,
    });
    effects.extend(read_next(state));
    effects
}

/// Stops say-all, if it is running, leaving the cursor where reading got
/// to.
pub(crate) fn stop(state: &mut SrState) -> Vec<Effect> {
    match state.say_all.take() {
        Some(say_all) if say_all.display_held => vec![Effect::KeepDisplayOn(false)],
        _ => Vec::new(),
    }
}

/// Asks for the next chunk: the first at the starting point, each later one
/// a unit on from the last.
fn read_next(state: &mut SrState) -> Vec<Effect> {
    let query_id = state.allocate_query_id();
    let Some(say_all) = state.say_all.as_mut() else {
        return Vec::new();
    };
    let (at, movement) = match say_all.last_chunk {
        None => (say_all.start, None),
        Some((position, unit)) => (
            TextPoint::At(position),
            Some(TextMovement { unit, count: 1 }),
        ),
    };
    say_all.pending = Some(query_id);
    vec![Effect::Text(TextRequest {
        query_id,
        node_id: say_all.node,
        op: TextOp::Read(TextRead {
            at,
            movement,
            unit: say_all.unit,
        }),
    })]
}

/// Whether `query_id` is say-all's read in flight.
pub(crate) fn is_pending(state: &SrState, query_id: verbatim_model::QueryId) -> bool {
    state
        .say_all
        .as_ref()
        .is_some_and(|say_all| say_all.pending == Some(query_id))
}

/// Handles the answer to say-all's read: speaks the chunk's pieces, each
/// after its mark, and reads on when little is queued; ends at the
/// document's end, and on any failure.
pub(crate) fn reply(state: &mut SrState, trace_id: TraceId, reply: TextReply) -> Vec<Effect> {
    let Some(say_all) = state.say_all.as_mut() else {
        return Vec::new();
    };
    say_all.pending = None;
    let chunk = match reply {
        TextReply::Read { moved, chunk } => {
            if say_all.last_chunk.is_some() && moved == 0 {
                say_all.finished = true;
                return end_if_done(state);
            }
            chunk
        }
        TextReply::UnsupportedUnit(_) if say_all.unit != TextUnit::Line => {
            say_all.unit = TextUnit::Line;
            return read_next(state);
        }
        _ => return stop(state),
    };
    let first = say_all.last_chunk.is_none();
    let by_sentence = say_all.unit == TextUnit::Sentence && chunk.unit == TextUnit::Paragraph;
    let grid = state
        .navigator
        .as_ref()
        .filter(|navigator| Some(navigator.object.id) == state.say_all.as_ref().map(|s| s.node))
        .is_some_and(|navigator| is_grid(navigator.object.role));
    let from = if first { chunk.offset as usize } else { 0 };
    let pieces = pieces(&chunk, from, by_sentence, grid);
    let mut effects = Vec::new();
    for range in pieces {
        let mark = state.allocate_mark();
        let position = TextPosition {
            anchor: chunk.start,
            offset: u32::try_from(range.start).unwrap_or(u32::MAX),
        };
        let mut segments = vec![UtteranceSegment::new(SegmentContent::Mark(mark))];
        segments.extend(text::text_segments(
            &chunk.text[range.clone()],
            chunk.language_at(range.start),
        ));
        effects.push(speak(trace_id, segments));
        if let Some(say_all) = state.say_all.as_mut() {
            say_all.queued.push_back((mark, position));
        }
    }
    let Some(say_all) = state.say_all.as_mut() else {
        return effects;
    };
    say_all.last_chunk = Some((TextPosition::at(chunk.start), chunk.unit));
    if chunk.last {
        say_all.finished = true;
    }
    if say_all.finished {
        effects.extend(end_if_done(state));
    } else if say_all.queued.len() <= LOOKAHEAD {
        effects.extend(read_next(state));
    }
    effects
}

/// The pieces of a chunk to speak from byte `from`, each a byte range of
/// its text with something to read: its sentences when a paragraph is read
/// by sentence, the whole rest of it otherwise.
fn pieces(
    chunk: &TextChunk,
    from: usize,
    by_sentence: bool,
    grid: bool,
) -> Vec<std::ops::Range<usize>> {
    let content = text::line_content(&chunk.text, grid);
    let from = from.min(content.len());
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    if by_sentence {
        ranges.extend(
            Segmenter::new()
                .sentences(content)
                .into_iter()
                .filter(|range| range.end > from)
                .map(|range| range.start.max(from)..range.end),
        );
    } else {
        ranges.push(from..content.len());
    }
    ranges.retain(|range| !text::is_blank(&content[range.clone()]));
    ranges
}

/// Handles playback reaching an index mark: moves the caret or review
/// cursor to the piece it starts, reads on when little is left queued, and
/// ends say-all after the last piece.
pub(crate) fn mark_reached(state: &mut SrState, mark: SpeechMark) -> Vec<Effect> {
    let Some(say_all) = state.say_all.as_mut() else {
        return Vec::new();
    };
    if !say_all.queued.iter().any(|(queued, _)| *queued == mark) {
        return Vec::new();
    }
    let mut reached = None;
    while let Some(&(queued, position)) = say_all.queued.front()
        && queued <= mark
    {
        say_all.queued.pop_front();
        reached = Some(position);
    }
    let node = say_all.node;
    let moves_caret = say_all.moves_caret;
    let mut effects = Vec::new();
    if let Some(position) = reached {
        if moves_caret {
            let query_id = state.allocate_query_id();
            effects.push(Effect::Text(TextRequest {
                query_id,
                node_id: node,
                op: TextOp::MoveCaret(TextPoint::At(position)),
            }));
        } else if let Some(navigator) = state
            .navigator
            .as_mut()
            .filter(|navigator| navigator.object.id == node)
        {
            navigator.text = ReviewText::Point(position);
        }
    }
    let Some(say_all) = state.say_all.as_ref() else {
        return effects;
    };
    if say_all.finished {
        effects.extend(end_if_done(state));
    } else if say_all.queued.len() <= LOOKAHEAD && say_all.pending.is_none() {
        effects.extend(read_next(state));
    }
    effects
}

/// Ends say-all once the document's end has been read and every piece has
/// been reached.
fn end_if_done(state: &mut SrState) -> Vec<Effect> {
    match &state.say_all {
        Some(say_all) if say_all.finished && say_all.queued.is_empty() => stop(state),
        _ => Vec::new(),
    }
}
