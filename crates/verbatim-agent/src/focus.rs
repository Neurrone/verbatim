//! The focused element as UI Automation reports it, for
//! [`Request::FocusedElement`](crate::protocol::Request::FocusedElement):
//! state a test reads independently of the screen reader under test, such
//! as which item of the desktop has the focus, to fix what it expects the
//! screen reader to say before it says it.

use std::io;

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Ole::{SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_ARRAY, VT_I4};
use windows::Win32::UI::Accessibility::{
    AnnotationType_SpellingError, CUIAutomation, IUIAutomation, IUIAutomationElement,
    IUIAutomationSelectionItemPattern, IUIAutomationTextPattern, TextPatternRangeEndpoint_End,
    TextPatternRangeEndpoint_Start, TextUnit_Word, TreeScope_Children, TreeScope_Descendants,
    UIA_AnnotationTypesAttributeId, UIA_AutomationIdPropertyId, UIA_PositionInSetPropertyId,
    UIA_SelectionItemPatternId, UIA_SizeOfSetPropertyId, UIA_TextPatternId,
};

use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::core::BSTR;

use crate::protocol::FocusedElement;

/// Focuses the first element of the foreground window whose UI Automation
/// identifier is `automation_id`, with UI Automation's `SetFocus`, which
/// injects no input, and returns how many children it has.
///
/// # Errors
///
/// Returns an error if UI Automation cannot be reached, there is no
/// foreground window, no such element is found, or it refuses the focus.
pub fn focus_by_automation_id(automation_id: &str) -> io::Result<u32> {
    // SAFETY: as in `focused_element`.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: as in `focused_element`.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(io::Error::other)?;
    // SAFETY: no preconditions.
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return Err(io::Error::other("there is no foreground window"));
    }
    // SAFETY: plain COM calls on live interfaces.
    let root = unsafe { automation.ElementFromHandle(window) }.map_err(io::Error::other)?;
    let value = VARIANT::from(BSTR::from(automation_id));
    // SAFETY: as above.
    let condition =
        unsafe { automation.CreatePropertyCondition(UIA_AutomationIdPropertyId, &value) }
            .map_err(io::Error::other)?;
    // SAFETY: as above.
    let element = unsafe { root.FindFirst(TreeScope_Descendants, &condition) }.map_err(|_| {
        io::Error::other(format!(
            "no element of the foreground window has the identifier {automation_id:?}"
        ))
    })?;
    // SAFETY: as above.
    unsafe { element.SetFocus() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let any = unsafe { automation.CreateTrueCondition() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let children =
        unsafe { element.FindAll(TreeScope_Children, &any) }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let count = unsafe { children.Length() }.map_err(io::Error::other)?;
    u32::try_from(count).map_err(io::Error::other)
}

/// The focused element's name, its position among its siblings as UI
/// Automation reports it, and whether it is selected, when it can be.
///
/// # Errors
///
/// Returns an error if UI Automation cannot be reached or reports no focus.
pub fn focused_element() -> io::Result<FocusedElement> {
    // SAFETY: initializes COM for this connection's thread; a thread already
    // initialized in the same mode is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: creates the in-process UI Automation client.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(io::Error::other)?;
    // SAFETY: a plain COM call on a live interface.
    let element = unsafe { automation.GetFocusedElement() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let name = unsafe { element.CurrentName() }
        .map_err(io::Error::other)?
        .to_string();
    let integer = |property| {
        // SAFETY: as above.
        unsafe { element.GetCurrentPropertyValue(property) }
            .ok()
            .and_then(|value| i32::try_from(&value).ok())
            .filter(|value| *value > 0)
    };
    let position = integer(UIA_PositionInSetPropertyId)
        .zip(integer(UIA_SizeOfSetPropertyId))
        .or_else(|| position_among_siblings(&automation, &element));
    // SAFETY: as above.
    let selected = unsafe {
        element.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
    }
    .ok()
    // SAFETY: as above.
    .and_then(|pattern| unsafe { pattern.CurrentIsSelected() }.ok())
    .map(windows::core::BOOL::as_bool);
    Ok(FocusedElement {
        name,
        position,
        selected,
    })
}

/// The most words of a text [`misspelt_words`] reads, so a long document
/// cannot hold a request for long.
const MAX_WORDS: usize = 1000;

/// The words of the focused element's text that its application marks as
/// misspelt with UI Automation's spelling-error annotation, in order, each
/// with the white space after it trimmed.
///
/// # Errors
///
/// Returns an error if UI Automation cannot be reached, nothing has the
/// focus, or the focused element has no text.
pub fn misspelt_words() -> io::Result<Vec<String>> {
    // SAFETY: as in `focused_element`.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: as in `focused_element`.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(io::Error::other)?;
    // SAFETY: plain COM calls on live interfaces.
    let focus = unsafe { automation.GetFocusedElement() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let text = unsafe { focus.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
        .map_err(|error| io::Error::other(format!("the focused element has no text: {error}")))?;
    // SAFETY: as above.
    let document = unsafe { text.DocumentRange() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let word = unsafe { document.Clone() }.map_err(io::Error::other)?;
    // SAFETY: as above: the word range collapses to the document's start.
    unsafe {
        word.MoveEndpointByRange(
            TextPatternRangeEndpoint_End,
            &word,
            TextPatternRangeEndpoint_Start,
        )
    }
    .map_err(io::Error::other)?;
    let mut misspelt = Vec::new();
    for _ in 0..MAX_WORDS {
        // SAFETY: as above.
        unsafe { word.ExpandToEnclosingUnit(TextUnit_Word) }.map_err(io::Error::other)?;
        // SAFETY: as above.
        let types = unsafe { word.GetAttributeValue(UIA_AnnotationTypesAttributeId) }
            .map_err(io::Error::other)?;
        if annotation_types(&types).contains(&AnnotationType_SpellingError.0) {
            // SAFETY: as above.
            let spelled = unsafe { word.GetText(-1) }.map_err(io::Error::other)?;
            misspelt.push(spelled.to_string().trim_end().to_owned());
        }
        // SAFETY: as above.
        let moved = unsafe { word.Move(TextUnit_Word, 1) }.map_err(io::Error::other)?;
        if moved == 0 {
            return Ok(misspelt);
        }
    }
    Err(io::Error::other(format!(
        "the focused text has more than {MAX_WORDS} words"
    )))
}

/// The annotation types a text range's attribute value holds: an array of
/// them, or one; nothing for anything else, such as UI Automation's "not
/// supported" and "mixed" values.
fn annotation_types(value: &VARIANT) -> Vec<i32> {
    if value.vt() == VT_I4 {
        return i32::try_from(value)
            .map(|single| vec![single])
            .unwrap_or_default();
    }
    if value.vt() != VARENUM(VT_ARRAY.0 | VT_I4.0) {
        return Vec::new();
    }
    // SAFETY: every initialized variant holds its type tag and value in
    // this member of the outer union.
    let tagged = unsafe { &value.Anonymous.Anonymous };
    // SAFETY: the variant's type says this union holds a SAFEARRAY of i32,
    // which the variant owns and keeps alive while it is borrowed.
    let array = unsafe { tagged.Anonymous.parray };
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: `array` is the variant's valid one-dimensional SAFEARRAY.
    let lower = unsafe { SafeArrayGetLBound(array, 1) };
    // SAFETY: as above.
    let upper = unsafe { SafeArrayGetUBound(array, 1) };
    let (Ok(lower), Ok(upper)) = (lower, upper) else {
        return Vec::new();
    };
    (lower..=upper)
        .filter_map(|index| {
            let mut element = 0i32;
            // SAFETY: `index` is within the array's bounds, and the element
            // is an i32, the size of what it is copied into.
            unsafe { SafeArrayGetElement(array, &raw const index, (&raw mut element).cast()) }
                .ok()
                .map(|()| element)
        })
        .collect()
}

/// The position of `element` among its parent's children, counting from
/// one, and how many children there are: what a list item's position is
/// when UI Automation reports none, as for the items of a Win32 list view
/// such as the desktop's. `None` when the parent or its children cannot be
/// read, or `element` is not among them.
fn position_among_siblings(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
) -> Option<(i32, i32)> {
    // SAFETY: plain COM calls on live interfaces.
    let walker = unsafe { automation.RawViewWalker() }.ok()?;
    // SAFETY: as above.
    let parent = unsafe { walker.GetParentElement(element) }.ok()?;
    // SAFETY: as above.
    let any = unsafe { automation.CreateTrueCondition() }.ok()?;
    // SAFETY: as above.
    let children = unsafe { parent.FindAll(TreeScope_Children, &any) }.ok()?;
    // SAFETY: as above.
    let count = unsafe { children.Length() }.ok()?;
    (0..count).find_map(|index| {
        // SAFETY: as above, with an index below the length.
        let child = unsafe { children.GetElement(index) }.ok()?;
        // SAFETY: as above.
        let same = unsafe { automation.CompareElements(&child, element) }.ok()?;
        same.as_bool().then_some((index + 1, count))
    })
}
