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
    CaretRequest, CaretState, LastRead, MAX_CHUNK_BYTES, NodeText, TextError, TextResult,
    TextSource, Unit, WatchRequest, before_unit_text, beside, caret_line, extra_unit, report_for,
    same_state, to_utf8, unit_at_caret,
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
    let (unit, at_effect) = match ask.final_read {
        WatchFinalRead::Unit { unit, .. } => (unit, false),
        WatchFinalRead::UnitAtEffect { unit, .. } => (unit, true),
    };
    let extra = extra_unit(source, unit);
    let mut tries = 0;
    let found = loop {
        let found = match read_at_once(source, &start, ask, extra)? {
            Some(found) => found,
            None => match read_by_parts(source, &start, ask)? {
                Some(found) => found,
                None => return Ok(None),
            },
        };
        if !found.effect && !ask.last {
            return Ok(None);
        }
        if !found.torn {
            break found;
        }
        tries += 1;
        if tries >= TORN_READS {
            if ask.last {
                break found;
            }
            return Ok(None);
        }
    };
    if !found.effect && at_effect {
        return Ok(Some(Watched::BoundUnread));
    }
    let read = read_final(
        source,
        anchors,
        (&found.state, read_at_us),
        unit,
        (found.line, found.unit),
    )?;
    Ok(Some(if found.effect {
        Watched::Effect(read)
    } else {
        Watched::Bound(read)
    }))
}

/// What one try of a check found: the caret, whether it shows the key's
/// effect, whether the caret moved while it was read, and what it read
/// that the final read reuses.
struct Found<P> {
    state: CaretState<P>,
    effect: bool,
    torn: bool,
    line: Option<(Unit<P>, usize)>,
    unit: Option<(Unit<P>, usize)>,
}

/// One try of a check in one go, where the source can make its reads at
/// once ([`TextSource::watch_read`]); `None` for a source that cannot.
fn read_at_once<S: TextSource>(
    source: &mut S,
    start: &LastRead<S::Pos>,
    ask: CheckAsk,
    unit: Option<TextUnit>,
) -> TextResult<Option<Found<S::Pos>>> {
    let start_selection = start
        .selection
        .as_ref()
        .map_or((&start.caret, &start.caret), |selection| {
            (&selection.start, &selection.end)
        });
    let request = WatchRequest {
        start: &start.caret,
        start_selection,
        word: ask.word,
        unit,
        last: ask.last,
    };
    let Some(read) = source.watch_read(&request)? else {
        return Ok(None);
    };
    let (line, offset) = &read.line;
    let word = read
        .word
        .as_ref()
        .map(|(word, _)| to_utf8(&word.text, MAX_CHUNK_BYTES, &[]).0);
    let compared = compare_texts(
        start,
        (read.caret_moved, read.selection_moved),
        (&line.text, *offset),
        (ask.motion, word),
    );
    Ok(Some(Found {
        effect: had_effect(compared, ask.motion),
        torn: read.moved_while_read,
        state: read.state,
        line: Some(read.line),
        unit: read.unit,
    }))
}

/// One try of a check call by call: `None` when it shows no effect before
/// the bound, so the caret is not read again.
fn read_by_parts<S: TextSource>(
    source: &mut S,
    start: &LastRead<S::Pos>,
    ask: CheckAsk,
) -> TextResult<Option<Found<S::Pos>>> {
    let state = source.caret()?;
    let compared = compare(source, start, &state, (ask.motion, ask.word))?;
    let effect = had_effect(compared, ask.motion);
    if !effect && !ask.last {
        return Ok(None);
    }
    let again = source.caret()?;
    let torn = !same_state(source, &again, &state)?;
    Ok(Some(Found {
        state: if torn { again } else { state },
        effect,
        torn,
        line: None,
        unit: None,
    }))
}

/// A watch's final read once an event showed the key's effect: the caret,
/// its line and the key's `unit` at it, in one round trip where the source
/// reads a caret in one go ([`TextSource::caret_read`]).
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
    let request = CaretRequest {
        since: None,
        previous: None,
        previous_collapsed: false,
        unit: extra_unit(source, unit),
        formats: None,
        recheck: false,
        read_selection_text: false,
        read_previous_text: false,
    };
    if let Some(read) = source.caret_read(&request)? {
        return read_final(
            source,
            anchors,
            (&read.state, read_at_us),
            unit,
            (Some(read.line), read.unit),
        );
    }
    let state = source.caret()?;
    read_final(source, anchors, (&state, read_at_us), unit, (None, None))
}

/// The final read from the caret `state`: its line, with the caret at its
/// offset, and the key's `unit` at the caret, the line where the text has
/// no such unit; the line and the unit the check already read are not
/// read again.
fn read_final<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (state, read_at_us): (&CaretState<S::Pos>, u64),
    unit: TextUnit,
    (line, unit_read): ReadAlready<S::Pos>,
) -> TextResult<WatchedRead> {
    let line = line.map(|(line, offset)| (line, offset, Vec::new()));
    let caret = report_for(source, anchors, (state, read_at_us), (line, None), None)?;
    let unit = unit_at_caret(
        source,
        anchors,
        (state, &caret.line),
        unit,
        (unit_read, Vec::new()),
    )?
    .unwrap_or_else(|| caret.line.clone());
    Ok(WatchedRead { caret, unit })
}

/// The caret's line and the key's unit, each with the caret's offset in
/// it, when a check already read them.
type ReadAlready<P> = (Option<(Unit<P>, usize)>, Option<(Unit<P>, usize)>);

/// How the caret `state` compares with the starting point `start`.
fn compare<S: TextSource>(
    source: &mut S,
    start: &LastRead<S::Pos>,
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
    let word = if word {
        Some(word_at(source, state)?)
    } else {
        None
    };
    Ok(compare_texts(
        start,
        (caret_moved, selection_moved),
        (&line.text, offset),
        (motion, word),
    ))
}

/// The comparison with the starting point `start`, given whether the caret
/// and the selection moved, the caret's line now with the caret's UTF-16
/// offset in it, and the word at the caret now when it is compared.
fn compare_texts<P>(
    start: &LastRead<P>,
    (caret_moved, selection_moved): (bool, bool),
    (line, offset): (&[u16], usize),
    (motion, word): (CaretMotion, Option<String>),
) -> Compared {
    let (text, mapped, _) = to_utf8(line, MAX_CHUNK_BYTES, &[offset]);
    let at = mapped.first().copied().unwrap_or_default();
    let beside_changed = beside(&text, at) != beside(&start.line.text, start.line.offset as usize);
    let line_changed = text != start.line.text;
    let unit_changed = word.is_some_and(|word| word != before_unit_text(start, TextUnit::Word));
    Compared {
        caret_moved,
        selection_moved,
        beside_changed,
        unit_changed: motion.compares_unit() && unit_changed,
        line_changed,
    }
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
