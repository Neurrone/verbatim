//! The alignment's computations (`books/dev/src/explanation/design/terminal-model.md`, "The
//! alignment"): the longest common subsequence of rows with the tie-break
//! of fewest blocks and then fewest rows moved, and the pairing of the
//! rows left between kept rows by likeness.

use std::collections::BTreeMap;

/// A kept row's worth in an alignment's score, above any number of blocks.
const KEPT: i64 = 1 << 42;
/// A block's cost in an alignment's score, above any number of rows moved.
const BLOCK: i64 = 1 << 21;
/// No predecessor.
const NONE: u32 = u32::MAX;

/// A kept pair found by the dynamic programming: old row `old` is new row
/// `new`, after the pair `prev` in the best alignment ending in it.
struct Match {
    old: usize,
    new: usize,
    prev: u32,
}

/// The longest common subsequence of the old rows `old` names and the new
/// rows `new` names (indices, each in increasing order), two rows kept when
/// `matches` says so of their positions in `old` and `new`, as pairs of an
/// old row's index and a new row's.
///
/// Among alignments that keep equally many rows, the one whose kept rows
/// move in the fewest blocks wins, a block being a run of kept pairs with
/// one shift (new index less old index), and then the one with the fewest
/// rows whose shift is not `fixed`, the shift of a row that did not move.
///
/// The dynamic programming runs over every cell of `old` by `new` once:
/// the best alignment ending in a kept pair either continues the block of
/// the best earlier pair on the same shift, which needs no new block, or
/// follows the best pair anywhere above and to the left of it, which
/// starts one; both are kept as running maxima, so each cell is a
/// comparison of two integers and a few additions.
pub(super) fn keep(
    old: &[usize],
    new: &[usize],
    matches: impl Fn(usize, usize) -> bool,
    fixed: isize,
) -> Vec<(usize, usize)> {
    let m = new.len();
    let (Some(&first_old), Some(&last_old), Some(&first_new), Some(&last_new)) =
        (old.first(), old.last(), new.first(), new.last())
    else {
        return Vec::new();
    };
    // Shifts run from the first new row less the last old row to the last
    // new row less the first old row.
    let low = isize_of(first_new).saturating_sub(isize_of(last_old));
    let span = usize::try_from(
        isize_of(last_new)
            .saturating_sub(isize_of(first_old))
            .saturating_sub(low)
            .saturating_add(1),
    )
    .unwrap_or(0);
    // Every cell starts with no pair, as each table is made.
    let empty = (0, NONE);
    let mut same: Vec<(i64, u32)> = vec![empty; span];
    let mut above: Vec<(i64, u32)> = vec![empty; m];
    let mut here: Vec<(i64, u32)> = vec![empty; m];
    let mut found: Vec<Match> = Vec::new();
    for (i, &old_index) in old.iter().enumerate() {
        let mut left = empty;
        for (j, &new_index) in new.iter().enumerate() {
            let mut best = above.get(j).copied().unwrap_or(empty);
            if left.0 > best.0 {
                best = left;
            }
            if matches(i, j) {
                let shift = isize_of(new_index).saturating_sub(isize_of(old_index));
                let moved = i64::from(shift != fixed);
                let diagonal = j
                    .checked_sub(1)
                    .and_then(|diagonal| above.get(diagonal))
                    .copied()
                    .unwrap_or(empty);
                let mut score = diagonal
                    .0
                    .saturating_add(KEPT)
                    .saturating_sub(BLOCK)
                    .saturating_sub(moved);
                let mut prev = diagonal.1;
                let slot = usize::try_from(shift.saturating_sub(low)).unwrap_or(0);
                let run = same.get(slot).copied().unwrap_or(empty);
                let continued = run.0.saturating_add(KEPT).saturating_sub(moved);
                if run.1 != NONE && continued > score {
                    score = continued;
                    prev = run.1;
                }
                let id = u32::try_from(found.len()).unwrap_or(NONE);
                found.push(Match {
                    old: old_index,
                    new: new_index,
                    prev,
                });
                if let Some(same) = same.get_mut(slot)
                    && score > same.0
                {
                    *same = (score, id);
                }
                if score > best.0 {
                    best = (score, id);
                }
            }
            if let Some(here) = here.get_mut(j) {
                *here = best;
            }
            left = best;
        }
        std::mem::swap(&mut above, &mut here);
    }
    let mut pairs = Vec::new();
    let mut at = above.last().map_or(NONE, |last| last.1);
    while at != NONE {
        let Some(pair) = usize::try_from(at).ok().and_then(|at| found.get(at)) else {
            break;
        };
        pairs.push((pair.old, pair.new));
        at = pair.prev;
    }
    pairs.reverse();
    pairs
}

