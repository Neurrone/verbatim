//! A sentence key's move, run whole in one request
//! (`books/dev/src/reference/crates/verbatim-core.md`, "The caret key on
//! the event queue"):
//! from the key's starting point, the caret's paragraph is read, the next
//! or previous sentence found in it by Unicode's rules, the paragraph
//! beside it read when the sentence is not in it, and the caret moved,
//! as NVDA's `_caretMoveBySentenceHelper` moves the position it read
//! before the key (`editableText.py` 245 to 262). At the text's ends the
//! caret goes to the end past the last sentence, or stays at the start
//! with the first sentence (`docs/parity/text.md`, "A caret
//! key that moves nothing"). Only a source with a sentence unit (an edit
//! control's messages) is asked.

#![forbid(unsafe_code)]

use std::ops::Range;

use verbatim_model::batch::{Missing, StepOutcome, StepRefusal};
use verbatim_model::{SentenceMoved, TextAnchor, TextChunk, TextUnit};

use super::held::held_after_move;
use super::{MAX_CHUNK_UNITS, NodeText, Sentences, TextResult, TextSource, Unit, chunk};

/// A sentence found in a paragraph: its start, a byte offset into the
/// paragraph's text, and its text.
struct Sentence {
    offset: usize,
    text: String,
}

/// The sentences of `paragraph`'s content, without the line break that
/// ends it, and the caret's byte offset in it.
fn sentences(paragraph: &TextChunk) -> (&str, usize, Vec<Range<usize>>) {
    let content = paragraph
        .text
        .trim_end_matches(verbatim_text::is_line_break);
    let mut caret = usize::try_from(paragraph.offset)
        .unwrap_or(usize::MAX)
        .min(content.len());
    while !content.is_char_boundary(caret) {
        caret = caret.saturating_sub(1);
    }
    let ranges = verbatim_text::Segmenter::new().sentences(content);
    (content, caret, ranges)
}

/// The sentence of `content` at `range`.
fn sentence_at(content: &str, range: Range<usize>) -> Sentence {
    Sentence {
        offset: range.start,
        text: content.get(range).unwrap_or_default().to_owned(),
    }
}

/// The first sentence starting after the caret, Alt+Down's landing.
fn next_sentence(paragraph: &TextChunk) -> Option<Sentence> {
    let (content, caret, ranges) = sentences(paragraph);
    let range = ranges.into_iter().find(|range| range.start > caret)?;
    Some(sentence_at(content, range))
}

/// The sentence holding the character before the caret, Alt+Up's
/// landing, as NVDA steps back one offset and takes that sentence's start.
fn sentence_before(paragraph: &TextChunk) -> Option<Sentence> {
    let (content, caret, ranges) = sentences(paragraph);
    let range = ranges.into_iter().rfind(|range| range.start < caret)?;
    Some(sentence_at(content, range))
}

/// The paragraph's first sentence; an empty one at its start when it has
/// none.
fn first_sentence(paragraph: &TextChunk) -> Sentence {
    let (content, _, ranges) = sentences(paragraph);
    sentence_at(content, ranges.into_iter().next().unwrap_or(0..0))
}

/// The paragraph's last sentence; an empty one at its start when it has
/// none.
fn last_sentence(paragraph: &TextChunk) -> Sentence {
    let (content, _, ranges) = sentences(paragraph);
    sentence_at(content, ranges.into_iter().next_back().unwrap_or(0..0))
}

