//! The pieces of a text's unit, range and location reads that the batch's
//! two executors share (the request list's design, section 5): the points
//! a read starts from as the protocol names them ([`TextFrom`]), the
//! emitters the program builder puts into a program ([`emit_units`],
//! [`emit_between`], [`emit_after`]), and the classic reads the
//! ordinary-calls executor makes one call at a time ([`classic_point`],
//! [`classic_units`], [`classic_range`], [`classic_location`]).

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit,
    TextUnit_Character, UIA_CultureAttributeId,
};

use verbatim_uia::text::{Endpoint, Language, TextPatternExt, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::caret::Constants;
use crate::error::Error;
use crate::instruction::TypeTest;
use crate::opcode::Comparison;
use crate::operation::{Outcome, Value};

/// The text a read is in: the element, which a program starts from, and
/// its text pattern, which the classic implementations read.
#[derive(Clone, Copy)]
pub struct TextTarget<'a> {
    /// The element with the text.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern.
    pub pattern: &'a IUIAutomationTextPattern,
}

/// A position the caller holds: an end of a range, and whether the range
/// is known to be collapsed (so either end is the position).
#[derive(Clone, Copy)]
pub struct Position<'a> {
    /// The range.
    pub range: &'a IUIAutomationTextRange,
    /// Which of its ends.
    pub endpoint: Endpoint,
    /// The range is known to be collapsed.
    pub collapsed: bool,
}

/// Where a read starts, as the text protocol names a point.
#[derive(Clone, Copy)]
pub enum TextFrom<'a> {
    /// The caret: the start of the selection's first range, as NVDA's
    /// caret is, or the start of the text when it has no caret.
    Caret,
    /// The selection's start, the caret when nothing is selected.
    SelectionStart,
    /// The selection's end, the caret when nothing is selected.
    SelectionEnd,
    /// The start of the text.
    Start,
    /// The end of the text.
    End,
    /// A position the caller holds.
    At(Position<'a>),
    /// The position `prefix.len()` UTF-16 code units after `from`, where
    /// `prefix` is the text between them as the caller read it. The
    /// provider's character may be a code unit, a code point, or a
    /// grapheme cluster, so each of `counts` (distinct character counts,
    /// the code units first) is tried until the text passed matches, the
    /// last tried being kept when none does.
    After {
        /// The position counted from.
        from: Position<'a>,
        /// The text between the two.
        prefix: &'a [u16],
        /// The character counts to try, in order.
        counts: &'a [i32],
    },
}

/// A point found: a range and the end of it that is the point.
#[derive(Clone, Debug)]
pub struct FoundPoint {
    /// The range.
    pub range: IUIAutomationTextRange,
    /// Which of its ends is the point.
    pub endpoint: Endpoint,
    /// The range is known to be collapsed.
    pub collapsed: bool,
}

impl FoundPoint {
    /// A collapsed range, as a program returns every point.
    pub(crate) fn collapsed(range: IUIAutomationTextRange) -> Self {
        Self {
            range,
            endpoint: Endpoint::Start,
            collapsed: true,
        }
    }
}

/// How a unit read moves before it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Movement {
    /// From the start of the unit containing the point, by `count` units,
    /// landing on a unit's start.
    By(TextUnit, i32),
    /// To the start of the text when the count is negative, else to its
    /// end; the count's sign is how far it moved, or zero when the point
    /// was there already (or the count is zero).
    Document(i32),
}

/// One unit read.
#[derive(Debug)]
pub struct UnitText {
    /// Its range.
    pub range: IUIAutomationTextRange,
    /// Its text, at most the query's `max_text` code units.
    pub text: Vec<u16>,
    /// How many code units of it come before the point reached (zero for
    /// every unit after the first, and for a first one moved onto).
    pub offset: usize,
    /// Its language as a BCP 47 tag, when asked for and the provider gives
    /// one for the whole unit.
    pub language: Option<String>,
}

