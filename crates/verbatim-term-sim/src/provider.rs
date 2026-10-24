//! The simulated terminal's answers to the questions Verbatim's read
//! program asks of a terminal's UI Automation provider: text ranges as
//! fixed positions in the active buffer, which read newer text once the
//! history rotates and fail when they lie beyond the active buffer; the
//! document ending at the end of the lower of the last row with text and
//! the cursor's; rows joined by line breaks, none after the last, and none
//! after a row whose line wraps onto the next (`docs/design/terminal-model.md`,
//! sections 2.1 and 3.3). The conformance expectations check every answer
//! against the real terminals.

use crate::grid::Grid;
use crate::oracle::Seen;
use crate::{Colour, Run, Snapshot};

/// A position: a row's coordinate and a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    /// The row's coordinate in the buffer.
    pub row: usize,
    /// The column, in cells.
    pub column: usize,
}

/// A text range: a start and an end position, the end exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Range {
    /// Where it starts.
    pub start: Position,
    /// Where it ends.
    pub end: Position,
}

impl Range {
    /// An empty range at `at`.
    #[must_use]
    pub fn at(at: Position) -> Self {
        Self { start: at, end: at }
    }

    /// This range collapsed to its start.
    #[must_use]
    pub fn collapsed(self) -> Self {
        Self::at(self.start)
    }
}

/// A call the provider refused, as `E_FAIL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

/// The two terminals, whose providers the conformance expectations tell
/// apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Flavor {
    /// The console host, `conhost.exe`.
    ConsoleHost,
    /// Windows Terminal.
    WindowsTerminal,
}

/// What the read program asks: where to start, the screen's height as
/// last found, whether to read the whole text, and how many rows to read
/// above the start.
#[derive(Clone, Copy, Debug, Default)]
pub struct Query {
    /// Where the last read said to start; `None` for a first read.
    pub start: Option<Range>,
    /// The screen's height in rows as the last read found it; 0 when not
    /// known.
    pub screen_rows: u32,
    /// Whether to read the whole text from the document's start.
    pub whole: bool,
    /// How many rows above the start to begin the read, as far as the
    /// document's start allows; ignored when reading the whole text.
    pub above: u32,
}

/// More lines than any terminal holds.
const FAR: isize = 1_000_000;

/// The provider's view of a grid.
pub(crate) struct Provider<'a> {
    pub(crate) grid: &'a Grid,
    /// Whether a row's range ends after its line break, as Windows
    /// Terminal's does, so an expanded line, the screen and the document
    /// each end with one; the console host's end before it (measured
    /// 2026-10-23).
    pub(crate) row_includes_break: bool,
    /// Whether a row marked as wrapping ends the document no higher than
    /// itself even with no text, as the console host's does: a whole row
    /// of spaces written without a line feed, or such a row erased, which
    /// keeps the mark (measured 2026-10-23).
    pub(crate) wrapping_rows_count: bool,
}

