//! Safe wrappers over UIA's text pattern (milestone M4): a pattern's
//! selection, caret, and document range, and a text range's moves, reads,
//! and comparisons, each method holding one documented `unsafe` call and
//! counting the cross-process call it makes, as [`crate::ElementExt`] does
//! for elements.
//!
//! A UIA text range is a provider object in the application's process; the
//! interface in hand is a proxy, so every method here, cloning included,
//! is a cross-process call. Positions in the outpost are ranges, mostly
//! degenerate ones, compared and moved through these.

use windows::Win32::Globalization::LCIDToLocaleName;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    IUIAutomationTextRange, IUIAutomationTextRange3, IUIAutomationTextRangeArray,
    TextPatternRangeEndpoint, TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start,
    TextUnit, TextUnit_Character, TextUnit_Document, TextUnit_Line, TextUnit_Page,
    TextUnit_Paragraph, TextUnit_Word, UIA_CultureAttributeId, UIA_TEXTATTRIBUTE_ID,
    UIA_TextPattern2Id, UIA_TextPatternId,
};
use windows::core::Interface;

use verbatim_model::CallKind;

use crate::ElementExt;
use crate::calls::count;
use crate::com::{take_f64_safearray, take_variant_safearray, variant_i32};

pub use windows::Win32::System::Variant::VARIANT;

/// A text range's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// Its start.
    Start,
    /// Its end.
    End,
}

impl Endpoint {
    fn uia(self) -> TextPatternRangeEndpoint {
        match self {
            Endpoint::Start => TextPatternRangeEndpoint_Start,
            Endpoint::End => TextPatternRangeEndpoint_End,
        }
    }
}

/// UIA's unit for a model [`TextUnit`](verbatim_model::TextUnit), `None` for
/// the sentence, which UIA does not have.
#[must_use]
pub fn uia_text_unit(unit: verbatim_model::TextUnit) -> Option<TextUnit> {
    Some(match unit {
        verbatim_model::TextUnit::Character => TextUnit_Character,
        verbatim_model::TextUnit::Word => TextUnit_Word,
        verbatim_model::TextUnit::Line => TextUnit_Line,
        verbatim_model::TextUnit::Paragraph => TextUnit_Paragraph,
        verbatim_model::TextUnit::Page => TextUnit_Page,
        verbatim_model::TextUnit::Document => TextUnit_Document,
        _ => return None,
    })
}

/// An element's text pattern: `TextPattern2` where the provider has it, for
/// the caret, else `TextPattern`. Cross-process: one call when the
/// provider has `TextPattern2`, two when it has only `TextPattern`.
///
/// # Errors
///
/// The COM error of the `TextPattern` fetch when the element has neither,
/// or is gone.
pub fn text_pattern(
    element: &IUIAutomationElement,
) -> windows::core::Result<(IUIAutomationTextPattern, Option<IUIAutomationTextPattern2>)> {
    if let Ok(pattern2) = element.current_pattern::<IUIAutomationTextPattern2>(UIA_TextPattern2Id) {
        // A `TextPattern2` is a `TextPattern`; the cast stays in this process.
        let pattern = pattern2.cast::<IUIAutomationTextPattern>()?;
        return Ok((pattern, Some(pattern2)));
    }
    let pattern = element.current_pattern::<IUIAutomationTextPattern>(UIA_TextPatternId)?;
    Ok((pattern, None))
}

/// The ranges of a range array, in order. Local.
fn ranges_of(array: &IUIAutomationTextRangeArray) -> Vec<IUIAutomationTextRange> {
    // SAFETY: `array` is a live range array; reading it stays in this
    // process.
    let length = unsafe { array.Length() }.unwrap_or(0);
    (0..length)
        // SAFETY: `index` is within the array's length.
        .filter_map(|index| unsafe { array.GetElement(index) }.ok())
        .collect()
}

/// Safe calls on a text pattern.
pub trait TextPatternExt {
    /// The selected ranges; a collapsed range at the caret when nothing is
    /// selected, and none when the text has no caret. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn selection(&self) -> windows::core::Result<Vec<IUIAutomationTextRange>>;

    /// The range of the whole text. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn document_range(&self) -> windows::core::Result<IUIAutomationTextRange>;
}

impl TextPatternExt for IUIAutomationTextPattern {
    fn selection(&self) -> windows::core::Result<Vec<IUIAutomationTextRange>> {
        count(CallKind::Uia);
        // SAFETY: `self` is a live pattern object; a gone provider fails the
        // call.
        let array = unsafe { self.GetSelection() }?;
        Ok(ranges_of(&array))
    }

    fn document_range(&self) -> windows::core::Result<IUIAutomationTextRange> {
        count(CallKind::Uia);
        // SAFETY: as in `selection`.
        unsafe { self.DocumentRange() }
    }
}

