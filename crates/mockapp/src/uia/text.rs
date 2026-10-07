//! UIA's text pattern over a fixture node's `text` (milestone M4):
//! `ITextProvider2` and `ITextRangeProvider`, so a client reads, moves
//! through, and selects scripted text cross-process as it does a real
//! application's.
//!
//! A range is two UTF-16 offsets into the node's text, read afresh on every
//! call. Units are simple and fixed: a character is one code unit; a word
//! is a run of letters and digits with the spaces after it, or one other
//! character; a line ends after its line feed, and a text ending in a line
//! feed has an empty last line, as an editor shows one; a paragraph is a
//! line; a stretch of the format unit ends wherever a spelling error or a
//! bold stretch of the fixture starts or ends; the page and the document
//! are the whole text.
//! Moving by a unit lands on a unit's start and never goes past the last
//! unit, so a client sees the text's ends. The caret is the selection's
//! start, as the edit controls report it, and the focused node's caret
//! moves with the keys [`caret_key`] lists. The language is English
//! (`en-US`); the annotation types are the spelling error type for a range
//! touching one of the fixture's spelling errors and unsupported
//! otherwise, as Windows 11 Notepad reports them; the font is 11 point
//! Consolas in black, neither italic nor underlined, its weight 700 in the
//! fixture's bold stretches, 400 elsewhere, and mixed across both; every
//! other text attribute is unsupported.
//!
//! Every provider method counts a hit ([`crate::hits`]), so the tests pin a
//! text operation's provider work exactly.

#![allow(
    clippy::inline_always,
    clippy::ref_as_ptr,
    clippy::used_underscore_binding
)]

use std::cell::Cell;
use std::mem::ManuallyDrop;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_ARRAY, VT_I4, VT_R8, VT_UNKNOWN,
};
use windows::Win32::UI::Accessibility::{
    IRawElementProviderSimple, ITextProvider, ITextProvider_Impl, ITextProvider2,
    ITextProvider2_Impl, ITextRangeProvider, ITextRangeProvider_Impl, SupportedTextSelection,
    SupportedTextSelection_Single, TextPatternRangeEndpoint, TextPatternRangeEndpoint_Start,
    TextUnit, TextUnit_Character, TextUnit_Format, TextUnit_Line, TextUnit_Paragraph,
    TextUnit_Word, UIA_AnnotationTypesAttributeId, UIA_CultureAttributeId, UIA_FontNameAttributeId,
    UIA_FontSizeAttributeId, UIA_FontWeightAttributeId, UIA_ForegroundColorAttributeId,
    UIA_IsItalicAttributeId, UIA_TEXTATTRIBUTE_ID, UIA_UnderlineStyleAttributeId,
    UiaGetReservedMixedAttributeValue, UiaGetReservedNotSupportedValue, UiaPoint,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_DOWN, VK_RIGHT, VK_UP};
use windows::core::{BSTR, Interface, Result as WinResult};
use windows_core::{AsImpl, Error, IUnknown, implement};

use crate::hits::{self, Method};
use crate::tree::{Formats, SharedTree};

/// The language every range reports: English (United States).
const EN_US: i32 = 0x0409;

/// The text pattern of the node at `index`.
#[implement(ITextProvider2, ITextProvider, Agile = false)]
pub(super) struct TextProvider {
    pub(super) tree: SharedTree,
    pub(super) hwnd: HWND,
    pub(super) index: usize,
}

impl TextProvider {
    fn range(&self, start: usize, end: usize) -> ITextRangeProvider {
        TextRange {
            tree: self.tree.clone(),
            hwnd: self.hwnd,
            index: self.index,
            start: Cell::new(start),
            end: Cell::new(end),
        }
        .into()
    }

    fn selection(&self) -> (usize, usize) {
        let guard = self
            .tree
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.nodes[self.index].selection
    }
}

