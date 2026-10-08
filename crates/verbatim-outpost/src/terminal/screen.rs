//! The screen diff (`phase6-design.md`, "Terminal reading by diffing the
//! screen"): a pure function from the screen as last seen and the screen
//! now, as lines without their padding, to what is new.
//!
//! Lines are compared whole. When the read found how far the text scrolled
//! since the last one ([`Shift::Known`]), the old screen's lines that
//! scrolled off its top are set aside first, so the two screens line up as
//! the terminal moved them; otherwise they are lined up by their longest
//! common run of lines. Then:
//!
//! - Lines inserted are new: spoken whole.
//! - A line replaced by another in the same place speaks what changed of it
//!   ([`line_change`]): what it gained at its end, or from the start of the
//!   word that changed. A line that only lost text, and a spinner (a single
//!   symbol replaced by another), say nothing.
//! - Lines only deleted say nothing, as NVDA speaks only insertions.
//!
//! Every line is a line, blank ones included; whether a blank line is
//! spoken is Core's business (it is not).

use std::ops::Range;

use verbatim_model::LineChange;
use verbatim_text::{Segmenter, WordRules};

/// How far the text scrolled between the two screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    /// The old screen's top row now lies this many rows above the new
    /// screen's top.
    Known(usize),
    /// Not known: the screens are lined up by their common lines.
    Unknown,
}

/// What changed between two screens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenDiff {
    /// What is new above the old screen's last line, in screen order.
    pub above: Vec<String>,
    /// The old screen's last line, changed in place.
    pub changed: Option<LineChange>,
    /// What is new below it, in screen order.
    pub below: Vec<String>,
}

impl ScreenDiff {
    /// Whether nothing is new.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.above.is_empty() && self.changed.is_none() && self.below.is_empty()
    }
}

/// The lines of a screen's text as the provider gave it: each without its
/// line break and padding, and without the blank lines at the end of the
/// screen, which are rows not yet written to.
#[must_use]
pub fn screen_lines(text: &str) -> Vec<String> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut lines: Vec<String> = if body.is_empty() {
        Vec::new()
    } else {
        body.split('\n')
            .map(|line| {
                verbatim_text::trim_padding(line.strip_suffix('\r').unwrap_or(line)).to_owned()
            })
            .collect()
    };
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// What changed from `old` to `new`, both as [`screen_lines`] gives them.
/// `cursor`, when known, is the line of `new` the caret is on: its change
/// is the one `changed` reports, where typing shows, wherever it is (a
/// prompt above a status bar on the bottom row); otherwise the old screen's
/// last line's is.
#[must_use]
pub fn diff(old: &[String], new: &[String], shift: Shift, cursor: Option<usize>) -> ScreenDiff {
    let old = match shift {
        Shift::Known(rows) => &old[rows.min(old.len())..],
        Shift::Unknown => old,
    };
    let mut result = ScreenDiff::default();
    let Some(last) = old.len().checked_sub(1) else {
        // Nothing of the old screen is left on it: everything is new, and
        // comes after whatever went by unread.
        result.below = new.to_vec();
        return result;
    };
    // The old screen's last line is the one the terminal was writing to.
    // Where the new screen still has it in its place, as it was or grown,
    // output was written at it and after it: the lines above it are
    // compared on their own, and everything after it is new.
    if new
        .get(last)
        .is_some_and(|line| line.starts_with(old[last].as_str()))
    {
        let above = diff_lines(
            &old[..last],
            &new[..last],
            None,
            cursor.filter(|&cursor| cursor < last),
        );
        result.above = above.above;
        let last_change = line_change(&old[last], &new[last]);
        if above.changed.is_some() {
            // The caret is above the last line: its line's change is the
            // one typing shows in, and the last line's is output after it.
            result.changed = above.changed;
            result
                .below
                .extend(last_change.map(|change| change.text.trim_start().to_owned()));
        } else {
            result.changed = last_change;
        }
        result.below.extend(new[last + 1..].iter().cloned());
        return result;
    }
    diff_lines(old, new, Some(last), cursor)
}

