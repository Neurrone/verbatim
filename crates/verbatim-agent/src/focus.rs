//! The focused object as MSAA reports it, for
//! [`Request::FocusedElement`](crate::protocol::Request::FocusedElement):
//! state a test reads independently of the screen reader under test, such
//! as which item of the desktop has the focus, to fix what it expects the
//! screen reader to say before it says it. Three requests need facts only
//! UI Automation has and keep it: [`focus_by_automation_id`],
//! [`controlled_items`], and [`name_by_automation_id`]
//! (`books/dev/src/explanation/design/listener-without-connections.md`, section 4.5).

use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, IDispatch,
};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::{
    AccessibleObjectFromWindow, CUIAutomation, IAccessible, IUIAutomation, IUIAutomationElement,
    IUIAutomationFocusChangedEventHandler, IUIAutomationTreeWalker, TreeScope_Children,
    TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_SizeOfSetPropertyId,
    WindowFromAccessibleObject,
};
use windows::Win32::UI::Controls::{LVM_GETITEMCOUNT, LVM_GETNEXTITEM, LVNI_FOCUSED};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, GUITHREADINFO, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo,
    OBJID_CLIENT, SMTO_ABORTIFHUNG, STATE_SYSTEM_SELECTABLE, STATE_SYSTEM_SELECTED,
    SendMessageTimeoutW,
};
use windows::core::{BSTR, Interface};

use crate::protocol::{FocusedElement, ListPosition, SwitcherItem};

/// How long [`list_view_position`] waits for the list view to answer each
/// of its messages.
const LIST_VIEW_LIMIT_MS: u32 = 1000;

/// Once [`focused_element`] has followed the focus into more windows than
/// this, it stops and reads the window it has reached.
const FOCUS_HOPS: u32 = 4;

/// `result`, a UI Automation lookup, with windows-core's answer for a null
/// result (an empty error, `S_OK`) read as `missing`, and any other error
/// reported with its HRESULT and message as `what` failing, so a failed
/// call is never taken for an element that is not there.
fn found<T>(
    result: windows::core::Result<T>,
    what: &str,
    missing: impl FnOnce() -> String,
) -> io::Result<T> {
    result.map_err(|error| {
        if error.code().0 == 0 {
            io::Error::other(missing())
        } else {
            io::Error::other(format!(
                "{what} failed with {:#010x}: {}",
                error.code().0,
                error.message()
            ))
        }
    })
}

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
    let (automation, element) = find_in_foreground(automation_id)?;
    // SAFETY: plain COM calls on live interfaces.
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

/// Of the first element the focused element's UI Automation
/// `ControllerFor` relation names, the size of the set its first child
/// reports (its `SizeOfSet` property, the count a screen reader says "of
/// N" from) and the names of its first `names` children.
///
/// # Errors
///
/// Returns an error if UI Automation cannot be reached, nothing has the
/// focus, the focused element controls nothing, or the controlled
/// element's first child reports no set size.
pub fn controlled_items(names: u32) -> io::Result<(u32, Vec<String>)> {
    through_uia("ControlledItems", || controlled_through_uia(names))
}

/// The first element of the foreground window whose UI Automation
/// identifier is `automation_id`, with the UI Automation client that found
/// it.
fn find_in_foreground(automation_id: &str) -> io::Result<(IUIAutomation, IUIAutomationElement)> {
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
    let element = found(
        unsafe { root.FindFirst(TreeScope_Descendants, &condition) },
        &format!("FindFirst for the identifier {automation_id:?}"),
        || format!("no element of the foreground window has the identifier {automation_id:?}"),
    )?;
    Ok((automation, element))
}

