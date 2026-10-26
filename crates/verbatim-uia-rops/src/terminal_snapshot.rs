//! A terminal's snapshot (`docs/design/terminal-model.md`, section 4.2):
//! the text from where the last read said to start to the end of the
//! document, read in one `GetText`, which the terminal answers from one
//! locked state of its buffer (measured in both terminals, 2026-10-23), so
//! everything spoken or counted comes from one consistent copy. The other
//! calls of a read are hints the caller checks against that copy by
//! content: the screen's text, for its height; where the next read starts,
//! the document's end moved back by the screen's height, which ignores
//! where the user has scrolled the view; whether the start had to be
//! refused because the text shrank or the start lay on the other screen;
//! and the document's first row. After a key, the screen's colours are read
//! too (`crate::terminal_colours`). A remote program reads it in one round
//! trip, more only for a screen of many colours, and a classic
//! implementation behind the same signature reads it call by call;
//! [`terminal_snapshot`] is the one function callers use.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit_Line,
};

use verbatim_uia::Uia;
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::caret::{
    CaretAnswer, CaretLineQuery, CaretLineRegisters, caret_line_classic, emit_caret_line,
    endpoint_number,
};
use crate::error::Error;
use crate::focus::Path;
use crate::opcode::Comparison;
use crate::operation::{Import, Outcome, ProgramCache, Value};
use crate::terminal_colours::{
    ColourBatch, ColourRegisters, ColourRun, colours_classic_read, emit_bounds, emit_colours,
    go_on_remote, row_width, screen_height,
};

/// More rows than any terminal holds, for moving to the end of the text.
const FAR: i32 = 1_000_000;

/// A string result, empty for a null one.
fn string_of(outcome: &Outcome, reg: Reg<kind::Str>) -> Result<String, Error> {
    Ok(match outcome.get(reg.any())? {
        Value::String(text) => text,
        _ => String::new(),
    })
}

/// The program's constants.
struct Constants {
    start: Reg<kind::Int>,
    end: Reg<kind::Int>,
    line: Reg<kind::Int>,
    all: Reg<kind::Int>,
    zero: Reg<kind::Int>,
}

impl Constants {
    fn new(b: &mut Builder) -> Self {
        Self {
            start: b.int(0),
            end: b.int(1),
            line: b.int(TextUnit_Line.0),
            all: b.int(-1),
            zero: b.int(0),
        }
    }

    /// A copy of `range` collapsed to its start.
    fn collapsed(&self, b: &mut Builder, range: Reg<kind::TextRange>) -> Reg<kind::TextRange> {
        let copy = b.text_range_clone(range);
        b.text_range_move_endpoint_by_range(copy, self.end, copy, self.start);
        copy
    }

    /// A copy of `range` expanded to the row containing its start.
    fn row(&self, b: &mut Builder, range: Reg<kind::TextRange>) -> Reg<kind::TextRange> {
        let copy = b.text_range_clone(range);
        b.text_range_expand_to_enclosing_unit(copy, self.line);
        copy
    }
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

/// What [`terminal_snapshot`] and its two implementations are asked
/// (`docs/design/terminal-model.md`, section 4.2).
#[derive(Clone, Copy)]
pub struct SnapshotQuery<'a> {
    /// The element with the text.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern.
    pub pattern: &'a IUIAutomationTextPattern,
    /// Where the last read said to start ([`Snapshot::next_start`]); `None`
    /// for a first read, which starts at the screen's top.
    pub start: Option<&'a IUIAutomationTextRange>,
    /// The screen's height in rows, as the last read found it
    /// ([`Snapshot::screen`]); 0 when not known, for a first read.
    pub screen_rows: u32,
    /// Whether to read the whole text instead, from the document's start:
    /// the fallback when the screen as last read is not in a snapshot
    /// (`docs/design/terminal-model.md`, section 4.2).
    pub whole: bool,
    /// How many rows above the start to begin the read, as far as the
    /// document's start allows: the history rows whose place shows how far
    /// a full history rotated (`docs/design/terminal-model.md`, section 4.3,
    /// item 5). Ignored with `whole`.
    pub above: u32,
    /// The caret and its line to read too, in the same round trip.
    pub caret: Option<CaretLineQuery<'a>>,
    /// Whether to read the screen's colours too, a read in a key's window
    /// (`docs/design/terminal-model.md`, "Lists and colours"), with the
    /// screen's width in cells as the last read found it, 0 when not
    /// known: the first rows' colours are read in the snapshot's own
    /// program at that width, and read again at this read's width, its top
    /// row's, when the screen's turns out otherwise.
    pub colours: Option<usize>,
}

