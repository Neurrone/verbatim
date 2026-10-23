//! The terminal line diff (`docs/design/terminal-reading.md`, section 3,
//! "The diff"): a pure function from the screen a terminal showed at its
//! last read, and one snapshot of its text taken now, to what is new, in
//! the order it is spoken. Original code written from that design, its
//! review (`docs/design/terminal-reading-review.md`, sections 5 and 6), and
//! `docs/parity/terminals.md`, "New terminal output".
//!
//! Lines are logical lines without their padding: a line that wrapped onto
//! more rows is one line, so no row arithmetic is needed. Two entry points
//! share one alignment:
//!
//! - [`diff_read`], the normal read: the snapshot starts at the top of the
//!   old screen as it was, so in the usual case its first lines are the old
//!   screen. Only its first lines, up to the old screen's length, one
//!   screen, and one line more, are aligned with the old screen; everything
//!   after the last old line kept is appended output.
//! - [`diff_document`], the whole-document read the outpost takes once,
//!   never repeated, when the normal read did not find the old screen
//!   ([`Found::old_missing`]): the old screen is found by content, nearest
//!   the document's end, and diffed from there; not found in a document
//!   with history above its screen, it is the history's overflow.
//!
//! What is spoken, as in `docs/parity/terminals.md`, "Reading output":
//! lines inserted are spoken whole; a line rewritten in place speaks what
//! it gained at its end, or from the start of the word where it first
//! differs ([`line_change`]); a spinner, a line that only lost text, and
//! lines only deleted say nothing.

use verbatim_model::{LineChange, Skipped};

use crate::{Segmenter, WordRules, graphemes, is_word_grapheme};

/// One read of a terminal, as both entry points take it.
#[derive(Clone, Copy, Debug)]
pub struct Read<'a> {
    /// The old screen's lines as last read, padding trimmed: the lines of
    /// the screen at the last read that count ([`written`]).
    pub old: &'a [String],
    /// The same lines as they were last said, one for each of
    /// [`Self::old`]: a line that only got shorter since it was said keeps
    /// what it said ([`Found::said`]), so a progress line read half
    /// rewritten says "51%", not "oading 51%" (`docs/design/terminal-reading.md`,
    /// extra 11). A line missing here said what it read.
    pub said: &'a [String],
    /// The snapshot's lines, padding trimmed, oldest first.
    pub lines: &'a [String],
    /// The screen's height in lines.
    pub height: usize,
    /// The line of [`Self::lines`] the caret is on, located by the caret's
    /// row text, or `None` when that text was not found in the snapshot.
    pub caret: Option<usize>,
}

/// What a read found, in the order it is spoken: [`Self::above`], then
/// [`Self::changed`], then [`Self::skipped`] and [`Self::appended`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// What is new above the old screen's last line, in screen order: lines
    /// inserted there, spoken whole, and what changed of lines rewritten in
    /// place there.
    pub above: Vec<String>,
    /// The caret's line rewritten in place, where typing shows; when the
    /// caret's line was not rewritten, the old screen's last line rewritten
    /// in place. `None` when neither was.
    pub changed: Option<LineChange>,
    /// Lines of the history that are not in [`Self::appended`], set only by
    /// the whole-document read's overflow verdict.
    pub skipped: Option<Skipped>,
    /// The lines after the old screen's last line kept, oldest first, blank
    /// ones as empty strings: every one of them, for the flood policy to
    /// cut.
    pub appended: Vec<String>,
    /// Lines of the snapshot, by their index in [`Read::lines`], that only
    /// got shorter than what they said, with what they said, which the
    /// memory keeps as their [`Read::said`] for the next read. Every other
    /// line of the snapshot said what it read.
    pub said: Vec<(usize, String)>,
    /// Whether the normal read did not find the old screen: it had a line
    /// that is not blank, and none of its lines that are not blank is in
    /// the snapshot's aligned lines, nor was its last line found grown in
    /// place. Everything else is then empty, and the outpost takes the
    /// whole-document read ([`diff_document`]). Never set by that read.
    pub old_missing: bool,
}