/// [`controlled_items`]' UI Automation calls.
fn controlled_through_uia(names: u32) -> io::Result<(u32, Vec<String>)> {
    // SAFETY: as in `focused_element`.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: as in `focused_element`.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(io::Error::other)?;
    // SAFETY: plain COM calls on live interfaces.
    let focus = unsafe { automation.GetFocusedElement() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let controlled = unsafe { focus.CurrentControllerFor() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let length = unsafe { controlled.Length() }.map_err(io::Error::other)?;
    if length == 0 {
        return Err(io::Error::other("the focused element controls nothing"));
    }
    // SAFETY: as above, with an index inside the array.
    let list = unsafe { controlled.GetElement(0) }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let any = unsafe { automation.CreateTrueCondition() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let children = unsafe { list.FindAll(TreeScope_Children, &any) }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let realized = unsafe { children.Length() }.map_err(io::Error::other)?;
    if realized == 0 {
        return Err(io::Error::other("the controlled element has no children"));
    }
    // SAFETY: as above, with an index inside the array.
    let first = unsafe { children.GetElement(0) }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let size = unsafe { first.GetCurrentPropertyValue(UIA_SizeOfSetPropertyId) }
        .map_err(io::Error::other)?;
    let count = i32::try_from(&size)
        .ok()
        .and_then(|size| u32::try_from(size).ok())
        .filter(|&size| size > 0)
        .ok_or_else(|| {
            io::Error::other("the controlled element's first child reports no set size")
        })?;
    let shown = realized.min(i32::try_from(names).map_err(io::Error::other)?);
    let mut read = Vec::new();
    for index in 0..shown {
        // SAFETY: as above, with an index inside the array.
        let child = unsafe { children.GetElement(index) }.map_err(io::Error::other)?;
        // SAFETY: as above.
        let name = unsafe { child.CurrentName() }.map_err(io::Error::other)?;
        read.push(name.to_string());
    }
    Ok((count, read))
}

/// The name of the first element of the foreground window whose UI
/// Automation identifier is `automation_id`, read without focusing it.
///
/// # Errors
///
/// Returns an error if UI Automation cannot be reached, there is no
/// foreground window, or no such element is found.
pub fn name_by_automation_id(automation_id: &str) -> io::Result<String> {
    through_uia("NameByAutomationId", || {
        let (_, element) = find_in_foreground(automation_id)?;
        // SAFETY: a plain COM call on a live interface.
        let name = unsafe { element.CurrentName() }.map_err(io::Error::other)?;
        Ok(name.to_string())
    })
}

/// How long one of the agent's UI Automation requests may take before it
/// is logged as slow: UIA's default connection timeout, which these
/// requests' client leaves as it is.
const SLOW_UIA_REQUEST: std::time::Duration = std::time::Duration::from_secs(2);

/// Runs `read`, one of the requests that keep UI Automation, and logs
/// how long it took, at warn when it took [`SLOW_UIA_REQUEST`] or longer,
/// so the scenarios that use them can count the agent's slow answers apart
/// from Verbatim's (`books/dev/src/explanation/design/listener-without-connections.md`, section
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
/// through MSAA (`books/dev/src/explanation/design/listener-without-connections.md`, decision
/// 10): the window with the keyboard focus of the thread in the foreground,
/// its client object (`AccessibleObjectFromWindow`), and the child its
/// `accFocus` names, followed into the window of an object it names. The desktop's list, which every scenario reads at its
/// start, is a comctl32 list view with no UIA provider, so MSAA reads it
/// as Verbatim does, and no UIA connection is made into File Explorer.
///
/// # Errors
///
/// Returns an error if no window has the keyboard focus, or its accessible
/// object, the focused object's name, or its state cannot be read: a
/// failed read is never taken for an empty name or no state.
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
    let mut hop_limit = false;
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
        if found.is_err() || inner.is_invalid() || inner == window {
            break (object, CHILDID_SELF.cast_signed(), focus);
        }
        // The object is in `inner`, which its position is read from too.
        window = inner;
        if hops > FOCUS_HOPS {
            hop_limit = true;
            break (object, CHILDID_SELF.cast_signed(), focus);
        }
    };
    let child = VARIANT::from(child);
    // SAFETY: as above.
    let name = unsafe { object.get_accName(&child) }
        .map(|name| name.to_string())
        .map_err(|error| {
            io::Error::other(format!(
                "the focused object's name could not be read (get_accName, in a window of class {:?}): {error}",
                class_name(window)
            ))
        })?;
    // SAFETY: as above.
    let state = unsafe { object.get_accState(&child) }.map_err(|error| {
        io::Error::other(format!(
            "the focused object's state could not be read (get_accState, in a window of class {:?}): {error}",
            class_name(window)
        ))
    })?;
    let state = i32::try_from(&state)
        .ok()
        .and_then(|state| u32::try_from(state).ok())
        .ok_or_else(|| {
            io::Error::other(format!(
                "the focused object's state (get_accState) is not a number: {state:?}"
            ))
        })?;
    let selected =
        (state & STATE_SYSTEM_SELECTABLE != 0).then_some(state & STATE_SYSTEM_SELECTED != 0);
    let position = list_view_position(window, hop_limit);
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
/// (`LVM_GETNEXTITEM` for the focused item, `LVM_GETITEMCOUNT`), or why
/// there is none: `window` is not a list view (`hop_limit` saying the
/// focus was followed as far as the agent follows it), has no focused
/// item, or does not answer within [`LIST_VIEW_LIMIT_MS`].
fn list_view_position(window: HWND, hop_limit: bool) -> ListPosition {
    let class = class_name(window);
    if class != "SysListView32" {
        return ListPosition::NotListView { class, hop_limit };
    }
    let ask = |message: u32, name: &str, wparam: usize, lparam: isize| {
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
                LIST_VIEW_LIMIT_MS,
                Some(&raw mut answer),
            )
        };
        if sent.0 == 0 {
            Err(ListPosition::NotAnswering {
                message: name.to_owned(),
                limit_ms: u64::from(LIST_VIEW_LIMIT_MS),
                error: windows::core::Error::from_thread().message(),
            })
        } else {
            Ok(answer.cast_signed())
        }
    };
    let read = || -> Result<ListPosition, ListPosition> {
        let flags = isize::try_from(LVNI_FOCUSED).unwrap_or_default();
        let focused = ask(LVM_GETNEXTITEM, "LVM_GETNEXTITEM", usize::MAX, flags)?;
        let Some(index) = i32::try_from(focused).ok().filter(|index| *index >= 0) else {
            return Ok(ListPosition::NoFocusedItem);
        };
        let count = ask(LVM_GETITEMCOUNT, "LVM_GETITEMCOUNT", 0, 0)?;
        Ok(ListPosition::At {
            position: index + 1,
            count: i32::try_from(count).unwrap_or(i32::MAX),
        })
    };
    read().unwrap_or_else(|failure| failure)
}

