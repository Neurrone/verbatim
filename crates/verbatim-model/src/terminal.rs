//! A terminal's reads and what its reading rules say
//! (`docs/design/terminal-model.md`, "Where it runs"): the outpost reads a
//! focused terminal and sends each read to Core as a [`TerminalSnapshot`];
//! Core's terminal model aligns it and applies the reading rules, whose
//! speech its flood policy takes as [`TerminalItem`]s.
//!
//! Output (new lines, what a growing line gained, a redrawn row's change, a
//! held line spoken as it was last read) goes through Core's flood policy,
//! one utterance each, and a summary counts each but a line's growth as a
//! line; a read's own flood summary is [`TerminalItem::Skipped`]. The
//! answers to keys (typing shown, the key's unit at the caret, text the
//! caret's line gained or lost, a row away from the caret that is a key's
//! answer, a suggestion) are spoken at once, ahead of any output.
//!
//! The keyboard hook's thread still tells the focused terminal's outpost of
//! each key it passes ([`TerminalKey`]), which the outpost no longer takes.

use serde::{Deserialize, Serialize};

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

/// One read of a focused terminal as the outpost sends it to Core
/// (`docs/design/terminal-model.md`, "Where it runs", and "The read"): the
/// wire form of the reading rules' read, which Core rebuilds exactly. Each
/// snapshot is whole by itself and names nothing of an earlier read. Every
/// text is sent without the spaces that pad its lines, with each line's
/// padded length ([`TerminalText`]), since the rules measure the screen's
/// width from the padding. The screen is sent beside the text, not as a
/// place in it: it is the provider's first visible range, read in a call of
/// its own, so it is where the user scrolled the view, and the terminal may
/// write between that call and the text's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent answers of one read, as the read program gives them"
)]
pub struct TerminalSnapshot {
    /// The text from the read's start to the document's end.
    pub text: TerminalText,
    /// The screen's text, for its width and height.
    pub screen: TerminalText,
    /// Whether the read started where the last read said to start.
    pub from_start: bool,
    /// Whether that start was refused: the text shrank, or the screen
    /// switched.
    pub shrank: bool,
    /// Whether the read started at the document's start.
    pub at_document_start: bool,
    /// Whether the screen starts at the document's start.
    pub alternate: bool,
    /// The document's first row.
    pub first_row: TerminalText,
    /// How many rows the caret's row and those below it hold, to the
    /// document's end; 0 with no caret.
    pub caret_rows_to_end: u32,
    /// The caret's row.
    pub caret_line: TerminalText,
    /// The caret's column in its row, in characters.
    pub caret_column: u32,
    /// How many rows the read holds above the start it was asked for.
    pub above: u32,
    /// When the read began, in milliseconds on the clock every process of
    /// a run shares.
    pub started_ms: u64,
    /// Whether the read waited for Core's speech queue ("Volatile rows").
    pub waited: bool,
    /// The screen's colour runs, row by row from the screen's top to the
    /// document's end, when the read took them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colours: Option<Vec<Vec<TerminalColourRun>>>,
    /// The caret the read found, as a caret report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caret: Option<crate::CaretReport>,
}

/// A terminal's text without the spaces that pad its lines, as a
/// [`TerminalSnapshot`] carries it: each line's padding left out, and how
/// long each line was with it, so the text is rebuilt exactly
/// ([`TerminalText::padded`]). A screen of 120 columns whose lines hold a
/// few words each sends its words and one run of lengths.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalText {
    /// The text, lines parted by a carriage return and line feed, each
    /// without the spaces at its end.
    pub text: String,
    /// Each line's length with its padding, in characters, as runs of
    /// lines of one length: how many lines, and their length.
    pub lengths: Vec<(u32, u32)>,
}

impl TerminalText {
    /// `padded` without the spaces at the end of each of its lines.
    #[must_use]
    pub fn unpadded(padded: &str) -> Self {
        let mut text = String::with_capacity(padded.len() / 2);
        let mut lengths: Vec<(u32, u32)> = Vec::new();
        for (index, line) in padded.split("\r\n").enumerate() {
            if index > 0 {
                text.push_str("\r\n");
            }
            let length = u32::try_from(characters(line)).unwrap_or(u32::MAX);
            text.push_str(line.trim_end_matches(' '));
            match lengths.last_mut() {
                Some((count, last)) if *last == length => *count += 1,
                _ => lengths.push((1, length)),
            }
        }
        Self { text, lengths }
    }

    /// The text with each line padded again to its length.
    #[must_use]
    pub fn padded(&self) -> String {
        let width = |length: u32| usize::try_from(length).unwrap_or(usize::MAX);
        let total: usize = self
            .lengths
            .iter()
            .map(|(count, length)| width(*count).saturating_mul(width(*length) + 2))
            .sum();
        let mut padded = String::with_capacity(total.max(self.text.len()));
        let mut lengths = self
            .lengths
            .iter()
            .flat_map(|(count, length)| std::iter::repeat_n(width(*length), width(*count)));
        for (index, line) in self.text.split("\r\n").enumerate() {
            if index > 0 {
                padded.push_str("\r\n");
            }
            padded.push_str(line);
            let length = lengths.next().unwrap_or(0);
            let mut spaces = length.saturating_sub(characters(line));
            while spaces > 0 {
                let run = spaces.min(SPACES.len());
                padded.push_str(&SPACES[..run]);
                spaces -= run;
            }
        }
        padded
    }
}

/// Spaces to pad a line with, copied a run at a time.
const SPACES: &str = "                                                                                                                                ";

/// How many characters `line` holds: its bytes when it is ASCII.
fn characters(line: &str) -> usize {
    if line.is_ascii() {
        line.len()
    } else {
        line.chars().count()
    }
}

/// A stretch of one row's cells in one foreground and background, as the
/// read program's colours give it ("Colours").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalColourRun {
    /// Its first column, in cells.
    pub column: u32,
    /// Its length, in cells.
    pub length: u32,
    /// Its foreground, `0x00BBGGRR`.
    pub foreground: u32,
    /// Its background, `0x00BBGGRR`.
    pub background: u32,
}

/// Which read Core asks a terminal's outpost for at once
/// ([`crate::TextOp::TerminalReadNow`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalScope {
    /// A read as any other, whatever the on-demand state, before a review
    /// command, which Core answers from its memory once the read is
    /// aligned.
    Fresh,
    /// The whole document, from its start, which the reading rules ask
    /// for after a lost read ("A lost read").
    Whole,
    /// A first read again, counted up from the document's end by `height`
    /// rows with as many rows above, so that it holds its history block
    /// ("The read").
    HistoryBlock {
        /// The screen's height the first read found.
        height: u32,
    },
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
    fn a_terminals_text_without_its_padding_is_padded_again_exactly() {
        // Lines of one padded length make one run; a line of other
        // characters is counted in characters; a tab before the padding
        // and a break at the end are kept.
        let padded = "ab   \r\ncd   \r\nпри  \r\nx\t   \r\n";
        let unpadded = TerminalText::unpadded(padded);
        assert_eq!(
            unpadded,
            TerminalText {
                text: "ab\r\ncd\r\nпри\r\nx\t\r\n".to_owned(),
                lengths: vec![(4, 5), (1, 0)],
            }
        );
        assert_eq!(unpadded.padded(), padded);
    }

    #[test]
    fn output_is_queued_and_a_keys_answer_is_not() {
        assert!(TerminalItem::Output("a".to_owned()).is_output());
        assert!(TerminalItem::Skipped(Skipped::Count(2)).is_output());
        assert!(!TerminalItem::Echo("a".to_owned()).is_output());
    }
}
