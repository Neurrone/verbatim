//! A terminal's screen (`phase6-design.md`, "Terminal reading by diffing
//! the screen"): the text now on screen, read in one call, and where the
//! screen's top row as last read is now, so the caller can tell how far the
//! text scrolled since. A remote program reads it in one round trip, and a
//! classic implementation behind the same signature reads it call by call;
//! [`terminal_screen`] is the one function callers use.
//!
//! The screen is the text pattern's first visible range. The anchor is the
//! top two rows of the screen as last read, as the provider gave them,
//! padding and line breaks included. It is found first by a range kept at
//! the start of the top row as last read ([`ScreenAnchor::range`]), trusted
//! only while nothing has been discarded from the text (its first row reads
//! as it did, or the old screen had no history above it), since once a
//! terminal's history is full a range keeps its row while the text moves up
//! beneath it. Otherwise it is found by its text: `FindText` looks for the
//! more distinctive of the two rows (not blank, then the longer), searching
//! backward from the screen's second row toward the start of the text, and
//! a match counts only when it starts its row, the row holds exactly the
//! row sought, and the other row of the pair is next to it; at most
//! [`SEARCH_MATCHES`] matches are checked. A line break is never sought.
//! A row's padding is sought only where the provider's `FindText` matches
//! it ([`ScreenQuery::matches_padding`], the console host's), so a longer
//! row starting with the same text does not match; Windows Terminal's
//! throws on padding, so there the row is sought without it. Found, the
//! rows from it to the screen's top are counted by moving by lines, which
//! transfers no text; not found, the rows of the whole text are counted,
//! for the size of a history the anchor has left.
//!
//! A screen read while the terminal wrote to it is marked unsettled: its
//! top row read on its own differs from the screen's text, or changed by
//! the end of the read. Whether the terminal's view moved while it was
//! read is reported on its own, for the caller to judge: output scrolling
//! the view leaves the rows read where they were, while a terminal that
//! redraws a fixed row lower as it moves its view does not.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit_Character,
    TextUnit_Line,
};

use verbatim_uia::Uia;
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::caret::{CaretAnswer, CaretLineQuery, caret_line_classic, emit_caret_line};
use crate::error::Error;
use crate::focus::Path;
use crate::opcode::Comparison;
use crate::operation::{Outcome, Value};

/// How many matches of the anchor's row are checked, nearest the screen
/// first, before it counts as not found (Dickson, 2026-10-07). The search
/// has no bound in lines: one `FindText` per match covers the whole text
/// above the screen.
pub const SEARCH_MATCHES: u32 = 20;

/// More lines than any terminal holds, for moving to the end of the text.
const FAR: i32 = 1_000_000;

/// The screen's top two rows as last read, as the provider gave them.
#[derive(Clone, Copy, Debug)]
pub struct ScreenAnchor<'a> {
    /// The top row.
    pub top: &'a str,
    /// The row after it; empty when the screen had one row.
    pub next: &'a str,
    /// A range at the start of the top row as it was read
    /// ([`Screen::top`]), tried first: until the terminal discards its
    /// oldest rows it stays on the row, which is then found with no search.
    pub range: Option<&'a IUIAutomationTextRange>,
    /// The text's first row as it was read ([`Screen::first_row`]): while it
    /// reads the same, nothing has been discarded.
    pub first: &'a str,
    /// Whether the screen as last read had no history above it, so nothing
    /// could have been discarded since.
    pub no_history: bool,
}

/// Which of the anchor's rows a match is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// The top row, with the next row under it.
    Top,
    /// The next row, with the top row over it.
    Next,
}

/// What the search looks for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sought {
    /// The text `FindText` is given.
    needle: String,
    /// Which row a match starts.
    side: Side,
}

impl ScreenAnchor<'_> {
    /// What the search looks for: the more distinctive of the two rows, not
    /// blank first, then the longer. With `matches_padding` (a provider
    /// whose `FindText` matches a row's padding, as the console host's
    /// does), the row as read, padding included and line break left out,
    /// which a longer row starting with the same text does not match;
    /// otherwise without its padding, since Windows Terminal's `FindText`
    /// throws on it. A needle spanning a line break is never used: the
    /// console host's `FindText` returns a range for one whose text cannot
    /// then be read, and Windows Terminal's matches none (measured
    /// 2026-10-08). `None` when both rows are blank, which no text search
    /// can find.
    fn sought(&self, matches_padding: bool) -> Option<Sought> {
        let top = self.top.trim_end();
        let next = self.next.trim_end();
        if top.is_empty() && next.is_empty() {
            return None;
        }
        let side =
            if top.is_empty() || (!next.is_empty() && next.chars().count() > top.chars().count()) {
                Side::Next
            } else {
                Side::Top
            };
        let row = match side {
            Side::Top => self.top,
            Side::Next => self.next,
        };
        let needle = if matches_padding {
            row.trim_end_matches(['\r', '\n'])
        } else {
            row.trim_end()
        };
        Some(Sought {
            needle: needle.to_owned(),
            side,
        })
    }
}

