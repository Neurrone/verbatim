//! `ReadColours`' program arm and call arm, the one operation for
//! terminals alone (the verbatim-outpost crate guide, "One implementation
//! per operation"): each row's runs of one foreground and background, from
//! a row's start, by the colour programs of `terminal_colours`, each
//! program going on from the row where the last stopped, or by its classic
//! reads.
//!
//! The first of those programs is the window read's own when the colour
//! read follows its screen's `Visible` in one program: the builder emits
//! the first rows' read there ([`emit_from`]) at the size the last colour
//! read on this thread found ([`fusable_size`]), a guess that can only
//! cost calls, since the rows count only when the size this batch's own
//! answers give is that size ([`finish`]) and the colour programs read
//! them otherwise. The outpost sized the old read's first rows from Core's
//! model; a batch names no size, so the executor keeps the last one.

use std::cell::Cell;

use windows::Win32::UI::Accessibility::{IUIAutomationTextRange, TextUnit_Line};

use verbatim_uia::text::{Endpoint, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::error::Error;
use crate::focus::Path;
use crate::operation::{Import, Outcome, ProgramCache};
use crate::terminal_colours::{COLOUR_BUDGET, ColourRegisters, ColourRun, classic_rows, emit_rows};
use crate::text::FoundPoint;

/// The widest row whose colours a window read's program reads: up to it a
/// row's worst case, under nine instructions a cell, stays under
/// [`COLOUR_BUDGET`], so the rows' loop executes no more than the budget. A
/// wider screen's colours are read by the colour programs alone.
const FUSED_WIDTH: u32 = 300;

/// The most instructions the colours a window read's program reads may
/// execute: the rows' loop, which stops at its budget, and finding where
/// the rows start and stop.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a constant far below `u32::MAX`"
)]
pub(super) const FUSED_COST: u32 = COLOUR_BUDGET as u32 + 60;

thread_local! {
    /// The rows and the width the last colour read on this thread found.
    static LAST_SIZE: Cell<Option<(u32, u32)>> = const { Cell::new(None) };
}

/// Keeps `size`, the rows and the width a colour read found, as the size
/// the next window read's program reads its colours at.
pub(super) fn note_size(size: (u32, u32)) {
    LAST_SIZE.with(|last| last.set(Some(size)));
}

/// The rows and the width a window read's program reads its colours at:
/// the last colour read's on this thread, when it read any rows no wider
/// than [`FUSED_WIDTH`]. A guess: the program's rows are taken only when
/// its batch's answers give the same size.
pub(super) fn fusable_size() -> Option<(u32, u32)> {
    LAST_SIZE
        .with(Cell::get)
        .filter(|&(rows, width)| rows > 0 && (1..=FUSED_WIDTH).contains(&width))
}

/// A colour read's first program's registers: the rows', and where the
/// rows stop.
pub(super) type FirstRows = (ColourRegisters, Reg<kind::TextRange>);

/// Emits the colour read's first program: the `count` rows, each `width`
/// cells, from the start of `top` (its end when `at_end`), as far as one
/// program's budget goes.
pub(super) fn emit_from(
    b: &mut Builder,
    top: Reg<kind::TextRange>,
    at_end: bool,
    (count, width): (u32, usize),
) -> FirstRows {
    let line = b.int(TextUnit_Line.0);
    let (start, end) = (b.int(0), b.int(1));
    let row = b.text_range_clone(top);
    if at_end {
        b.text_range_move_endpoint_by_range(row, start, row, end);
    } else {
        b.text_range_move_endpoint_by_range(row, end, row, start);
    }
    let none = b.int(0);
    let _ = b.text_range_move(row, line, none);
    b.text_range_move_endpoint_by_range(row, end, row, start);
    let limit = b.text_range_clone(row);
    let rows = b.int(i32::try_from(count).unwrap_or(i32::MAX));
    let _ = b.text_range_move(limit, line, rows);
    b.text_range_move_endpoint_by_range(limit, end, limit, start);
    let limit = b.add_to_results(limit);
    (emit_rows(b, row, limit, width), limit)
}

