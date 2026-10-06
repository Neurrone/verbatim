//! The tail of a terminal's text (milestone M4 item 9; `phase6-design.md`,
//! "How the outpost finds new lines"): the line at an anchor and the line
//! before it, checked against what they held when last read; where that
//! pair is now, searching upward when the text has scrolled beneath the
//! anchor; how many lines follow it to the end of the text; and the text
//! of only the last of those lines. A remote program does it in one round
//! trip, and a classic implementation behind the same signature does it
//! call by call; [`terminal_tail`] is the one function callers use.
//!
//! Lines are found by the provider's line unit, never by counting line
//! breaks in text. A range is not trusted on its own: once a terminal's
//! scrollback is full, a range keeps its row while the text moves up
//! beneath it, so the anchor's line is always compared with the text it
//! held. The text is returned as the provider gives it, padding and line
//! breaks included, so the comparison is exact; the caller trims it.
//!
//! Providers differ in where moving forward by lines stops: on the start
//! of the last line (mockapp) or at the end of the text after it (the
//! terminals). Both read the same here, since the last line is found as
//! the line containing the text's last character and the count is checked
//! against it.

use windows::Win32::UI::Accessibility::{
    IUIAutomationTextRange, TextUnit_Character, TextUnit_Document, TextUnit_Line,
};

use verbatim_uia::Uia;
use verbatim_uia::text::{Endpoint, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::error::Error;
use crate::focus::Path;
use crate::opcode::Comparison;
use crate::operation::{Outcome, Value};

/// How many lines above the anchor are searched for its fingerprint when
/// the text moved beneath it: more than a busy terminal usually writes
/// between two reads.
pub const SEARCH_LINES: u32 = 256;

/// More lines than any terminal holds, for moving to the end of the text.
const FAR: i32 = 1_000_000;

/// The text the anchor's line and the line before it held when last read,
/// as the provider gave it.
#[derive(Clone, Copy, Debug)]
pub struct Fingerprint<'a> {
    /// The anchor's line.
    pub line: &'a str,
    /// The line before it, empty when it was the first.
    pub previous: &'a str,
}

impl Fingerprint<'_> {
    /// The forms the anchor's line may take now: as read, and, read as the
    /// last line of the text with no line break, with the line feed or
    /// carriage return and line feed it gains once more text follows it.
    fn line_forms(&self) -> [String; 3] {
        let line = self.line.trim_end_matches(['\r', '\n']);
        [
            self.line.to_owned(),
            format!("{line}\n"),
            format!("{line}\r\n"),
        ]
    }
}

/// Where a [`TailQuery`] starts.
#[derive(Clone, Copy)]
pub enum TailStart<'a> {
    /// From an anchor kept from the last read: a range whose start is the
    /// start of the last line read ([`Tail::last`]), with that line's
    /// fingerprint.
    Anchor {
        /// The anchor.
        range: &'a IUIAutomationTextRange,
        /// What its line and the line before it held.
        fingerprint: Fingerprint<'a>,
    },
    /// Afresh, with no anchor: the end of the text of this range's
    /// document (the text pattern's document range).
    Document(&'a IUIAutomationTextRange),
}

/// What [`terminal_tail`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct TailQuery<'a> {
    /// Where to start.
    pub start: TailStart<'a>,
    /// The most lines to read from the end of the text.
    pub lines_wanted: u32,
    /// How many lines above the anchor to search for its fingerprint.
    pub search_lines: u32,
}

/// Where the anchor's fingerprint was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// The line before the anchor still holds what it held: the anchor is
    /// where it was, and its own line may have changed in place.
    AtAnchor,
    /// Both lines were found this many lines above the anchor: the text
    /// scrolled up beneath it.
    Moved(u32),
    /// Neither: the text under the anchor changed (the screen was cleared,
    /// a full-screen program switched screens, or more scrolled by than
    /// the search covers). Counted from the anchor nonetheless.
    NotFound,
    /// There was no anchor ([`TailStart::Document`]).
    Afresh,
}

