//! A terminal's window read, the request list's `TerminalRead`
//! (`verbatim_model::batch::Op::TerminalRead`): the rows from two screens
//! above the document's end to its end in one `GetText`, which the terminal
//! answers from one locked state of its buffer, with the hints a read of
//! the terminal needs beside it, each from a call of its own: the screen's
//! text and where its top lies counted from the document's end, whether the
//! screen starts at the document's start, the document's end as a range to
//! hold, the document's first row and its row count, and, for each end
//! mark the caller names, how many rows were written past it, with the
//! first rows after it when more were written than the caller speaks in
//! full and the window holds. The caret and its line are read in the same
//! round trip.
//!
//! The screen does not always end at the document's end: before a screen
//! fills, and after a clear, its last rows lie past the end, blank. No
//! document row ever lies below the screen (checked over every read of the
//! conformance corpus, `verbatim-term-sim`'s `no_document_row_lies_below_a_reads_screen`),
//! so the window from the document's end holds the whole screen.
//!
//! A remote program reads it in one round trip and a classic
//! implementation behind the same signature call by call; [`terminal_read`]
//! is the one function callers use. Two parts are not in the program. The
//! screen's colours are read afterwards, row by row from the screen's top
//! ([`terminal_row_colours`]), at the width its top row gives, which the
//! colour programs are built for. And the search for an end mark's line
//! ([`terminal_read_find`]) runs only as classic calls: output that trims
//! the history between two searches makes a later miss Windows Terminal's
//! nowhere range, whose calls end a remote run whatever try block holds
//! them (`terminal_find.rs`, "Limits"), while the classic calls answer it.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit_Line,
};

use verbatim_uia::Uia;
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::caret::{
    CaretAnswer, CaretLineQuery, CaretLineRegisters, caret_line_classic, emit_caret_line,
};
use crate::error::Error;
use crate::focus::Path;
use crate::opcode::Comparison;
use crate::operation::{Import, Outcome, ProgramCache, Value};
use crate::terminal_colours::{ColourRun, classic_rows, emit_rows};
use crate::terminal_find::{FindMatch, FindQuery, classic_search};

/// More rows than any terminal holds, for moving to the end of the text.
const FAR: i32 = 1_000_000;

/// What [`terminal_read`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct ReadQuery<'a> {
    /// The element with the text.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern.
    pub pattern: &'a IUIAutomationTextPattern,
    /// The screen's height in rows as the last read found it, 0 at first:
    /// the window starts twice that many rows above the document's end, or
    /// at the screen's top when it is 0.
    pub height: u32,
    /// The end marks the caller names, each a range at an earlier read's
    /// document end: the current one, then the one kept aside.
    pub marks: [Option<&'a IUIAutomationTextRange>; 2],
    /// How many rows the caller speaks in full ("Lines spoken in full"):
    /// past a mark with more rows written after it than this and the
    /// window together, the first this many are read.
    pub opening: u32,
    /// The caret and its line.
    pub caret: CaretLineQuery<'a>,
}

/// What one end mark showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkRows {
    /// The mark lies past the document's end, or the provider would not
    /// compare it with the text: the text shrank, or the mark is on the
    /// other screen.
    Refused,
    /// How many rows were written past the mark, with the text of the
    /// first [`ReadQuery::opening`] rows after it, padding included, when
    /// more were written than those and the window together.
    Rows {
        /// Rows from the mark to the document's end.
        written: u32,
        /// The opening rows' text, UTF-16 as the provider gave it.
        opening: Option<Vec<u16>>,
    },
}