/// Units read the classic way ([`classic_units`]).
#[derive(Debug)]
pub(crate) struct UnitsAnswer {
    /// How far the movement went: less than asked at the text's ends.
    pub(crate) moved: i32,
    /// The units read, in order, at least one.
    pub(crate) units: Vec<UnitText>,
    /// Reading ahead found no unit after the last one read: the text ends
    /// there. Never set for a single unit's read.
    pub(crate) ended: bool,
}

// The remote programs.

/// Emits the whole text's range of `element`.
pub(crate) fn document_of(b: &mut Builder, element: Reg<kind::Element>) -> Reg<kind::TextRange> {
    let pattern = b.get_text_pattern(element, false);
    b.text_pattern_get_document_range(pattern)
}

/// Emits the advance of [`TextFrom::After`] from the collapsed range
/// `base`: each of `counts` tried until the text passed equals `prefix`,
/// compared inside the provider, the last tried kept when none does.
pub(crate) fn emit_after(
    b: &mut Builder,
    c: &Constants,
    base: Reg<kind::TextRange>,
    (prefix, counts): (&[u16], &[i32]),
) -> Reg<kind::TextRange> {
    let last = b.new_null().assume::<kind::TextRange>();
    b.set(last, base);
    let found = b.new_bool(false);
    let wanted = b.string(&String::from_utf16_lossy(prefix));
    let limit = b.int(i32::try_from(prefix.len().saturating_add(1)).unwrap_or(i32::MAX));
    let character = b.int(TextUnit_Character.0);
    for &count in counts {
        let searching = b.not(found);
        b.if_(searching, |b| {
            let range = b.text_range_clone(base);
            let count = b.int(count);
            b.text_range_move_endpoint_by_unit(range, c.end, character, count);
            let passed = b.text_range_get_text(range, limit);
            b.set(last, range);
            let same = b.equal(passed, wanted);
            b.if_(same, |b| {
                let yes = b.bool(true);
                b.set(found, yes);
            });
        });
    }
    b.text_range_move_endpoint_by_range(last, c.start, last, c.end);
    last
}

/// Emits the units' language, as the classic reads find it: one `Culture`
/// read over them all, from `first`'s start to `last`'s end (over `first`
/// alone without a `last`), and, only when that answers UIA's "mixed" for
/// more than one unit, a read of each of `ranges`, appended in order to the
/// returned array, which is otherwise left empty. Each value is kept when
/// it is a plain integer (a locale id), else set to null.
pub(crate) fn emit_languages(
    b: &mut Builder,
    c: &Constants,
    (first, last): (Reg<kind::TextRange>, Option<Reg<kind::TextRange>>),
    ranges: Reg<kind::Array>,
) -> (Reg<kind::Any>, Reg<kind::Array>) {
    {
        let id = b.int(UIA_CultureAttributeId.0);
        let whole = match last {
            Some(last) => {
                let whole = b.text_range_clone(first);
                b.text_range_move_endpoint_by_range(whole, c.end, last, c.end);
                whole
            }
            None => first,
        };
        let value = b.text_range_get_attribute_value(whole, id);
        let per_unit = b.new_array();
        let mixed = b.is(TypeTest::MixedAttribute, value);
        let size = b.array_size(ranges);
        let one = b.uint(1);
        let several = b.compare(size, one, Comparison::GreaterThan);
        let apart = b.and(mixed, several);
        b.if_(apart, |b| {
            let index = b.new_uint(0);
            b.while_(
                |b| b.compare(index, size, Comparison::LessThan),
                |b| {
                    let range = b.array_get_at(ranges, index).assume::<kind::TextRange>();
                    let value = b.text_range_get_attribute_value(range, id);
                    plain_locale(b, value);
                    b.array_append(per_unit, value);
                    b.add_assign(index, one);
                },
            );
        });
        plain_locale(b, value);
        (value, per_unit)
    }
}

