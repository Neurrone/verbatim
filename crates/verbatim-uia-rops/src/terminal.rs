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

    /// Whether the line before the anchor's is enough to find the anchor's
    /// line by, whatever that line holds now: when it is not blank, as at
    /// the anchor itself. The last line read is often the one output is
    /// still being written to (the cursor's line, blank or half written),
    /// which is complete by the next read; under a blank line, a changed
    /// line matches too easily to trust.
    fn previous_tells(&self) -> bool {
        !self.previous.trim().is_empty()
    }

    /// Whether `text`, a line read now under a line that matches the
    /// fingerprint's line before, is the anchor's line.
    fn matches(&self, text: &str) -> bool {
        self.previous_tells() || self.line_forms().iter().any(|form| form == text)
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
    /// The text of the line where the fingerprint was found, as it is now,
    /// which may have grown since it was read; `line` when it was found at
    /// the anchor, empty when it was not found.
    pub found_line: String,
    /// How many lines follow the anchor's line to the end of the text; with
    /// no anchor, how many lines the text has.
    pub count: u32,
    /// How many of the last lines were read: as many as `count` up to the
    /// query's `lines_wanted`.
    pub rows: u32,
    /// The text of the last `rows` lines, read in one call, oldest first,
    /// each without its line break. A line the terminal wrapped across
    /// rows is one line here, so there can be fewer than `rows`.
    pub lines: Vec<String>,
    /// The text of the last line, read as a line.
    pub last_line: String,
    /// The text of the line before the last, read as a line; empty at the
    /// top of the text.
    pub before_last: String,
    /// The last line's range, whose start is the next read's anchor.
    pub last: IUIAutomationTextRange,
    /// Whether the text held still while it was read ([`is_settled`]);
    /// when it did not, the lines and the count may mix two moments.
    pub settled: bool,
}