/// The answer to a [`ReadQuery`]. Only [`Self::rows`] is read in one
/// provider call; everything else is a hint read in calls of its own.
#[derive(Debug)]
pub struct Window {
    /// The text from two screens above the document's end to its end,
    /// UTF-16 as the provider gave it, padding and line breaks included.
    pub rows: Vec<u16>,
    /// How many rows the window starts above the document's end.
    pub above: u32,
    /// The first visible range's text, for the screen's height.
    pub screen: Vec<u16>,
    /// The screen's top row as the provider expands its start to a line,
    /// for the screen's width ([`crate::row_width`]): a wrapped line's
    /// first row alone, where the screen's text gives the line whole.
    pub top_row: Vec<u16>,
    /// How many rows the screen's top lies above the document's end: the
    /// lines a move from the screen's top to the end went.
    pub screen_top: u32,
    /// Where the screen's top row starts, from which the caller's colour
    /// reads count rows.
    pub top: Option<IUIAutomationTextRange>,
    /// Whether the screen starts at the document's start.
    pub screen_at_start: bool,
    /// The document's end, collapsed, for the caller to hold as the next
    /// end mark.
    pub end: Option<IUIAutomationTextRange>,
    /// The document's first row, as the provider gave it.
    pub first_row: Vec<u16>,
    /// How many lines a move from the document's end to its start went.
    pub document_rows: u32,
    /// What each mark named showed, in the query's order.
    pub marks: [Option<MarkRows>; 2],
    /// The caret and its line.
    pub caret: Option<CaretAnswer>,
    /// How many rows the caret's row and those below it hold, to the
    /// document's end: the lines a move from the caret to the end went,
    /// as the snapshot program counts them. `None` when the provider
    /// reports no caret.
    pub caret_rows_to_end: Option<u32>,
}

/// The signature both window read implementations share.
pub type TerminalReadFn = fn(&Uia, &ReadQuery<'_>) -> Result<Window, Error>;

/// A terminal's window read, the one function call sites use: the remote
/// program when `remote` is true, falling back to the classic
/// implementation for this call when the program fails, and the classic
/// implementation alone when `remote` is false. Says which path answered.
///
/// # Errors
///
/// The classic implementation's [`Error`], when it ran and failed.
pub fn terminal_read(
    uia: &Uia,
    query: &ReadQuery<'_>,
    remote: bool,
) -> Result<(Window, Path), Error> {
    if !remote {
        return terminal_read_classic(uia, query).map(|window| (window, Path::Classic));
    }
    match terminal_read_remote(uia, query) {
        Ok(window) => Ok((window, Path::Remote)),
        Err(error) => {
            terminal_read_classic(uia, query).map(|window| (window, Path::Fallback(error)))
        }
    }
}

/// Everything the program is built from but its imports.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ReadKey {
    height: u32,
    marks: [bool; 2],
    opening: u32,
    max_text: i32,
}

/// The registers of one mark's answer.
#[derive(Clone, Copy)]
struct MarkRegisters {
    refused: Reg<kind::Bool>,
    written: Reg<kind::Int>,
    long: Reg<kind::Bool>,
    opening: Reg<kind::Str>,
}

/// The registers the program's answer is read from.
#[derive(Clone, Copy)]
struct ReadRegisters {
    rows: Reg<kind::Str>,
    above: Reg<kind::Int>,
    screen: Reg<kind::Str>,
    top_row: Reg<kind::Str>,
    screen_top: Reg<kind::Int>,
    top: Reg<kind::TextRange>,
    screen_at_start: Reg<kind::Bool>,
    end: Reg<kind::TextRange>,
    first_row: Reg<kind::Str>,
    document_rows: Reg<kind::Int>,
    marks: [Option<MarkRegisters>; 2],
    caret: CaretLineRegisters,
    /// The caret's rows to the end, -1 for no caret.
    caret_rows: Reg<kind::Int>,
}

thread_local! {
    /// The window read programs this thread built.
    static READS: ProgramCache<ReadKey, ReadRegisters> = ProgramCache::default();
}

/// A count of rows from a move's answer, never below zero.
fn rows_of(moved: i32) -> u32 {
    moved.unsigned_abs()
}