impl ITextProvider_Impl for TextProvider_Impl {
    fn GetSelection(&self) -> WinResult<*mut SAFEARRAY> {
        hits::hit(Method::TextGetSelection);
        let (start, end) = self.selection();
        Ok(range_array(&self.range(start, end)))
    }
    fn GetVisibleRanges(&self) -> WinResult<*mut SAFEARRAY> {
        hits::hit(Method::TextGetVisibleRanges);
        let length = text_of(&self.tree, self.index).len();
        Ok(range_array(&self.range(0, length)))
    }
    fn RangeFromChild(
        &self,
        _child: windows_core::Ref<IRawElementProviderSimple>,
    ) -> WinResult<ITextRangeProvider> {
        Err(Error::empty())
    }
    fn RangeFromPoint(&self, _point: &UiaPoint) -> WinResult<ITextRangeProvider> {
        Err(Error::empty())
    }
    fn DocumentRange(&self) -> WinResult<ITextRangeProvider> {
        hits::hit(Method::TextDocumentRange);
        let length = text_of(&self.tree, self.index).len();
        Ok(self.range(0, length))
    }
    fn SupportedTextSelection(&self) -> WinResult<SupportedTextSelection> {
        Ok(SupportedTextSelection_Single)
    }
}

impl ITextProvider2_Impl for TextProvider_Impl {
    fn RangeFromAnnotation(
        &self,
        _annotation: windows_core::Ref<IRawElementProviderSimple>,
    ) -> WinResult<ITextRangeProvider> {
        Err(Error::empty())
    }
    fn GetCaretRange(&self, isactive: *mut windows_core::BOOL) -> WinResult<ITextRangeProvider> {
        hits::hit(Method::TextGetCaretRange);
        if !isactive.is_null() {
            // SAFETY: a non-null out-parameter the caller provides.
            unsafe { *isactive = true.into() };
        }
        let (start, _) = self.selection();
        Ok(self.range(start, start))
    }
}

/// A range of the node's text, two UTF-16 offsets.
#[implement(ITextRangeProvider, Agile = false)]
struct TextRange {
    tree: SharedTree,
    hwnd: HWND,
    index: usize,
    start: Cell<usize>,
    end: Cell<usize>,
}

/// The node's text now.
fn text_of(tree: &SharedTree, index: usize) -> Vec<u16> {
    tree.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .nodes[index]
        .text
        .clone()
        .unwrap_or_default()
}

/// The node's formatting now.
fn formats_of(tree: &SharedTree, index: usize) -> Formats {
    tree.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .nodes[index]
        .formats
        .clone()
}

/// The spans of `unit` in the node's text: the format unit's from its
/// formatting, any other unit's from the text alone.
fn spans_of(tree: &SharedTree, index: usize, unit: TextUnit) -> Vec<(usize, usize)> {
    let text = text_of(tree, index);
    if unit != TextUnit_Format {
        return units(&text, unit);
    }
    let formats = formats_of(tree, index);
    let mut boundaries = vec![0, text.len()];
    for &(start, end) in formats.spelling_errors.iter().chain(&formats.bold) {
        boundaries.push(start.min(text.len()));
        boundaries.push(end.min(text.len()));
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    if text.is_empty() {
        return Vec::new();
    }
    boundaries
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .collect()
}

/// Whether a stretch from `start` to `end` touches any of `stretches`; an
/// empty one, whether one contains its position.
fn touches(stretches: &[(usize, usize)], start: usize, end: usize) -> bool {
    stretches.iter().any(|&(from, to)| {
        if start == end {
            from <= start && start < to
        } else {
            from < end && start < to
        }
    })
}

/// Whether every character from `start` to `end` lies in `stretches`; an
/// empty stretch, whether one contains its position.
fn within(stretches: &[(usize, usize)], start: usize, end: usize) -> bool {
    if start == end {
        return touches(stretches, start, end);
    }
    (start..end).all(|at| touches(stretches, at, at))
}

/// A variant holding `value`, an integer.
fn int_variant(value: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: value },
            }),
        },
    }
}

