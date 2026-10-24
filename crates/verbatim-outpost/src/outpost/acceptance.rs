//! The outpost's side of the acceptance test, whose rules are defined once
//! in `verbatim_core::focus_rules` (decision D14, "Where the rules run";
//! `docs/parity/focus-events.md`, "Event acceptance"): the name a
//! `WinEvent` is judged by, and the window queries, every one a local call,
//! so the test never waits on the application. The outpost's worker makes
//! the test for every event but a UIA focus, which the focus listener's
//! reading thread makes before reading the focus (`crate::focus_read`).
//!
//! The test is made per event as it is handled, against the foreground
//! window of that moment, not in the event callback, because filtering
//! there lost the focus events of applications starting up before the
//! foreground had changed (NVDA issue 4001). The rule NVDA makes by calling
//! into an application, for a windowless Chromium document under a Chrome
//! embedding window, is not made: Verbatim has no Chromium support yet.

use std::ffi::c_void;

use verbatim_core::focus_rules::{EventName, WindowQueries};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GA_PARENT, GA_ROOT, GA_ROOTOWNER, GET_ANCESTOR_FLAGS, GUITHREADINFO, GWL_EXSTYLE, GetAncestor,
    GetDesktopWindow, GetGUIThreadInfo, GetWindowLongW, IsChild, WS_EX_TOPMOST,
};

use verbatim_ia2::WinEventKind;

use crate::arbitration::window_class_name;

/// The name a `WinEvent` of `kind` is judged by, or `None` for a kind
/// handled outside the test: an object destroyed, which NVDA handles in its
/// event callback, and Verbatim's own kinds, the text selection of edit
/// controls, the console host's updates and caret, and the shell's coming
/// to the front, which only the listener takes
/// (`docs/parity/focus-events.md`, "Event acceptance").
pub(crate) fn event_name_of(kind: WinEventKind) -> Option<EventName> {
    Some(match kind {
        WinEventKind::Focus | WinEventKind::Foreground => EventName::GainFocus,
        WinEventKind::MenuPopupStart => EventName::MenuStart,
        WinEventKind::MenuEnd => EventName::MenuEnd,
        WinEventKind::SwitchEnd => EventName::SwitchEnd,
        WinEventKind::Show => EventName::Show,
        WinEventKind::Alert => EventName::Alert,
        WinEventKind::ValueChange => EventName::ValueChange,
        WinEventKind::StateChange => EventName::StateChange,
        WinEventKind::NameChange => EventName::NameChange,
        WinEventKind::DescriptionChange => EventName::DescriptionChange,
        WinEventKind::Selection => EventName::Selection,
        WinEventKind::Caret => EventName::LocationChange,
        WinEventKind::Destroy
        | WinEventKind::TextSelectionChange
        | WinEventKind::ConsoleUpdate
        | WinEventKind::ConsoleCaret
        | WinEventKind::ShellForeground => return None,
    })
}

/// The system's windows, read with local calls, with the foreground window
/// given: the outpost's foreground reader's, or the system's.
pub(crate) struct SystemWindows {
    pub(crate) foreground: isize,
}

fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut c_void)
}

fn ancestor(window: isize, flags: GET_ANCESTOR_FLAGS) -> isize {
    // SAFETY: GetAncestor tolerates any handle, returning null for an
    // invalid one.
    unsafe { GetAncestor(hwnd(window), flags) }.0 as isize
}

impl WindowQueries for SystemWindows {
    fn class_name(&self, window: isize) -> String {
        window_class_name(window)
    }

    fn process(&self, window: isize) -> u32 {
        super::window::window_owner(window).1
    }

    fn parent(&self, window: isize) -> isize {
        ancestor(window, GA_PARENT)
    }

    fn root(&self, window: isize) -> isize {
        ancestor(window, GA_ROOT)
    }

    fn root_owner(&self, window: isize) -> isize {
        ancestor(window, GA_ROOTOWNER)
    }

    fn is_child(&self, parent: isize, child: isize) -> bool {
        // SAFETY: IsChild tolerates any pair of handles.
        unsafe { IsChild(hwnd(parent), hwnd(child)) }.as_bool()
    }

    fn is_topmost(&self, window: isize) -> bool {
        // SAFETY: GetWindowLongW reads a window's style word; an invalid
        // handle yields 0.
        let style = unsafe { GetWindowLongW(hwnd(window), GWL_EXSTYLE) };
        style.cast_unsigned() & WS_EX_TOPMOST.0 != 0
    }

    fn desktop(&self) -> isize {
        // SAFETY: GetDesktopWindow has no preconditions.
        unsafe { GetDesktopWindow() }.0 as isize
    }

    fn active_window(&self) -> isize {
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: `info` has cbSize set before the call, which fails safely.
        if unsafe { GetGUIThreadInfo(0, &raw mut info) }.is_err() {
            return 0;
        }
        info.hwndActive.0 as isize
    }

    fn foreground(&self) -> isize {
        self.foreground
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_carets_event_is_tested_as_a_location_change() {
        assert_eq!(
            event_name_of(WinEventKind::Caret),
            Some(EventName::LocationChange)
        );
    }
}