/// The window read in one cross-process round trip, the program built
/// once for each [`ReadKey`] on a thread and run again with the next
/// read's imports: the element, then each mark named.
///
/// `uia` is unused; it is in the signature so that this and
/// [`terminal_read_classic`] are interchangeable ([`TerminalReadFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn terminal_read_remote(_uia: &Uia, query: &ReadQuery<'_>) -> Result<Window, Error> {
    let key = ReadKey {
        height: query.height,
        marks: query.marks.map(|mark| mark.is_some()),
        opening: query.opening,
        max_text: query.caret.max_text,
    };
    let (operation, r) = READS.with(|programs| {
        programs.operation(
            key,
            || {
                std::iter::once(Import::Element(query.element.clone()))
                    .chain(
                        query
                            .marks
                            .iter()
                            .flatten()
                            .map(|mark| Import::TextRange((*mark).clone())),
                    )
                    .collect()
            },
            |b| build_read(b, query),
        )
    });
    let outcome = operation.execute()?;
    let mark = |registers: Option<MarkRegisters>| -> Result<Option<MarkRows>, Error> {
        let Some(m) = registers else {
            return Ok(None);
        };
        if outcome.get(m.refused)? {
            return Ok(Some(MarkRows::Refused));
        }
        Ok(Some(MarkRows::Rows {
            written: rows_of(outcome.get(m.written)?),
            opening: if outcome.get(m.long)? {
                Some(utf16_of(&outcome, m.opening)?)
            } else {
                None
            },
        }))
    };
    Ok(Window {
        rows: outcome.get_utf16(r.rows)?,
        above: rows_of(outcome.get(r.above)?),
        screen: outcome.get_utf16(r.screen)?,
        top_row: utf16_of(&outcome, r.top_row)?,
        screen_top: rows_of(outcome.get(r.screen_top)?),
        top: outcome.get(r.top)?,
        screen_at_start: outcome.get(r.screen_at_start)?,
        end: outcome.get(r.end)?,
        first_row: utf16_of(&outcome, r.first_row)?,
        document_rows: rows_of(outcome.get(r.document_rows)?),
        marks: [mark(r.marks[0])?, mark(r.marks[1])?],
        caret: Some(r.caret.read(&outcome)?),
        caret_rows_to_end: u32::try_from(outcome.get(r.caret_rows)?).ok(),
    })
}

/// A string result as UTF-16, empty for a null one.
fn utf16_of(outcome: &Outcome, reg: Reg<kind::Str>) -> Result<Vec<u16>, Error> {
    Ok(match outcome.get(reg.any())? {
        Value::String(_) => outcome.get_utf16(reg)?,
        _ => Vec::new(),
    })
}

/// Emits the window read's program into `b`.
fn build_read(b: &mut Builder, query: &ReadQuery<'_>) -> ReadRegisters {
    let element = b.import_element(query.element);
    let imported = query
        .marks
        .map(|mark| mark.map(|range| b.import_text_range(range)));
    let start = b.int(0);
    let end = b.int(1);
    let line = b.int(TextUnit_Line.0);
    let all = b.int(-1);
    let zero = b.int(0);
    let far = b.int(FAR);
    let back = b.int(-FAR);
    // A copy of a range collapsed to its start, and one to its end.
    let at_start = |b: &mut Builder, range: Reg<kind::TextRange>| {
        let copy = b.text_range_clone(range);
        b.text_range_move_endpoint_by_range(copy, end, copy, start);
        copy
    };
    let at_finish = |b: &mut Builder, range: Reg<kind::TextRange>| {
        let copy = b.text_range_clone(range);
        b.text_range_move_endpoint_by_range(copy, start, copy, end);
        copy
    };
    let pattern = b.get_text_pattern(element, false);
    let document = b.text_pattern_get_document_range(pattern);

    // The screen: the first visible range, or the whole text when the
    // provider reports none.
    let screen = b.text_range_clone(document);
    let ranges = b.text_pattern_get_visible_ranges(pattern);
    let size = b.array_size(ranges);
    let none = b.uint(0);
    let some = b.compare(size, none, Comparison::GreaterThan);
    b.if_(some, |b| {
        let first = b.array_get_at(ranges, none).assume::<kind::TextRange>();
        b.set(screen, first);
    });
    let screen_text = b.text_range_get_text(screen, all);
    let screen_text = b.add_to_results(screen_text);
    let order = b.text_range_compare_endpoints(document, start, screen, start);
    let screen_at_start = b.equal(order, zero);
    let screen_at_start = b.add_to_results(screen_at_start);
    let top = at_start(b, screen);
    let top = b.add_to_results(top);
    let top_row = b.text_range_clone(top);
    b.text_range_expand_to_enclosing_unit(top_row, line);
    let top_row = b.text_range_get_text(top_row, all);
    let top_row = b.add_to_results(top_row);
    let walker = b.text_range_clone(top);
    let screen_top = b.text_range_move(walker, line, far);
    let screen_top = b.add_to_results(screen_top);

    // The document's end, and the window: the one `GetText`.
    let at_end = at_finish(b, document);
    let at_end = b.add_to_results(at_end);
    let window = if query.height > 0 {
        let window = b.text_range_clone(at_end);
        let up = b.int(-2 * i32::try_from(query.height).unwrap_or(i32::MAX / 2));
        let moved = b.text_range_move(window, line, up);
        (window, moved)
    } else {
        let window = b.text_range_clone(top);
        let walker = b.text_range_clone(top);
        let down = b.text_range_move(walker, line, far);
        (window, b.subtract(zero, down))
    };
    let above = b.add_to_results(window.1);
    let window = window.0;
    b.text_range_move_endpoint_by_range(window, end, document, end);
    let rows = b.text_range_get_text(window, all);
    let rows = b.add_to_results(rows);

    let first = at_start(b, document);
    b.text_range_expand_to_enclosing_unit(first, line);
    let first_row = b.text_range_get_text(first, all);
    let first_row = b.add_to_results(first_row);
    let walker = b.text_range_clone(at_end);
    let document_rows = b.text_range_move(walker, line, back);
    let document_rows = b.add_to_results(document_rows);

    let opening = b.int(i32::try_from(query.opening).unwrap_or(i32::MAX));
    let marks =
        imported.map(|mark| mark.map(|mark| emit_mark(b, mark, document, (above, opening))));
    let caret_rows = emit_caret_rows(b, pattern);
    let caret = emit_caret_line(b, &query.caret, (pattern, None));
    ReadRegisters {
        rows,
        above,
        screen: screen_text,
        top_row,
        screen_top,
        top,
        screen_at_start,
        end: at_end,
        first_row,
        document_rows,
        marks,
        caret,
        caret_rows,
    }
}