/// The answer to a [`TailQuery`]. Text is as the provider gave it.
#[derive(Debug)]
pub struct Tail {
    /// Where the fingerprint was found.
    pub found: Found,
    /// The text of the line at the anchor now; empty when there was no
    /// anchor. Where the fingerprint was found above the anchor, the line
    /// there holds the fingerprint's line exactly.
    pub line: String,
    /// The text of the line before the anchor, read at the anchor; empty at
    /// the top of the text or with no anchor.
    pub previous: String,
    /// How many lines follow the anchor's line to the end of the text; with
    /// no anchor, how many lines the text has.
    pub count: u32,
    /// The text of the last lines, oldest first: as many as `count` up to
    /// the query's `lines_wanted`.
    pub lines: Vec<String>,
    /// The text of the line just above the first of `lines`, empty when
    /// there is none or no line was read.
    pub above: String,
    /// The last line's range, whose start is the next read's anchor.
    pub last: IUIAutomationTextRange,
}

/// The signature both implementations share.
pub type TerminalTailFn = fn(&Uia, &TailQuery<'_>) -> Result<Tail, Error>;

/// The tail of a terminal's text, the one function call sites use: the
/// remote program when `remote` is true, falling back to the classic
/// implementation for this call when the program fails, and the classic
/// implementation alone when `remote` is false. Says which path answered,
/// as [`crate::focus_ancestry`] does.
///
/// # Errors
///
/// The classic implementation's [`Error`], when it ran and failed: a range
/// that can no longer be compared with the text (a terminal that switched
/// to or from its alternate screen) fails this way, and the caller then
/// starts afresh from the document.
pub fn terminal_tail(
    uia: &Uia,
    query: &TailQuery<'_>,
    remote: bool,
) -> Result<(Tail, Path), Error> {
    if !remote {
        return terminal_tail_classic(uia, query).map(|tail| (tail, Path::Classic));
    }
    match terminal_tail_remote(uia, query) {
        Ok(tail) => Ok((tail, Path::Remote)),
        Err(error) => terminal_tail_classic(uia, query).map(|tail| (tail, Path::Fallback(error))),
    }
}

/// A UIA endpoint's number in a program.
fn endpoint_number(endpoint: Endpoint) -> i32 {
    match endpoint {
        Endpoint::Start => 0,
        Endpoint::End => 1,
    }
}

/// The program's constants: endpoints, units, and small numbers.
struct Constants {
    start: Reg<kind::Int>,
    end: Reg<kind::Int>,
    character: Reg<kind::Int>,
    line: Reg<kind::Int>,
    document: Reg<kind::Int>,
    all: Reg<kind::Int>,
    zero: Reg<kind::Int>,
    one: Reg<kind::Int>,
    back: Reg<kind::Int>,
    far: Reg<kind::Int>,
}

impl Constants {
    fn new(b: &mut Builder) -> Self {
        Self {
            start: b.int(endpoint_number(Endpoint::Start)),
            end: b.int(endpoint_number(Endpoint::End)),
            character: b.int(TextUnit_Character.0),
            line: b.int(TextUnit_Line.0),
            document: b.int(TextUnit_Document.0),
            all: b.int(-1),
            zero: b.int(0),
            one: b.int(1),
            back: b.int(-1),
            far: b.int(FAR),
        }
    }

    /// Emits a copy of `range` collapsed to its start.
    fn collapsed(&self, b: &mut Builder, range: Reg<kind::TextRange>) -> Reg<kind::TextRange> {
        let copy = b.text_range_clone(range);
        b.text_range_move_endpoint_by_range(copy, self.end, copy, self.start);
        copy
    }

    /// Emits the text of the line containing `range`'s start.
    fn line_text(&self, b: &mut Builder, range: Reg<kind::TextRange>) -> Reg<kind::Str> {
        let copy = self.collapsed(b, range);
        b.text_range_expand_to_enclosing_unit(copy, self.line);
        b.text_range_get_text(copy, self.all)
    }
}

/// Emits the end of the program shared by both starts, from the line
/// `from`: the last line, the count of lines after `from`'s line to it
/// (`count`), and the last lines' text.
fn emit_tail(
    b: &mut Builder,
    c: &Constants,
    from: Reg<kind::TextRange>,
    count_lines_from: bool,
    wanted: u32,
) -> TailRegisters {
    // The last line: the one holding the text's last character.
    let document = b.text_range_clone(from);
    b.text_range_expand_to_enclosing_unit(document, c.document);
    let last = b.text_range_clone(document);
    b.text_range_move_endpoint_by_range(last, c.start, document, c.end);
    let _ = b.text_range_move_endpoint_by_unit(last, c.start, c.character, c.back);
    b.text_range_expand_to_enclosing_unit(last, c.line);
    let last = b.add_to_results(last);

    // Lines from `from`'s line to the last: moving on by lines lands on the
    // last line's start, or past it at the text's end.
    let walker = c.collapsed(b, if count_lines_from { from } else { document });
    let moved = b.text_range_move(walker, c.line, c.far);
    let count = b.new_int(0);
    b.set(count, moved);
    let order = b.text_range_compare_endpoints(walker, c.start, last, c.start);
    let past = b.compare(order, c.zero, Comparison::GreaterThan);
    b.if_(past, |b| b.subtract_assign(count, c.one));
    if !count_lines_from {
        // With no anchor, the count is of every line, the first included.
        b.add_assign(count, c.one);
    }
    let count = b.add_to_results(count);

    // The last lines' text, newest first, and the line above them.
    let wanted = b.int(i32::try_from(wanted).unwrap_or(i32::MAX));
    let reading = b.new_int(0);
    b.set(reading, count);
    let more = b.compare(reading, wanted, Comparison::GreaterThan);
    b.if_(more, |b| b.set(reading, wanted));
    let lines = b.new_array();
    let lines = b.add_to_results(lines);
    let above = b.new_string("");
    let above = b.add_to_results(above);
    let row = b.text_range_clone(last);
    let read = b.new_int(0);
    let at_top = b.new_bool(false);
    b.while_(
        |b| b.compare(read, reading, Comparison::LessThan),
        |b| {
            let text = b.text_range_get_text(row, c.all);
            b.array_append(lines, text);
            b.add_assign(read, c.one);
            let up = b.text_range_move(row, c.line, c.back);
            let top = b.equal(up, c.zero);
            b.if_(top, |b| {
                let yes = b.bool(true);
                b.set(at_top, yes);
                b.break_loop();
            });
            b.text_range_expand_to_enclosing_unit(row, c.line);
        },
    );
    let some = b.compare(read, c.zero, Comparison::GreaterThan);
    let below_top = b.not(at_top);
    let wanted_above = b.and(some, below_top);
    b.if_(wanted_above, |b| {
        let text = b.text_range_get_text(row, c.all);
        b.set(above, text);
    });
    TailRegisters {
        last,
        count,
        lines,
        above,
    }
}

/// The registers [`emit_tail`] asks for.
struct TailRegisters {
    last: Reg<kind::TextRange>,
    count: Reg<kind::Int>,
    lines: Reg<kind::Array>,
    above: Reg<kind::Str>,
}

/// The tail in one cross-process round trip: a program that reads the
/// anchor's line and the line before it, searches upward for the
/// fingerprint when the line before it changed, counts the lines from
/// where it was found to the end of the text, and reads the last of them.
///
/// `uia` is unused; it is in the signature so that this and
/// [`terminal_tail_classic`] are interchangeable ([`TerminalTailFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn terminal_tail_remote(_uia: &Uia, query: &TailQuery<'_>) -> Result<Tail, Error> {
    let mut b = Builder::new();
    let c = Constants::new(&mut b);
    match query.start {
        TailStart::Document(range) => {
            let document = b.import_text_range(range);
            let tail = emit_tail(&mut b, &c, document, false, query.lines_wanted);
            let outcome = b.finish().execute()?;
            Ok(Tail {
                found: Found::Afresh,
                line: String::new(),
                previous: String::new(),
                count: count_of(outcome.get(tail.count)?),
                lines: texts(outcome.get(tail.lines)?),
                above: string_of(&outcome, tail.above)?,
                last: outcome
                    .get(tail.last)?
                    .ok_or(Error::MissingResult(tail.last.id()))?,
            })
        }
        TailStart::Anchor { range, fingerprint } => {
            let anchor = b.import_text_range(range);
            let at = c.collapsed(&mut b, anchor);
            let line = c.line_text(&mut b, at);
            let line = b.add_to_results(line);

            let previous = b.new_string("");
            let previous = b.add_to_results(previous);
            let above = c.collapsed(&mut b, at);
            let moved = b.text_range_move(above, c.line, c.back);
            let has_previous = b.not_equal(moved, c.zero);
            b.if_(has_previous, |b| {
                let text = c.line_text(b, above);
                b.set(previous, text);
            });

            let found = b.new_int(-1);
            let found = b.add_to_results(found);
            let position = b.text_range_clone(at);
            let fingerprint_line = fingerprint.line_forms().map(|form| b.string(&form));
            let fingerprint_previous = b.string(fingerprint.previous);
            let in_place = b.equal(previous, fingerprint_previous);
            b.if_else(
                in_place,
                |b| b.set(found, c.zero),
                |b| {
                    emit_search(
                        b,
                        &c,
                        &Search {
                            from: above,
                            has_previous,
                            previous,
                            line: fingerprint_line,
                            before: fingerprint_previous,
                            limit: query.search_lines,
                            found,
                            position,
                        },
                    );
                },
            );
            let tail = emit_tail(&mut b, &c, position, true, query.lines_wanted);
            let outcome = b.finish().execute()?;
            let found = match outcome.get(found)? {
                0 => Found::AtAnchor,
                shift if shift > 0 => Found::Moved(count_of(shift)),
                _ => Found::NotFound,
            };
            Ok(Tail {
                found,
                line: string_of(&outcome, line)?,
                previous: string_of(&outcome, previous)?,
                count: count_of(outcome.get(tail.count)?),
                lines: texts(outcome.get(tail.lines)?),
                above: string_of(&outcome, tail.above)?,
                last: outcome
                    .get(tail.last)?
                    .ok_or(Error::MissingResult(tail.last.id()))?,
            })
        }
    }
}