/// What [`terminal_screen`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct ScreenQuery<'a> {
    /// The element with the text.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern.
    pub pattern: &'a IUIAutomationTextPattern,
    /// The screen's top rows as last read; `None` for a first read.
    pub anchor: Option<ScreenAnchor<'a>>,
    /// Whether the provider's `FindText` matches a row's padding (the
    /// console host's does, Windows Terminal's throws), so the anchor's
    /// row is sought with it.
    pub matches_padding: bool,
    /// How many rows of the screen as last read held its text: rows past
    /// these, found between the anchor and the screen now, went by unread.
    pub seen_rows: u32,
    /// The most of those unread rows to read, from the first, so the start
    /// of a flood can be heard.
    pub head_wanted: u32,
    /// The caret and its line to read too, in the same round trip: a
    /// terminal raises no caret event for every character typed.
    pub caret: Option<CaretLineQuery<'a>>,
}

/// The answer to a [`ScreenQuery`]. Text is as the provider gave it.
#[derive(Debug, Default)]
pub struct Screen {
    /// The text of the screen, read in one call.
    pub text: String,
    /// The screen's top row, read as a row: the next read's anchor.
    pub top_row: String,
    /// The row after it, empty when there is none.
    pub next_row: String,
    /// Whether the text starts where the screen starts: no history above
    /// it, as on a full-screen program's alternate screen, or in a terminal
    /// whose text still fits on its screen.
    pub alternate: bool,
    /// How many rows the anchor's top row now lies above the screen's top:
    /// how far the text scrolled since the last read. `None` with no anchor
    /// or when it was not found.
    pub shift: Option<u32>,
    /// The first of the rows that went by unread (`shift` less the
    /// query's `seen_rows`), up to the query's `head_wanted`, in one read.
    pub head: String,
    /// How many rows `head` holds.
    pub head_rows: u32,
    /// With the anchor found, the row the old screen's last line was on
    /// (the query's `seen_rows` from the anchor's top row), as it is now.
    pub old_last_row: String,
    /// How many rows the whole text holds, counted when an anchor was
    /// sought and not found (an anchor of two blank rows is not sought).
    pub document_rows: Option<u32>,
    /// Whether the text held still while it was read: its top row and the
    /// walks to its end.
    pub settled: bool,
    /// Whether the terminal's view moved while it was read: the first
    /// visible range started elsewhere at the end of the read, so the rows
    /// read are no longer all on screen.
    pub view_moved: bool,
    /// The caret and its line, when the query asked for them.
    pub caret: Option<CaretAnswer>,
    /// A range at the start of the screen's top row, for the next read's
    /// anchor ([`ScreenAnchor::range`]).
    pub top: Option<IUIAutomationTextRange>,
    /// The text's first row, for the next read's anchor
    /// ([`ScreenAnchor::first`]).
    pub first_row: String,
}

