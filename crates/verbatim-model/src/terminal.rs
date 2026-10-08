//! New output in a terminal (milestone M4 item 9; `phase6-design.md`,
//! "Terminal output: notifications or diffing"): what an outpost's diff of
//! a terminal's text found since its last read, sent to Core as
//! [`NormalizedEvent::TerminalOutput`](crate::NormalizedEvent::TerminalOutput).
//!
//! The outpost never reads a terminal's whole buffer. Each read fetches the
//! screen and diffs it by line with the screen as last seen
//! (`phase6-design.md`, "Terminal reading by diffing the screen"), and
//! finds the last screen's top row again by its text to count the lines
//! that scrolled by unread. What it found arrives in the order it is
//! spoken: what is new above the last line read, that line where it
//! changed in place, the lines that went by unread, then the newest lines.
//! Lines are sent without their trailing padding; blank ones are sent as
//! empty strings, which Core counts as lines but does not speak. A line is
//! sent whole, however long (Dickson, 2026-10-07); Core bounds what it
//! keeps waiting.

use serde::{Deserialize, Serialize};

/// What a terminal's diff found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOutput {
    /// What is new on the screen above the last line read before: lines
    /// inserted there, and what changed of lines rewritten in place there
    /// (a message printed above a prompt, a status block, a selection
    /// list's marker).
    #[serde(default)]
    pub above: Vec<String>,
    /// The last line read before, changed in place since: a prompt that
    /// grew, or a progress bar rewritten. `None` when it is unchanged.
    pub changed: Option<LineChange>,
    /// When lines went by unread, the first new lines, oldest first, before
    /// them: the start of a flood, which is spoken in full ("Lines spoken in
    /// full"). Each as in [`Self::lines`], at most as many. Empty when every
    /// new line is in `lines`.
    #[serde(default)]
    pub head: Vec<String>,
    /// Lines that went by unread, between [`Self::head`] (or the changed
    /// line) and [`Self::lines`]. `None` when every new line is in `head`
    /// and `lines`.
    pub skipped: Option<Skipped>,
    /// The newest lines, oldest first, each without its trailing padding;
    /// an empty string is a blank line. At most `ReaderSettings::terminal_read_lines` of them.
    pub lines: Vec<String>,
}

impl TerminalOutput {
    /// Whether nothing at all changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changed.is_none()
            && self.above.is_empty()
            && self.head.is_empty()
            && self.skipped.is_none()
            && self.lines.is_empty()
    }
}

/// The line an outpost last read, changed in place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineChange {
    /// What to speak of it: the text added at its end when it only grew,
    /// otherwise the line from the start of the word where it first
    /// differs, without padding.
    pub text: String,
    /// The whole line as it is now, without padding: what Core speaks in
    /// place of an earlier version of the line still waiting to be
    /// spoken, so a line rewritten quickly is spoken once.
    pub line: String,
    /// Whether the line only grew at its end, so `text` is exactly the
    /// characters added, which may be the user's own typing showing.
    pub appended: bool,
    /// For a line that grew, the length in bytes of the white space at the
    /// start of `text` that may have been on the line already. Lines are
    /// compared without their trailing white space, which cannot be told
    /// from the terminal's padding, so a line's own trailing space (the
    /// space ending a prompt) comes back as if added once text follows it.
    /// Core matches typing after the part of it the typing does not start
    /// with.
    #[serde(default)]
    pub uncertain: usize,
    /// What the line gained where it changed, between what it kept at its
    /// start and at its end: for a line that grew, `text`; for one
    /// rewritten, the characters inserted (a typed character in the middle
    /// of a command), which Core matches with typing it holds, so a
    /// rewrite that does not show the typing (a clock ticking on a password
    /// prompt's line) never echoes it.
    #[serde(default)]
    pub inserted: String,
}

/// How many lines went by unread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Skipped {
    /// Exactly this many.
    Count(u32),
    /// Some, but how many is not known.
    Uncounted,
    /// More than this many: the terminal's history overflowed past the
    /// screen last read, so its oldest lines are gone, and this many are
    /// the lines the history holds that are not spoken.
    MoreThan(u32),
}

impl Skipped {
    /// This count followed by the `newer` one. Counts add, exact only when
    /// both are. A newer [`Skipped::MoreThan`] stands for everything before
    /// it: what went before it has left the history, and its count is the
    /// history's lines left unspoken. Lines counted after one add to it.
    #[must_use]
    pub fn plus(self, newer: Self) -> Self {
        match (self, newer) {
            (Self::Count(a), Self::Count(b)) => Self::Count(a.saturating_add(b)),
            (_, Self::MoreThan(b)) => Self::MoreThan(b),
            (Self::MoreThan(a), Self::Count(b)) => Self::MoreThan(a.saturating_add(b)),
            (_, Self::Uncounted) | (Self::Uncounted, Self::Count(_)) => Self::Uncounted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_counts_add_and_stay_exact_only_when_both_are() {
        assert_eq!(Skipped::Count(3).plus(Skipped::Count(4)), Skipped::Count(7));
        assert_eq!(
            Skipped::Count(3).plus(Skipped::Uncounted),
            Skipped::Uncounted
        );
        assert_eq!(
            Skipped::Uncounted.plus(Skipped::Count(1)),
            Skipped::Uncounted
        );
        // What went before a history's overflow is part of it.
        assert_eq!(
            Skipped::Count(40).plus(Skipped::MoreThan(9001)),
            Skipped::MoreThan(9001)
        );
        assert_eq!(
            Skipped::Uncounted.plus(Skipped::MoreThan(9001)),
            Skipped::MoreThan(9001)
        );
        assert_eq!(
            Skipped::MoreThan(9001).plus(Skipped::Count(10)),
            Skipped::MoreThan(9011)
        );
        assert_eq!(
            Skipped::MoreThan(9001).plus(Skipped::Uncounted),
            Skipped::Uncounted
        );
    }

    #[test]
    fn output_with_nothing_in_it_is_empty() {
        assert!(TerminalOutput::default().is_empty());
        let blank = TerminalOutput {
            lines: vec![String::new()],
            ..TerminalOutput::default()
        };
        assert!(!blank.is_empty());
    }
}