/// The screen position of the character on line `line` at `column`, both
/// from zero, of the focused Win32 edit control, as the control gives it:
/// the line's first character's index (`EM_LINEINDEX`), the position of
/// the character `column` past it (`EM_POSFROMCHAR`, packed in its answer
/// as a classic edit control packs it), converted to screen coordinates.
///
/// # Errors
///
/// Returns an error if no window has the focus, it does not answer, or the
/// character is not in its client area.
pub fn edit_character_position(line: u32, column: u32) -> io::Result<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::Controls::{EM_LINEINDEX, EM_POSFROMCHAR};

    let window = focused_window().ok_or_else(|| io::Error::other("no window has the focus"))?;
    let ask = |message: u32, wparam: usize| -> io::Result<usize> {
        let mut answer = 0usize;
        // SAFETY: both messages take plain integers and answer one; nothing
        // points into this process.
        let sent = unsafe {
            SendMessageTimeoutW(
                window,
                message,
                WPARAM(wparam),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                1000,
                Some(&raw mut answer),
            )
        };
        if sent.0 == 0 {
            return Err(io::Error::other(format!(
                "the focused window ({}) did not answer message {message:#x}",
                class_name(window)
            )));
        }
        Ok(answer)
    };
    let start = ask(EM_LINEINDEX, line as usize)?;
    if start.cast_signed() < 0 {
        return Err(io::Error::other(format!(
            "the edit control has no line {line}"
        )));
    }
    let packed = ask(EM_POSFROMCHAR, start + column as usize)?;
    if packed.cast_signed() == -1 {
        return Err(io::Error::other(format!(
            "the character at line {line}, column {column} has no position"
        )));
    }
    // The client coordinates, each a signed 16-bit value in one of the
    // answer's two low words.
    let low = u16::try_from(packed & 0xFFFF).unwrap_or(0);
    let high = u16::try_from((packed >> 16) & 0xFFFF).unwrap_or(0);
    let mut point = POINT {
        x: i32::from(low.cast_signed()),
        y: i32::from(high.cast_signed()),
    };
    // SAFETY: a local conversion of a local point.
    if !unsafe { ClientToScreen(window, &raw mut point) }.as_bool() {
        return Err(io::Error::other(
            "the point could not be converted to the screen",
        ));
    }
    Ok((point.x, point.y))
}