/// Emits the caret's rows to the document's end: a move by line from the
/// selection's first range, collapsed to its start, to the end; -1 when
/// the provider reports no selection.
fn emit_caret_rows(b: &mut Builder, pattern: Reg<kind::TextPattern>) -> Reg<kind::Int> {
    let (start, end) = (b.int(0), b.int(1));
    let line = b.int(TextUnit_Line.0);
    let far = b.int(FAR);
    let none = b.uint(0);
    let caret_rows = b.new_int(-1);
    let caret_rows = b.add_to_results(caret_rows);
    let selection = b.text_pattern_get_selection(pattern);
    let size = b.array_size(selection);
    let some = b.compare(size, none, Comparison::GreaterThan);
    b.if_(some, |b| {
        let first = b.array_get_at(selection, none).assume::<kind::TextRange>();
        let walker = b.text_range_clone(first);
        b.text_range_move_endpoint_by_range(walker, end, walker, start);
        let moved = b.text_range_move(walker, line, far);
        b.set(caret_rows, moved);
    });
    caret_rows
}

/// Emits what one end mark shows: refused when it lies past the
/// document's end or the provider will not compare it, else the rows from
/// it to the end, and the `opening` rows after it when more than those and
/// the window's `above` rows (a negative count) follow it.
fn emit_mark(
    b: &mut Builder,
    mark: Reg<kind::TextRange>,
    document: Reg<kind::TextRange>,
    (above, opening): (Reg<kind::Int>, Reg<kind::Int>),
) -> MarkRegisters {
    let start = b.int(0);
    let end = b.int(1);
    let line = b.int(TextUnit_Line.0);
    let all = b.int(-1);
    let zero = b.int(0);
    let far = b.int(FAR);
    let refused = b.new_bool(true);
    let refused = b.add_to_results(refused);
    let written = b.new_int(0);
    let written = b.add_to_results(written);
    let long = b.new_bool(false);
    let long = b.add_to_results(long);
    let text = b.new_string("");
    let text = b.add_to_results(text);
    b.try_catch(
        |b| {
            let at = b.text_range_clone(mark);
            b.text_range_move_endpoint_by_range(at, end, at, start);
            let order = b.text_range_compare_endpoints(at, start, document, end);
            let within = b.compare(order, zero, Comparison::LessThanOrEqual);
            b.if_(within, |b| {
                let no = b.bool(false);
                b.set(refused, no);
                let walker = b.text_range_clone(at);
                let moved = b.text_range_move(walker, line, far);
                b.set(written, moved);
                let limit = b.subtract(zero, above);
                let limit = b.add(limit, opening);
                let past = b.compare(moved, limit, Comparison::GreaterThan);
                b.if_(past, |b| {
                    let yes = b.bool(true);
                    b.set(long, yes);
                    let rows = b.text_range_clone(at);
                    let _ = b.text_range_move_endpoint_by_unit(rows, end, line, opening);
                    let read = b.text_range_get_text(rows, all);
                    b.set(text, read);
                });
            });
        },
        |_, _status| {},
    );
    MarkRegisters {
        refused,
        written,
        long,
        opening: text,
    }
}

