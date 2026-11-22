//! UIA's text pattern as a [`TextSource`], through `verbatim-uia`'s safe
//! text wrappers.
//!
//! A position is a text range and one of its ends: a collapsed range for a
//! caret, a unit's range with its start or end. Ranges are provider objects
//! in the application, so every move, comparison, and read is a
//! cross-process call, counted where `verbatim-uia` makes it; a range is
//! always copied before it is changed, so a position, once made, never
//! moves.
//!
//! UIA has no sentence unit, so a sentence is answered as unsupported and
//! Core reads by line. A terminal (Windows Terminal's control, the console
//! host) reports its paragraph and page as the whole buffer, as NVDA found,
//! so there the line is the largest unit read. A UIA character is the
//! provider's; positions Core computed inside a chunk are found by moving
//! by characters over the chunk's own text and checking the text passed,
//! so a provider whose characters are code points or grapheme clusters,
//! rather than UTF-16 code units, still lands where Core meant.
//!
//! A focus's caret is read as one segment of the program builder
//! ([`super::fused::caret_line`]): the caret, its line, and the formatting
//! the request names ([`TextDetails`]), converted here to the model's
//! [`TextAttributes`] (`docs/nvda/document-formatting.md`, "UIA
//! providers").
//!
//! Which attributes are read is generic, never by application: those the
//! request names, among those the control supports. Support
//! is learned from the control itself ([`TextSupport`]): an attribute UIA
//! answers "not supported" for, over every stretch of a line read, is one
//! the control does not support, and is not asked again while the control
//! is read; one it answers otherwise is supported, and no longer watched.
//! The annotation types are never learned that way, as a provider may
//! answer "not supported" for text without annotations; the caret read
//! asks a line for them once, and its stretches only when it has some.
//! `IsHidden` is never read.

use std::cmp::Ordering;

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit_Character,
};
use windows::core::AgileReference;

use verbatim_model::{
    BulletStyle, FormatAttribute, LineStyle, TextAttributes, TextDetails, TextUnit,
};
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt, uia_text_unit};

use verbatim_uia_rops::{Attributes, RunAttributes, TextAttribute, TextTarget};

use super::{
    CaretState, Formatting, MAX_CHUNK_UNITS, Sentences, TextError, TextResult, TextSource, Unit,
    UnitRead,
};

/// A position in UIA text: an end of a range.
#[derive(Clone)]
pub struct UiaPos {
    range: AgileReference<IUIAutomationTextRange>,
    endpoint: Endpoint,
    /// The range is known to be collapsed, so either end is the position.
    collapsed: bool,
}

impl UiaPos {
    /// The end `endpoint` of `range`, `collapsed` when the range is known
    /// to be collapsed. Local: the range is kept, not read.
    ///
    /// # Errors
    ///
    /// [`TextError::Failed`] when the range cannot be kept.
    pub fn new(
        range: &IUIAutomationTextRange,
        endpoint: Endpoint,
        collapsed: bool,
    ) -> TextResult<Self> {
        Ok(Self {
            range: AgileReference::new(range).map_err(failed)?,
            endpoint,
            collapsed,
        })
    }

    /// The start of `range`, as a position. Local: the range is kept, not
    /// read.
    ///
    /// # Errors
    ///
    /// [`TextError::Failed`] when the range cannot be kept.
    pub fn start_of(range: &IUIAutomationTextRange) -> TextResult<Self> {
        Self::new(range, Endpoint::Start, false)
    }

    fn range(&self) -> TextResult<IUIAutomationTextRange> {
        self.range.resolve().map_err(failed)
    }

    /// The range and which of its ends the position is, for a remote
    /// operation that compares with it. Local.
    ///
    /// # Errors
    ///
    /// [`TextError::Failed`] when the range cannot be had on this thread.
    pub fn range_end(&self) -> TextResult<(IUIAutomationTextRange, Endpoint)> {
        Ok((self.range()?, self.endpoint))
    }

