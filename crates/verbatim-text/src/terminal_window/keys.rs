//! A read in the keys' window (F8, the larger form): the keys' answers by
//! TM rules 19 to 23 as written, the screen before and after compared row
//! by row by the alignment, with every screen row's colours read in the
//! window. Each read is compared once with the last (TM 18); the window
//! before the first key gives where the keys started.

use crate::proven::text_in;
use crate::{Segmenter, WordRules, grapheme_at, lost_text, squeezed};

use super::{
    Fate, Held, Hold, Judged, KeyKind, KeyWindow, QUIET_US, Reading, Said, Unit, changed_words,
    common_ends,
};

/// The unit of the last caret key in the window, if any.
fn unit(window: &KeyWindow) -> Option<Unit> {
    window.keys.iter().rev().find_map(|key| match key {
        KeyKind::Caret(unit) => Some(*unit),
        _ => None,
    })
}

/// The text at the caret of line `text`, at byte `at`, in `unit`: the
/// character, the word, or the caret's row, `row`; empty past the line's
/// end.
fn at_unit(text: &str, at: usize, unit: Unit, row: &str) -> String {
    match unit {
        Unit::Line => row.to_owned(),
        Unit::Character => {
            grapheme_at(text, at).map_or_else(String::new, |range| text_in(text, range).to_owned())
        }
        Unit::Word => Segmenter::new()
            .word_at(text, at, WordRules::for_text(text, None))
            .map_or_else(String::new, |range| text_in(text, range).to_owned()),
    }
}

/// A row away from the caret that the keys changed from `base` to `new`
/// (TM 21): one that gained text is spoken whole at once, one that only
/// lost text is silent, any other is held while keys keep coming and spoken
/// once by its changed words.
pub(super) fn away(
    reading: &mut Reading,
    read: &mut Judged<'_>,
    (i, j): (usize, usize),
    (base, new): (&str, &str),
) {
    if lost_text(base, new) {
        return;
    }
    let (was, now) = (squeezed(base), squeezed(new));
    if now.len() > was.len() && now.contains(&was) && !now.starts_with(&was) {
        read.answer.keys.push(Said {
            unit: Unit::Line,
            text: new.to_owned(),
        });
        return;
    }
    let said = read
        .held
        .iter()
        .position(|held| held.line == i && matches!(held.hold, Hold::Keys { .. }))
        .map(|at| read.held.remove(at))
        .and_then(|held| match held.hold {
            Hold::Keys { said } => Some(said),
            Hold::Word { .. } | Hold::Volatile => None,
        })
        .unwrap_or_else(|| base.to_owned());
    let due = reading
        .keys
        .as_ref()
        .map_or(read.now, |window| window.last_us)
        .saturating_add(QUIET_US);
    reading.held.push(Held {
        line: j,
        text: new.to_owned(),
        hold: Hold::Keys { said },
        due_us: due,
    });
}

/// The caret's suggestion on a read in the window of typed keys (TM 22):
/// the text after the caret on its row, when the cell at the caret is in
/// other colours than the cell before it, the typing's.
fn suggestion_at(read: &Judged<'_>, window: &KeyWindow) -> Option<String> {
    if !window.keys.contains(&KeyKind::Typed) {
        return None;
    }
    let caret = read.new.caret?;
    let row = read.new.row_text(caret.line, caret.row);
    let head: String = row.chars().take(caret.column).collect();
    let column = u32::try_from(crate::cell_width(&head)).ok()?;
    let runs = read.new.row_colours(caret.line, caret.row)?;
    let colour = |at: u32| {
        runs.iter()
            .find(|run| run.column <= at && at < run.column.saturating_add(run.length))
            .map(|run| (run.foreground, run.background))
    };
    let typed = colour(column.checked_sub(1)?)?;
    let after = colour(column)?;
    let tail: String = row.chars().skip(caret.column).collect();
    let tail = tail.trim();
    (after != typed && !tail.is_empty()).then(|| tail.to_owned())
}

/// The caret's suggestion, spoken when it appears or becomes another
/// (TM 22).
fn suggest(reading: &mut Reading, read: &mut Judged<'_>, window: &KeyWindow) -> Option<String> {
    let suggestion = suggestion_at(read, window);
    if let Some(text) = &suggestion
        && reading.suggestion.as_ref() != Some(text)
    {
        read.answer.keys.push(Said {
            unit: Unit::Line,
            text: text.clone(),
        });
    }
    reading.suggestion.clone_from(&suggestion);
    suggestion
}

