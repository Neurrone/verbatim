//! The focused object as MSAA reports it, for
//! [`Request::FocusedElement`](crate::protocol::Request::FocusedElement):
//! state a test reads independently of the screen reader under test, such
//! as which item of the desktop has the focus, to fix what it expects the
//! screen reader to say before it says it. Two requests need facts only UI
//! Automation has and keep it: [`focus_by_automation_id`] and
//! [`misspelt_words`] (`docs/design/listener-without-connections.md`,
//! section 4.5).

use std::io;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, IDispatch,
};
use windows::Win32::System::Ole::{SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_ARRAY, VT_I4};
use windows::Win32::UI::Accessibility::{
    AccessibleObjectFromWindow, AnnotationType_SpellingError, CUIAutomation, IAccessible,
    IUIAutomation, IUIAutomationTextPattern, TextPatternRangeEndpoint_End,
    TextPatternRangeEndpoint_Start, TextUnit_Word, TreeScope_Children, TreeScope_Descendants,
    UIA_AnnotationTypesAttributeId, UIA_AutomationIdPropertyId, UIA_TextPatternId,
    WindowFromAccessibleObject,
};
use windows::Win32::UI::Controls::{LVM_GETITEMCOUNT, LVM_GETNEXTITEM, LVNI_FOCUSED};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, GUITHREADINFO, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo,
    OBJID_CLIENT, SMTO_ABORTIFHUNG, STATE_SYSTEM_SELECTABLE, STATE_SYSTEM_SELECTED,
    SendMessageTimeoutW,
};
use windows::core::{BSTR, Interface};

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
    through_uia("FocusByAutomationId", || focus_through_uia(automation_id))
}

