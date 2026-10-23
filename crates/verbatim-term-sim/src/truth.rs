//! The ground truth the oracle checks readers against
//! (`docs/design/terminal-model.md`, section 9): every line the stream
//! printed and ended with a line feed, recorded by the grid as the stream is
//! written, with its rows' identities, when it was finished, when it first
//! changed, when one of its rows left the buffer, and whether it repeated
//! the screen a clear erased. Which of these lines are new output depends on
//! the reads, so the oracle decides that.

use std::collections::{HashMap, HashSet};

use crate::grid::{Row, RowId};

/// A line the stream printed and ended with a line feed.
///
/// A line is the rows a wrapped line spans: a row and the rows after it
/// while each wraps onto the next. One is recorded each time all of this
/// holds:
///
/// - On the main screen, an explicit line feed (line feed, vertical tab,
///   form feed, index or next line) on its last row moved the cursor to
///   another row, by moving down or by scrolling. A line feed that leaves
///   the cursor on the line's row, as one below the scroll region on the
///   screen's last row does, finishes nothing, and neither does one on a
///   row that wraps onto the next.
/// - It has text: something other than spaces.
/// - Its first row was not the continuation of a row that left the
///   buffer, which would make it the tail of a line whose start is gone.
/// - It is not the line recorded last on the same rows, unchanged since:
///   a line feed on a line already finished finishes nothing new.
///
/// How its characters came to be does not matter: a line redrawn after a
/// carriage return, an erased row written again and a region redrawn in
/// place are recorded as they stand when the line feed ends them, and each
/// redraw ended by another line feed is recorded again.
///
/// It changes when a character of its rows changes (written over, erased,
/// or shifted by characters inserted or deleted before it), when a row of
/// it starts or stops wrapping onto the next, or when the row above it
/// starts or stops wrapping onto it, which would join it to that row's line
/// or split it; a row scrolled under a wrapping row joins it too. Erasing
/// cells that hold only spaces, and colours, change nothing. Rows that move
/// change nothing either: a line scrolled into the history, pushed there by
/// erasing the screen, or moved by inserting or deleting rows elsewhere is
/// the same line. A row that leaves the buffer, by rotation out of a full
/// history, by erasing the history, or by being deleted or scrolled out of
/// a scroll region, is not a change: `left` says when it went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintedLine {
    /// Its text: its rows' cells joined, padding after its last character
    /// left out.
    pub text: String,
    /// Its rows' identities, first to last.
    pub rows: Vec<RowId>,
    /// The write that finished it, counting `Terminal::write` calls from
    /// 1: a read taken after `finished` writes is the first that can find
    /// it.
    pub finished: u64,
    /// The write during which it first changed after it was finished, if it
    /// did: a read taken after that write shows something else.
    pub changed: Option<u64>,
    /// The write during which one of its rows left the buffer, if one did.
    pub left: Option<u64>,
    /// Whether it repeats the screen the last clear erased: after the
    /// screen is erased on the main buffer (erase in display 2), the lines
    /// recorded next, while each has the text of the next line with text
    /// the screen showed just before the clear, from its first, in order.
    /// The first line that differs, or one recorded after all of them were
    /// repeated, ends the reprint until the next clear.
    pub reprint: bool,
}

/// What the grid records for the oracle as the stream is written.
#[derive(Clone, Debug, Default)]
pub(crate) struct Record {
    /// How many writes have begun.
    pub(crate) writes: u64,
    /// Every line printed, in the order they were finished.
    lines: Vec<PrintedLine>,
    /// The line each row of a line not yet changed belongs to.
    live: HashMap<RowId, usize>,
    /// Rows whose row above wrapped onto them and left the buffer.
    headless: HashSet<RowId>,
    /// Since the last clear, while lines repeat it: the lines with text the
    /// screen showed just before it, and how many have been repeated.
    reprint: Option<(Vec<String>, usize)>,
}

impl Record {
    /// Line `line` changed, if it had not before.
    fn change(&mut self, line: usize) {
        let printed = &mut self.lines[line];
        if printed.changed.is_none() {
            printed.changed = Some(self.writes);
            for row in &printed.rows {
                self.live.remove(row);
            }
        }
    }

    /// Row `row`'s characters changed.
    pub(crate) fn changed(&mut self, row: RowId) {
        if let Some(&line) = self.live.get(&row) {
            self.change(line);
        }
    }

    /// Row `row` started or stopped wrapping onto `next`, or `next` was
    /// placed under `row` while it wraps.
    pub(crate) fn wraps_changed(&mut self, row: RowId, next: Option<RowId>) {
        for id in std::iter::once(row).chain(next) {
            if let Some(&line) = self.live.get(&id) {
                self.change(line);
            }
        }
    }

    /// `row` left the buffer; `next` is the row that followed it.
    pub(crate) fn left(&mut self, row: &Row, next: Option<RowId>) {
        if let Some(&line) = self.live.get(&row.id) {
            self.lines[line].left.get_or_insert(self.writes);
        }
        if row.wraps
            && let Some(next) = next
        {
            self.headless.insert(next);
        }
    }

    /// The main screen was erased; `shown` is its lines with text just
    /// before, top to bottom.
    pub(crate) fn cleared(&mut self, shown: Vec<String>) {
        self.reprint = Some((shown, 0));
    }

    /// A line feed moved the cursor off the line of `rows`, whose text is
    /// `text`, padding left out.
    pub(crate) fn finish(&mut self, rows: Vec<RowId>, text: String) {
        if text.is_empty() || rows.iter().any(|row| self.headless.contains(row)) {
            return;
        }
        if let Some(&line) = self.live.get(&rows[0])
            && self.lines[line].rows == rows
        {
            return;
        }
        let reprint = match &mut self.reprint {
            Some((shown, repeated)) if shown.get(*repeated) == Some(&text) => {
                *repeated += 1;
                true
            }
            _ => {
                self.reprint = None;
                false
            }
        };
        let index = self.lines.len();
        for &row in &rows {
            self.live.insert(row, index);
        }
        self.lines.push(PrintedLine {
            text,
            rows,
            finished: self.writes,
            changed: None,
            left: None,
            reprint,
        });
    }

    /// Every line printed, in the order they were finished.
    pub(crate) fn printed(&self) -> &[PrintedLine] {
        &self.lines
    }
}