/// The diff of `old` and `new` lined up by their longest common run of
/// lines, with the old line `last`, when given, as the boundary: new lines
/// before where it is now are above it, the rest below. The changed line is
/// the one replacing the line at `cursor` in `new` when given, otherwise
/// the one replacing `last`.
fn diff_lines(
    old: &[String],
    new: &[String],
    last: Option<usize>,
    cursor: Option<usize>,
) -> ScreenDiff {
    let hunks = hunks(old, new);
    // Where the old last line is in the new screen: its match, or its place
    // in the hunk that replaced it.
    let boundary = last.map(|last| {
        let mut shift: isize = 0;
        for hunk in &hunks {
            if hunk.old.contains(&last) {
                return hunk.new.start + (last - hunk.old.start).min(hunk.new.len());
            }
            if hunk.old.end <= last {
                shift += hunk.new.len().cast_signed() - hunk.old.len().cast_signed();
            }
        }
        last.saturating_add_signed(shift)
    });
    let mut result = ScreenDiff::default();
    for hunk in hunks {
        let paired = hunk.old.len().min(hunk.new.len());
        for (offset, new_index) in hunk.new.clone().enumerate() {
            let old_index = hunk.old.start + offset;
            let spoken = if offset < paired {
                let change = line_change(&old[old_index], &new[new_index]);
                let is_changed = match cursor {
                    Some(cursor) => new_index == cursor,
                    None => Some(old_index) == last,
                };
                if is_changed {
                    result.changed = change;
                    continue;
                }
                change.map(|change| change.text.trim_start().to_owned())
            } else {
                Some(new[new_index].clone())
            };
            if let Some(text) = spoken {
                if boundary.is_some_and(|boundary| new_index > boundary) {
                    result.below.push(text);
                } else {
                    result.above.push(text);
                }
            }
        }
    }
    result
}

/// A run of old lines replaced by a run of new ones, either possibly empty.
#[derive(Debug, PartialEq, Eq)]
struct Hunk {
    old: Range<usize>,
    new: Range<usize>,
}

/// The hunks that turn `old` into `new`, in order, by their longest common
/// subsequence of lines. Lines past the end of `old` that are new are one
/// last hunk.
fn hunks(old: &[String], new: &[String]) -> Vec<Hunk> {
    let (n, m) = (old.len(), new.len());
    // `table[i][j]`: the longest common subsequence of `old[i..]` and
    // `new[j..]`.
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    let (mut old_start, mut new_start) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            if old_start < i || new_start < j {
                hunks.push(Hunk {
                    old: old_start..i,
                    new: new_start..j,
                });
            }
            i += 1;
            j += 1;
            old_start = i;
            new_start = j;
        } else if j < m && (i == n || table[i][j + 1] >= table[i + 1][j]) {
            j += 1;
        } else {
            i += 1;
        }
    }
    if old_start < n || new_start < m {
        hunks.push(Hunk {
            old: old_start..n,
            new: new_start..m,
        });
    }
    hunks
}

/// How a line changed in place, `None` when it did not, only lost text, or
/// is a spinner (one symbol replaced by another, nothing else changed):
/// what it gained at its end, or, rewritten, the line from the start of the
/// word that changed, by Unicode's word rules (with ICU's dictionaries for
/// scripts written without spaces) and whole graphemes. `old` and `new` are
/// lines without their padding. White space at the start of what a line
/// gained is [`LineChange::uncertain`].
#[must_use]
pub fn line_change(old: &str, new: &str) -> Option<LineChange> {
    if old == new || old.starts_with(new) {
        return None;
    }
    if let Some(added) = new.strip_prefix(old) {
        // The line is compared without its padding, so white space it
        // gained at its end may have been its own (a prompt's trailing
        // space), which cannot be told from padding.
        return Some(LineChange {
            text: added.to_owned(),
            line: new.to_owned(),
            appended: true,
            uncertain: added.len() - added.trim_start().len(),
            inserted: added.to_owned(),
        });
    }
    let old_graphemes = verbatim_text::graphemes(old);
    let new_graphemes = verbatim_text::graphemes(new);
    let differs = old_graphemes
        .iter()
        .zip(&new_graphemes)
        .position(|(a, b)| old[a.clone()] != new[b.clone()])
        .unwrap_or(old_graphemes.len().min(new_graphemes.len()));
    if is_spinner(old, new, &old_graphemes, &new_graphemes, differs) {
        return None;
    }
    let at = new_graphemes
        .get(differs)
        .map_or(new.len(), |range| range.start);
    // What the line gained between what it kept at its start and at its
    // end: nothing, or only white space (a marker cleared), is a deletion,
    // which is not spoken.
    let kept_at_end = old_graphemes[differs..]
        .iter()
        .rev()
        .zip(new_graphemes[differs..].iter().rev())
        .take_while(|(a, b)| old[(*a).clone()] == new[(*b).clone()])
        .count();
    let gained_end = new_graphemes
        .len()
        .checked_sub(kept_at_end)
        .and_then(|index| new_graphemes.get(index))
        .map_or(new.len(), |range| range.start);
    if new[at..gained_end.max(at)].trim().is_empty() {
        return None;
    }
    let segmenter = Segmenter::new();
    let words = segmenter.words(new, WordRules::for_text(new, None));
    let mut start = words
        .iter()
        .find(|word| word.contains(&at))
        .map_or(at, |word| word.start);
    // A change at white space speaks from the word after it.
    if new[start..].starts_with(char::is_whitespace) {
        start = words
            .iter()
            .find(|word| word.start > start)
            .map_or(new.len(), |word| word.start);
    }
    let text = new[start..].trim_end();
    if text.is_empty() {
        return None;
    }
    Some(LineChange {
        text: text.to_owned(),
        line: new.to_owned(),
        appended: false,
        uncertain: 0,
        inserted: new[at..gained_end.max(at)].to_owned(),
    })
}