/// The keys' answer on the caret's line (TM 19), with its suggestion
/// (TM 22).
pub(super) fn caret(reading: &mut Reading, read: &mut Judged<'_>, window: &KeyWindow) {
    let suggestion = suggest(reading, read, window);
    let Some(now) = read.new.caret else {
        return;
    };
    if window.keys.contains(&KeyKind::Enter) {
        return;
    }
    let i = match read.fates.get(now.line) {
        Some(Fate::Kept(i) | Fate::Paired(i)) => *i,
        Some(Fate::Inserted) | None => return,
    };
    // Each read is compared once with the last (TM 18): one that finds the
    // caret and its line as the last read did has nothing to answer, so a
    // key's answer is not said again by a later read in its window.
    if matches!(read.fates.get(now.line), Some(Fate::Kept(_)))
        && read
            .old
            .caret
            .is_some_and(|was| (was.line, was.row, was.column) == (i, now.row, now.column))
    {
        return;
    }
    let text = read.new.text(now.line);
    let Some((_, head, _)) = read.new.at_caret() else {
        return;
    };
    let at = head.len();
    let row = read.new.row_text(now.line, now.row);
    let started = window.before.at_caret().map(|(line, head, tail)| {
        (
            window.before.lines.get(line).map(super::Line::top),
            head.len(),
            format!("{head}{tail}"),
        )
    });
    let place = read.new.lines.get(now.line).map(super::Line::top);
    if started == Some((place, at, text.to_owned())) {
        return;
    }
    let unit = unit(window);
    let typed = window.keys.contains(&KeyKind::Typed);
    let say = |read: &mut Judged<'_>, unit: Unit, text: String| {
        read.answer.keys.push(Said { unit, text });
    };
    let was = read.old.caret;
    if was.map(|caret| (caret.line, caret.row)) != Some((i, now.row)) {
        let unit = unit.unwrap_or(Unit::Line);
        say(read, unit, at_unit(text, at, unit, &row));
        return;
    }
    // A line of several rows, such as a full-screen program's screen the
    // console host gives as one line, is compared by the caret's row.
    let several = read
        .new
        .lines
        .get(now.line)
        .is_some_and(|line| line.rows > 1);
    let change = read.caret_change.take().and_then(|change| {
        if !several {
            return Some(change);
        }
        let base = read.old.row_text(i, now.row);
        (base != row).then(|| (base, row.clone()))
    });
    if let Some((base, changed)) = change {
        let changed = match &suggestion {
            Some(tail) => changed
                .strip_suffix(tail.as_str())
                .unwrap_or(&changed)
                .trim_end()
                .to_owned(),
            None => changed,
        };
        if unit == Some(Unit::Line) {
            // Keys sharing a window answer once: a recalled line with a
            // character typed after it is the line, not the echo.
            read.answer.echo.clear();
            say(read, Unit::Line, row.clone());
            return;
        }
        let (prefix, suffix) = common_ends(&base, &changed);
        let gained = text_in(&changed, prefix..changed.len().saturating_sub(suffix));
        let lost = text_in(&base, prefix..base.len().saturating_sub(suffix));
        if lost.is_empty() && !gained.trim().is_empty() {
            say(read, Unit::Line, gained.trim().to_owned());
        } else if gained.is_empty() && !lost.is_empty() {
            if window.keys.contains(&KeyKind::Delete) {
                say(
                    read,
                    Unit::Character,
                    at_unit(text, at, Unit::Character, &row),
                );
            } else if crate::graphemes(lost).len() == 1 {
                // One character lost is spelled, as NVDA's Backspace
                // script spells a one-character deletion; a longer one is
                // said as text (`docs/parity/text.md`, "What Backspace and
                // Delete say").
                say(read, Unit::Character, lost.to_owned());
            } else {
                say(read, Unit::Line, lost.to_owned());
            }
        } else if let Some(words) = changed_words(&base, &changed) {
            say(read, Unit::Line, words);
        }
        return;
    }
    if !read.answer.echo.is_empty() {
        return;
    }
    if unit == Some(Unit::Line) && !typed {
        say(read, Unit::Line, row);
    } else if read.old.caret.map(|caret| (caret.row, caret.column)) != Some((now.row, now.column)) {
        let unit = unit.unwrap_or(Unit::Character);
        say(read, unit, at_unit(text, at, unit, &row));
    }
}

/// Whether screen row `at` of `window` stands out: its emphasis
/// ([`super::Window::emphasis`]) differs from both its neighbours'.
fn stands_out(window: &super::Window, at: usize) -> bool {
    let own = window.emphasis(at);
    let differs = |other: Option<usize>| {
        other.is_some_and(|other| {
            window
                .emphasis(other)
                .is_some_and(|other| Some(other) != own)
        })
    };
    own.is_some() && at > 0 && differs(at.checked_sub(1)) && differs(at.checked_add(1))
}

/// The row that gained emphasis (TM 21), the screens compared row by row:
/// of the screen's rows away from the caret whose text is as it was at the
/// same screen row and whose emphasis changed, or was read for the first
/// time, the one that now stands out from both its neighbours and did not
/// before, spoken whole when there is exactly one.
pub(super) fn emphasis(read: &mut Judged<'_>) {
    let Some(height) = read.new.colours.as_ref().map(|colours| {
        colours
            .len()
            .min(usize::try_from(read.new.screen_rows).unwrap_or(0))
    }) else {
        return;
    };
    let caret = read.new.caret.map(|caret| (caret.line, caret.row));
    let mut picked = Vec::new();
    for at in 0..height {
        let Some((j, row)) = read.new.at_screen_row(at) else {
            continue;
        };
        let text = read.new.row_text(j, row);
        if Some((j, row)) == caret || text.is_empty() {
            continue;
        }
        let same = read
            .old
            .at_screen_row(at)
            .is_some_and(|(i, old_row)| read.old.row_text(i, old_row) == text);
        if same
            && read.old.emphasis(at) != read.new.emphasis(at)
            && stands_out(read.new, at)
            && !stands_out(read.old, at)
        {
            picked.push(text);
        }
    }
    if let [text] = picked.as_slice() {
        read.answer.keys.push(Said {
            unit: Unit::Line,
            text: text.clone(),
        });
    }
}
