//! A caret key's watch check (`docs/design/readers-tell-core-asks-build1.md`,
//! "The watch"): the caret and the selection compared with the key's
//! starting point, the caret's line, the word at the caret when a deleting
//! key compares it, and, when the caret or the selection moved or the
//! check is the last before the bound, the key's unit at the caret for its
//! final read, all in one remote program where the provider runs them, so
//! a key whose move a check finds costs one round trip, not two. The
//! caret is read again at the end, so a check whose caret moved while it
//! read is known to be torn.
//!
//! The text comparisons of the effect test (the characters beside the
//! caret, the line, the word) are made by the caller on the texts this
//! returns, since they depend on how the caller cuts its text; a check
//! that shows the effect only by them, with the caret and the selection
//! where they were, has its unit read by the caller afterwards.
//!
//! The formatting of the final read's span (`FormatSpan`: the character,
//! the unit, or the line), with the attributes the request names, is read
//! in the same program, under the same condition as the unit, so a key's
//! answer carries what its unit's formatting says, as NVDA speaks a caret
//! key's unit with its formatting (`speech/speech.py` 904 to 922).
//!
//! [`watch_check`] is the one function callers use; a classic
//! implementation behind the same signature makes the same reads call by
//! call.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit,
    TextUnit_Character, TextUnit_Line, TextUnit_Word,
};

use verbatim_uia::text::{Endpoint, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::caret::{
    Attributes, FormatSpan, Run, RunRegisters, TextAttribute, classic_runs, emit_runs,
};
use crate::caret::{
    CaretQuery, CaretRegisters, Constants, RangeEnd, UnitRead, UnitRegisters, classic_caret,
    collapsed, emit_caret, emit_caret_with, endpoint_number, gone_or_timed_out, unit_at,
};
use crate::error::Error;
use crate::focus::Path;
use crate::operation::{Import, ProgramCache};

/// What a watch check is asked.
#[derive(Clone, Copy)]
pub struct WatchQuery<'a> {
    /// The element with the text, which the program starts from.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern, which the classic implementation reads.
    pub pattern: &'a IUIAutomationTextPattern,
    /// The caret at the key's starting point.
    pub start: RangeEnd<'a>,
    /// The selection's ends at the starting point: both the caret when
    /// nothing was selected.
    pub start_selection: (RangeEnd<'a>, RangeEnd<'a>),
    /// Whether to read the word at the caret, which a deleting key's check
    /// compares once 30 ms have passed since its pass.
    pub word: bool,
    /// The key's unit for its final read, read when the caret or the
    /// selection moved, or when [`last`](Self::last); `None` for a unit
    /// the line already gives (the line, a character).
    pub unit: Option<TextUnit>,
    /// Whether this is the check at the bound, whose final read is made
    /// whatever it shows.
    pub last: bool,
    /// Whose formatting the final read reads, with `attributes`, read when
    /// the unit is; `None` for none.
    pub formats: Option<FormatSpan>,
    /// The attributes to read.
    pub attributes: Attributes,
    /// The most UTF-16 code units of text read for a line or a unit.
    pub max_text: i32,
}

/// The answer to a [`WatchQuery`].
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a separate fact the check found"
)]
pub struct WatchAnswer {
    /// A range whose start is the caret.
    pub caret: IUIAutomationTextRange,
    /// Whether `caret` is known to be collapsed.
    pub collapsed: bool,
    /// The selection, when text is selected.
    pub selection: Option<IUIAutomationTextRange>,
    /// Whether the caret is not at the starting point's caret: a compare
    /// the provider refuses counts as a move.
    pub caret_moved: bool,
    /// Whether the selection's ends (both the caret when nothing is
    /// selected) are not the starting point's; a refused compare counts
    /// as a move.
    pub selection_moved: bool,
    /// The caret's line.
    pub line: UnitRead,
    /// The word at the caret, when asked.
    pub word: Option<UnitRead>,
    /// The key's unit at the caret, when asked and read.
    pub unit: Option<UnitRead>,
    /// The formatting of [`WatchQuery::formats`]'s span, stretch by
    /// stretch, when asked and read, the unit's condition; `None` when not
    /// read.
    pub runs: Option<Vec<Run>>,
    /// Whether the caret read again at the end is not where it was first
    /// read.
    pub moved_while_read: bool,
}

