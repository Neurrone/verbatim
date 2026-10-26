//! Runs of a terminal's history rows read by their place
//! (`docs/design/terminal-model.md`, "Reading rules", rule 4): when a candidate
//! rotation puts the history's block above the snapshot just read, the rows
//! it predicts there are read from the same start the snapshot was asked
//! for, each run moved up by its rows, so one round trip checks every
//! candidate. The document's first row is read after them, so the caller
//! can tell whether the buffer moved between the snapshot and this read. A
//! remote program reads them in one round trip, and a classic
//! implementation behind the same signature reads them call by call;
//! [`terminal_rows`] is the one function callers use.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit_Line,
};

use verbatim_uia::Uia;
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::error::Error;
use crate::focus::Path;
use crate::operation::{Import, ProgramCache};

/// The most runs one read takes. Each run is ten instructions of the
/// remote program, beside the twelve it takes once.
pub const MOST_RUNS: usize = 16;

/// What [`terminal_rows`] is asked.
#[derive(Clone, Copy)]
pub struct RowsQuery<'a> {
    /// The element with the text.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern.
    pub pattern: &'a IUIAutomationTextPattern,
    /// The start the snapshot was asked for (its `start`, the last read's
    /// `next_start`), from which every run is counted.
    pub start: &'a IUIAutomationTextRange,
    /// The runs, each how many rows above `start` it begins and how many
    /// rows it holds; at most [`MOST_RUNS`].
    pub runs: &'a [(u32, u32)],
}

/// The answer to a [`RowsQuery`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowsAnswer {
    /// Each run's text, UTF-16 as the provider gave it, line breaks
    /// included; empty for a run whose first row the document's start came
    /// before.
    pub runs: Vec<Vec<u16>>,
    /// The document's first row, read after the runs.
    pub first_row: String,
}

/// The signature both implementations share.
pub type TerminalRowsFn = fn(&Uia, &RowsQuery<'_>) -> Result<RowsAnswer, Error>;

/// Runs of rows by their place, the one function call sites use: the
/// remote program when `remote` is true, falling back to the classic
/// implementation for this call when the program fails, and the classic
/// implementation alone when `remote` is false. Says which path answered.
///
/// # Errors
///
/// The classic implementation's [`Error`], when it ran and failed.
pub fn terminal_rows(
    uia: &Uia,
    query: &RowsQuery<'_>,
    remote: bool,
) -> Result<(RowsAnswer, Path), Error> {
    if !remote {
        return terminal_rows_classic(uia, query).map(|answer| (answer, Path::Classic));
    }
    match terminal_rows_remote(uia, query) {
        Ok(answer) => Ok((answer, Path::Remote)),
        Err(error) => {
            terminal_rows_classic(uia, query).map(|answer| (answer, Path::Fallback(error)))
        }
    }
}

/// The registers the program's answer is read from: each run's text, then
/// the first row.
#[derive(Clone)]
struct RowsRegisters {
    runs: Vec<Reg<kind::Str>>,
    first_row: Reg<kind::Str>,
}

thread_local! {
    /// The programs this thread built, by their runs.
    static PROGRAMS: ProgramCache<Vec<(u32, u32)>, RowsRegisters> = ProgramCache::default();
}

/// The runs in one cross-process round trip, the program built once for
/// each list of runs on a thread and run again with the next read's start.
///
/// `uia` is unused; it is in the signature so that this and
/// [`terminal_rows_classic`] are interchangeable ([`TerminalRowsFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn terminal_rows_remote(_uia: &Uia, query: &RowsQuery<'_>) -> Result<RowsAnswer, Error> {
    let runs = query.runs[..query.runs.len().min(MOST_RUNS)].to_vec();
    let (operation, r) = PROGRAMS.with(|programs| {
        programs.operation(
            runs.clone(),
            || {
                vec![
                    Import::Element(query.element.clone()),
                    Import::TextRange(query.start.clone()),
                ]
            },
            |b| build_rows(b, query, &runs),
        )
    });
    let outcome = operation.execute()?;
    Ok(RowsAnswer {
        runs: r
            .runs
            .iter()
            .map(|&reg| outcome.get_utf16(reg))
            .collect::<Result<_, _>>()?,
        first_row: String::from_utf16_lossy(&outcome.get_utf16(r.first_row)?),
    })
}

/// Emits the program into `b`: for each run, a copy of the start collapsed
/// to it, moved up by the run's rows and, when it moved that far, its end
/// moved down by the run's rows and its text read; then the document's
/// first row.
fn build_rows(b: &mut Builder, query: &RowsQuery<'_>, runs: &[(u32, u32)]) -> RowsRegisters {
    let element = b.import_element(query.element);
    let kept = b.import_text_range(query.start);
    let start_endpoint = b.int(0);
    let end_endpoint = b.int(1);
    let line = b.int(TextUnit_Line.0);
    let all = b.int(-1);
    let pattern = b.get_text_pattern(element, false);
    let mut registers = Vec::with_capacity(runs.len());
    for &(above, rows) in runs {
        let text = b.new_string("");
        let text = b.add_to_results(text);
        let at = b.text_range_clone(kept);
        b.text_range_move_endpoint_by_range(at, end_endpoint, at, start_endpoint);
        let back = b.int(-i32::try_from(above).unwrap_or(i32::MAX));
        let moved = b.text_range_move(at, line, back);
        let there = b.equal(moved, back);
        b.if_(there, |b| {
            let count = b.int(i32::try_from(rows).unwrap_or(i32::MAX));
            let _ = b.text_range_move_endpoint_by_unit(at, end_endpoint, line, count);
            let read = b.text_range_get_text(at, all);
            b.set(text, read);
        });
        registers.push(text);
    }
    let document = b.text_pattern_get_document_range(pattern);
    let first = b.text_range_clone(document);
    b.text_range_move_endpoint_by_range(first, end_endpoint, first, start_endpoint);
    b.text_range_expand_to_enclosing_unit(first, line);
    let first_row = b.text_range_get_text(first, all);
    let first_row = b.add_to_results(first_row);
    RowsRegisters {
        runs: registers,
        first_row,
    }
}

/// The runs the classic way, the fallback and the reference: the same
/// steps as the remote program, each a cross-process call, counted on the
/// thread's `verbatim_uia::calls`.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails.
pub fn terminal_rows_classic(_uia: &Uia, query: &RowsQuery<'_>) -> Result<RowsAnswer, Error> {
    let mut answer = RowsAnswer::default();
    for &(above, rows) in &query.runs[..query.runs.len().min(MOST_RUNS)] {
        let at = query.start.clone_range()?;
        at.move_endpoint_to(Endpoint::End, &at, Endpoint::Start)?;
        let back = -i32::try_from(above).unwrap_or(i32::MAX);
        if at.move_by(TextUnit_Line, back)? == back {
            at.move_endpoint_by_unit(
                Endpoint::End,
                TextUnit_Line,
                i32::try_from(rows).unwrap_or(i32::MAX),
            )?;
            answer.runs.push(at.text(-1)?);
        } else {
            answer.runs.push(Vec::new());
        }
    }
    let first = query.pattern.document_range()?;
    first.move_endpoint_to(Endpoint::End, &first, Endpoint::Start)?;
    first.expand(TextUnit_Line)?;
    answer.first_row = String::from_utf16_lossy(&first.text(-1)?);
    Ok(answer)
}