/// The answer to a [`SnapshotQuery`]. Only [`Self::text`] is read in one
/// provider call, which the terminal answers from one locked state of its
/// buffer (measured 2026-10-23 in both terminals); everything else is a
/// hint, read in calls of its own, which the terminal may write between.
#[derive(Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent hints of one read, each read from its own call"
)]
pub struct Snapshot {
    /// The text from the start to the document's end, UTF-16 as the
    /// provider gave it, padding and line breaks included: everything
    /// spoken or counted comes from it.
    pub text: Vec<u16>,
    /// The first visible range's text, for the screen's height in rows.
    pub screen: Vec<u16>,
    /// Whether the snapshot started where the query's start is. False for
    /// a first read, for a whole-text read, and when the start could not
    /// be used: it lay below where the next read would start, so the text
    /// shrank (the screen cleared, or a full-screen program's alternate
    /// screen opened or closed), or the provider could not compare it with
    /// the text (a range on the other screen's rows); the snapshot then
    /// started at the screen's top.
    pub from_start: bool,
    /// Whether the start was refused because the text shrank or the range
    /// was on the other screen: evidence of a clear or a screen switch.
    pub shrank: bool,
    /// Whether the snapshot starts at the document's start: no text above
    /// it.
    pub at_document_start: bool,
    /// Whether the screen starts at the document's start: no history above
    /// it.
    pub alternate: bool,
    /// How many rows the caret's row and those below it hold, to the
    /// document's end: a blank caret row is the line that many from the
    /// snapshot's end, which its text cannot tell (measured in both
    /// terminals, 2026-10-23). 0 when the provider reports no caret.
    pub caret_rows_to_end: u32,
    /// The document's first row, as the provider gave it: when it reads
    /// otherwise, the history rotated (`docs/design/terminal-model.md`,
    /// section 2.1).
    pub first_row: String,
    /// Where the next read starts: the document's end moved back by the
    /// screen's height, so the top of the screen output is written to,
    /// wherever the user has scrolled the view (`docs/design/terminal-model.md`,
    /// section 4.2); the screen's top when the height is not known.
    pub next_start: Option<IUIAutomationTextRange>,
    /// How many rows the snapshot holds above its start: the query's
    /// `above`, or fewer when the document's start came first.
    pub above: u32,
    /// The caret and its line, when the query asked for them.
    pub caret: Option<CaretAnswer>,
    /// The screen's colours, when the query asked for them: each row's runs
    /// of one foreground and background, from the screen's top to the
    /// document's end, at most the screen's height, the form the terminal
    /// reading rules take. Read after the text, in calls of their own, so a
    /// hint like the rest.
    pub colours: Option<Vec<Vec<ColourRun>>>,
}

/// The signature both snapshot implementations share.
pub type TerminalSnapshotFn = fn(&Uia, &SnapshotQuery<'_>) -> Result<Snapshot, Error>;

/// A terminal's snapshot, the one function call sites use: the remote
/// program when `remote` is true, falling back to the classic
/// implementation for this call when the program fails, and the classic
/// implementation alone when `remote` is false. Says which path answered.
///
/// # Errors
///
/// The classic implementation's [`Error`], when it ran and failed.
pub fn terminal_snapshot(
    uia: &Uia,
    query: &SnapshotQuery<'_>,
    remote: bool,
) -> Result<(Snapshot, Path), Error> {
    if !remote {
        return terminal_snapshot_classic(uia, query).map(|snapshot| (snapshot, Path::Classic));
    }
    match terminal_snapshot_remote(uia, query) {
        Ok(snapshot) => Ok((snapshot, Path::Remote)),
        Err(error) => {
            terminal_snapshot_classic(uia, query).map(|snapshot| (snapshot, Path::Fallback(error)))
        }
    }
}

/// Everything the snapshot program is built from but its imports.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SnapshotKey {
    start: bool,
    screen_rows: u32,
    whole: bool,
    above: u32,
    /// Whether the caret is read, with `TextPattern2`, at most how much of
    /// its line, and the end of `since` it is compared with.
    caret: Option<(bool, i32, Option<i32>)>,
    /// Whether colours are read, and at what width.
    colours: Option<usize>,
}

