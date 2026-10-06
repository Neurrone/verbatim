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
    IUIAutomationTextPattern, TreeScope_Children, UIA_AutomationIdPropertyId,
    UIA_FontNameAttributeId, UIA_FrameworkIdPropertyId, UIA_TextPatternId,
};
use windows::core::{BSTR, HRESULT};

use crate::client::Uia;
use crate::element::{ElementExt, attribute_value, visible_ranges};

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
        match read(uia) {
            Ok(answer) => Some(answer),
            Err(error) if error.code() == UIA_E_TIMEOUT => None,
            Err(_) => Some(false),
        }
    })
}

/// Whether the console window `hwnd` has the complete provider of current
/// Windows, which NVDA uses: its text area reports one visible range, and
/// that range reports its font, as a console with formatted text does.
/// Older consoles' providers are incomplete, and NVDA reads them through
/// MSAA. Cross-process; the outpost's worker only.
#[must_use]
pub fn console_reports_formatting(hwnd: isize) -> Option<bool> {
    with_client(|uia| {
        let cache = uia.base_cache_request()?;
        let window = uia.element_from_handle(hwnd, &cache)?;
        // Find the child whose automation id is "Text Area", ask it for its
        // text pattern, its visible ranges, and the first range's font.
        let id = VARIANT::from(BSTR::from("Text Area"));
        let condition = uia.property_condition(UIA_AutomationIdPropertyId, &id)?;
        let Some(text_area) = window.find_first(TreeScope_Children, &condition)? else {
            // No match: `FindFirst` answers a null element.
            return Err(windows::core::Error::from(E_FAIL));
        };
        let pattern = text_area.current_pattern::<IUIAutomationTextPattern>(UIA_TextPatternId)?;
        let ranges = visible_ranges(&pattern)?;
        let [range] = ranges.as_slice() else {
            return Ok(false);
        };
        let font = attribute_value(range, UIA_FontNameAttributeId)?;
        Ok(font.vt() == VT_BSTR)
    })
}

/// Whether the window `hwnd` comes from Windows Forms: its UIA framework is
/// `WinForm`. Cross-process; the outpost's worker only.
#[must_use]
pub fn is_windows_forms(hwnd: isize) -> Option<bool> {
    with_client(|uia| {
        // A cache request for the framework, then the window's element.
        let cache = uia.cache_request(&[UIA_FrameworkIdPropertyId])?;
        let window = uia.element_from_handle(hwnd, &cache)?;
        Ok(window.cached_framework_id().as_deref() == Some("WinForm"))
    })
}