/// Emits the setting of `value` to null unless it is a plain integer.
fn plain_locale(b: &mut Builder, value: Reg<kind::Any>) {
    let plain = b.is(TypeTest::Int, value);
    let other = b.not(plain);
    b.if_(other, |b| {
        let null = b.new_null();
        b.set(value, null);
    });
}

/// The language of a locale id the program returned.
pub(crate) fn language(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Int(lcid) => verbatim_uia::text::locale_name(u32::try_from(*lcid).ok()?),
        Value::Uint(lcid) => verbatim_uia::text::locale_name(*lcid),
        _ => None,
    }
}

/// Emits the correction of `moved`, the count a backward move returned, to
/// a negative count: some providers answer a backward move with a positive
/// one, which NVDA corrects (`docs/parity/speech-audio.md`, "Text range moves"), and the
/// classic reads correct too (`verbatim_uia::text::signed_move`).
pub(crate) fn emit_backward_count(b: &mut Builder, moved: Reg<kind::Int>) {
    let zero = b.int(0);
    let positive = b.compare(moved, zero, Comparison::GreaterThan);
    b.if_(positive, |b| {
        let negative = b.subtract(zero, moved);
        b.set(moved, negative);
    });
}

/// The registers of a unit read ([`emit_units`]).
#[derive(Clone, Copy)]
pub(crate) struct UnitsRegisters {
    moved: Reg<kind::Int>,
    /// Each unit's range, in order.
    pub(crate) ranges: Reg<kind::Array>,
    texts: Reg<kind::Array>,
    offset: Reg<kind::Uint>,
    ended: Reg<kind::Bool>,
    language: Option<(Reg<kind::Any>, Reg<kind::Array>)>,
    /// The first unit's range and text.
    pub(crate) first: (Reg<kind::TextRange>, Reg<kind::Str>),
}