impl Tail {
    /// A tail from where the fingerprint was found, the anchor's line and
    /// the one before it, what was read below, and the last line's range.
    fn new(
        found: Found,
        (line, previous, found_line): (String, String, String),
        end: TailEnd,
        last: IUIAutomationTextRange,
    ) -> Self {
        Self {
            found,
            line,
            previous,
            found_line,
            count: end.count,
            rows: end.rows,
            lines: end.lines,
            last_line: end.last_line,
            before_last: end.before_last,
            last,
            settled: end.settled,
        }
    }
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
/// (`count`), the last lines' text in one read, and the last line and the
/// one before it, each read as a line.
fn emit_tail(
    b: &mut Builder,
    c: &Constants,
    from: Reg<kind::TextRange>,
    found_under: Option<Reg<kind::Str>>,
    wanted: u32,
) -> TailRegisters {
    let count_lines_from = found_under.is_some();
    // The guard: the line above `from`'s, as it was when `from` was found
    // (`found_under`), and again at the end, which scrolling changes; above
    // an anchor at the top there is none. With no anchor, the guard is the
    // text's first line itself, read now, there being none above it.
    let guard = c.collapsed(b, from);
    let up = b.text_range_move(guard, c.line, c.back);
    let has_guard = b.not_equal(up, c.zero);
    let guard_before = if let Some(text) = found_under {
        text
    } else {
        let yes = b.bool(true);
        b.set(has_guard, yes);
        c.line_text(b, guard)
    };
    let guard_before = b.add_to_results(guard_before);

    // Lines from `from`'s line to the last: moving on by lines lands on the
    // last line's start, or past it at the text's end.
    let document = b.text_range_clone(from);
    b.text_range_expand_to_enclosing_unit(document, c.document);
    let walker = c.collapsed(b, if count_lines_from { from } else { document });
    let moved = b.text_range_move(walker, c.line, c.far);

    // The last line: the one where the walk stopped, or, stopped past the
    // final line break, the one before; found from the walk itself, so the
    // count and the last line agree however the text grows meanwhile.
    let last = b.text_range_clone(walker);
    b.text_range_expand_to_enclosing_unit(last, c.line);
    let empty = b.text_range_compare_endpoints(last, c.start, last, c.end);
    let empty = b.equal(empty, c.zero);
    b.if_(empty, |b| {
        let _ = b.text_range_move_endpoint_by_unit(last, c.start, c.character, c.back);
        b.text_range_expand_to_enclosing_unit(last, c.line);
    });
    let last = b.add_to_results(last);

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

    // The last lines, as many as are counted up to the number wanted, in
    // one read, which the provider answers at one moment; and the last
    // line and the one before it, each read as a line: the next read's
    // fingerprint.
    let wanted = b.int(i32::try_from(wanted).unwrap_or(i32::MAX));
    let reading = b.new_int(0);
    b.set(reading, count);
    let more = b.compare(reading, wanted, Comparison::GreaterThan);
    b.if_(more, |b| b.set(reading, wanted));
    let block = b.new_string("");
    let block = b.add_to_results(block);
    let rows = b.new_int(0);
    let rows = b.add_to_results(rows);
    let last_line = b.new_string("");
    let last_line = b.add_to_results(last_line);
    let before_last = b.new_string("");
    let before_last = b.add_to_results(before_last);
    let some = b.compare(reading, c.zero, Comparison::GreaterThan);
    b.if_(some, |b| {
        let text = b.text_range_get_text(last, c.all);
        b.set(last_line, text);
        let before = c.collapsed(b, last);
        let up = b.text_range_move(before, c.line, c.back);
        let has_before = b.not_equal(up, c.zero);
        b.if_(has_before, |b| {
            let text = c.line_text(b, before);
            b.set(before_last, text);
        });

        // Up from the last line by one line fewer than are read.
        let back = b.new_int(1);
        b.subtract_assign(back, reading);
        let first = c.collapsed(b, last);
        let went = b.text_range_move(first, c.line, back);
        b.set(rows, c.one);
        b.subtract_assign(rows, went);
        b.text_range_move_endpoint_by_range(first, c.end, last, c.end);
        let text = b.text_range_get_text(first, c.all);
        b.set(block, text);
    });

    let guard_after = b.new_string("");
    let guard_after = b.add_to_results(guard_after);
    b.if_(has_guard, |b| {
        let text = c.line_text(b, guard);
        b.set(guard_after, text);
    });
    TailRegisters {
        last,
        count,
        rows,
        block,
        last_line,
        before_last,
        guard_before,
        guard_after,
    }
}

/// The registers [`emit_tail`] asks for.
struct TailRegisters {
    last: Reg<kind::TextRange>,
    count: Reg<kind::Int>,
    rows: Reg<kind::Int>,
    block: Reg<kind::Str>,
    last_line: Reg<kind::Str>,
    before_last: Reg<kind::Str>,
    guard_before: Reg<kind::Str>,
    guard_after: Reg<kind::Str>,
}

/// The text a tail read found below where it started, however it was read.
struct TailEnd {
    count: u32,
    rows: u32,
    lines: Vec<String>,
    last_line: String,
    before_last: String,
    settled: bool,
}

impl TailRegisters {
    /// The read's results, the block split into its lines.
    fn end(&self, outcome: &Outcome) -> Result<TailEnd, Error> {
        let rows = count_of(outcome.get(self.rows)?);
        let block = string_of(outcome, self.block)?;
        let last_line = string_of(outcome, self.last_line)?;
        let before_last = string_of(outcome, self.before_last)?;
        let settled = is_settled(
            &string_of(outcome, self.guard_before)?,
            &string_of(outcome, self.guard_after)?,
            rows,
            &block,
            [before_last.as_str(), last_line.as_str()],
        );
        Ok(TailEnd {
            count: count_of(outcome.get(self.count)?),
            rows,
            lines: split_lines(&block, rows),
            last_line,
            before_last,
            settled,
        })
    }
}

/// The lines of `block`, the text of `rows` lines read in one call, each
/// without its line break. A line wrapped across rows is one line; the
/// break after the last line, where the provider gives one, ends it rather
/// than starting another.
fn split_lines(block: &str, rows: u32) -> Vec<String> {
    if rows == 0 {
        return Vec::new();
    }
    let body = block.strip_suffix('\n').unwrap_or(block);
    body.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect()
}

/// Whether a read settled: `guard` read the same before and after, and the
/// `rows` lines read in one call, `block`, end with the last line and the
/// one before it as each was read on its own (`fingerprint`, the one before
/// first), line breaks aside (Windows Terminal ends each line's text with
/// one, the console host gives a line without it but separates lines with
/// one in a longer range). Text written while a read is under way (output
/// scrolling a full scrollback beneath its ranges, or a line rewritten in
/// place) makes the reads a mixture of two moments; the change that did it
/// raises a text change of its own, and the read that follows it settles.
fn is_settled(
    guard_before: &str,
    guard_after: &str,
    rows: u32,
    block: &str,
    [before_last, last_line]: [&str; 2],
) -> bool {
    let text =
        |line: &str| -> String { line.chars().filter(|c| !matches!(c, '\r' | '\n')).collect() };
    let ending = match rows {
        0 => String::new(),
        1 => text(last_line),
        _ => text(before_last) + &text(last_line),
    };
    guard_before == guard_after && text(block).ends_with(&ending)
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
            let tail = emit_tail(&mut b, &c, document, None, query.lines_wanted);
            let outcome = b.finish().execute()?;
            Ok(Tail::new(
                Found::Afresh,
                (String::new(), String::new(), String::new()),
                tail.end(&outcome)?,
                outcome
                    .get(tail.last)?
                    .ok_or(Error::MissingResult(tail.last.id()))?,
            ))
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
            let found_line = b.new_string("");
            let found_line = b.add_to_results(found_line);
            let position = b.text_range_clone(at);
            // The line above `position`, as it was when found.
            let position_under = b.new_string("");
            b.set(position_under, previous);
            let fingerprint_line = fingerprint.line_forms().map(|form| b.string(&form));
            let previous_tells = fingerprint.previous_tells();
            let fingerprint_previous = b.string(fingerprint.previous);
            let in_place = b.equal(previous, fingerprint_previous);
            b.if_else(
                in_place,
                |b| {
                    b.set(found, c.zero);
                    b.set(found_line, line);
                },
                |b| {
                    emit_search(
                        b,
                        &c,
                        &Search {
                            from: above,
                            has_previous,
                            previous,
                            line: fingerprint_line,
                            previous_tells,
                            before: fingerprint_previous,
                            limit: query.search_lines,
                            found,
                            found_line,
                            position,
                            position_under,
                        },
                    );
                },
            );
            let tail = emit_tail(
                &mut b,
                &c,
                position,
                Some(position_under),
                query.lines_wanted,
            );
            let outcome = b.finish().execute()?;
            let found = match outcome.get(found)? {
                0 => Found::AtAnchor,
                shift if shift > 0 => Found::Moved(count_of(shift)),
                _ => Found::NotFound,
            };
            Ok(Tail::new(
                found,
                (
                    string_of(&outcome, line)?,
                    string_of(&outcome, previous)?,
                    string_of(&outcome, found_line)?,
                ),
                tail.end(&outcome)?,
                outcome
                    .get(tail.last)?
                    .ok_or(Error::MissingResult(tail.last.id()))?,
            ))
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
    /// Whether the line before is enough to find the line by
    /// ([`Fingerprint::previous_tells`]).
    previous_tells: bool,
    /// The fingerprint's line before it.
    before: Reg<kind::Str>,
    /// How many lines up to search.
    limit: u32,
    /// Set to how far up the fingerprint was found.
    found: Reg<kind::Int>,
    /// Set to the text of the line where it was found.
    found_line: Reg<kind::Str>,
    /// Set to the line where it was found.
    position: Reg<kind::TextRange>,
    /// Set to the text of the line above it.
    position_under: Reg<kind::Str>,
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
                let is_line = if search.previous_tells {
                    b.bool(true)
                } else {
                    is_line
                };
                let is_before = b.equal(above, search.before);
                let both = b.and(is_line, is_before);
                b.if_(both, |b| {
                    let found = b.add(shift, c.zero);
                    b.set(search.found, found);
                    b.set(search.found_line, text);
                    b.set(search.position, here);
                    b.set(search.position_under, above);
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

/// The classic implementation's tail, from the line `from`, with the last
/// line's range: what [`emit_tail`] emits, call by call.
fn classic_tail(
    from: &IUIAutomationTextRange,
    found_under: Option<&str>,
    wanted: u32,
) -> Result<(TailEnd, IUIAutomationTextRange), Error> {
    let count_lines_from = found_under.is_some();
    // With no anchor the guard is the text's first line itself, there
    // being none above it; above an anchor at the top there is none.
    let guard = collapsed(from)?;
    let has_guard = guard.move_by(TextUnit_Line, -1)? != 0 || found_under.is_none();
    let guard_before = match found_under {
        Some(text) => text.to_owned(),
        None => line_text(&guard)?,
    };
    let document = from.clone_range()?;
    document.expand(TextUnit_Document)?;
    let walker = collapsed(if count_lines_from { from } else { &document })?;
    let moved = walker.move_by(TextUnit_Line, FAR)?;
    let last = walker.clone_range()?;
    last.expand(TextUnit_Line)?;
    if last.compare_endpoints(Endpoint::Start, &last, Endpoint::End)? == 0 {
        last.move_endpoint_by_unit(Endpoint::Start, TextUnit_Character, -1)?;
        last.expand(TextUnit_Line)?;
    }
    let mut count = moved;
    if walker.compare_endpoints(Endpoint::Start, &last, Endpoint::Start)? > 0 {
        count -= 1;
    }
    if !count_lines_from {
        count += 1;
    }
    let count = count_of(count);
    let reading = count.min(wanted);
    let (rows, block, last_line, before_last) = if reading == 0 {
        (0, String::new(), String::new(), String::new())
    } else {
        let last_line = String::from_utf16_lossy(&last.text(-1)?);
        let before = collapsed(&last)?;
        let before_last = if before.move_by(TextUnit_Line, -1)? == 0 {
            String::new()
        } else {
            line_text(&before)?
        };
        let first = collapsed(&last)?;
        let back = 1 - i32::try_from(reading).unwrap_or(i32::MAX);
        let went = first.move_by(TextUnit_Line, back)?;
        first.move_endpoint_to(Endpoint::End, &last, Endpoint::End)?;
        (
            count_of(1 - went),
            String::from_utf16_lossy(&first.text(-1)?),
            last_line,
            before_last,
        )
    };
    let guard_after = if has_guard {
        line_text(&guard)?
    } else {
        String::new()
    };
    let settled = is_settled(
        &guard_before,
        &guard_after,
        rows,
        &block,
        [before_last.as_str(), last_line.as_str()],
    );
    Ok((
        TailEnd {
            count,
            rows,
            lines: split_lines(&block, rows),
            last_line,
            before_last,
            settled,
        },
        last,
    ))
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
            let (end, last) = classic_tail(range, None, query.lines_wanted)?;
            Ok(Tail::new(
                Found::Afresh,
                (String::new(), String::new(), String::new()),
                end,
                last,
            ))
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
            let mut found_line = String::new();
            let mut position = at.clone_range()?;
            let mut position_under = previous.clone();
            if previous == fingerprint.previous {
                found = Found::AtAnchor;
                found_line.clone_from(&line);
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
                    if fingerprint.matches(&text) && up == fingerprint.previous {
                        found = Found::Moved(shift);
                        found_line = text;
                        position = here;
                        position_under = up;
                        break;
                    }
                    if !moved {
                        break;
                    }
                    text = up;
                }
            }
            let (end, last) = classic_tail(&position, Some(&position_under), query.lines_wanted)?;
            Ok(Tail::new(found, (line, previous, found_line), end, last))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_settled, split_lines};

    #[test]
    fn a_read_settles_only_when_the_text_held_still() {
        let wt = ["one\r\n", "two\r\n"];
        assert!(is_settled("top", "top", 3, "zero\r\none\r\ntwo\r\n", wt));
        assert!(is_settled("top", "top", 0, "", wt));
        // The console host's lines come without their line breaks.
        assert!(is_settled("top", "top", 2, "one\r\ntwo", ["one", "two"]));
        // Scrolled beneath the guard.
        assert!(!is_settled("top", "next", 2, "one\r\ntwo\r\n", wt));
        // The lines read on their own are not where the single read ends.
        assert!(!is_settled("top", "top", 2, "two\r\nthree\r\n", wt));
    }

    #[test]
    fn a_block_splits_into_its_lines_wrapped_ones_whole() {
        assert_eq!(split_lines("a  \r\nb  \r\n", 2), ["a  ", "b  "]);
        assert_eq!(split_lines("a  \r\nb  ", 2), ["a  ", "b  "]);
        assert_eq!(split_lines("xxxyy \r\nb  \r\n", 3), ["xxxyy ", "b  "]);
        assert_eq!(split_lines("", 1), [""]);
        assert_eq!(split_lines("", 0), Vec::<String>::new());
    }
}
