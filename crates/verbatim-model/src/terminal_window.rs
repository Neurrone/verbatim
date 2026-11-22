//! A terminal's window read, the request list's shared operation
//! [`crate::batch::Op::TerminalRead`]: what Core names to it
//! ([`WindowAsk`]) and what it answers ([`TerminalWindow`]). The outpost
//! reads and Core decides: every count here is a fact the outpost read,
//! and what it means for speech is Core's.

use serde::{Deserialize, Serialize};

use crate::{CaretReport, TerminalColourRun, TerminalText, TextAnchor};

/// An end mark Core names: a held range at an earlier read's document
/// end, with the text of the last line with text at or above it and that
/// line's distance in rows above the mark, by which a read finds the line
/// again once the history rotated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndMark {
    /// The held range at the earlier read's document end.
    pub anchor: TextAnchor,
    /// The last line with text at or above the mark, without its padding.
    pub line: String,
    /// How many rows that line lies above the mark.
    pub line_above: u32,
}

/// The current end mark, and whether the read searches for its line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentMark {
    /// The mark.
    pub mark: EndMark,
    /// Whether the read searches for the mark's line when the window it
    /// read lacks it ([`MarkSearch`]): Core asks only on a read its pacing
    /// held once an earlier read showed the history rotated.
    pub find: bool,
}

/// What Core names to a window read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowAsk {
    /// The current end mark, none for a first read.
    pub end: Option<CurrentMark>,
    /// The end mark kept aside while a full-screen program's screen is up.
    pub aside: Option<EndMark>,
    /// The screen's height in rows as the last read found it, 0 at first:
    /// the window starts twice that many rows above the document's end,
    /// or at the screen's top when it is 0.
    pub height: u32,
    /// "Lines spoken in full": past a mark with more rows written after it
    /// than this and the window together, the read gives this many rows
    /// after the mark ([`MarkPlace::Rows::opening`]).
    pub opening: u32,
    /// Whether to read the colours of every screen row, as Core does for
    /// each read in a key's window.
    pub colours: bool,
}

/// What a mark showed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkPlace {
    /// The mark lies past the document's end or the provider would not
    /// compare it: the text shrank under a clear, or the mark is on the
    /// other screen.
    Refused,
    /// The rows written past the mark.
    Rows {
        /// How many rows lie from the mark to the document's end.
        written: u32,
        /// The first "Lines spoken in full" rows after the mark, when more
        /// rows follow it than those and the window together.
        opening: Option<TerminalText>,
    },
}

/// The search for the current mark's line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkSearch {
    /// Core did not ask for it.
    NotAsked,
    /// The window read holds the line, so nothing was searched.
    InWindow,
    /// The nearest row starting with the line's text, this many rows above
    /// the document's end.
    Found(u32),
    /// No row starting with it was found.
    Missed,
}

/// The caret a window read found, as a caret report, with the point held
/// at it: a caret key's starting point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowCaret {
    /// The caret and its line.
    pub report: CaretReport,
    /// The point held at the caret.
    pub point: TextAnchor,
}

/// What a window read answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWindow {
    /// The rows from two screens above the document's end to its end, read
    /// in one call, without their padding.
    pub rows: TerminalText,
    /// How many rows the window starts above the document's end.
    pub above: u32,
    /// How many rows the screen's top lies above the document's end; the
    /// screen's last rows may lie past the end, blank.
    pub screen_top: u32,
    /// The screen's height in rows.
    pub screen_rows: u32,
    /// The screen's width in cells: a line longer than it wraps over
    /// several rows, which `above`, `screen_top` and the marks count.
    pub width: u32,
    /// Whether the screen starts at the document's start.
    pub screen_at_start: bool,
    /// The caret, when the provider reports one.
    pub caret: Option<WindowCaret>,
    /// How many rows the caret's row and those below it hold, to the
    /// document's end, when the provider reports a caret: a blank caret
    /// row is told from the blank rows around it only by its place.
    pub caret_rows_to_end: Option<u32>,
    /// The new end mark, held at the document's end.
    pub end: TextAnchor,
    /// The document's first row, without its padding.
    pub first_row: TerminalText,
    /// How many rows lie from the document's end to its start.
    pub document_rows: u32,
    /// What the current mark showed, when Core named one.
    pub end_place: Option<MarkPlace>,
    /// What the aside mark showed, when Core named one.
    pub aside_place: Option<MarkPlace>,
    /// The search for the current mark's line.
    pub found: MarkSearch,
    /// When Core asked for colours, each screen row's runs from the
    /// screen's top, a wrapped line's rows each its own; the rows stop at
    /// the document's end.
    pub colours: Option<Vec<Vec<TerminalColourRun>>>,
    /// When the read began, in microseconds on the run's shared clock.
    pub began_us: u64,
    /// When the read's calls ended, before its text was converted.
    pub ended_us: u64,
}

impl TerminalWindow {
    /// Calls `anchors` on every text anchor the read carries: the caret
    /// report's line start and selection ends, the caret's point and the
    /// end mark.
    pub fn visit(&mut self, anchors: &mut dyn FnMut(&mut TextAnchor)) {
        if let Some(caret) = &mut self.caret {
            anchors(&mut caret.report.line.start);
            if let Some(selection) = &mut caret.report.selection {
                anchors(&mut selection.range.start.anchor);
                anchors(&mut selection.range.end.anchor);
            }
            anchors(&mut caret.point);
        }
        anchors(&mut self.end);
    }
}
