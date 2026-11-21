//! A caret wait's check (`docs/design/readers-tell-core-asks-build1.md`,
//! "The watch"; the request list's design, section 5, `WaitForCaret`):
//! the caret and the selection compared with the key's starting point,
//! the caret's line, and the word at the caret when a deleting key
//! compares it. The caret is read again at the end, so a check whose
//! caret moved while it read is known to be torn.
//!
//! The text comparisons of the effect test (the characters beside the
//! caret, the line, the word) are made by the caller on the texts this
//! returns, since they depend on how the caller cuts its text.
//!
//! The program builder emits the check as the first step of each poll's
//! program ([`emit_check`]), the batch's later reads fused after it under
//! its gate; [`watch_check_classic`] makes the same reads call by call.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit_Line,
    TextUnit_Word,
};

use verbatim_uia::text::{Endpoint, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::caret::{
    CaretQuery, Constants, RangeEnd, UnitRead, UnitRegisters, classic_caret, collapsed, emit_caret,
    emit_caret_with, gone_or_timed_out, unit_at,
};
use crate::error::Error;

/// What a wait's check is asked.
#[derive(Clone, Copy)]
pub struct WatchQuery<'a> {
    /// The element with the text.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern.
    pub pattern: &'a IUIAutomationTextPattern,
    /// The caret at the key's starting point.
    pub start: RangeEnd<'a>,
    /// The selection's ends at the starting point: both the caret when
    /// nothing was selected.
    pub start_selection: (RangeEnd<'a>, RangeEnd<'a>),
    /// Whether to read the word at the caret, which a deleting key's check
    /// compares once 30 ms have passed since its pass.
    pub word: bool,
    /// The most UTF-16 code units of text read for a line or a word.
    pub max_text: i32,
}

/// The answer to a [`WatchQuery`].
#[derive(Debug)]
pub struct WatchAnswer {
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
    /// Whether the caret read again at the end is not where it was first
    /// read.
    pub moved_while_read: bool,
}

/// The registers a check's answer is read from.
#[derive(Clone)]
pub(crate) struct CheckRegisters {
    pub(crate) caret_moved: Reg<kind::Bool>,
    pub(crate) selection_moved: Reg<kind::Bool>,
    pub(crate) line: UnitRegisters,
    pub(crate) word: Option<UnitRegisters>,
    pub(crate) moved_while_read: Reg<kind::Bool>,
}

impl CheckRegisters {
    /// The check's answer from a run's `outcome`.
    pub(crate) fn read(&self, outcome: &crate::operation::Outcome) -> Result<WatchAnswer, Error> {
        Ok(WatchAnswer {
            caret_moved: outcome.get(self.caret_moved)?,
            selection_moved: outcome.get(self.selection_moved)?,
            line: self.line.read(outcome)?,
            word: self.word.map(|word| word.read(outcome)).transpose()?,
            moved_while_read: outcome.get(self.moved_while_read)?,
        })
    }
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

/// A range's end in a program: the range and which of its ends.
pub(crate) type ProgramPoint = (Reg<kind::TextRange>, Reg<kind::Int>);

/// Emits a check of the caret in the text of `element` against the
/// starting point: the starting caret `start` and the starting
/// selection's ends `(was_start, was_end)`, the caret's line, the word at
/// the caret when `word`, and the caret read again at the end, each text
/// read up to `c`'s `max_text`.
pub(crate) fn emit_check(
    b: &mut Builder,
    c: &Constants,
    element: Reg<kind::Element>,
    (start, was_start, was_end): (ProgramPoint, ProgramPoint, ProgramPoint),
    word: bool,
) -> CheckRegisters {
    let pattern = b.get_text_pattern(element, false);
    let caret = emit_caret(b, c, pattern);
    let point = c.collapsed(b, caret.caret);

    let caret_moved = b.new_bool(true);
    let caret_moved = b.add_to_results(caret_moved);
    emit_differs(b, c, caret_moved, (point, c.start), start);

    // The selection's ends, both the caret when nothing is selected.
    let selection_moved = b.new_bool(true);
    let selection_moved = b.add_to_results(selection_moved);
    let first = b.new_bool(true);
    let second = b.new_bool(true);
    b.if_else(
        caret.has_selection,
        |b| {
            emit_differs(b, c, first, (caret.selection, c.start), was_start);
            emit_differs(b, c, second, (caret.selection, c.end), was_end);
        },
        |b| {
            emit_differs(b, c, first, (point, c.start), was_start);
            emit_differs(b, c, second, (point, c.start), was_end);
        },
    );
    let either = b.or(first, second);
    b.set(selection_moved, either);

    let line_unit = b.int(TextUnit_Line.0);
    let line = c.unit_at(b, point, line_unit);
    let word = word.then(|| {
        let word_unit = b.int(TextUnit_Word.0);
        c.unit_at(b, point, word_unit)
    });

    // The caret read again, once everything else is read.
    let again = emit_caret_with(b, c, pattern, false);
    let moved_while_read = b.new_bool(true);
    let moved_while_read = b.add_to_results(moved_while_read);
    emit_differs(
        b,
        c,
        moved_while_read,
        (again.caret, c.start),
        (point, c.start),
    );

    CheckRegisters {
        caret_moved,
        selection_moved,
        line,
        word,
        moved_while_read,
    }
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

/// The wait's check the classic way, the fallback and the reference: the
/// same reads as the program's, each a cross-process call.
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
        formats: false,
        attributes: crate::caret::Attributes::NONE,
        learning: crate::caret::Attributes::NONE,
        max_text: query.max_text,
        max_change_text: 0,
        read_selection_text: false,
        read_previous_text: false,
    };
    let (caret, collapsed_caret, selection) = classic_caret(&caret_query)?;
    let point = if collapsed_caret {
        caret
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
    let (again, _, _) = classic_caret(&caret_query)?;
    let moved_while_read = again
        .compare_endpoints(Endpoint::Start, &point, Endpoint::Start)
        .ok()
        .is_none_or(|order| order != 0);
    Ok(WatchAnswer {
        caret_moved,
        selection_moved,
        line,
        word,
        moved_while_read,
    })
}