/// A variant holding one of UIA's sentinel objects.
fn sentinel_variant(sentinel: IUnknown) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_UNKNOWN,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    punkVal: ManuallyDrop::new(Some(sentinel)),
                },
            }),
        },
    }
}

/// UIA's spelling error annotation type.
const ANNOTATION_SPELLING_ERROR: i32 = 60001;

/// The spans of `unit` in `text`, in order; none for an empty text.
fn units(text: &[u16], unit: TextUnit) -> Vec<(usize, usize)> {
    let word = |code: u16| char::from_u32(u32::from(code)).is_some_and(char::is_alphanumeric);
    if unit == TextUnit_Character {
        return (0..text.len()).map(|start| (start, start + 1)).collect();
    }
    if unit == TextUnit_Word {
        let mut spans = Vec::new();
        let mut index = 0;
        while index < text.len() {
            let start = index;
            if word(text[index]) {
                while index < text.len() && word(text[index]) {
                    index += 1;
                }
                while index < text.len() && text[index] == u16::from(b' ') {
                    index += 1;
                }
            } else {
                index += 1;
            }
            spans.push((start, index));
        }
        return spans;
    }
    if unit == TextUnit_Line || unit == TextUnit_Paragraph {
        let mut spans = Vec::new();
        let mut start = 0;
        for (index, &code) in text.iter().enumerate() {
            if code == u16::from(b'\n') {
                spans.push((start, index + 1));
                start = index + 1;
            }
        }
        spans.push((start, text.len()));
        return spans;
    }
    vec![(0, text.len())]
}

/// The index of the span containing `at`: the last one at the text's end.
fn containing(spans: &[(usize, usize)], at: usize) -> usize {
    spans
        .iter()
        .position(|&(_, end)| at < end)
        .unwrap_or(spans.len().saturating_sub(1))
}

impl TextRange {
    fn endpoint(&self, endpoint: TextPatternRangeEndpoint) -> usize {
        if endpoint == TextPatternRangeEndpoint_Start {
            self.start.get()
        } else {
            self.end.get()
        }
    }

    /// Sets `endpoint` to `at`, moving the other with it when they would
    /// cross.
    fn set_endpoint(&self, endpoint: TextPatternRangeEndpoint, at: usize) {
        if endpoint == TextPatternRangeEndpoint_Start {
            self.start.set(at);
            if self.end.get() < at {
                self.end.set(at);
            }
        } else {
            self.end.set(at);
            if self.start.get() > at {
                self.start.set(at);
            }
        }
    }
}

/// The offsets of the range behind `range`, one of this module's.
fn offsets_of(range: &ITextRangeProvider) -> (usize, usize) {
    // SAFETY: every range a client hands back to this provider is one this
    // module created, so its implementation is a `TextRange`.
    let other: &TextRange = unsafe { range.as_impl() };
    (other.start.get(), other.end.get())
}

