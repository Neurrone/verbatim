//! The ground truth the oracle checks readers against
//! (`docs/design/terminal-model.md`, section 9): the lines the stream wrote
//! as output, recorded by the grid as the stream is written, with their
//! rows' identities and when they were finished and left the buffer.

use std::collections::{HashMap, HashSet};

use crate::grid::{Row, RowId};

/// A line the stream wrote as output.
///
/// A line is the rows a wrapped line spans: a row and the rows after it
/// while each wraps onto the next. It is output when all of this holds:
///
/// - It was written on the main screen, and an explicit line feed (line
///   feed, vertical tab or form feed) on its last row moved the cursor to
///   another row, by moving down or by scrolling. A line feed that leaves
///   the cursor on the line's row, as one below the scroll region on the
///   screen's last row does, finishes nothing, and neither does one on a
///   row that wraps onto the next.
/// - It has text: something other than spaces.
/// - Until that line feed, every change to its rows' characters appended:
///   it put a character where the row had only spaces from that column to
///   its end. A character written over another, a cell with text erased,
///   and characters inserted or deleted before text are rewrites, so a line
///   redrawn after a carriage return, an erased row written again, and a
///   region redrawn in place are never output, whatever they show at the
///   end. Erasing cells that hold only spaces, and colours, change nothing.
/// - Never changed afterwards: after that line feed no character of its
///   rows changed, no row of it started or stopped wrapping onto the next,
///   and the row above it did not start or stop wrapping onto it, which
///   would join it to that row's line or split it.
/// - Its first row was not the continuation of a row that left the
///   buffer, which would make it the tail of a line whose start is gone.
///
/// Rows that move change nothing: a line scrolled into the history, pushed
/// there by erasing the screen, or moved by inserting or deleting rows
/// elsewhere is the same line. A row that leaves the buffer, by rotation
/// out of a full history, by erasing the history, or by being deleted or
/// scrolled out of a scroll region, is not a change either: the line
/// stays output, and `left` says when it went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputLine {
    /// Its text: its rows' cells joined, padding after its last character
    /// left out.
    pub text: String,
    /// Its rows' identities, first to last.
    pub rows: Vec<RowId>,
    /// The write that finished it, counting `Terminal::write` calls from
    /// 1: a read taken after `finished` writes is the first that can find
    /// it finished.
    pub finished: u64,
    /// The write during which one of its rows left the buffer, if one did.
    pub left: Option<u64>,
}

/// What the grid records for the oracle as the stream is written.
#[derive(Clone, Debug, Default)]
pub(crate) struct Record {
    /// How many writes have begun.
    pub(crate) writes: u64,
    /// Every line finished, with whether it changed afterwards.
    lines: Vec<(OutputLine, bool)>,
    /// The line each finished row belongs to.
    rows: HashMap<RowId, usize>,
    /// The line each finished line's first row starts.
    firsts: HashMap<RowId, usize>,
    /// Rows whose characters were ever rewritten rather than appended to.
    rewritten: HashSet<RowId>,
    /// Rows whose row above wrapped onto them and left the buffer.
    headless: HashSet<RowId>,
}

impl Record {
    /// Row `row`'s characters changed; `append` when the change only put
    /// characters where the row had only spaces to its end.
    pub(crate) fn changed(&mut self, row: RowId, append: bool) {
        if !append {
            self.rewritten.insert(row);
        }
        if let Some(&line) = self.rows.get(&row) {
            self.lines[line].1 = true;
        }
    }

    /// Row `row` started or stopped wrapping onto `next`.
    pub(crate) fn wraps_changed(&mut self, row: RowId, next: Option<RowId>) {
        if let Some(&line) = self.rows.get(&row) {
            self.lines[line].1 = true;
        }
        if let Some(next) = next
            && let Some(&line) = self.firsts.get(&next)
        {
            self.lines[line].1 = true;
        }
    }

    /// `row` left the buffer; `next` is the row that followed it.
    pub(crate) fn left(&mut self, row: &Row, next: Option<RowId>) {
        if let Some(&line) = self.rows.get(&row.id) {
            self.lines[line].0.left.get_or_insert(self.writes);
        }
        if row.wraps
            && let Some(next) = next
        {
            self.headless.insert(next);
        }
    }

    /// A line feed moved the cursor off the line of `rows`, whose text is
    /// `text`, padding left out.
    pub(crate) fn finish(&mut self, rows: Vec<RowId>, text: String) {
        let fresh = rows.iter().all(|row| {
            !self.rewritten.contains(row)
                && !self.rows.contains_key(row)
                && !self.headless.contains(row)
        });
        if text.is_empty() || !fresh {
            return;
        }
        let index = self.lines.len();
        for &row in &rows {
            self.rows.insert(row, index);
        }
        self.firsts.insert(rows[0], index);
        self.lines.push((
            OutputLine {
                text,
                rows,
                finished: self.writes,
                left: None,
            },
            false,
        ));
    }

    /// The output lines, in the order they were finished.
    pub(crate) fn output(&self) -> Vec<OutputLine> {
        self.lines
            .iter()
            .filter(|(_, changed)| !changed)
            .map(|(line, _)| line.clone())
            .collect()
    }
}
