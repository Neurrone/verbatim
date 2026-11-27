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
//! Each read asks for a batch of units ahead ([`READ_AHEAD`], one
//! `ReadText` of that many units), one round trip where the provider runs remote
//! operations. Every unit read is spoken without pauses
//! (`docs/nvda/speech.md`, "Say-all speaks without pauses"): an utterance
//! ends at the last sentence end in a unit, and what follows it is held
//! back to start the next utterance, so a sentence that runs from one line
//! to the next is spoken whole, and a line holding two sentences' parts is
//! split between them. Ten units in a row with no sentence end
//! ([`MAX_HELD`]) are spoken together, as is what is held back when the
//! text ends. Each unit's start carries an index mark, inside its
//! utterance where the unit starts there.
//!
//! The utterances wait in a buffer and are handed to speech one at a time,
//! [`HANDED`] ahead of playback, each opening with an index mark; when
//! playback reaches a unit's mark, the caret or review cursor moves to the
//! unit's start, and when it reaches an utterance's first mark, the next
//! utterance is handed on. The next batch is read once fewer than
//! [`LOW_WATER`] utterances are left to speak, handed out and buffered
//! together. Any key stops it, dropping the buffer and leaving the cursor
//! at the start of the last unit playback reached, and the display is kept
//! on while it reads.
//!
//! Started by a press at its turn in the event queue
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Core's requests"),
//! each read is an entry of its own that joins the back when the read is
//! wanted and asks the node's window as the queue's request, and each
//! caret move is a request of the index mark's entry. Its speech carries
//! its starting key's number. Stopping it withdraws a read out, and a read
//! still waiting ends at its turn having asked nothing; the end of the
//! outpost generation holding its positions stops it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use verbatim_model::batch::Chunks;
use verbatim_model::{
    Effect, KeySeq, Message, NodeId, SayAllUnit, SegmentContent, SpeechMark, SpeechPriority,
    TextChunk, TextDetails, TextMovement, TextPoint, TextPosition, TextUnit, TraceId, Utterance,
    UtteranceSegment,
};
use verbatim_text::Segmenter;

use crate::editing::is_grid;
use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US, RequestWindow};
use crate::keys::entry::Fence;
use crate::state::{
    BufferedPiece, QueuedMark, ReviewText, SayAll, SharedChunk, SrState, UnitStart,
};
use crate::text;
use crate::text_request::{Answerer, CaretTarget, Missed};

/// How many units each read asks for: a line or a sentence each, a batch
/// lasts tens of seconds of speech, so a round trip is rare.
pub(crate) const READ_AHEAD: u8 = 20;

/// How many utterances are handed to speech ahead of playback: two queued
/// behind the one playing, so speech never waits for Core between them.
pub(crate) const HANDED: usize = 2;

/// The next batch is read once fewer than this many utterances are left to
/// speak, handed to speech and buffered together (`books/dev/src/reference/performance.md`,
/// "Say-all"): even short ones last far longer than a batch's read takes,
/// classically against a busy provider, so speech never runs dry.
pub(crate) const LOW_WATER: usize = 10;

/// How many units in a row with no sentence end are held back before they
/// are spoken anyway (`docs/nvda/speech.md`, "Say-all speaks without
/// pauses").
pub(crate) const MAX_HELD: usize = 10;

/// A say-all started in the event queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QueueRun {
    /// The trace of the press that started it, which names it: a read
    /// entry of another say-all finds it is not its own.
    pub(crate) trace: TraceId,
    /// Its starting key's number, which all its speech carries, so a key
    /// pressed after it fences off what it hands to speech later.
    pub(crate) key: Option<KeySeq>,
    /// The window of the node it reads, which its requests name.
    pub(crate) window: RequestWindow,
    /// Where its next read is.
    pub(crate) read: QueueRead,
}

/// Where a say-all's next read is, in the event queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum QueueRead {
    /// No read is wanted or waiting.
    Idle,
    /// A read is wanted: its entry joins the back at the end of the step.
    Wanted,
    /// Its entry is in the queue, waiting or asking.
    Joined,
}

/// A caret move playback reaching a unit asks: to `position` in `node`,
/// whose line `line` becomes Core's caret line once it is made.
#[derive(Debug)]
struct CaretMove {
    node: NodeId,
    position: TextPosition,
    line: TextChunk,
}

