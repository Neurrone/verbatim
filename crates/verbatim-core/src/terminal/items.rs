//! What a terminal's reading rules say, as Core's flood policy takes it
//! (`docs/design/terminal-model.md`, "Core's flood policy"), and what a key
//! is to the rules.

use serde::{Deserialize, Serialize};

/// The unit a key's answer at the caret is spoken in ("Keys and their window").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TerminalUnit {
    /// The character at the caret.
    Character,
    /// The word at the caret.
    Word,
    /// The caret's line.
    Line,
}

/// One thing a terminal says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TerminalItem {
    /// Output: a new line whole, a redrawn row from the word that changed,
    /// or a held line as it was last read (`docs/design/terminal-model.md`,
    /// "New text" and "Changed rows"). Spoken by the flood policy, one
    /// utterance each; an empty one is a blank line, counted but not
    /// spoken.
    Output(String),
    /// What a growing line gained ("Growing lines"): output, spoken by the
    /// flood policy as one utterance like a line, but part of the line it
    /// grew, with which it counts and is dropped as one ("Core's flood
    /// policy").
    Growth(String),
    /// Lines of output that went by unread: after an overflow,
    /// [`Skipped::MoreThan`] of none, before the lines the history holds,
    /// each of which adds to it when the flood policy skips it.
    Skipped(Skipped),
    /// Typing the terminal showed at the caret, echoed by the typing echo
    /// settings ("Keys and their window").
    Echo(String),
    /// A key's answer at the caret: the key's unit for a caret key, or the
    /// landing rule's for any other ("The caret's line"); empty text past the
    /// line's end.
    Caret {
        /// The unit.
        unit: TerminalUnit,
        /// The text at the caret in that unit.
        text: String,
    },
    /// Text the caret's line gained that the user did not type ("Keys and their window").
    Gained(String),
    /// Text the caret's line lost next to the caret ("Keys and their window").
    Lost(String),
    /// A row away from the caret that gained text or emphasis within a
    /// key's window: the key's answer, whole ("A key's answer").
    Answer(String),
    /// A suggestion after the caret, in other colours than the typing
    /// ("A key's answer").
    Suggestion(String),
}

impl TerminalItem {
    /// Whether it is output, which the flood policy queues, rather than a
    /// key's answer, which is spoken at once.
    #[must_use]
    pub(crate) fn is_output(&self) -> bool {
        matches!(self, Self::Output(_) | Self::Growth(_) | Self::Skipped(_))
    }
}

/// What a key is, as the reading rules care.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TerminalKeyKind {
    /// Characters typed.
    Typed(String),
    /// A caret key Verbatim knows, with the unit it moves by.
    Caret(TerminalUnit),
    /// Enter: it forgets typing not yet shown, and its answer is only new
    /// output, never the caret's line ("The caret's line").
    Enter,
    /// A key that forgets typing not yet shown: Tab, Escape, Control+C,
    /// Control+D and Control+Break.
    Forget,
    /// Delete, answered by what it leaves at the caret.
    Delete,
    /// Any other key, answered by what it did.
    Other,
}

/// How many lines went by unread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Skipped {
    /// Exactly this many.
    Count(u32),
    /// Some, but how many is not known.
    Uncounted,
    /// More than this many: the terminal's history overflowed past the
    /// screen last read, so its oldest lines are gone, and this many are
    /// the lines the history holds that are not spoken.
    MoreThan(u32),
    /// About this many: the reading lost track of the terminal and counted
    /// these lines from text, which repeated text can put a little off.
    About(u32),
}

impl Skipped {
    /// This count followed by the `newer` one. Counts add, exact only when
    /// both are. A newer [`Skipped::MoreThan`] stands for everything before
    /// it: what went before it has left the history, and its count is the
    /// history's lines left unspoken. Lines counted after one add to it.
    #[must_use]
    pub(crate) fn plus(self, newer: Self) -> Self {
        match (self, newer) {
            (Self::Count(a), Self::Count(b)) => Self::Count(a.saturating_add(b)),
            (_, Self::MoreThan(b)) => Self::MoreThan(b),
            (Self::MoreThan(a), Self::Count(b) | Self::About(b)) => {
                Self::MoreThan(a.saturating_add(b))
            }
            (Self::About(a) | Self::Count(a), Self::About(b))
            | (Self::About(a), Self::Count(b)) => Self::About(a.saturating_add(b)),
            (_, Self::Uncounted) | (Self::Uncounted, Self::Count(_) | Self::About(_)) => {
                Self::Uncounted
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_counts_add_and_stay_exact_only_when_both_are() {
        assert_eq!(Skipped::Count(3).plus(Skipped::Count(4)), Skipped::Count(7));
        assert_eq!(Skipped::About(0).plus(Skipped::Count(4)), Skipped::About(4));
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
    fn output_is_queued_and_a_keys_answer_is_not() {
        assert!(TerminalItem::Output("a".to_owned()).is_output());
        assert!(TerminalItem::Skipped(Skipped::Count(2)).is_output());
        assert!(!TerminalItem::Echo("a".to_owned()).is_output());
    }
}
