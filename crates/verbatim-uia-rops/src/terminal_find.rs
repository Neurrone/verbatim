//! A command mark's line sought in a terminal's history by its text
//! (`docs/design/terminal-model.md`, "Features and settings", moving by
//! command): `FindText` for the line's first row, outward from the mark's
//! own row, counted from where the next read starts: alternately the next
//! match at or below that row and the next above it, the row itself first,
//! so that the mark's own line is the first match whenever it is still
//! there. Each match that starts a row is answered with how many rows above
//! that start it lies and the text of as many rows from it as the mark's
//! line holds, for the caller to verify by its line's hash, since the
//! console host's `FindText` also matches a longer row with the same start
//! ("flood line 14" in "flood line 140"). At most [`FIND_MATCHES`] matches
//! that start a row are taken, the bound the console host's search needed,
//! over at most [`FIND_CALLS`] `FindText` calls, so that a needle found
//! mostly within rows cannot walk the whole history (`docs/performance.md`,
//! "A terminal's upward search").
//!
//! A `FindText` that fails, or whose range's calls fail, as Windows
//! Terminal's not-found range does (a range rather than null, whose
//! `GetText`, `CompareEndpoints` and `MoveEndpointByRange` fail with
//! `E_FAIL` or `E_INVALIDARG`; seen in Windows Terminal 1.24.12741 on 2026-10-24), means not
//! found in that direction, never an error: the search goes on in the
//! other direction only. A remote program does it in one round trip and a
//! classic implementation call by call; [`terminal_find`] is the one
//! function callers use.

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

/// How many matches that start a row a search takes at most
/// (`docs/performance.md`, "A terminal's upward search").
pub const FIND_MATCHES: usize = 20;

/// How many `FindText` calls a search makes at most, counting the hits
/// within a row it passes over: five for each match it takes, so a needle
/// with up to four hits within rows for each row it starts still gets all
/// [`FIND_MATCHES`], while one found mostly within rows, such as a short
/// input, ends after about 300 milliseconds at the estimated 3 a call
/// remotely rather than walking the whole history (`docs/performance.md`,
/// "A terminal's upward search").
pub const FIND_CALLS: usize = 5 * FIND_MATCHES;

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
    /// (negative below it): it works outward from that row, alternately
    /// the next match at or below it and the next above it, the row's own
    /// match first.
    pub from: i32,
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

/// The matches of `query` that start a row, outward from its row,
/// alternately the next at or below it and the next above it, at most
/// [`FIND_MATCHES`] of them over at most [`FIND_CALLS`] `FindText` calls:
/// the remote program when `remote` is true, falling back to the classic
/// implementation for this call when the program fails, and the classic
/// implementation alone when `remote` is false. Says which path answered.
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

/// One direction of the search program: the span it searches, whether
/// up, toward the document's start, and whether it may still find a match.
#[derive(Clone, Copy)]
struct Direction {
    span: Reg<kind::TextRange>,
    up: bool,
    backward: Reg<kind::Bool>,
    open: Reg<kind::Bool>,
}

/// The registers each `FindText` call of the search program uses.
#[derive(Clone, Copy)]
struct Search {
    start_end: Reg<kind::Int>,
    end_end: Reg<kind::Int>,
    line: Reg<kind::Int>,
    all: Reg<kind::Int>,
    zero: Reg<kind::Int>,
    one: Reg<kind::Int>,
    far: Reg<kind::Int>,
    closed: Reg<kind::Bool>,
    needle: Reg<kind::Str>,
    ignore_case: Reg<kind::Bool>,
    rows: Reg<kind::Int>,
    start_to_end: Reg<kind::Int>,
    aboves: Reg<kind::Array>,
    texts: Reg<kind::Array>,
    taken: Reg<kind::Int>,
    going_down: Reg<kind::Bool>,
}

/// A copy of `range` collapsed to its start.
fn collapsed(
    b: &mut Builder,
    range: Reg<kind::TextRange>,
    (start_end, end_end): (Reg<kind::Int>, Reg<kind::Int>),
) -> Reg<kind::TextRange> {
    let copy = b.text_range_clone(range);
    b.text_range_move_endpoint_by_range(copy, end_end, copy, start_end);
    copy
}

/// One `FindText` call in `way`: a match that starts a row is taken and
/// the other direction searched next; a hit within a row is passed over;
/// none, or a failure, closes this direction and turns to the other.
fn find_once(b: &mut Builder, s: Search, way: Direction) {
    b.try_catch(
        |b| {
            let hit = b.text_range_find_text(way.span, s.needle, way.backward, s.ignore_case);
            let missing = b.is_null(hit);
            b.if_else(
                missing,
                |b| {
                    b.set(way.open, s.closed);
                    b.not_assign(s.going_down);
                },
                |b| {
                    let row = b.text_range_clone(hit);
                    b.text_range_expand_to_enclosing_unit(row, s.line);
                    let order = b.text_range_compare_endpoints(row, s.start_end, hit, s.start_end);
                    // The next search in this direction, past this hit.
                    if way.up {
                        b.text_range_move_endpoint_by_range(way.span, s.end_end, hit, s.start_end);
                    } else {
                        b.text_range_move_endpoint_by_range(way.span, s.start_end, hit, s.end_end);
                    }
                    let starts = b.equal(order, s.zero);
                    b.if_(starts, |b| {
                        let from = collapsed(b, row, (s.start_end, s.end_end));
                        let walker = b.text_range_clone(from);
                        let to_end = b.text_range_move(walker, s.line, s.far);
                        let above = b.subtract(to_end, s.start_to_end);
                        let _ = b.text_range_move_endpoint_by_unit(from, s.end_end, s.line, s.rows);
                        let text = b.text_range_get_text(from, s.all);
                        b.array_append(s.aboves, above);
                        b.array_append(s.texts, text);
                        b.add_assign(s.taken, s.one);
                        b.not_assign(s.going_down);
                    });
                },
            );
        },
        // Not found in this direction, Windows Terminal's not-found range
        // included: the other goes on.
        |b, _| {
            b.set(way.open, s.closed);
            b.not_assign(s.going_down);
        },
    );
}

