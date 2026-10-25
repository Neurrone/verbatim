//! The alignment's computations (`docs/design/terminal-model.md`, "The
//! alignment"): the longest common subsequence of rows with the tie-break
//! of fewest blocks and then fewest rows moved, and the pairing of the
//! rows left between kept rows by likeness.

use std::collections::HashMap;

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
    let (n, m) = (old.len(), new.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }
    // Shifts run from the first new row less the last old row to the last
    // new row less the first old row.
    let low = isize_of(new[0]) - isize_of(old[n - 1]);
    let span = usize::try_from(isize_of(new[m - 1]) - isize_of(old[0]) - low + 1).unwrap_or(0);
    let mut same: Vec<(i64, u32)> = vec![(0, NONE); span];
    let mut above: Vec<(i64, u32)> = vec![(0, NONE); m];
    let mut here: Vec<(i64, u32)> = vec![(0, NONE); m];
    let mut found: Vec<Match> = Vec::new();
    for (i, &old_index) in old.iter().enumerate() {
        let mut left = (0, NONE);
        for j in 0..m {
            let new_index = new[j];
            let mut best = above[j];
            if left.0 > best.0 {
                best = left;
            }
            if matches(i, j) {
                let shift = isize_of(new_index) - isize_of(old_index);
                let moved = i64::from(shift != fixed);
                let diagonal = if j > 0 { above[j - 1] } else { (0, NONE) };
                let mut score = diagonal.0 + KEPT - BLOCK - moved;
                let mut prev = diagonal.1;
                let slot = usize::try_from(shift - low).unwrap_or(0);
                let run = same[slot];
                if run.1 != NONE && run.0 + KEPT - moved > score {
                    score = run.0 + KEPT - moved;
                    prev = run.1;
                }
                let id = u32::try_from(found.len()).unwrap_or(NONE);
                found.push(Match {
                    old: old_index,
                    new: new_index,
                    prev,
                });
                if score > same[slot].0 {
                    same[slot] = (score, id);
                }
                if score > best.0 {
                    best = (score, id);
                }
            }
            here[j] = best;
            left = best;
        }
        std::mem::swap(&mut above, &mut here);
    }
    let mut pairs = Vec::new();
    let mut at = above[m - 1].1;
    while at != NONE {
        let pair = &found[at as usize];
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
    other: HashMap<char, u16>,
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
            other: HashMap::new(),
        };
        for (at, &c) in chars.iter().enumerate() {
            let slot = likeness.slot(c).unwrap_or_else(|| {
                likeness.masks.extend(std::iter::repeat_n(0, words));
                let slot = likeness.masks.len() / words - 1;
                let number = u16::try_from(slot + 1).unwrap_or(u16::MAX);
                match usize::try_from(u32::from(c)) {
                    Ok(code) if code < 128 => likeness.ascii[code] = number,
                    _ => {
                        likeness.other.insert(c, number);
                    }
                }
                slot
            });
            likeness.masks[slot * words + at / 64] |= 1 << (at % 64);
        }
        likeness.chars = chars;
        likeness
    }

    /// The number of `c`'s mask, if the row holds it.
    fn slot(&self, c: char) -> Option<usize> {
        let number = match usize::try_from(u32::from(c)) {
            Ok(code) if code < 128 => self.ascii[code],
            _ => self.other.get(&c).copied().unwrap_or(0),
        };
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
            let mask = &self.masks[slot * words..(slot + 1) * words];
            let mut carry = 0_u64;
            for word in 0..words {
                let kept = v[word] & mask[word];
                let rest = v[word] & !mask[word];
                let (sum, first) = v[word].overflowing_add(kept);
                let (sum, second) = sum.overflowing_add(carry);
                carry = u64::from(first || second);
                v[word] = sum | rest;
            }
        }
        // The bits past the row's length are outside it, whatever the
        // carries left in them.
        let mut zeros = 0;
        for (word, bits) in v.iter().enumerate() {
            let length = (self.chars.len() - (word * 64).min(self.chars.len())).min(64);
            let inside = if length == 64 {
                u64::MAX
            } else {
                (1_u64 << length) - 1
            };
            zeros += (!bits & inside).count_ones();
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
        if short.len() * 2 < long.len() {
            return false;
        }
        self.common(b) * 2 >= long.len()
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
    // best[a][b]: the best matching of old[a..] with new[b..].
    let width = q + 1;
    let mut best = vec![0_i64; (p + 1) * width];
    let mut alike = vec![false; p * q];
    for a in 0..p {
        for b in 0..q {
            alike[a * q + b] = old[a].1.alike(new[b].1);
        }
    }
    for a in (0..p).rev() {
        for b in (0..q).rev() {
            let mut value = best[(a + 1) * width + b].max(best[a * width + b + 1]);
            if alike[a * q + b] {
                let shift = isize_of(new[b].0) - isize_of(old[a].0);
                let weight = KEPT + i64::from(shift == prefer);
                value = value.max(best[(a + 1) * width + b + 1] + weight);
            }
            best[a * width + b] = value;
        }
    }
    let mut pairs = Vec::new();
    let (mut a, mut b) = (0, 0);
    while a < p && b < q {
        let value = best[a * width + b];
        if alike[a * q + b] {
            let shift = isize_of(new[b].0) - isize_of(old[a].0);
            let weight = KEPT + i64::from(shift == prefer);
            if value == best[(a + 1) * width + b + 1] + weight {
                pairs.push((old[a].0, new[b].0));
                a += 1;
                b += 1;
                continue;
            }
        }
        if value == best[(a + 1) * width + b] {
            a += 1;
        } else {
            b += 1;
        }
    }
    pairs
}