impl SnapshotKey {
    fn of(query: &SnapshotQuery<'_>) -> Self {
        Self {
            start: query.start.is_some(),
            screen_rows: query.screen_rows,
            whole: query.whole,
            above: query.above,
            caret: query.caret.map(|caret| {
                (
                    caret.pattern2.is_some(),
                    caret.max_text,
                    caret.since.map(|since| endpoint_number(since.endpoint)),
                )
            }),
            colours: query.colours,
        }
    }
}

/// The registers the snapshot program's answer is read from.
#[derive(Clone, Copy)]
struct SnapshotRegisters {
    text: Reg<kind::Str>,
    screen: Reg<kind::Str>,
    from_start: Reg<kind::Bool>,
    shrank: Reg<kind::Bool>,
    at_document_start: Reg<kind::Bool>,
    alternate: Reg<kind::Bool>,
    first_row: Reg<kind::Str>,
    caret_rows_to_end: Reg<kind::Int>,
    next_start: Reg<kind::TextRange>,
    /// How far the start moved up, a count of rows not above zero; none
    /// when no rows above the start were asked for.
    above: Option<Reg<kind::Int>>,
    caret: Option<CaretLineRegisters>,
    /// The colour read's start and limit, and its first rows' registers
    /// when the width was known.
    colours: Option<SnapshotColours>,
}

/// The snapshot program's colour registers.
#[derive(Clone, Copy)]
struct SnapshotColours {
    top: Reg<kind::TextRange>,
    top_row: Reg<kind::Str>,
    limit: Reg<kind::TextRange>,
    rows: Option<ColourRegisters>,
}

thread_local! {
    /// The snapshot programs this thread built.
    static SNAPSHOTS: ProgramCache<SnapshotKey, SnapshotRegisters> = ProgramCache::default();
}

/// The snapshot program's imports, in the order it imports them: the
/// element, the start when given, and the caret's `since`.
fn snapshot_imports(query: &SnapshotQuery<'_>) -> Vec<Import> {
    let mut imports = vec![Import::Element(query.element.clone())];
    if let Some(start) = query.start {
        imports.push(Import::TextRange(start.clone()));
    }
    if let Some(since) = query.caret.and_then(|caret| caret.since) {
        imports.push(Import::TextRange(since.range.clone()));
    }
    imports
}

/// The snapshot in one cross-process round trip, the program built once for
/// each [`SnapshotKey`] on a thread and run again with the next read's
/// imports.
///
/// `uia` is unused; it is in the signature so that this and
/// [`terminal_snapshot_classic`] are interchangeable
/// ([`TerminalSnapshotFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn terminal_snapshot_remote(_uia: &Uia, query: &SnapshotQuery<'_>) -> Result<Snapshot, Error> {
    let (operation, r) = SNAPSHOTS.with(|programs| {
        programs.operation(
            SnapshotKey::of(query),
            || snapshot_imports(query),
            |b| build_snapshot(b, query),
        )
    });
    let outcome = operation.execute()?;
    let screen = outcome.get_utf16(r.screen)?;
    let colours = match (r.colours, query.colours) {
        (Some(registers), Some(hint)) => Some(colours_remote(&outcome, registers, hint, &screen)?),
        _ => None,
    };
    Ok(Snapshot {
        text: outcome.get_utf16(r.text)?,
        screen,
        from_start: outcome.get(r.from_start)?,
        shrank: outcome.get(r.shrank)?,
        at_document_start: outcome.get(r.at_document_start)?,
        alternate: outcome.get(r.alternate)?,
        first_row: string_of(&outcome, r.first_row)?,
        caret_rows_to_end: u32::try_from(outcome.get(r.caret_rows_to_end)?).unwrap_or(0),
        next_start: outcome.get(r.next_start)?,
        above: match r.above {
            Some(moved) => u32::try_from(-outcome.get(moved)?).unwrap_or(0),
            None => 0,
        },
        caret: r.caret.map(|caret| caret.read(&outcome)).transpose()?,
        colours,
    })
}