/// The signature both implementations share.
pub type TerminalScreenFn = fn(&Uia, &ScreenQuery<'_>) -> Result<Screen, Error>;

/// A terminal's screen, the one function call sites use: the remote program
/// when `remote` is true, falling back to the classic implementation for
/// this call when the program fails, and the classic implementation alone
/// when `remote` is false. Says which path answered.
///
/// # Errors
///
/// The classic implementation's [`Error`], when it ran and failed.
pub fn terminal_screen(
    uia: &Uia,
    query: &ScreenQuery<'_>,
    remote: bool,
) -> Result<(Screen, Path), Error> {
    if !remote {
        return terminal_screen_classic(uia, query).map(|screen| (screen, Path::Classic));
    }
    match terminal_screen_remote(uia, query) {
        Ok(screen) => Ok((screen, Path::Remote)),
        Err(error) => {
            terminal_screen_classic(uia, query).map(|screen| (screen, Path::Fallback(error)))
        }
    }
}

/// Whether a read settled, as far as its rows' text tells: the screen's text
/// starts with its top row as read on its own, and that row read the same
/// at the end. (The walks that count rows are checked on their own.)
fn is_settled(text: &str, top_row: &str, top_after: &str) -> bool {
    let first = text.split('\n').next().unwrap_or_default();
    let first = first.trim_end();
    top_row == top_after && first.starts_with(top_row.trim_end())
}

/// The rows from the anchor's top row to the screen's top, from the walks
/// to the text's end: `low`, the anchor's walk less the screen's second,
/// is exact when the text did not grow between the screen's two walks
/// (`spread`, how far they differ, is 0). Output written during the walks
/// grew the text by `spread` rows, some before the anchor's walk and some
/// after, so the shift lies from `low` to `low + spread`; rows at a fixed
/// place keep their distance while output is added below them, so it is
/// found by `probe`, which moves from the anchor by that many rows and
/// compares where it lands with the screen's top, halving the range each
/// time. `None` when the text shrank, or no row in the range is the
/// screen's top (the history discarded rows meanwhile): the read is then
/// not trusted. During a flood in the console host the text grows during
/// nearly every read, so a read untrusted for it left the flood unread
/// until it ended.
fn exact_shift(
    low: i32,
    spread: i32,
    mut probe: impl FnMut(i32) -> Result<std::cmp::Ordering, Error>,
) -> Result<Option<i32>, Error> {
    if spread == 0 {
        return Ok(Some(low));
    }
    let (mut from, mut to) = (0, spread);
    while from <= to {
        let mid = from.midpoint(to);
        match probe(low + mid)? {
            std::cmp::Ordering::Equal => return Ok(Some(low + mid)),
            std::cmp::Ordering::Less => from = mid + 1,
            std::cmp::Ordering::Greater => to = mid - 1,
        }
    }
    Ok(None)
}

/// A string result, empty for a null one.
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

/// The program's constants.
struct Constants {
    start: Reg<kind::Int>,
    end: Reg<kind::Int>,
    line: Reg<kind::Int>,
    all: Reg<kind::Int>,
    zero: Reg<kind::Int>,
    one: Reg<kind::Int>,
    back: Reg<kind::Int>,
    far: Reg<kind::Int>,
}

impl Constants {
    fn new(b: &mut Builder) -> Self {
        Self {
            start: b.int(0),
            end: b.int(1),
            line: b.int(TextUnit_Line.0),
            all: b.int(-1),
            zero: b.int(0),
            one: b.int(1),
            back: b.int(-1),
            far: b.int(FAR),
        }
    }

    /// A copy of `range` collapsed to its start.
    fn collapsed(&self, b: &mut Builder, range: Reg<kind::TextRange>) -> Reg<kind::TextRange> {
        let copy = b.text_range_clone(range);
        b.text_range_move_endpoint_by_range(copy, self.end, copy, self.start);
        copy
    }

    /// The rows of the text `document` holds, as [`document_rows`] counts
    /// them.
    fn rows(&self, b: &mut Builder, document: Reg<kind::TextRange>) -> Reg<kind::Int> {
        let walker = self.collapsed(b, document);
        let moved = b.text_range_move(walker, self.line, self.far);
        let rows = b.new_int(1);
        b.add_assign(rows, moved);
        let last = self.row(b, walker);
        let empty = b.text_range_compare_endpoints(last, self.start, last, self.end);
        let empty = b.equal(empty, self.zero);
        let character = b.int(TextUnit_Character.0);
        b.if_(empty, |b| {
            let _ = b.text_range_move_endpoint_by_unit(last, self.start, character, self.back);
            b.text_range_expand_to_enclosing_unit(last, self.line);
        });
        let order = b.text_range_compare_endpoints(walker, self.start, last, self.start);
        let past = b.compare(order, self.zero, Comparison::GreaterThan);
        b.if_(past, |b| b.subtract_assign(rows, self.one));
        rows
    }

    /// A copy of `range` expanded to the row containing its start.
    fn row(&self, b: &mut Builder, range: Reg<kind::TextRange>) -> Reg<kind::TextRange> {
        let copy = b.text_range_clone(range);
        b.text_range_expand_to_enclosing_unit(copy, self.line);
        copy
    }
}

/// The screen in one cross-process round trip.
///
/// `uia` is unused; it is in the signature so that this and
/// [`terminal_screen_classic`] are interchangeable ([`TerminalScreenFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program.
#[expect(
    clippy::too_many_lines,
    reason = "one program, read top to bottom as it runs"
)]
pub fn terminal_screen_remote(_uia: &Uia, query: &ScreenQuery<'_>) -> Result<Screen, Error> {
    let mut b = Builder::new();
    let c = Constants::new(&mut b);
    let element = b.import_element(query.element);
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
    let text = b.text_range_get_text(screen, c.all);
    let text = b.add_to_results(text);
    let top = c.collapsed(&mut b, screen);
    let top_range = b.add_to_results(top);
    let top_row = c.row(&mut b, top);
    let top_text = b.text_range_get_text(top_row, c.all);
    let top_text = b.add_to_results(top_text);
    let next = c.collapsed(&mut b, top);
    let has_next = b.text_range_move(next, c.line, c.one);
    let has_next = b.not_equal(has_next, c.zero);
    let next_text = b.new_string("");
    let next_text = b.add_to_results(next_text);
    // The search ends at the end of the screen's second row, where the old
    // second row is when nothing scrolled.
    let search_end = b.text_range_clone(top_row);
    b.if_(has_next, |b| {
        let row = c.row(b, next);
        let text = b.text_range_get_text(row, c.all);
        b.set(next_text, text);
        b.text_range_move_endpoint_by_range(search_end, c.end, row, c.end);
    });
    let order = b.text_range_compare_endpoints(document, c.start, screen, c.start);
    let alternate = b.equal(order, c.zero);
    let alternate = b.add_to_results(alternate);
    let first = c.collapsed(&mut b, document);
    let first = c.row(&mut b, first);
    let first_now = b.text_range_get_text(first, c.all);
    let first_now = b.add_to_results(first_now);

    let shift = b.new_int(-1);
    let shift = b.add_to_results(shift);
    let head = b.new_string("");
    let head = b.add_to_results(head);
    let head_rows = b.new_int(0);
    let head_rows = b.add_to_results(head_rows);
    let document_rows = b.new_int(-1);
    let document_rows = b.add_to_results(document_rows);
    let old_last_row = b.new_string("");
    let old_last_row = b.add_to_results(old_last_row);
    let walks_agree = b.new_bool(true);
    let walks_agree = b.add_to_results(walks_agree);

    let sought = query
        .anchor
        .as_ref()
        .and_then(|anchor| anchor.sought(query.matches_padding));
    if let Some(anchor) = &query.anchor
        && (anchor.range.is_some() || sought.is_some())
    {
        let found = b.new_bool(false);
        let found_top = b.text_range_clone(top);
        let top_sought = b.string(anchor.top);
        let next_sought = b.string(anchor.next);
        // First where the anchor's range is. Until the terminal discards its
        // oldest rows, a range stays on its row however the rows' text
        // changes; nothing has been discarded while the text's first row
        // reads as it did, or when the old screen had no history above it.
        // A range from the other screen of a terminal that switched screens
        // cannot be compared with the text, which is not found.
        if let Some(range) = anchor.range {
            let range = b.import_text_range(range);
            let first_then = b.string(anchor.first);
            b.try_catch(
                |b| {
                    let at = c.collapsed(b, range);
                    let _ = b.text_range_compare_endpoints(at, c.start, document, c.start);
                    let kept = b.equal(first_now, first_then);
                    if anchor.no_history {
                        let yes = b.bool(true);
                        b.set(kept, yes);
                    }
                    b.if_(kept, |b| {
                        b.set(found_top, at);
                        let yes = b.bool(true);
                        b.set(found, yes);
                    });
                },
                |_, _| {},
            );
        }
        // Then by its text.
        if let Some(sought) = &sought {
            let needle = b.string(&sought.needle);
            let (row_sought, other) = match sought.side {
                Side::Top => (top_sought, next_sought),
                Side::Next => (next_sought, top_sought),
            };
            let other_blank = anchor.next.is_empty() && sought.side == Side::Top;
            let span = b.text_range_clone(document);
            b.text_range_move_endpoint_by_range(span, c.end, search_end, c.end);
            let backward = b.bool(true);
            let ignore_case = b.bool(false);
            let tries = b.new_int(0);
            let most = b.int(i32::try_from(SEARCH_MATCHES).unwrap_or(i32::MAX));
            let not_found = b.not(found);
            b.if_(not_found, |b| {
                // `FindText` has thrown in Windows Terminal: a throw is not
                // found.
                b.try_catch(
                    |b| {
                        b.while_(
                            |b| b.compare(tries, most, Comparison::LessThan),
                            |b| {
                                let hit =
                                    b.text_range_find_text(span, needle, backward, ignore_case);
                                let missing = b.is_null(hit);
                                b.if_(missing, Builder::break_loop);
                                let row = c.row(b, hit);
                                let order =
                                    b.text_range_compare_endpoints(row, c.start, hit, c.start);
                                let starts = b.equal(order, c.zero);
                                let text = b.text_range_get_text(row, c.all);
                                let holds = b.equal(text, row_sought);
                                let candidate = b.and(starts, holds);
                                b.if_(candidate, |b| {
                                    let beside = c.collapsed(b, row);
                                    let step = match sought.side {
                                        Side::Top => c.one,
                                        Side::Next => c.back,
                                    };
                                    let went = b.text_range_move(beside, c.line, step);
                                    let went = b.not_equal(went, c.zero);
                                    let beside_text = b.new_string("");
                                    b.if_(went, |b| {
                                        let row = c.row(b, beside);
                                        let text = b.text_range_get_text(row, c.all);
                                        b.set(beside_text, text);
                                    });
                                    let pairs = b.equal(beside_text, other);
                                    if other_blank {
                                        let yes = b.bool(true);
                                        b.set(pairs, yes);
                                    }
                                    b.if_(pairs, |b| {
                                        let top_of_pair = match sought.side {
                                            Side::Top => c.collapsed(b, row),
                                            Side::Next => beside,
                                        };
                                        b.set(found_top, top_of_pair);
                                        let yes = b.bool(true);
                                        b.set(found, yes);
                                        b.break_loop();
                                    });
                                });
                                // Search again above this match.
                                b.text_range_move_endpoint_by_range(span, c.end, hit, c.start);
                                b.add_assign(tries, c.one);
                            },
                        );
                    },
                    |_, _| {},
                );
            });
        }
        let counts_rows = sought.is_some();
        b.if_else(
            found,
            |b| {
                // Rows from the old top to the end, less the rows from
                // the screen's top to the end, counted before and after
                // ([`exact_shift`]).
                let from_screen = c.collapsed(b, top);
                let screen_to_end = b.text_range_move(from_screen, c.line, c.far);
                let from_old = c.collapsed(b, found_top);
                let to_end = b.text_range_move(from_old, c.line, c.far);
                let again = c.collapsed(b, top);
                let screen_to_end_again = b.text_range_move(again, c.line, c.far);
                let unchanged = b.equal(screen_to_end, screen_to_end_again);
                b.set(walks_agree, unchanged);
                b.set(shift, to_end);
                b.subtract_assign(shift, screen_to_end_again);
                let spread = b.subtract(screen_to_end_again, screen_to_end);
                let grew = b.compare(spread, c.zero, Comparison::GreaterThan);
                b.if_(grew, |b| {
                    // Output written during the walks: the shift is found
                    // by moving from the anchor and comparing with the
                    // screen's top, halving the rows it may lie within.
                    let base = c.collapsed(b, found_top);
                    let _ = b.text_range_move(base, c.line, shift);
                    let low = b.new_int(0);
                    let high = b.new_int(0);
                    b.set(high, spread);
                    let two = b.int(2);
                    b.while_(
                        |b| b.compare(low, high, Comparison::LessThanOrEqual),
                        |b| {
                            let mid = b.add(low, high);
                            b.divide_assign(mid, two);
                            let probe = b.text_range_clone(base);
                            let _ = b.text_range_move(probe, c.line, mid);
                            let order =
                                b.text_range_compare_endpoints(probe, c.start, top, c.start);
                            let at = b.equal(order, c.zero);
                            b.if_(at, |b| {
                                b.add_assign(shift, mid);
                                let yes = b.bool(true);
                                b.set(walks_agree, yes);
                                b.break_loop();
                            });
                            let before = b.compare(order, c.zero, Comparison::LessThan);
                            b.if_else(
                                before,
                                |b| {
                                    b.set(low, mid);
                                    b.add_assign(low, c.one);
                                },
                                |b| {
                                    b.set(high, mid);
                                    b.subtract_assign(high, c.one);
                                },
                            );
                        },
                    );
                });
                let seen = b.int(i32::try_from(query.seen_rows).unwrap_or(i32::MAX));
                // The old screen's last row as it is now, which may have
                // changed since (the line output was being written to).
                if query.seen_rows > 0 {
                    let last = c.collapsed(b, found_top);
                    let down = b.int(i32::try_from(query.seen_rows - 1).unwrap_or(i32::MAX));
                    let _ = b.text_range_move(last, c.line, down);
                    let row = c.row(b, last);
                    let text = b.text_range_get_text(row, c.all);
                    b.set(old_last_row, text);
                }
                // The first rows that went by unread.
                let unread = b.new_int(0);
                b.set(unread, shift);
                b.subtract_assign(unread, seen);
                let wanted = b.int(i32::try_from(query.head_wanted).unwrap_or(i32::MAX));
                let more = b.compare(unread, wanted, Comparison::GreaterThan);
                b.if_(more, |b| b.set(unread, wanted));
                let any = b.compare(unread, c.zero, Comparison::GreaterThan);
                b.if_(any, |b| {
                    let first = c.collapsed(b, found_top);
                    let _ = b.text_range_move(first, c.line, seen);
                    let after = b.text_range_clone(first);
                    let went = b.text_range_move(after, c.line, unread);
                    b.set(head_rows, went);
                    b.text_range_move_endpoint_by_range(first, c.end, after, c.start);
                    let text = b.text_range_get_text(first, c.all);
                    b.set(head, text);
                });
            },
            |b| {
                if counts_rows {
                    let rows = c.rows(b, document);
                    b.set(document_rows, rows);
                }
            },
        );
    }

    let after = c.row(&mut b, top);
    let top_after = b.text_range_get_text(after, c.all);
    let top_after = b.add_to_results(top_after);
    // The screen still where the terminal shows it: a terminal that moved
    // its view while it was read gave the text of rows it no longer shows
    // (output scrolling the view, or the console host moving its view down
    // a row for each line written into a scroll region above a footer).
    let view_moved = b.new_bool(false);
    let view_moved = b.add_to_results(view_moved);
    let ranges_after = b.text_pattern_get_visible_ranges(pattern);
    let size_after = b.array_size(ranges_after);
    let some_after = b.compare(size_after, none, Comparison::GreaterThan);
    b.if_(some_after, |b| {
        let first = b
            .array_get_at(ranges_after, none)
            .assume::<kind::TextRange>();
        let order = b.text_range_compare_endpoints(first, c.start, screen, c.start);
        let moved = b.not_equal(order, c.zero);
        b.set(view_moved, moved);
    });
    let caret = query.caret.map(|caret| emit_caret_line(&mut b, &caret));
    let outcome = b.finish().execute()?;

    let text = string_of(&outcome, text)?;
    let top_row = string_of(&outcome, top_text)?;
    let top_after = string_of(&outcome, top_after)?;
    let shift = outcome.get(shift)?;
    let document_rows = outcome.get(document_rows)?;
    Ok(Screen {
        settled: is_settled(&text, &top_row, &top_after) && outcome.get(walks_agree)?,
        view_moved: outcome.get(view_moved)?,
        text,
        top_row,
        next_row: string_of(&outcome, next_text)?,
        alternate: outcome.get(alternate)?,
        shift: (shift >= 0).then(|| count_of(shift)),
        head: string_of(&outcome, head)?,
        old_last_row: string_of(&outcome, old_last_row)?,
        head_rows: count_of(outcome.get(head_rows)?),
        document_rows: (document_rows >= 0).then(|| count_of(document_rows)),
        caret: caret.map(|caret| caret.read(&outcome)).transpose()?,
        top: outcome.get(top_range)?,
        first_row: string_of(&outcome, first_now)?,
    })
}

