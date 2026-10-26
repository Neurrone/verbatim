//! A terminal's reads (`docs/design/terminal-model.md`, "Where it runs"):
//! the outpost reads a focused terminal and sends each read to Core as a
//! [`TerminalSnapshot`], and Core's terminal model aligns it and applies
//! the reading rules; Core asks for the reads it needs
//! ([`TerminalScope`]).

use serde::{Deserialize, Serialize};

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
    /// a run shares: output it shows was written before it ended, and
    /// output read before speech was cut off, which began before the cut,
    /// is not spoken.
    pub started_ms: u64,
    /// When the read ended, on the same clock: the terminal's text and
    /// caret are as of a moment between its start and its end, so every
    /// key whose effect it can show reached the program by then, and a key
    /// is given to the reading rules before the read when it reached the
    /// program by its end; 0 when not known, the start standing in.
    #[serde(default)]
    pub ended_ms: u64,
    /// Whether the read waited for Core's speech queue ("Volatile rows").
    pub waited: bool,
    /// The read's number among the terminal's reads, counted by its
    /// outpost from 1, by which a search's or a history read's answer
    /// names the read its rows are counted from
    /// ([`crate::TextReply::TerminalFound`],
    /// [`crate::TextReply::TerminalHistory`]); 0 when not numbered.
    #[serde(default)]
    pub read: u64,
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

/// A row a terminal's search found ([`crate::TextReply::TerminalFound`]):
/// where it is, in rows above where the next read starts (negative below
/// it), and the text read from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalMatch {
    /// Rows above the start.
    pub above: i32,
    /// The text of the rows read from it, padding and line breaks as the
    /// terminal gave them.
    pub text: String,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