/// The colours the snapshot's program read and those the programs after it
/// read: at this read's width, from where the snapshot's program stopped,
/// or from the screen's top when it read none or read them at another
/// width.
fn colours_remote(
    outcome: &Outcome,
    registers: SnapshotColours,
    hint: usize,
    screen: &[u16],
) -> Result<Vec<Vec<ColourRun>>, Error> {
    let width = row_width(&outcome.get_utf16(registers.top_row)?);
    let size = (width, screen_height(screen, width));
    let mut rows = Vec::new();
    if size.0 == 0 {
        return Ok(rows);
    }
    let Some(limit) = outcome.get(registers.limit)? else {
        return Ok(rows);
    };
    let batch = match registers.rows {
        Some(first) if hint == size.0 => first.read(outcome, size.0, &mut rows)?,
        _ => ColourBatch {
            row: outcome.get(registers.top)?,
            done: false,
        },
    };
    go_on_remote(batch, &limit, size, &mut rows)?;
    Ok(rows)
}

/// Emits the snapshot's program into `b`: the screen and where the next
/// read starts, then the start, then the one `GetText` of the snapshot,
/// then the caret.
#[expect(
    clippy::too_many_lines,
    reason = "one program, read top to bottom as it runs"
)]
fn build_snapshot(b: &mut Builder, query: &SnapshotQuery<'_>) -> SnapshotRegisters {
    let element = b.import_element(query.element);
    let kept = query.start.map(|range| b.import_text_range(range));
    let since = query
        .caret
        .and_then(|caret| caret.since)
        .map(|since| b.import_text_range(since.range));
    let c = Constants::new(b);
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
    let screen_text = b.text_range_get_text(screen, c.all);
    let screen_text = b.add_to_results(screen_text);
    let order = b.text_range_compare_endpoints(document, c.start, screen, c.start);
    let alternate = b.equal(order, c.zero);
    let alternate = b.add_to_results(alternate);

    // Where the next read starts.
    let next = if query.screen_rows > 0 {
        let next = b.text_range_clone(document);
        b.text_range_move_endpoint_by_range(next, c.start, document, c.end);
        let back = b.int(-i32::try_from(query.screen_rows).unwrap_or(i32::MAX));
        let _ = b.text_range_move(next, c.line, back);
        next
    } else {
        c.collapsed(b, screen)
    };
    let next_start = b.add_to_results(next);

    // Where this read starts.
    let start = if query.whole {
        c.collapsed(b, document)
    } else {
        c.collapsed(b, screen)
    };
    if query.screen_rows > 0 && !query.whole {
        b.set(start, next);
    }
    let from_start = b.new_bool(false);
    let from_start = b.add_to_results(from_start);
    let shrank = b.new_bool(false);
    let shrank = b.add_to_results(shrank);
    if let Some(kept) = kept.filter(|_| !query.whole) {
        b.try_catch(
            |b| {
                let at = c.collapsed(b, kept);
                let order = b.text_range_compare_endpoints(at, c.start, next, c.start);
                let above = b.compare(order, c.zero, Comparison::LessThanOrEqual);
                b.if_else(
                    above,
                    |b| {
                        b.set(start, at);
                        let yes = b.bool(true);
                        b.set(from_start, yes);
                    },
                    |b| {
                        let yes = b.bool(true);
                        b.set(shrank, yes);
                    },
                );
            },
            |b, _status| {
                let yes = b.bool(true);
                b.set(shrank, yes);
            },
        );
    }

    // The rows above the start, as far as the document's start.
    let (start, above) = if query.above > 0 && !query.whole {
        let begin = b.text_range_clone(start);
        let back = b.int(-i32::try_from(query.above).unwrap_or(i32::MAX));
        let moved = b.text_range_move(begin, c.line, back);
        let above = b.new_int(0);
        let above = b.add_to_results(above);
        b.set(above, moved);
        (begin, Some(above))
    } else {
        (start, None)
    };

    // The snapshot: one call.
    let snapshot = b.text_range_clone(start);
    b.text_range_move_endpoint_by_range(snapshot, c.end, document, c.end);
    let text = b.text_range_get_text(snapshot, c.all);
    let text = b.add_to_results(text);
    let order = b.text_range_compare_endpoints(start, c.start, document, c.start);
    let at_document_start = b.equal(order, c.zero);
    let at_document_start = b.add_to_results(at_document_start);
    let first = c.collapsed(b, document);
    let first = c.row(b, first);
    let first_row = b.text_range_get_text(first, c.all);
    let first_row = b.add_to_results(first_row);
    let caret_rows_to_end = b.new_int(0);
    let caret_rows_to_end = b.add_to_results(caret_rows_to_end);
    let selection = b.text_pattern_get_selection(pattern);
    let size = b.array_size(selection);
    let some = b.compare(size, none, Comparison::GreaterThan);
    b.if_(some, |b| {
        let first = b.array_get_at(selection, none).assume::<kind::TextRange>();
        let walker = c.collapsed(b, first);
        let far = b.int(FAR);
        let moved = b.text_range_move(walker, c.line, far);
        b.set(caret_rows_to_end, moved);
    });

    let caret = query
        .caret
        .map(|caret| emit_caret_line(b, &caret, (element, pattern, since)));
    // The colours last, after the text they describe.
    let colours = query.colours.map(|width| {
        let (top, limit, top_row) = emit_bounds(b, screen, document);
        SnapshotColours {
            top,
            top_row,
            limit,
            rows: (width > 0).then(|| emit_colours(b, (top, limit), width)),
        }
    });
    SnapshotRegisters {
        text,
        screen: screen_text,
        from_start,
        shrank,
        at_document_start,
        alternate,
        first_row,
        caret_rows_to_end,
        next_start,
        above,
        caret,
        colours,
    }
}