/// The signature both implementations share.
pub type WatchCheckFn = fn(&WatchQuery<'_>) -> Result<WatchAnswer, Error>;

/// The watch check, the one function call sites use: the remote program
/// when `remote` is true, falling back to the classic implementation for
/// this call when the program fails, and the classic implementation alone
/// when `remote` is false. Says which path answered.
///
/// # Errors
///
/// The classic implementation's [`Error`], when it ran and failed.
pub fn watch_check(query: &WatchQuery<'_>, remote: bool) -> Result<(WatchAnswer, Path), Error> {
    if !remote {
        return watch_check_classic(query).map(|answer| (answer, Path::Classic));
    }
    match watch_check_remote(query) {
        Ok(answer) => Ok((answer, Path::Remote)),
        Err(error) if gone_or_timed_out(&error) => Err(error),
        Err(error) => watch_check_classic(query).map(|answer| (answer, Path::Fallback(error))),
    }
}

/// Everything the program is built from but its imports.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProgramKey {
    start: i32,
    start_selection: (i32, i32),
    word: bool,
    unit: Option<i32>,
    last: bool,
    formats: Option<FormatSpan>,
    attributes: Attributes,
    max_text: i32,
}

impl ProgramKey {
    fn of(query: &WatchQuery<'_>) -> Self {
        Self {
            start: endpoint_number(query.start.endpoint),
            start_selection: (
                endpoint_number(query.start_selection.0.endpoint),
                endpoint_number(query.start_selection.1.endpoint),
            ),
            word: query.word,
            unit: query.unit.map(|unit| unit.0),
            last: query.last,
            formats: query.formats,
            attributes: query.attributes,
            max_text: query.max_text,
        }
    }
}

/// The program's imports, in the order it imports them: the element, the
/// starting caret, and the starting selection's ends.
fn imports(query: &WatchQuery<'_>) -> Vec<Import> {
    vec![
        Import::Element(query.element.clone()),
        Import::TextRange(query.start.range.clone()),
        Import::TextRange(query.start_selection.0.range.clone()),
        Import::TextRange(query.start_selection.1.range.clone()),
    ]
}

/// The registers the answer is read from.
#[derive(Clone)]
struct Registers {
    caret: CaretRegisters,
    caret_moved: Reg<kind::Bool>,
    selection_moved: Reg<kind::Bool>,
    line: UnitRegisters,
    word: Option<UnitRegisters>,
    unit: Option<(Reg<kind::Bool>, UnitRegisters)>,
    runs: Option<(Reg<kind::Bool>, RunRegisters)>,
    moved_while_read: Reg<kind::Bool>,
}

thread_local! {
    /// The watch check programs this thread built.
    static PROGRAMS: ProgramCache<ProgramKey, Registers> = ProgramCache::default();
}

/// Emits whether `(range, endpoint)` and `(known, known_endpoint)` differ
/// into `differs`, which a compare the provider refuses leaves true.
fn emit_differs(
    b: &mut Builder,
    c: &Constants,
    differs: Reg<kind::Bool>,
    (range, endpoint): (Reg<kind::TextRange>, Reg<kind::Int>),
    (known, known_endpoint): (Reg<kind::TextRange>, Reg<kind::Int>),
) {
    let yes = b.bool(true);
    b.set(differs, yes);
    b.try_catch(
        |b| {
            let order = b.text_range_compare_endpoints(range, endpoint, known, known_endpoint);
            let differ = b.not_equal(order, c.zero);
            b.set(differs, differ);
        },
        |_, _| {},
    );
}

/// Emits the final read's unit, read once the caret or the selection
/// moved, or at the bound; the word already read when it is the word.
fn emit_unit(
    b: &mut Builder,
    c: &Constants,
    query: &WatchQuery<'_>,
    (point, unit): (Reg<kind::TextRange>, TextUnit),
    (caret_moved, selection_moved, word): (Reg<kind::Bool>, Reg<kind::Bool>, Option<UnitRegisters>),
) -> (Reg<kind::Bool>, UnitRegisters) {
    let read = b.new_bool(query.last);
    let read = b.add_to_results(read);
    let moved = b.or(caret_moved, selection_moved);
    b.or_assign(read, moved);
    let range = b.new_null().assume::<kind::TextRange>();
    let text = b.new_string("");
    let offset = b.new_uint(0);
    let registers = UnitRegisters {
        range: b.add_to_results(range),
        text: b.add_to_results(text),
        offset: b.add_to_results(offset),
    };
    let same_as_word = query.word && unit == TextUnit_Word;
    b.if_(read, |b| {
        let found = match word {
            Some(word) if same_as_word => word,
            _ => {
                let unit = b.int(unit.0);
                c.unit_at(b, point, unit)
            }
        };
        b.set(registers.range, found.range);
        b.set(registers.text, found.text);
        b.set(registers.offset, found.offset);
    });
    (read, registers)
}