/// The search program: two arrays, each match's rows above the start and
/// its text.
fn build_find(b: &mut Builder, query: &FindQuery<'_>) -> (Reg<kind::Array>, Reg<kind::Array>) {
    let element = b.import_element(query.element);
    let kept = b.import_text_range(query.start);
    let start_end = b.int(0);
    let end_end = b.int(1);
    let line = b.int(TextUnit_Line.0);
    let far = b.int(FAR);
    let pattern = b.get_text_pattern(element, false);
    let document = b.text_pattern_get_document_range(pattern);
    let start = collapsed(b, kept, (start_end, end_end));
    let boundary = b.text_range_clone(start);
    let back = b.int(-query.from);
    let _ = b.text_range_move(boundary, line, back);
    let below = b.text_range_clone(document);
    b.text_range_move_endpoint_by_range(below, start_end, boundary, start_end);
    let above = b.text_range_clone(document);
    b.text_range_move_endpoint_by_range(above, end_end, boundary, start_end);
    let walker = b.text_range_clone(start);
    let start_to_end = b.text_range_move(walker, line, far);
    let aboves = b.new_array();
    let texts = b.new_array();
    let s = Search {
        start_end,
        end_end,
        line,
        all: b.int(-1),
        zero: b.int(0),
        one: b.int(1),
        far,
        closed: b.bool(false),
        needle: b.string(query.needle),
        ignore_case: b.bool(false),
        rows: b.int(i32::try_from(query.rows.max(1)).unwrap_or(i32::MAX)),
        start_to_end,
        aboves: b.add_to_results(aboves),
        texts: b.add_to_results(texts),
        taken: b.new_int(0),
        going_down: b.new_bool(true),
    };
    let (aboves, texts, taken, going_down, one) = (s.aboves, s.texts, s.taken, s.going_down, s.one);
    let calls = b.new_int(0);
    let most_calls = b.int(i32::try_from(FIND_CALLS).unwrap_or(i32::MAX));
    let most = b.int(i32::try_from(FIND_MATCHES).unwrap_or(i32::MAX));
    let down = Direction {
        span: below,
        up: false,
        backward: b.bool(false),
        open: b.new_bool(true),
    };
    let up = Direction {
        span: above,
        up: true,
        backward: b.bool(true),
        open: b.new_bool(true),
    };
    b.while_(
        |b| {
            let more = b.compare(taken, most, Comparison::LessThan);
            let calls_left = b.compare(calls, most_calls, Comparison::LessThan);
            let open = b.or(down.open, up.open);
            let going = b.and(more, calls_left);
            b.and(going, open)
        },
        |b| {
            // A closed direction gives way to the other, which the loop's
            // condition says is open.
            let down_closed = b.not(down.open);
            let up_closed = b.not(up.open);
            let going_up = b.not(going_down);
            let stuck_down = b.and(going_down, down_closed);
            let stuck_up = b.and(going_up, up_closed);
            let stuck = b.or(stuck_down, stuck_up);
            b.if_(stuck, |b| b.not_assign(going_down));
            b.add_assign(calls, one);
            b.if_else(
                going_down,
                |b| find_once(b, s, down),
                |b| find_once(b, s, up),
            );
        },
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
    let below = document.clone_range()?;
    below.move_endpoint_to(Endpoint::Start, &boundary, Endpoint::Start)?;
    let above = document.clone_range()?;
    above.move_endpoint_to(Endpoint::End, &boundary, Endpoint::Start)?;
    let start_to_end = start.clone_range()?.move_by(TextUnit_Line, FAR)?;
    let rows = i32::try_from(query.rows.max(1)).unwrap_or(i32::MAX);
    let mut found = Vec::new();
    // One call in a direction: `None` when it has no more matches.
    let next = |span: &IUIAutomationTextRange,
                backward: bool|
     -> Result<Option<Option<FindMatch>>, Error> {
        let Some(hit) = span.find_text(query.needle, backward)? else {
            return Ok(None);
        };
        let row = hit.clone_range()?;
        row.expand(TextUnit_Line)?;
        let order = row.compare_endpoints(Endpoint::Start, &hit, Endpoint::Start)?;
        if backward {
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
    let mut open = [true, true];
    let mut going_down = true;
    for _ in 0..FIND_CALLS {
        if found.len() >= FIND_MATCHES || open == [false, false] {
            break;
        }
        if !open[usize::from(!going_down)] {
            going_down = !going_down;
        }
        let (span, way) = if going_down { (&below, 0) } else { (&above, 1) };
        // A failure is not found in this direction: the other goes on.
        match next(span, !going_down) {
            Ok(Some(Some(found_one))) => {
                found.push(found_one);
                going_down = !going_down;
            }
            Ok(Some(None)) => {}
            Ok(None) | Err(_) => {
                open[way] = false;
                going_down = !going_down;
            }
        }
    }
    Ok(found)
}