/// A copy of `range` collapsed to its `at` end. Two calls.
fn collapsed(
    range: &IUIAutomationTextRange,
    at: Endpoint,
) -> Result<IUIAutomationTextRange, Error> {
    let copy = range.clone_range()?;
    let other = match at {
        Endpoint::Start => Endpoint::End,
        Endpoint::End => Endpoint::Start,
    };
    copy.move_endpoint_to(other, &copy, at)?;
    Ok(copy)
}

/// What one mark showed, the classic way: refused when a call fails.
fn mark_classic(
    mark: &IUIAutomationTextRange,
    document: &IUIAutomationTextRange,
    (above, opening): (u32, u32),
) -> MarkRows {
    let read = || -> Result<MarkRows, Error> {
        let at = collapsed(mark, Endpoint::Start)?;
        if at.compare_endpoints(Endpoint::Start, document, Endpoint::End)? > 0 {
            return Ok(MarkRows::Refused);
        }
        let written = at.clone_range()?.move_by(TextUnit_Line, FAR)?;
        let long = i64::from(written) > i64::from(opening) + i64::from(above);
        let opening = if long {
            let rows = at.clone_range()?;
            rows.move_endpoint_by_unit(
                Endpoint::End,
                TextUnit_Line,
                i32::try_from(opening).unwrap_or(i32::MAX),
            )?;
            Some(rows.text(-1)?)
        } else {
            None
        };
        Ok(MarkRows::Rows {
            written: rows_of(written),
            opening,
        })
    };
    read().unwrap_or(MarkRows::Refused)
}

/// The window read the classic way, the fallback and the reference: the
/// same steps as the remote program, each a cross-process call.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails.
pub fn terminal_read_classic(_uia: &Uia, query: &ReadQuery<'_>) -> Result<Window, Error> {
    let document = query.pattern.document_range()?;
    let screen = query
        .pattern
        .visible_ranges()?
        .into_iter()
        .next()
        .unwrap_or_else(|| document.clone());
    let screen_text = screen.text(-1)?;
    let screen_at_start =
        document.compare_endpoints(Endpoint::Start, &screen, Endpoint::Start)? == 0;
    let top = collapsed(&screen, Endpoint::Start)?;
    let top_row = top.clone_range()?;
    top_row.expand(TextUnit_Line)?;
    let top_row = top_row.text(-1)?;
    let screen_top = top.clone_range()?.move_by(TextUnit_Line, FAR)?;
    let at_end = collapsed(&document, Endpoint::End)?;
    let (window, moved) = if query.height > 0 {
        let window = at_end.clone_range()?;
        let up = -2 * i32::try_from(query.height).unwrap_or(i32::MAX / 2);
        let moved = window.move_by(TextUnit_Line, up)?;
        (window, moved)
    } else {
        (top.clone_range()?, -screen_top)
    };
    window.move_endpoint_to(Endpoint::End, &document, Endpoint::End)?;
    let rows = window.text(-1)?;
    let above = rows_of(moved);
    let first = collapsed(&document, Endpoint::Start)?;
    first.expand(TextUnit_Line)?;
    let first_row = first.text(-1)?;
    let document_rows = at_end.clone_range()?.move_by(TextUnit_Line, -FAR)?;
    let marks = query
        .marks
        .map(|mark| mark.map(|mark| mark_classic(mark, &document, (above, query.opening))));
    let caret_rows_to_end = match query.pattern.selection()?.into_iter().next() {
        Some(caret) => Some(rows_of(
            collapsed(&caret, Endpoint::Start)?.move_by(TextUnit_Line, FAR)?,
        )),
        None => None,
    };
    let caret = caret_line_classic(&query.caret)?;
    Ok(Window {
        rows,
        above,
        screen: screen_text,
        top_row,
        screen_top: rows_of(screen_top),
        top: Some(top),
        screen_at_start,
        end: Some(at_end),
        first_row,
        document_rows: rows_of(document_rows),
        marks,
        caret: Some(caret),
        caret_rows_to_end,
    })
}

