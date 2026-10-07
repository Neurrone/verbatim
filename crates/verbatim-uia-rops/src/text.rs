//! Reads of a text's units and ranges for the text protocol's requests
//! (`docs/crates/verbatim-model.md`, "The text protocol"): a unit read
//! after a movement, or several units ahead for say-all ([`text_units`]);
//! the text between two points, or selecting it ([`text_range`]); and a
//! point's place on the screen ([`text_location`]). Each starts from a
//! point given as the protocol gives it ([`TextFrom`]): the caret, a
//! selection's end, an end of the text, a position the outpost holds, or a
//! position some characters after one, found by matching the text passed.
//!
//! Each has a remote program, one cross-process round trip, and a classic
//! implementation behind the same signature that makes the same reads one
//! call at a time, as the outpost made them before remote operations; the
//! entry point picks between them as [`crate::caret_read`] does.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    IUIAutomationTextRange, TextUnit, TextUnit_Character, UIA_CultureAttributeId,
};

use verbatim_uia::text::{Endpoint, Language, TextPatternExt, TextRangeExt, caret_range};

use crate::builder::{Builder, Reg, kind};
use crate::caret::{
    CaretRegisters, Constants, emit_caret, endpoint_number, gone_or_timed_out, string_of,
};
use crate::error::Error;
use crate::focus::Path;
use crate::instruction::TypeTest;
use crate::opcode::Comparison;
use crate::operation::{Outcome, Value};

/// The text a read is in: the element, which a program starts from, and
/// its text patterns, which the classic implementations read.
#[derive(Clone, Copy)]
pub struct TextTarget<'a> {
    /// The element with the text.
    pub element: &'a IUIAutomationElement,
    /// Its text pattern.
    pub pattern: &'a IUIAutomationTextPattern,
    /// Its `TextPattern2`, when the provider has one.
    pub pattern2: Option<&'a IUIAutomationTextPattern2>,
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
    /// The caret: the selection's first range collapsed, the caret
    /// `TextPattern2` reports when text is selected, or the start of the
    /// text when it has no caret.
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
    fn collapsed(range: IUIAutomationTextRange) -> Self {
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

/// What [`text_units`] is asked.
#[derive(Clone, Copy)]
pub struct UnitsQuery<'a> {
    /// The text.
    pub target: TextTarget<'a>,
    /// Where to start.
    pub from: TextFrom<'a>,
    /// How to move before reading, if at all.
    pub movement: Option<Movement>,
    /// The unit to read.
    pub unit: TextUnit,
    /// How many units to read: one for a read, more for say-all's reading
    /// ahead, each later one the unit after the one before.
    pub count: u32,
    /// The most UTF-16 code units of text read for one unit.
    pub max_text: i32,
    /// Units after the first are read only while the text read so far is
    /// shorter than this, in UTF-16 code units, so a batch of long units
    /// stays bounded.
    pub max_total: u32,
    /// Whether to read each unit's language (UIA's `Culture` attribute).
    pub culture: bool,
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

/// The answer to a [`UnitsQuery`].
#[derive(Debug)]
pub struct UnitsAnswer {
    /// The starting point, found.
    pub from: FoundPoint,
    /// The point reached by the movement, which the first unit contains.
    pub point: FoundPoint,
    /// How far the movement went: less than asked at the text's ends.
    pub moved: i32,
    /// The units read, in order, at least one.
    pub units: Vec<UnitText>,
    /// Reading ahead found no unit after the last one read: the text ends
    /// there. Never set for a single unit's read.
    pub ended: bool,
}

/// The signature both unit read implementations share.
pub type TextUnitsFn = fn(&UnitsQuery<'_>) -> Result<UnitsAnswer, Error>;

/// What [`text_range`] does with the text between its two points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeAction {
    /// Reads it, at most this many UTF-16 code units.
    Text(i32),
    /// Selects it, which also puts the caret there.
    Select,
}

/// What [`text_range`] is asked.
#[derive(Clone, Copy)]
pub struct RangeQuery<'a> {
    /// The text.
    pub target: TextTarget<'a>,
    /// One end.
    pub start: TextFrom<'a>,
    /// The other end; `None` for the same point as `start` (moving the
    /// caret there).
    pub end: Option<TextFrom<'a>>,
    /// What to do with the text between them, whichever comes first.
    pub action: RangeAction,
}