impl Provider<'_> {
    /// How many rows the active buffer holds: both terminals allocate the
    /// whole buffer at once, so a position on any row of it is accepted,
    /// unwritten rows reading as blank, and one past it is refused.
    fn rows(&self) -> usize {
        self.grid.buffer().capacity
    }

    fn check(&self, at: Position) -> Result<(), Refused> {
        if at.row < self.rows() && at.column <= self.grid.width + 1 {
            Ok(())
        } else {
            Err(Refused)
        }
    }

    /// The document's last row: the lower of the last row with text and the
    /// cursor's, as both providers find them when asked, so rows moved down
    /// by an insertion stay in it and a row erased leaves it, unless it
    /// still wraps in the console host (measured 2026-10-23).
    fn last_row(&self) -> usize {
        let cursor = self.grid.cursor_coordinate();
        self.grid
            .buffer()
            .rows
            .iter()
            .rposition(|row| row.has_text() || (self.wrapping_rows_count && row.wraps))
            .map_or(cursor, |text| text.max(cursor))
    }

    /// The end of row `row`: after its line break when the provider's rows
    /// include it, which a column one past the width stands for. Windows
    /// Terminal's last row of the buffer has no row after it to hold the
    /// break, and ends at the width (measured 2026-10-23).
    fn row_end(&self, row: usize) -> Position {
        let column = if self.row_includes_break && row + 1 < self.rows() {
            self.grid.width + 1
        } else {
            self.grid.width
        };
        Position { row, column }
    }

    /// How many cells of row `row` its text gives: all of them, but for
    /// Windows Terminal's last row of the buffer, whose text leaves out its
    /// last cell though its range ends after it: the alternate screen's last
    /// row, and the main screen's once the history is full (measured
    /// 2026-10-23).
    fn cells_given(&self, row: usize) -> usize {
        if self.row_includes_break && row + 1 == self.rows() {
            self.grid.width - 1
        } else {
            self.grid.width
        }
    }

    /// The whole document.
    pub(crate) fn document(&self) -> Range {
        Range {
            start: Position { row: 0, column: 0 },
            end: self.row_end(self.last_row()),
        }
    }

    /// The screen.
    pub(crate) fn visible(&self) -> Range {
        let top = self.grid.buffer().top;
        Range {
            start: Position {
                row: top,
                column: 0,
            },
            end: self.row_end(top + self.grid.height - 1),
        }
    }

    /// The caret, an empty range at the cursor.
    pub(crate) fn caret(&self) -> Range {
        Range::at(Position {
            row: self.grid.cursor_coordinate(),
            column: self.grid.cursor.column,
        })
    }

    /// The text of `range`: each row padded to the width, a line break
    /// between a row and the next unless its line wraps onto it.
    pub(crate) fn text(&self, range: Range) -> Result<String, Refused> {
        self.check(range.start)?;
        self.check(range.end)?;
        let rows = &self.grid.buffer().rows;
        let blank = crate::grid::Cell::default();
        let cell = |row: usize, column: usize| {
            rows.get(row)
                .map_or(blank.character, |cells| cells.cells[column].character)
        };
        let wraps = |row: usize| rows.get(row).is_some_and(|row| row.wraps);
        let mut text = String::new();
        let mut row = range.start.row;
        let mut column = range.start.column;
        while row <= range.end.row {
            let last = if row == range.end.row {
                range.end.column.min(self.cells_given(row))
            } else {
                self.cells_given(row)
            };
            text.extend((column..last).map(|column| cell(row, column)));
            if row == range.end.row {
                if range.end.column > self.grid.width && !wraps(row) {
                    text.push_str("\r\n");
                }
                break;
            }
            if !wraps(row) {
                text.push_str("\r\n");
            }
            row += 1;
            column = 0;
        }
        Ok(text)
    }

    /// Compares two endpoints, as `CompareEndpoints` does.
    pub(crate) fn compare(&self, left: Position, right: Position) -> Result<i32, Refused> {
        self.check(left)?;
        self.check(right)?;
        Ok(match left.cmp(&right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        })
    }

    /// An empty range at `at` moved by `count` lines, with how many it
    /// moved: the stops are each row's start and the document's end, so
    /// the last step forward goes from the last row's start to its end.
    pub(crate) fn move_rows(
        &self,
        at: Position,
        count: isize,
    ) -> Result<(Position, isize), Refused> {
        self.check(at)?;
        let last = self.last_row();
        let end = self.row_end(last);
        let start_of = |row: usize| Position { row, column: 0 };
        let mut position = at.min(end);
        let mut moved = 0;
        if count > 0 {
            while moved < count && position < end {
                position = if position.row < last {
                    start_of(position.row + 1)
                } else {
                    end
                };
                moved += 1;
            }
        } else {
            while moved > count && position > start_of(0) {
                position = if position.column > 0 {
                    start_of(position.row)
                } else {
                    start_of(position.row - 1)
                };
                moved -= 1;
            }
        }
        Ok((position, moved))
    }

    /// The row holding `at`, without its line break; a position past the
    /// document's last row gives that row (measured in the console host,
    /// 2026-10-23).
    pub(crate) fn row_of(&self, at: Position) -> Result<Range, Refused> {
        self.check(at)?;
        let row = at.row.min(self.last_row());
        Ok(Range {
            start: Position { row, column: 0 },
            end: self.row_end(row),
        })
    }

    /// The read program's snapshot, step for step as its classic
    /// implementation takes it, with the coordinate of the row its text
    /// starts on.
    pub(crate) fn snapshot(&self, query: &Query) -> Result<(Snapshot, usize), Refused> {
        let document = self.document();
        let screen = self.visible();
        let screen_text = self.text(screen)?;
        let alternate = self.compare(document.start, screen.start)? == 0;
        let next = if query.screen_rows > 0 {
            let rows = isize::try_from(query.screen_rows).unwrap_or(isize::MAX);
            Range::at(self.move_rows(document.end, -rows)?.0)
        } else {
            screen.collapsed()
        };
        let mut start = if query.whole {
            document.collapsed()
        } else {
            screen.collapsed()
        };
        if query.screen_rows > 0 && !query.whole {
            start = next;
        }
        let (mut from_start, mut shrank) = (false, false);
        if let Some(kept) = query.start.filter(|_| !query.whole) {
            match self.compare(kept.start, next.start) {
                Ok(order) if order <= 0 => {
                    start = kept.collapsed();
                    from_start = true;
                }
                Ok(_) | Err(Refused) => shrank = true,
            }
        }
        let mut above = 0;
        if query.above > 0 && !query.whole {
            let rows = isize::try_from(query.above).unwrap_or(isize::MAX);
            let (moved_to, moved) = self.move_rows(start.start, -rows)?;
            start = Range::at(moved_to);
            above = u32::try_from(-moved).unwrap_or(0);
        }
        let text = self.text(Range {
            start: start.start,
            end: document.end,
        })?;
        let at_document_start = self.compare(start.start, document.start)? == 0;
        let first_row = self.text(self.row_of(document.start)?)?;
        let caret = self.caret();
        let caret_rows_to_end = u32::try_from(self.move_rows(caret.start, FAR)?.1).unwrap_or(0);
        let caret_row = self.row_of(caret.start)?;
        let caret_line = self.text(caret_row)?;
        let caret_column = if caret_row.start.row == caret.start.row {
            caret.start.column
        } else {
            0
        };
        let snapshot = Snapshot {
            text,
            screen: screen_text,
            from_start,
            shrank,
            at_document_start,
            alternate,
            first_row,
            caret_rows_to_end,
            caret_line,
            caret_column,
            next_start: next,
            above,
        };
        Ok((snapshot, start.start.row))
    }

    /// The rows from coordinate `from` to the document's last row, as a
    /// read's text gives them, with their identities.
    pub(crate) fn seen(&self, from: usize) -> Vec<Seen> {
        let rows = &self.grid.buffer().rows;
        (from..=self.last_row())
            .map(|at| Seen {
                id: rows[at].id,
                text: rows[at].cells[..self.cells_given(at)]
                    .iter()
                    .map(|cell| cell.character)
                    .collect(),
                wraps: rows[at].wraps,
            })
            .collect()
    }

    /// A cell's foreground and background as shown. The buffer's last cell
    /// reads in the style the next character would be written in, whatever
    /// it holds, in both terminals: the alternate screen's last cell, and
    /// the main screen's once the history is full (measured 2026-10-23).
    fn cell_colours(&self, row: usize, column: usize) -> (Colour, Colour) {
        let style = if row + 1 == self.rows() && column + 1 == self.grid.width {
            self.grid.style
        } else {
            self.grid
                .buffer()
                .rows
                .get(row)
                .and_then(|cells| cells.cells.get(column))
                .map_or(self.grid.style, |cell| cell.style)
        };
        let (foreground, background) = style.shown();
        if self.grid.screen_reversed {
            (background, foreground)
        } else {
            (foreground, background)
        }
    }

    /// The colours of the screen's rows to the document's last row: for
    /// each row, its stretches of one foreground and background as shown,
    /// cell by cell, as the conformance runner reads them.
    pub(crate) fn colours(&self) -> Vec<Vec<Run>> {
        let buffer = self.grid.buffer();
        (buffer.top..=self.last_row().min(buffer.top + self.grid.height - 1))
            .map(|row| {
                let mut runs = Vec::new();
                for column in 0..self.grid.width {
                    push(&mut runs, column, 1, self.cell_colours(row, column));
                }
                runs
            })
            .collect()
    }

    /// A stretch's colours as the providers answer a colour attribute over
    /// it: its first cell's when every cell but its last has them, and
    /// "mixed", `None`, otherwise; neither provider compares a stretch's
    /// last cell (measured in both terminals, 2026-10-24).
    fn stretch_colours(&self, row: usize, from: usize, to: usize) -> Option<(Colour, Colour)> {
        let first = self.cell_colours(row, from);
        (from + 1..to.saturating_sub(1).max(from + 1))
            .all(|column| self.cell_colours(row, column) == first)
            .then_some(first)
    }

    /// Whether a stretch is of one foreground and background, as the read
    /// program checks it: the stretch's answer and its last cell's agree.
    fn check_stretch(&self, row: usize, from: usize, to: usize) -> Option<(Colour, Colour)> {
        self.stretch_colours(row, from, to)
            .filter(|&colours| colours == self.cell_colours(row, to - 1))
    }

    /// The colours as Verbatim's read program reads them
    /// (`verbatim-uia-rops`'s `terminal_colours`, whose chunk this
    /// [`COLOUR_CHUNK`] matches): rows from the screen's top to the earlier
    /// of the document's end and the screen's, as wide as the screen's top
    /// row; each row checked whole, a row of several colours checked in
    /// chunks, and a chunk of several read cell by cell, every check
    /// answered as the providers answer it, and the last cell of a check
    /// made as the conformance runner makes a cell.
    pub(crate) fn program_colours(&self) -> Result<Vec<Vec<Run>>, Refused> {
        let screen = self.visible();
        let document = self.document();
        let top_row = self.text(self.row_of(screen.start)?)?;
        let top_row = top_row.strip_suffix("\r\n").unwrap_or(&top_row);
        let width = top_row.chars().count();
        let limit = document.end.min(screen.end);
        let mut rows = Vec::new();
        let mut at = screen.start;
        while width > 0 && rows.len() < self.grid.height && at < limit {
            let row = at.row;
            // Windows Terminal's last row of the buffer gives a cell fewer
            // to moves by characters than the width, so a stretch to its
            // end cannot vouch for its last cells, and the program reads
            // the row and its last chunk cell by cell.
            let full = self.cells_given(row) == self.grid.width;
            let mut runs = Vec::new();
            if let Some(colours) = self.check_stretch(row, 0, width).filter(|_| full) {
                push(&mut runs, 0, width, colours);
            } else {
                for from in (0..width).step_by(COLOUR_CHUNK) {
                    let to = (from + COLOUR_CHUNK).min(width);
                    let checked = self.check_stretch(row, from, to);
                    if let Some(colours) = checked.filter(|_| full || to < width) {
                        push(&mut runs, from, to - from, colours);
                    } else {
                        for column in from..to {
                            push(&mut runs, column, 1, self.cell_colours(row, column));
                        }
                    }
                }
            }
            rows.push(runs);
            let (next, moved) = self.move_rows(at, 1)?;
            if moved == 0 {
                break;
            }
            at = next;
        }
        Ok(rows)
    }
}

/// The cells Verbatim's read program checks a row of several colours in
/// at a time: `verbatim-uia-rops`'s `COLOUR_CHUNK`, which the conformance
/// runner's check of the program against the real terminals relies on.
const COLOUR_CHUNK: usize = 16;

/// Adds a stretch of one colour to a row's runs, joining it to the last
/// run when their colours agree.
fn push(
    runs: &mut Vec<Run>,
    column: usize,
    length: usize,
    (foreground, background): (Colour, Colour),
) {
    match runs.last_mut() {
        Some(run) if run.foreground == foreground && run.background == background => {
            run.length += length;
        }
        _ => runs.push(Run {
            column,
            length,
            foreground,
            background,
        }),
    }
}