    /// Whether its range is known to be collapsed, so either end is the
    /// position. Local.
    #[must_use]
    pub fn collapsed(&self) -> bool {
        self.collapsed
    }
}

/// The character counts `prefix` may be to a provider, distinct, in the
/// order they are tried: UTF-16 code units, code points, grapheme
/// clusters.
pub(crate) fn character_counts(prefix: &[u16]) -> Vec<i32> {
    let text = String::from_utf16_lossy(prefix);
    let mut counts = Vec::new();
    for count in [
        prefix.len(),
        text.chars().count(),
        verbatim_text::graphemes(&text).len(),
    ] {
        let count = i32::try_from(count).unwrap_or(i32::MAX);
        if !counts.contains(&count) {
            counts.push(count);
        }
    }
    counts
}

/// A unit a remote operation read, as a [`UnitRead`], its text cut to a
/// chunk's limit.
pub(crate) fn unit_text(read: verbatim_uia_rops::UnitText) -> TextResult<UnitRead<UiaPos>> {
    let mut text = read.text;
    let truncated = text.len() > MAX_CHUNK_UNITS;
    text.truncate(MAX_CHUNK_UNITS);
    let languages = read
        .language
        .map(|language| vec![(0, text.len(), language)])
        .unwrap_or_default();
    Ok(UnitRead {
        offset: read.offset.min(text.len()),
        unit: Unit {
            start: UiaPos::new(&read.range, Endpoint::Start, false)?,
            end: UiaPos::new(&read.range, Endpoint::End, false)?,
            text,
            truncated,
        },
        languages,
    })
}

/// A COM failure as a text error: a gone element or provider as gone.
#[expect(
    clippy::needless_pass_by_value,
    reason = "an adapter for `map_err`, which hands the error over by value"
)]
fn failed(error: windows::core::Error) -> TextError {
    if verbatim_uia::element_is_gone(&error) {
        TextError::Gone
    } else {
        TextError::Failed(error.to_string())
    }
}

/// A node's UIA text.
pub struct UiaText {
    element: IUIAutomationElement,
    pattern: IUIAutomationTextPattern,
    terminal: bool,
    remote: bool,
    window: Option<isize>,
    details: TextDetails,
    support: TextSupport,
    read_classically: bool,
}

/// What a control is known to support of the text attributes the caret
/// read can read, learned from its own answers and kept by the outpost for
/// as long as the control is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextSupport {
    /// The attributes it answered "not supported" for: never asked again.
    pub unsupported: Attributes,
    /// The attributes it answered with something else: asked without
    /// watching for "not supported".
    pub supported: Attributes,
}

impl UiaText {
    /// The text of `element` behind `pattern`; `terminal` for a terminal's
    /// text. The caret is the start of the selection, as NVDA's is.
    /// Caret reads use remote operations and read no detail beyond the
    /// text until [`remote`](Self::remote) and
    /// [`details`](Self::details) say otherwise.
    #[must_use]
    pub fn new(
        element: IUIAutomationElement,
        pattern: IUIAutomationTextPattern,
        terminal: bool,
    ) -> Self {
        Self {
            element,
            pattern,
            terminal,
            remote: true,
            window: None,
            details: TextDetails::NONE,
            support: TextSupport::default(),
            read_classically: false,
        }
    }

    /// Whether caret reads try a remote operation first (false for a window
    /// whose provider cannot run them, or with remote operations off).
    #[must_use]
    pub fn remote(mut self, remote: bool) -> Self {
        self.remote = remote;
        self
    }

    /// What reads fetch beyond the text, as the request names it: the
    /// format attributes, errors and links.
    #[must_use]
    pub fn details(mut self, details: TextDetails) -> Self {
        self.details = details;
        self
    }

    /// What the control is already known to support, from earlier reads.
    #[must_use]
    pub fn support(mut self, support: TextSupport) -> Self {
        self.support = support;
        self
    }

    /// What the control is known to support now, the earlier reads' and
    /// this text's, for the caller to keep for the next read.
    #[must_use]
    pub fn known_support(&self) -> TextSupport {
        self.support
    }

