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
//! A stretch with an attribute UIA answers as "mixed", from a provider whose
//! format unit does not end where that attribute changes, is read again by
//! its words, and a mixed word by its characters, as NVDA reads one. A
//! character is one stretch, never walked.

use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    IUIAutomationTextRange, TextUnit, TextUnit_Character, TextUnit_Format, TextUnit_Line,
    TextUnit_Word, UIA_AnnotationTypesAttributeId, UIA_BackgroundColorAttributeId,
    UIA_BulletStyleAttributeId, UIA_FontNameAttributeId, UIA_FontSizeAttributeId,
    UIA_FontWeightAttributeId, UIA_ForegroundColorAttributeId, UIA_IsItalicAttributeId,
    UIA_LinkAttributeId, UIA_StrikethroughStyleAttributeId, UIA_TEXTATTRIBUTE_ID,
    UIA_UnderlineStyleAttributeId,
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

/// A text attribute the caret read can read, each read only when an
/// indication of the theme asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextAttribute {
    /// The annotation types, for spelling and grammar errors.
    Annotations,
    /// The font's name.
    FontName,
    /// The font's size.
    FontSize,
    /// The font's weight.
    FontWeight,
    /// Italic.
    Italic,
    /// The underline style.
    UnderlineStyle,
    /// The strikethrough style.
    StrikethroughStyle,
    /// The foreground color.
    ForegroundColor,
    /// The background color.
    BackgroundColor,
    /// The bullet style of a list item.
    BulletStyle,
    /// The link attribute.
    Link,
}

impl TextAttribute {
    /// Every attribute, in the order a read gives them.
    pub const ALL: [Self; 11] = [
        Self::Annotations,
        Self::FontName,
        Self::FontSize,
        Self::FontWeight,
        Self::Italic,
        Self::UnderlineStyle,
        Self::StrikethroughStyle,
        Self::ForegroundColor,
        Self::BackgroundColor,
        Self::BulletStyle,
        Self::Link,
    ];

    /// UIA's id of the attribute.
    #[must_use]
    pub const fn id(self) -> UIA_TEXTATTRIBUTE_ID {
        match self {
            Self::Annotations => UIA_AnnotationTypesAttributeId,
            Self::FontName => UIA_FontNameAttributeId,
            Self::FontSize => UIA_FontSizeAttributeId,
            Self::FontWeight => UIA_FontWeightAttributeId,
            Self::Italic => UIA_IsItalicAttributeId,
            Self::UnderlineStyle => UIA_UnderlineStyleAttributeId,
            Self::StrikethroughStyle => UIA_StrikethroughStyleAttributeId,
            Self::ForegroundColor => UIA_ForegroundColorAttributeId,
            Self::BackgroundColor => UIA_BackgroundColorAttributeId,
            Self::BulletStyle => UIA_BulletStyleAttributeId,
            Self::Link => UIA_LinkAttributeId,
        }
    }

    /// The attribute's bit in [`Attributes`].
    const fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// A set of [`TextAttribute`]s: which to read, or which a provider was
/// found not to support.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Attributes(u16);

impl Attributes {
    /// No attribute.
    pub const NONE: Self = Self(0);

    /// Every attribute.
    pub const ALL: Self = Self::of(&TextAttribute::ALL);

    /// The set of `attributes`.
    #[must_use]
    pub const fn of(attributes: &[TextAttribute]) -> Self {
        let mut bits = 0;
        let mut index = 0;
        while index < attributes.len() {
            bits |= attributes[index].bit();
            index += 1;
        }
        Self(bits)
    }

    /// This set and `attribute`.
    #[must_use]
    pub const fn with(self, attribute: TextAttribute) -> Self {
        Self(self.0 | attribute.bit())
    }

    /// Whether `attribute` is in the set.
    #[must_use]
    pub const fn contains(self, attribute: TextAttribute) -> bool {
        self.0 & attribute.bit() != 0
    }

