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

use windows::Win32::System::Variant::{VARIANT, VT_BSTR};
use windows::Win32::UI::Accessibility::{
    IUIAutomationTextPattern, TreeScope_Children, UIA_AutomationIdPropertyId,
    UIA_FontNameAttributeId, UIA_FrameworkIdPropertyId, UIA_TextPatternId,
};
use windows::core::{BSTR, HRESULT, Interface};

use verbatim_model::CallKind;

use crate::calls::count;
use crate::client::Uia;

/// UIA's error for a provider that did not answer within the connection
/// timeout.
const UIA_E_TIMEOUT: HRESULT = HRESULT(0x8013_1505_u32.cast_signed());

thread_local! {
    /// This thread's client, built on first use; `None` again after a failed
    /// build so the next call retries.
    static CLIENT: RefCell<Option<Uia>> = const { RefCell::new(None) };
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
        // SAFETY: plain COM calls on live objects from this thread's client:
        // find the child whose automation id is "Text Area", ask it for its
        // text pattern, its visible ranges, and the first range's font.
        unsafe {
            let id = VARIANT::from(BSTR::from("Text Area"));
            let condition = uia
                .client()
                .CreatePropertyCondition(UIA_AutomationIdPropertyId, &id)?;
            count(CallKind::Uia);
            let text_area = window.FindFirst(TreeScope_Children, &condition)?;
            count(CallKind::Uia);
            let pattern: IUIAutomationTextPattern =
                text_area.GetCurrentPattern(UIA_TextPatternId)?.cast()?;
            count(CallKind::Uia);
            let ranges = pattern.GetVisibleRanges()?;
            if ranges.Length()? != 1 {
                return Ok(false);
            }
            count(CallKind::Uia);
            let font = ranges
                .GetElement(0)?
                .GetAttributeValue(UIA_FontNameAttributeId)?;
            Ok(font.vt() == VT_BSTR)
        }
    })
}

/// Whether the window `hwnd` comes from Windows Forms: its UIA framework is
/// `WinForm`. Cross-process; the outpost's worker only.
#[must_use]
pub fn is_windows_forms(hwnd: isize) -> Option<bool> {
    with_client(|uia| {
        // SAFETY: plain COM calls on live objects from this thread's client:
        // a cache request for the framework, then the window's element.
        unsafe {
            let cache = uia.client().CreateCacheRequest()?;
            cache.AddProperty(UIA_FrameworkIdPropertyId)?;
            let window = uia.element_from_handle(hwnd, &cache)?;
            Ok(window.CachedFrameworkId()? == "WinForm")
        }
    })
}
