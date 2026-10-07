//! The caret read for a caret report (milestone M4 items 3 and 7;
//! `phase6-design.md`, "Caret responsiveness"): the caret and the
//! selection, whether they moved from where they were known to be, the
//! caret's line and the caret's place in it, another unit at the caret
//! (a word, a paragraph), and the formatting of the text to be spoken,
//! stretch by stretch. A remote program does it in one round trip, and a
//! classic implementation behind the same signature does it call by call;
//! [`caret_read`] is the one function callers use.
//!
//! The caret is `TextPattern2`'s caret range where the provider has it and
//! text is selected, and otherwise the selection's start, as the outpost
//! has always read it; text with no selection at all (no caret) is read
//! from the start of its document. Formatting is read by UIA's format unit,
//! as NVDA reads it (`docs/nvda/document-formatting.md`): each stretch of
//! the text whose attributes are the same, with the attributes asked for.
//! A character is one stretch, never walked.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    IUIAutomationTextRange, TextUnit, TextUnit_Character, TextUnit_Format, TextUnit_Line,
    UIA_AnnotationTypesAttributeId, UIA_FontNameAttributeId, UIA_FontSizeAttributeId,
    UIA_FontWeightAttributeId, UIA_ForegroundColorAttributeId, UIA_IsItalicAttributeId,
    UIA_TEXTATTRIBUTE_ID, UIA_UnderlineStyleAttributeId,
};

use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt, caret_range};

use crate::builder::{Builder, Reg, kind};
use crate::error::Error;
use crate::focus::Path;
use crate::instruction::TypeTest;
use crate::opcode::Comparison;
use crate::operation::{Outcome, Value};

/// UIA's spelling error annotation type.
pub const ANNOTATION_SPELLING_ERROR: i32 = 60001;

/// UIA's grammar error annotation type.
pub const ANNOTATION_GRAMMAR_ERROR: i32 = 60002;

/// The most stretches of formatting read for one unit: more than a line
/// of prose has, and a bound on the program's work for one that has more.
pub const MAX_RUNS: u32 = 64;

/// A position: one end of a text range.
#[derive(Clone, Copy)]
pub struct RangeEnd<'a> {
    /// The range.
    pub range: &'a IUIAutomationTextRange,
    /// Which of its ends.
    pub endpoint: Endpoint,
}

/// Which text's formatting to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatSpan {
    /// The character at the caret.
    Character,
    /// The unit read at the caret ([`CaretQuery::unit`]).
    Unit,
    /// The caret's line.
    Line,
}

/// Which text attributes to read, each only when its indication is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent attributes, each read or not"
)]
pub struct Attributes {
    /// The annotation types, for spelling and grammar errors.
    pub annotations: bool,
    /// The font's name and size.
    pub font: bool,
    /// Font weight, italic, and underline style.
    pub font_attributes: bool,
    /// The foreground color.
    pub color: bool,
}

impl Attributes {
    /// The attribute ids read, in the order the answer gives them.
    fn ids(self) -> Vec<(Attribute, UIA_TEXTATTRIBUTE_ID)> {
        let mut ids = Vec::new();
        if self.annotations {
            ids.push((Attribute::Annotations, UIA_AnnotationTypesAttributeId));
        }
        if self.font {
            ids.push((Attribute::FontName, UIA_FontNameAttributeId));
            ids.push((Attribute::FontSize, UIA_FontSizeAttributeId));
        }
        if self.font_attributes {
            ids.push((Attribute::FontWeight, UIA_FontWeightAttributeId));
            ids.push((Attribute::Italic, UIA_IsItalicAttributeId));
            ids.push((Attribute::Underline, UIA_UnderlineStyleAttributeId));
        }
        if self.color {
            ids.push((Attribute::Color, UIA_ForegroundColorAttributeId));
        }
        ids
    }

    /// Whether any attribute is read.
    #[must_use]
    pub fn any(self) -> bool {
        self.annotations || self.font || self.font_attributes || self.color
    }
}

/// One attribute read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attribute {
    Annotations,
    FontName,
    FontSize,
    FontWeight,
    Italic,
    Underline,
    Color,
}

impl Attribute {
    /// The type its value has when the provider gives one.
    fn test(self) -> TypeTest {
        match self {
            Self::Annotations | Self::FontWeight | Self::Underline | Self::Color => TypeTest::Int,
            Self::FontName => TypeTest::String,
            Self::FontSize => TypeTest::Double,
            Self::Italic => TypeTest::Bool,
        }
    }
}

/// What [`caret_read`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct CaretQuery<'a> {
    /// The element with the text, which a program starts from.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern, which the classic implementation reads.
    pub pattern: &'a IUIAutomationTextPattern,
    /// Its `TextPattern2`, when the provider has one.
    pub pattern2: Option<&'a IUIAutomationTextPattern2>,
    /// Where the caret was known to be, to say whether it moved.
    pub since: Option<RangeEnd<'a>>,
    /// The selection's ends as they were known (both the caret when
    /// nothing was selected), to say whether the selection changed.
    pub previous_selection: Option<(RangeEnd<'a>, RangeEnd<'a>)>,
    /// A unit to read at the caret besides the line: a word, a paragraph,
    /// or a page.
    pub unit: Option<TextUnit>,
    /// Whose formatting to read, if any.
    pub formats: Option<FormatSpan>,
    /// The attributes to read.
    pub attributes: Attributes,
    /// The most UTF-16 code units of text read for a line or a unit.
    pub max_text: i32,
    /// The most UTF-16 code units read for each selection change, when the
    /// selection is not [`previous_selection`](Self::previous_selection).
    pub max_change_text: i32,
}

