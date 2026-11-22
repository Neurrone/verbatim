//! The diff of a read against the last (`docs/parity/terminals.md`, "New
//! terminal output"): NVDA compares the whole text with the last and
//! speaks only what was inserted, so lines kept in order say nothing and a
//! line only deleted says nothing. Here the two windows' lines are aligned
//! as a longest common subsequence of equal lines, a line with text
//! weighing more than a blank one, so that blank rows never displace text;
//! each run of new lines left between two kept ones is then paired, in
//! order, with the old lines left there that it resembles, a changed line,
//! and the rest is inserted. A changed line's own inserted text is found by
//! the rules in the parent module.

use crate::proven::items_in;

use super::Window;

/// What became of a line of the new window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fate {
    /// The old line at this index, unchanged.
    Kept(usize),
    /// The old line at this index, changed.
    Paired(usize),
    /// A line the old window did not have.
    Inserted,
}

/// How much an equal pair of lines weighs in the alignment: a line with
/// text far more than a blank one, so that the blank rows a screen is full
/// of never pull the text out of place.
fn weight(text: &str) -> u32 {
    if text.is_empty() { 1 } else { 16 }
}

/// The most characters of a pair of lines whose common subsequence is
/// counted; longer pairs count their common start and end alone.
const SUBSEQUENCE_CELLS: usize = 250_000;

/// Whether `old` and `new`, unequal, are one line changed rather than two
/// lines (TM 5): they share at least half the longer one's characters in
/// order, or one is the start of the other (a line that grew or lost its
/// end), so a rewritten bar or status line pairs with its last version
/// and an unrelated line does not.
pub(super) fn resembles(old: &str, new: &str) -> bool {
    if old.is_empty() || new.is_empty() {
        return false;
    }
    if new.starts_with(old) || old.starts_with(new) {
        return true;
    }
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let longer = a.len().max(b.len());
    let shared = if a.len().saturating_mul(b.len()) <= SUBSEQUENCE_CELLS {
        common_subsequence(&a, &b)
    } else {
        let (prefix, suffix) = common_ends(old, new);
        let count = |part: Option<&str>| part.map_or(0, |part| part.chars().count());
        count(old.get(..prefix)).saturating_add(count(old.get(old.len().saturating_sub(suffix)..)))
    };
    shared.saturating_mul(2) >= longer
}

/// The length of the longest common subsequence of `a` and `b`.
fn common_subsequence(a: &[char], b: &[char]) -> usize {
    let mut last = vec![0usize; b.len().saturating_add(1)];
    let mut row = vec![0usize; b.len().saturating_add(1)];
    for x in a {
        for (j, y) in b.iter().enumerate() {
            let next = j.saturating_add(1);
            let value = if x == y {
                last.get(j).copied().unwrap_or(0).saturating_add(1)
            } else {
                last.get(next)
                    .copied()
                    .unwrap_or(0)
                    .max(row.get(j).copied().unwrap_or(0))
            };
            if let Some(cell) = row.get_mut(next) {
                *cell = value;
            }
        }
        std::mem::swap(&mut last, &mut row);
    }
    last.last().copied().unwrap_or(0)
}

/// The byte lengths of the longest common start and the longest common end
/// of `old` and `new`, on character boundaries and not overlapping in
/// either.
pub(super) fn common_ends(old: &str, new: &str) -> (usize, usize) {
    let prefix: usize = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let room = old.len().min(new.len()).saturating_sub(prefix);
    let mut suffix = 0usize;
    for (a, b) in old.chars().rev().zip(new.chars().rev()) {
        if a != b || suffix.saturating_add(a.len_utf8()) > room {
            break;
        }
        suffix = suffix.saturating_add(a.len_utf8());
    }
    (prefix, suffix)
}

