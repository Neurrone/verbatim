//! A terminal's places (`books/dev/src/explanation/design/terminal-model.md`,
//! "The review cursor" and "Moving by command"; build 9's design, 5.6): the
//! end mark the next read counts the rows written from, the end mark kept
//! aside while a full-screen program's screen is up, the review position
//! and the command marks, each a place counted in rows above the
//! document's end. One record per terminal node, Core's, dropped with the
//! outpost generation that minted the node; the outpost only holds the
//! marks' ranges, as anchors Core names, and counts.
//!
//! Each read gives the document's rows and the rows from the one the mark
//! Core named sits after, and Core decides what they mean: while the
//! document grows, an exact count, the rows added since the mark was set
//! and the mark's own row when output filled or changed it since; once the
//! history is full and rows rotate under fixed places (the same row count
//! with another first row), the mark's line found again by its text, in
//! the window or by the outpost's search.

use std::collections::VecDeque;

use crate::terminal_window::{
    CurrentMark, EndMark, MarkPlace, MarkSearch, TerminalWindow, WindowAsk,
};
use serde::{Deserialize, Serialize};
use verbatim_text::terminal_window::Window;

use crate::terminal::pacing::{Skipped, Waiting};

/// At most this many command marks are kept, the oldest dropped first.
pub(crate) const MARKS: usize = 1_000;

/// A place in a terminal: rows above the document's last row, a column, and
/// its line's text, by which it is found again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RowPlace {
    /// Rows above the document's last row, 0 for the last.
    pub(crate) above: u32,
    /// Its column, in characters.
    pub(crate) column: u32,
    /// Its line's text, without padding.
    pub(crate) text: String,
}

/// The last read's end mark with what the next read compares to tell
/// rotation: the document's row count and its first row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct LastEnd {
    mark: EndMark,
    document_rows: u32,
    first_row: String,
}

/// A terminal's places.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalPlaces {
    end: Option<LastEnd>,
    /// The main screen's end mark while a full-screen program's screen is
    /// up (TM rules 7 and 8).
    aside: Option<EndMark>,
    /// The review position, once a review command or a mark placed it.
    pub(crate) review: Option<RowPlace>,
    /// The command marks, oldest first.
    pub(crate) marks: VecDeque<RowPlace>,
    /// Whether a read showed the history rotating: once full it stays full.
    rotated: bool,
    /// The screen's height as the last read found it.
    height: u32,
    /// The rows every place has moved up by so far, past the document for
    /// a count not known: a search's answer, counted from the end mark it
    /// named, is placed by what reads moved since it was sent.
    written: u64,
}

/// What a read the watch took says beside its diff, from its marks.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Placed {
    /// More was written than the window holds: the output in its place,
    /// in order, the diff's output not spoken.
    pub(crate) past: Option<Vec<Waiting>>,
    /// The rows written past the mark lie within the window, so a read
    /// sharing no line with the last is a new screen (F14).
    pub(crate) within: bool,
}

impl TerminalPlaces {
    /// The window read Core names: the current end mark, searched for when
    /// `held` (a read the pacing held) once the history rotated, the aside
    /// mark, the last height, "Lines spoken in full" as `opening`, and the
    /// colours when `colours`.
    pub(crate) fn ask(&self, held: bool, opening: u32, colours: bool) -> WindowAsk {
        WindowAsk {
            end: self.end.as_ref().map(|end| CurrentMark {
                mark: end.mark.clone(),
                find: held && self.rotated,
            }),
            aside: self.aside.clone(),
            height: self.height,
            opening,
            colours,
        }
    }

    /// The current end mark's anchor, which the rows of every place are
    /// counted above.
    pub(crate) fn end_anchor(&self) -> Option<verbatim_model::TextAnchor> {
        self.end.as_ref().map(|end| end.mark.anchor)
    }

    /// The rows every place has moved up by so far.
    pub(crate) fn written(&self) -> u64 {
        self.written
    }

