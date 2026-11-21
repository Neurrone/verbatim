//! A caret wait's check (`docs/design/readers-tell-core-asks-build1.md`,
//! "The watch", Table 15; the request list's design, section 3,
//! `WaitForCaret`): it compares the caret with the starting point by the
//! effect test, the starting texts given with the wait.
//!
//! This is the check call by call, an edit control's; through UI
//! Automation the check is the first step of each poll's program
//! (`super::fused::poll`), which judges it by the same test
//! ([`effect_of`]).
//!
//! A check reads the caret and the selection's ends and compares them with
//! the starting point's, held under the wait's anchor; it reads the
//! caret's line, for the characters beside the caret and, where the effect
//! test counts the line, the line itself; and where it counts the unit at
//! the caret, once 30 ms have passed, the word at the caret. A compare the
//! provider refuses counts as a move (NVDA compares bookmarks,
//! `editableText.py` 121 to 149). A check whose caret moved while it read
//! reads again at once rather than ending on a torn read.

#![forbid(unsafe_code)]

use std::cmp::Ordering;

use verbatim_model::batch::{CaretWait, Waited};
use verbatim_model::{Compared, TextUnit, had_effect};

use super::{
    CaretState, LastRead, MAX_CHUNK_BYTES, MAX_CHUNK_UNITS, TextError, TextResult, TextSource,
    beside, caret_line, same_state, to_utf8,
};

/// How many times a check reads again when the caret moved while it read,
/// before it counts as showing nothing: the next check, 10 ms later, reads
/// again in any case.
pub(crate) const TORN_READS: usize = 3;

/// What a check is asked: the wait, whether to compare the word at the
/// caret, and whether it is the check at the bound.
#[derive(Clone, Copy, Debug)]
pub struct CheckAsk<'a> {
    /// The wait.
    pub wait: &'a CaretWait,
    /// Whether to compare the word at the caret.
    pub word: bool,
    /// Whether this is the check at the bound.
    pub last: bool,
}

/// A wait's check from the starting point `start`: `None` when it shows
/// no effect before the bound; otherwise how the wait ended.
///
/// # Errors
///
/// [`TextError::Gone`] when the node is gone; [`TextError::Failed`] when
/// the application does not answer, which counts as no effect.
pub fn check<S: TextSource>(
    source: &mut S,
    start: &LastRead<S::Pos>,
    ask: CheckAsk<'_>,
) -> TextResult<Option<Waited>> {
    let mut tries = 0;
    let found = loop {
        let Some(found) = read_by_parts(source, start, ask)? else {
            return Ok(None);
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
    Ok(Some(if found.effect {
        Waited::Effect
    } else {
        Waited::Bound
    }))
}

/// What one try of a check found: whether it shows the key's effect, and
/// whether the caret moved while it was read.
struct Found {
    effect: bool,
    torn: bool,
}

/// One try of a check call by call: `None` when it shows no effect before
/// the bound, so the caret is not read again.
fn read_by_parts<S: TextSource>(
    source: &mut S,
    start: &LastRead<S::Pos>,
    ask: CheckAsk<'_>,
) -> TextResult<Option<Found>> {
    let state = source.caret()?;
    let compared = compare(source, start, &state, ask)?;
    let effect = had_effect(compared, ask.wait.effect);
    if !effect && !ask.last {
        return Ok(None);
    }
    let again = source.caret()?;
    let torn = !same_state(source, &again, &state)?;
    Ok(Some(Found { effect, torn }))
}

/// How the caret `state` compares with the starting point `start`.
fn compare<S: TextSource>(
    source: &mut S,
    start: &LastRead<S::Pos>,
    state: &CaretState<S::Pos>,
    ask: CheckAsk<'_>,
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
    let word = if ask.word {
        Some(word_at(source, state)?)
    } else {
        None
    };
    Ok(compare_texts(
        ask.wait,
        (caret_moved, selection_moved),
        (&line.text, offset),
        word,
    ))
}

/// Whether a check that read whether the caret and the selection moved,
/// the caret's line now with the caret's UTF-16 offset in it, and the word
/// at the caret now when it is compared, shows the key's effect by
/// `wait`'s effect test: a check made in one program, through UI
/// Automation (`super::fused::poll`).
pub(crate) fn effect_of(
    wait: &CaretWait,
    moved: (bool, bool),
    line: (&[u16], usize),
    word: Option<&[u16]>,
) -> bool {
    let word = word.map(|word| to_utf8(word, MAX_CHUNK_BYTES, &[]).0);
    had_effect(compare_texts(wait, moved, line, word), wait.effect)
}

/// The comparison with the wait's starting texts, given whether the caret
/// and the selection moved, the caret's line now with the caret's UTF-16
/// offset in it, and the word at the caret now when it is compared.
fn compare_texts(
    wait: &CaretWait,
    (caret_moved, selection_moved): (bool, bool),
    (line, offset): (&[u16], usize),
    word: Option<String>,
) -> Compared {
    let (text, mapped, _) = to_utf8(line, MAX_CHUNK_BYTES, &[offset]);
    let at = mapped.first().copied().unwrap_or_default();
    let beside_changed = beside(&text, at) != beside(&wait.line, wait.offset as usize);
    let line_changed = text != wait.line;
    let unit_changed = word.is_some_and(|word| wait.word.as_deref() != Some(word.as_str()));
    Compared {
        caret_moved,
        selection_moved,
        beside_changed,
        unit_changed,
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
        .unit_at(&state.caret, TextUnit::Word, MAX_CHUNK_UNITS)?
        .map(|unit| to_utf8(&unit.text, MAX_CHUNK_BYTES, &[]).0)
        .unwrap_or_default())
}
