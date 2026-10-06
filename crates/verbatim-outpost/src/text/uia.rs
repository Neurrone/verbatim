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
//! The caret is read for a caret report or a caret key's answer through
//! `verbatim-uia-rops`'s `caret_read`, in one remote operation where the
//! provider runs them, with its classic reads behind it: the caret, the
//! evidence, the line and the unit at the caret, and the formatting the
//! theme asks for ([`Fetches`]), converted here to the model's
//! [`TextAttributes`] (`docs/nvda/document-formatting.md`, "UIA
//! providers").

use std::cmp::Ordering;

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    IUIAutomationTextRange, TextUnit_Character,
};
use windows::core::AgileReference;

use verbatim_model::{Fetches, TextAttributes, TextUnit};
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt, caret_range, uia_text_unit};
use verbatim_uia_rops::{Attributes, CaretQuery, Path, RangeEnd, RunAttributes};

use super::{
    CaretRead, CaretRequest, CaretState, FormatSpan, MAX_CHUNK_UNITS, Sentences, TextError,
    TextResult, TextSource, Unit,
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
    fn new(
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

    fn range(&self) -> TextResult<IUIAutomationTextRange> {
        self.range.resolve().map_err(failed)
    }
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
    pattern2: Option<IUIAutomationTextPattern2>,
    terminal: bool,
    remote: bool,
    attributes: Attributes,
    fallback: Option<verbatim_uia_rops::Error>,
}

impl UiaText {
    /// The text of `element` behind `pattern`, with `pattern2` for the
    /// caret where the provider has it; `terminal` for a terminal's text.
    /// Caret reads use remote operations and read every formatting
    /// attribute until [`remote`](Self::remote) and
    /// [`fetches`](Self::fetches) say otherwise.
    #[must_use]
    pub fn new(
        element: IUIAutomationElement,
        pattern: IUIAutomationTextPattern,
        pattern2: Option<IUIAutomationTextPattern2>,
        terminal: bool,
    ) -> Self {
        Self {
            element,
            pattern,
            pattern2,
            terminal,
            remote: true,
            attributes: attributes_for(Fetches::default()),
            fallback: None,
        }
    }

    /// Whether caret reads try a remote operation first (false for a window
    /// whose provider cannot run them, or with remote operations off).
    #[must_use]
    pub fn remote(mut self, remote: bool) -> Self {
        self.remote = remote;
        self
    }

    /// The details the theme wants read: the formatting attributes whose
    /// indications are on.
    #[must_use]
    pub fn fetches(mut self, fetches: Fetches) -> Self {
        self.attributes = attributes_for(fetches);
        self
    }

    /// The remote operation's error, when a caret read fell back to the
    /// classic reads, for the caller to log and remember.
    pub fn take_fallback(&mut self) -> Option<verbatim_uia_rops::Error> {
        self.fallback.take()
    }

    /// The text pattern, for reads outside the text protocol (a terminal's
    /// new output, `crate::terminal`).
    #[must_use]
    pub fn pattern(&self) -> &IUIAutomationTextPattern {
        &self.pattern
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
        let caret = match &self.pattern2 {
            Some(pattern2) => UiaPos::new(
                &caret_range(pattern2).map_err(failed)?,
                Endpoint::Start,
                true,
            )?,
            None => UiaPos::new(range, Endpoint::Start, false)?,
        };
        Ok(CaretState {
            caret,
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
        let range = Self::collapsed(at)?;
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
        let range = Self::collapsed(at)?;
        range.expand(uia_unit).map_err(failed)?;
        let moved = range.move_by(uia_unit, count).map_err(failed)?;
        range
            .move_endpoint_to(Endpoint::End, &range, Endpoint::Start)
            .map_err(failed)?;
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
        let range = Self::collapsed(at)?;
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

    fn caret_read(
        &mut self,
        request: &CaretRequest<'_, UiaPos>,
    ) -> TextResult<Option<CaretRead<UiaPos>>> {
        let since = match request.since {
            Some(pos) => Some((pos.range()?, pos.endpoint)),
            None => None,
        };
        let previous = match request.previous {
            Some((start, end)) => Some((
                (start.range()?, start.endpoint),
                (end.range()?, end.endpoint),
            )),
            None => None,
        };
        // A unit this text does not have is not read.
        let unit = request.unit.and_then(|unit| self.unit(unit));
        let query = CaretQuery {
            element: &self.element,
            pattern: &self.pattern,
            pattern2: self.pattern2.as_ref(),
            since: since.as_ref().map(end_of),
            previous_selection: previous
                .as_ref()
                .map(|(start, end)| (end_of(start), end_of(end))),
            unit,
            formats: request.formats.map(|span| match span {
                FormatSpan::Character => verbatim_uia_rops::FormatSpan::Character,
                FormatSpan::Unit => verbatim_uia_rops::FormatSpan::Unit,
                FormatSpan::Line => verbatim_uia_rops::FormatSpan::Line,
            }),
            attributes: self.attributes,
            max_text: i32::try_from(MAX_CHUNK_UNITS + 1).unwrap_or(i32::MAX),
        };
        let (answer, path) =
            verbatim_uia_rops::caret_read(&query, self.remote).map_err(rops_failed)?;
        if let Path::Fallback(error) = path {
            self.fallback = Some(error);
        }
        let caret = UiaPos::new(&answer.caret, Endpoint::Start, answer.collapsed)?;
        let selection = match &answer.selection {
            Some(range) => Some((
                UiaPos::new(range, Endpoint::Start, false)?,
                UiaPos::new(range, Endpoint::End, false)?,
            )),
            None => None,
        };
        let line = unit_read(answer.line)?;
        let unit = answer.unit.map(unit_read).transpose()?;
        // A character's one stretch has no length read; it covers the
        // character, as the caller takes it.
        let mut formats = Vec::with_capacity(answer.runs.len());
        let mut at = 0;
        for run in answer.runs {
            formats.push((at, at + run.length, attributes_of(&run.attributes)));
            at += run.length;
        }
        Ok(Some(CaretRead {
            state: CaretState { caret, selection },
            moved: answer.moved,
            selection_moved: answer.selection_moved,
            line,
            unit,
            formats,
        }))
    }
}

/// A position as remote operations take it.
fn end_of((range, endpoint): &(IUIAutomationTextRange, Endpoint)) -> RangeEnd<'_> {
    RangeEnd {
        range,
        endpoint: *endpoint,
    }
}

/// A unit the caret read returned, as a [`Unit`] and the caret's offset in
/// it, its text cut to a chunk's limit.
fn unit_read(read: verbatim_uia_rops::UnitRead) -> TextResult<(Unit<UiaPos>, usize)> {
    let mut text = read.text;
    let truncated = text.len() > MAX_CHUNK_UNITS;
    text.truncate(MAX_CHUNK_UNITS);
    let offset = read.offset.min(text.len());
    Ok((
        Unit {
            start: UiaPos::new(&read.range, Endpoint::Start, false)?,
            end: UiaPos::new(&read.range, Endpoint::End, false)?,
            text,
            truncated,
        },
        offset,
    ))
}

/// The attributes to read for the details the theme wants.
fn attributes_for(fetches: Fetches) -> Attributes {
    Attributes {
        annotations: fetches.spelling_errors || fetches.grammar_errors,
        font: fetches.font,
        font_attributes: fetches.font_attributes,
        color: fetches.color,
    }
}

/// A stretch's attributes in the model's words, as NVDA words UIA's: bold
/// from a weight of 700 or more, underlined from any underline style but
/// none, the size in points ("11.0 pt"), and the color by its name.
fn attributes_of(run: &RunAttributes) -> TextAttributes {
    TextAttributes {
        spelling_error: run.spelling_error,
        grammar_error: run.grammar_error,
        font_name: run.font_name.clone(),
        font_size: run.font_size.map(|size| format!("{size:?} pt")),
        color: run
            .color
            .map(|color| super::color::color_name(color.cast_unsigned())),
        bold: run.font_weight.map(|weight| weight >= 700),
        italic: run.italic,
        underline: run.underline.map(|style| style != 0),
    }
}

/// A remote operations or UIA failure as a text error: a gone element or
/// provider as gone.
#[expect(
    clippy::needless_pass_by_value,
    reason = "an adapter for `map_err`, which hands the error over by value"
)]
fn rops_failed(error: verbatim_uia_rops::Error) -> TextError {
    match error.hresult() {
        Some(code) if verbatim_uia::element_is_gone(&windows::core::Error::from(code)) => {
            TextError::Gone
        }
        _ => TextError::Failed(error.to_string()),
    }
}

/// A screen coordinate in whole pixels.
fn round(value: f64) -> i32 {
    // Screen coordinates fit an i32; a value that does not saturates.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a rounded screen coordinate, saturating if out of range"
    )]
    let pixels = value.round() as i32;
    pixels
}