/// The work one diff did, which tests assert exactly
/// (`docs/design/terminal-reading-review.md`, section 5, "The proposed
/// work-bound test").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    /// How many lines of the snapshot were aligned with the old screen: the
    /// alignment window.
    pub aligned_new: usize,
    /// How many lines of the old screen were aligned.
    pub aligned_old: usize,
    /// How many times two whole lines were compared: tested for equality,
    /// or for one starting with the other.
    pub compared: usize,
}

/// The indices of the lines of `lines` that count as lines
/// (`docs/design/terminal-reading.md`, extra 7): every line that is not
/// blank, and a blank line only when it is above the caret's line, since
/// the cursor has passed it; a blank row at or below the caret is not a
/// line yet. With no caret, or one past the lines, the blank lines at the
/// end are the ones that are not lines yet.
#[must_use]
pub fn written(lines: &[String], caret: Option<usize>) -> Vec<usize> {
    if let Some(caret) = caret.filter(|&caret| caret < lines.len()) {
        return (0..lines.len())
            .filter(|&index| index < caret || !is_blank(&lines[index]))
            .collect();
    }
    let end = lines
        .iter()
        .rposition(|line| !is_blank(line))
        .map_or(0, |last| last + 1);
    (0..end).collect()
}

/// The normal read's diff: `read.lines` start at the top of the old screen
/// as it was.
#[must_use]
pub fn diff_read(read: &Read<'_>) -> (Found, Work) {
    let rows = written(read.lines, read.caret);
    let new: Vec<&str> = rows.iter().map(|&row| read.lines[row].as_str()).collect();
    let caret = read.caret.and_then(|caret| rows.binary_search(&caret).ok());
    let mut work = Work::default();
    let found = align(read, &new, &rows, caret, true, &mut work);
    (found, work)
}

/// The whole-document read's diff, `read.lines` being the whole document:
/// the old screen is found by content, from the document's end backward,
/// and the lines after it diffed as [`diff_read`] does. Each candidate is
/// a line that is the old screen's last line that is not blank, or that
/// line grown, with every old line above it standing above it unchanged;
/// the nearest the end wins, which gives the smallest, and so honest,
/// count. The work is bounded by the document's lines times the old
/// screen's (`docs/design/terminal-reading-review.md`, section 5, "The diff
/// window").
///
/// Not found, in a document with more lines than a screen, the history
/// overflowed past the old screen (`docs/design/terminal-reading.md`, extra
/// 3; the review's section 6 states the rule as "old screen not found in a
/// document that has history"): the screen's lines are appended and the
/// history's other lines are [`Skipped::MoreThan`], or
/// [`Skipped::Uncounted`] when they are one line repeated, since identical
/// lines with a full history cannot be told apart and any count would be a
/// guess (extra 2). Not found in a document no longer than a screen, the
/// screen was cleared or another buffer shown, and the document is diffed
/// from its top.
#[must_use]
pub fn diff_document(read: &Read<'_>) -> (Found, Work) {
    let rows = written(read.lines, read.caret);
    let new: Vec<&str> = rows.iter().map(|&row| read.lines[row].as_str()).collect();
    let caret = read.caret.and_then(|caret| rows.binary_search(&caret).ok());
    let mut work = Work::default();
    let Some(sought) = read.old.iter().rposition(|line| !is_blank(line)) else {
        // Nothing of the old screen to find: every line is new.
        let found = align(read, &new, &rows, caret, false, &mut work);
        return (found, work);
    };
    let top = (sought..new.len()).rev().find_map(|end| {
        work.compared += 1;
        if !new[end].starts_with(read.old[sought].as_str()) {
            return None;
        }
        let top = end - sought;
        let above_stands = (0..sought).rev().all(|index| {
            work.compared += 1;
            new[top + index] == read.old[index]
        });
        above_stands.then_some(top)
    });
    if let Some(top) = top {
        let caret = caret.and_then(|caret| caret.checked_sub(top));
        let found = align(read, &new[top..], &rows[top..], caret, false, &mut work);
        return (found, work);
    }
    if new.len() <= read.height {
        let found = align(read, &new, &rows, caret, false, &mut work);
        return (found, work);
    }
    let unspoken = new.len() - read.height;
    let history = &new[..unspoken];
    let repeated = history.len() >= 2
        && history[1..].iter().all(|line| {
            work.compared += 1;
            *line == history[0]
        });
    let found = Found {
        skipped: Some(if repeated {
            Skipped::Uncounted
        } else {
            Skipped::MoreThan(u32::try_from(unspoken).unwrap_or(u32::MAX))
        }),
        appended: new[unspoken..]
            .iter()
            .map(|&line| line.to_owned())
            .collect(),
        ..Found::default()
    };
    (found, work)
}