/// Emits a unit read from `from` in the text of `element`: after
/// `movement`, `count` units of `unit`, reading on while the text read is
/// shorter than `max_total`, with their language when `culture`.
#[expect(clippy::too_many_lines, reason = "one read, top to bottom as it runs")]
pub(crate) fn emit_units(
    b: &mut Builder,
    c: &Constants,
    (from, element): (Reg<kind::TextRange>, Reg<kind::Element>),
    (movement, unit_read, count): (Option<Movement>, TextUnit, u32),
    (max_total, culture): (u32, bool),
) -> UnitsRegisters {
    let moved = b.new_int(0);
    let moved = b.add_to_results(moved);
    let unit = b.int(unit_read.0);
    let (point, on_start) = match movement {
        None => (from, false),
        Some(Movement::Document(count)) => {
            let document = document_of(b, element);
            let target = if count < 0 {
                c.collapsed(b, document)
            } else {
                let copy = b.text_range_clone(document);
                b.text_range_move_endpoint_by_range(copy, c.start, copy, c.end);
                copy
            };
            if count != 0 {
                let order = b.text_range_compare_endpoints(from, c.start, target, c.start);
                let differs = b.not_equal(order, c.zero);
                b.if_(differs, |b| {
                    let sign = b.int(count.signum());
                    b.set(moved, sign);
                });
            }
            (target, false)
        }
        Some(Movement::By(by, count)) => {
            let by_unit = b.int(by.0);
            let range = b.text_range_clone(from);
            b.text_range_expand_to_enclosing_unit(range, by_unit);
            b.text_range_move_endpoint_by_range(range, c.end, range, c.start);
            let backward = count < 0;
            let count = b.int(count);
            // A collapsed range stays collapsed when it moves.
            let went = b.text_range_move(range, by_unit, count);
            if backward {
                emit_backward_count(b, went);
            }
            b.set(moved, went);
            (range, by == unit_read)
        }
    };
    let point = b.add_to_results(point);
    let ranges = b.new_array();
    let ranges = b.add_to_results(ranges);
    let texts = b.new_array();
    let texts = b.add_to_results(texts);
    let first = b.text_range_clone(point);
    b.text_range_expand_to_enclosing_unit(first, unit);
    let text = b.text_range_get_text(first, c.max_text);
    b.array_append(ranges, first);
    b.array_append(texts, text);
    let current = b.new_null().assume::<kind::TextRange>();
    b.set(current, first);
    let offset = if on_start {
        b.new_uint(0)
    } else {
        let before = b.text_range_clone(first);
        b.text_range_move_endpoint_by_range(before, c.end, point, c.start);
        let before = b.text_range_get_text(before, c.max_text);
        b.string_size(before)
    };
    let offset = b.add_to_results(offset);
    let ended = b.new_bool(false);
    let ended = b.add_to_results(ended);
    if count > 1 {
        let read = b.new_int(1);
        let wanted = b.int(i32::try_from(count).unwrap_or(i32::MAX));
        let total = b.string_size(text);
        let max_total = b.uint(max_total);
        let going = b.new_bool(true);
        b.while_(
            |_| going,
            |b| {
                let stop = |b: &mut Builder, at_end: bool| {
                    if at_end {
                        let yes = b.bool(true);
                        b.set(ended, yes);
                    }
                    let no = b.bool(false);
                    b.set(going, no);
                    b.break_loop();
                };
                // As the classic reads: a collapsed copy moved, which stays
                // collapsed, then expanded in place.
                let next = b.text_range_clone(current);
                b.text_range_move_endpoint_by_range(next, c.end, next, c.start);
                let went = b.text_range_move(next, unit, c.one);
                let stuck = b.equal(went, c.zero);
                b.if_(stuck, |b| stop(b, true));
                let enough = b.compare(read, wanted, Comparison::GreaterThanOrEqual);
                let long = b.compare(total, max_total, Comparison::GreaterThanOrEqual);
                let full = b.or(enough, long);
                b.if_(full, |b| stop(b, false));
                b.text_range_expand_to_enclosing_unit(next, unit);
                // A unit that starts before the one read last is no unit
                // after it: the console host clamps an expansion below its
                // last row with text back onto that row, so the text ends.
                let order = b.text_range_compare_endpoints(next, c.start, current, c.end);
                let back = b.compare(order, c.zero, Comparison::LessThan);
                b.if_(back, |b| stop(b, true));
                let text = b.text_range_get_text(next, c.max_text);
                b.array_append(ranges, next);
                b.array_append(texts, text);
                let size = b.string_size(text);
                b.add_assign(total, size);
                b.set(current, next);
                b.add_assign(read, c.one);
            },
        );
    }
    let language = culture.then(|| {
        let (language, languages) =
            emit_languages(b, c, (first, (count > 1).then_some(current)), ranges);
        (b.add_to_results(language), b.add_to_results(languages))
    });
    UnitsRegisters {
        moved,
        ranges,
        texts,
        offset,
        ended,
        language,
        first: (first, text),
    }
}

impl UnitsRegisters {
    /// How far the movement went, the units read, and whether the text
    /// ended after them.
    pub(crate) fn read(&self, outcome: &Outcome) -> Result<(i32, Vec<UnitText>, bool), Error> {
        let ranges = outcome.get(self.ranges)?;
        let texts = outcome.get(self.texts)?;
        let (whole, each) = match self.language {
            Some((language, languages)) => (
                self::language(Some(&outcome.get(language)?)),
                outcome.get(languages)?,
            ),
            None => (None, Vec::new()),
        };
        let first_offset = usize::try_from(outcome.get(self.offset)?).unwrap_or(usize::MAX);
        let mut units = Vec::with_capacity(ranges.len());
        for (index, range) in ranges.into_iter().enumerate() {
            let Value::TextRange(range) = range else {
                return Err(Error::MissingResult(self.ranges.id()));
            };
            let text: Vec<u16> = match texts.get(index) {
                Some(Value::String(text)) => text.encode_utf16().collect(),
                _ => Vec::new(),
            };
            units.push(UnitText {
                range,
                offset: if index == 0 {
                    first_offset.min(text.len())
                } else {
                    0
                },
                text,
                language: if each.is_empty() {
                    whole.clone()
                } else {
                    self::language(each.get(index))
                },
            });
        }
        if units.is_empty() {
            return Err(Error::MissingResult(self.ranges.id()));
        }
        Ok((outcome.get(self.moved)?, units, outcome.get(self.ended)?))
    }
}