    /// Every anchor the places hold.
    pub(crate) fn anchors(&self) -> impl Iterator<Item = verbatim_model::TextAnchor> + '_ {
        self.end
            .iter()
            .map(|end| end.mark.anchor)
            .chain(self.aside.iter().map(|aside| aside.anchor))
    }

    /// Takes the read `read`, parsed as `window`, whose diff kept a line of
    /// the last (`shares_line`) and said `said` lines of output, a read past
    /// more than `opening` ("Lines spoken in full") rows and its window
    /// speaking that many first: the count of rows written past its mark
    /// judged (the table of 5.6), every place moved by the rows added, the
    /// end mark replaced, and the aside mark set or used.
    pub(crate) fn took(
        &mut self,
        read: &TerminalWindow,
        window: &Window,
        (shares_line, said, opening): (bool, usize, u32),
    ) -> Placed {
        let first_row = read.first_row.text.clone();
        let rotated_now = self.end.as_ref().is_some_and(|end| {
            end.document_rows == read.document_rows && end.first_row != first_row
        });
        self.rotated |= rotated_now;
        let window_rows = read.above;
        let from_aside = self.aside.is_some() && !read.screen_at_start;
        let (place, mark) = if from_aside {
            (read.aside_place.as_ref(), self.aside.as_ref())
        } else {
            (
                read.end_place.as_ref(),
                self.end.as_ref().map(|end| &end.mark),
            )
        };
        let line = self
            .end
            .as_ref()
            .map(|end| (end.mark.line.as_str(), end.mark.line_above));
        let counted = match (place, mark) {
            (Some(MarkPlace::Rows { from }), Some(mark)) if !rotated_now || from_aside => {
                written_past(mark, from, read.document_rows)
            }
            (Some(MarkPlace::Rows { .. } | MarkPlace::Refused) | None, _) => None,
        };
        let (written, added, past) = match (place, counted) {
            (None, _) => (None, None, None),
            (Some(MarkPlace::Rows { .. }), Some(counted)) => {
                // Past more rows than the window and the opening group
                // together, the opening group is spoken first.
                let long = u64::from(counted.written)
                    > u64::from(window_rows).saturating_add(u64::from(opening));
                let opening: Vec<String> = if long {
                    counted
                        .rows
                        .into_iter()
                        .take(usize::try_from(opening).unwrap_or(usize::MAX))
                        .collect()
                } else {
                    Vec::new()
                };
                let skipped = counted
                    .written
                    .saturating_sub(window_rows)
                    .saturating_sub(u32::try_from(opening.len()).unwrap_or(u32::MAX));
                (
                    Some(counted.written),
                    Some(counted.added),
                    (counted.written > window_rows)
                        .then(|| past(window, Some(opening), Skipped::Count(skipped))),
                )
            }
            (Some(MarkPlace::Rows { .. }), None) if rotated_now && !from_aside => {
                let (written, past) = rotated(read, window, line, (window_rows, said));
                (written, written, past)
            }
            // Refused, or a document shorter than when the mark was set: the
            // text shrank under a clear.
            (Some(MarkPlace::Refused | MarkPlace::Rows { .. }), _) => {
                let written = read.document_rows;
                (
                    Some(written),
                    Some(written),
                    (written > window_rows).then(|| {
                        past(
                            window,
                            None,
                            Skipped::About(written.saturating_sub(window_rows)),
                        )
                    }),
                )
            }
        };
        let within = written.is_some_and(|written| written <= window_rows);
        if from_aside {
            self.aside = None;
        } else {
            let new_screen = within && !shares_line;
            if new_screen && read.screen_at_start && self.aside.is_none() {
                self.aside = self.end.as_ref().map(|end| end.mark.clone());
            } else if self.aside.is_none() {
                self.moved(added, read.document_rows);
            }
        }
        let (line, line_above) = window.last_text_line().unwrap_or_default();
        self.end = Some(LastEnd {
            mark: EndMark {
                anchor: read.end,
                line,
                line_above,
                rows: read.document_rows,
                row: window.last_row_text(),
            },
            document_rows: read.document_rows,
            first_row,
        });
        self.height = read.screen_rows;
        Placed { past, within }
    }

    /// Moves every place up by `written` rows, all of them past the
    /// document when the count is unknown, and drops those past its
    /// `document_rows`.
    fn moved(&mut self, written: Option<u32>, document_rows: u32) {
        let by = written.unwrap_or(u32::MAX);
        self.written = self.written.saturating_add(u64::from(by));
        let keep = |place: &mut RowPlace| {
            place.above = place.above.saturating_add(by);
            place.above < document_rows
        };
        self.marks.retain_mut(keep);
        if let Some(review) = &mut self.review
            && !keep(review)
        {
            self.review = None;
        }
    }

    /// Sets a command mark at `place`, unless one is there already.
    pub(crate) fn mark(&mut self, place: RowPlace) {
        if self.marks.iter().any(|mark| mark.above == place.above) {
            return;
        }
        if self.marks.len() >= MARKS {
            self.marks.pop_front();
        }
        self.marks.push_back(place);
    }
}