/// A copy of `range` collapsed to its start. Two calls.
fn collapsed(range: &IUIAutomationTextRange) -> Result<IUIAutomationTextRange, Error> {
    let copy = range.clone_range()?;
    copy.move_endpoint_to(Endpoint::End, &copy, Endpoint::Start)?;
    Ok(copy)
}

/// A copy of `range` expanded to the row containing its start. Two calls.
fn row(range: &IUIAutomationTextRange) -> Result<IUIAutomationTextRange, Error> {
    let copy = range.clone_range()?;
    copy.expand(TextUnit_Line)?;
    Ok(copy)
}

/// The text of a range.
fn text_of(range: &IUIAutomationTextRange) -> Result<String, Error> {
    Ok(String::from_utf16_lossy(&range.text(-1)?))
}

/// The rows of the whole text: a walk from its start to its end by rows,
/// and the first row.
fn document_rows(document: &IUIAutomationTextRange) -> Result<u32, Error> {
    let walker = collapsed(document)?;
    let moved = walker.move_by(TextUnit_Line, FAR)?;
    // Providers differ in where moving forward by rows stops: on the start
    // of the last row (mockapp) or at the end of the text after it (the
    // terminals); the walk past the last row's start is not a row.
    let last = row(&walker)?;
    if last.compare_endpoints(Endpoint::Start, &last, Endpoint::End)? == 0 {
        last.move_endpoint_by_unit(Endpoint::Start, TextUnit_Character, -1)?;
        last.expand(TextUnit_Line)?;
    }
    let past = walker.compare_endpoints(Endpoint::Start, &last, Endpoint::Start)? > 0;
    Ok(count_of(moved.saturating_add(1) - i32::from(past)))
}

