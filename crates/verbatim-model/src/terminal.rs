//! What a terminal says (`docs/design/terminal-model.md`, "Reading rules"): the
//! outpost reads a focused terminal's text, aligns each read with its memory
//! of the terminal's rows, and applies the reading rules
//! (`verbatim_text::terminal_reading`); what they say is sent to Core as
//! [`NormalizedEvent::TerminalSpeech`](crate::NormalizedEvent::TerminalSpeech),
//! in the order it is spoken.
//!
//! Output (new lines, what a growing line gained, a redrawn row's change, a
//! held line spoken as it was last read) goes through Core's flood policy,
//! one utterance each, and a summary counts each but a line's growth as a
//! line; a read's own flood summary arrives as
//! [`TerminalItem::Skipped`]. The answers to keys (typing shown, the key's
//! unit at the caret, text the caret's line gained or lost, a row away from
//! the caret that is a key's answer, a suggestion) are spoken at once, ahead
//! of any output. Text is sent without padding, and a line whole, however
//! long (Dickson, 2026-10-07); Core bounds what it keeps waiting.
//!
//! The outpost is told of every key that reaches a focused terminal
//! ([`TerminalKey`]), with when it reached the program, since the rules
//! judge which read a key's effects can be in by that time ("Keys and their window"):
//! by the keyboard hook's thread for a key it passes itself, by the reducer
//! thread for a key deferred behind a held one as it sends it on, and by
//! Core, in [`crate::TextOp::TerminalKey`], for a key it held.

use serde::{Deserialize, Serialize};

/// What one read, key or quiet period of a terminal said, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSpeech {
    /// What to say, in the order it is spoken.
    pub items: Vec<TerminalItem>,
    /// Whether the review cursor followed the caret: a key moved the caret
    /// within its window with "review follows caret" on ("The review anchor and following"), so
    /// the next review command reads at the caret.
    #[serde(default)]
    pub followed: bool,
}

impl TerminalSpeech {
    /// Whether there is nothing to say and the review cursor did not
    /// follow the caret.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && !self.followed
    }
}

/// The unit a key's answer at the caret is spoken in ("Keys and their window").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalUnit {
    /// The character at the caret.
    Character,
    /// The word at the caret.
    Word,
    /// The caret's line.
    Line,
}

/// One thing a terminal says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalItem {
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
    pub fn is_output(&self) -> bool {
        matches!(self, Self::Output(_) | Self::Growth(_) | Self::Skipped(_))
    }

    /// The text it carries, for bounding a message's size; none for a
    /// count.
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Output(text)
            | Self::Growth(text)
            | Self::Echo(text)
            | Self::Caret { text, .. }
            | Self::Gained(text)
            | Self::Lost(text)
            | Self::Answer(text)
            | Self::Suggestion(text) => text,
            Self::Skipped(_) => "",
        }
    }
}

/// A key that reached a focused terminal ("Keys and their window"), as the reading
/// rules take it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalKey {
    /// What it is.
    pub kind: TerminalKeyKind,
    /// When it reached the program, in milliseconds on the run's shared
    /// clock, the clock reads are stamped with; a key held back reaches it
    /// when it is passed on.
    pub at_ms: u64,
}

/// What a key is, as the reading rules care.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalKeyKind {
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

/// The reading rules' settings, which Core sends each outpost when it
/// starts and whenever they change
/// (`SupervisorToOutpost::TerminalSettings`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSettings {
    /// "Report new output".
    pub report_new_output: bool,
    /// "Review follows caret".
    pub review_follows_caret: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            report_new_output: true,
            review_follows_caret: true,
        }
    }
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
    pub fn plus(self, newer: Self) -> Self {
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
        assert!(TerminalSpeech::default().is_empty());
    }
}
