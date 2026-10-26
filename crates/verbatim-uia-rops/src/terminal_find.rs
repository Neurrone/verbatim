//! A command mark's line sought in a terminal's history by its text
//! (`docs/design/terminal-model.md`, "Features and settings", moving by
//! command): `FindText` for the line's first row, from a boundary row
//! counted from where the next read starts, backward to the document's
//! start or forward to its end, nearest first. Each match that starts a row
//! is answered with how many rows above that start it lies and the text of
//! as many rows from it as the mark's line holds, for the caller to verify
//! by its line's hash, since the console host's `FindText` also matches a
//! longer row with the same start ("flood line 14" in "flood line 140"). At
//! most [`FIND_MATCHES`] matches are taken, the bound the console host's
//! search needed (`docs/performance.md`, "A terminal's upward search").
//!
//! A `FindText` that fails, or whose range's calls fail, as Windows
//! Terminal's not-found range does (a range rather than null, whose
//! `GetText`, `CompareEndpoints` and `MoveEndpointByRange` fail with
//! `E_FAIL` or `E_INVALIDARG`; `docs/roadmap.md`, 2026-10-24), ends the
//! search with what it found: not found, never an error. A remote program
//! does it in one round trip and a classic implementation call by call;
//! [`terminal_find`] is the one function callers use.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit_Line,
};

use verbatim_uia::Uia;
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::error::Error;
use crate::focus::Path;
use crate::opcode::Comparison;
use crate::operation::Value;

/// How many matches a search takes at most (`docs/performance.md`, "A
/// terminal's upward search").
pub const FIND_MATCHES: usize = 20;

/// More rows than any terminal holds, for counting rows to the end.
const FAR: i32 = 1_000_000;

/// What [`terminal_find`] is asked.
#[derive(Clone, Copy)]
pub struct FindQuery<'a> {
    /// The element with the text.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern.
    pub pattern: &'a IUIAutomationTextPattern,
    /// Where the next read starts, from which rows are counted.
    pub start: &'a IUIAutomationTextRange,
    /// The text sought: a row's text, with no line break.
    pub needle: &'a str,
    /// The row the search starts at, counted in rows above `start`
    /// (negative below it): backward, the search covers the rows above it;
    /// forward, that row and the rows below it.
    pub from: i32,
    /// Whether to search backward, toward the document's start.
    pub backward: bool,
    /// How many rows of text to read from each match's row.
    pub rows: u32,
}

/// One match: its row, counted in rows above where the next read starts
/// (negative below it), and the text of the rows read from it, padding and
/// line breaks as the provider gave them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindMatch {
    /// Rows above the start.
    pub above: i32,
    /// The text from the match's row.
    pub text: String,
}

/// The matches of `query`, nearest first in the search's direction, at most
/// [`FIND_MATCHES`]: the remote program when `remote` is true, falling back
/// to the classic implementation for this call when the program fails, and
/// the classic implementation alone when `remote` is false. Says which path
/// answered.
///
/// # Errors
///
/// The classic implementation's [`Error`], when a call before the search
/// fails.
pub fn terminal_find(
    uia: &Uia,
    query: &FindQuery<'_>,
    remote: bool,
) -> Result<(Vec<FindMatch>, Path), Error> {
    if !remote {
        return terminal_find_classic(uia, query).map(|found| (found, Path::Classic));
    }
    match terminal_find_remote(uia, query) {
        Ok(found) => Ok((found, Path::Remote)),
        Err(error) => terminal_find_classic(uia, query).map(|found| (found, Path::Fallback(error))),
    }
}

/// The search in one cross-process round trip. Built for each search, as
/// the text sought is part of the program and a search follows only a key
/// press.
///
/// `uia` is unused; it is in the signature so that this and
/// [`terminal_find_classic`] are interchangeable.
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn terminal_find_remote(_uia: &Uia, query: &FindQuery<'_>) -> Result<Vec<FindMatch>, Error> {
    let mut b = Builder::new();
    let (aboves, texts) = build_find(&mut b, query);
    let outcome = b.finish().execute()?;
    let aboves = outcome.get(aboves)?;
    let texts = outcome.get(texts)?;
    Ok(aboves
        .into_iter()
        .zip(texts)
        .filter_map(|(above, text)| match (above, text) {
            (Value::Int(above), Value::String(text)) => Some(FindMatch { above, text }),
            _ => None,
        })
        .collect())
}