    /// Both sets together.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// This set without the attributes of `other`.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The attributes in the set, in the order of [`TextAttribute::ALL`].
    pub fn iter(self) -> impl Iterator<Item = TextAttribute> {
        TextAttribute::ALL
            .into_iter()
            .filter(move |attribute| self.contains(*attribute))
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
    /// Those of [`attributes`](Self::attributes) whose support is not yet
    /// known: the answer says which of them the provider answered "not
    /// supported" for ([`CaretAnswer::unsupported`]). The annotation types
    /// are never learned this way, since a provider may answer "not
    /// supported" for text without annotations (Windows 11 Notepad does).
    pub learning: Attributes,
    /// The most UTF-16 code units of text read for a line or a unit.
    pub max_text: i32,
    /// The most UTF-16 code units read for each selection change, when the
    /// selection is not [`previous_selection`](Self::previous_selection).
    pub max_change_text: i32,
    /// A comparison with an end of the document, for a caret key that
    /// cannot take the caret past it ([`CaretAnswer::at_edge`]).
    pub edge: Option<EdgeQuery>,
}

/// A comparison of the caret, or of its line, with an end of the document
/// ([`CaretQuery::edge`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeQuery {
    /// Whether the caret's line is compared, rather than the caret.
    pub line: bool,
    /// Which end: the line's end and the document's, or the line's start
    /// and the document's; for the caret, the document's.
    pub end: Endpoint,
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
    /// The strikethrough style (0 none).
    pub strikethrough: Option<i32>,
    /// The foreground color, a `COLORREF` (0x00bbggrr).
    pub color: Option<i32>,
    /// The background color, a `COLORREF`.
    pub background_color: Option<i32>,
    /// The bullet style (0 none).
    pub bullet_style: Option<i32>,
    /// Whether the stretch is a link: `None` when the provider does not
    /// support links, false where it says there is none.
    pub link: Option<bool>,
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
    /// Of the query's [`learning`](CaretQuery::learning) attributes, those
    /// the provider answered "not supported" for in every stretch read:
    /// attributes it does not support; the rest of them it supports.
    /// Learned only from the formatting of a line or a unit with text, and
    /// `None` for any other read: a character's one read, or an empty
    /// line's, says too little about the provider.
    pub unsupported: Option<Attributes>,
    /// Whether the comparison [`CaretQuery::edge`] asked for holds: the
    /// caret, or the line's end, is at that end of the document; `None`
    /// without one. False, with no comparison made, when the caret or the
    /// selection moved, which is evidence already.
    pub at_edge: Option<bool>,
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

/// The registers of the formatting read: the stretches' lengths; one array
/// per attribute read, of the values as the provider gave them; and, for
/// each attribute being learned, whether the span answered "not supported"
/// for it.
struct RunRegisters {
    lengths: Reg<kind::Array>,
    values: Vec<(TextAttribute, Reg<kind::Array>)>,
    unsupported: Vec<(TextAttribute, Reg<kind::Bool>)>,
}

/// What a walk reads of each stretch: the attributes, and, when the span
/// was asked for its annotation types first, whether it has any, so a
/// stretch's are read only when it does.
#[derive(Clone, Copy)]
struct Reads<'a> {
    ids: &'a [TextAttribute],
    annotations: Option<Reg<kind::Bool>>,
}

/// The units a span's formatting is walked by, coarsest first: UIA's format
/// unit, and for a stretch one of whose attributes reads as UIA's "mixed"
/// (a provider whose format unit does not split where that attribute
/// changes), its words, then their characters, as NVDA walks a mixed
/// stretch. A character's attributes are taken as they come, a mixed one
/// as none.
const WALK_UNITS: [TextUnit; 3] = [TextUnit_Format, TextUnit_Word, TextUnit_Character];

/// Emits the reads of `run`'s attributes, in the order of `reads.ids`, and
/// whether any of them answered UIA's "mixed".
fn emit_attribute_reads(
    b: &mut Builder,
    run: Reg<kind::TextRange>,
    reads: Reads<'_>,
) -> (Vec<Reg<kind::Any>>, Reg<kind::Bool>) {
    let mixed = b.new_bool(false);
    let mut values = Vec::with_capacity(reads.ids.len());
    for &attribute in reads.ids {
        let id = b.int(attribute.id().0);
        let value = match (attribute, reads.annotations) {
            (TextAttribute::Annotations, Some(present)) => {
                let value = b.new_null();
                b.if_(present, |b| {
                    let read = b.text_range_get_attribute_value(run, id);
                    b.set(value, read);
                });
                value
            }
            _ => b.text_range_get_attribute_value(run, id),
        };
        let this = b.is(TypeTest::MixedAttribute, value);
        b.or_assign(mixed, this);
        values.push(value);
    }
    (values, mixed)
}

/// Emits the appending of the attribute values [`emit_attribute_reads`]
/// read, each to its array, as the provider gave them: the caller reads
/// them by their types ([`RunRegisters::read`]), so the program spends no
/// instructions on them.
fn emit_attributes(b: &mut Builder, read: &[Reg<kind::Any>], registers: &RunRegisters) {
    for (&(_, values), &value) in registers.values.iter().zip(read) {
        b.array_append(values, value);
    }
}

/// Emits the formatting read of `span`, whose text is `text`: one stretch
/// for a character, and otherwise a walk by the format unit, each stretch
/// cut at the span's end, and a stretch with a mixed attribute walked again
/// by finer units ([`WALK_UNITS`]), up to [`MAX_RUNS`] stretches in all.
/// The annotation types are asked of the whole span first, and of each
/// stretch only when the span has some; a span with none and nothing else
/// to read is one stretch, never walked. Each attribute of `learning` but
/// the annotation types is asked of the whole span too, whose "not
/// supported" says the provider does not support it.
fn emit_runs(
    b: &mut Builder,
    c: &Constants,
    (span, text): (Reg<kind::TextRange>, Reg<kind::Str>),
    character: bool,
    ids: &[TextAttribute],
    learning: Attributes,
) -> RunRegisters {
    let lengths = b.new_array();
    let lengths = b.add_to_results(lengths);
    let mut values = Vec::new();
    let mut unsupported = Vec::new();
    for &attribute in ids {
        let array = b.new_array();
        values.push((attribute, b.add_to_results(array)));
        if learning.contains(attribute) && attribute != TextAttribute::Annotations && !character {
            let id = b.int(attribute.id().0);
            let value = b.text_range_get_attribute_value(span, id);
            let not_supported = b.is(TypeTest::NotSupported, value);
            unsupported.push((attribute, b.add_to_results(not_supported)));
        }
    }
    let registers = RunRegisters {
        lengths,
        values,
        unsupported,
    };
    if character {
        // A character is one stretch, covering it whatever its length.
        let length = b.uint(0);
        b.array_append(lengths, length);
        let reads = Reads {
            ids,
            annotations: None,
        };
        let (read, _) = emit_attribute_reads(b, span, reads);
        emit_attributes(b, &read, &registers);
        return registers;
    }
    let count = Count {
        runs: b.new_int(0),
        limit: b.int(i32::try_from(MAX_RUNS).unwrap_or(i32::MAX)),
    };
    if !ids.contains(&TextAttribute::Annotations) {
        let reads = Reads {
            ids,
            annotations: None,
        };
        emit_walk(b, c, span, 0, reads, &registers, count);
        return registers;
    }
    let id = b.int(TextAttribute::Annotations.id().0);
    let types = b.text_range_get_attribute_value(span, id);
    let none = b.is(TypeTest::NotSupported, types);
    let present = b.not(none);
    let reads = Reads {
        ids,
        annotations: Some(present),
    };
    if ids.len() == 1 {
        b.if_else(
            present,
            |b| emit_walk(b, c, span, 0, reads, &registers, count),
            |b| {
                // No annotations and nothing else to read: the span is one
                // stretch, with none.
                let length = b.string_size(text);
                b.array_append(lengths, length);
                let null = b.new_null();
                for &(_, values) in &registers.values {
                    b.array_append(values, null);
                }
            },
        );
    } else {
        emit_walk(b, c, span, 0, reads, &registers, count);
    }
    registers
}

/// How many stretches a walk has appended, and the most it may.
#[derive(Clone, Copy)]
struct Count {
    runs: Reg<kind::Int>,
    limit: Reg<kind::Int>,
}

/// Emits the walk of `span` by `WALK_UNITS[level]`: from its start, a copy
/// whose end moves one unit on, cut at the span's end, its attributes read,
/// and then either walked again by the next unit, when one of them is mixed
/// and there is a next unit, or appended with its text's length; until the
/// span's end, or until the stretches appended reach the limit, which ends
/// every walk.
fn emit_walk(
    b: &mut Builder,
    c: &Constants,
    span: Reg<kind::TextRange>,
    level: usize,
    reads: Reads<'_>,
    registers: &RunRegisters,
    count: Count,
) {
    let unit = b.int(WALK_UNITS[level].0);
    let walker = c.collapsed(b, span);
    let going = b.new_bool(true);
    b.while_(
        |_| going,
        |b| {
            let run = b.text_range_clone(walker);
            let moved = b.text_range_move_endpoint_by_unit(run, c.end, unit, c.one);
            let order = b.text_range_compare_endpoints(run, c.end, span, c.end);
            let past = b.compare(order, c.zero, Comparison::GreaterThan);
            let stuck = b.equal(moved, c.zero);
            let cut = b.or(past, stuck);
            b.if_(cut, |b| {
                b.text_range_move_endpoint_by_range(run, c.end, span, c.end);
            });
            let (read, mixed) = emit_attribute_reads(b, run, reads);
            let append = |b: &mut Builder| {
                let text = b.text_range_get_text(run, c.max_text);
                let length = b.string_size(text);
                b.array_append(registers.lengths, length);
                emit_attributes(b, &read, registers);
                b.add_assign(count.runs, c.one);
            };
            if level + 1 < WALK_UNITS.len() {
                b.if_else(
                    mixed,
                    |b| emit_walk(b, c, run, level + 1, reads, registers, count),
                    append,
                );
            } else {
                append(b);
            }
            // The stretch reached the span's end when it was cut there or
            // ended on it, which the comparison above already says.
            let reached = b.compare(order, c.zero, Comparison::GreaterThanOrEqual);
            let done = b.or(reached, stuck);
            let full = b.compare(count.runs, count.limit, Comparison::GreaterThanOrEqual);
            let stop = b.or(done, full);
            b.if_else(
                stop,
                |b| {
                    let no = b.bool(false);
                    b.set(going, no);
                },
                |b| b.text_range_move_endpoint_by_range(walker, c.start, run, c.end),
            );
        },
    );
}

impl RunRegisters {
    /// The stretches, from the arrays the program filled: each value read
    /// by its type, a sentinel ("not supported", "mixed") or a value of
    /// another type as none.
    fn read(&self, outcome: &Outcome) -> Result<Vec<Run>, Error> {
        let lengths = outcome.get(self.lengths)?;
        let mut columns = Vec::new();
        for (attribute, values) in &self.values {
            columns.push((*attribute, outcome.get(*values)?));
        }
        Ok(lengths
            .iter()
            .enumerate()
            .map(|(index, length)| {
                let mut attributes = RunAttributes::default();
                for (attribute, values) in &columns {
                    let value = values.get(index);
                    match attribute {
                        TextAttribute::Annotations => {
                            let types = annotation_types(value);
                            attributes.spelling_error = types.contains(&ANNOTATION_SPELLING_ERROR);
                            attributes.grammar_error = types.contains(&ANNOTATION_GRAMMAR_ERROR);
                        }
                        TextAttribute::FontName => {
                            attributes.font_name = match value {
                                Some(Value::String(name)) if !name.is_empty() => Some(name.clone()),
                                _ => None,
                            };
                        }
                        TextAttribute::FontSize => attributes.font_size = double(value),
                        TextAttribute::FontWeight => attributes.font_weight = int(value),
                        TextAttribute::Italic => {
                            attributes.italic = match value {
                                Some(Value::Bool(italic)) => Some(*italic),
                                _ => None,
                            };
                        }
                        TextAttribute::UnderlineStyle => attributes.underline = int(value),
                        TextAttribute::StrikethroughStyle => {
                            attributes.strikethrough = int(value);
                        }
                        TextAttribute::ForegroundColor => attributes.color = int(value),
                        TextAttribute::BackgroundColor => {
                            attributes.background_color = int(value);
                        }
                        TextAttribute::BulletStyle => attributes.bullet_style = int(value),
                        TextAttribute::Link => {
                            // A link's value is the range it leads to; no
                            // link is null.
                            attributes.link = match value {
                                Some(Value::TextRange(_)) => Some(true),
                                Some(Value::Null) => Some(false),
                                _ => None,
                            };
                        }
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

    /// The attributes being learned that the span answered "not supported"
    /// for.
    fn unsupported(&self, outcome: &Outcome) -> Result<Attributes, Error> {
        let mut unsupported = Attributes::NONE;
        for &(attribute, flag) in &self.unsupported {
            if outcome.get(flag)? {
                unsupported = unsupported.with(attribute);
            }
        }
        Ok(unsupported)
    }
}

/// The annotation types a value returned holds: an array of them, or one
/// alone as a single integer; none for anything else.
fn annotation_types(value: Option<&Value>) -> Vec<i32> {
    match value {
        Some(Value::IntArray(types)) => types.clone(),
        Some(Value::Array(items)) => items.iter().filter_map(|item| int(Some(item))).collect(),
        Some(single) => int(Some(single)).into_iter().collect(),
        None => Vec::new(),
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
        pattern,
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
    let at_edge = query.edge.map(|edge| {
        let at = b.new_bool(false);
        let at = b.add_to_results(at);
        // Evidence already found needs no comparison.
        let found = b.or(moved, selection_moved);
        let still = b.not(found);
        b.if_(still, |b| {
            let document = b.text_pattern_get_document_range(pattern);
            let end = b.int(endpoint_number(edge.end));
            let order = if edge.line {
                b.text_range_compare_endpoints(line.range, end, document, end)
            } else {
                b.text_range_compare_endpoints(point, c.start, document, end)
            };
            let equal = b.equal(order, c.zero);
            b.set(at, equal);
        });
        at
    });
    let ids: Vec<TextAttribute> = query.attributes.iter().collect();
    let runs = match query.formats {
        Some(span) if !ids.is_empty() => {
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
            Some(emit_runs(
                &mut b,
                &c,
                (range, text),
                character,
                &ids,
                query.learning,
            ))
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
    let line = line.read(&outcome)?;
    let unit = unit.map(|unit| unit.read(&outcome)).transpose()?;
    let (runs, unsupported) = match runs {
        Some(runs) => {
            let unsupported = if spoken_text(query.formats, &line, unit.as_ref()) {
                Some(runs.unsupported(&outcome)?)
            } else {
                None
            };
            (runs.read(&outcome)?, unsupported)
        }
        None => (Vec::new(), None),
    };
    Ok(CaretAnswer {
        caret: caret_range,
        collapsed: outcome.get(collapsed)?,
        selection,
        moved: outcome.get(moved)?,
        selection_moved: outcome.get(selection_moved)?,
        line,
        unit,
        runs,
        changes: changes
            .map(|(selected, texts)| read_changes(&outcome, selected, texts))
            .transpose()?,
        unsupported,
        at_edge: at_edge.map(|at| outcome.get(at)).transpose()?,
    })
}

/// Whether the formatting read was of a line or a unit with text, the only
/// reads support is learned from: a character's one read, or an empty
/// line's, says too little about the provider.
fn spoken_text(formats: Option<FormatSpan>, line: &UnitRead, unit: Option<&UnitRead>) -> bool {
    match formats {
        Some(FormatSpan::Line) => !line.text.is_empty(),
        Some(FormatSpan::Unit) => !unit.unwrap_or(line).text.is_empty(),
        Some(FormatSpan::Character) | None => false,
    }
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
    /// The text pattern the caret was read through.
    pub(crate) pattern: Reg<kind::TextPattern>,
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
        pattern,
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

/// The values of `run`'s attributes, in the order of `ids`: all of them in
/// one call (`GetAttributeValues`), or, where the range cannot answer that,
/// one call each; an attribute whose read fails is not supported.
fn attribute_values(
    run: &IUIAutomationTextRange,
    ids: &[TextAttribute],
) -> Result<Vec<VARIANT>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(run.attributes(
        &ids.iter()
            .map(|attribute| attribute.id())
            .collect::<Vec<_>>(),
    )?)
}

/// The attributes `values` give, read in the order of `ids`: a sentinel,
/// "mixed" or "not supported", or a value of another type, as none.
fn attributes_of(ids: &[TextAttribute], values: &[VARIANT]) -> RunAttributes {
    let mut attributes = RunAttributes::default();
    for (attribute, value) in ids.iter().zip(values) {
        match attribute {
            TextAttribute::Annotations => {
                let types = verbatim_uia::variant_i32_array(value).unwrap_or_default();
                attributes.spelling_error = types.contains(&ANNOTATION_SPELLING_ERROR);
                attributes.grammar_error = types.contains(&ANNOTATION_GRAMMAR_ERROR);
            }
            TextAttribute::FontName => {
                attributes.font_name = (value.vt() == windows::Win32::System::Variant::VT_BSTR)
                    .then(|| verbatim_uia::variant_string(value))
                    .flatten();
            }
            TextAttribute::FontSize => attributes.font_size = verbatim_uia::variant_f64(value),
            TextAttribute::FontWeight => attributes.font_weight = plain_i32(value),
            TextAttribute::Italic => {
                attributes.italic = verbatim_uia::variant_optional_bool(value);
            }
            TextAttribute::UnderlineStyle => attributes.underline = plain_i32(value),
            TextAttribute::StrikethroughStyle => attributes.strikethrough = plain_i32(value),
            TextAttribute::ForegroundColor => attributes.color = plain_i32(value),
            TextAttribute::BackgroundColor => attributes.background_color = plain_i32(value),
            TextAttribute::BulletStyle => attributes.bullet_style = plain_i32(value),
            TextAttribute::Link => attributes.link = link_of(value),
        }
    }
    attributes
}

/// Whether a link attribute's value is a link, as the remote program reads
/// it: a text range is one; none, or "mixed", is not; and "not supported"
/// is `None`.
fn link_of(value: &VARIANT) -> Option<bool> {
    if verbatim_uia::is_not_supported(value) {
        return None;
    }
    Some(
        value.vt() == windows::Win32::System::Variant::VT_UNKNOWN
            && windows::core::IUnknown::try_from(value).is_ok()
            && !verbatim_uia::is_mixed(value),
    )
}

/// A variant's 32-bit integer, `None` for any other type.
fn plain_i32(value: &VARIANT) -> Option<i32> {
    (value.vt() == windows::Win32::System::Variant::VT_I4)
        .then(|| verbatim_uia::variant_i32(value))
        .flatten()
}

/// The formatting of `span` the classic way, as [`emit_runs`] emits it:
/// the stretches, and which of `learning` the span answered "not
/// supported" for. `length` is the span's text's length, the one stretch
/// of a span without annotations when nothing else is read. The span's
/// annotation types and the attributes being learned are asked in one
/// call.
fn classic_runs(
    (span, length): (&IUIAutomationTextRange, usize),
    character: bool,
    ids: &[TextAttribute],
    learning: Attributes,
    max_text: i32,
) -> Result<(Vec<Run>, Attributes), Error> {
    if character {
        return Ok((
            vec![Run {
                length: 0,
                attributes: attributes_of(ids, &attribute_values(span, ids)?),
            }],
            Attributes::NONE,
        ));
    }
    let annotations = ids.contains(&TextAttribute::Annotations);
    let learned: Vec<TextAttribute> = ids
        .iter()
        .copied()
        .filter(|attribute| {
            learning.contains(*attribute) && *attribute != TextAttribute::Annotations
        })
        .collect();
    let asked: Vec<TextAttribute> = annotations
        .then_some(TextAttribute::Annotations)
        .into_iter()
        .chain(learned.iter().copied())
        .collect();
    let answers = attribute_values(span, &asked)?;
    let mut unsupported = Attributes::NONE;
    let mut ids = ids.to_vec();
    for (attribute, value) in asked.iter().zip(&answers) {
        if !verbatim_uia::is_not_supported(value) {
            continue;
        }
        if *attribute == TextAttribute::Annotations {
            // No annotations in the span: none is read of its stretches.
            ids.retain(|attribute| *attribute != TextAttribute::Annotations);
        } else {
            unsupported = unsupported.with(*attribute);
        }
    }
    if ids.is_empty() {
        return Ok((
            vec![Run {
                length,
                attributes: RunAttributes::default(),
            }],
            unsupported,
        ));
    }
    let mut runs = Vec::new();
    classic_walk(span, 0, &ids, max_text, &mut runs)?;
    Ok((runs, unsupported))
}

/// The walk of `span` by `WALK_UNITS[level]` the classic way, as
/// [`emit_walk`] emits it, appending to `runs`.
fn classic_walk(
    span: &IUIAutomationTextRange,
    level: usize,
    ids: &[TextAttribute],
    max_text: i32,
    runs: &mut Vec<Run>,
) -> Result<(), Error> {
    let walker = collapsed(span)?;
    loop {
        let run = walker.clone_range()?;
        let moved = run.move_endpoint_by_unit(Endpoint::End, WALK_UNITS[level], 1)?;
        let order = run.compare_endpoints(Endpoint::End, span, Endpoint::End)?;
        if order > 0 || moved == 0 {
            run.move_endpoint_to(Endpoint::End, span, Endpoint::End)?;
        }
        let values = attribute_values(&run, ids)?;
        if level + 1 < WALK_UNITS.len() && values.iter().any(verbatim_uia::is_mixed) {
            classic_walk(&run, level + 1, ids, max_text, runs)?;
        } else {
            runs.push(Run {
                length: run.text(max_text)?.len(),
                attributes: attributes_of(ids, &values),
            });
        }
        // The stretch reached the span's end when it was cut there or ended
        // on it, which the comparison already says.
        let done = order >= 0 || moved == 0;
        if done || runs.len() >= MAX_RUNS as usize {
            return Ok(());
        }
        walker.move_endpoint_to(Endpoint::Start, &run, Endpoint::End)?;
    }
}

// The caret read the classic way, the fallback and the reference: the
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
    let at_edge = query
        .edge
        .map(|edge| -> Result<bool, Error> {
            if moved || selection_moved {
                // Evidence already found needs no comparison.
                return Ok(false);
            }
            let document = query.pattern.document_range()?;
            let order = if edge.line {
                line.range
                    .compare_endpoints(edge.end, &document, edge.end)?
            } else {
                point.compare_endpoints(Endpoint::Start, &document, edge.end)?
            };
            Ok(order == 0)
        })
        .transpose()?;
    let (runs, unsupported) = classic_formats(query, &point, &line, unit.as_ref())?;
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
        unsupported,
        at_edge,
    })
}

/// The formatting the query asks for the classic way, of the span it
/// names at `point`, given the caret's `line` and `unit`, and what was
/// learned of the attributes' support, as [`caret_read_remote`] reads them.
fn classic_formats(
    query: &CaretQuery<'_>,
    point: &IUIAutomationTextRange,
    line: &UnitRead,
    unit: Option<&UnitRead>,
) -> Result<(Vec<Run>, Option<Attributes>), Error> {
    let ids: Vec<TextAttribute> = query.attributes.iter().collect();
    let Some(span) = query.formats.filter(|_| !ids.is_empty()) else {
        return Ok((Vec::new(), None));
    };
    let learns = spoken_text(Some(span), line, unit);
    let learning = if learns {
        query.learning
    } else {
        Attributes::NONE
    };
    let (runs, unsupported) = match span {
        FormatSpan::Line | FormatSpan::Unit => {
            let read = match span {
                FormatSpan::Unit => unit.unwrap_or(line),
                _ => line,
            };
            classic_runs(
                (&read.range, read.text.len()),
                false,
                &ids,
                learning,
                query.max_text,
            )?
        }
        FormatSpan::Character => {
            let character = point.clone_range()?;
            character.expand(TextUnit_Character)?;
            classic_runs((&character, 0), true, &ids, learning, query.max_text)?
        }
    };
    Ok((runs, learns.then_some(unsupported)))
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