    /// The window the text is in, for a fallback's log.
    #[must_use]
    pub fn window(mut self, window: Option<isize>) -> Self {
        self.window = window;
        self
    }

    /// Whether a read found the window's programs cannot run at all, for
    /// the caller to read it classically from now on.
    pub fn take_read_classically(&mut self) -> bool {
        std::mem::take(&mut self.read_classically)
    }

    /// The element with the text, for reads outside the text protocol (a
    /// terminal's new output, `crate::terminal`).
    #[must_use]
    pub fn element(&self) -> &IUIAutomationElement {
        &self.element
    }

    /// The text pattern, for reads outside the text protocol (a terminal's
    /// new output, `crate::terminal`).
    #[must_use]
    pub fn pattern(&self) -> &IUIAutomationTextPattern {
        &self.pattern
    }

    /// What reads fetch beyond the text, as the request names it.
    #[must_use]
    pub fn details_read(&self) -> TextDetails {
        self.details
    }

    /// The window the text is in, for a fallback's log.
    #[must_use]
    pub fn window_of(&self) -> Option<isize> {
        self.window
    }

    /// Notes that a read found the window's programs cannot run at all.
    pub fn note_read_classically(&mut self, read_classically: bool) {
        self.read_classically |= read_classically;
    }

    /// What a read of units fetching `details` reads of their formatting:
    /// the attributes the details name that the control is not known to
    /// lack, and of those, the ones whose support is still learned, the
    /// annotations never learned that way.
    #[must_use]
    pub fn format_reads(&self, details: TextDetails) -> (Attributes, Attributes) {
        let attributes = attributes_for(details).without(self.support.unsupported);
        let learning = attributes
            .without(self.support.supported)
            .without(Attributes::NONE.with(TextAttribute::Annotations));
        (attributes, learning)
    }

    /// Takes in what a read of `learning`'s attributes learned: those the
    /// provider answered "not supported" for over the unit read
    /// (`unsupported`), the rest being supported; nothing when it learned
    /// nothing.
    pub fn learned(&mut self, learning: Attributes, unsupported: Option<Attributes>) {
        if let Some(unsupported) = unsupported {
            self.support.unsupported = self.support.unsupported.union(unsupported);
            self.support.supported = self.support.supported.union(learning.without(unsupported));
        }
    }

    /// A unit's formatting read for `details`, stretch by stretch, as
    /// UTF-16 ranges of its text: a character's one stretch, which has no
    /// length read, covers the character's `length` units.
    #[must_use]
    pub fn formats_read(
        runs: Vec<verbatim_uia_rops::Run>,
        details: TextDetails,
        character: Option<usize>,
    ) -> Vec<Formatting> {
        let mut formats = formats_with(runs, details);
        if let (Some(length), [(start, end, _)]) = (character, formats.as_mut_slice()) {
            *start = 0;
            *end = length;
        }
        formats
    }

    /// UI Automation's unit for `unit`, `None` for one this text does not
    /// have.
    #[must_use]
    pub fn provider_unit(
        &self,
        unit: TextUnit,
    ) -> Option<windows::Win32::UI::Accessibility::TextUnit> {
        self.unit(unit)
    }

    /// Whether reads may run remote programs: the setting allows them and
    /// the window is not marked for classic reading.
    #[must_use]
    pub fn tries_remote(&self) -> bool {
        self.remote
    }