impl ITextRangeProvider_Impl for TextRange_Impl {
    fn Clone(&self) -> WinResult<ITextRangeProvider> {
        hits::hit(Method::RangeClone);
        Ok(TextRange {
            tree: self.tree.clone(),
            hwnd: self.hwnd,
            index: self.index,
            start: Cell::new(self.start.get()),
            end: Cell::new(self.end.get()),
        }
        .into())
    }
    fn Compare(
        &self,
        range: windows_core::Ref<ITextRangeProvider>,
    ) -> WinResult<windows_core::BOOL> {
        hits::hit(Method::RangeCompare);
        let other = range.as_ref().ok_or_else(Error::empty)?;
        Ok((offsets_of(other) == (self.start.get(), self.end.get())).into())
    }
    fn CompareEndpoints(
        &self,
        endpoint: TextPatternRangeEndpoint,
        targetrange: windows_core::Ref<ITextRangeProvider>,
        targetendpoint: TextPatternRangeEndpoint,
    ) -> WinResult<i32> {
        hits::hit(Method::RangeCompareEndpoints);
        let other = offsets_of(targetrange.as_ref().ok_or_else(Error::empty)?);
        let target = if targetendpoint == TextPatternRangeEndpoint_Start {
            other.0
        } else {
            other.1
        };
        Ok(match self.endpoint(endpoint).cmp(&target) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        })
    }
    fn ExpandToEnclosingUnit(&self, unit: TextUnit) -> WinResult<()> {
        hits::hit(Method::RangeExpandToEnclosingUnit);
        let spans = spans_of(&self.tree, self.index, unit);
        let (start, end) = spans
            .get(containing(&spans, self.start.get()))
            .copied()
            .unwrap_or((0, 0));
        self.start.set(start);
        self.end.set(end);
        Ok(())
    }
    fn FindAttribute(
        &self,
        _attributeid: UIA_TEXTATTRIBUTE_ID,
        _val: &VARIANT,
        _backward: windows_core::BOOL,
    ) -> WinResult<ITextRangeProvider> {
        Err(Error::empty())
    }
    fn FindText(
        &self,
        _text: &BSTR,
        _backward: windows_core::BOOL,
        _ignorecase: windows_core::BOOL,
    ) -> WinResult<ITextRangeProvider> {
        Err(Error::empty())
    }
    fn GetAttributeValue(&self, attributeid: UIA_TEXTATTRIBUTE_ID) -> WinResult<VARIANT> {
        hits::hit(Method::RangeGetAttributeValue);
        let (start, end) = (self.start.get(), self.end.get());
        let formats = formats_of(&self.tree, self.index);
        let not_supported = || {
            // SAFETY: UIA's own sentinel object, owned by the returned
            // variant.
            unsafe { UiaGetReservedNotSupportedValue() }.map(sentinel_variant)
        };
        // The attribute ids are the `windows` crate's constants, named as
        // UIA names them.
        #[allow(non_upper_case_globals)]
        Ok(match attributeid {
            UIA_CultureAttributeId => int_variant(EN_US),
            UIA_AnnotationTypesAttributeId => {
                if touches(&formats.spelling_errors, start, end) {
                    let spelling = ANNOTATION_SPELLING_ERROR;
                    // SAFETY: a one-element vector of plain integers, the
                    // pointer to a local the call copies.
                    let array = unsafe {
                        super::props::filled_vector(VT_I4, &[(&raw const spelling).cast()])
                    };
                    // The returned variant owns the array.
                    VARIANT {
                        Anonymous: VARIANT_0 {
                            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                                vt: VARENUM(VT_ARRAY.0 | VT_I4.0),
                                wReserved1: 0,
                                wReserved2: 0,
                                wReserved3: 0,
                                Anonymous: VARIANT_0_0_0 { parray: array },
                            }),
                        },
                    }
                } else {
                    not_supported()?
                }
            }
            UIA_FontNameAttributeId => VARIANT::from(BSTR::from("Consolas")),
            UIA_FontSizeAttributeId => VARIANT::from(11.0_f64),
            UIA_FontWeightAttributeId => {
                if within(&formats.bold, start, end) {
                    int_variant(700)
                } else if touches(&formats.bold, start, end) {
                    // SAFETY: UIA's own sentinel object, owned by the
                    // returned variant.
                    sentinel_variant(unsafe { UiaGetReservedMixedAttributeValue() }?)
                } else {
                    int_variant(400)
                }
            }
            UIA_IsItalicAttributeId => VARIANT::from(false),
            UIA_UnderlineStyleAttributeId | UIA_ForegroundColorAttributeId => int_variant(0),
            _ => not_supported()?,
        })
    }
    fn GetBoundingRectangles(&self) -> WinResult<*mut SAFEARRAY> {
        hits::hit(Method::RangeGetBoundingRectangles);
        // One rectangle at the range's start, in a fixed grid: each
        // character 8 pixels wide, each line 16 high, from (100, 200).
        let text = text_of(&self.tree, self.index);
        let start = self.start.get().min(text.len());
        let before = &text[..start];
        let line_feed = u16::from(b'\n');
        let line = before.iter().filter(|&&code| code == line_feed).count();
        let column = before
            .iter()
            .rev()
            .take_while(|&&code| code != line_feed)
            .count();
        #[expect(
            clippy::cast_precision_loss,
            reason = "a fixture's small line and column numbers"
        )]
        let rectangle = [
            100.0 + 8.0 * column as f64,
            200.0 + 16.0 * line as f64,
            8.0,
            16.0,
        ];
        let pointers: Vec<*const std::ffi::c_void> = rectangle
            .iter()
            .map(|value| std::ptr::from_ref(value).cast())
            .collect();
        // SAFETY: pointers to doubles that outlive the call, for a `VT_R8`
        // vector, whose elements the call copies.
        Ok(unsafe { super::props::filled_vector(VT_R8, &pointers) })
    }
    fn GetEnclosingElement(&self) -> WinResult<IRawElementProviderSimple> {
        super::props::provider_for(self.tree.clone(), self.hwnd, self.index).cast()
    }
    fn GetText(&self, maxlength: i32) -> WinResult<BSTR> {
        hits::hit(Method::RangeGetText);
        let text = text_of(&self.tree, self.index);
        let end = self.end.get().min(text.len());
        let start = self.start.get().min(end);
        let mut slice = &text[start..end];
        if let Ok(max) = usize::try_from(maxlength) {
            slice = &slice[..slice.len().min(max)];
        }
        Ok(BSTR::from_wide(slice))
    }
    fn Move(&self, unit: TextUnit, count: i32) -> WinResult<i32> {
        hits::hit(Method::RangeMove);
        let spans = spans_of(&self.tree, self.index, unit);
        if spans.is_empty() || count == 0 {
            return Ok(0);
        }
        let collapsed = self.start.get() == self.end.get();
        let from = containing(&spans, self.start.get());
        let mut moved = 0;
        let mut at = from;
        if count > 0 {
            while moved < count && at + 1 < spans.len() {
                at += 1;
                moved += 1;
            }
        } else {
            if self.start.get() > spans[from].0 {
                // Back to the start of the unit it is in counts as one.
                moved -= 1;
            }
            while moved > count && at > 0 {
                at -= 1;
                moved -= 1;
            }
        }
        let (start, end) = spans[at];
        self.start.set(start);
        self.end.set(if collapsed { start } else { end });
        Ok(moved)
    }
    fn MoveEndpointByUnit(
        &self,
        endpoint: TextPatternRangeEndpoint,
        unit: TextUnit,
        count: i32,
    ) -> WinResult<i32> {
        hits::hit(Method::RangeMoveEndpointByUnit);
        let text = text_of(&self.tree, self.index);
        let mut boundaries: Vec<usize> = spans_of(&self.tree, self.index, unit)
            .iter()
            .map(|&(start, _)| start)
            .collect();
        boundaries.push(text.len());
        boundaries.dedup();
        let mut at = self.endpoint(endpoint);
        let mut moved = 0;
        while moved < count {
            let Some(&next) = boundaries.iter().find(|&&boundary| boundary > at) else {
                break;
            };
            at = next;
            moved += 1;
        }
        while moved > count {
            let Some(&previous) = boundaries.iter().rev().find(|&&boundary| boundary < at) else {
                break;
            };
            at = previous;
            moved -= 1;
        }
        self.set_endpoint(endpoint, at);
        Ok(moved)
    }
    fn MoveEndpointByRange(
        &self,
        endpoint: TextPatternRangeEndpoint,
        targetrange: windows_core::Ref<ITextRangeProvider>,
        targetendpoint: TextPatternRangeEndpoint,
    ) -> WinResult<()> {
        hits::hit(Method::RangeMoveEndpointByRange);
        let other = offsets_of(targetrange.as_ref().ok_or_else(Error::empty)?);
        let target = if targetendpoint == TextPatternRangeEndpoint_Start {
            other.0
        } else {
            other.1
        };
        self.set_endpoint(endpoint, target);
        Ok(())
    }
    fn Select(&self) -> WinResult<()> {
        hits::hit(Method::RangeSelect);
        let mut guard = self
            .tree
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.nodes[self.index].selection = (self.start.get(), self.end.get());
        Ok(())
    }
    fn AddToSelection(&self) -> WinResult<()> {
        Err(Error::empty())
    }
    fn RemoveFromSelection(&self) -> WinResult<()> {
        Err(Error::empty())
    }
    fn ScrollIntoView(&self, _aligntotop: windows_core::BOOL) -> WinResult<()> {
        Ok(())
    }
    fn GetChildren(&self) -> WinResult<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }
}