/// What the upward search in the program works with.
struct Search {
    /// A collapsed range at the start of the line before the anchor.
    from: Reg<kind::TextRange>,
    /// Whether there is a line before the anchor.
    has_previous: Reg<kind::Bool>,
    /// That line's text.
    previous: Reg<kind::Str>,
    /// The fingerprint's line.
    line: [Reg<kind::Str>; 3],
    /// The fingerprint's line before it.
    before: Reg<kind::Str>,
    /// How many lines up to search.
    limit: u32,
    /// Set to how far up the fingerprint was found.
    found: Reg<kind::Int>,
    /// Set to the line where it was found.
    position: Reg<kind::TextRange>,
}

/// Emits the search upward from the line before the anchor for the line
/// pair the fingerprint names, a line at a time, each read once.
fn emit_search(b: &mut Builder, c: &Constants, search: &Search) {
    b.if_(search.has_previous, |b| {
        let row = b.text_range_clone(search.from);
        let text = b.new_string("");
        b.set(text, search.previous);
        let shift = b.new_int(1);
        let limit = b.int(i32::try_from(search.limit).unwrap_or(i32::MAX));
        b.while_(
            |b| b.compare(shift, limit, Comparison::LessThanOrEqual),
            |b| {
                let here = b.text_range_clone(row);
                let up = b.text_range_move(row, c.line, c.back);
                let above = b.new_string("");
                let moved = b.not_equal(up, c.zero);
                b.if_(moved, |b| {
                    let line = c.line_text(b, row);
                    b.set(above, line);
                });
                let [line, with_feed, with_break] = search.line;
                let is_line = b.equal(text, line);
                let is_fed = b.equal(text, with_feed);
                let is_broken = b.equal(text, with_break);
                let is_line = b.or(is_line, is_fed);
                let is_line = b.or(is_line, is_broken);
                let is_before = b.equal(above, search.before);
                let both = b.and(is_line, is_before);
                b.if_(both, |b| {
                    let found = b.add(shift, c.zero);
                    b.set(search.found, found);
                    b.set(search.position, here);
                    b.break_loop();
                });
                let top = b.not(moved);
                b.if_(top, Builder::break_loop);
                b.set(text, above);
                b.add_assign(shift, c.one);
            },
        );
    });
}