/// The anchor's top row where its range is, the classic way, as the
/// program tries it first: the range, when nothing has been discarded from
/// the text since (its first row, `first_now`, reads as it did, or the old
/// screen had no history above it), or `None`, or when the range cannot be
/// compared with the text (one from the other screen of a terminal that
/// switched screens).
fn classic_at_range(
    range: &IUIAutomationTextRange,
    document: &IUIAutomationTextRange,
    first_now: &str,
    anchor: &ScreenAnchor<'_>,
) -> Option<IUIAutomationTextRange> {
    let check = || -> Result<Option<IUIAutomationTextRange>, Error> {
        let at = collapsed(range)?;
        at.compare_endpoints(Endpoint::Start, document, Endpoint::Start)?;
        Ok((anchor.no_history || first_now == anchor.first).then_some(at))
    };
    check().ok().flatten()
}

/// The search for the anchor the classic way, as the program does it:
/// the top of the pair where it was found, or `None`. A call that fails
/// (Windows Terminal has thrown from `FindText`) is not found.
fn classic_find(
    document: &IUIAutomationTextRange,
    search_end: &IUIAutomationTextRange,
    anchor: &ScreenAnchor<'_>,
    sought: &Sought,
) -> Option<IUIAutomationTextRange> {
    let (row_sought, other) = match sought.side {
        Side::Top => (anchor.top, anchor.next),
        Side::Next => (anchor.next, anchor.top),
    };
    let other_blank = anchor.next.is_empty() && sought.side == Side::Top;
    let needle = sought.needle.as_str();
    let search = || -> Result<Option<IUIAutomationTextRange>, Error> {
        let span = document.clone_range()?;
        span.move_endpoint_to(Endpoint::End, search_end, Endpoint::End)?;
        for _ in 0..SEARCH_MATCHES {
            let Some(hit) = span.find_text(needle, true)? else {
                return Ok(None);
            };
            let found_row = row(&hit)?;
            let starts = found_row.compare_endpoints(Endpoint::Start, &hit, Endpoint::Start)? == 0;
            if starts && text_of(&found_row)? == row_sought {
                let beside = collapsed(&found_row)?;
                let step = match sought.side {
                    Side::Top => 1,
                    Side::Next => -1,
                };
                let beside_text = if beside.move_by(TextUnit_Line, step)? == 0 {
                    String::new()
                } else {
                    text_of(&row(&beside)?)?
                };
                if other_blank || beside_text == other {
                    return Ok(Some(match sought.side {
                        Side::Top => collapsed(&found_row)?,
                        Side::Next => beside,
                    }));
                }
            }
            span.move_endpoint_to(Endpoint::End, &hit, Endpoint::Start)?;
        }
        Ok(None)
    };
    search().ok().flatten()
}