/// Whether `new` is `old` with the one grapheme at `differs` replaced by
/// another, both symbols rather than letters, marks, or numbers: a spinner,
/// which is silent until the line changes to text (Dickson, 2026-10-07).
fn is_spinner(
    old: &str,
    new: &str,
    old_graphemes: &[Range<usize>],
    new_graphemes: &[Range<usize>],
    differs: usize,
) -> bool {
    if old_graphemes.len() != new_graphemes.len() || differs >= old_graphemes.len() {
        return false;
    }
    let after_same = old_graphemes[differs + 1..]
        .iter()
        .zip(&new_graphemes[differs + 1..])
        .all(|(a, b)| old[a.clone()] == new[b.clone()]);
    let symbol = |text: &str| !text.trim().is_empty() && !verbatim_text::is_word_grapheme(text);
    after_same
        && symbol(&old[old_graphemes[differs].clone()])
        && symbol(&new[new_graphemes[differs].clone()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| (*line).to_owned()).collect()
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "compared with `line_change`'s answers, which are options"
    )]
    fn change(text: &str, line: &str, appended: bool) -> Option<LineChange> {
        Some(LineChange {
            text: text.to_owned(),
            line: line.to_owned(),
            appended,
            uncertain: if appended {
                text.len() - text.trim_start().len()
            } else {
                0
            },
            inserted: if appended {
                text.to_owned()
            } else {
                String::new()
            },
        })
    }

    #[test]
    fn a_screens_lines_lose_padding_breaks_and_unwritten_rows() {
        assert_eq!(
            screen_lines("ready> echo  \r\nhello   \r\n\u{3000}\u{a0} \r\n   \r\n"),
            ["ready> echo", "hello"]
        );
        assert_eq!(screen_lines("a\r\n\r\nb\r\n"), ["a", "", "b"]);
        assert_eq!(screen_lines("a\nb"), ["a", "b"]);
        assert_eq!(screen_lines(""), Vec::<String>::new());
        assert_eq!(screen_lines("   \r\n  "), Vec::<String>::new());
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "compared with `line_change`'s answers, which are options"
    )]
    fn rewrite(text: &str, line: &str, inserted: &str) -> Option<LineChange> {
        Some(LineChange {
            text: text.to_owned(),
            line: line.to_owned(),
            appended: false,
            uncertain: 0,
            inserted: inserted.to_owned(),
        })
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "one table of cases")]
    fn insertions_are_spoken_and_deletions_are_not() {
        let cases: &[(&[&str], &[&str], Shift, ScreenDiff)] = &[
            // Output at the end, after the prompt line that grew.
            (
                &["ready>"],
                &["ready> echo hi", "hi", "ready>"],
                Shift::Known(0),
                ScreenDiff {
                    above: Vec::new(),
                    changed: change(" echo hi", "ready> echo hi", true),
                    below: lines(&["hi", "ready>"]),
                },
            ),
            // The screen scrolled by two lines: only the last two are new.
            (
                &["a", "b", "c"],
                &["c", "d", "e"],
                Shift::Known(2),
                ScreenDiff {
                    above: Vec::new(),
                    changed: None,
                    below: lines(&["d", "e"]),
                },
            ),
            // Scrolled past the whole old screen: everything is new.
            (
                &["a", "b"],
                &["x", "y"],
                Shift::Known(5),
                ScreenDiff {
                    below: lines(&["x", "y"]),
                    ..ScreenDiff::default()
                },
            ),
            // Inserted in the middle, above a fixed footer.
            (
                &["one", "two", "-- footer --"],
                &["one", "two", "three", "-- footer --"],
                Shift::Known(0),
                ScreenDiff {
                    above: lines(&["three"]),
                    ..ScreenDiff::default()
                },
            ),
            // A message printed above the prompt while typing.
            (
                &["out", "ready> ech"],
                &["out", "Message", "ready> ech"],
                Shift::Known(0),
                ScreenDiff {
                    above: lines(&["Message"]),
                    ..ScreenDiff::default()
                },
            ),
            // Deletion only.
            (
                &["a", "b", "c"],
                &["a", "c"],
                Shift::Known(0),
                ScreenDiff::default(),
            ),
            // Scrolling up in a full-screen program: a line appears at the
            // top, one leaves at the bottom.
            (
                &["2", "3", "4"],
                &["1", "2", "3"],
                Shift::Unknown,
                ScreenDiff {
                    above: lines(&["1"]),
                    ..ScreenDiff::default()
                },
            ),
            // Scrolling down a line in a full-screen program.
            (
                &["1", "2", "3"],
                &["2", "3", "4"],
                Shift::Unknown,
                ScreenDiff {
                    below: lines(&["4"]),
                    ..ScreenDiff::default()
                },
            ),
            // Repeated identical lines, lined up by the shift.
            (
                &["x", "x", "x"],
                &["x", "x", "x"],
                Shift::Known(1),
                ScreenDiff {
                    below: lines(&["x"]),
                    ..ScreenDiff::default()
                },
            ),
            // A redraw with the same text.
            (
                &["a", "b"],
                &["a", "b"],
                Shift::Known(0),
                ScreenDiff::default(),
            ),
            // A selection list's marker moved by two characters.
            (
                &["> one", "  two", "  three"],
                &["  one", "> two", "  three"],
                Shift::Known(0),
                ScreenDiff {
                    above: lines(&["> two"]),
                    ..ScreenDiff::default()
                },
            ),
            // Several changes in one status block.
            (
                &["cpu 10%", "mem 20%", "ready>"],
                &["cpu 11%", "mem 25%", "ready>"],
                Shift::Known(0),
                ScreenDiff {
                    above: lines(&["11%", "25%"]),
                    ..ScreenDiff::default()
                },
            ),
            // Blank lines count as lines.
            (
                &["ready>"],
                &["ready>", "a", "", "b"],
                Shift::Known(0),
                ScreenDiff {
                    below: lines(&["a", "", "b"]),
                    ..ScreenDiff::default()
                },
            ),
        ];
        for (old, new, shift, expected) in cases {
            assert_eq!(
                diff(&lines(old), &lines(new), *shift, None),
                *expected,
                "{old:?} to {new:?} by {shift:?}"
            );
        }
    }

    #[test]
    fn a_changed_line_speaks_from_the_word_that_changed() {
        assert_eq!(
            line_change("Loading 10%", "Loading 20%"),
            rewrite("20%", "Loading 20%", "2")
        );
        assert_eq!(
            line_change("progress step one", "progress step two"),
            rewrite("two", "progress step two", "two")
        );
        // Grown.
        assert_eq!(
            line_change("ready>", "ready> ls"),
            change(" ls", "ready> ls", true)
        );
        // Only shorter, or the same.
        assert_eq!(line_change("ready> ls", "ready>"), None);
        assert_eq!(line_change("abc", "abc"), None);
        // A change at a space speaks from the next word.
        assert_eq!(line_change("a b", "a  c"), rewrite("c", "a  c", " c"));
        // Chinese has no spaces: the word the dictionary finds.
        assert_eq!(
            line_change("我们今天去学校", "我们明天去学校"),
            rewrite("明天去学校", "我们明天去学校", "明")
        );
        // Thai likewise.
        assert_eq!(
            line_change("สวัสดีครับ", "สวัสดีค่ะ"),
            rewrite("ค่ะ", "สวัสดีค่ะ", "ค่ะ")
        );
        // A combining mark changes its whole grapheme and word.
        assert_eq!(
            line_change("cafe noir", "cafe\u{301} noir"),
            rewrite("cafe\u{301} noir", "cafe\u{301} noir", "e\u{301}")
        );
        // Wide characters and tabs are characters like any other.
        assert_eq!(
            line_change("名前\tA", "名前\tB"),
            rewrite("B", "名前\tB", "B")
        );
    }

    #[test]
    fn a_line_that_only_lost_text_or_gained_white_space_is_silent() {
        assert_eq!(line_change("echo hello", "echo hllo"), None);
        assert_eq!(line_change("> one", "  one"), None);
        assert_eq!(line_change("a-b", "a b"), None);
    }

    #[test]
    fn a_spinner_is_silent_until_the_line_changes_to_text() {
        assert_eq!(line_change("Working |", "Working /"), None);
        assert_eq!(line_change("⠋ build", "⠙ build"), None);
        assert_eq!(
            line_change("Working |", "Working done"),
            rewrite("done", "Working done", "done")
        );
        // A letter replaced is not a spinner.
        assert_eq!(line_change("step a", "step b"), rewrite("b", "step b", "b"));
    }

    #[test]
    fn symbol_only_lines_are_spoken() {
        assert_eq!(
            diff(
                &lines(&["a"]),
                &lines(&["a", "~", "────"]),
                Shift::Known(0),
                None
            ),
            ScreenDiff {
                below: lines(&["~", "────"]),
                ..ScreenDiff::default()
            }
        );
    }
}