/// The screen position of the character on line `line` at `column`, both
/// from zero, of the focused UI Automation element's text: a range at the
/// text's start moved by `line` lines and then by `column` characters,
/// expanded to its character, and the top left of its first bounding
/// rectangle, each coordinate rounded.
///
/// # Errors
///
/// Returns an error if UI Automation cannot be reached, the focus has no
/// text pattern, the move falls short, or the character has no rectangle.
pub fn uia_character_position(line: u32, column: u32) -> io::Result<(i32, i32)> {
    through_uia("UiaCharacterPosition", || {
        uia_position_through_uia(line, column)
    })
}

/// [`uia_character_position`]'s UI Automation calls.
fn uia_position_through_uia(line: u32, column: u32) -> io::Result<(i32, i32)> {
    use windows::Win32::System::Ole::{
        SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetUBound, SafeArrayUnaccessData,
    };
    use windows::Win32::UI::Accessibility::{
        IUIAutomationTextPattern, TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start,
        TextUnit_Character, TextUnit_Line, UIA_TextPatternId,
    };

    // SAFETY: as in `focused_element`.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: as in `focused_element`.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(io::Error::other)?;
    // SAFETY: plain COM calls on live interfaces.
    let focus = unsafe { automation.GetFocusedElement() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let pattern: IUIAutomationTextPattern = found(
        unsafe { focus.GetCurrentPatternAs(UIA_TextPatternId) },
        "GetCurrentPatternAs for the text pattern",
        || "the focused element has no text pattern".to_owned(),
    )?;
    // SAFETY: as above.
    let range = unsafe { pattern.DocumentRange() }.map_err(io::Error::other)?;
    // SAFETY: as above; the range is collapsed to its start.
    unsafe {
        range.MoveEndpointByRange(
            TextPatternRangeEndpoint_End,
            &range,
            TextPatternRangeEndpoint_Start,
        )
    }
    .map_err(io::Error::other)?;
    for (unit, count, what) in [
        (TextUnit_Line, line, "lines"),
        (TextUnit_Character, column, "characters"),
    ] {
        let count = i32::try_from(count).map_err(io::Error::other)?;
        if count == 0 {
            continue;
        }
        // SAFETY: as above.
        let moved = unsafe { range.Move(unit, count) }.map_err(io::Error::other)?;
        if moved != count {
            return Err(io::Error::other(format!(
                "the text moved {moved} of {count} {what}"
            )));
        }
    }
    // SAFETY: as above.
    unsafe { range.ExpandToEnclosingUnit(TextUnit_Character) }.map_err(io::Error::other)?;
    // SAFETY: as above; the array returned is this call's to destroy.
    let array = unsafe { range.GetBoundingRectangles() }.map_err(io::Error::other)?;
    let rectangle = (|| {
        // SAFETY: a live one-dimensional array of doubles.
        let upper = unsafe { SafeArrayGetUBound(array, 1) }.map_err(io::Error::other)?;
        if upper < 1 {
            return Err(io::Error::other("the character has no bounding rectangle"));
        }
        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: as above; `data` is a local the call fills.
        unsafe { SafeArrayAccessData(array, &raw mut data) }.map_err(io::Error::other)?;
        // SAFETY: the array holds at least two doubles, accessed until
        // unaccessed below.
        let pair = unsafe { std::slice::from_raw_parts(data.cast::<f64>(), 2) };
        let (left, top) = (pair[0], pair[1]);
        // SAFETY: accessed above.
        unsafe { SafeArrayUnaccessData(array) }.map_err(io::Error::other)?;
        Ok((left, top))
    })();
    // SAFETY: the array is this call's, and no longer accessed.
    let _ = unsafe { SafeArrayDestroy(array) };
    let (left, top) = rectangle?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a rounded screen coordinate, saturating if out of range"
    )]
    Ok((left.round() as i32, top.round() as i32))
}

