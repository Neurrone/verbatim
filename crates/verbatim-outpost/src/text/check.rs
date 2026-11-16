//! A caret key's watch's programs (`docs/design/readers-tell-core-asks-build1.md`,
//! "The watch", Table 15): a check, which compares the caret with the
//! key's starting point by the effect test and, with the effect or at the
//! bound, makes the final read; and the final read alone, once an event
//! showed the effect.
//!
//! A check reads the caret and the selection's ends and compares them with
//! the starting point; it reads the caret's line, for the characters beside
//! the caret and, after a key that deletes, the line itself; and for Delete
//! and Control+Delete, once 30 ms have passed, the word at the caret. A
//! compare the provider refuses counts as a move (NVDA compares bookmarks,
//! `editableText.py` 121 to 149). A check whose caret moved while it read
//! reads again at once rather than ending on a torn read.

#![forbid(unsafe_code)]

use std::cmp::Ordering;

use verbatim_model::{
    CaretMotion, Compared, TextAnchor, TextUnit, WatchFinalRead, Watched, WatchedRead, had_effect,
};

use super::{
    CaretState, MAX_CHUNK_BYTES, NodeText, TextError, TextResult, TextSource, before_unit_text,
    beside, caret_line, report_for, same_state, to_utf8, unit_at_caret,
};

/// How many times a check reads again when the caret moved while it read,
/// before it counts as showing nothing: the next check, 10 ms later, reads
/// again in any case.
const TORN_READS: usize = 3;

/// What a check is asked: the starting point, the key's motion, whether to
/// compare the word at the caret, whether it is the check at the bound, and
/// the final read.
#[derive(Clone, Copy, Debug)]
pub struct CheckAsk {
    /// The starting point.
    pub point: TextAnchor,
    /// The key's motion.
    pub motion: CaretMotion,
    /// Whether to compare the word at the caret.
    pub word: bool,
    /// Whether this is the check at the bound.
    pub last: bool,
    /// The final read.
    pub final_read: WatchFinalRead,
}

/// A watch's check: `None` when it shows no effect before the bound;
/// otherwise the watch's answer, its final read made from the caret the
/// check read.
///
/// # Errors
///
/// [`TextError::Gone`] when the starting point is no longer held or the
/// node is gone; [`TextError::Failed`] when the application does not
/// answer, which counts as no effect.
pub fn check<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    ask: CheckAsk,
    read_at_us: u64,
) -> TextResult<Option<Watched>> {
    let start = anchors.held_read(ask.point).ok_or(TextError::Gone)?;
    let mut tries = 0;
    let (state, effect) = loop {
        let state = source.caret()?;
        let compared = compare(source, &start, &state, (ask.motion, ask.word))?;
        let effect = had_effect(compared, ask.motion);
        if !effect && !ask.last {
            return Ok(None);
        }
        let again = source.caret()?;
        if same_state(source, &again, &state)? {
            break (state, effect);
        }
        tries += 1;
        if tries >= TORN_READS {
            if ask.last {
                break (again, effect);
            }
            return Ok(None);
        }
    };
    let (unit, at_effect) = match ask.final_read {
        WatchFinalRead::Unit { unit, .. } => (unit, false),
        WatchFinalRead::UnitAtEffect { unit, .. } => (unit, true),
    };
    if !effect && at_effect {
        return Ok(Some(Watched::BoundUnread));
    }
    let read = read_final(source, anchors, &state, unit, read_at_us)?;
    Ok(Some(if effect {
        Watched::Effect(read)
    } else {
        Watched::Bound(read)
    }))
}

/// A watch's final read once an event showed the key's effect: the caret,
/// its line and the key's `unit` at it.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn final_read<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    unit: TextUnit,
    read_at_us: u64,
) -> TextResult<WatchedRead> {
    let state = source.caret()?;
    read_final(source, anchors, &state, unit, read_at_us)
}

/// The final read from the caret `state`: its line, with the caret at its
/// offset, and the key's `unit` at the caret, the line where the text has
/// no such unit.
fn read_final<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    state: &CaretState<S::Pos>,
    unit: TextUnit,
    read_at_us: u64,
) -> TextResult<WatchedRead> {
    let caret = report_for(source, anchors, (state, read_at_us), (None, None), None)?;
    let unit = unit_at_caret(
        source,
        anchors,
        (state, &caret.line),
        unit,
        (None, Vec::new()),
    )?
    .unwrap_or_else(|| caret.line.clone());
    Ok(WatchedRead { caret, unit })
}

/// How the caret `state` compares with the starting point `start`.
fn compare<S: TextSource>(
    source: &mut S,
    start: &super::LastRead<S::Pos>,
    state: &CaretState<S::Pos>,
    (motion, word): (CaretMotion, bool),
) -> TextResult<Compared> {
    let caret_moved = differs(source, &state.caret, &start.caret)?;
    let (now_start, now_end) = state
        .selection
        .clone()
        .unwrap_or_else(|| (state.caret.clone(), state.caret.clone()));
    let (was_start, was_end) = start.selection.as_ref().map_or_else(
        || (start.caret.clone(), start.caret.clone()),
        |selection| (selection.start.clone(), selection.end.clone()),
    );
    let selection_moved =
        differs(source, &now_start, &was_start)? || differs(source, &now_end, &was_end)?;
    let (line, offset) = caret_line(source, state)?;
    let (text, mapped, _) = to_utf8(&line.text, MAX_CHUNK_BYTES, &[offset]);
    let at = mapped.first().copied().unwrap_or_default();
    let beside_changed = beside(&text, at) != beside(&start.line.text, start.line.offset as usize);
    let line_changed = text != start.line.text;
    let unit_changed = word && word_at(source, state)? != before_unit_text(start, TextUnit::Word);
    Ok(Compared {
        caret_moved,
        selection_moved,
        beside_changed,
        unit_changed: motion.compares_unit() && unit_changed,
        line_changed,
    })
}

/// Whether `a` and `b` differ: a compare the provider refuses (ranges of
/// different documents) counts as a difference.
fn differs<S: TextSource>(source: &mut S, a: &S::Pos, b: &S::Pos) -> TextResult<bool> {
    match source.compare(a, b) {
        Ok(order) => Ok(order != Ordering::Equal),
        Err(TextError::Failed(_)) => Ok(true),
        Err(gone @ TextError::Gone) => Err(gone),
    }
}

/// The word at the caret, as text.
fn word_at<S: TextSource>(source: &mut S, state: &CaretState<S::Pos>) -> TextResult<String> {
    Ok(source
        .unit_at(&state.caret, TextUnit::Word, super::MAX_CHUNK_UNITS)?
        .map(|unit| to_utf8(&unit.text, MAX_CHUNK_BYTES, &[]).0)
        .unwrap_or_default())
}
