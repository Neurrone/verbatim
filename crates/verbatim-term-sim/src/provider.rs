//! The simulated terminal's answers to the questions Verbatim's read
//! program asks of a terminal's UI Automation provider: text ranges as
//! fixed positions in the active buffer, which read newer text once the
//! history rotates and fail when they lie beyond the active buffer; the
//! document ending at the end of the lower of the last row with text and
//! the cursor's; rows joined by line breaks, none after the last, and none
//! after a row whose line wraps onto the next (`docs/design/terminal-model.md`,
//! sections 2.1 and 3.3). The conformance expectations check every answer
//! against the real terminals.

use crate::Snapshot;
use crate::grid::Grid;

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
/// last found, and whether to read the whole text.
#[derive(Clone, Copy, Debug, Default)]
pub struct Query {
    /// Where the last read said to start; `None` for a first read.
    pub start: Option<Range>,
    /// The screen's height in rows as the last read found it; 0 when not
    /// known.
    pub screen_rows: u32,
    /// Whether to read the whole text from the document's start.
    pub whole: bool,
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
    /// by an insertion stay in it and a row erased leaves it (measured
    /// 2026-10-23).
    fn last_row(&self) -> usize {
        let cursor = self.grid.cursor_coordinate();
        self.grid
            .buffer()
            .rows
            .iter()
            .rposition(crate::grid::Row::has_text)
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
    /// implementation takes it.
    pub(crate) fn snapshot(&self, query: &Query) -> Result<Snapshot, Refused> {
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
        Ok(Snapshot {
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
        })
    }

    /// The colours of the screen's rows to the document's last row: for
    /// each row, its stretches of one foreground and background as shown.
    pub(crate) fn colours(&self) -> Vec<Vec<crate::Run>> {
        let buffer = self.grid.buffer();
        (buffer.top..=self.last_row().min(buffer.top + self.grid.height - 1))
            .map(|row| {
                let mut runs: Vec<crate::Run> = Vec::new();
                for (column, cell) in buffer.rows[row].cells.iter().enumerate() {
                    let (foreground, background) = if self.grid.screen_reversed {
                        let (foreground, background) = cell.style.shown();
                        (background, foreground)
                    } else {
                        cell.style.shown()
                    };
                    match runs.last_mut() {
                        Some(run)
                            if run.foreground == foreground && run.background == background =>
                        {
                            run.length += 1;
                        }
                        _ => runs.push(crate::Run {
                            column,
                            length: 1,
                            foreground,
                            background,
                        }),
                    }
                }
                runs
            })
            .collect()
    }
}