/// Every item of the Alt+Tab switcher's list, read through UI Automation
/// while the switcher is open, in the list's order: its name, its grid row
/// and column, each from one, its position in the list, from one, and the
/// list's size. The switcher's window is in front before its list exists,
/// so the read first waits up to `timeout`, on UI Automation's focus
/// events ([`until_found`]), for the keyboard focus to be an item of the
/// list, an element whose raw-view parent has the automation id
/// `SwitchItemListControl`: the focus Verbatim says the item from. The
/// items, that parent's children, are then read one property at a time:
/// the agent is the harness's own reader, apart from Verbatim's shapes
/// (`books/dev/src/explanation/design/uia-shapes.md`).
///
/// # Errors
///
/// Returns an error if UI Automation cannot be reached, no item of the
/// switcher's list has the focus within `timeout`, or an item reports no
/// row, column or position.
pub fn switcher_items(timeout: Duration) -> io::Result<Vec<SwitcherItem>> {
    through_uia("SwitcherItems", || switcher_items_through_uia(timeout))
}

/// Waits up to `timeout` for `read` to find what it looks for: reads once
/// now and again after each wake on `wakes`, and returns what it found, or
/// `None` once `timeout` has passed with no wake that found it. The waker
/// is installed before the call, so a change before the first read is
/// seen by that read and one after it wakes the wait. An error from `read`
/// ends the wait with that error.
fn until_found<T>(
    mut read: impl FnMut() -> io::Result<Option<T>>,
    wakes: &mpsc::Receiver<()>,
    timeout: Duration,
) -> io::Result<Option<T>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(found) = read()? {
            return Ok(Some(found));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match wakes.recv_timeout(left) {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("the focus event handler is gone"));
            }
        }
    }
}

/// The UI Automation focus-changed handler [`switcher_items`] waits on:
/// each focus event sends one wake, and the waiting thread reads the focus
/// itself, so no element crosses threads.
mod focus_wake {
    // The `implement` macro's expansion.
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use std::sync::mpsc::Sender;

    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationFocusChangedEventHandler,
        IUIAutomationFocusChangedEventHandler_Impl,
    };

    /// A handler that sends a wake to `sender` at each focus event.
    pub(super) fn new(sender: Sender<()>) -> IUIAutomationFocusChangedEventHandler {
        Waking { sender }.into()
    }

    #[windows_core::implement(IUIAutomationFocusChangedEventHandler)]
    struct Waking {
        sender: Sender<()>,
    }

    impl IUIAutomationFocusChangedEventHandler_Impl for Waking_Impl {
        fn HandleFocusChangedEvent(
            &self,
            _sender: windows_core::Ref<IUIAutomationElement>,
        ) -> windows_core::Result<()> {
            // The receiver is dropped only after the handler is removed,
            // so a failed send is an event already past the wait.
            let _ = self.sender.send(());
            Ok(())
        }
    }
}