/// The snapshot the classic way, the fallback and the reference: the same
/// steps as the remote program, each a cross-process call, counted on the
/// thread's `verbatim_uia::calls`.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails.
pub fn terminal_snapshot_classic(_uia: &Uia, query: &SnapshotQuery<'_>) -> Result<Snapshot, Error> {
    let document = query.pattern.document_range()?;
    let screen = query
        .pattern
        .visible_ranges()?
        .into_iter()
        .next()
        .unwrap_or_else(|| document.clone());
    let screen_text = screen.text(-1)?;
    let alternate = document.compare_endpoints(Endpoint::Start, &screen, Endpoint::Start)? == 0;
    let next = if query.screen_rows > 0 {
        let next = document.clone_range()?;
        next.move_endpoint_to(Endpoint::Start, &document, Endpoint::End)?;
        next.move_by(
            TextUnit_Line,
            -i32::try_from(query.screen_rows).unwrap_or(i32::MAX),
        )?;
        next
    } else {
        collapsed(&screen)?
    };
    let mut start = if query.whole {
        collapsed(&document)?
    } else {
        collapsed(&screen)?
    };
    if query.screen_rows > 0 && !query.whole {
        start = next.clone_range()?;
    }
    let (mut from_start, mut shrank) = (false, false);
    if let Some(kept) = query.start.filter(|_| !query.whole) {
        let at = || -> Result<(IUIAutomationTextRange, i32), Error> {
            let at = collapsed(kept)?;
            let order = at.compare_endpoints(Endpoint::Start, &next, Endpoint::Start)?;
            Ok((at, order))
        };
        match at() {
            Ok((at, order)) if order <= 0 => {
                start = at;
                from_start = true;
            }
            Ok(_) | Err(_) => shrank = true,
        }
    }
    let mut above = 0;
    if query.above > 0 && !query.whole {
        let begin = start.clone_range()?;
        let moved = begin.move_by(
            TextUnit_Line,
            -i32::try_from(query.above).unwrap_or(i32::MAX),
        )?;
        start = begin;
        above = u32::try_from(-moved).unwrap_or(0);
    }
    let snapshot = start.clone_range()?;
    snapshot.move_endpoint_to(Endpoint::End, &document, Endpoint::End)?;
    let text = snapshot.text(-1)?;
    let at_document_start =
        start.compare_endpoints(Endpoint::Start, &document, Endpoint::Start)? == 0;
    let first_row = text_of(&row(&collapsed(&document)?)?)?;
    let caret_rows_to_end = match query.pattern.selection()?.into_iter().next() {
        Some(caret) => u32::try_from(collapsed(&caret)?.move_by(TextUnit_Line, FAR)?).unwrap_or(0),
        None => 0,
    };
    let caret = query.caret.as_ref().map(caret_line_classic).transpose()?;
    let colours = query
        .colours
        .map(|_| colours_classic_read(&screen, &document, &screen_text))
        .transpose()?;
    Ok(Snapshot {
        text,
        screen: screen_text,
        from_start,
        shrank,
        at_document_start,
        alternate,
        first_row,
        caret_rows_to_end,
        next_start: Some(next),
        above,
        caret,
        colours,
    })
}
