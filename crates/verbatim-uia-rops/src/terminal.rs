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
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit_Character,
    TextUnit_Document, TextUnit_Line,
};

use verbatim_uia::Uia;
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::caret::{CaretAnswer, CaretLineQuery, caret_line_classic, emit_caret_line};
use crate::error::Error;
use crate::focus::Path;
use crate::opcode::Comparison;
use crate::operation::{Outcome, Value};

/// How many matches of the fingerprint's text are checked, nearest first,
/// before it counts as not found. The search has no bound in lines: it
/// covers the whole text above the anchor in one `FindText` per match, so
/// its cost is the same however far the text scrolled (`docs/performance.md`,
/// "A terminal's upward search"). Only a match that is part of a longer
/// line, or a line under the wrong line, is passed over, so this bounds a
/// read whose text repeats the fingerprint's many times above it.
pub const SEARCH_MATCHES: u32 = 64;

/// How many lines above the anchor the walk searches, line by line, for a
/// provider whose `FindText` failed (Windows Terminal has thrown from it).
const WALK_LINES: u32 = 256;

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

/// What the search for a fingerprint looks for ([`Fingerprint::search`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sought<'a> {
    /// The line before the anchor's, by this text; the anchor's line is the
    /// one under it, whatever it holds.
    Previous(&'a str),
    /// The anchor's line itself, by this text, under a line that holds
    /// exactly the fingerprint's line before.
    Line(&'a str),
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

    /// What `FindText` searches for: the line before the anchor's when it
    /// is not blank, else the anchor's line, each without its trailing
    /// white space and line break, since Windows Terminal's `FindText`
    /// matches neither a line break nor the padding after a line's text,
    /// and failed with an exception searching for padding (the console host
    /// matches all three). `None` when both lines are blank, which no text
    /// search can find, and which is then not found.
    fn search(&self) -> Option<Sought<'_>> {
        if self.previous_tells() {
            return Some(Sought::Previous(self.previous.trim_end()));
        }
        let line = self.line.trim_end();
        (!line.is_empty()).then_some(Sought::Line(line))
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
    /// Afresh, with no anchor, from the element with the text: the program
    /// gets the document range itself, so a fresh read is one round trip;
    /// the classic implementation asks `pattern` for it, one call more.
    Text {
        /// The element with the text, which the program starts from.
        element: &'a IUIAutomationElement,
        /// Its text pattern, which the classic implementation reads.
        pattern: &'a IUIAutomationTextPattern,
    },
}

/// What [`terminal_tail`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct TailQuery<'a> {
    /// Where to start.
    pub start: TailStart<'a>,
    /// The most lines to read from the end of the text.
    pub lines_wanted: u32,
    /// The caret and its line to read too, in the same round trip
    /// ([`Tail::caret`]): a terminal raises no caret event for every
    /// character typed (the console host's come on a schedule of their
    /// own), so its caret is read with each change of its text.
    pub caret: Option<CaretLineQuery<'a>>,
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
    /// the scrollback keeps), or both lines were blank, which no search by
    /// text can find. Counted from the anchor nonetheless.
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
    /// From an anchor, when more lines follow it than the last `rows`: how
    /// many of the first of them were read too, up to the query's
    /// `lines_wanted`, so the start of a flood is heard. Zero otherwise.
    pub head_rows: u32,
    /// The text of those first `head_rows` lines, oldest first, as
    /// [`Self::lines`] is.
    pub head: Vec<String>,
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
    /// Whether the text above where the read started moved while it was
    /// read: a full scrollback scrolled beneath the ranges, so text went by
    /// that the read did not see, rather than only the last lines being
    /// written to.
    pub scrolled: bool,
    /// The caret and its line, when the query asked for them
    /// ([`TailQuery::caret`]), read after the text.
    pub caret: Option<CaretAnswer>,
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
            head_rows: end.head_rows,
            head: end.head,
            last_line: end.last_line,
            before_last: end.before_last,
            last,
            settled: end.settled,
            scrolled: end.scrolled,
            caret: None,
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

    /// Emits the text of the line containing `range`'s start: a copy
    /// expanded to its line, which normalizes the range from its start
    /// alone, so it needs no collapsing first.
    fn line_text(&self, b: &mut Builder, range: Reg<kind::TextRange>) -> Reg<kind::Str> {
        let copy = b.text_range_clone(range);
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
        crate::text::emit_backward_count(b, went);
        b.set(rows, c.one);
        b.subtract_assign(rows, went);
        b.text_range_move_endpoint_by_range(first, c.end, last, c.end);
        let text = b.text_range_get_text(first, c.all);
        b.set(block, text);
    });

    let (head_rows, head_block) = emit_head(
        b,
        c,
        count_lines_from.then_some(from),
        (count, reading, wanted),
    );

    let guard_after = b.new_string("");
    let guard_after = b.add_to_results(guard_after);
    b.if_(has_guard, |b| {
        let text = c.line_text(b, guard);
        b.set(guard_after, text);
    });
    TailRegisters {
        walked: moved,
        last,
        count,
        rows,
        block,
        head_rows,
        head_block,
        last_line,
        before_last,
        guard_before,
        guard_after,
    }
}