/// Whether a line is blank.
fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// What became of a line of the snapshot in the alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fate {
    /// Not an old line: inserted.
    Inserted,
    /// The old line at this index, unchanged.
    Kept(usize),
    /// The old line at this index, rewritten in place.
    Rewritten(usize),
}

/// Aligns `read.old` with `new`, the snapshot's lines that count, `rows`
/// being each one's index in `read.lines`, and `caret` the line of `new`
/// the caret is on. With `require_old`, an old screen not found is
/// [`Found::old_missing`].
fn align(
    read: &Read<'_>,
    new: &[&str],
    rows: &[usize],
    caret: Option<usize>,
    require_old: bool,
    work: &mut Work,
) -> Found {
    let old = read.old;
    let n = old.len();
    let window = new.len().min(n + read.height + 1);
    work.aligned_old += n;
    work.aligned_new += window;
    let mut fates = vec![Fate::Inserted; window];
    let mut old_found = false;

    // The common start: the lines above the old last line unchanged, then
    // the old last line, which output is written at and after, as it was
    // or grown. That is the usual case, and it is linear.
    let mut start = 0;
    while start + 1 < n && start < window {
        work.compared += 1;
        if old[start] != new[start] {
            break;
        }
        fates[start] = Fate::Kept(start);
        old_found |= !is_blank(&old[start]);
        start += 1;
    }
    if n > 0 && start == n - 1 && start < window {
        work.compared += 1;
        if new[start].starts_with(old[start].as_str()) {
            fates[start] = if new[start].len() == old[start].len() {
                Fate::Kept(start)
            } else {
                Fate::Rewritten(start)
            };
            old_found = true;
            start = n;
        }
    }

    if start < n {
        // The common end, within the window.
        let mut end = 0;
        while end < n - start && end < window - start {
            work.compared += 1;
            if old[n - 1 - end] != new[window - 1 - end] {
                break;
            }
            fates[window - 1 - end] = Fate::Kept(n - 1 - end);
            old_found |= !is_blank(&old[n - 1 - end]);
            end += 1;
        }
        // The rest by their longest common subsequence of lines.
        for (old_index, new_index) in
            common_lines(&old[start..n - end], &new[start..window - end], start, work)
        {
            fates[new_index] = Fate::Kept(old_index);
            old_found |= !is_blank(&old[old_index]);
        }
        pair_rewrites(old, new, &mut fates);
    }

    if require_old && !old_found && old.iter().any(|line| !is_blank(line)) {
        return Found {
            old_missing: true,
            ..Found::default()
        };
    }
    speak(read, new, rows, caret, &fates)
}