/// Text that became selected or stopped being selected
/// ([`CaretAnswer::changes`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionTextChange {
    /// True for text newly selected, false for text no longer selected.
    pub selected: bool,
    /// The text, at most the query's `max_change_text` code units; never
    /// empty.
    pub text: Vec<u16>,
}

/// A unit read at the caret.
#[derive(Debug)]
pub struct UnitRead {
    /// Its range.
    pub range: IUIAutomationTextRange,
    /// Its text, at most the query's `max_text` code units.
    pub text: Vec<u16>,
    /// How many code units of it come before the caret.
    pub offset: usize,
}

/// The attributes of one stretch of text, as the provider gave them;
/// `None` for one not read, not supported, or of another type.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunAttributes {
    /// The stretch is a spelling error.
    pub spelling_error: bool,
    /// The stretch is a grammar error.
    pub grammar_error: bool,
    /// The font's name.
    pub font_name: Option<String>,
    /// The font's size in points.
    pub font_size: Option<f64>,
    /// The font's weight (400 normal, 700 bold).
    pub font_weight: Option<i32>,
    /// Whether it is italic.
    pub italic: Option<bool>,
    /// The underline style (0 none).
    pub underline: Option<i32>,
    /// The foreground color, a `COLORREF` (0x00bbggrr).
    pub color: Option<i32>,
}

/// One stretch of formatting: how many UTF-16 code units of the span's text
/// it covers, from where the last one ended, and its attributes. A
/// character's one stretch covers it whole, and its length is not read
/// (zero).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// Its length in UTF-16 code units.
    pub length: usize,
    /// Its attributes.
    pub attributes: RunAttributes,
}

/// The answer to a [`CaretQuery`].
#[derive(Debug)]
pub struct CaretAnswer {
    /// A range whose start is the caret.
    pub caret: IUIAutomationTextRange,
    /// Whether `caret` is known to be collapsed.
    pub collapsed: bool,
    /// The selection, when text is selected.
    pub selection: Option<IUIAutomationTextRange>,
    /// Whether the caret is not where [`CaretQuery::since`] says; false
    /// without one.
    pub moved: bool,
    /// Whether the selection's ends are not those of
    /// [`CaretQuery::previous_selection`]; false without one.
    pub selection_moved: bool,
    /// The caret's line.
    pub line: UnitRead,
    /// The query's unit at the caret, when it asked for one.
    pub unit: Option<UnitRead>,
    /// The formatting of the query's span, stretch by stretch from its
    /// start; empty when it asked for none.
    pub runs: Vec<Run>,
    /// With a [`CaretQuery::previous_selection`], how the selection changed
    /// from it, by the text protocol's rule (`docs/crates/verbatim-model.md`,
    /// "Requests and replies"): two selections that neither overlap nor
    /// touch are an unselection then a selection; otherwise the start side
    /// changes, then the end side. Empty when the selection did not move;
    /// `None` without a previous selection.
    pub changes: Option<Vec<SelectionTextChange>>,
}

