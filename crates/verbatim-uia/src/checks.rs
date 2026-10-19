//! The checks NVDA makes on a window that has a UIA provider before using
//! that provider (`nvda/source/UIAHandler/utils.py`, reference only): whether
//! a console's provider is the complete one of current Windows, and whether
//! a list view comes from Windows Forms. The arbitrator decides which check
//! a window needs; these read the answer.
//!
//! Each answer is `None` when the application did not answer in time, so the
//! caller can ask again rather than keep a verdict a busy application
//! forced, and `Some(false)` for any other failure, as NVDA treats one.

use std::cell::RefCell;

use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Variant::{VARIANT, VT_BSTR};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, TreeScope_Children, UIA_AutomationIdPropertyId,
    UIA_FontNameAttributeId, UIA_TextPatternId,
};
use windows::core::{BSTR, HRESULT};

use crate::client::Uia;
use crate::element::{ElementExt, attribute_value, visible_ranges};
use crate::shape::FrameworkCheck;

/// UIA's error for a provider that did not answer within the connection
/// timeout.
const UIA_E_TIMEOUT: HRESULT = HRESULT(0x8013_1505_u32.cast_signed());

thread_local! {
    /// This thread's client, built on first use; `None` again after a failed
    /// build so the next call retries.
    static CLIENT: RefCell<Option<Uia>> = const { RefCell::new(None) };
}

/// Drops this thread's client, if one was built (see
/// [`crate::release_thread_state`]).
pub(crate) fn release_thread_client() {
    let client = CLIENT.with(|cell| cell.borrow_mut().take());
    drop(client);
}

/// Runs `read` with this thread's client, mapping a timeout to `None` and any
/// other failure to `Some(false)`.
fn with_client(read: impl FnOnce(&Uia) -> windows::core::Result<bool>) -> Option<bool> {
    CLIENT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Uia::new().ok();
        }
        let uia = slot.as_ref()?;
        answer(read(uia))
    })
}

/// A check's answer: `None` for a timeout, `Some(false)` for any other
/// failure.
fn answer(read: windows::core::Result<bool>) -> Option<bool> {
    match read {
        Ok(answer) => Some(answer),
        Err(error) if error.code() == UIA_E_TIMEOUT => None,
        Err(_) => Some(false),
    }
}

/// The automation id of a console's text area, which is not localized.
pub const CONSOLE_TEXT_AREA: &str = "Text Area";

/// Whether the console window `hwnd` has the complete provider of current
/// Windows, which NVDA uses: its text area reports one visible range, and
/// that range reports its font, as a console with formatted text does.
/// Older consoles' providers are incomplete, and NVDA reads them through
/// MSAA. Cross-process, and a connection to the window; an outpost's worker
/// only, on its own application's window.
#[must_use]
pub fn console_reports_formatting(hwnd: isize) -> Option<bool> {
    with_client(|uia| {
        // Nothing of the window's element is read: it is only searched
        // under.
        let window = uia.window_live(hwnd)?;
        reports_formatting(&text_area_under(uia, window.element())?)
    })
}

/// [`console_reports_formatting`] from an element of the console the caller
/// already holds, with no connection to its window
/// (`docs/design/listener-without-connections.md`, section 3, step 5): the
/// focus listener's check, from an event's sender. `element` is the
/// console's text area, when `automation_id`, its cached automation id, is
/// "Text Area"; else the console window's element, under which the text
/// area is found. Cross-process calls on held elements: three from the text
/// area, four from the window's element.
#[must_use]
pub fn held_console_reports_formatting(
    uia: &Uia,
    element: &IUIAutomationElement,
    automation_id: Option<&str>,
) -> Option<bool> {
    answer((|| {
        if automation_id == Some(CONSOLE_TEXT_AREA) {
            reports_formatting(element)
        } else {
            reports_formatting(&text_area_under(uia, element)?)
        }
    })())
}

/// The console's text area: the child of `window`, the console window's
/// element, whose automation id is "Text Area". One call.
fn text_area_under(
    uia: &Uia,
    window: &IUIAutomationElement,
) -> windows::core::Result<IUIAutomationElement> {
    let id = VARIANT::from(BSTR::from(CONSOLE_TEXT_AREA));
    let condition = uia.property_condition(UIA_AutomationIdPropertyId, &id)?;
    // No match: `FindFirst` answers a null element.
    window
        .find_first(TreeScope_Children, &condition)?
        .ok_or_else(|| windows::core::Error::from(E_FAIL))
}

/// Whether the console's text area `text_area` reports one visible range,
/// and that range its font: its text pattern, its visible ranges and the
/// first range's font, three calls.
fn reports_formatting(text_area: &IUIAutomationElement) -> windows::core::Result<bool> {
    let pattern = text_area.current_pattern::<IUIAutomationTextPattern>(UIA_TextPatternId)?;
    let ranges = visible_ranges(&pattern)?;
    let [range] = ranges.as_slice() else {
        return Ok(false);
    };
    let font = attribute_value(range, UIA_FontNameAttributeId)?;
    Ok(font.vt() == VT_BSTR)
}

/// Whether the window `hwnd` comes from Windows Forms: its UIA framework is
/// `WinForm`. Cross-process; the outpost's worker only.
#[must_use]
pub fn is_windows_forms(hwnd: isize) -> Option<bool> {
    with_client(|uia| {
        let request = uia.request_fixed::<FrameworkCheck>()?;
        let window = uia.window_element(hwnd, &request)?;
        Ok(window.read().framework_id.as_deref() == Some("WinForm"))
    })
}
