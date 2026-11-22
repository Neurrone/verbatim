//! `ReadColours`' program arm and call arm, the one operation for
//! terminals alone (the verbatim-outpost crate guide, "One implementation
//! per operation"): each row's runs of one foreground and background, from
//! a row's start, by the colour programs of `terminal_colours`, each
//! program going on from the row where the last stopped, or by its classic
//! reads.

use windows::Win32::UI::Accessibility::{IUIAutomationTextRange, TextUnit_Line};

use verbatim_uia::text::{Endpoint, TextRangeExt};

use crate::builder::{Reg, kind};
use crate::error::Error;
use crate::focus::Path;
use crate::operation::{Import, ProgramCache};
use crate::terminal_colours::{ColourRun, classic_rows, emit_rows};
use crate::text::FoundPoint;

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
    static ROW_COLOURS: ProgramCache<
        (usize, u32, bool),
        (crate::terminal_colours::ColourRegisters, Reg<kind::TextRange>),
    > = ProgramCache::default();
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
            },
        )
    });
    let (registers, limit) = registers;
    let outcome = operation.execute()?;
    let height = usize::try_from(count).unwrap_or(usize::MAX);
    let mut rows = Vec::new();
    let batch = registers.read(&outcome, width, &mut rows)?;
    let Some(limit): Option<IUIAutomationTextRange> = outcome.get(limit)? else {
        return Ok(rows);
    };
    crate::terminal_colours::go_on_remote(batch, &limit, (width, height), &mut rows)?;
    Ok(rows)
}