/// Say-all from the focus's caret or the review cursor, pressed as
/// `press` and started at its turn in the event queue: reads `node` from
/// `at`, moving the caret as it goes when `moves_caret`, the review cursor
/// otherwise, its reads entries of their own ([`read_entry`]). Stops a
/// say-all already running.
pub(crate) fn start_in_queue(
    state: &mut SrState,
    (node, window): (NodeId, RequestWindow),
    (moves_caret, at): (bool, TextPoint),
    (trace, fence): (TraceId, Fence),
) -> Vec<Effect> {
    let run = QueueRun {
        trace,
        key: fence.key(),
        window,
        read: QueueRead::Idle,
    };
    let mut effects = stop(state);
    let grid = state
        .navigator_object()
        .filter(|object| object.id == node)
        .map(|object| object.role)
        .or_else(|| {
            state
                .versions
                .focus()
                .map(|focus| &focus.context)
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
        last_chunk: None,
        queued: std::collections::VecDeque::new(),
        buffer: std::collections::VecDeque::new(),
        held: Vec::new(),
        held_units: 0,
        finished: false,
        display_held,
        run,
    });
    read_next(state);
    effects
}

/// Say-all from the caret, pressed under `trace_id` with the speech fence
/// `fence`, at its turn in the event queue: from the committed focus's
/// caret as the outpost finds it; "no caret" with no focus a reader holds,
/// and nothing on a focus that cannot have text, as NVDA's say-all says
/// nothing where no caret position can be made (`speech/sayAll.py` 419 to
/// 423, 137 to 139; Dickson, 2026-11-19). Verbatim judges "cannot have
/// text" by role, NVDA by whether a caret position can be made: where they
/// differ is recorded in `docs/parity/review.md`.
pub(crate) fn from_caret(cx: &mut Cx<Entries>, trace_id: TraceId, fence: Fence) {
    let target = CaretTarget::of(&cx.facts());
    let effects = match target {
        CaretTarget::NoCaret => vec![crate::editing::speak(
            trace_id,
            crate::review_text::message(Message::NoCaret),
        )],
        CaretTarget::NoText => Vec::new(),
        CaretTarget::Text(text) => start_in_queue(
            &mut cx.facts_mut(),
            (text.node, text.window),
            (true, TextPoint::Caret),
            (trace_id, fence),
        ),
    };
    for effect in effects {
        cx.emit(effect);
    }
}

/// The trace naming the say-all whose read entry is wanted, which then
/// joins the back; `None` when no read is wanted.
pub(crate) fn joining(state: &mut SrState) -> Option<TraceId> {
    let run = match state.say_all.as_mut().map(|say_all| &mut say_all.run) {
        Some(run) if run.read == QueueRead::Wanted => run,
        _ => return None,
    };
    run.read = QueueRead::Joined;
    Some(run.trace)
}

/// Whether the read entry of the say-all traced `run` is no longer needed:
/// that say-all stopped. Its request out is withdrawn.
pub(crate) fn read_unneeded(state: &SrState, run: TraceId) -> bool {
    state
        .say_all
        .as_ref()
        .is_none_or(|say_all| say_all.run.trace != run)
}