/// The search for an end mark's line, only as classic calls (the module's
/// documentation): the nearest row starting with `line` at or above the
/// document's end, at most [`crate::FIND_CALLS`] `FindText` calls, as its
/// distance in rows from the document's end; `None` when none is found.
///
/// # Errors
///
/// [`Error::Uia`] when a call before the search fails.
pub fn terminal_read_find(
    uia: &Uia,
    (element, pattern): (&IUIAutomationElement, &IUIAutomationTextPattern),
    line: &str,
) -> Result<Option<u32>, Error> {
    let document = pattern.document_range()?;
    let at_end = collapsed(&document, Endpoint::End)?;
    let query = FindQuery {
        element,
        pattern,
        start: &at_end,
        needle: line,
        from: 0,
        rows: 1,
    };
    let found: Vec<FindMatch> = classic_search(uia, &query, 1)?;
    Ok(found
        .first()
        .map(|found| u32::try_from(found.above).unwrap_or(0)))
}

/// The colours of the screen's rows `first` and on, `count` of them, each
/// `width` cells, from `top`, the screen's top row as the window read
/// found it ([`Window::top`]): the remote programs when `remote` is true,
/// falling back to the classic reads for this call, and the classic reads
/// alone otherwise. The rows stop at the document's end.
///
/// # Errors
///
/// The classic reads' [`Error`], when they ran and failed.
pub fn terminal_row_colours(
    top: &IUIAutomationTextRange,
    (first, count, width): (u32, u32, usize),
    remote: bool,
) -> Result<(Vec<Vec<ColourRun>>, Path), Error> {
    if width == 0 || count == 0 {
        return Ok((
            Vec::new(),
            if remote { Path::Remote } else { Path::Classic },
        ));
    }
    if remote {
        match row_colours_remote(top, (first, count, width)) {
            Ok(rows) => return Ok((rows, Path::Remote)),
            Err(error) => {
                return row_colours_classic(top, (first, count, width))
                    .map(|rows| (rows, Path::Fallback(error)));
            }
        }
    }
    row_colours_classic(top, (first, count, width)).map(|rows| (rows, Path::Classic))
}

/// Where the rows `first` to `first + count` start and stop below `top`,
/// the classic way, the stop no later than the document's end.
fn row_bounds_classic(
    top: &IUIAutomationTextRange,
    (first, count): (u32, u32),
) -> Result<(IUIAutomationTextRange, IUIAutomationTextRange), Error> {
    let row = top.clone_range()?;
    row.move_by(TextUnit_Line, i32::try_from(first).unwrap_or(i32::MAX))?;
    let row = collapsed(&row, Endpoint::Start)?;
    let limit = row.clone_range()?;
    limit.move_by(TextUnit_Line, i32::try_from(count).unwrap_or(i32::MAX))?;
    Ok((row, collapsed(&limit, Endpoint::Start)?))
}

/// The rows' colours the classic way.
fn row_colours_classic(
    top: &IUIAutomationTextRange,
    (first, count, width): (u32, u32, usize),
) -> Result<Vec<Vec<ColourRun>>, Error> {
    let (row, limit) = row_bounds_classic(top, (first, count))?;
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
    /// The colour programs for rows below the screen's top, one for each
    /// width, first row and count.
    static ROW_COLOURS: ProgramCache<
        (usize, u32, u32),
        (crate::terminal_colours::ColourRegisters, Reg<kind::TextRange>),
    > = ProgramCache::default();
}

/// The rows' colours in programs: the first starts at row `first`, and
/// each later one goes on from where the last stopped.
fn row_colours_remote(
    top: &IUIAutomationTextRange,
    (first, count, width): (u32, u32, usize),
) -> Result<Vec<Vec<ColourRun>>, Error> {
    let (operation, registers) = ROW_COLOURS.with(|programs| {
        programs.operation(
            (width, first, count),
            || vec![Import::TextRange(top.clone())],
            |b| {
                let top = b.import_text_range(top);
                let line = b.int(TextUnit_Line.0);
                let (start, end) = (b.int(0), b.int(1));
                let row = b.text_range_clone(top);
                let down = b.int(i32::try_from(first).unwrap_or(i32::MAX));
                let _ = b.text_range_move(row, line, down);
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