/// Emits the range from the earlier of `start` and `end` to the point just
/// past the character at the later, `past_start` or `past_end`, which are
/// the points themselves for no character.
pub(crate) fn emit_between(
    b: &mut Builder,
    c: &Constants,
    (start, end): (Reg<kind::TextRange>, Reg<kind::TextRange>),
    (past_start, past_end): (Reg<kind::TextRange>, Reg<kind::TextRange>),
) -> Reg<kind::TextRange> {
    let order = b.text_range_compare_endpoints(start, c.start, end, c.start);
    let first = b.new_null().assume::<kind::TextRange>();
    let second = b.new_null().assume::<kind::TextRange>();
    let reversed = b.compare(order, c.zero, Comparison::GreaterThan);
    b.if_else(
        reversed,
        |b| {
            b.set(first, end);
            b.set(second, past_start);
        },
        |b| {
            b.set(first, start);
            b.set(second, past_end);
        },
    );
    let range = b.text_range_clone(first);
    b.text_range_move_endpoint_by_range(range, c.end, second, c.start);
    range
}

// The classic implementations: the same reads, one call each, counted on
// the thread's `verbatim_uia::calls`.

/// The caret and the selection as the classic reads find them.
pub(crate) struct CaretState {
    pub(crate) caret: FoundPoint,
    pub(crate) selection: Option<IUIAutomationTextRange>,
}

/// Reads the caret and the selection.
pub(crate) fn classic_caret(target: TextTarget<'_>) -> Result<CaretState, Error> {
    let ranges = target.pattern.selection()?;
    let Some(first) = ranges.first() else {
        return Ok(CaretState {
            caret: FoundPoint {
                range: target.pattern.document_range()?,
                endpoint: Endpoint::Start,
                collapsed: false,
            },
            selection: None,
        });
    };
    if first.compare_endpoints(Endpoint::Start, first, Endpoint::End)? == 0 {
        return Ok(CaretState {
            caret: FoundPoint::collapsed(first.clone()),
            selection: None,
        });
    }
    let caret = FoundPoint {
        range: first.clone(),
        endpoint: Endpoint::Start,
        collapsed: false,
    };

    Ok(CaretState {
        caret,
        selection: Some(first.clone()),
    })
}

/// A copy of `at`'s range whose start is the point. One call, or two.
pub(crate) fn starting_at(at: &FoundPoint) -> Result<IUIAutomationTextRange, Error> {
    let range = at.range.clone_range()?;
    if at.endpoint == Endpoint::End && !at.collapsed {
        range.move_endpoint_to(Endpoint::Start, &range, Endpoint::End)?;
    }
    Ok(range)
}

/// A copy of `at`'s range collapsed to the point. One call, or two.
pub(crate) fn collapsed_copy(at: &FoundPoint) -> Result<IUIAutomationTextRange, Error> {
    let range = starting_at(at)?;
    if at.endpoint == Endpoint::Start && !at.collapsed {
        range.move_endpoint_to(Endpoint::End, &range, Endpoint::Start)?;
    }
    Ok(range)
}

/// A held position as a point found, with no call.
fn held(position: Position<'_>) -> FoundPoint {
    FoundPoint {
        range: position.range.clone(),
        endpoint: position.endpoint,
        collapsed: position.collapsed,
    }
}