/// A say-all read at its turn as an event-queue entry: the next batch of
/// the say-all traced `run`, asked of its node's window as the queue's
/// request. A say-all that stopped before its turn asks nothing; any
/// outcome but a result or the text not there stops it, as a failed read
/// does.
pub(crate) async fn read_entry(mut cx: Cx<Entries>, run: TraceId) {
    let (node, window, (at, movement, unit)) = {
        let facts = cx.facts();
        let Some(say_all) = facts.say_all.as_ref() else {
            return;
        };
        let own = &say_all.run;
        if own.trace != run {
            return;
        }
        (say_all.node, own.window, next_read(say_all))
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let mut sent = crate::batch::batch(|| ());
    let read = sent.read_text(
        node,
        at.into(),
        movement,
        unit,
        READ_AHEAD,
        TextDetails::NONE,
    );
    let answers =
        crate::text_request::ask(&mut cx, Answerer::Outpost(window), sent.finish(), due_us).await;
    let effects = {
        let mut facts = cx.facts_mut();
        let Some(own) = facts
            .say_all
            .as_mut()
            .map(|say_all| &mut say_all.run)
            .filter(|own| own.trace == run)
        else {
            return;
        };
        own.read = QueueRead::Idle;
        match answers {
            Some(answers) => reply(&mut facts, crate::text_request::got(&answers, read)),
            None => stop(&mut facts),
        }
    };
    for effect in effects {
        cx.emit(effect);
    }
}

/// Stops say-all, if it is running, leaving the cursor where reading got
/// to.
pub(crate) fn stop(state: &mut SrState) -> Vec<Effect> {
    match state.say_all.take() {
        Some(say_all) if say_all.display_held => vec![Effect::KeepDisplayOn(false)],
        _ => Vec::new(),
    }
}

/// Where the next batch's read starts, how it moves first and the unit it
/// reads: the first at the starting point, each later one a unit on from
/// the last chunk read.
fn next_read(say_all: &SayAll) -> (TextPoint, Option<TextMovement>, TextUnit) {
    let (at, movement) = match say_all.last_chunk {
        None => (say_all.start, None),
        Some((position, unit)) => (
            TextPoint::At(position),
            Some(TextMovement { unit, count: 1 }),
        ),
    };
    (at, movement, say_all.unit)
}

/// Asks for the next batch: in the event queue, its entry is wanted and
/// joins the back at the end of the step ([`joining`]).
fn read_next(state: &mut SrState) {
    if let Some(say_all) = state.say_all.as_mut()
        && say_all.run.read == QueueRead::Idle
    {
        say_all.run.read = QueueRead::Wanted;
    }
}

/// Handles the answer to say-all's read: buffers the chunks' pieces, hands
/// speech what it can take, and reads on when little is left to speak;
/// ends at the document's end, and on any failure.
pub(crate) fn reply(state: &mut SrState, answer: Result<&Chunks, Missed>) -> Vec<Effect> {
    let grid = state.say_all.as_ref().is_some_and(|say_all| {
        state
            .navigator_object()
            .filter(|object| object.id == say_all.node)
            .is_some_and(|object| is_grid(object.role))
    });
    let Some(say_all) = state.say_all.as_mut() else {
        return Vec::new();
    };
    let (moved, chunks) = match answer {
        Ok(read) => (read.moved, read.chunks.clone()),
        Err(Missed::Refused(verbatim_model::batch::StepRefusal::NoSuchUnit))
            if say_all.unit != TextUnit::Line =>
        {
            say_all.unit = TextUnit::Line;
            read_next(state);
            return Vec::new();
        }
        _ => return stop(state),
    };
    if (say_all.last_chunk.is_some() && moved == 0) || chunks.is_empty() {
        say_all.finished = true;
    } else {
        let mut first = say_all.last_chunk.is_none();
        for chunk in chunks {
            let by_sentence =
                say_all.unit == TextUnit::Sentence && chunk.unit == TextUnit::Paragraph;
            let from = if first {
                text::byte_offset(chunk.offset)
            } else {
                0
            };
            first = false;
            say_all.last_chunk = Some((TextPosition::at(chunk.start), chunk.unit));
            say_all.finished |= chunk.last;
            let chunk = SharedChunk::new(chunk);
            let units = pieces(&chunk, from, by_sentence, grid);
            if units.is_empty() {
                // A blank unit says nothing, and counts among those with
                // no sentence end.
                hold(say_all, None);
            }
            for range in units {
                hold(
                    say_all,
                    Some(BufferedPiece {
                        chunk: SharedChunk::clone(&chunk),
                        start: u32::try_from(range.start).unwrap_or(u32::MAX),
                        end: u32::try_from(range.end).unwrap_or(u32::MAX),
                        marked: true,
                    }),
                );
            }
        }
    }
    if say_all.finished {
        release(say_all);
    }
    let mut effects = hand_out(state);
    effects.extend(read_on_or_end(state));
    effects
}

/// Speaks a unit read without pauses (`docs/nvda/speech.md`, "Say-all
/// speaks without pauses"), `None` for a blank one: what is held back and
/// the unit up to its last sentence end become an utterance, and what
/// follows that end is held back; a unit with no sentence end is held back
/// whole, until [`MAX_HELD`] units in a row have none.
fn hold(say_all: &mut SayAll, unit: Option<BufferedPiece>) {
    let pause = unit
        .as_ref()
        .and_then(|unit| verbatim_text::last_pause(unit.text()));
    match (unit, pause) {
        (Some(unit), Some(pause)) => {
            let split = unit
                .start
                .saturating_add(u32::try_from(pause).unwrap_or(u32::MAX));
            let rest = (split < unit.end).then(|| BufferedPiece {
                chunk: SharedChunk::clone(&unit.chunk),
                start: split,
                end: unit.end,
                marked: false,
            });
            say_all.held.push(BufferedPiece { end: split, ..unit });
            release(say_all);
            say_all.held.extend(rest);
        }
        (unit, _) => {
            say_all.held.extend(unit);
            say_all.held_units = say_all.held_units.saturating_add(1);
            if say_all.held_units >= MAX_HELD {
                release(say_all);
            }
        }
    }
}

/// Makes what is held back an utterance, if anything is.
fn release(say_all: &mut SayAll) {
    say_all.held_units = 0;
    if !say_all.held.is_empty() {
        say_all.buffer.push_back(std::mem::take(&mut say_all.held));
    }
}

/// How many utterances handed to speech playback has not started.
fn unstarted(say_all: &SayAll) -> usize {
    say_all.queued.iter().filter(|queued| queued.opens).count()
}

/// Hands speech the buffered utterances it can take, keeping [`HANDED`]
/// with speech. Each opens with an index mark, and each unit starting
/// inside it has its own, followed by its text; a unit's text is followed
/// by a space where it ends and another unit's follows, in place of the
/// line break between them.
fn hand_out(state: &mut SrState) -> Vec<Effect> {
    let mut effects = Vec::new();
    loop {
        let Some(say_all) = state.say_all.as_ref() else {
            return effects;
        };
        let trace_id = say_all.run.trace;
        if unstarted(say_all) >= HANDED {
            return effects;
        }
        let Some(pieces) = state
            .say_all
            .as_mut()
            .and_then(|say_all| say_all.buffer.pop_front())
        else {
            return effects;
        };
        let mut segments = Vec::new();
        let mut marks = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            if index == 0 || piece.marked {
                let mark = state.allocate_mark();
                segments.push(UtteranceSegment::new(SegmentContent::Mark(mark)));
                marks.push(QueuedMark {
                    mark,
                    unit: piece.marked.then(|| UnitStart {
                        chunk: Arc::clone(&piece.chunk),
                        offset: piece.start,
                    }),
                    opens: index == 0,
                });
            }
            let mut text = piece.text().to_owned();
            if index.saturating_add(1) < pieces.len() && !text.ends_with(char::is_whitespace) {
                text.push(' ');
            }
            segments.extend(text::text_segments(
                &text,
                piece.chunk.language_at(text::byte_offset(piece.start)),
            ));
        }
        // Read by say-all, so the theme's "play sounds during say all"
        // setting applies to it. In the event queue it carries its starting
        // key's number, whichever entry hands it on.
        let key = state.say_all.as_ref().and_then(|say_all| say_all.run.key);
        effects.push(Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments,
                source: None,
                validity: None,
                say_all: true,
            },
            key,
        ));
        if let Some(say_all) = state.say_all.as_mut() {
            say_all.queued.extend(marks);
        }
    }
}