/// The alignment of `new`'s lines with `old`'s: each new line's [`Fate`],
/// and for each old line whether a new line keeps or changes it.
pub(super) fn align(old: &Window, new: &Window) -> (Vec<Fate>, Vec<bool>) {
    let (olds, news): (Vec<&str>, Vec<&str>) = (
        old.lines.iter().map(|line| line.text.as_str()).collect(),
        new.lines.iter().map(|line| line.text.as_str()).collect(),
    );
    let (n, m) = (olds.len(), news.len());
    let columns = m.saturating_add(1);
    // `table[i][j]`: the heaviest alignment of `olds[i..]` with `news[j..]`.
    let mut table = vec![0u32; n.saturating_add(1).saturating_mul(columns)];
    let at = |i: usize, j: usize| i.saturating_mul(columns).saturating_add(j);
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let take = match (olds.get(i), news.get(j)) {
                (Some(left), Some(right)) if left == right => table
                    .get(at(i.saturating_add(1), j.saturating_add(1)))
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(weight(left)),
                _ => 0,
            };
            let skip_old = table.get(at(i.saturating_add(1), j)).copied().unwrap_or(0);
            let skip_new = table.get(at(i, j.saturating_add(1))).copied().unwrap_or(0);
            if let Some(cell) = table.get_mut(at(i, j)) {
                *cell = take.max(skip_old).max(skip_new);
            }
        }
    }
    let mut fates = vec![Fate::Inserted; m];
    let mut used = vec![false; n];
    let (mut i, mut j) = (0, 0);
    // The kept pairs, as old and new indices, in order.
    let mut kept: Vec<(usize, usize)> = Vec::new();
    while i < n && j < m {
        let here = table.get(at(i, j)).copied().unwrap_or(0);
        let equal = olds.get(i) == news.get(j);
        let diagonal = table
            .get(at(i.saturating_add(1), j.saturating_add(1)))
            .copied()
            .unwrap_or(0);
        if equal && here == diagonal.saturating_add(weight(olds.get(i).copied().unwrap_or(""))) {
            kept.push((i, j));
            i = i.saturating_add(1);
            j = j.saturating_add(1);
        } else if table.get(at(i.saturating_add(1), j)).copied().unwrap_or(0) == here {
            i = i.saturating_add(1);
        } else {
            j = j.saturating_add(1);
        }
    }
    for &(i, j) in &kept {
        if let Some(fate) = fates.get_mut(j) {
            *fate = Fate::Kept(i);
        }
        if let Some(used) = used.get_mut(i) {
            *used = true;
        }
    }
    // The gaps between kept pairs, each paired in order.
    let mut from = (0, 0);
    for &(i, j) in kept.iter().chain(std::iter::once(&(n, m))) {
        pair_gap(
            (items_in(&olds, from.0..i), from.0),
            (items_in(&news, from.1..j), from.1),
            (&mut fates, &mut used),
        );
        from = (i.saturating_add(1), j.saturating_add(1));
    }
    (fates, used)
}

/// Pairs the new lines of one gap, `new` from index `new_from`, with the
/// old lines of the same gap, `old` from `old_from`: each new line with the
/// next old line it resembles, the old lines passed over gone.
fn pair_gap(
    (old, old_from): (&[&str], usize),
    (new, new_from): (&[&str], usize),
    (fates, used): (&mut [Fate], &mut [bool]),
) {
    let mut next = 0usize;
    for (offset, line) in new.iter().enumerate() {
        let found = old
            .iter()
            .enumerate()
            .skip(next)
            .find(|(_, old_line)| resembles(old_line, line))
            .map(|(index, _)| index);
        if let Some(index) = found {
            let old_index = old_from.saturating_add(index);
            if let Some(fate) = fates.get_mut(new_from.saturating_add(offset)) {
                *fate = Fate::Paired(old_index);
            }
            if let Some(used) = used.get_mut(old_index) {
                *used = true;
            }
            next = index.saturating_add(1);
        }
    }
}

/// In the keys' window, the screens before and after compared row by row
/// (F8's larger form): a new line left inserted pairs with the old line
/// left gone whose first row was at the same place, so a list the keys
/// redraw is its rows changed, not new output.
pub(super) fn pair_by_place(old: &Window, new: &Window, (fates, used): (&mut [Fate], &mut [bool])) {
    for (j, fate) in fates.iter_mut().enumerate() {
        if *fate != Fate::Inserted || !new.on_screen(j) {
            continue;
        }
        let Some(place) = new.lines.get(j).map(super::Line::top) else {
            continue;
        };
        let gone = old
            .lines
            .iter()
            .enumerate()
            .find(|(i, line)| line.top() == place && used.get(*i) == Some(&false))
            .map(|(i, _)| i);
        if let Some(i) = gone {
            *fate = Fate::Paired(i);
            if let Some(used) = used.get_mut(i) {
                *used = true;
            }
        }
    }
}