fn isize_of(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

/// One row's characters, prepared for comparing by likeness.
pub(super) struct Likeness {
    chars: Vec<char>,
    /// How many 64-bit words a mask of the row's positions takes.
    words: usize,
    /// For each character the row holds, the bits of its positions,
    /// `words` words each.
    masks: Vec<u64>,
    /// For each ASCII character, one more than its mask's number, 0 for
    /// none; other characters' are in `other`.
    ascii: [u16; 128],
    other: BTreeMap<char, u16>,
}

impl Likeness {
    pub(super) fn of(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let words = chars.len().div_ceil(64).max(1);
        let mut likeness = Self {
            chars: Vec::new(),
            words,
            masks: Vec::new(),
            ascii: [0; 128],
            other: BTreeMap::new(),
        };
        for (at, &c) in chars.iter().enumerate() {
            let slot = likeness.slot(c).unwrap_or_else(|| {
                let slot = likeness.masks.len().checked_div(words).unwrap_or(0);
                likeness.masks.extend(std::iter::repeat_n(0, words));
                let number = u16::try_from(slot.saturating_add(1)).unwrap_or(u16::MAX);
                match usize::try_from(u32::from(c))
                    .ok()
                    .and_then(|code| likeness.ascii.get_mut(code))
                {
                    Some(entry) => *entry = number,
                    None => {
                        likeness.other.insert(c, number);
                    }
                }
                slot
            });
            let word = slot.saturating_mul(words).saturating_add(at / 64);
            // The slot's masks were added above, `words` of them.
            let Some(mask) = likeness.masks.get_mut(word) else {
                debug_assert!(false, "no mask {word} for slot {slot}");
                continue;
            };
            *mask |= 1 << (at % 64);
        }
        likeness.chars = chars;
        likeness
    }

    /// The number of `c`'s mask, if the row holds it.
    fn slot(&self, c: char) -> Option<usize> {
        // An ASCII character's number is in `ascii`, any other's in
        // `other`, and 0 for one not seen.
        let number = usize::try_from(u32::from(c))
            .ok()
            .and_then(|code| self.ascii.get(code))
            .or_else(|| self.other.get(&c))
            .copied()
            .unwrap_or(0);
        usize::from(number).checked_sub(1)
    }

    /// How many characters `other` shares with this row in order: the
    /// length of their longest common subsequence, by the bit-parallel
    /// method, one pass over `other` with a few word operations per
    /// character.
    fn common(&self, other: &[char]) -> usize {
        let words = self.words;
        let mut v = vec![u64::MAX; words];
        for &c in other {
            let Some(slot) = self.slot(c) else {
                continue;
            };
            let start = slot.saturating_mul(words);
            // Every slot has its `words` masks, as `of` makes them.
            let Some(mask) = self.masks.get(start..start.saturating_add(words)) else {
                debug_assert!(false, "no masks for slot {slot}");
                continue;
            };
            let mut carry = 0_u64;
            for (word, &mask) in v.iter_mut().zip(mask) {
                let kept = *word & mask;
                let rest = *word & !mask;
                let (sum, first) = word.overflowing_add(kept);
                let (sum, second) = sum.overflowing_add(carry);
                carry = u64::from(first || second);
                *word = sum | rest;
            }
        }
        // The bits past the row's length are outside it, whatever the
        // carries left in them.
        let mut zeros: u32 = 0;
        for (word, bits) in v.iter().enumerate() {
            let length = self
                .chars
                .len()
                .saturating_sub(word.saturating_mul(64))
                .min(64);
            let inside = if length == 64 {
                u64::MAX
            } else {
                (1_u64 << length).saturating_sub(1)
            };
            zeros = zeros.saturating_add((!bits & inside).count_ones());
        }
        usize::try_from(zeros).unwrap_or(0)
    }

    /// Whether two rows are alike ("The alignment"): neither blank, and
    /// sharing in order at least half the characters of the longer.
    pub(super) fn alike(&self, other: &Self) -> bool {
        let (a, b) = (&self.chars, &other.chars);
        if a.is_empty() || b.is_empty() {
            return false;
        }
        let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
        if short.len().saturating_mul(2) < long.len() {
            return false;
        }
        self.common(b).saturating_mul(2) >= long.len()
    }
}

/// The pairing of the rows left between two kept rows as rewrites: an
/// order-keeping matching of alike rows, as many pairs as there can be,
/// and among those the most whose shift (new index less old index) is
/// `prefer`, the shift of the kept row beside them. `old` and `new` give
/// each row's index and its likeness.
pub(super) fn pair(
    old: &[(usize, &Likeness)],
    new: &[(usize, &Likeness)],
    prefer: isize,
) -> Vec<(usize, usize)> {
    let (p, q) = (old.len(), new.len());
    if p == 0 || q == 0 {
        return Vec::new();
    }
    // best[a][b]: the best matching of old[a..] with new[b..], row by row
    // of `width` cells; past the last old or new row, 0, as made.
    let width = q.saturating_add(1);
    let cell = |a: usize, b: usize| a.saturating_mul(width).saturating_add(b);
    let mut best = vec![0_i64; p.saturating_add(1).saturating_mul(width)];
    // Every `a` up to `p` and `b` up to `q` has its cell.
    let value_at = |best: &[i64], a: usize, b: usize| {
        best.get(cell(a, b)).copied().unwrap_or_else(|| {
            debug_assert!(false, "no cell for {a}, {b}");
            0
        })
    };
    // alike[a][b], row by row of `q`.
    let alike: Vec<bool> = old
        .iter()
        .flat_map(|(_, old)| new.iter().map(|(_, new)| old.alike(new)))
        .collect();
    // What keeping old[a] as new[b] adds, when they are alike.
    let weight = |a: usize, b: usize| {
        let (Some((old_row, _)), Some((new_row, _)), Some(true)) = (
            old.get(a),
            new.get(b),
            alike.get(a.saturating_mul(q).saturating_add(b)),
        ) else {
            return None;
        };
        let shift = isize_of(*new_row).saturating_sub(isize_of(*old_row));
        Some(KEPT.saturating_add(i64::from(shift == prefer)))
    };
    for a in (0..p).rev() {
        for b in (0..q).rev() {
            let (below, right) = (a.saturating_add(1), b.saturating_add(1));
            let mut value = value_at(&best, below, b).max(value_at(&best, a, right));
            if let Some(weight) = weight(a, b) {
                value = value.max(value_at(&best, below, right).saturating_add(weight));
            }
            if let Some(cell) = best.get_mut(cell(a, b)) {
                *cell = value;
            }
        }
    }
    let mut pairs = Vec::new();
    let (mut a, mut b) = (0, 0);
    while a < p && b < q {
        let value = value_at(&best, a, b);
        let (below, right) = (a.saturating_add(1), b.saturating_add(1));
        if let Some(weight) = weight(a, b)
            && value == value_at(&best, below, right).saturating_add(weight)
            && let (Some((old_row, _)), Some((new_row, _))) = (old.get(a), new.get(b))
        {
            pairs.push((*old_row, *new_row));
            a = below;
            b = right;
            continue;
        }
        if value == value_at(&best, below, b) {
            a = below;
        } else {
            b = right;
        }
    }
    pairs
}