    /// The text, as remote operations take it.
    #[must_use]
    pub fn target(&self) -> TextTarget<'_> {
        TextTarget {
            element: &self.element,
            pattern: &self.pattern,
        }
    }

    /// A copy of `at`'s range whose start is the position.
    fn starting_at(at: &UiaPos) -> TextResult<IUIAutomationTextRange> {
        let range = at.range()?.clone_range().map_err(failed)?;
        if at.endpoint == Endpoint::End && !at.collapsed {
            range
                .move_endpoint_to(Endpoint::Start, &range, Endpoint::End)
                .map_err(failed)?;
        }
        Ok(range)
    }

    /// A copy of `at`'s range collapsed to the position.
    fn collapsed(at: &UiaPos) -> TextResult<IUIAutomationTextRange> {
        let range = Self::starting_at(at)?;
        if at.endpoint == Endpoint::Start && !at.collapsed {
            range
                .move_endpoint_to(Endpoint::End, &range, Endpoint::Start)
                .map_err(failed)?;
        }
        Ok(range)
    }

    /// UIA's unit for `unit`, `None` for one this text does not have.
    fn unit(&self, unit: TextUnit) -> Option<windows::Win32::UI::Accessibility::TextUnit> {
        if self.terminal && matches!(unit, TextUnit::Paragraph | TextUnit::Page) {
            return None;
        }
        uia_text_unit(unit)
    }
}

impl TextSource for UiaText {
    type Pos = UiaPos;

    fn caret(&mut self) -> TextResult<CaretState<UiaPos>> {
        let selection = self.pattern.selection().map_err(failed)?;
        let Some(range) = selection.first() else {
            // Text with no caret is reviewed from its start.
            return Ok(CaretState {
                caret: self.start()?,
                selection: None,
            });
        };
        let collapsed = range
            .compare_endpoints(Endpoint::Start, range, Endpoint::End)
            .map_err(failed)?
            == 0;
        if collapsed {
            return Ok(CaretState {
                caret: UiaPos::new(range, Endpoint::Start, true)?,
                selection: None,
            });
        }
        // The caret is the selection's start, as NVDA's is
        // (`NVDAObjects/UIA/__init__.py` 488 to 498).
        Ok(CaretState {
            caret: UiaPos::new(range, Endpoint::Start, false)?,
            selection: Some((
                UiaPos::new(range, Endpoint::Start, false)?,
                UiaPos::new(range, Endpoint::End, false)?,
            )),
        })
    }

    fn start(&mut self) -> TextResult<UiaPos> {
        UiaPos::new(
            &self.pattern.document_range().map_err(failed)?,
            Endpoint::Start,
            false,
        )
    }

    fn end(&mut self) -> TextResult<UiaPos> {
        UiaPos::new(
            &self.pattern.document_range().map_err(failed)?,
            Endpoint::End,
            false,
        )
    }

    fn unit_at(
        &mut self,
        at: &UiaPos,
        unit: TextUnit,
        max_units: usize,
    ) -> TextResult<Option<Unit<UiaPos>>> {
        let Some(uia_unit) = self.unit(unit) else {
            return Ok(None);
        };
        let range = Self::starting_at(at)?;
        range.expand(uia_unit).map_err(failed)?;
        let limit = i32::try_from(max_units.saturating_add(1)).unwrap_or(i32::MAX);
        let mut text = range.text(limit).map_err(failed)?;
        let truncated = text.len() > max_units;
        text.truncate(max_units);
        Ok(Some(Unit {
            start: UiaPos::new(&range, Endpoint::Start, false)?,
            end: UiaPos::new(&range, Endpoint::End, false)?,
            text,
            truncated,
        }))
    }

    fn move_by(
        &mut self,
        at: &UiaPos,
        unit: TextUnit,
        count: i32,
    ) -> TextResult<Option<(UiaPos, i32)>> {
        let Some(uia_unit) = self.unit(unit) else {
            return Ok(None);
        };
        let range = Self::starting_at(at)?;
        range.expand(uia_unit).map_err(failed)?;
        // From the unit's start, collapsed, as NVDA moves its review
        // position: moving the whole unit instead stops short of an empty
        // last line, such as the one after a document's final line break,
        // which a collapsed range reaches.
        range
            .move_endpoint_to(Endpoint::End, &range, Endpoint::Start)
            .map_err(failed)?;
        // A collapsed range stays collapsed when it moves.
        let moved = range.move_by(uia_unit, count).map_err(failed)?;
        Ok(Some((UiaPos::new(&range, Endpoint::Start, true)?, moved)))
    }

