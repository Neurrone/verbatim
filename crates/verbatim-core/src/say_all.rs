//! Say-all (milestone M4; `docs/nvda/speech.md`, "Say-all"): continuous
//! reading from the caret or the review cursor, moving it as speech goes.
//!
//! Reading goes by the "Say all reads by" setting: by sentence where the
//! text can be split into sentences, by paragraph, or by line. A provider
//! that NVDA splits into sentences itself answers a sentence read with the
//! paragraph, which Core splits by Unicode's sentence rules
//! (`verbatim-text`); a provider with no sentence unit (UIA) answers that
//! it has none, and reading goes by line, as it always does in a terminal.
//!
//! Each read asks for a batch of units ahead ([`READ_AHEAD`],
//! `TextOp::ReadAhead`), one round trip where the provider runs remote
//! operations. The batch's pieces wait in a buffer and are handed to speech
//! one at a time, [`HANDED`] ahead of playback, each with its own index
//! mark; when playback reaches a mark, the caret or review cursor moves to
//! its piece and the next piece is handed on. The next batch is read once
//! fewer than [`LOW_WATER`] pieces are left to speak, handed out and
//! buffered together. Any key stops it, dropping the buffer and leaving the
//! cursor where reading stopped, and the display is kept on while it reads.

use verbatim_model::{
    Effect, NodeId, SayAllUnit, SegmentContent, SpeechMark, SpeechPriority, TextChunk,
    TextMovement, TextOp, TextPoint, TextPosition, TextReadAhead, TextReply, TextRequest, TextUnit,
    TraceId, Utterance, UtteranceSegment,
};
use verbatim_text::Segmenter;

use crate::editing::is_grid;
use crate::state::{BufferedPiece, ReviewText, SayAll, SharedChunk, SrState};
use crate::text;

/// How many units each read asks for: a line or a sentence each, a batch
/// lasts tens of seconds of speech, so a round trip is rare.
pub(crate) const READ_AHEAD: u8 = 20;

/// How many pieces are handed to speech ahead of playback: two queued
/// behind the one playing, so speech never waits for Core between pieces.
pub(crate) const HANDED: usize = 2;

/// The next batch is read once fewer than this many pieces are left to
/// speak, handed to speech and buffered together (`docs/performance.md`,
/// "Say-all"): even short pieces last far longer than a batch's read
/// takes, classically against a busy provider, so speech never runs dry.
pub(crate) const LOW_WATER: usize = 10;

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
        buffer: std::collections::VecDeque::new(),
        trace: None,
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

/// Asks for the next batch: the first at the starting point, each later one
/// a unit on from the last chunk read.
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
        op: TextOp::ReadAhead(TextReadAhead {
            at,
            movement,
            unit: say_all.unit,
            count: READ_AHEAD,
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

/// Handles the answer to say-all's read: buffers the chunks' pieces, hands
/// speech what it can take, and reads on when little is left to speak;
/// ends at the document's end, and on any failure.
pub(crate) fn reply(state: &mut SrState, trace_id: TraceId, reply: TextReply) -> Vec<Effect> {
    let Some(say_all) = state.say_all.as_mut() else {
        return Vec::new();
    };
    say_all.pending = None;
    let (moved, chunks) = match reply {
        TextReply::Chunks { moved, chunks } => (moved, chunks),
        TextReply::Read { moved, chunk } => (moved, vec![chunk]),
        TextReply::UnsupportedUnit(_) if say_all.unit != TextUnit::Line => {
            say_all.unit = TextUnit::Line;
            return read_next(state);
        }
        _ => return stop(state),
    };
    say_all.trace = Some(trace_id);
    if (say_all.last_chunk.is_some() && moved == 0) || chunks.is_empty() {
        say_all.finished = true;
        return end_if_done(state);
    }
    let grid = state
        .navigator
        .as_ref()
        .filter(|navigator| navigator.object.id == say_all.node)
        .is_some_and(|navigator| is_grid(navigator.object.role));
    let mut first = say_all.last_chunk.is_none();
    for chunk in chunks {
        let by_sentence = say_all.unit == TextUnit::Sentence && chunk.unit == TextUnit::Paragraph;
        let from = if first { chunk.offset as usize } else { 0 };
        first = false;
        say_all.last_chunk = Some((TextPosition::at(chunk.start), chunk.unit));
        say_all.finished |= chunk.last;
        let chunk = SharedChunk::new(chunk);
        for range in pieces(&chunk, from, by_sentence, grid) {
            let start = u32::try_from(range.start).unwrap_or(u32::MAX);
            let end = u32::try_from(range.end).unwrap_or(u32::MAX);
            say_all.buffer.push_back(BufferedPiece {
                chunk: SharedChunk::clone(&chunk),
                start,
                end,
            });
        }
    }
    let mut effects = hand_out(state);
    effects.extend(read_on_or_end(state));
    effects
}

/// Hands speech the buffered pieces it can take, each in its own utterance
/// starting with its own index mark, keeping [`HANDED`] with speech.
fn hand_out(state: &mut SrState) -> Vec<Effect> {
    let mut effects = Vec::new();
    loop {
        let Some(say_all) = state.say_all.as_mut() else {
            return effects;
        };
        let Some(trace_id) = say_all.trace else {
            return effects;
        };
        if say_all.queued.len() >= HANDED {
            return effects;
        }
        let Some(piece) = say_all.buffer.pop_front() else {
            return effects;
        };
        let mark = state.allocate_mark();
        let (chunk, start) = (&piece.chunk, piece.start);
        let text = piece.text();
        let mut segments = vec![UtteranceSegment::new(SegmentContent::Mark(mark))];
        segments.extend(text::text_segments(text, chunk.language_at(start as usize)));
        // Read by say-all, so the theme's "play sounds during say all"
        // setting applies to it.
        effects.push(Effect::Speak(Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: None,
            validity: None,
            say_all: true,
        }));
        let position = TextPosition {
            anchor: chunk.start,
            offset: start,
        };
        if let Some(say_all) = state.say_all.as_mut() {
            say_all.queued.push_back((mark, position));
        }
    }
}

/// Ends say-all once everything is read and heard, or reads the next batch
/// when fewer than [`LOW_WATER`] pieces are left to speak.
fn read_on_or_end(state: &mut SrState) -> Vec<Effect> {
    let Some(say_all) = &state.say_all else {
        return Vec::new();
    };
    if say_all.finished {
        return end_if_done(state);
    }
    if say_all.pending.is_none() && say_all.queued.len() + say_all.buffer.len() < LOW_WATER {
        return read_next(state);
    }
    Vec::new()
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
    let from = text::boundary(content, from);
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
/// cursor to the piece it starts, hands speech the next piece, reads on
/// when little is left, and ends say-all after the last piece.
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
    effects.extend(hand_out(state));
    effects.extend(read_on_or_end(state));
    effects
}

/// Ends say-all once the document's end has been read and every piece has
/// been handed out and reached.
fn end_if_done(state: &mut SrState) -> Vec<Effect> {
    match &state.say_all {
        Some(say_all)
            if say_all.finished && say_all.queued.is_empty() && say_all.buffer.is_empty() =>
        {
            stop(state)
        }
        _ => Vec::new(),
    }
}