/// Emits, from the anchor's line `from` (none with no anchor, when nothing
/// is read), the read of the first of the lines that follow it: as many
/// more as follow the last ones read (`count` less `reading`), up to the
/// number `wanted`, in one read. Returns the registers of how many were
/// read and their text.
fn emit_head(
    b: &mut Builder,
    c: &Constants,
    from: Option<Reg<kind::TextRange>>,
    (count, reading, wanted): (Reg<kind::Int>, Reg<kind::Int>, Reg<kind::Int>),
) -> (Reg<kind::Int>, Reg<kind::Str>) {
    let head_block = b.new_string("");
    let head_block = b.add_to_results(head_block);
    let head_rows = b.new_int(0);
    let head_rows = b.add_to_results(head_rows);
    let Some(from) = from else {
        return (head_rows, head_block);
    };
    let heading = b.new_int(0);
    b.set(heading, count);
    b.subtract_assign(heading, reading);
    let more = b.compare(heading, wanted, Comparison::GreaterThan);
    b.if_(more, |b| b.set(heading, wanted));
    let some = b.compare(heading, c.zero, Comparison::GreaterThan);
    b.if_(some, |b| {
        let first = c.collapsed(b, from);
        let _ = b.text_range_move(first, c.line, c.one);
        let after = b.text_range_clone(first);
        let went = b.text_range_move(after, c.line, heading);
        b.set(head_rows, went);
        b.text_range_move_endpoint_by_range(first, c.end, after, c.start);
        let text = b.text_range_get_text(first, c.all);
        b.set(head_block, text);
    });
    (head_rows, head_block)
}

/// The registers [`emit_tail`] asks for.
struct TailRegisters {
    /// How many lines the walk from `from` to the end of the text moved.
    walked: Reg<kind::Int>,
    last: Reg<kind::TextRange>,
    count: Reg<kind::Int>,
    rows: Reg<kind::Int>,
    block: Reg<kind::Str>,
    head_rows: Reg<kind::Int>,
    head_block: Reg<kind::Str>,
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
    head_rows: u32,
    head: Vec<String>,
    last_line: String,
    before_last: String,
    settled: bool,
    scrolled: bool,
}