/// The screen the classic way, the fallback and the reference: the same
/// steps as the remote program, each a cross-process call, counted on the
/// thread's `verbatim_uia::calls`.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails.
pub fn terminal_screen_classic(_uia: &Uia, query: &ScreenQuery<'_>) -> Result<Screen, Error> {
    let document = query.pattern.document_range()?;
    let screen = query
        .pattern
        .visible_ranges()?
        .into_iter()
        .next()
        .unwrap_or_else(|| document.clone());
    let text = text_of(&screen)?;
    let top = collapsed(&screen)?;
    let top_row_range = row(&top)?;
    let top_row = text_of(&top_row_range)?;
    let next = collapsed(&top)?;
    let (next_row, search_end) = if next.move_by(TextUnit_Line, 1)? == 0 {
        (String::new(), top_row_range)
    } else {
        let next_range = row(&next)?;
        (text_of(&next_range)?, next_range)
    };
    let alternate = document.compare_endpoints(Endpoint::Start, &screen, Endpoint::Start)? == 0;
    let mut answer = Screen {
        text,
        top_row,
        next_row,
        alternate,
        top: Some(top.clone()),
        first_row: text_of(&row(&collapsed(&document)?)?)?,
        ..Screen::default()
    };
    let mut walks_agree = true;
    if let Some(anchor) = &query.anchor {
        let sought = anchor.sought(query.matches_padding);
        let found = match anchor
            .range
            .and_then(|range| classic_at_range(range, &document, &answer.first_row, anchor))
        {
            Some(found) => Some(found),
            None => sought
                .as_ref()
                .and_then(|sought| classic_find(&document, &search_end, anchor, sought)),
        };
        match found {
            None if sought.is_none() => {}
            Some(found_top) => {
                let screen_to_end = collapsed(&top)?.move_by(TextUnit_Line, FAR)?;
                let to_end = collapsed(&found_top)?.move_by(TextUnit_Line, FAR)?;
                let again = collapsed(&top)?.move_by(TextUnit_Line, FAR)?;
                let low = to_end - again;
                let found = exact_shift(low, again - screen_to_end, |rows| {
                    let probe = collapsed(&found_top)?;
                    probe.move_by(TextUnit_Line, rows)?;
                    Ok(probe
                        .compare_endpoints(Endpoint::Start, &top, Endpoint::Start)?
                        .cmp(&0))
                })?;
                walks_agree = found.is_some();
                let shift = found.unwrap_or(low);
                answer.shift = (shift >= 0).then(|| count_of(shift));
                let seen = i32::try_from(query.seen_rows).unwrap_or(i32::MAX);
                if seen > 0 {
                    let last = collapsed(&found_top)?;
                    last.move_by(TextUnit_Line, seen - 1)?;
                    answer.old_last_row = text_of(&row(&last)?)?;
                }
                let unread =
                    (shift - seen).min(i32::try_from(query.head_wanted).unwrap_or(i32::MAX));
                if unread > 0 {
                    let first = collapsed(&found_top)?;
                    first.move_by(TextUnit_Line, seen)?;
                    let after = first.clone_range()?;
                    let went = after.move_by(TextUnit_Line, unread)?;
                    first.move_endpoint_to(Endpoint::End, &after, Endpoint::Start)?;
                    answer.head = text_of(&first)?;
                    answer.head_rows = count_of(went);
                }
            }
            None => answer.document_rows = Some(document_rows(&document)?),
        }
    }
    let top_after = text_of(&row(&top)?)?;
    // The screen still where the terminal shows it, as the program checks.
    answer.view_moved = match query.pattern.visible_ranges()?.into_iter().next() {
        Some(first) => first.compare_endpoints(Endpoint::Start, &screen, Endpoint::Start)? != 0,
        None => false,
    };
    answer.settled = is_settled(&answer.text, &answer.top_row, &top_after) && walks_agree;
    if let Some(caret) = &query.caret {
        answer.caret = Some(caret_line_classic(caret)?);
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::{ScreenAnchor, Side, Sought, exact_shift, is_settled};

    /// Where `exact_shift` finds the screen's top `shift` rows below the
    /// anchor, and how many probes it took.
    fn search(low: i32, spread: i32, shift: i32) -> (Option<i32>, u32) {
        let mut probes = 0;
        let found = exact_shift(low, spread, |rows| {
            probes += 1;
            Ok(rows.cmp(&shift))
        })
        .expect("a probe never fails here");
        (found, probes)
    }

    #[test]
    fn a_shift_from_walks_that_agree_needs_no_probe() {
        assert_eq!(search(40, 0, 40), (Some(40), 0));
    }

    #[test]
    fn a_shift_from_walks_the_text_grew_between_is_found_wherever_it_lies() {
        for shift in 100..=160 {
            let (found, probes) = search(100, 60, shift);
            assert_eq!(found, Some(shift));
            assert!(probes <= 6, "{probes} probes for a spread of 60");
        }
    }

    #[test]
    fn a_shift_outside_the_walks_or_text_that_shrank_is_not_trusted() {
        assert_eq!(search(100, 10, 120).0, None);
        assert_eq!(search(100, -3, 100), (None, 0));
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "compared with `sought`'s answers, which are options"
    )]
    fn sought(needle: &str, side: Side) -> Option<Sought> {
        Some(Sought {
            needle: needle.to_owned(),
            side,
        })
    }

    fn anchor<'a>(top: &'a str, next: &'a str) -> ScreenAnchor<'a> {
        ScreenAnchor {
            top,
            next,
            range: None,
            first: "",
            no_history: false,
        }
    }

    #[test]
    fn the_more_distinctive_row_is_sought() {
        assert_eq!(
            anchor("ab  \r\n", "abc \r\n").sought(false),
            sought("abc", Side::Next)
        );
        assert_eq!(
            anchor("abc \r\n", "ab  \r\n").sought(false),
            sought("abc", Side::Top)
        );
        assert_eq!(anchor("ab", "cd").sought(false), sought("ab", Side::Top));
        assert_eq!(
            anchor("    \r\n", "x").sought(false),
            sought("x", Side::Next)
        );
        assert_eq!(anchor("x", "").sought(false), sought("x", Side::Top));
        assert_eq!(anchor("  ", "\r\n").sought(false), None);
    }

    #[test]
    fn a_provider_that_matches_padding_is_given_the_padded_row() {
        assert_eq!(
            anchor("line 14   ", "line 15   ").sought(true),
            sought("line 14   ", Side::Top)
        );
        assert_eq!(
            anchor("line 1  \r\n", "line 15 \r\n").sought(true),
            sought("line 15 ", Side::Next)
        );
    }

    #[test]
    fn a_read_settles_only_when_its_top_row_held_still() {
        assert!(is_settled("one  \r\ntwo\r\n", "one  \r\n", "one  \r\n"));
        assert!(is_settled("", "", ""));
        // Scrolled between the screen's read and the row's.
        assert!(!is_settled("two\r\nthree\r\n", "three\r\n", "three\r\n"));
        // Scrolled by the end of the read.
        assert!(!is_settled("one\r\ntwo\r\n", "one\r\n", "two\r\n"));
        // A wrapped top row is the start of the screen's first line.
        assert!(is_settled("abcdefgh\r\n", "abcd", "abcd"));
    }
}