/// A one-element `VT_UNKNOWN` array of `range`, as `GetSelection` answers,
/// or null when it cannot be built. `SafeArrayPutElement` takes its own
/// reference.
fn range_array(range: &ITextRangeProvider) -> *mut SAFEARRAY {
    // SAFETY: the range's interface pointer, for a `VT_UNKNOWN` vector.
    unsafe { super::props::filled_vector(VT_UNKNOWN, &[range.as_raw().cast_const()]) }
}

/// Moves the focused node's caret for `key`, with Control held when
/// `control` is set, as an editor moves its caret, by this module's units:
/// Right Arrow to the next character, Control+Right Arrow to the next
/// word's start, Down and Up Arrow to the same column of the next or the
/// previous line (its end when the line is shorter). These are the keys the
/// end-to-end suite presses.
/// Returns whether the caret was moved; any other key, or a focus without
/// text, is left alone.
pub(crate) fn caret_key(tree: &SharedTree, key: VIRTUAL_KEY, control: bool) -> bool {
    let mut guard = tree
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(index) = guard.focused else {
        return false;
    };
    let node = &mut guard.nodes[index];
    let Some(text) = node.text.as_deref() else {
        return false;
    };
    let caret = node.selection.0.min(text.len());
    // A line's end, before its line feed.
    let line_end = |(start, end): (usize, usize)| {
        if end > start && text[end - 1] == u16::from(b'\n') {
            end - 1
        } else {
            end
        }
    };
    let to = match (key, control) {
        (VK_RIGHT, false) => (caret + 1).min(text.len()),
        (VK_RIGHT, true) => units(text, TextUnit_Word)
            .iter()
            .map(|&(start, _)| start)
            .find(|&start| start > caret)
            .unwrap_or(text.len()),
        (VK_DOWN | VK_UP, false) => {
            let lines = units(text, TextUnit_Line);
            let line = containing(&lines, caret);
            let target = if key == VK_DOWN {
                Some(line + 1).filter(|&next| next < lines.len())
            } else {
                line.checked_sub(1)
            };
            let Some(target) = target else {
                return true;
            };
            let column = caret - lines[line].0;
            (lines[target].0 + column).min(line_end(lines[target]))
        }
        _ => return false,
    };
    node.selection = (to, to);
    true
}

/// Whether the provider at `index` serves the text pattern: it has text.
pub(super) fn has_text(tree: &SharedTree, index: usize) -> bool {
    tree.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .nodes[index]
        .text
        .is_some()
}

/// The text pattern's interface pointer for `GetPatternProvider`.
pub(super) fn provider(tree: &SharedTree, hwnd: HWND, index: usize) -> IUnknown {
    let provider: ITextProvider2 = TextProvider {
        tree: tree.clone(),
        hwnd,
        index,
    }
    .into();
    provider.into()
}