/// The caret state, read into `caret` the first time it is needed.
fn caret_state<'c>(
    target: TextTarget<'_>,
    caret: &'c mut Option<CaretState>,
) -> Result<&'c CaretState, Error> {
    if caret.is_none() {
        *caret = Some(classic_caret(target)?);
    }
    caret
        .as_ref()
        .ok_or_else(|| Error::Uia(windows::core::Error::empty()))
}

/// Finds the point `from` names, reading the caret into `caret` the first
/// time it is needed.
pub(crate) fn classic_point(
    target: TextTarget<'_>,
    from: TextFrom<'_>,
    caret: &mut Option<CaretState>,
) -> Result<FoundPoint, Error> {
    Ok(match from {
        TextFrom::Caret => caret_state(target, caret)?.caret.clone(),
        TextFrom::SelectionStart | TextFrom::SelectionEnd => {
            let state = caret_state(target, caret)?;
            match &state.selection {
                Some(selection) => FoundPoint {
                    range: selection.clone(),
                    endpoint: if matches!(from, TextFrom::SelectionEnd) {
                        Endpoint::End
                    } else {
                        Endpoint::Start
                    },
                    collapsed: false,
                },
                None => state.caret.clone(),
            }
        }
        TextFrom::Start | TextFrom::End => FoundPoint {
            range: target.pattern.document_range()?,
            endpoint: if matches!(from, TextFrom::End) {
                Endpoint::End
            } else {
                Endpoint::Start
            },
            collapsed: false,
        },
        TextFrom::At(position) => held(position),
        TextFrom::After {
            from,
            prefix,
            counts,
        } => {
            let from = held(from);
            let limit = i32::try_from(prefix.len().saturating_add(1)).unwrap_or(i32::MAX);
            let mut last = None;
            for &count in counts {
                let range = collapsed_copy(&from)?;
                range.move_endpoint_by_unit(Endpoint::End, TextUnit_Character, count)?;
                let matched = range.text(limit)? == prefix;
                last = Some(range);
                if matched {
                    break;
                }
            }
            let range = match last {
                Some(range) => range,
                None => collapsed_copy(&from)?,
            };
            range.move_endpoint_to(Endpoint::Start, &range, Endpoint::End)?;
            FoundPoint::collapsed(range)
        }
    })
}

/// Units read the classic way from `from`, found: after `movement`,
/// `count` units of `unit`, one call at a time.
pub(crate) fn classic_units(
    target: TextTarget<'_>,
    from: FoundPoint,
    (movement, unit, count): (Option<Movement>, TextUnit, u32),
    (max_text, max_total, culture): (i32, u32, bool),
) -> Result<UnitsAnswer, Error> {
    let (point, moved, on_start) = match movement {
        None => (from, 0, false),
        Some(Movement::Document(count)) => {
            let point = FoundPoint {
                range: target.pattern.document_range()?,
                endpoint: if count < 0 {
                    Endpoint::Start
                } else {
                    Endpoint::End
                },
                collapsed: false,
            };
            let moved = if count == 0
                || from
                    .range
                    .compare_endpoints(from.endpoint, &point.range, point.endpoint)?
                    == 0
            {
                0
            } else {
                count.signum()
            };
            (point, moved, false)
        }
        Some(Movement::By(by, count)) => {
            // Expanding normalizes from the start alone, so a copy whose
            // start is the point is enough; a collapsed range stays
            // collapsed when it moves.
            let range = starting_at(&from)?;
            range.expand(by)?;
            range.move_endpoint_to(Endpoint::End, &range, Endpoint::Start)?;
            let moved = range.move_by(by, count)?;
            (FoundPoint::collapsed(range), moved, by == unit)
        }
    };
    let read = |range: IUIAutomationTextRange| -> Result<UnitText, Error> {
        range.expand(unit)?;
        let text = range.text(max_text)?;
        Ok(UnitText {
            range,
            text,
            offset: 0,
            language: None,
        })
    };
    let mut first = read(starting_at(&point)?)?;
    if !on_start {
        let before = first.range.clone_range()?;
        before.move_endpoint_to(Endpoint::End, &point.range, point.endpoint)?;
        first.offset = before.text(max_text)?.len().min(first.text.len());
    }
    let mut total = first.text.len();
    let mut units = vec![first];
    let mut ended = false;
    if count > 1 {
        while let Some(current) = units.last() {
            // The next unit's start, by moving a collapsed copy: a whole
            // unit moved stops short of an empty last line, which a
            // collapsed range reaches. It stays collapsed, so it is
            // expanded in place.
            let next = current.range.clone_range()?;
            next.move_endpoint_to(Endpoint::End, &next, Endpoint::Start)?;
            if next.move_by(unit, 1)? == 0 {
                ended = true;
                break;
            }
            if units.len() >= count as usize || total >= max_total as usize {
                break;
            }
            let unit = read(next)?;
            // A unit that starts before the one read last is no unit after
            // it, as the program checks.
            if unit
                .range
                .compare_endpoints(Endpoint::Start, &current.range, Endpoint::End)?
                < 0
            {
                ended = true;
                break;
            }
            total += unit.text.len();
            units.push(unit);
        }
    }
    if culture {
        classic_languages(&mut units, count > 1)?;
    }
    Ok(UnitsAnswer {
        moved,
        units,
        ended,
    })
}