/// Moves the caret by sentence from the starting point held under
/// `point`, `forward` or back, and answers where it went, with the next
/// selection baseline. A source with no sentence unit is refused no such
/// unit, and a starting point no longer held is not there: `Ok(Err)` with
/// the step's outcome.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn move_by_sentence<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (point, forward): (TextAnchor, bool),
) -> TextResult<Result<SentenceMoved, StepOutcome>> {
    if source.sentences() == Sentences::Unsupported {
        return Ok(Err(StepOutcome::Refused(StepRefusal::NoSuchUnit)));
    }
    let Some(start) = anchors.held_read(point) else {
        return Ok(Err(StepOutcome::NotThere(Missing::Anchor(point))));
    };
    let Some(here) = paragraph_at(source, anchors, &start.caret, Some(&start.caret))? else {
        return Ok(Err(StepOutcome::Refused(StepRefusal::Unsupported)));
    };
    let found = if forward {
        next_sentence(&here.1)
    } else {
        sentence_before(&here.1)
    };
    if let Some(sentence) = found {
        return move_to(source, anchors, here, sentence).map(Ok);
    }
    let count = if forward { 1 } else { -1 };
    let beside = match source.move_by(&here.0.start, TextUnit::Paragraph, count)? {
        Some((at, moved)) if moved != 0 => paragraph_at(source, anchors, &at, None)?,
        _ => None,
    };
    match (beside, forward) {
        (Some(next), true) => {
            let sentence = first_sentence(&next.1);
            move_to(source, anchors, next, sentence).map(Ok)
        }
        (Some(previous), false) => {
            let sentence = last_sentence(&previous.1);
            move_to(source, anchors, previous, sentence).map(Ok)
        }
        (None, true) => {
            let end = source.end()?;
            let mut line = here.1;
            line.offset = u32::try_from(line.text.len()).unwrap_or(u32::MAX);
            let baseline = place(source, anchors, &end)?;
            Ok(Ok(SentenceMoved::End { line, baseline }))
        }
        (None, false) => {
            let sentence = first_sentence(&here.1);
            // Nothing moved: the baseline is a fresh read of the caret.
            let read_at_us = verbatim_process::clock::now_us();
            let kept = anchors.anchors.last.as_ref().map(|last| last.read_at_us);
            let baseline = held_after_move(source, anchors, kept, read_at_us)?;
            Ok(Ok(SentenceMoved::StayedAtStart {
                sentence: sentence.text,
                line: here.1,
                baseline,
            }))
        }
    }
}

/// The paragraph containing `at`, read, with its chunk, the chunk's
/// offset at `caret` when given, else at its start.
fn paragraph_at<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    at: &S::Pos,
    caret: Option<&S::Pos>,
) -> TextResult<Option<(Unit<S::Pos>, TextChunk)>> {
    let Some(unit) = source.unit_at(at, TextUnit::Paragraph, MAX_CHUNK_UNITS)? else {
        return Ok(None);
    };
    let (offset, pos) = match caret {
        Some(caret) => (source.offset_in(&unit, caret)?, caret.clone()),
        None => (0, unit.start.clone()),
    };
    let chunk = chunk(
        source,
        anchors,
        (&unit, TextUnit::Paragraph),
        (offset, pos),
        (Vec::new(), &[]),
    );
    Ok(Some((unit, chunk)))
}

/// Moves the caret to `sentence` in the paragraph `(unit, chunk)`.
fn move_to<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (unit, chunk): (Unit<S::Pos>, TextChunk),
    sentence: Sentence,
) -> TextResult<SentenceMoved> {
    let prefix: Vec<u16> = chunk
        .text
        .get(..sentence.offset)
        .unwrap_or_default()
        .encode_utf16()
        .collect();
    let target = source.advance(&unit.start, &prefix)?;
    let baseline = place(source, anchors, &target)?;
    let line = TextChunk {
        offset: u32::try_from(sentence.offset).unwrap_or(u32::MAX),
        ..chunk
    };
    Ok(SentenceMoved::Moved {
        sentence: sentence.text,
        line,
        baseline,
    })
}

/// Puts the caret at `target` and holds the next baseline there.
fn place<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    target: &S::Pos,
) -> TextResult<TextAnchor> {
    let read_at_us = verbatim_process::clock::now_us();
    let kept_before = anchors.anchors.last.as_ref().map(|last| last.read_at_us);
    if !source.select(target, target)? {
        return Err(super::TextError::Failed(
            "the caret could not be moved".to_owned(),
        ));
    }
    anchors.keep_made((target.clone(), target.clone()), false, None, read_at_us);
    held_after_move(source, anchors, kept_before, read_at_us)
}