fn build(b: &mut Builder, query: &WatchQuery<'_>) -> Registers {
    let element = b.import_element(query.element);
    let start = b.import_text_range(query.start.range);
    let was_start = b.import_text_range(query.start_selection.0.range);
    let was_end = b.import_text_range(query.start_selection.1.range);
    let c = Constants {
        start: b.int(endpoint_number(Endpoint::Start)),
        end: b.int(endpoint_number(Endpoint::End)),
        zero: b.int(0),
        one: b.int(1),
        max_text: b.int(query.max_text),
    };
    let start_endpoint = b.int(endpoint_number(query.start.endpoint));
    let was_start_endpoint = b.int(endpoint_number(query.start_selection.0.endpoint));
    let was_end_endpoint = b.int(endpoint_number(query.start_selection.1.endpoint));
    let pattern = b.get_text_pattern(element, false);
    let caret = emit_caret(b, &c, pattern);
    let point = c.collapsed(b, caret.caret);

    let caret_moved = b.new_bool(true);
    let caret_moved = b.add_to_results(caret_moved);
    emit_differs(
        b,
        &c,
        caret_moved,
        (point, c.start),
        (start, start_endpoint),
    );

    // The selection's ends, both the caret when nothing is selected.
    let selection_moved = b.new_bool(true);
    let selection_moved = b.add_to_results(selection_moved);
    let first = b.new_bool(true);
    let second = b.new_bool(true);
    b.if_else(
        caret.has_selection,
        |b| {
            emit_differs(
                b,
                &c,
                first,
                (caret.selection, c.start),
                (was_start, was_start_endpoint),
            );
            emit_differs(
                b,
                &c,
                second,
                (caret.selection, c.end),
                (was_end, was_end_endpoint),
            );
        },
        |b| {
            emit_differs(
                b,
                &c,
                first,
                (point, c.start),
                (was_start, was_start_endpoint),
            );
            emit_differs(b, &c, second, (point, c.start), (was_end, was_end_endpoint));
        },
    );
    let either = b.or(first, second);
    b.set(selection_moved, either);

    let line_unit = b.int(TextUnit_Line.0);
    let line = c.unit_at(b, point, line_unit);
    let word = query.word.then(|| {
        let word_unit = b.int(TextUnit_Word.0);
        c.unit_at(b, point, word_unit)
    });
    let unit = query.unit.map(|unit| {
        emit_unit(
            b,
            &c,
            query,
            (point, unit),
            (caret_moved, selection_moved, word),
        )
    });

    let runs = emit_formats(
        b,
        &c,
        query,
        (point, &line, unit.as_ref().map(|(_, unit)| *unit)),
        (caret_moved, selection_moved),
    );

    // The caret read again, once everything else is read.
    let again = emit_caret_with(b, &c, pattern, false);
    let moved_while_read = b.new_bool(true);
    let moved_while_read = b.add_to_results(moved_while_read);
    emit_differs(
        b,
        &c,
        moved_while_read,
        (again.caret, c.start),
        (point, c.start),
    );

    Registers {
        caret,
        caret_moved,
        selection_moved,
        line,
        word,
        unit,
        runs,
        moved_while_read,
    }
}

/// Emits the formatting read of the query's span, read under the unit's
/// condition: once the caret or the selection moved, or at the bound.
fn emit_formats(
    b: &mut Builder,
    c: &Constants,
    query: &WatchQuery<'_>,
    (point, line, unit): (Reg<kind::TextRange>, &UnitRegisters, Option<UnitRegisters>),
    (caret_moved, selection_moved): (Reg<kind::Bool>, Reg<kind::Bool>),
) -> Option<(Reg<kind::Bool>, RunRegisters)> {
    let ids: Vec<TextAttribute> = query.attributes.iter().collect();
    let span = query.formats.filter(|_| !ids.is_empty())?;
    let read = b.new_bool(query.last);
    let read = b.add_to_results(read);
    let moved = b.or(caret_moved, selection_moved);
    b.or_assign(read, moved);
    let mut runs = None;
    b.if_(read, |b| {
        let (range, text, character) = match span {
            FormatSpan::Line => (line.range, line.text, false),
            FormatSpan::Unit => unit.map_or((line.range, line.text, false), |unit| {
                (unit.range, unit.text, false)
            }),
            FormatSpan::Character => {
                let character = b.text_range_clone(point);
                let unit = b.int(TextUnit_Character.0);
                b.text_range_expand_to_enclosing_unit(character, unit);
                (character, line.text, true)
            }
        };
        runs = Some(emit_runs(
            b,
            c,
            (range, text),
            character,
            &ids,
            Attributes::NONE,
        ));
    });
    Some((read, runs.expect("the branch's body is emitted")))
}