/// The answer to a [`RangeQuery`].
#[derive(Debug)]
pub struct RangeAnswer {
    /// The query's `start`, found.
    pub start: FoundPoint,
    /// The query's `end`, found (`start` again when it had none).
    pub end: FoundPoint,
    /// The text read, for [`RangeAction::Text`].
    pub text: Vec<u16>,
    /// Whether the selection was made, for [`RangeAction::Select`].
    pub selected: bool,
}

/// The signature both range implementations share.
pub type TextRangeFn = fn(&RangeQuery<'_>) -> Result<RangeAnswer, Error>;

/// What [`text_location`] is asked.
#[derive(Clone, Copy)]
pub struct LocationQuery<'a> {
    /// The text.
    pub target: TextTarget<'a>,
    /// The point.
    pub at: TextFrom<'a>,
}

/// The answer to a [`LocationQuery`].
#[derive(Debug)]
pub struct LocationAnswer {
    /// The point, found.
    pub at: FoundPoint,
    /// The left and top of the character there, in screen pixels; `None`
    /// when the provider gives no rectangle.
    pub location: Option<(f64, f64)>,
}

/// The signature both location implementations share.
pub type TextLocationFn = fn(&LocationQuery<'_>) -> Result<LocationAnswer, Error>;

/// Runs `remote` when asked to, falling back to `classic` for this call
/// when it fails, except for a provider that has gone or did not answer in
/// time, which the classic reads would meet too; `classic` alone when not.
fn choose<Q, A>(
    query: &Q,
    remote: bool,
    remote_fn: fn(&Q) -> Result<A, Error>,
    classic_fn: fn(&Q) -> Result<A, Error>,
) -> Result<(A, Path), Error> {
    if !remote {
        return classic_fn(query).map(|answer| (answer, Path::Classic));
    }
    match remote_fn(query) {
        Ok(answer) => Ok((answer, Path::Remote)),
        Err(error) if gone_or_timed_out(&error) => Err(error),
        Err(error) => classic_fn(query).map(|answer| (answer, Path::Fallback(error))),
    }
}

/// Units read, the one function call sites use: the remote program when
/// `remote` is true, falling back to the classic reads for this call when
/// it fails, and the classic reads alone when `remote` is false.
///
/// # Errors
///
/// The classic implementation's [`Error`] when it ran and failed, or the
/// program's when the provider has gone or did not answer in time.
pub fn text_units(query: &UnitsQuery<'_>, remote: bool) -> Result<(UnitsAnswer, Path), Error> {
    choose(query, remote, text_units_remote, text_units_classic)
}

/// The text between two points read, or selected, the one function call
/// sites use, choosing as [`text_units`] does.
///
/// # Errors
///
/// As [`text_units`].
pub fn text_range(query: &RangeQuery<'_>, remote: bool) -> Result<(RangeAnswer, Path), Error> {
    choose(query, remote, text_range_remote, text_range_classic)
}

/// A point's place on the screen, the one function call sites use,
/// choosing as [`text_units`] does.
///
/// # Errors
///
/// As [`text_units`].
pub fn text_location(
    query: &LocationQuery<'_>,
    remote: bool,
) -> Result<(LocationAnswer, Path), Error> {
    choose(query, remote, text_location_remote, text_location_classic)
}

// The remote programs.

/// The program's constants, its element, and the caret and document range
/// once emitted, so a program reads each at most once.
struct Program<'t> {
    target: TextTarget<'t>,
    c: Constants,
    element: Option<Reg<kind::Element>>,
    caret: Option<CaretRegisters>,
}

impl<'t> Program<'t> {
    fn new(b: &mut Builder, target: TextTarget<'t>, max_text: i32) -> Self {
        Self {
            target,
            c: Constants {
                start: b.int(endpoint_number(Endpoint::Start)),
                end: b.int(endpoint_number(Endpoint::End)),
                zero: b.int(0),
                one: b.int(1),
                max_text: b.int(max_text),
            },
            element: None,
            caret: None,
        }
    }

    fn element(&mut self, b: &mut Builder) -> Reg<kind::Element> {
        if let Some(element) = self.element {
            return element;
        }
        let element = b.import_element(self.target.element);
        self.element = Some(element);
        element
    }