/// The rows a read found written past an end mark, while the history does
/// not rotate.
struct Counted {
    /// The rows added since the mark was set, by which every place moves.
    added: u32,
    /// The rows written past the mark: those added, and the mark's own row
    /// when output filled or changed it since.
    written: u32,
    /// The rows written past the mark from the first, as many as were read.
    rows: Vec<String>,
}

/// What the rows `from` the mark's row, read when the document held
/// `document_rows` rows, show of `mark`; `None` when the document is
/// shorter than when the mark was set. The mark's row, the last row of the
/// read that set it, is written past the mark when its text changed, or,
/// blank then and so not yet spoken or counted, when it has text now or
/// rows below it (TM 12).
fn written_past(
    mark: &EndMark,
    from: &verbatim_model::TerminalText,
    document_rows: u32,
) -> Option<Counted> {
    let added = document_rows.checked_sub(mark.rows)?;
    let mut rows = lines_of(&from.text);
    let own = rows.first().map_or("", String::as_str);
    let refilled = if mark.row.is_empty() {
        added > 0 || !own.is_empty()
    } else {
        own != mark.row
    };
    if !refilled && !rows.is_empty() {
        rows.remove(0);
    }
    Some(Counted {
        added,
        written: added.saturating_add(u32::from(refilled)),
        rows,
    })
}

/// The count of a read after the history rotated (5.6's rows with
/// rotation): the mark's line in the window, found by the search, or
/// missed; `None` for the count when it is unknown.
fn rotated(
    read: &TerminalWindow,
    window: &Window,
    line: Option<(&str, u32)>,
    (window_rows, said): (u32, usize),
) -> (Option<u32>, Option<Vec<Waiting>>) {
    let Some((text, line_above)) = line else {
        return (None, None);
    };
    if let Some(at) = window.line_place(text) {
        let written = at.saturating_sub(line_above);
        // A flood of identical lines over a full history: the line is at
        // the window's end and nothing was inserted.
        let uncounted = read.found != MarkSearch::NotAsked && said == 0 && at <= line_above;
        return (
            Some(written),
            uncounted.then(|| past(window, Some(Vec::new()), Skipped::Uncounted)),
        );
    }
    match read.found {
        MarkSearch::Found(distance) => {
            let written = distance.saturating_sub(line_above);
            (
                Some(written),
                (written > window_rows).then(|| {
                    past(
                        window,
                        None,
                        Skipped::Count(written.saturating_sub(window_rows)),
                    )
                }),
            )
        }
        MarkSearch::Missed => {
            let lines = u32::try_from(window.texts().count()).unwrap_or(u32::MAX);
            (
                None,
                Some(past(
                    window,
                    None,
                    Skipped::MoreThan(read.document_rows.saturating_sub(lines)),
                )),
            )
        }
        // Not asked, on a read the pacing did not hold: less than a window
        // was written, so the diff speaks it.
        MarkSearch::NotAsked | MarkSearch::InWindow => (None, None),
    }
}

/// What a read says when more was written than its window holds: the
/// opening lines after the mark, the count of those skipped, then every
/// line of the window but the blank ones at its end, which count only once
/// a line with text is below them (TM 12), as the next read finds.
fn past(window: &Window, opening: Option<Vec<String>>, skipped: Skipped) -> Vec<Waiting> {
    let mut output: Vec<Waiting> = opening
        .unwrap_or_default()
        .into_iter()
        .map(Waiting::Line)
        .collect();
    if skipped != Skipped::Count(0) {
        output.push(Waiting::Skipped(skipped));
    }
    let texts: Vec<&str> = window.texts().collect();
    let end = texts
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(0, |last| last.saturating_add(1));
    output.extend(
        texts
            .iter()
            .take(end)
            .map(|line| Waiting::Line((*line).to_owned())),
    );
    output
}

/// The lines of `text`, as a terminal's text parts them.
fn lines_of(text: &str) -> Vec<String> {
    text.strip_suffix("\r\n")
        .unwrap_or(text)
        .split("\r\n")
        .map(str::to_owned)
        .collect()
}