/// A string result, empty for a null one: an empty string may come back
/// as null.
fn string_of(outcome: &Outcome, reg: Reg<kind::Str>) -> Result<String, Error> {
    Ok(match outcome.get(reg.any())? {
        Value::String(text) => text,
        _ => String::new(),
    })
}

/// A count from a program, never negative.
fn count_of(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

/// The strings of a program's array of lines, newest first, put oldest
/// first.
fn texts(values: Vec<Value>) -> Vec<String> {
    let mut lines: Vec<String> = values
        .into_iter()
        .map(|value| match value {
            Value::String(text) => text,
            _ => String::new(),
        })
        .collect();
    lines.reverse();
    lines
}

/// A copy of `range` collapsed to its start. Two calls.
fn collapsed(range: &IUIAutomationTextRange) -> Result<IUIAutomationTextRange, Error> {
    let copy = range.clone_range()?;
    copy.move_endpoint_to(Endpoint::End, &copy, Endpoint::Start)?;
    Ok(copy)
}

/// The text of the line containing `range`'s start. Four calls.
fn line_text(range: &IUIAutomationTextRange) -> Result<String, Error> {
    let copy = collapsed(range)?;
    copy.expand(TextUnit_Line)?;
    Ok(String::from_utf16_lossy(&copy.text(-1)?))
}

/// The classic implementation's tail, from the line `from`: what
/// [`emit_tail`] emits, call by call.
fn classic_tail(
    from: &IUIAutomationTextRange,
    count_lines_from: bool,
    wanted: u32,
) -> Result<(IUIAutomationTextRange, u32, Vec<String>, String), Error> {
    let document = from.clone_range()?;
    document.expand(TextUnit_Document)?;
    let last = document.clone_range()?;
    last.move_endpoint_to(Endpoint::Start, &document, Endpoint::End)?;
    last.move_endpoint_by_unit(Endpoint::Start, TextUnit_Character, -1)?;
    last.expand(TextUnit_Line)?;
    let walker = collapsed(if count_lines_from { from } else { &document })?;
    let moved = walker.move_by(TextUnit_Line, FAR)?;
    let mut count = moved;
    if walker.compare_endpoints(Endpoint::Start, &last, Endpoint::Start)? > 0 {
        count -= 1;
    }
    if !count_lines_from {
        count += 1;
    }
    let count = count_of(count);
    let reading = count.min(wanted);
    let row = last.clone_range()?;
    let mut lines = Vec::new();
    let mut at_top = false;
    while lines.len() < usize::try_from(reading).unwrap_or(usize::MAX) {
        lines.push(String::from_utf16_lossy(&row.text(-1)?));
        if row.move_by(TextUnit_Line, -1)? == 0 {
            at_top = true;
            break;
        }
        row.expand(TextUnit_Line)?;
    }
    let above = if !lines.is_empty() && !at_top {
        String::from_utf16_lossy(&row.text(-1)?)
    } else {
        String::new()
    };
    lines.reverse();
    Ok((last, count, lines, above))
}

/// The tail the classic way, the fallback and the reference: the same
/// steps as the remote program, each a cross-process call, counted on the
/// thread's `verbatim_uia::calls`.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails, as one on a range from before a
/// terminal switched screens does.
pub fn terminal_tail_classic(_uia: &Uia, query: &TailQuery<'_>) -> Result<Tail, Error> {
    match query.start {
        TailStart::Document(range) => {
            let (last, count, lines, above) = classic_tail(range, false, query.lines_wanted)?;
            Ok(Tail {
                found: Found::Afresh,
                line: String::new(),
                previous: String::new(),
                count,
                lines,
                above,
                last,
            })
        }
        TailStart::Anchor { range, fingerprint } => {
            let at = collapsed(range)?;
            let line = line_text(&at)?;
            let above = collapsed(&at)?;
            let has_previous = above.move_by(TextUnit_Line, -1)? != 0;
            let previous = if has_previous {
                line_text(&above)?
            } else {
                String::new()
            };
            let mut found = Found::NotFound;
            let mut position = at.clone_range()?;
            if previous == fingerprint.previous {
                found = Found::AtAnchor;
            } else if has_previous {
                let row = above.clone_range()?;
                let mut text = previous.clone();
                for shift in 1..=query.search_lines {
                    let here = row.clone_range()?;
                    let moved = row.move_by(TextUnit_Line, -1)? != 0;
                    let up = if moved {
                        line_text(&row)?
                    } else {
                        String::new()
                    };
                    if fingerprint.line_forms().contains(&text) && up == fingerprint.previous {
                        found = Found::Moved(shift);
                        position = here;
                        break;
                    }
                    if !moved {
                        break;
                    }
                    text = up;
                }
            }
            let (last, count, lines, above) = classic_tail(&position, true, query.lines_wanted)?;
            Ok(Tail {
                found,
                line,
                previous,
                count,
                lines,
                above,
                last,
            })
        }
    }
}
