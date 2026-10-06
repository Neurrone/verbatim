//! New output in a terminal (milestone M4 item 9; `phase6-design.md`,
//! "Terminal output: notifications or diffing"): what an outpost's diff of
//! a terminal's text found since its last read, sent to Core as
//! [`NormalizedEvent::TerminalOutput`](crate::NormalizedEvent::TerminalOutput).
//!
//! The outpost never reads a terminal's whole buffer. It keeps an anchor at
//! the start of the last line it read, with that line's text and the text
//! of the line before it, and on each change reads only the lines it will
//! speak. What it found arrives in the order it is spoken: the anchor's own
//! line where it changed in place, then the lines that went by unread, then
//! the newest lines. Lines are sent without their trailing padding; blank
//! ones are sent as empty strings, and Core drops them.

use serde::{Deserialize, Serialize};

/// The most UTF-8 bytes of one terminal line an outpost sends; a longer line
/// is cut at a character boundary.
pub const MAX_TERMINAL_LINE_BYTES: usize = 4 * 1024;

/// What a terminal's diff found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOutput {
    /// The last line read before, changed in place since: a prompt that
    /// grew, or a progress bar rewritten. `None` when it is unchanged.
    pub changed: Option<LineChange>,
    /// Lines that went by unread, between the changed line and
    /// [`Self::lines`]. `None` when every new line is in `lines`.
    pub skipped: Option<Skipped>,
    /// The newest lines, oldest first, each without its trailing padding
    /// and at most [`MAX_TERMINAL_LINE_BYTES`]; an empty string is a blank
    /// line. At most `ReaderSettings::terminal_read_lines` of them.
    pub lines: Vec<String>,
}

impl TerminalOutput {
    /// Whether nothing at all changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changed.is_none() && self.skipped.is_none() && self.lines.is_empty()
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
}

/// How many lines went by unread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Skipped {
    /// Exactly this many.
    Count(u32),
    /// Some, but how many is not known: the scrollback overflowed past the
    /// anchor, or the screen changed under it, and the discarded lines are
    /// gone.
    Uncounted,
}

impl Skipped {
    /// The two counts together: exact only when both are.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        match (self, other) {
            (Self::Count(a), Self::Count(b)) => Self::Count(a.saturating_add(b)),
            _ => Self::Uncounted,
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