    /// The caret and the selection, read the first time they are needed.
    /// Emitted at the program's top level, never inside a branch.
    fn caret(&mut self, b: &mut Builder) -> CaretRegisters {
        if let Some(caret) = self.caret {
            return caret;
        }
        let element = self.element(b);
        let caret = emit_caret(b, &self.c, element, self.target.pattern2.is_some());
        self.caret = Some(caret);
        caret
    }

    /// The whole text's range.
    fn document(&mut self, b: &mut Builder) -> Reg<kind::TextRange> {
        let element = self.element(b);
        let pattern = b.get_text_pattern(element, false);
        b.text_pattern_get_document_range(pattern)
    }

    /// A copy of `range` collapsed to its end.
    fn collapsed_to_end(
        &self,
        b: &mut Builder,
        range: Reg<kind::TextRange>,
    ) -> Reg<kind::TextRange> {
        let copy = b.text_range_clone(range);
        b.text_range_move_endpoint_by_range(copy, self.c.start, copy, self.c.end);
        copy
    }

    /// A collapsed copy of a position the caller holds.
    fn at(&self, b: &mut Builder, position: Position<'_>) -> Reg<kind::TextRange> {
        let range = b.import_text_range(position.range);
        match position.endpoint {
            Endpoint::Start => self.c.collapsed(b, range),
            Endpoint::End => self.collapsed_to_end(b, range),
        }
    }

    /// Emits the point `from` names, as a collapsed range of its own.
    fn point(&mut self, b: &mut Builder, from: TextFrom<'_>) -> Reg<kind::TextRange> {
        match from {
            TextFrom::Caret => {
                let caret = self.caret(b);
                self.c.collapsed(b, caret.caret)
            }
            TextFrom::SelectionStart | TextFrom::SelectionEnd => {
                let caret = self.caret(b);
                let point = b.new_null().assume::<kind::TextRange>();
                let end = matches!(from, TextFrom::SelectionEnd);
                b.if_else(
                    caret.has_selection,
                    |b| {
                        let copy = if end {
                            self.collapsed_to_end(b, caret.selection)
                        } else {
                            self.c.collapsed(b, caret.selection)
                        };
                        b.set(point, copy);
                    },
                    |b| {
                        let copy = self.c.collapsed(b, caret.caret);
                        b.set(point, copy);
                    },
                );
                point
            }
            TextFrom::Start => {
                let document = self.document(b);
                self.c.collapsed(b, document)
            }
            TextFrom::End => {
                let document = self.document(b);
                self.collapsed_to_end(b, document)
            }
            TextFrom::At(position) => self.at(b, position),
            TextFrom::After {
                from,
                prefix,
                counts,
            } => self.after(b, from, prefix, counts),
        }
    }

    /// Emits the advance of [`TextFrom::After`]: each count tried from the
    /// position until the text passed equals `prefix`, compared inside the
    /// provider.
    fn after(
        &self,
        b: &mut Builder,
        from: Position<'_>,
        prefix: &[u16],
        counts: &[i32],
    ) -> Reg<kind::TextRange> {
        let base = self.at(b, from);
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
                b.text_range_move_endpoint_by_unit(range, self.c.end, character, count);
                let passed = b.text_range_get_text(range, limit);
                b.set(last, range);
                let same = b.equal(passed, wanted);
                b.if_(same, |b| {
                    let yes = b.bool(true);
                    b.set(found, yes);
                });
            });
        }
        b.text_range_move_endpoint_by_range(last, self.c.start, last, self.c.end);
        last
    }

    /// Emits the units' language, as the classic reads find it: one
    /// `Culture` read over them all, from `first`'s start to `last`'s end
    /// (over `first` alone without a `last`), and, only when that answers
    /// UIA's "mixed" for more than one unit, a read of each of `ranges`,
    /// appended in order to the returned array, which is otherwise left
    /// empty. Each value is kept when it is a plain integer (a locale id),
    /// else set to null.
    fn languages(
        &self,
        b: &mut Builder,
        first: Reg<kind::TextRange>,
        last: Option<Reg<kind::TextRange>>,
        ranges: Reg<kind::Array>,
    ) -> (Reg<kind::Any>, Reg<kind::Array>) {
        let id = b.int(UIA_CultureAttributeId.0);
        let whole = match last {
            Some(last) => {
                let whole = b.text_range_clone(first);
                b.text_range_move_endpoint_by_range(whole, self.c.end, last, self.c.end);
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
                    Self::plain_locale(b, value);
                    b.array_append(per_unit, value);
                    b.add_assign(index, one);
                },
            );
        });
        Self::plain_locale(b, value);
        (value, per_unit)
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
}