/// The search program: two arrays, each match's rows above the start and
/// its text.
fn build_find(b: &mut Builder, query: &FindQuery<'_>) -> (Reg<kind::Array>, Reg<kind::Array>) {
    let element = b.import_element(query.element);
    let kept = b.import_text_range(query.start);
    let start_end = b.int(0);
    let end_end = b.int(1);
    let line = b.int(TextUnit_Line.0);
    let all = b.int(-1);
    let zero = b.int(0);
    let one = b.int(1);
    let far = b.int(FAR);
    let collapsed = |b: &mut Builder, range: Reg<kind::TextRange>| {
        let copy = b.text_range_clone(range);
        b.text_range_move_endpoint_by_range(copy, end_end, copy, start_end);
        copy
    };
    let pattern = b.get_text_pattern(element, false);
    let document = b.text_pattern_get_document_range(pattern);
    let start = collapsed(b, kept);
    let boundary = b.text_range_clone(start);
    let back = b.int(-query.from);
    let _ = b.text_range_move(boundary, line, back);
    let span = b.text_range_clone(document);
    if query.backward {
        b.text_range_move_endpoint_by_range(span, end_end, boundary, start_end);
    } else {
        b.text_range_move_endpoint_by_range(span, start_end, boundary, start_end);
    }
    let walker = b.text_range_clone(start);
    let start_to_end = b.text_range_move(walker, line, far);
    let needle = b.string(query.needle);
    let backward = b.bool(query.backward);
    let ignore_case = b.bool(false);
    let rows = b.int(i32::try_from(query.rows.max(1)).unwrap_or(i32::MAX));
    let aboves = b.new_array();
    let aboves = b.add_to_results(aboves);
    let texts = b.new_array();
    let texts = b.add_to_results(texts);
    let tries = b.new_int(0);
    let most = b.int(i32::try_from(FIND_MATCHES).unwrap_or(i32::MAX));
    // A failure is not found: the search ends with what it has.
    b.try_catch(
        |b| {
            b.while_(
                |b| b.compare(tries, most, Comparison::LessThan),
                |b| {
                    b.add_assign(tries, one);
                    let hit = b.text_range_find_text(span, needle, backward, ignore_case);
                    let missing = b.is_null(hit);
                    b.if_(missing, Builder::break_loop);
                    let row = b.text_range_clone(hit);
                    b.text_range_expand_to_enclosing_unit(row, line);
                    let order = b.text_range_compare_endpoints(row, start_end, hit, start_end);
                    // The next search, past this match.
                    if query.backward {
                        b.text_range_move_endpoint_by_range(span, end_end, hit, start_end);
                    } else {
                        b.text_range_move_endpoint_by_range(span, start_end, hit, end_end);
                    }
                    let starts = b.equal(order, zero);
                    b.if_(starts, |b| {
                        let from = collapsed(b, row);
                        let walker = b.text_range_clone(from);
                        let to_end = b.text_range_move(walker, line, far);
                        let above = b.subtract(to_end, start_to_end);
                        let _ = b.text_range_move_endpoint_by_unit(from, end_end, line, rows);
                        let text = b.text_range_get_text(from, all);
                        b.array_append(aboves, above);
                        b.array_append(texts, text);
                    });
                },
            );
        },
        |_, _| {},
    );
    (aboves, texts)
}

/// The search the classic way, the fallback and the reference: the same
/// steps as the remote program, each a cross-process call.
///
/// # Errors
///
/// [`Error::Uia`] when a call before the search fails.
pub fn terminal_find_classic(_uia: &Uia, query: &FindQuery<'_>) -> Result<Vec<FindMatch>, Error> {
    let document = query.pattern.document_range()?;
    let start = query.start.clone_range()?;
    start.move_endpoint_to(Endpoint::End, &start, Endpoint::Start)?;
    let boundary = start.clone_range()?;
    boundary.move_by(TextUnit_Line, -query.from)?;
    let span = document.clone_range()?;
    if query.backward {
        span.move_endpoint_to(Endpoint::End, &boundary, Endpoint::Start)?;
    } else {
        span.move_endpoint_to(Endpoint::Start, &boundary, Endpoint::Start)?;
    }
    let start_to_end = start.clone_range()?.move_by(TextUnit_Line, FAR)?;
    let rows = i32::try_from(query.rows.max(1)).unwrap_or(i32::MAX);
    let mut found = Vec::new();
    // One match: `None` when there are no more.
    let next = || -> Result<Option<Option<FindMatch>>, Error> {
        let Some(hit) = span.find_text(query.needle, query.backward)? else {
            return Ok(None);
        };
        let row = hit.clone_range()?;
        row.expand(TextUnit_Line)?;
        let order = row.compare_endpoints(Endpoint::Start, &hit, Endpoint::Start)?;
        if query.backward {
            span.move_endpoint_to(Endpoint::End, &hit, Endpoint::Start)?;
        } else {
            span.move_endpoint_to(Endpoint::Start, &hit, Endpoint::End)?;
        }
        if order != 0 {
            return Ok(Some(None));
        }
        row.move_endpoint_to(Endpoint::End, &row, Endpoint::Start)?;
        let to_end = row.clone_range()?.move_by(TextUnit_Line, FAR)?;
        row.move_endpoint_by_unit(Endpoint::End, TextUnit_Line, rows)?;
        Ok(Some(Some(FindMatch {
            above: to_end - start_to_end,
            text: String::from_utf16_lossy(&row.text(-1)?),
        })))
    };
    for _ in 0..FIND_MATCHES {
        // A failure is not found: the search ends with what it has.
        match next() {
            Ok(Some(Some(found_one))) => found.push(found_one),
            Ok(Some(None)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    Ok(found)
}