/// The longest common subsequence of `old` and `new`, which both start at
/// line `offset` of their screens, as pairs of indices on those screens.
/// Where alignments tie, as identical lines make them, the one keeping
/// lines nearest their own position wins, since the snapshot starts at the
/// old screen's top (`docs/design/terminal-reading.md`, "The diff").
fn common_lines(
    old: &[String],
    new: &[&str],
    offset: usize,
    work: &mut Work,
) -> Vec<(usize, usize)> {
    let (rows, columns) = (old.len(), new.len());
    if rows == 0 || columns == 0 {
        return Vec::new();
    }
    let equal: Vec<bool> = old
        .iter()
        .flat_map(|old_line| new.iter().map(move |new_line| old_line == new_line))
        .collect();
    work.compared += rows * columns;
    // `longest[i * (columns + 1) + j]`: the longest common subsequence of
    // `old[i..]` and `new[j..]`.
    let width = columns + 1;
    let mut longest = vec![0_usize; (rows + 1) * width];
    for i in (0..rows).rev() {
        for j in (0..columns).rev() {
            longest[i * width + j] = if equal[i * columns + j] {
                longest[(i + 1) * width + j + 1] + 1
            } else {
                longest[(i + 1) * width + j].max(longest[i * width + j + 1])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < rows && j < columns {
        if equal[i * columns + j] {
            pairs.push((offset + i, offset + j));
            i += 1;
            j += 1;
            continue;
        }
        let without_old = longest[(i + 1) * width + j];
        let without_new = longest[i * width + j + 1];
        // On a tie, the step that brings the two positions together.
        if without_old > without_new || (without_old == without_new && j >= i) {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Pairs old lines that were not kept with new lines in the same region,
/// between the same two kept lines (or the screen's start or the window's
/// end), as lines rewritten in place. One old line pairs with the new line
/// sharing the longest start with it, the first on a tie, when they share
/// any: a footer redrawn below lines written above it pairs with its new
/// version. Replaced by one line sharing nothing with it, it pairs only
/// when one of the two is blank: a row erased, or an erased row written
/// again. Several pair first to first when at least as many new lines
/// replace them (a status block, a selection list's marker); when fewer
/// do, as when a screen is cleared down to its prompt, none pair, and the
/// new lines are spoken whole.
fn pair_rewrites(old: &[String], new: &[&str], fates: &mut [Fate]) {
    let mut kept: Vec<(usize, usize)> = fates
        .iter()
        .enumerate()
        .filter_map(|(new_index, fate)| match fate {
            Fate::Kept(old_index) | Fate::Rewritten(old_index) => Some((*old_index, new_index)),
            Fate::Inserted => None,
        })
        .collect();
    kept.push((old.len(), fates.len()));
    let (mut old_from, mut new_from) = (0, 0);
    for (old_to, new_to) in kept {
        let (gone, came) = (old_from..old_to, new_from..new_to);
        if gone.len() == 1 && !came.is_empty() {
            let was = old[gone.start].as_str();
            let shared = |index: &usize| {
                was.chars()
                    .zip(new[*index].chars())
                    .take_while(|(a, b)| a == b)
                    .count()
            };
            let best = came.clone().map(|index| (shared(&index), index)).fold(
                (0, came.start),
                |best, next| if next.0 > best.0 { next } else { best },
            );
            let erased = came.len() == 1 && (is_blank(was) || is_blank(new[came.start]));
            if best.0 > 0 || erased {
                fates[best.1] = Fate::Rewritten(gone.start);
            }
        } else if gone.len() > 1 && gone.len() <= came.len() {
            for (old_index, new_index) in gone.zip(came) {
                fates[new_index] = Fate::Rewritten(old_index);
            }
        }
        old_from = old_to + 1;
        new_from = new_to + 1;
    }
}

/// What the alignment `fates` of the window's lines of `new` says, in
/// speaking order, with the lines after the window appended.
fn speak(
    read: &Read<'_>,
    new: &[&str],
    rows: &[usize],
    caret: Option<usize>,
    fates: &[Fate],
) -> Found {
    let (old, said) = (read.old, read.said);
    let said_of = |old_index: usize| said.get(old_index).unwrap_or(&old[old_index]);
    // Where the last old line kept or rewritten is now: new lines before it
    // are above the old last line, the rest appended.
    let last_kept = fates.iter().rposition(|fate| *fate != Fate::Inserted);
    let old_last = old.len().checked_sub(1);
    let changed_at = caret
        .filter(|&caret| matches!(fates.get(caret), Some(Fate::Rewritten(_))))
        .or_else(|| {
            fates
                .iter()
                .position(|fate| Some(*fate) == old_last.map(Fate::Rewritten))
        });
    let mut found = Found::default();
    for (index, fate) in fates.iter().enumerate() {
        let line = new[index];
        let text = match *fate {
            Fate::Inserted => Some(line.to_owned()),
            Fate::Kept(old_index) => {
                keep_said(&mut found, rows[index], said_of(old_index), line);
                None
            }
            Fate::Rewritten(old_index) => {
                let was_said = said_of(old_index);
                keep_said(&mut found, rows[index], was_said, line);
                let since_read = if *was_said == old[old_index] {
                    None
                } else {
                    line_change(&old[old_index], line)
                };
                let change = with_since_read(line_change(was_said, line), since_read);
                if Some(index) == changed_at {
                    found.changed = change;
                    None
                } else {
                    change
                        .map(|change| change.text.trim_start().to_owned())
                        .filter(|text| !text.is_empty())
                }
            }
        };
        if let Some(text) = text {
            if last_kept.is_some_and(|last| index <= last) {
                found.above.push(text);
            } else {
                found.appended.push(text);
            }
        }
    }
    found
        .appended
        .extend(new[fates.len()..].iter().map(|&line| line.to_owned()));
    found
}

/// Records what the line at `row` of the snapshot, `line` now, said, when
/// it only got shorter than `said`: a row erased whole is drawn again, not
/// cut short, so what is written to it next is new.
fn keep_said(found: &mut Found, row: usize, said: &str, line: &str) {
    if !is_blank(line) && said != line && said.starts_with(line) {
        found.said.push((row, said.to_owned()));
    }
}

/// A line's change from what it said, with its change since it was read
/// beside it where the two differ ([`LineChange::since_read`]); a line
/// whose change since it was read brought it back to what it said is a
/// change with nothing to speak.
fn with_since_read(said: Option<LineChange>, since_read: Option<LineChange>) -> Option<LineChange> {
    let Some(since_read) = since_read else {
        return said;
    };
    match said {
        Some(said) if said == since_read => Some(said),
        Some(said) => Some(LineChange {
            since_read: Some(Box::new(since_read)),
            ..said
        }),
        None => Some(LineChange {
            text: String::new(),
            line: since_read.line.clone(),
            appended: false,
            uncertain: 0,
            inserted: String::new(),
            since_read: Some(Box::new(since_read)),
        }),
    }
}

/// How a line changed in place (`docs/parity/terminals.md`, "Reading
/// output"; `docs/design/terminal-reading.md`, extra 11), `old` and `new`
/// being the line without its padding before and now. `None` when it is
/// the same, only lost text or gained only white space, or is a spinner:
/// one symbol replaced by another and nothing else changed, silent until
/// the line changes to text (Dickson, 2026-10-07). A line that grew at its
/// end gives exactly what it gained, the white space at its start being
/// [`LineChange::uncertain`], since a line's own trailing space cannot be
/// told from padding. Any other change speaks the line from the start of
/// the word where it first differs, by Unicode's word rules with ICU's
/// dictionaries and in whole graphemes, or from the word after it when
/// that is white space.
#[must_use]
pub fn line_change(old: &str, new: &str) -> Option<LineChange> {
    if old.starts_with(new) {
        return None;
    }
    if let Some(added) = new.strip_prefix(old) {
        return Some(LineChange {
            text: added.to_owned(),
            line: new.to_owned(),
            appended: true,
            uncertain: added.len() - added.trim_start().len(),
            inserted: added.to_owned(),
            since_read: None,
        });
    }
    let was = graphemes(old);
    let now = graphemes(new);
    let same =
        |a: &std::ops::Range<usize>, b: &std::ops::Range<usize>| old[a.clone()] == new[b.clone()];
    let first = was.iter().zip(&now).take_while(|(a, b)| same(a, b)).count();
    let same_after = was[first..]
        .iter()
        .rev()
        .zip(now[first..].iter().rev())
        .take_while(|(a, b)| same(a, b))
        .count();
    let symbol = |grapheme: &str| !is_blank(grapheme) && !is_word_grapheme(grapheme);
    let one_replaced = was.len() == now.len() && same_after + first + 1 == now.len();
    if one_replaced && symbol(&old[was[first].clone()]) && symbol(&new[now[first].clone()]) {
        return None;
    }
    let at = now.get(first).map_or(new.len(), |range| range.start);
    let gained_end = now
        .get(now.len() - same_after)
        .map_or(new.len(), |range| range.start)
        .max(at);
    let gained = &new[at..gained_end];
    if is_blank(gained) {
        return None;
    }
    let words = Segmenter::new().words(new, WordRules::for_text(new, None));
    let mut word = words
        .iter()
        .find(|word| word.contains(&at))
        .map_or(at, |word| word.start);
    if new[word..].starts_with(char::is_whitespace) {
        word = words
            .iter()
            .find(|next| next.start > word)
            .map_or(new.len(), |next| next.start);
    }
    let text = new[word..].trim_end();
    if text.is_empty() {
        return None;
    }
    Some(LineChange {
        text: text.to_owned(),
        line: new.to_owned(),
        appended: false,
        uncertain: 0,
        inserted: gained.to_owned(),
        since_read: None,
    })
}

#[cfg(test)]
mod tests;