    fn text(
        &mut self,
        start: &UiaPos,
        end: &UiaPos,
        max_units: usize,
    ) -> TextResult<(Vec<u16>, bool)> {
        let range = Self::starting_at(start)?;
        range
            .move_endpoint_to(Endpoint::End, &end.range()?, end.endpoint)
            .map_err(failed)?;
        let limit = i32::try_from(max_units.saturating_add(1)).unwrap_or(i32::MAX);
        let mut text = range.text(limit).map_err(failed)?;
        let truncated = text.len() > max_units;
        text.truncate(max_units);
        Ok((text, truncated))
    }

    fn offset_in(&mut self, unit: &Unit<UiaPos>, at: &UiaPos) -> TextResult<usize> {
        let range = Self::starting_at(&unit.start)?;
        range
            .move_endpoint_to(Endpoint::End, &at.range()?, at.endpoint)
            .map_err(failed)?;
        let limit = i32::try_from(unit.text.len()).unwrap_or(i32::MAX);
        Ok(range
            .text(limit)
            .map_err(failed)?
            .len()
            .min(unit.text.len()))
    }

    fn advance(&mut self, from: &UiaPos, prefix: &[u16]) -> TextResult<UiaPos> {
        let text = String::from_utf16_lossy(prefix);
        // The provider's character may be a code unit, a code point, or a
        // grapheme cluster: try each count until the text passed matches.
        let counts = [
            prefix.len(),
            text.chars().count(),
            verbatim_text::graphemes(&text).len(),
        ];
        let mut tried = Vec::new();
        let mut last = None;
        for count in counts {
            if tried.contains(&count) {
                continue;
            }
            tried.push(count);
            let range = Self::collapsed(from)?;
            range
                .move_endpoint_by_unit(
                    Endpoint::End,
                    TextUnit_Character,
                    i32::try_from(count).unwrap_or(i32::MAX),
                )
                .map_err(failed)?;
            let passed = range
                .text(i32::try_from(prefix.len().saturating_add(1)).unwrap_or(i32::MAX))
                .map_err(failed)?;
            let matched = passed == prefix;
            last = Some(range);
            if matched {
                break;
            }
        }
        let range = last.ok_or_else(|| TextError::Failed("nothing to advance".to_owned()))?;
        range
            .move_endpoint_to(Endpoint::Start, &range, Endpoint::End)
            .map_err(failed)?;
        UiaPos::new(&range, Endpoint::Start, true)
    }

    fn compare(&mut self, a: &UiaPos, b: &UiaPos) -> TextResult<Ordering> {
        let order = a
            .range()?
            .compare_endpoints(a.endpoint, &b.range()?, b.endpoint)
            .map_err(failed)?;
        Ok(order.cmp(&0))
    }

    fn select(&mut self, start: &UiaPos, end: &UiaPos) -> TextResult<bool> {
        let range = Self::starting_at(start)?;
        range
            .move_endpoint_to(Endpoint::End, &end.range()?, end.endpoint)
            .map_err(failed)?;
        Ok(range.select().is_ok())
    }

    fn location(&mut self, at: &UiaPos) -> TextResult<Option<(i32, i32)>> {
        let range = Self::starting_at(at)?;
        range.expand(TextUnit_Character).map_err(failed)?;
        let rectangles = range.bounding_rectangles().map_err(failed)?;
        Ok(match rectangles.as_slice() {
            [left, top, ..] => Some((round(*left), round(*top))),
            _ => None,
        })
    }

    fn languages(&mut self, unit: &Unit<UiaPos>) -> Vec<(usize, usize, String)> {
        let culture = unit
            .start
            .range()
            .ok()
            .and_then(|range| range.culture().ok().flatten());
        culture
            .map(|language| vec![(0, unit.text.len(), language)])
            .unwrap_or_default()
    }