/// The signature both implementations share.
pub type CaretReadFn = fn(&CaretQuery<'_>) -> Result<CaretAnswer, Error>;

/// The caret read, the one function call sites use: the remote program when
/// `remote` is true, falling back to the classic implementation for this
/// call when the program fails, and the classic implementation alone when
/// `remote` is false. Says which path answered, as
/// [`crate::focus_ancestry`] does.
///
/// # Errors
///
/// The classic implementation's [`Error`], when it ran and failed.
pub fn caret_read(query: &CaretQuery<'_>, remote: bool) -> Result<(CaretAnswer, Path), Error> {
    if !remote {
        return caret_read_classic(query).map(|answer| (answer, Path::Classic));
    }
    match caret_read_remote(query) {
        Ok(answer) => Ok((answer, Path::Remote)),
        Err(error) if gone_or_timed_out(&error) => Err(error),
        Err(error) => caret_read_classic(query).map(|answer| (answer, Path::Fallback(error))),
    }
}

/// Whether a run failed because the provider's process has gone or did not
/// answer in time, which the classic implementation would meet too.
pub(crate) fn gone_or_timed_out(error: &Error) -> bool {
    use windows::Win32::UI::Accessibility::{UIA_E_ELEMENTNOTAVAILABLE, UIA_E_TIMEOUT};
    error.hresult().is_some_and(|code| {
        let code = code.0.cast_unsigned();
        code == UIA_E_ELEMENTNOTAVAILABLE || code == UIA_E_TIMEOUT
    })
}

/// A UIA endpoint's number in a program.
pub(crate) fn endpoint_number(endpoint: Endpoint) -> i32 {
    match endpoint {
        Endpoint::Start => 0,
        Endpoint::End => 1,
    }
}

/// The program's constants.
pub(crate) struct Constants {
    pub(crate) start: Reg<kind::Int>,
    pub(crate) end: Reg<kind::Int>,
    pub(crate) zero: Reg<kind::Int>,
    pub(crate) one: Reg<kind::Int>,
    pub(crate) max_text: Reg<kind::Int>,
}

impl Constants {
    /// Emits a copy of `range` collapsed to its start.
    pub(crate) fn collapsed(
        &self,
        b: &mut Builder,
        range: Reg<kind::TextRange>,
    ) -> Reg<kind::TextRange> {
        let copy = b.text_range_clone(range);
        b.text_range_move_endpoint_by_range(copy, self.end, copy, self.start);
        copy
    }

    /// Emits the `unit` at `point` (a collapsed range at the caret): its
    /// range, its text, and how much of the text comes before the caret.
    fn unit_at(
        &self,
        b: &mut Builder,
        point: Reg<kind::TextRange>,
        unit: Reg<kind::Int>,
    ) -> UnitRegisters {
        let range = b.text_range_clone(point);
        b.text_range_expand_to_enclosing_unit(range, unit);
        let text = b.text_range_get_text(range, self.max_text);
        let before = b.text_range_clone(range);
        b.text_range_move_endpoint_by_range(before, self.end, point, self.start);
        let before = b.text_range_get_text(before, self.max_text);
        let offset = b.string_size(before);
        UnitRegisters {
            range: b.add_to_results(range),
            text: b.add_to_results(text),
            offset: b.add_to_results(offset),
        }
    }
}

/// The registers of a unit read.
#[derive(Clone, Copy)]
struct UnitRegisters {
    range: Reg<kind::TextRange>,
    text: Reg<kind::Str>,
    offset: Reg<kind::Uint>,
}

impl UnitRegisters {
    fn read(self, outcome: &Outcome) -> Result<UnitRead, Error> {
        Ok(UnitRead {
            range: outcome
                .get(self.range)?
                .ok_or(Error::MissingResult(self.range.id()))?,
            text: string_of(outcome, self.text)?.encode_utf16().collect(),
            offset: usize::try_from(outcome.get(self.offset)?).unwrap_or(usize::MAX),
        })
    }
}

/// A string result, empty for a null one: an empty string may come back
/// as null.
pub(crate) fn string_of(outcome: &Outcome, reg: Reg<kind::Str>) -> Result<String, Error> {
    Ok(match outcome.get(reg.any())? {
        Value::String(text) => text,
        _ => String::new(),
    })
}

/// The registers of the formatting read: the stretches' lengths, and one
/// array per attribute read (two for the annotations: spelling, then
/// grammar).
struct RunRegisters {
    lengths: Reg<kind::Array>,
    values: Vec<(Attribute, Reg<kind::Array>, Option<Reg<kind::Array>>)>,
}

/// Emits the attributes of `run`, appending each to its array.
fn emit_attributes(
    b: &mut Builder,
    run: Reg<kind::TextRange>,
    ids: &[(Attribute, UIA_TEXTATTRIBUTE_ID)],
    registers: &RunRegisters,
) {
    for ((attribute, id), (_, values, grammar)) in ids.iter().zip(&registers.values) {
        let id = b.int(id.0);
        let value = b.text_range_get_attribute_value(run, id);
        if *attribute == Attribute::Annotations {
            let spelling = b.new_bool(false);
            let grammar_error = b.new_bool(false);
            let spelling_type = b.int(ANNOTATION_SPELLING_ERROR);
            let grammar_type = b.int(ANNOTATION_GRAMMAR_ERROR);
            // One annotation type may come as a single integer.
            let single = b.is(TypeTest::Int, value);
            b.if_(single, |b| {
                let value = value.assume::<kind::Int>();
                let is_spelling = b.equal(value, spelling_type);
                b.if_(is_spelling, |b| {
                    let yes = b.bool(true);
                    b.set(spelling, yes);
                });
                let is_grammar = b.equal(value, grammar_type);
                b.if_(is_grammar, |b| {
                    let yes = b.bool(true);
                    b.set(grammar_error, yes);
                });
            });
            let array = b.is(TypeTest::Array, value);
            b.if_(array, |b| {
                let types = value.assume::<kind::Array>();
                let size = b.array_size(types);
                let index = b.new_uint(0);
                b.while_(
                    |b| b.compare(index, size, Comparison::LessThan),
                    |b| {
                        let item = b.array_get_at(types, index).assume::<kind::Int>();
                        let is_spelling = b.equal(item, spelling_type);
                        b.if_(is_spelling, |b| {
                            let yes = b.bool(true);
                            b.set(spelling, yes);
                        });
                        let is_grammar = b.equal(item, grammar_type);
                        b.if_(is_grammar, |b| {
                            let yes = b.bool(true);
                            b.set(grammar_error, yes);
                        });
                        let one = b.uint(1);
                        b.add_assign(index, one);
                    },
                );
            });
            b.array_append(*values, spelling);
            if let Some(grammar) = grammar {
                b.array_append(*grammar, grammar_error);
            }
        } else {
            // A sentinel ("not supported", "mixed") or a value of another
            // type is returned as null.
            let wanted = b.is(attribute.test(), value);
            let other = b.not(wanted);
            b.if_(other, |b| {
                let null = b.new_null();
                b.set(value, null);
            });
            b.array_append(*values, value);
        }
    }
}

/// Emits the formatting read of `span`: one stretch for a character, and
/// otherwise a walk by the format unit, each stretch cut at the span's end.
fn emit_runs(
    b: &mut Builder,
    c: &Constants,
    span: Reg<kind::TextRange>,
    character: bool,
    ids: &[(Attribute, UIA_TEXTATTRIBUTE_ID)],
) -> RunRegisters {
    let lengths = b.new_array();
    let lengths = b.add_to_results(lengths);
    let mut values = Vec::new();
    for (attribute, _) in ids {
        let array = b.new_array();
        let array = b.add_to_results(array);
        let grammar = (*attribute == Attribute::Annotations).then(|| {
            let array = b.new_array();
            b.add_to_results(array)
        });
        values.push((*attribute, array, grammar));
    }
    let registers = RunRegisters { lengths, values };
    if character {
        // A character is one stretch, covering it whatever its length.
        let length = b.uint(0);
        b.array_append(lengths, length);
        emit_attributes(b, span, ids, &registers);
        return registers;
    }
    let format = b.int(TextUnit_Format.0);
    let limit = b.int(i32::try_from(MAX_RUNS).unwrap_or(i32::MAX));
    let walker = c.collapsed(b, span);
    let count = b.new_int(0);
    let going = b.new_bool(true);
    b.while_(
        |_| going,
        |b| {
            let run = b.text_range_clone(walker);
            let moved = b.text_range_move_endpoint_by_unit(run, c.end, format, c.one);
            let order = b.text_range_compare_endpoints(run, c.end, span, c.end);
            let past = b.compare(order, c.zero, Comparison::GreaterThan);
            let stuck = b.equal(moved, c.zero);
            let cut = b.or(past, stuck);
            b.if_(cut, |b| {
                b.text_range_move_endpoint_by_range(run, c.end, span, c.end);
            });
            let text = b.text_range_get_text(run, c.max_text);
            let length = b.string_size(text);
            b.array_append(lengths, length);
            emit_attributes(b, run, ids, &registers);
            b.text_range_move_endpoint_by_range(walker, c.start, run, c.end);
            b.add_assign(count, c.one);
            let order = b.text_range_compare_endpoints(walker, c.start, span, c.end);
            let done = b.compare(order, c.zero, Comparison::GreaterThanOrEqual);
            let full = b.compare(count, limit, Comparison::GreaterThanOrEqual);
            let stop = b.or(done, full);
            b.if_(stop, |b| {
                let no = b.bool(false);
                b.set(going, no);
            });
        },
    );
    registers
}

impl RunRegisters {
    /// The stretches, from the arrays the program filled.
    fn read(&self, outcome: &Outcome) -> Result<Vec<Run>, Error> {
        let lengths = outcome.get(self.lengths)?;
        let mut columns = Vec::new();
        for (attribute, values, grammar) in &self.values {
            let grammar = match grammar {
                Some(grammar) => Some(outcome.get(*grammar)?),
                None => None,
            };
            columns.push((*attribute, outcome.get(*values)?, grammar));
        }
        Ok(lengths
            .iter()
            .enumerate()
            .map(|(index, length)| {
                let mut attributes = RunAttributes::default();
                for (attribute, values, grammar) in &columns {
                    let value = values.get(index);
                    match attribute {
                        Attribute::Annotations => {
                            attributes.spelling_error = matches!(value, Some(Value::Bool(true)));
                            attributes.grammar_error = matches!(
                                grammar.as_ref().and_then(|grammar| grammar.get(index)),
                                Some(Value::Bool(true))
                            );
                        }
                        Attribute::FontName => {
                            attributes.font_name = match value {
                                Some(Value::String(name)) if !name.is_empty() => Some(name.clone()),
                                _ => None,
                            };
                        }
                        Attribute::FontSize => attributes.font_size = double(value),
                        Attribute::FontWeight => attributes.font_weight = int(value),
                        Attribute::Italic => {
                            attributes.italic = match value {
                                Some(Value::Bool(italic)) => Some(*italic),
                                _ => None,
                            };
                        }
                        Attribute::Underline => attributes.underline = int(value),
                        Attribute::Color => attributes.color = int(value),
                    }
                }
                Run {
                    length: match length {
                        Value::Uint(length) => usize::try_from(*length).unwrap_or(0),
                        Value::Int(length) => usize::try_from(*length).unwrap_or(0),
                        _ => 0,
                    },
                    attributes,
                }
            })
            .collect())
    }
}

/// An integer value returned, of any integer type.
fn int(value: Option<&Value>) -> Option<i32> {
    match value? {
        Value::Int(value) => Some(*value),
        Value::Uint(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

/// A floating-point value returned.
fn double(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Double(value) => Some(*value),
        Value::Int(value) => Some(f64::from(*value)),
        _ => None,
    }
}

/// The caret read in one cross-process round trip.
///
/// # Errors
///
/// Any [`Error`] from running the program.
#[expect(
    clippy::too_many_lines,
    reason = "one program, read top to bottom as it runs"
)]
pub fn caret_read_remote(query: &CaretQuery<'_>) -> Result<CaretAnswer, Error> {
    let mut b = Builder::new();
    let c = Constants {
        start: b.int(endpoint_number(Endpoint::Start)),
        end: b.int(endpoint_number(Endpoint::End)),
        zero: b.int(0),
        one: b.int(1),
        max_text: b.int(query.max_text),
    };
    let element = b.import_element(query.element);
    let CaretRegisters {
        caret,
        collapsed,
        has_selection,
        selection,
    } = emit_caret(&mut b, &c, element, query.pattern2.is_some());
    let point = c.collapsed(&mut b, caret);

    // The evidence.
    let moved = b.new_bool(false);
    let moved = b.add_to_results(moved);
    if let Some(since) = query.since {
        let known = b.import_text_range(since.range);
        let endpoint = b.int(endpoint_number(since.endpoint));
        let order = b.text_range_compare_endpoints(point, c.start, known, endpoint);
        let differs = b.not_equal(order, c.zero);
        b.set(moved, differs);
    }
    let selection_moved = b.new_bool(false);
    let selection_moved = b.add_to_results(selection_moved);
    let mut changes = None;
    if let Some((old_start, old_end)) = query.previous_selection {
        let start_range = b.import_text_range(old_start.range);
        let start_endpoint = b.int(endpoint_number(old_start.endpoint));
        let end_range = b.import_text_range(old_end.range);
        let end_endpoint = b.int(endpoint_number(old_end.endpoint));
        let compare_ends = |b: &mut Builder,
                            range: Reg<kind::TextRange>,
                            start: Reg<kind::Int>,
                            end: Reg<kind::Int>| {
            let first = b.text_range_compare_endpoints(range, start, start_range, start_endpoint);
            let first = b.not_equal(first, c.zero);
            let second = b.text_range_compare_endpoints(range, end, end_range, end_endpoint);
            let second = b.not_equal(second, c.zero);
            let either = b.or(first, second);
            b.set(selection_moved, either);
        };
        b.if_else(
            has_selection,
            |b| compare_ends(b, selection, c.start, c.end),
            |b| compare_ends(b, point, c.start, c.start),
        );
        changes = Some(emit_changes(
            &mut b,
            &c,
            (has_selection, selection, point),
            ((start_range, start_endpoint), (end_range, end_endpoint)),
            (selection_moved, query.max_change_text),
        ));
    }

    // The line, the unit, and the formatting.
    let line_unit = b.int(TextUnit_Line.0);
    let line = c.unit_at(&mut b, point, line_unit);
    let unit = query.unit.map(|unit| {
        let unit = b.int(unit.0);
        c.unit_at(&mut b, point, unit)
    });
    let ids = query.attributes.ids();
    let runs = match query.formats {
        Some(span) if !ids.is_empty() => {
            let (range, character) = match span {
                FormatSpan::Line => (line.range, false),
                FormatSpan::Unit => (unit.map_or(line.range, |unit| unit.range), false),
                FormatSpan::Character => {
                    let character = b.text_range_clone(point);
                    let unit = b.int(TextUnit_Character.0);
                    b.text_range_expand_to_enclosing_unit(character, unit);
                    (character, true)
                }
            };
            Some(emit_runs(&mut b, &c, range, character, &ids))
        }
        _ => None,
    };

    let outcome = b.finish().execute()?;
    let caret_range = outcome
        .get(caret)?
        .ok_or(Error::MissingResult(caret.id()))?;
    let selection = if outcome.get(has_selection)? {
        outcome.get(selection)?
    } else {
        None
    };
    Ok(CaretAnswer {
        caret: caret_range,
        collapsed: outcome.get(collapsed)?,
        selection,
        moved: outcome.get(moved)?,
        selection_moved: outcome.get(selection_moved)?,
        line: line.read(&outcome)?,
        unit: unit.map(|unit| unit.read(&outcome)).transpose()?,
        runs: runs
            .map(|runs| runs.read(&outcome))
            .transpose()?
            .unwrap_or_default(),
        changes: changes
            .map(|(selected, texts)| read_changes(&outcome, selected, texts))
            .transpose()?,
    })
}

/// A position in a program: a range register and the endpoint's number.
type ProgramEnd = (Reg<kind::TextRange>, Reg<kind::Int>);

/// Emits the selection's changes from the old ends to the new selection,
/// read only when `moved`: two arrays, whether each change is a selection,
/// and its text. The new selection is `selection` when `has_selection`,
/// else collapsed at `point`.
fn emit_changes(
    b: &mut Builder,
    c: &Constants,
    (has_selection, selection, point): (
        Reg<kind::Bool>,
        Reg<kind::TextRange>,
        Reg<kind::TextRange>,
    ),
    (old_start, old_end): (ProgramEnd, ProgramEnd),
    (moved, max_text): (Reg<kind::Bool>, i32),
) -> (Reg<kind::Array>, Reg<kind::Array>) {
    let selected = b.new_array();
    let selected = b.add_to_results(selected);
    let texts = b.new_array();
    let texts = b.add_to_results(texts);
    let max_text = b.int(max_text);
    b.if_(moved, |b| {
        let range = b.new_null().assume::<kind::TextRange>();
        let start_end = b.new_int(0);
        let end_end = b.new_int(0);
        b.if_else(
            has_selection,
            |b| {
                b.set(range, selection);
                b.set(start_end, c.start);
                b.set(end_end, c.end);
            },
            |b| {
                b.set(range, point);
                b.set(start_end, c.start);
                b.set(end_end, c.start);
            },
        );
        let new_start = (range, start_end);
        let new_end = (range, end_end);
        let change = |b: &mut Builder, is_selection: bool, from: ProgramEnd, to: ProgramEnd| {
            let between = b.text_range_clone(from.0);
            b.text_range_move_endpoint_by_range(between, c.start, from.0, from.1);
            b.text_range_move_endpoint_by_range(between, c.end, to.0, to.1);
            let text = b.text_range_get_text(between, max_text);
            let size = b.string_size(text);
            let none = b.uint(0);
            let some = b.compare(size, none, Comparison::GreaterThan);
            b.if_(some, |b| {
                let flag = b.bool(is_selection);
                b.array_append(selected, flag);
                b.array_append(texts, text);
            });
        };
        let order = |b: &mut Builder, a: ProgramEnd, other: ProgramEnd| {
            b.text_range_compare_endpoints(a.0, a.1, other.0, other.1)
        };
        let before = order(b, new_end, old_start);
        let before = b.compare(before, c.zero, Comparison::LessThan);
        let after = order(b, new_start, old_end);
        let after = b.compare(after, c.zero, Comparison::GreaterThan);
        let apart = b.or(before, after);
        b.if_else(
            apart,
            |b| {
                change(b, false, old_start, old_end);
                change(b, true, new_start, new_end);
            },
            |b| {
                let starts = order(b, new_start, old_start);
                let back = b.compare(starts, c.zero, Comparison::LessThan);
                b.if_(back, |b| change(b, true, new_start, old_start));
                let on = b.compare(starts, c.zero, Comparison::GreaterThan);
                b.if_(on, |b| change(b, false, old_start, new_start));
                let ends = order(b, new_end, old_end);
                let on = b.compare(ends, c.zero, Comparison::GreaterThan);
                b.if_(on, |b| change(b, true, old_end, new_end));
                let back = b.compare(ends, c.zero, Comparison::LessThan);
                b.if_(back, |b| change(b, false, new_end, old_end));
            },
        );
    });
    (selected, texts)
}

/// The changes [`emit_changes`] found.
fn read_changes(
    outcome: &Outcome,
    selected: Reg<kind::Array>,
    texts: Reg<kind::Array>,
) -> Result<Vec<SelectionTextChange>, Error> {
    let selected = outcome.get(selected)?;
    let texts = outcome.get(texts)?;
    Ok(selected
        .iter()
        .zip(texts)
        .map(|(selected, text)| SelectionTextChange {
            selected: matches!(selected, Value::Bool(true)),
            text: match text {
                Value::String(text) => text.encode_utf16().collect(),
                _ => Vec::new(),
            },
        })
        .collect())
}

/// The registers of the caret and the selection.
#[derive(Clone, Copy)]
pub(crate) struct CaretRegisters {
    pub(crate) caret: Reg<kind::TextRange>,
    pub(crate) collapsed: Reg<kind::Bool>,
    pub(crate) has_selection: Reg<kind::Bool>,
    pub(crate) selection: Reg<kind::TextRange>,
}

/// Emits the caret and the selection, as the classic implementation reads
/// them: the selection's first range, collapsed at the caret when nothing
/// is selected; with text selected, `TextPattern2`'s caret where the
/// provider has it (`pattern2`), else the selection's start; with no
/// selection at all, the start of the document.
pub(crate) fn emit_caret(
    b: &mut Builder,
    c: &Constants,
    element: Reg<kind::Element>,
    pattern2: bool,
) -> CaretRegisters {
    let pattern = b.get_text_pattern(element, false);
    let caret = b.new_null().assume::<kind::TextRange>();
    let caret = b.add_to_results(caret);
    let collapsed = b.new_bool(false);
    let collapsed = b.add_to_results(collapsed);
    let has_selection = b.new_bool(false);
    let has_selection = b.add_to_results(has_selection);
    let selection = b.new_null().assume::<kind::TextRange>();
    let selection = b.add_to_results(selection);
    let ranges = b.text_pattern_get_selection(pattern);
    let size = b.array_size(ranges);
    let none = b.uint(0);
    let empty = b.equal(size, none);
    b.if_else(
        empty,
        |b| {
            // Text with no caret is read from its start.
            let document = b.text_pattern_get_document_range(pattern);
            b.set(caret, document);
        },
        |b| {
            let first = b.array_get_at(ranges, none).assume::<kind::TextRange>();
            let order = b.text_range_compare_endpoints(first, c.start, first, c.end);
            let is_collapsed = b.equal(order, c.zero);
            b.if_else(
                is_collapsed,
                |b| {
                    b.set(caret, first);
                    let yes = b.bool(true);
                    b.set(collapsed, yes);
                },
                |b| {
                    let yes = b.bool(true);
                    b.set(has_selection, yes);
                    b.set(selection, first);
                    if pattern2 {
                        // Only text with a selection needs the caret apart
                        // from it.
                        let pattern2 = b.get_text_pattern(element, true);
                        let active = b.new_bool(false);
                        let range = b.text_pattern2_get_caret_range(pattern2, active);
                        b.set(caret, range);
                        b.set(collapsed, yes);
                    } else {
                        b.set(caret, first);
                    }
                },
            );
        },
    );
    CaretRegisters {
        caret,
        collapsed,
        has_selection,
        selection,
    }
}

/// A copy of `range` collapsed to its start. Two calls.
fn collapsed(range: &IUIAutomationTextRange) -> Result<IUIAutomationTextRange, Error> {
    let copy = range.clone_range()?;
    copy.move_endpoint_to(Endpoint::End, &copy, Endpoint::Start)?;
    Ok(copy)
}

/// The `unit` at `point`, as [`Constants::unit_at`] emits it. Five calls.
fn unit_at(
    point: &IUIAutomationTextRange,
    unit: TextUnit,
    max_text: i32,
) -> Result<UnitRead, Error> {
    let range = point.clone_range()?;
    range.expand(unit)?;
    let text = range.text(max_text)?;
    let before = range.clone_range()?;
    before.move_endpoint_to(Endpoint::End, point, Endpoint::Start)?;
    let offset = before.text(max_text)?.len();
    Ok(UnitRead {
        range,
        text,
        offset,
    })
}

/// The attributes of `run`, all of them in one call (`GetAttributeValues`),
/// or, where the range cannot answer that, one call each (one for both
/// errors); an attribute whose read fails is not supported.
fn attributes_of(
    run: &IUIAutomationTextRange,
    ids: &[(Attribute, UIA_TEXTATTRIBUTE_ID)],
) -> Result<RunAttributes, Error> {
    let mut attributes = RunAttributes::default();
    if ids.is_empty() {
        return Ok(attributes);
    }
    let values = run.attributes(&ids.iter().map(|(_, id)| *id).collect::<Vec<_>>())?;
    for ((attribute, _), value) in ids.iter().zip(values) {
        match attribute {
            Attribute::Annotations => {
                let types = verbatim_uia::variant_i32_array(&value).unwrap_or_default();
                attributes.spelling_error = types.contains(&ANNOTATION_SPELLING_ERROR);
                attributes.grammar_error = types.contains(&ANNOTATION_GRAMMAR_ERROR);
            }
            Attribute::FontName => {
                attributes.font_name = (value.vt() == windows::Win32::System::Variant::VT_BSTR)
                    .then(|| verbatim_uia::variant_string(&value))
                    .flatten();
            }
            Attribute::FontSize => attributes.font_size = verbatim_uia::variant_f64(&value),
            Attribute::FontWeight => attributes.font_weight = plain_i32(&value),
            Attribute::Italic => attributes.italic = verbatim_uia::variant_optional_bool(&value),
            Attribute::Underline => attributes.underline = plain_i32(&value),
            Attribute::Color => attributes.color = plain_i32(&value),
        }
    }
    Ok(attributes)
}

/// A variant's 32-bit integer, `None` for any other type.
fn plain_i32(value: &windows::Win32::System::Variant::VARIANT) -> Option<i32> {
    (value.vt() == windows::Win32::System::Variant::VT_I4)
        .then(|| verbatim_uia::variant_i32(value))
        .flatten()
}

/// The formatting of `span` the classic way, as [`emit_runs`] emits it.
fn classic_runs(
    span: &IUIAutomationTextRange,
    character: bool,
    ids: &[(Attribute, UIA_TEXTATTRIBUTE_ID)],
    max_text: i32,
) -> Result<Vec<Run>, Error> {
    if character {
        return Ok(vec![Run {
            length: 0,
            attributes: attributes_of(span, ids)?,
        }]);
    }
    let walker = collapsed(span)?;
    let mut runs = Vec::new();
    loop {
        let run = walker.clone_range()?;
        let moved = run.move_endpoint_by_unit(Endpoint::End, TextUnit_Format, 1)?;
        if run.compare_endpoints(Endpoint::End, span, Endpoint::End)? > 0 || moved == 0 {
            run.move_endpoint_to(Endpoint::End, span, Endpoint::End)?;
        }
        runs.push(Run {
            length: run.text(max_text)?.len(),
            attributes: attributes_of(&run, ids)?,
        });
        walker.move_endpoint_to(Endpoint::Start, &run, Endpoint::End)?;
        let done = walker.compare_endpoints(Endpoint::Start, span, Endpoint::End)? >= 0;
        if done || runs.len() >= MAX_RUNS as usize {
            return Ok(runs);
        }
    }
}

/// The caret read the classic way, the fallback and the reference: the
/// same steps as the remote program, each a cross-process call, counted on
/// the thread's `verbatim_uia::calls`.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails.
pub fn caret_read_classic(query: &CaretQuery<'_>) -> Result<CaretAnswer, Error> {
    let ranges = query.pattern.selection()?;
    let (caret, collapsed_caret, selection) = match ranges.first() {
        None => (query.pattern.document_range()?, false, None),
        Some(first) => {
            if first.compare_endpoints(Endpoint::Start, first, Endpoint::End)? == 0 {
                (first.clone(), true, None)
            } else {
                match query.pattern2 {
                    Some(pattern2) => (caret_range(pattern2)?, true, Some(first.clone())),
                    None => (first.clone(), false, Some(first.clone())),
                }
            }
        }
    };
    // A collapsed caret is its own point; the program copies it either way,
    // which costs nothing there.
    let point = if collapsed_caret {
        caret.clone()
    } else {
        collapsed(&caret)?
    };
    let moved = match query.since {
        Some(since) => point.compare_endpoints(Endpoint::Start, since.range, since.endpoint)? != 0,
        None => false,
    };
    let selection_moved = match query.previous_selection {
        Some((old_start, old_end)) => {
            let (range, start, end) = match &selection {
                Some(selection) => (selection, Endpoint::Start, Endpoint::End),
                None => (&point, Endpoint::Start, Endpoint::Start),
            };
            range.compare_endpoints(start, old_start.range, old_start.endpoint)? != 0
                || range.compare_endpoints(end, old_end.range, old_end.endpoint)? != 0
        }
        None => false,
    };
    let changes = match query.previous_selection {
        Some(old) if selection_moved => {
            let (range, end) = match &selection {
                Some(selection) => (selection, Endpoint::End),
                None => (&point, Endpoint::Start),
            };
            let new = (
                RangeEnd {
                    range,
                    endpoint: Endpoint::Start,
                },
                RangeEnd {
                    range,
                    endpoint: end,
                },
            );
            Some(classic_changes(old, new, query.max_change_text)?)
        }
        Some(_) => Some(Vec::new()),
        None => None,
    };
    let line = unit_at(&point, TextUnit_Line, query.max_text)?;
    let unit = query
        .unit
        .map(|unit| unit_at(&point, unit, query.max_text))
        .transpose()?;
    let ids = query.attributes.ids();
    let runs = match query.formats {
        Some(span) if !ids.is_empty() => match span {
            FormatSpan::Line => classic_runs(&line.range, false, &ids, query.max_text)?,
            FormatSpan::Unit => classic_runs(
                unit.as_ref().map_or(&line.range, |unit| &unit.range),
                false,
                &ids,
                query.max_text,
            )?,
            FormatSpan::Character => {
                let character = point.clone_range()?;
                character.expand(TextUnit_Character)?;
                classic_runs(&character, true, &ids, query.max_text)?
            }
        },
        _ => Vec::new(),
    };
    Ok(CaretAnswer {
        caret,
        collapsed: collapsed_caret,
        selection,
        moved,
        selection_moved,
        line,
        unit,
        runs,
        changes,
    })
}

/// The selection's changes from `old` to `new`, the classic way, as
/// [`emit_changes`] emits them.
fn classic_changes(
    (old_start, old_end): (RangeEnd<'_>, RangeEnd<'_>),
    (new_start, new_end): (RangeEnd<'_>, RangeEnd<'_>),
    max_text: i32,
) -> Result<Vec<SelectionTextChange>, Error> {
    let order = |a: RangeEnd<'_>, b: RangeEnd<'_>| -> Result<i32, Error> {
        Ok(a.range.compare_endpoints(a.endpoint, b.range, b.endpoint)?)
    };
    let mut changes = Vec::new();
    let mut change = |selected: bool, from: RangeEnd<'_>, to: RangeEnd<'_>| -> Result<(), Error> {
        let between = from.range.clone_range()?;
        between.move_endpoint_to(Endpoint::Start, from.range, from.endpoint)?;
        between.move_endpoint_to(Endpoint::End, to.range, to.endpoint)?;
        let text = between.text(max_text)?;
        if !text.is_empty() {
            changes.push(SelectionTextChange { selected, text });
        }
        Ok(())
    };
    if order(new_end, old_start)? < 0 || order(new_start, old_end)? > 0 {
        change(false, old_start, old_end)?;
        change(true, new_start, new_end)?;
    } else {
        match order(new_start, old_start)? {
            starts if starts < 0 => change(true, new_start, old_start)?,
            starts if starts > 0 => change(false, old_start, new_start)?,
            _ => {}
        }
        match order(new_end, old_end)? {
            ends if ends > 0 => change(true, old_end, new_end)?,
            ends if ends < 0 => change(false, new_end, old_end)?,
            _ => {}
        }
    }
    Ok(changes)
}