/// [`focus_by_automation_id`]'s UI Automation calls.
fn focus_through_uia(automation_id: &str) -> io::Result<u32> {
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

/// How long one of the agent's UI Automation requests may take before it
/// is logged as slow: UIA's default connection timeout, which these
/// requests' client leaves as it is.
const SLOW_UIA_REQUEST: std::time::Duration = std::time::Duration::from_secs(2);

/// Runs `read`, one of the two requests that keep UI Automation, and logs
/// how long it took, at warn when it took [`SLOW_UIA_REQUEST`] or longer,
/// so the scenarios that use them can count the agent's slow answers apart
/// from Verbatim's (`docs/design/listener-without-connections.md`, section
/// 4.5).
fn through_uia<T>(request: &str, read: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let started = std::time::Instant::now();
    let answer = read();
    let elapsed_ms = started.elapsed().as_millis();
    if started.elapsed() >= SLOW_UIA_REQUEST {
        tracing::warn!(
            request,
            elapsed_ms,
            ok = answer.is_ok(),
            "a slow UI Automation request"
        );
    } else {
        tracing::info!(
            request,
            elapsed_ms,
            ok = answer.is_ok(),
            "a UI Automation request"
        );
    }
    answer
}

/// The focused object's name, its position in its list when it is an item
/// of a Win32 list view, and whether it is selected, when it can be, read
/// through MSAA (`docs/design/listener-without-connections.md`, decision
/// 10): the window with the keyboard focus of the thread in the foreground,
/// its client object (`AccessibleObjectFromWindow`), and the child its
/// `accFocus` names, followed into the window of an object it names. The desktop's list, which every scenario reads at its
/// start, is a comctl32 list view with no UIA provider, so MSAA reads it
/// as Verbatim does, and no UIA connection is made into File Explorer.
///
/// # Errors
///
/// Returns an error if no window has the keyboard focus or its accessible
/// object cannot be read.
pub fn focused_element() -> io::Result<FocusedElement> {
    // SAFETY: initializes COM for this connection's thread, which the
    // marshaling of a cross-process `IAccessible` needs; a thread already
    // initialized in the same mode is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut window = focused_window().ok_or_else(|| io::Error::other("no window has the focus"))?;
    // The focus window's client, and the object its `accFocus` names. A
    // focus window's client can name a child object in a window of its own
    // as its focus, as the desktop's `SHELLDLL_DefView` names its list
    // while the keyboard focus is still on it, just after a dialog closes:
    // that window is then read the same way, a few windows deep at most.
    let mut hops = 0;
    let (object, child, focus) = loop {
        let client = client_of(window)?;
        // SAFETY: a plain cross-process call on a live object.
        let focus = unsafe { client.accFocus() }.unwrap_or_default();
        if focus.vt() == VT_I4 {
            let child = i32::try_from(&focus).unwrap_or(CHILDID_SELF.cast_signed());
            break (client, child, focus);
        }
        let Ok(object) =
            IDispatch::try_from(&focus).and_then(|dispatch| dispatch.cast::<IAccessible>())
        else {
            break (client, CHILDID_SELF.cast_signed(), focus);
        };
        let mut inner = HWND::default();
        // SAFETY: `object` is a live accessible object; `inner` is a local
        // the call fills.
        let found = unsafe { WindowFromAccessibleObject(&object, Some(&raw mut inner)) };
        hops += 1;
        if found.is_err() || inner.is_invalid() || inner == window || hops > 4 {
            break (object, CHILDID_SELF.cast_signed(), focus);
        }
        window = inner;
    };
    let child = VARIANT::from(child);
    // SAFETY: as above.
    let name = unsafe { object.get_accName(&child) }
        .map(|name| name.to_string())
        .unwrap_or_default();
    // SAFETY: as above.
    let state = unsafe { object.get_accState(&child) }
        .ok()
        .and_then(|state| i32::try_from(&state).ok())
        .and_then(|state| u32::try_from(state).ok())
        .unwrap_or(0);
    let selected =
        (state & STATE_SYSTEM_SELECTABLE != 0).then_some(state & STATE_SYSTEM_SELECTED != 0);
    let position = list_view_position(window);
    tracing::info!(
        window = window.0 as isize,
        class = class_name(window),
        focus_type = focus.vt().0,
        name,
        ?position,
        ?selected,
        "the focused object, read through MSAA"
    );
    Ok(FocusedElement {
        name,
        position,
        selected,
    })
}

/// `window`'s client object (`AccessibleObjectFromWindow`).
fn client_of(window: HWND) -> io::Result<IAccessible> {
    let mut client: Option<IAccessible> = None;
    // SAFETY: `client` is a local the call fills with the window's client
    // object; the interface id matches its type.
    unsafe {
        AccessibleObjectFromWindow(
            window,
            OBJID_CLIENT.0.cast_unsigned(),
            &IAccessible::IID,
            (&raw mut client).cast(),
        )
    }
    .map_err(io::Error::other)?;
    client.ok_or_else(|| io::Error::other("the focus window has no client"))
}

/// `window`'s class name, empty when it cannot be read. Local.
fn class_name(window: HWND) -> String {
    let mut class = [0u16; 64];
    // SAFETY: `class` is a local buffer of the length passed.
    let length = unsafe { GetClassNameW(window, &mut class) };
    usize::try_from(length).map_or_else(
        |_| String::new(),
        |length| String::from_utf16_lossy(&class[..length]),
    )
}

/// The window with the keyboard focus of the thread in the foreground,
/// else its active window (`GetGUIThreadInfo`, local).
fn focused_window() -> Option<HWND> {
    let mut info = GUITHREADINFO {
        cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: `info` is a local with `cbSize` set, which the call fills in.
    unsafe { GetGUIThreadInfo(0, &raw mut info) }.ok()?;
    [info.hwndFocus, info.hwndActive]
        .into_iter()
        .find(|window| !window.is_invalid())
}

/// The focused item's position in the list view `window`, counting from
/// one, and how many items it has, from the list view's own messages
/// (`LVM_GETNEXTITEM` for the focused item, `LVM_GETITEMCOUNT`); `None`
/// when `window` is not a list view, has no focused item, or does not
/// answer within a second.
fn list_view_position(window: HWND) -> Option<(i32, i32)> {
    if class_name(window) != "SysListView32" {
        return None;
    }
    let ask = |message: u32, wparam: usize, lparam: isize| -> Option<isize> {
        let mut answer = 0usize;
        // SAFETY: the list view's messages take plain integers and answer
        // one; nothing points into this process.
        let sent = unsafe {
            SendMessageTimeoutW(
                window,
                message,
                WPARAM(wparam),
                LPARAM(lparam),
                SMTO_ABORTIFHUNG,
                1000,
                Some(&raw mut answer),
            )
        };
        (sent.0 != 0).then_some(answer.cast_signed())
    };
    let focused = ask(
        LVM_GETNEXTITEM,
        usize::MAX,
        isize::try_from(LVNI_FOCUSED).ok()?,
    )?;
    let count = ask(LVM_GETITEMCOUNT, 0, 0)?;
    let index = i32::try_from(focused).ok().filter(|index| *index >= 0)?;
    Some((index + 1, i32::try_from(count).ok()?))
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
    through_uia("MisspeltWords", misspelt_through_uia)
}

/// [`misspelt_words`]' UI Automation calls.
fn misspelt_through_uia() -> io::Result<Vec<String>> {
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
