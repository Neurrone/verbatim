//! A simulated terminal for Verbatim's tests (`docs/design/terminal-model.md`,
//! section 9): a VT emulator whose rows carry identities through scrolls,
//! rotation and clears, answering the questions Verbatim's read program
//! asks of a terminal's UI Automation provider as the console host or
//! Windows Terminal does. Its answers are checked against the real
//! terminals by the conformance expectations under `conformance/`, which
//! `cargo xtask regenerate-terminal-conformance-expectations` writes and
//! this crate's tests replay. Used by tests only.

pub mod conformance;
mod grid;
mod provider;
mod vt;

pub use grid::{
    Buffer, Cell, Colour, Cursor, DEFAULT_BACKGROUND, DEFAULT_FOREGROUND, Grid, PALETTE, Row,
    RowId, Style,
};
pub use provider::{Flavor, Position, Query, Range, Refused};

use provider::Provider;

/// What one read of a terminal answers: the read program's snapshot, as
/// its fields are compared with the real terminals'.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent answers of one read, as the read program gives them"
)]
pub struct Snapshot {
    /// The text from the read's start to the document's end, padding and
    /// line breaks included.
    pub text: String,
    /// The screen's text.
    pub screen: String,
    /// Whether the read started at the query's start.
    pub from_start: bool,
    /// Whether the query's start was refused.
    pub shrank: bool,
    /// Whether the read started at the document's start.
    pub at_document_start: bool,
    /// Whether the screen starts at the document's start.
    pub alternate: bool,
    /// The document's first row.
    pub first_row: String,
    /// How many rows the caret moved to reach the document's end.
    pub caret_rows_to_end: u32,
    /// The caret's row, as the provider expands the caret to a line.
    pub caret_line: String,
    /// The caret's column in that row.
    pub caret_column: usize,
    /// Where the next read starts.
    pub next_start: Range,
}

/// A stretch of a row in one foreground and background, as shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    /// Its first column.
    pub column: usize,
    /// Its length in cells.
    pub length: usize,
    /// Its foreground, `0x00BBGGRR`.
    pub foreground: Colour,
    /// Its background, `0x00BBGGRR`.
    pub background: Colour,
}

/// A simulated terminal.
pub struct Terminal {
    grid: Grid,
    parser: vte::Parser,
    flavor: Flavor,
}

impl Terminal {
    /// An empty terminal of `flavor`, `width` cells by `height` rows, its
    /// history holding up to `history` rows above the screen.
    #[must_use]
    pub fn new(flavor: Flavor, width: usize, height: usize, history: usize) -> Self {
        Self {
            grid: Grid::new(width, height, history, flavor == Flavor::ConsoleHost),
            parser: vte::Parser::new(),
            flavor,
        }
    }

    /// Writes `bytes` to the terminal, as a program writes to its output.
    pub fn write(&mut self, bytes: &[u8]) {
        let mut performer = vt::Performer {
            grid: &mut self.grid,
        };
        self.parser.advance(&mut performer, bytes);
    }

    /// The terminal's state, row identities included.
    #[must_use]
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    fn provider(&self) -> Provider<'_> {
        Provider {
            grid: &self.grid,
            row_includes_break: self.flavor == Flavor::WindowsTerminal,
        }
    }

    /// One read, as Verbatim's read program takes it.
    ///
    /// # Errors
    ///
    /// [`Refused`] when the provider would fail a call the read makes.
    pub fn snapshot(&self, query: &Query) -> Result<Snapshot, Refused> {
        self.provider().snapshot(query)
    }

    /// The colours of the screen's rows, each row's stretches of one
    /// foreground and background.
    #[must_use]
    pub fn colours(&self) -> Vec<Vec<Run>> {
        self.provider().colours()
    }

    /// The text of a range, as `GetText` gives it.
    ///
    /// # Errors
    ///
    /// [`Refused`] when the range lies beyond the active buffer.
    pub fn text(&self, range: Range) -> Result<String, Refused> {
        self.provider().text(range)
    }
}

/// How many rows a screen's text holds, as Verbatim counts them: each line
/// as many rows as its padded length fills at the screen's width. Every
/// line's padded length is a whole number of rows, so the width is the
/// narrowest line's, leaving out the last line, which Windows Terminal cuts
/// one cell short when it is the buffer's last row
/// (`docs/design/terminal-model.md`, section 2.1).
#[must_use]
pub fn screen_rows(screen: &str) -> u32 {
    let lines: Vec<&str> = screen
        .strip_suffix("\r\n")
        .unwrap_or(screen)
        .split("\r\n")
        .collect();
    let measured = if lines.len() > 1 {
        &lines[..lines.len() - 1]
    } else {
        &lines[..]
    };
    let width = measured
        .iter()
        .map(|line| line.chars().count())
        .filter(|&cells| cells > 0)
        .min()
        .unwrap_or(0);
    if width == 0 {
        return 0;
    }
    let rows: usize = lines
        .iter()
        .map(|line| line.chars().count().div_ceil(width).max(1))
        .sum();
    u32::try_from(rows).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
