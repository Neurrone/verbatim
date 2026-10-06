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

use std::cmp::Ordering;

use windows::Win32::UI::Accessibility::{
    IUIAutomationTextPattern, IUIAutomationTextPattern2, IUIAutomationTextRange, TextUnit_Character,
};
use windows::core::AgileReference;

use verbatim_model::TextUnit;
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt, caret_range, uia_text_unit};

use super::{CaretState, Sentences, TextError, TextResult, TextSource, Unit};

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
    pattern: IUIAutomationTextPattern,
    pattern2: Option<IUIAutomationTextPattern2>,
    terminal: bool,
}

impl UiaText {
    /// The text behind `pattern`, with `pattern2` for the caret where the
    /// provider has it; `terminal` for a terminal's text.
    #[must_use]
    pub fn new(
        pattern: IUIAutomationTextPattern,
        pattern2: Option<IUIAutomationTextPattern2>,
        terminal: bool,
    ) -> Self {
        Self {
            pattern,
            pattern2,
            terminal,
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