/// Ends say-all once everything is read and heard, or reads the next batch
/// when fewer than [`LOW_WATER`] utterances are left to speak.
fn read_on_or_end(state: &mut SrState) -> Vec<Effect> {
    let Some(say_all) = &state.say_all else {
        return Vec::new();
    };
    let reading = say_all.run.read != QueueRead::Idle;
    if say_all.finished {
        return end_if_done(state);
    }
    if !reading && unstarted(say_all).saturating_add(say_all.buffer.len()) < LOW_WATER {
        read_next(state);
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
    // The chunk's offset decoded as a boundary of its text.
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
    ranges.retain(|range| {
        content
            .get(range.clone())
            .is_some_and(|piece| !text::is_blank(piece))
    });
    ranges
}

/// An index mark's entry at its turn: playback reached `mark`, which
/// joined the event queue in arrival order (ADR 0003, "Core's inbox and
/// the event queue"); say-all and terminal output take it. A say-all
/// started in the queue moves the caret as the entry's request on its
/// node's window, holding the queue until it is made, as NVDA's say-all
/// moves the caret on its queue.
pub(crate) async fn mark_entry(mut cx: Cx<Entries>, mark: SpeechMark) {
    let window = cx
        .facts()
        .say_all
        .as_ref()
        .map(|say_all| say_all.run.window);
    let now = cx.now();
    let (effects, moved) = {
        let mut facts = cx.facts_mut();
        let (mut effects, moved) = passed(&mut facts, mark, true);
        let moved = moved.zip(window);
        effects.extend(crate::terminal::mark_reached(&mut facts, mark, now));
        (effects, moved)
    };
    for effect in effects {
        cx.emit(effect);
    }
    let Some((moved, window)) = moved else {
        return;
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    // The caret moved by a selection of two equal points, then read for
    // the selection baseline the move leaves.
    let mut sent = crate::batch::batch(|| ());
    let to = TextPoint::At(moved.position);
    sent.select(moved.node, (to.into(), to.into()), None);
    let caret = sent.caret(moved.node, None);
    let Some(answers) =
        crate::text_request::ask(&mut cx, Answerer::Outpost(window), sent.finish(), due_us).await
    else {
        return;
    };
    // The move mints the next selection baseline, and says nothing of it
    // (`books/dev/src/reference/crates/verbatim-core.md`, "Caret tells and Core's text facts").
    if let Ok(caret) = crate::text_request::got(&answers, caret) {
        let mut facts = cx.facts_mut();
        crate::editing::caret_placed(&mut facts, moved.node, moved.line);
        crate::text_facts::minted(&mut facts, moved.node, caret.held);
    }
}

/// An utterance failed before playback reached `marks`: it counts as
/// heard, so say-all goes on past its last mark as if playback had reached
/// it, handing on and reading on, but moves neither the caret nor the
/// review cursor, since none of it was heard.
pub(crate) fn utterance_failed(state: &mut SrState, marks: &[SpeechMark]) -> Vec<Effect> {
    match marks.iter().max() {
        Some(&last) => passed(state, last, false).0,
        None => Vec::new(),
    }
}

/// Playback has gone past `mark`: the marks up to it are taken off, the
/// cursor moved to the start of the last unit among them when `moves`,
/// and the next utterance handed on. A caret move is returned for the
/// caller to ask.
fn passed(state: &mut SrState, mark: SpeechMark, moves: bool) -> (Vec<Effect>, Option<CaretMove>) {
    let Some(say_all) = state.say_all.as_mut() else {
        return (Vec::new(), None);
    };
    if !say_all.queued.iter().any(|queued| queued.mark == mark) {
        return (Vec::new(), None);
    }
    let mut reached = None;
    while say_all
        .queued
        .front()
        .is_some_and(|queued| queued.mark <= mark)
    {
        if let Some(unit) = say_all.queued.pop_front().and_then(|queued| queued.unit) {
            reached = Some(unit);
        }
    }
    let node = say_all.node;
    let moves_caret = say_all.moves_caret;
    let mut caret_move = None;
    if let Some(unit) = reached.filter(|_| moves) {
        let position = unit.position();
        if moves_caret {
            let line = TextChunk {
                offset: unit.offset,
                ..TextChunk::clone(&unit.chunk)
            };
            caret_move = Some(CaretMove {
                node,
                position,
                line,
            });
        } else if let Some(text) = state.navigator_text_on(node) {
            *text = ReviewText::Point(position);
        }
    }
    let mut effects = hand_out(state);
    effects.extend(read_on_or_end(state));
    (effects, caret_move)
}

/// The outpost generation `outpost` ended: a say-all reading one of its
/// nodes holds positions that ended with it, and stops.
pub(crate) fn outpost_ended(
    state: &mut SrState,
    outpost: verbatim_model::OutpostId,
) -> Vec<Effect> {
    if state
        .say_all
        .as_ref()
        .is_some_and(|say_all| say_all.node.outpost() == Some(outpost))
    {
        stop(state)
    } else {
        Vec::new()
    }
}

/// An utterance cut off before playback reached `marks`: when say-all
/// still waits for one of them, it stops, as on any cut, leaving the cursor
/// where reading got to.
pub(crate) fn utterance_cancelled(state: &mut SrState, marks: &[SpeechMark]) -> Vec<Effect> {
    let waits = state.say_all.as_ref().is_some_and(|say_all| {
        say_all
            .queued
            .iter()
            .any(|queued| marks.contains(&queued.mark))
    });
    if waits { stop(state) } else { Vec::new() }
}

/// Ends say-all once the document's end has been read and every piece has
/// been handed out and reached.
fn end_if_done(state: &mut SrState) -> Vec<Effect> {
    match &state.say_all {
        Some(say_all)
            if say_all.finished
                && say_all.queued.is_empty()
                && say_all.buffer.is_empty()
                && say_all.held.is_empty() =>
        {
            stop(state)
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "say_all_tests.rs"]
mod tests;