/// A registered focus-changed handler, removed on drop.
struct FocusHandler<'a> {
    automation: &'a IUIAutomation,
    handler: IUIAutomationFocusChangedEventHandler,
}

impl Drop for FocusHandler<'_> {
    fn drop(&mut self) {
        // SAFETY: a plain COM call removing the handler this value added.
        if let Err(error) = unsafe {
            self.automation
                .RemoveFocusChangedEventHandler(&self.handler)
        } {
            tracing::warn!(%error, "removing the switcher's focus handler failed");
        }
    }
}

/// `result`, a UI Automation read that answers a null element when there
/// is none, as `None` then, and any other error reported with its HRESULT
/// and message as `what` failing.
fn present<T>(result: windows::core::Result<T>, what: &str) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code().0 == 0 => Ok(None),
        Err(error) => Err(io::Error::other(format!(
            "{what} failed with {:#010x}: {}",
            error.code().0,
            error.message()
        ))),
    }
}

/// The switcher's list, when the item with the keyboard focus has it as
/// its raw-view parent, found by its automation id
/// `SwitchItemListControl`; otherwise `None`, with `seen` set to what has
/// the focus, for the error if the wait runs out.
fn focused_switcher_list(
    automation: &IUIAutomation,
    walker: &IUIAutomationTreeWalker,
    seen: &mut String,
) -> io::Result<Option<IUIAutomationElement>> {
    // SAFETY: plain COM calls on live interfaces.
    let Some(focus) = present(
        unsafe { automation.GetFocusedElement() },
        "GetFocusedElement",
    )?
    else {
        "nothing".clone_into(seen);
        return Ok(None);
    };
    // SAFETY: as above.
    let name = unsafe { focus.CurrentName() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let parent = unsafe { walker.GetParentElement(&focus) };
    let Some(parent) = present(parent, "GetParentElement of the focus")? else {
        *seen = format!("{name:?}, with no parent");
        return Ok(None);
    };
    // SAFETY: as above.
    let parent_id = unsafe { parent.CurrentAutomationId() }.map_err(io::Error::other)?;
    if parent_id == "SwitchItemListControl" {
        return Ok(Some(parent));
    }
    *seen = format!("{name:?}, whose parent's automation id is {parent_id:?}");
    Ok(None)
}

/// [`switcher_items`]'s UI Automation calls.
fn switcher_items_through_uia(timeout: Duration) -> io::Result<Vec<SwitcherItem>> {
    use windows::Win32::UI::Accessibility::{
        UIA_GridItemColumnPropertyId, UIA_GridItemRowPropertyId,
        UIA_IsGridItemPatternAvailablePropertyId, UIA_PositionInSetPropertyId,
    };

    // SAFETY: as in `focused_element`.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: as in `focused_element`.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(io::Error::other)?;
    // SAFETY: plain COM calls on live interfaces.
    let walker = unsafe { automation.RawViewWalker() }.map_err(io::Error::other)?;
    let (sender, wakes) = mpsc::channel();
    let handler = focus_wake::new(sender);
    // SAFETY: as above.
    unsafe { automation.AddFocusChangedEventHandler(None, &handler) }.map_err(io::Error::other)?;
    let registered = FocusHandler {
        automation: &automation,
        handler,
    };
    let mut seen = String::new();
    let list = until_found(
        || focused_switcher_list(&automation, &walker, &mut seen),
        &wakes,
        timeout,
    );
    drop(registered);
    let list = list?.ok_or_else(|| {
        io::Error::other(format!(
            "no item of the switcher's list of running applications had the focus within \
             {timeout:?}; the focus was {seen}"
        ))
    })?;
    // SAFETY: as above.
    let every = unsafe { automation.CreateTrueCondition() }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let found = unsafe { list.FindAll(TreeScope_Children, &every) }.map_err(io::Error::other)?;
    // SAFETY: as above.
    let length = unsafe { found.Length() }.map_err(io::Error::other)?;
    let mut items = Vec::new();
    for index in 0..length {
        // SAFETY: as above, with an index below the array's length.
        let item = unsafe { found.GetElement(index) }.map_err(io::Error::other)?;
        let number = |property| -> io::Result<i32> {
            // SAFETY: as above.
            let value =
                unsafe { item.GetCurrentPropertyValue(property) }.map_err(io::Error::other)?;
            i32::try_from(&value).map_err(io::Error::other)
        };
        // SAFETY: as above.
        let name = unsafe { item.CurrentName() }.map_err(io::Error::other)?;
        let name = name.to_string();
        // SAFETY: as above.
        let grid =
            unsafe { item.GetCurrentPropertyValue(UIA_IsGridItemPatternAvailablePropertyId) }
                .map_err(io::Error::other)?;
        if !bool::try_from(&grid).unwrap_or(false) {
            return Err(io::Error::other(format!(
                "the switcher's item {name:?} has no grid position"
            )));
        }
        let from_one = |value: i32| {
            u32::try_from(value + 1).map_err(|_| {
                io::Error::other(format!("the switcher's item {name:?} has no grid position"))
            })
        };
        let counted = |value: i32| {
            u32::try_from(value)
                .ok()
                .filter(|&value| value > 0)
                .ok_or_else(|| {
                    io::Error::other(format!(
                        "the switcher's item {name:?} reports no position in its list"
                    ))
                })
        };
        items.push(SwitcherItem {
            row: from_one(number(UIA_GridItemRowPropertyId)?)?,
            column: from_one(number(UIA_GridItemColumnPropertyId)?)?,
            position: counted(number(UIA_PositionInSetPropertyId)?)?,
            set_size: counted(number(UIA_SizeOfSetPropertyId)?)?,
            name,
        });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_whose_first_read_finds_needs_no_wake() {
        let (_sender, wakes) = mpsc::channel();
        let mut reads = 0;
        let found = until_found(
            || {
                reads += 1;
                Ok(Some("the list"))
            },
            &wakes,
            Duration::ZERO,
        )
        .expect("the wait");
        assert_eq!((found, reads), (Some("the list"), 1));
    }

    #[test]
    fn a_wait_reads_again_at_each_wake_until_one_finds() {
        let (sender, wakes) = mpsc::channel();
        for _ in 0..3 {
            sender.send(()).expect("a wake");
        }
        let mut reads = 0;
        let found = until_found(
            || {
                reads += 1;
                Ok((reads == 3).then_some("the list"))
            },
            &wakes,
            Duration::from_secs(30),
        )
        .expect("the wait");
        assert_eq!((found, reads), (Some("the list"), 3));
        assert_eq!(wakes.try_recv(), Ok(()), "the third wake is left unread");
    }

    #[test]
    fn a_wait_with_no_wake_that_finds_ends_at_its_timeout() {
        let (sender, wakes) = mpsc::channel();
        sender.send(()).expect("a wake");
        let mut reads = 0;
        let found = until_found(
            || {
                reads += 1;
                Ok(None::<&str>)
            },
            &wakes,
            Duration::from_millis(1),
        )
        .expect("the wait");
        assert_eq!((found, reads), (None, 2));
    }

    #[test]
    fn a_failed_read_ends_the_wait_with_its_error() {
        let (sender, wakes) = mpsc::channel();
        sender.send(()).expect("a wake");
        let mut reads = 0;
        let error = until_found(
            || {
                reads += 1;
                if reads == 2 {
                    Err(io::Error::other("the read failed"))
                } else {
                    Ok(None::<&str>)
                }
            },
            &wakes,
            Duration::from_secs(30),
        )
        .expect_err("the read's error");
        assert_eq!(
            (error.to_string(), reads),
            ("the read failed".to_owned(), 2)
        );
    }
}