/// The caret, a collapsed range, from `TextPattern2`. Cross-process.
///
/// # Errors
///
/// The COM error if the provider fails.
pub fn caret_range(
    pattern: &IUIAutomationTextPattern2,
) -> windows::core::Result<IUIAutomationTextRange> {
    count(CallKind::Uia);
    let mut active = windows::core::BOOL::default();
    // SAFETY: `pattern` is a live pattern object and `active` a local the
    // call writes whether the text has the keyboard focus.
    unsafe { pattern.GetCaretRange(&raw mut active) }
}

/// Safe calls on a text range, each one cross-process call.
pub trait TextRangeExt {
    /// A copy of the range.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn clone_range(&self) -> windows::core::Result<IUIAutomationTextRange>;

    /// How this range's `endpoint` compares with `other`'s `other_endpoint`:
    /// negative before, zero equal, positive after.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn compare_endpoints(
        &self,
        endpoint: Endpoint,
        other: &IUIAutomationTextRange,
        other_endpoint: Endpoint,
    ) -> windows::core::Result<i32>;

    /// Grows the range to the `unit` that contains its start.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn expand(&self, unit: TextUnit) -> windows::core::Result<()>;

    /// Moves the range by `count` units, collapsing it to a unit's start;
    /// returns how many it moved.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn move_by(&self, unit: TextUnit, count: i32) -> windows::core::Result<i32>;

    /// Moves `endpoint` to `other`'s `other_endpoint`.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn move_endpoint_to(
        &self,
        endpoint: Endpoint,
        other: &IUIAutomationTextRange,
        other_endpoint: Endpoint,
    ) -> windows::core::Result<()>;

    /// Moves `endpoint` by `count` units; returns how many it moved.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn move_endpoint_by_unit(
        &self,
        endpoint: Endpoint,
        unit: TextUnit,
        count: i32,
    ) -> windows::core::Result<i32>;

    /// The range's text, UTF-16, at most `max` code units (all of it for
    /// `-1`).
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn text(&self, max: i32) -> windows::core::Result<Vec<u16>>;

    /// Selects the range, moving the caret.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn select(&self) -> windows::core::Result<()>;

    /// The range's bounding rectangles, as left, top, width, and height for
    /// each, in screen pixels.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn bounding_rectangles(&self) -> windows::core::Result<Vec<f64>>;

    /// The range's language, as a BCP 47 tag, from its `Culture` attribute:
    /// `None` when the range mixes languages or the provider does not say.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn culture(&self) -> windows::core::Result<Option<String>>;

    /// A text attribute's value over the range: UIA's "not supported" or
    /// "mixed" sentinel object when the range has none or several, which
    /// the `variant_*` readers in this crate read as `None`.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails.
    fn attribute(&self, attribute: UIA_TEXTATTRIBUTE_ID) -> windows::core::Result<VARIANT>;

    /// Several text attributes' values over the range, in the order asked,
    /// each as [`attribute`](Self::attribute) gives it: in one call,
    /// `IUIAutomationTextRange3::GetAttributeValues`, as NVDA fetches a
    /// range's formatting. Where the range has no `IUIAutomationTextRange3`,
    /// or that call fails or answers with the wrong number of values, each
    /// attribute is read on its own, one call each, and an attribute whose
    /// read fails is "not supported" (an empty `VARIANT`, which the
    /// `variant_*` readers read as `None`), as NVDA treats a failed
    /// attribute read.
    ///
    /// # Errors
    ///
    /// The COM error when the range's provider is gone
    /// ([`element_is_gone`](crate::element_is_gone)).
    fn attributes(
        &self,
        attributes: &[UIA_TEXTATTRIBUTE_ID],
    ) -> windows::core::Result<Vec<VARIANT>>;
}

impl TextRangeExt for IUIAutomationTextRange {
    fn clone_range(&self) -> windows::core::Result<IUIAutomationTextRange> {
        count(CallKind::Uia);
        // SAFETY: `self` is a live range; a gone provider fails the call.
        unsafe { self.Clone() }
    }

    fn compare_endpoints(
        &self,
        endpoint: Endpoint,
        other: &IUIAutomationTextRange,
        other_endpoint: Endpoint,
    ) -> windows::core::Result<i32> {
        count(CallKind::Uia);
        // SAFETY: both ranges are live; the endpoints are plain values.
        unsafe { self.CompareEndpoints(endpoint.uia(), other, other_endpoint.uia()) }
    }

    fn expand(&self, unit: TextUnit) -> windows::core::Result<()> {
        count(CallKind::Uia);
        // SAFETY: as in `clone_range`; the unit is a plain value.
        unsafe { self.ExpandToEnclosingUnit(unit) }
    }

    fn move_by(&self, unit: TextUnit, count_by: i32) -> windows::core::Result<i32> {
        count(CallKind::Uia);
        // SAFETY: as in `expand`.
        unsafe { self.Move(unit, count_by) }
    }