/// Sets each unit's language: one `Culture` read over all of them, from the
/// first one's start to the last one's end (`whole`, a read ahead; else the
/// first unit's own range), and a read per unit only when that one answers
/// "mixed" for more than one unit. A read that fails gives no language, as
/// UIA's "not supported" does.
fn classic_languages(units: &mut [UnitText], whole: bool) -> Result<(), Error> {
    let (Some(first), Some(last)) = (units.first(), units.last()) else {
        return Ok(());
    };
    let language = if whole {
        let whole = first.range.clone_range()?;
        whole.move_endpoint_to(Endpoint::End, &last.range, Endpoint::End)?;
        whole.language()
    } else {
        first.range.language()
    }
    .unwrap_or(Language::Unknown);
    match language {
        Language::Tag(tag) => {
            for unit in units.iter_mut() {
                unit.language = Some(tag.clone());
            }
        }
        Language::Mixed if units.len() > 1 => {
            for unit in units.iter_mut() {
                unit.language = unit.range.culture().ok().flatten();
            }
        }
        Language::Mixed | Language::Unknown => {}
    }
    Ok(())
}

/// The range between `start` and `end`, whichever comes first, and with
/// `through`, through the character at the later one, as
/// the ordinary-calls executor reads or selects it.
pub(crate) fn classic_range(
    start: &FoundPoint,
    end: &FoundPoint,
    through: Option<(&FoundPoint, &FoundPoint)>,
) -> Result<IUIAutomationTextRange, Error> {
    let reversed = start
        .range
        .compare_endpoints(start.endpoint, &end.range, end.endpoint)?
        > 0;
    // Through the character at the later end, when asked.
    let (first, second) = match (through, reversed) {
        (None, true) => (end, start),
        (None, false) => (start, end),
        (Some((past_start, _)), true) => (end, past_start),
        (Some((_, past_end)), false) => (start, past_end),
    };
    let range = starting_at(first)?;
    range.move_endpoint_to(Endpoint::End, &second.range, second.endpoint)?;
    Ok(range)
}

/// The left and top of the character at `at`, in screen pixels, when the
/// provider gives a rectangle: three calls.
pub(crate) fn classic_location(at: &FoundPoint) -> Result<Option<(f64, f64)>, Error> {
    let range = starting_at(at)?;
    range.expand(TextUnit_Character)?;
    let rectangles = range.bounding_rectangles()?;
    Ok(match rectangles.as_slice() {
        [left, top, ..] => Some((*left, *top)),
        _ => None,
    })
}