impl TailRegisters {
    /// The read's results, the block split into its lines.
    fn end(&self, outcome: &Outcome) -> Result<TailEnd, Error> {
        let rows = count_of(outcome.get(self.rows)?);
        let block = string_of(outcome, self.block)?;
        let head_rows = count_of(outcome.get(self.head_rows)?);
        let head = split_lines(&string_of(outcome, self.head_block)?, head_rows);
        let last_line = string_of(outcome, self.last_line)?;
        let before_last = string_of(outcome, self.before_last)?;
        let guard_before = string_of(outcome, self.guard_before)?;
        let guard_after = string_of(outcome, self.guard_after)?;
        let settled = is_settled(
            &guard_before,
            &guard_after,
            rows,
            &block,
            [before_last.as_str(), last_line.as_str()],
        );
        Ok(TailEnd {
            count: count_of(outcome.get(self.count)?),
            rows,
            lines: split_lines(&block, rows),
            head_rows,
            head,
            last_line,
            before_last,
            settled,
            scrolled: guard_before != guard_after,
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
/// anchor's line and the line before it, searches the text above for the
/// fingerprint by its text when the line before it changed ([`emit_find`]),
/// counts the lines from where it was found to the end of the text, and
/// reads the last of them.
///
/// `uia` is unused; it is in the signature so that this and
/// [`terminal_tail_classic`] are interchangeable ([`TerminalTailFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program.
#[expect(
    clippy::too_many_lines,
    reason = "one program, read top to bottom as it runs"
)]
pub fn terminal_tail_remote(_uia: &Uia, query: &TailQuery<'_>) -> Result<Tail, Error> {
    let mut b = Builder::new();
    let c = Constants::new(&mut b);
    match query.start {
        TailStart::Document(_) | TailStart::Text { .. } => {
            let document = match query.start {
                TailStart::Text { element, .. } => {
                    let element = b.import_element(element);
                    let pattern = b.get_text_pattern(element, false);
                    b.text_pattern_get_document_range(pattern)
                }
                TailStart::Document(range) | TailStart::Anchor { range, .. } => {
                    b.import_text_range(range)
                }
            };
            let tail = emit_tail(&mut b, &c, document, None, query.lines_wanted);
            let caret = query.caret.map(|caret| emit_caret_line(&mut b, &caret));
            let outcome = b.finish().execute()?;
            let mut answer = Tail::new(
                Found::Afresh,
                (String::new(), String::new(), String::new()),
                tail.end(&outcome)?,
                outcome
                    .get(tail.last)?
                    .ok_or(Error::MissingResult(tail.last.id()))?,
            );
            answer.caret = caret.map(|caret| caret.read(&outcome)).transpose()?;
            Ok(answer)
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
            let by_text = b.new_bool(false);
            let fingerprint_previous = b.string(fingerprint.previous);
            let in_place = b.equal(previous, fingerprint_previous);
            b.if_else(
                in_place,
                |b| {
                    b.set(found, c.zero);
                    b.set(found_line, line);
                },
                |b| {
                    emit_find(
                        b,
                        &c,
                        &Find {
                            at,
                            above,
                            has_previous,
                            fingerprint,
                            before: fingerprint_previous,
                            by_text,
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
            // How far up it was found, as the walk line by line counts it
            // ([`classic_distance`]).
            b.if_(by_text, |b| {
                let from_anchor = c.collapsed(b, at);
                let to_end = b.text_range_move(from_anchor, c.line, c.far);
                let own = b.text_range_clone(at);
                b.text_range_expand_to_enclosing_unit(own, c.line);
                let order = b.text_range_compare_endpoints(own, c.start, at, c.start);
                let inside = b.not_equal(order, c.zero);
                let shift = b.new_int(0);
                b.set(shift, tail.walked);
                b.subtract_assign(shift, to_end);
                b.if_(inside, |b| b.add_assign(shift, c.one));
                let short = b.compare(shift, c.one, Comparison::LessThan);
                b.if_(short, |b| b.set(shift, c.one));
                b.set(found, shift);
            });
            let caret = query.caret.map(|caret| emit_caret_line(&mut b, &caret));
            let outcome = b.finish().execute()?;
            let found = match outcome.get(found)? {
                0 => Found::AtAnchor,
                shift if shift > 0 => Found::Moved(count_of(shift)),
                _ => Found::NotFound,
            };
            let mut answer = Tail::new(
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
            );
            answer.caret = caret.map(|caret| caret.read(&outcome)).transpose()?;
            Ok(answer)
        }
    }
}

/// What the search in the program works with.
struct Find<'a> {
    /// A collapsed range at the anchor.
    at: Reg<kind::TextRange>,
    /// A collapsed range at the start of the line before the anchor.
    above: Reg<kind::TextRange>,
    /// Whether there is a line before the anchor.
    has_previous: Reg<kind::Bool>,
    /// The fingerprint sought.
    fingerprint: Fingerprint<'a>,
    /// The fingerprint's line before, as a program string.
    before: Reg<kind::Str>,
    /// Set when the fingerprint was found.
    by_text: Reg<kind::Bool>,
    /// Set to the text of the anchor's line where it was found.
    found_line: Reg<kind::Str>,
    /// Set to that line.
    position: Reg<kind::TextRange>,
    /// Set to the text of the line above it.
    position_under: Reg<kind::Str>,
}

/// Emits the search for the fingerprint by its text
/// ([`Fingerprint::search`]): `FindText` backward over the whole text above
/// the line before the anchor (above the anchor, for the anchor's own
/// line), on a range the program made (one imported
/// would be the caller's own, which the search would move), each match
/// taken only when it starts its line and that line holds exactly the line
/// sought (and, for the anchor's line sought by itself, lies under a line
/// holding the fingerprint's line before), the nearest first, up to
/// [`SEARCH_MATCHES`] matches. Nothing is emitted for a fingerprint of two
/// blank lines, which is then not found.
fn emit_find(b: &mut Builder, c: &Constants, find: &Find<'_>) {
    let Some(sought) = find.fingerprint.search() else {
        return;
    };
    let forms = find.fingerprint.line_forms().map(|form| b.string(&form));
    b.if_(find.has_previous, |b| {
        // The line before the anchor's is sought above the line before the
        // anchor (it is not that line, or the fingerprint would be in
        // place); the anchor's line, above the anchor.
        let limit = match sought {
            Sought::Previous(_) => find.above,
            Sought::Line(_) => find.at,
        };
        let span = b.text_range_clone(limit);
        let _ = b.text_range_move_endpoint_by_unit(span, c.start, c.document, c.back);
        let needle = b.string(match sought {
            Sought::Previous(text) | Sought::Line(text) => text,
        });
        let backward = b.bool(true);
        let exact_case = b.bool(false);
        let tries = b.new_int(0);
        let most = b.int(i32::try_from(SEARCH_MATCHES).unwrap_or(i32::MAX));
        b.while_(
            |b| b.compare(tries, most, Comparison::LessThan),
            |b| {
                let hit = b.text_range_find_text(span, needle, backward, exact_case);
                let none = b.is_null(hit);
                b.if_(none, Builder::break_loop);
                let row = b.text_range_clone(hit);
                b.text_range_expand_to_enclosing_unit(row, c.line);
                let order = b.text_range_compare_endpoints(row, c.start, hit, c.start);
                let starts = b.equal(order, c.zero);
                let text = b.text_range_get_text(row, c.all);
                match sought {
                    Sought::Previous(_) => {
                        let same = b.equal(text, find.before);
                        let taken = b.and(starts, same);
                        b.if_(taken, |b| {
                            let under = c.collapsed(b, row);
                            let _ = b.text_range_move(under, c.line, c.one);
                            let under_text = c.line_text(b, under);
                            b.set(find.found_line, under_text);
                            b.set(find.position, under);
                            b.set(find.position_under, text);
                            let yes = b.bool(true);
                            b.set(find.by_text, yes);
                            b.break_loop();
                        });
                    }
                    Sought::Line(_) => {
                        let [as_read, with_feed, with_break] = forms;
                        let is_line = b.equal(text, as_read);
                        let is_fed = b.equal(text, with_feed);
                        let is_broken = b.equal(text, with_break);
                        let is_line = b.or(is_line, is_fed);
                        let is_line = b.or(is_line, is_broken);
                        let candidate = b.and(starts, is_line);
                        b.if_(candidate, |b| {
                            let up = c.collapsed(b, row);
                            let went = b.text_range_move(up, c.line, c.back);
                            let over = b.new_string("");
                            let has_over = b.not_equal(went, c.zero);
                            b.if_(has_over, |b| {
                                let line = c.line_text(b, up);
                                b.set(over, line);
                            });
                            let under_before = b.equal(over, find.before);
                            b.if_(under_before, |b| {
                                b.set(find.found_line, text);
                                b.set(find.position, row);
                                b.set(find.position_under, over);
                                let yes = b.bool(true);
                                b.set(find.by_text, yes);
                                b.break_loop();
                            });
                        });
                    }
                }
                // Search again above this match.
                b.text_range_move_endpoint_by_range(span, c.end, hit, c.start);
                b.add_assign(tries, c.one);
            },
        );
    });
}

// A string result, empty for a null one: an empty string may come back
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

/// The text of the line containing `range`'s start, as
/// [`Constants::line_text`] emits it. Three calls.
fn line_text(range: &IUIAutomationTextRange) -> Result<String, Error> {
    let copy = range.clone_range()?;
    copy.expand(TextUnit_Line)?;
    Ok(String::from_utf16_lossy(&copy.text(-1)?))
}

/// The classic implementation's tail, from the line `from`, with the last
/// line's range: what [`emit_tail`] emits, call by call.
fn classic_tail(
    from: &IUIAutomationTextRange,
    found_under: Option<&str>,
    wanted: u32,
) -> Result<(TailEnd, IUIAutomationTextRange, i32), Error> {
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
    let heading = if count_lines_from {
        (count - reading).min(wanted)
    } else {
        0
    };
    let (head_rows, head_block) = if heading == 0 {
        (0, String::new())
    } else {
        let first = collapsed(from)?;
        let _ = first.move_by(TextUnit_Line, 1)?;
        let after = first.clone_range()?;
        let went = after.move_by(TextUnit_Line, i32::try_from(heading).unwrap_or(i32::MAX))?;
        first.move_endpoint_to(Endpoint::End, &after, Endpoint::Start)?;
        (count_of(went), String::from_utf16_lossy(&first.text(-1)?))
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
            head_rows,
            head: split_lines(&head_block, head_rows),
            last_line,
            before_last,
            settled,
            scrolled: guard_before != guard_after,
        },
        last,
        moved,
    ))
}

/// The search for the fingerprint by its text, the classic way, as
/// [`emit_find`] emits it: `FindText` backward over the whole text above
/// `above` (a collapsed range at the start of the line before the anchor
/// `at`), or above the anchor for the anchor's own line, each match taken only when it starts its line and that line holds
/// exactly the line sought, the nearest first, up to [`SEARCH_MATCHES`]
/// matches. One call per match rather than six per line: against Windows
/// Terminal and the console host with a full scrollback, 6 to 10
/// milliseconds wherever the fingerprint is (2026-10-07).
///
/// A call that failed (Windows Terminal has thrown from `FindText`) is
/// [`ByText::Failed`], for the caller to search line by line instead.
fn classic_find(
    at: &IUIAutomationTextRange,
    above: &IUIAutomationTextRange,
    fingerprint: &Fingerprint<'_>,
) -> ByText {
    let Some(sought) = fingerprint.search() else {
        return ByText::Absent;
    };
    let search = || -> Result<ByText, Error> {
        // Both are collapsed: a copy, its start moved to the text's start.
        let span = match sought {
            Sought::Previous(_) => above,
            Sought::Line(_) => at,
        }
        .clone_range()?;
        span.move_endpoint_by_unit(Endpoint::Start, TextUnit_Document, -1)?;
        let needle = match sought {
            Sought::Previous(text) | Sought::Line(text) => text,
        };
        for _ in 0..SEARCH_MATCHES {
            let Some(hit) = span.find_text(needle, true)? else {
                return Ok(ByText::Absent);
            };
            let row = hit.clone_range()?;
            row.expand(TextUnit_Line)?;
            let starts = row.compare_endpoints(Endpoint::Start, &hit, Endpoint::Start)? == 0;
            let text = String::from_utf16_lossy(&row.text(-1)?);
            match sought {
                Sought::Previous(_) if starts && text == fingerprint.previous => {
                    let under = collapsed(&row)?;
                    under.move_by(TextUnit_Line, 1)?;
                    let under_text = line_text(&under)?;
                    return Ok(ByText::Found {
                        previous: text,
                        line: under,
                        text: under_text,
                    });
                }
                Sought::Line(_) if starts && fingerprint.line_forms().contains(&text) => {
                    let up = collapsed(&row)?;
                    let over = if up.move_by(TextUnit_Line, -1)? == 0 {
                        String::new()
                    } else {
                        line_text(&up)?
                    };
                    if over == fingerprint.previous {
                        return Ok(ByText::Found {
                            previous: over,
                            line: row,
                            text,
                        });
                    }
                }
                _ => {}
            }
            // Search again above this match.
            span.move_endpoint_to(Endpoint::End, &hit, Endpoint::Start)?;
        }
        Ok(ByText::Absent)
    };
    search().unwrap_or(ByText::Failed)
}

/// What [`classic_find`] found.
enum ByText {
    /// The fingerprint: the text of the line before the anchor's line, as
    /// read, that line's range, and its text.
    Found {
        previous: String,
        line: IUIAutomationTextRange,
        text: String,
    },
    /// Nowhere above the anchor, or not searchable by text.
    Absent,
    /// A call failed.
    Failed,
}

/// How many lines above the anchor `at` (collapsed) the fingerprint was
/// found by its text, as the search line by line counts them: the walk to
/// the end of the text from where it was found (`walked`) less the same
/// walk from the anchor, one more when the anchor is inside its line
/// (moving up from there first reaches that line's own start, which the
/// search line by line counts as a line), and at least 1.
fn classic_distance(at: &IUIAutomationTextRange, walked: i32) -> Result<u32, Error> {
    let from_anchor = collapsed(at)?;
    let to_end = from_anchor.move_by(TextUnit_Line, FAR)?;
    let own = at.clone_range()?;
    own.expand(TextUnit_Line)?;
    let inside = own.compare_endpoints(Endpoint::Start, at, Endpoint::Start)? != 0;
    Ok(count_of((walked - to_end + i32::from(inside)).max(1)))
}

/// The walk up from `above` (a collapsed range at the start of the line
/// before the anchor, whose text is `previous`) a line at a time, for a
/// provider whose `FindText` failed: up to [`WALK_LINES`] lines, each read
/// once, until a line under a line equal to the fingerprint's line before
/// matches the fingerprint's line ([`Fingerprint::matches`]). The line
/// found, its distance up, its text, and the text of the line above it.
fn classic_walk(
    above: &IUIAutomationTextRange,
    previous: &str,
    fingerprint: &Fingerprint<'_>,
) -> Result<Option<(IUIAutomationTextRange, u32, String, String)>, Error> {
    let row = above.clone_range()?;
    let mut text = previous.to_owned();
    for shift in 1..=WALK_LINES {
        let here = row.clone_range()?;
        let moved = row.move_by(TextUnit_Line, -1)? != 0;
        let up = if moved {
            line_text(&row)?
        } else {
            String::new()
        };
        if fingerprint.matches(&text) && up == fingerprint.previous {
            return Ok(Some((here, shift, text, up)));
        }
        if !moved {
            break;
        }
        text = up;
    }
    Ok(None)
}

/// The tail the classic way, the fallback and the reference: the same
/// steps as the remote program, each a cross-process call, counted on the
/// thread's `verbatim_uia::calls`, and, where the provider's `FindText`
/// fails, a walk up line by line in its place.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails, as one on a range from before a
/// terminal switched screens does.
pub fn terminal_tail_classic(_uia: &Uia, query: &TailQuery<'_>) -> Result<Tail, Error> {
    let mut tail = classic_text(query)?;
    if let Some(caret) = &query.caret {
        tail.caret = Some(caret_line_classic(caret)?);
    }
    Ok(tail)
}

/// The tail's text the classic way, as [`terminal_tail_classic`] reads it.
fn classic_text(query: &TailQuery<'_>) -> Result<Tail, Error> {
    match query.start {
        TailStart::Document(_) | TailStart::Text { .. } => {
            let document = match query.start {
                TailStart::Text { pattern, .. } => pattern.document_range()?,
                TailStart::Document(range) | TailStart::Anchor { range, .. } => range.clone(),
            };
            let (end, last, _) = classic_tail(&document, None, query.lines_wanted)?;
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
            let mut by_text = false;
            if previous == fingerprint.previous {
                found = Found::AtAnchor;
                found_line.clone_from(&line);
            } else if has_previous {
                match classic_find(&at, &above, &fingerprint) {
                    ByText::Found {
                        previous: row,
                        line: under,
                        text: under_text,
                    } => {
                        by_text = true;
                        found_line = under_text;
                        position = under;
                        position_under = row;
                    }
                    ByText::Absent => {}
                    ByText::Failed => {
                        if let Some((here, shift, text, up)) =
                            classic_walk(&above, &previous, &fingerprint)?
                        {
                            found = Found::Moved(shift);
                            found_line = text;
                            position = here;
                            position_under = up;
                        }
                    }
                }
            }
            let (end, last, walked) =
                classic_tail(&position, Some(&position_under), query.lines_wanted)?;
            if by_text {
                found = Found::Moved(classic_distance(&at, walked)?);
            }
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