    fn move_endpoint_to(
        &self,
        endpoint: Endpoint,
        other: &IUIAutomationTextRange,
        other_endpoint: Endpoint,
    ) -> windows::core::Result<()> {
        count(CallKind::Uia);
        // SAFETY: as in `compare_endpoints`.
        unsafe { self.MoveEndpointByRange(endpoint.uia(), other, other_endpoint.uia()) }
    }

    fn move_endpoint_by_unit(
        &self,
        endpoint: Endpoint,
        unit: TextUnit,
        count_by: i32,
    ) -> windows::core::Result<i32> {
        count(CallKind::Uia);
        // SAFETY: as in `expand`.
        unsafe { self.MoveEndpointByUnit(endpoint.uia(), unit, count_by) }
    }

    fn text(&self, max: i32) -> windows::core::Result<Vec<u16>> {
        count(CallKind::Uia);
        // SAFETY: as in `clone_range`; the returned string is owned here.
        let text = unsafe { self.GetText(max) }?;
        Ok(text.to_vec())
    }

    fn select(&self) -> windows::core::Result<()> {
        count(CallKind::Uia);
        // SAFETY: as in `clone_range`.
        unsafe { self.Select() }
    }

    fn bounding_rectangles(&self) -> windows::core::Result<Vec<f64>> {
        count(CallKind::Uia);
        // SAFETY: as in `clone_range`; the returned array of doubles is owned
        // here and taken, destroyed once, by the helper.
        let array = unsafe { self.GetBoundingRectangles() }?;
        // SAFETY: `array` is the caller-owned SAFEARRAY of doubles the call
        // returned.
        Ok(unsafe { take_f64_safearray(array) })
    }

    fn attribute(&self, attribute: UIA_TEXTATTRIBUTE_ID) -> windows::core::Result<VARIANT> {
        count(CallKind::Uia);
        // SAFETY: as in `clone_range`; the attribute id is a plain value.
        unsafe { self.GetAttributeValue(attribute) }
    }

    fn attributes(
        &self,
        attributes: &[UIA_TEXTATTRIBUTE_ID],
    ) -> windows::core::Result<Vec<VARIANT>> {
        // A local query of the client's range object.
        if let Ok(range) = self.cast::<IUIAutomationTextRange3>() {
            count(CallKind::Uia);
            // SAFETY: `range` is a live range; the ids are plain values, and
            // the returned SAFEARRAY is owned here.
            match unsafe { range.GetAttributeValues(attributes) } {
                Ok(array) => {
                    // SAFETY: the array was just returned to this caller,
                    // which hands its ownership to the helper.
                    let values = unsafe { take_variant_safearray(array) };
                    if values.len() == attributes.len() {
                        return Ok(values);
                    }
                }
                Err(error) if crate::element_is_gone(&error) => return Err(error),
                Err(error) => {
                    tracing::debug!(%error, "GetAttributeValues failed; reading attributes one by one");
                }
            }
        }
        attributes
            .iter()
            .map(|&attribute| match self.attribute(attribute) {
                Err(error) if crate::element_is_gone(&error) => Err(error),
                read => Ok(read.unwrap_or_default()),
            })
            .collect()
    }

    fn culture(&self) -> windows::core::Result<Option<String>> {
        count(CallKind::Uia);
        // SAFETY: as in `clone_range`; the attribute id is a plain value.
        let value = unsafe { self.GetAttributeValue(UIA_CultureAttributeId) }?;
        // A mixed or unsupported value is an object, not an integer.
        let Some(lcid) = variant_i32(&value) else {
            return Ok(None);
        };
        Ok(locale_name(u32::try_from(lcid).unwrap_or(0)))
    }
}

/// The BCP 47 name of a Windows locale id, `None` for none.
#[must_use]
pub fn locale_name(lcid: u32) -> Option<String> {
    if lcid == 0 {
        return None;
    }
    let mut buffer = [0u16; 85];
    // SAFETY: a local call writing at most the buffer's length.
    let length = unsafe { LCIDToLocaleName(lcid, Some(&mut buffer), 0) };
    let length = usize::try_from(length).ok()?.checked_sub(1)?;
    (length > 0).then(|| String::from_utf16_lossy(&buffer[..length]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locale_id_names_its_language() {
        assert_eq!(locale_name(0x0409).as_deref(), Some("en-US"));
        assert_eq!(locale_name(0x0804).as_deref(), Some("zh-CN"));
        assert_eq!(locale_name(0), None);
    }

    #[test]
    fn uia_has_every_unit_but_the_sentence() {
        assert_eq!(
            uia_text_unit(verbatim_model::TextUnit::Line),
            Some(TextUnit_Line)
        );
        assert_eq!(uia_text_unit(verbatim_model::TextUnit::Sentence), None);
    }
}