/// A point the program returned, as a [`FoundPoint`].
fn found(outcome: &Outcome, reg: Reg<kind::TextRange>) -> Result<FoundPoint, Error> {
    outcome
        .get(reg)?
        .map(FoundPoint::collapsed)
        .ok_or(Error::MissingResult(reg.id()))
}

/// The language of a locale id the program returned.
fn language(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Int(lcid) => verbatim_uia::text::locale_name(u32::try_from(*lcid).ok()?),
        Value::Uint(lcid) => verbatim_uia::text::locale_name(*lcid),
        _ => None,
    }
}

/// Units read in one cross-process round trip.
///
/// # Errors
///
/// Any [`Error`] from running the program.
#[expect(
    clippy::too_many_lines,
    reason = "one program, read top to bottom as it runs"
)]
pub fn text_units_remote(query: &UnitsQuery<'_>) -> Result<UnitsAnswer, Error> {
    let mut b = Builder::new();
    let mut p = Program::new(&mut b, query.target, query.max_text);
    let from = p.point(&mut b, query.from);
    let from = b.add_to_results(from);
    let moved = b.new_int(0);
    let moved = b.add_to_results(moved);
    let c = &p.c;
    let unit = b.int(query.unit.0);
    let (point, on_start) = match query.movement {
        None => (from, false),
        Some(Movement::Document(count)) => {
            let document = p.document(&mut b);
            let c = &p.c;
            let target = if count < 0 {
                c.collapsed(&mut b, document)
            } else {
                p.collapsed_to_end(&mut b, document)
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
            let count = b.int(count);
            // A collapsed range stays collapsed when it moves.
            let went = b.text_range_move(range, by_unit, count);
            b.set(moved, went);
            (range, by == query.unit)
        }
    };
    let c = &p.c;
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
    if query.count > 1 {
        let read = b.new_int(1);
        let wanted = b.int(i32::try_from(query.count).unwrap_or(i32::MAX));
        let total = b.string_size(text);
        let max_total = b.uint(query.max_total);
        let going = b.new_bool(true);
        b.while_(
            |_| going,
            |b| {
                // As the classic reads: a collapsed copy moved, which stays
                // collapsed, then expanded in place.
                let next = b.text_range_clone(current);
                b.text_range_move_endpoint_by_range(next, c.end, next, c.start);
                let went = b.text_range_move(next, unit, c.one);
                let stuck = b.equal(went, c.zero);
                b.if_(stuck, |b| {
                    let yes = b.bool(true);
                    b.set(ended, yes);
                    let no = b.bool(false);
                    b.set(going, no);
                    b.break_loop();
                });
                let enough = b.compare(read, wanted, Comparison::GreaterThanOrEqual);
                let long = b.compare(total, max_total, Comparison::GreaterThanOrEqual);
                let stop = b.or(enough, long);
                b.if_(stop, |b| {
                    let no = b.bool(false);
                    b.set(going, no);
                    b.break_loop();
                });
                b.text_range_expand_to_enclosing_unit(next, unit);
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
    let (language, languages) = if query.culture {
        let whole = query.count > 1;
        let (language, languages) = p.languages(&mut b, first, whole.then_some(current), ranges);
        (
            Some(b.add_to_results(language)),
            Some(b.add_to_results(languages)),
        )
    } else {
        (None, None)
    };

    let outcome = b.finish().execute()?;
    let ranges = outcome.get(ranges)?;
    let texts = outcome.get(texts)?;
    let language = match language {
        Some(language) => self::language(Some(&outcome.get(language)?)),
        None => None,
    };
    let languages = match languages {
        Some(languages) => outcome.get(languages)?,
        None => Vec::new(),
    };
    let first_offset = usize::try_from(outcome.get(offset)?).unwrap_or(usize::MAX);
    let mut units = Vec::with_capacity(ranges.len());
    for (index, range) in ranges.into_iter().enumerate() {
        let Value::TextRange(range) = range else {
            return Err(Error::MissingResult(offset.id()));
        };
        let text = match texts.get(index) {
            Some(Value::String(text)) => text.encode_utf16().collect(),
            _ => Vec::new(),
        };
        units.push(UnitText {
            range,
            offset: if index == 0 { first_offset } else { 0 },
            text,
            language: if languages.is_empty() {
                language.clone()
            } else {
                self::language(languages.get(index))
            },
        });
    }
    if units.is_empty() {
        return Err(Error::MissingResult(offset.id()));
    }
    Ok(UnitsAnswer {
        from: found(&outcome, from)?,
        point: found(&outcome, point)?,
        moved: outcome.get(moved)?,
        units,
        ended: outcome.get(ended)?,
    })
}

/// The text between two points read or selected in one round trip.
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn text_range_remote(query: &RangeQuery<'_>) -> Result<RangeAnswer, Error> {
    let max_text = match query.action {
        RangeAction::Text(max) => max,
        RangeAction::Select => 0,
    };
    let mut b = Builder::new();
    let mut p = Program::new(&mut b, query.target, max_text);
    let start = p.point(&mut b, query.start);
    let start = b.add_to_results(start);
    let end = match query.end {
        Some(end) => {
            let end = p.point(&mut b, end);
            b.add_to_results(end)
        }
        None => start,
    };
    let c = &p.c;
    let order = b.text_range_compare_endpoints(start, c.start, end, c.start);
    let first = b.new_null().assume::<kind::TextRange>();
    let second = b.new_null().assume::<kind::TextRange>();
    let reversed = b.compare(order, c.zero, Comparison::GreaterThan);
    b.if_else(
        reversed,
        |b| {
            b.set(first, end);
            b.set(second, start);
        },
        |b| {
            b.set(first, start);
            b.set(second, end);
        },
    );
    let range = b.text_range_clone(first);
    b.text_range_move_endpoint_by_range(range, c.end, second, c.start);
    let text = b.new_string("");
    let text = b.add_to_results(text);
    let selected = b.new_bool(false);
    let selected = b.add_to_results(selected);
    match query.action {
        RangeAction::Text(_) => {
            let read = b.text_range_get_text(range, c.max_text);
            b.set(text, read);
        }
        RangeAction::Select => b.try_catch(
            |b| {
                b.text_range_select(range);
                let yes = b.bool(true);
                b.set(selected, yes);
            },
            |_, _| {},
        ),
    }
    let outcome = b.finish().execute()?;
    let start = found(&outcome, start)?;
    let end = if query.end.is_some() {
        found(&outcome, end)?
    } else {
        start.clone()
    };
    Ok(RangeAnswer {
        start,
        end,
        text: string_of(&outcome, text)?.encode_utf16().collect(),
        selected: outcome.get(selected)?,
    })
}

/// A point's place on the screen in one round trip.
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn text_location_remote(query: &LocationQuery<'_>) -> Result<LocationAnswer, Error> {
    let mut b = Builder::new();
    let mut p = Program::new(&mut b, query.target, 0);
    let at = p.point(&mut b, query.at);
    let at = b.add_to_results(at);
    let range = b.text_range_clone(at);
    let character = b.int(TextUnit_Character.0);
    b.text_range_expand_to_enclosing_unit(range, character);
    let rectangles = b.text_range_get_bounding_rectangles(range);
    let rectangles = b.add_to_results(rectangles);
    let outcome = b.finish().execute()?;
    let location = match outcome.get(rectangles)?.as_slice() {
        [Value::Double(left), Value::Double(top), ..] => Some((*left, *top)),
        [Value::Rect { x, y, .. }, ..] => Some((f64::from(*x), f64::from(*y))),
        _ => None,
    };
    Ok(LocationAnswer {
        at: found(&outcome, at)?,
        location,
    })
}

// The classic implementations: the same reads, one call each, counted on
// the thread's `verbatim_uia::calls`.

/// The caret and the selection as the classic reads find them.
struct CaretState {
    caret: FoundPoint,
    selection: Option<IUIAutomationTextRange>,
}

/// Reads the caret and the selection.
fn classic_caret(target: TextTarget<'_>) -> Result<CaretState, Error> {
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
    let caret = match target.pattern2 {
        Some(pattern2) => FoundPoint::collapsed(caret_range(pattern2)?),
        None => FoundPoint {
            range: first.clone(),
            endpoint: Endpoint::Start,
            collapsed: false,
        },
    };
    Ok(CaretState {
        caret,
        selection: Some(first.clone()),
    })
}

/// A copy of `at`'s range whose start is the point. One call, or two.
fn starting_at(at: &FoundPoint) -> Result<IUIAutomationTextRange, Error> {
    let range = at.range.clone_range()?;
    if at.endpoint == Endpoint::End && !at.collapsed {
        range.move_endpoint_to(Endpoint::Start, &range, Endpoint::End)?;
    }
    Ok(range)
}

/// A copy of `at`'s range collapsed to the point. One call, or two.
fn collapsed_copy(at: &FoundPoint) -> Result<IUIAutomationTextRange, Error> {
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
fn classic_point(
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

/// Units read the classic way, the fallback and the reference.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails.
pub fn text_units_classic(query: &UnitsQuery<'_>) -> Result<UnitsAnswer, Error> {
    let target = query.target;
    let from = classic_point(target, query.from, &mut None)?;
    let (point, moved, on_start) = match query.movement {
        None => (from.clone(), 0, false),
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
            (FoundPoint::collapsed(range), moved, by == query.unit)
        }
    };
    let read = |range: IUIAutomationTextRange| -> Result<UnitText, Error> {
        range.expand(query.unit)?;
        let text = range.text(query.max_text)?;
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
        first.offset = before.text(query.max_text)?.len().min(first.text.len());
    }
    let mut total = first.text.len();
    let mut units = vec![first];
    let mut ended = false;
    if query.count > 1 {
        while let Some(current) = units.last() {
            // The next unit's start, by moving a collapsed copy: a whole
            // unit moved stops short of an empty last line, which a
            // collapsed range reaches. It stays collapsed, so it is
            // expanded in place.
            let next = current.range.clone_range()?;
            next.move_endpoint_to(Endpoint::End, &next, Endpoint::Start)?;
            if next.move_by(query.unit, 1)? == 0 {
                ended = true;
                break;
            }
            if units.len() >= query.count as usize || total >= query.max_total as usize {
                break;
            }
            let unit = read(next)?;
            total += unit.text.len();
            units.push(unit);
        }
    }
    if query.culture {
        classic_languages(&mut units, query.count > 1)?;
    }
    Ok(UnitsAnswer {
        from,
        point,
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

/// The text between two points read or selected the classic way.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails.
pub fn text_range_classic(query: &RangeQuery<'_>) -> Result<RangeAnswer, Error> {
    let mut caret = None;
    let start = classic_point(query.target, query.start, &mut caret)?;
    let end = match query.end {
        Some(end) => classic_point(query.target, end, &mut caret)?,
        None => start.clone(),
    };
    let reversed = start
        .range
        .compare_endpoints(start.endpoint, &end.range, end.endpoint)?
        > 0;
    let (first, second) = if reversed {
        (&end, &start)
    } else {
        (&start, &end)
    };
    let range = starting_at(first)?;
    range.move_endpoint_to(Endpoint::End, &second.range, second.endpoint)?;
    let (text, selected) = match query.action {
        RangeAction::Text(max) => (range.text(max)?, false),
        RangeAction::Select => (Vec::new(), range.select().is_ok()),
    };
    Ok(RangeAnswer {
        start,
        end,
        text,
        selected,
    })
}

/// A point's place on the screen the classic way.
///
/// # Errors
///
/// [`Error::Uia`] when a call fails.
pub fn text_location_classic(query: &LocationQuery<'_>) -> Result<LocationAnswer, Error> {
    let at = classic_point(query.target, query.at, &mut None)?;
    let range = starting_at(&at)?;
    range.expand(TextUnit_Character)?;
    let rectangles = range.bounding_rectangles()?;
    let location = match rectangles.as_slice() {
        [left, top, ..] => Some((*left, *top)),
        _ => None,
    };
    Ok(LocationAnswer { at, location })
}