/// The colours of `count` rows, each `width` cells, the first of them read
/// by the program that answered `outcome` at `registers` and the rest by
/// the programs that go on from where it stopped.
///
/// # Errors
///
/// An answer missing what the registers name, or a later program's
/// [`Error`].
pub(super) fn finish(
    outcome: &Outcome,
    (registers, limit): FirstRows,
    (count, width): (u32, usize),
) -> Result<Vec<Vec<ColourRun>>, Error> {
    let height = usize::try_from(count).unwrap_or(usize::MAX);
    let mut rows = Vec::new();
    let batch = registers.read(outcome, width, &mut rows)?;
    let Some(limit): Option<IUIAutomationTextRange> = outcome.get(limit)? else {
        return Ok(rows);
    };
    crate::terminal_colours::go_on_remote(batch, &limit, (width, height), &mut rows)?;
    Ok(rows)
}

/// The colours of `count` rows, each `width` cells, from `top`'s point,
/// the first row's start: the remote programs when `remote` is true,
/// falling back to the classic reads for this call, and the classic reads
/// alone otherwise. The rows stop at the document's end.
///
/// # Errors
///
/// The classic reads' [`Error`], when they ran and failed.
pub(crate) fn row_colours(
    top: &FoundPoint,
    (count, width): (u32, usize),
    remote: bool,
) -> Result<(Vec<Vec<ColourRun>>, Path), Error> {
    if width == 0 || count == 0 {
        return Ok((
            Vec::new(),
            if remote { Path::Remote } else { Path::Classic },
        ));
    }
    if remote {
        match row_colours_remote(top, (count, width)) {
            Ok(rows) => return Ok((rows, Path::Remote)),
            Err(error) => {
                return row_colours_classic(top, (count, width))
                    .map(|rows| (rows, Path::Fallback(error)));
            }
        }
    }
    row_colours_classic(top, (count, width)).map(|rows| (rows, Path::Classic))
}

/// Where the `count` rows from `top` start and stop, the classic way, the
/// stop no later than the document's end.
fn row_bounds_classic(
    top: &FoundPoint,
    count: u32,
) -> Result<(IUIAutomationTextRange, IUIAutomationTextRange), Error> {
    // A move by no rows from the point, as the colour reads have always
    // started.
    let row = crate::text::collapsed_copy(top)?;
    row.move_by(TextUnit_Line, 0)?;
    let row = crate::caret::collapsed(&row)?;
    let limit = row.clone_range()?;
    limit.move_by(TextUnit_Line, i32::try_from(count).unwrap_or(i32::MAX))?;
    Ok((row, crate::caret::collapsed(&limit)?))
}

/// The rows' colours the classic way.
fn row_colours_classic(
    top: &FoundPoint,
    (count, width): (u32, usize),
) -> Result<Vec<Vec<ColourRun>>, Error> {
    let (row, limit) = row_bounds_classic(top, count)?;
    let at_end = row.compare_endpoints(Endpoint::Start, &limit, Endpoint::Start)? == 0;
    let mut rows = if at_end {
        Vec::new()
    } else {
        classic_rows(
            &row,
            &limit,
            width,
            usize::try_from(count).unwrap_or(usize::MAX),
        )?
    };
    rows.truncate(usize::try_from(count).unwrap_or(usize::MAX));
    Ok(rows)
}

thread_local! {
    /// The colour programs, one for each width, count of rows and end of
    /// the range the point is at.
    static ROW_COLOURS: ProgramCache<(usize, u32, bool), FirstRows> = ProgramCache::default();
}

/// The rows' colours in programs: the first starts at `top`'s point, and
/// each later one goes on from where the last stopped.
fn row_colours_remote(
    top: &FoundPoint,
    (count, width): (u32, usize),
) -> Result<Vec<Vec<ColourRun>>, Error> {
    let at_end = top.endpoint == Endpoint::End;
    let (operation, registers) = ROW_COLOURS.with(|programs| {
        programs.operation(
            (width, count, at_end),
            || vec![Import::TextRange(top.range.clone())],
            |b| {
                let top = b.import_text_range(&top.range);
                emit_from(b, top, at_end, (count, width))
            },
        )
    });
    finish(&operation.execute()?, registers, (count, width))
}