/// The watch check in one cross-process round trip. The program is built
/// once for each [`ProgramKey`] on a thread and run again with the next
/// check's imports.
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn watch_check_remote(query: &WatchQuery<'_>) -> Result<WatchAnswer, Error> {
    let (operation, r) = PROGRAMS.with(|programs| {
        programs.operation(
            ProgramKey::of(query),
            || imports(query),
            |b| build(b, query),
        )
    });
    let outcome = operation.execute()?;
    let caret = outcome
        .get(r.caret.caret)?
        .ok_or(Error::MissingResult(r.caret.caret.id()))?;
    let selection = if outcome.get(r.caret.has_selection)? {
        outcome.get(r.caret.selection)?
    } else {
        None
    };
    let unit = match r.unit {
        Some((read, unit)) if outcome.get(read)? => Some(unit.read(&outcome)?),
        _ => None,
    };
    let runs = match &r.runs {
        Some((read, runs)) if outcome.get(*read)? => Some(runs.read(&outcome)?),
        _ => None,
    };
    Ok(WatchAnswer {
        caret,
        collapsed: outcome.get(r.caret.collapsed)?,
        selection,
        caret_moved: outcome.get(r.caret_moved)?,
        selection_moved: outcome.get(r.selection_moved)?,
        line: r.line.read(&outcome)?,
        word: r.word.map(|word| word.read(&outcome)).transpose()?,
        unit,
        runs,
        moved_while_read: outcome.get(r.moved_while_read)?,
    })
}

/// Whether `(range, endpoint)` and `known` differ, the classic way: a
/// compare the provider refuses counts as a difference, unless the
/// provider has gone or did not answer.
fn differs(
    range: &IUIAutomationTextRange,
    endpoint: Endpoint,
    known: RangeEnd<'_>,
) -> Result<bool, Error> {
    match range.compare_endpoints(endpoint, known.range, known.endpoint) {
        Ok(order) => Ok(order != 0),
        Err(error) => {
            let error = Error::from(error);
            if gone_or_timed_out(&error) {
                Err(error)
            } else {
                Ok(true)
            }
        }
    }
}

/// The watch check the classic way, the fallback and the reference: the
/// same reads as the remote program, each a cross-process call.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails.
pub fn watch_check_classic(query: &WatchQuery<'_>) -> Result<WatchAnswer, Error> {
    let caret_query = CaretQuery {
        element: query.element,
        pattern: query.pattern,
        since: None,

        previous_selection: None,
        previous_collapsed: false,
        unit: None,
        formats: None,
        attributes: crate::caret::Attributes::NONE,
        learning: crate::caret::Attributes::NONE,
        max_text: query.max_text,
        max_change_text: 0,
        recheck: false,
        read_selection_text: false,
        read_previous_text: false,
    };
    let (caret, collapsed_caret, selection) = classic_caret(&caret_query)?;
    let point = if collapsed_caret {
        caret.clone()
    } else {
        collapsed(&caret)?
    };
    let caret_moved = differs(&point, Endpoint::Start, query.start)?;
    let (range, start, end) = match &selection {
        Some(selection) => (selection, Endpoint::Start, Endpoint::End),
        None => (&point, Endpoint::Start, Endpoint::Start),
    };
    let selection_moved = differs(range, start, query.start_selection.0)?
        || differs(range, end, query.start_selection.1)?;
    let line = unit_at(&point, TextUnit_Line, query.max_text)?;
    let word = query
        .word
        .then(|| unit_at(&point, TextUnit_Word, query.max_text))
        .transpose()?;
    let read = query.last || caret_moved || selection_moved;
    let unit = match query.unit {
        Some(unit) if read => Some(unit_at(&point, unit, query.max_text)?),
        _ => None,
    };
    let ids: Vec<TextAttribute> = query.attributes.iter().collect();
    let runs = match query.formats.filter(|_| !ids.is_empty()) {
        Some(span) if read => {
            let (runs, _) = match span {
                FormatSpan::Line | FormatSpan::Unit => {
                    let spanned = match span {
                        FormatSpan::Unit => unit.as_ref().unwrap_or(&line),
                        _ => &line,
                    };
                    classic_runs(
                        (&spanned.range, spanned.text.len()),
                        false,
                        &ids,
                        Attributes::NONE,
                        query.max_text,
                    )?
                }
                FormatSpan::Character => {
                    let character = point.clone_range()?;
                    character.expand(TextUnit_Character)?;
                    classic_runs(
                        (&character, 0),
                        true,
                        &ids,
                        Attributes::NONE,
                        query.max_text,
                    )?
                }
            };
            Some(runs)
        }
        _ => None,
    };
    let (again, _, _) = classic_caret(&caret_query)?;
    let moved_while_read = again
        .compare_endpoints(Endpoint::Start, &point, Endpoint::Start)
        .ok()
        .is_none_or(|order| order != 0);
    Ok(WatchAnswer {
        caret,
        collapsed: collapsed_caret,
        selection,
        caret_moved,
        selection_moved,
        line,
        word,
        unit,
        runs,
        moved_while_read,
    })
}