    fn edges(&mut self, _unit: &Unit<UiaPos>, _kind: TextUnit) -> (bool, bool) {
        // Not known without further calls; Core moves to find out.
        (false, false)
    }

    fn sentences(&self) -> Sentences {
        Sentences::Unsupported
    }
}

/// A span's formatting as its stretches were read, as UTF-16 ranges of
/// its text, each stretch's attributes in the words `details` asks for.
fn formats_with(runs: Vec<verbatim_uia_rops::Run>, details: TextDetails) -> Vec<Formatting> {
    let mut formats = Vec::with_capacity(runs.len());
    let mut at = 0;
    for run in runs {
        formats.push((at, at + run.length, attributes_of(&run.attributes, details)));
        at += run.length;
    }
    formats
}

/// The attributes to read for the details a request names.
fn attributes_for(details: TextDetails) -> Attributes {
    let format = |attribute| details.formats.contains(attribute);
    let mut attributes = Attributes::NONE;
    for (wanted, read) in [
        (
            details.spelling_errors || details.grammar_errors,
            &[TextAttribute::Annotations][..],
        ),
        (
            format(FormatAttribute::FontName),
            &[TextAttribute::FontName],
        ),
        (
            format(FormatAttribute::FontSize),
            &[TextAttribute::FontSize],
        ),
        (
            format(FormatAttribute::FontAttributes),
            &[
                TextAttribute::FontWeight,
                TextAttribute::Italic,
                TextAttribute::UnderlineStyle,
            ],
        ),
        (
            format(FormatAttribute::UnderlineStyle),
            &[TextAttribute::UnderlineStyle],
        ),
        (
            format(FormatAttribute::Strikethrough),
            &[TextAttribute::StrikethroughStyle],
        ),
        (
            format(FormatAttribute::Color),
            &[TextAttribute::ForegroundColor],
        ),
        (
            format(FormatAttribute::BackgroundColor),
            &[TextAttribute::BackgroundColor],
        ),
        (
            format(FormatAttribute::BulletStyle),
            &[TextAttribute::BulletStyle],
        ),
        (details.links, &[TextAttribute::Link]),
    ] {
        if wanted {
            attributes = attributes.union(Attributes::of(read));
        }
    }
    attributes
}

/// A stretch's attributes in the model's words, as NVDA words UIA's: bold
/// from a weight of 700 or more, underlined from any underline style but
/// none, the size in points ("11.0 pt"), the colors by their names, and the
/// line and bullet styles by UIA's numbers (`docs/nvda/document-formatting.md`,
/// "UIA providers"). The underline style is read for the font attributes
/// and for the kind of underline alike, and given to each that `details`
/// asks for; the errors are given as `details` asks for each.
fn attributes_of(run: &RunAttributes, details: TextDetails) -> TextAttributes {
    let color_name = |color: i32| super::color::color_name(color.cast_unsigned());
    TextAttributes {
        spelling_error: run.spelling_error && details.spelling_errors,
        grammar_error: run.grammar_error && details.grammar_errors,
        font_name: run.font_name.clone(),
        font_size: run.font_size.map(|size| format!("{size:?} pt")),
        color: run.color.map(color_name),
        bold: run.font_weight.map(|weight| weight >= 700),
        italic: run.italic,
        underline: run
            .underline
            .filter(|_| details.formats.contains(FormatAttribute::FontAttributes))
            .map(|style| style != 0),
        underline_style: run
            .underline
            .filter(|_| details.formats.contains(FormatAttribute::UnderlineStyle))
            .map(line_style),
        strikethrough: run.strikethrough.map(line_style),
        background_color: run.background_color.map(color_name),
        bullet: run.bullet_style.map(bullet_style),
        link: run.link == Some(true),
    }
}

/// UIA's text decoration line style (`TextDecorationLineStyle`) as the
/// model's.
fn line_style(style: i32) -> LineStyle {
    match style {
        0 => LineStyle::None,
        1 => LineStyle::Single,
        2 => LineStyle::WordsOnly,
        3 => LineStyle::Double,
        4 => LineStyle::Dotted,
        5 => LineStyle::Dashed,
        6 => LineStyle::DotDash,
        7 => LineStyle::DotDotDash,
        8 => LineStyle::Wavy,
        9 => LineStyle::Thick,
        11 => LineStyle::DoubleWavy,
        12 => LineStyle::ThickWavy,
        13 => LineStyle::LongDash,
        14 => LineStyle::ThickDashed,
        15 => LineStyle::ThickDotDash,
        16 => LineStyle::ThickDotDotDash,
        17 => LineStyle::ThickDotted,
        18 => LineStyle::ThickLongDash,
        _ => LineStyle::Other,
    }
}

/// UIA's bullet style (`BulletStyle`) as the model's.
fn bullet_style(style: i32) -> BulletStyle {
    match style {
        0 => BulletStyle::None,
        1 => BulletStyle::HollowRound,
        2 => BulletStyle::FilledRound,
        3 => BulletStyle::HollowSquare,
        4 => BulletStyle::FilledSquare,
        5 => BulletStyle::Dash,
        _ => BulletStyle::Other,
    }
}

/// A screen coordinate in whole pixels.
pub(crate) fn round(value: f64) -> i32 {
    // Screen coordinates fit an i32; a value that does not saturates.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a rounded screen coordinate, saturating if out of range"
    )]
    let pixels = value.round() as i32;
    pixels
}

#[cfg(test)]
mod tests {
    use verbatim_model::{FormatAttribute, FormatSet, TextDetails};
    use verbatim_uia_rops::{Attributes, TextAttribute};

    use super::attributes_for;

    /// Each detail a request leaves out leaves its UI Automation attributes
    /// unread, but those another detail still asked for reads: the
    /// underline style, which the font attributes read too, and the
    /// annotations, which hold spelling and grammar errors alike.
    #[test]
    fn a_detail_left_out_leaves_its_attributes_unread() {
        let without_format = |attribute| {
            attributes_for(TextDetails {
                formats: FormatSet::ALL.without(attribute),
                ..TextDetails::ALL
            })
        };
        let less = |gone: &[TextAttribute]| Attributes::ALL.without(Attributes::of(gone));
        assert_eq!(attributes_for(TextDetails::ALL), Attributes::ALL);
        assert_eq!(attributes_for(TextDetails::NONE), Attributes::NONE);
        assert_eq!(
            without_format(FormatAttribute::FontName),
            less(&[TextAttribute::FontName])
        );
        assert_eq!(
            without_format(FormatAttribute::FontSize),
            less(&[TextAttribute::FontSize])
        );
        assert_eq!(
            without_format(FormatAttribute::Color),
            less(&[TextAttribute::ForegroundColor])
        );
        assert_eq!(
            without_format(FormatAttribute::BackgroundColor),
            less(&[TextAttribute::BackgroundColor])
        );
        assert_eq!(
            without_format(FormatAttribute::FontAttributes),
            less(&[TextAttribute::FontWeight, TextAttribute::Italic])
        );
        assert_eq!(
            without_format(FormatAttribute::Strikethrough),
            less(&[TextAttribute::StrikethroughStyle])
        );
        assert_eq!(
            without_format(FormatAttribute::UnderlineStyle),
            Attributes::ALL
        );
        assert_eq!(
            without_format(FormatAttribute::BulletStyle),
            less(&[TextAttribute::BulletStyle])
        );
        assert_eq!(
            attributes_for(TextDetails {
                links: false,
                ..TextDetails::ALL
            }),
            less(&[TextAttribute::Link])
        );
        assert_eq!(
            attributes_for(TextDetails {
                spelling_errors: false,
                ..TextDetails::ALL
            }),
            Attributes::ALL
        );
        assert_eq!(
            attributes_for(TextDetails {
                spelling_errors: false,
                grammar_errors: false,
                ..TextDetails::ALL
            }),
            less(&[TextAttribute::Annotations])
        );
    }
}
