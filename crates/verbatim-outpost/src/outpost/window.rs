//! Window queries only an outpost makes, with local calls only: none of
//! them sends the window a message or calls into another process, so they
//! are safe on any thread and cannot block on a hung application. The
//! queries other roles make too are `verbatim_win32::window`'s.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_PARENT, GA_ROOT, GA_ROOTOWNER, GWL_STYLE, GetAncestor, GetForegroundWindow,
    GetWindowLongPtrW, InternalGetWindowText, WS_DISABLED, WS_SYSMENU,
};

use verbatim_model::{Pid, WindowFacts, WindowHandle};

use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_win32::window::{
    served_by, top_level_of, top_level_windows, window_class_name, window_is_hidden_frame,
    window_is_visible, window_owner,
};

/// Milliseconds since the Unix epoch on the run's shared clock
/// ([`verbatim_process::clock::now_us`]), the observation timestamp that orders
/// events across processes and anchors the keypress-to-audio latency
/// timeline. Shared with the listener, which stamps facts at observation.
pub(crate) fn now_ms() -> u64 {
    verbatim_process::clock::now_us() / 1_000
}

/// The observation `timing` holds, in microseconds on the run's shared
/// clock; now, when `timing` holds none.
pub(crate) fn observed_us(timing: &verbatim_model::protocol::EventTiming) -> u64 {
    match timing.observed_at_us {
        0 => verbatim_process::clock::now_us(),
        observed_at_us => observed_at_us,
    }
}

fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut c_void)
}

/// The application an event in `handle` belongs to when it is not
/// `target_pid`, the outpost's target: for a window in a console window,
/// which only the console host's outpost reports, the console's client,
/// which Windows names as the console window's owner, as NVDA gives a
/// console window and its text area the client's app module
/// (`UIA._get_processID`, `NVDAObjects/UIA/__init__.py` 2336 to 2346;
/// `appModuleHandler.getAppModuleForNVDAObject`, `appModuleHandler.py` 227
/// to 238). `None` for every other window.
pub(super) fn console_client(handle: isize, target_pid: u32) -> Option<Pid> {
    let top = top_level_of(handle);
    client_of(&window_class_name(top), window_owner(top).1, target_pid).map(Pid)
}

/// The client of a top-level window of class `class` that
/// `GetWindowThreadProcessId` says `owner` owns, reported by `target_pid`'s
/// outpost: the owner, for a console window that is not the target's own.
fn client_of(class: &str, owner: u32, target_pid: u32) -> Option<u32> {
    (class == CONSOLE_WINDOW_CLASS && owner != 0 && owner != target_pid).then_some(owner)
}

/// The class name of `handle`'s parent window, or `None` when it has none.
pub(super) fn parent_class(handle: isize) -> Option<String> {
    // SAFETY: GetAncestor tolerates any handle, returning null for an invalid
    // one.
    let parent = unsafe { GetAncestor(hwnd(handle), GA_PARENT) };
    (!parent.0.is_null()).then(|| window_class_name(parent.0 as isize))
}

/// `handle` as the model's opaque window handle.
pub(super) fn window_handle(handle: isize) -> WindowHandle {
    WindowHandle(u64::from_ne_bytes(handle.to_ne_bytes()))
}

/// The window the model's `handle` names, as [`window_handle`] made it.
pub(super) fn hwnd_of(handle: WindowHandle) -> isize {
    isize::from_ne_bytes(handle.0.to_ne_bytes())
}

/// Facts about `handle`'s window for the reducer's attention classification
/// (`docs/parity/focus-events.md`, "Event acceptance"): its top-level window, the top of
/// its owner chain, whether it or its top-level window is topmost, and, for a
/// `Windows.UI.Core` window only, whether it is the input thread's active
/// window or inside it — NVDA's test for UWP windows — whether it is in
/// the system's foreground window right now, and its class, which Core's
/// rules for an application's windows match (decision D6).
pub(super) fn window_facts(handle: isize) -> WindowFacts {
    let window = hwnd(handle);
    // SAFETY: GetAncestor tolerates any handle, returning null for an
    // invalid one.
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    // SAFETY: as above.
    let root_owner = unsafe { GetAncestor(window, GA_ROOTOWNER) };
    let root = if root.0.is_null() { window } else { root };
    let root_owner = if root_owner.0.is_null() {
        root
    } else {
        root_owner
    };
    let class = window_class_name(handle);
    // How the window stands to the foreground window, by the acceptance
    // test's own rule (`acceptance::foreground_relation`), against the system's
    // foreground window now.
    let relation = verbatim_reader_rules::acceptance::foreground_relation(
        handle,
        &class,
        &verbatim_win32::window::SystemWindows {
            foreground: verbatim_win32::window::foreground_window_handle(),
        },
    );
    WindowFacts {
        top_level: window_handle(root.0 as isize),
        root_owner: window_handle(root_owner.0 as isize),
        topmost: relation.topmost,
        under_active_window: relation.under_active_window,
        in_foreground: relation.in_foreground,
        class,
        application: String::new(),
        alt_held: false,
    }
}

/// Whether Alt is held now, by the system's asynchronous key state, as
/// NVDA's File Explorer module reads it when it judges a task switching
/// list item's container (`appModules/explorer.py`,
/// `MultitaskingViewFrameListItem._get_container`): a local call.
pub(super) fn alt_is_held() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU};
    // SAFETY: GetAsyncKeyState takes any virtual-key code.
    let state = unsafe { GetAsyncKeyState(i32::from(VK_MENU.0)) };
    // The high bit, the sign, says the key is down.
    state < 0
}

/// What window `handle`'s style says of it as a window read from Windows'
/// own state, from one local read of the style.
#[derive(Clone, Copy)]
pub(super) struct WindowStyle {
    /// The style disables the window (`WS_DISABLED`), unavailable as a
    /// plain window (`NVDAObjects/window/__init__.py`, `Window._get_states`).
    pub(super) disabled: bool,
    /// The window has a system menu (`WS_SYSMENU`), which makes its MSAA
    /// client a top-level window's, presented by its title
    /// (`NVDAObjects/IAccessible/__init__.py` 2095 to 2106).
    pub(super) system_menu: bool,
}

/// Window `handle`'s [`WindowStyle`]. A local call.
pub(super) fn window_style(handle: isize) -> WindowStyle {
    // SAFETY: GetWindowLongPtrW tolerates any handle, answering 0 for an
    // invalid one.
    let style = unsafe { GetWindowLongPtrW(hwnd(handle), GWL_STYLE) }.cast_unsigned();
    let has = |bit: u32| style & usize::try_from(bit).unwrap_or(0) != 0;
    WindowStyle {
        disabled: has(WS_DISABLED.0),
        system_menu: has(WS_SYSMENU.0),
    }
}

/// `handle`'s window text, read with `InternalGetWindowText`, which never
/// sends the window a message. `None` when the window has no text.
pub(super) fn window_text(handle: isize) -> Option<String> {
    let mut buffer = [0u16; 512];
    // SAFETY: the buffer outlives the call, which writes at most its length.
    let length = unsafe { InternalGetWindowText(hwnd(handle), &mut buffer) };
    let length = usize::try_from(length).ok()?;
    let text = String::from_utf16_lossy(&buffer[..length.min(buffer.len())]);
    (!text.trim().is_empty()).then_some(text)
}

/// `GetForegroundWindow()` if it is `target_pid`'s ([`served_by`]), else
/// `None`. Always a genuine top-level window when it returns `Some`.
pub(super) fn foreground_window_of(target_pid: u32) -> Option<isize> {
    // SAFETY: GetForegroundWindow has no preconditions.
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return None;
    }
    served_by(window.0 as isize, target_pid).then_some(window.0 as isize)
}

/// The target's top-level window for a tree dump: its foreground window,
/// else its first visible top-level window, skipping Core's hidden frame.
pub(super) fn main_window_of(target_pid: u32) -> Option<isize> {
    foreground_window_of(target_pid)
        .filter(|&window| !window_is_hidden_frame(window))
        .or_else(|| {
            top_level_windows(target_pid)
                .into_iter()
                .find(|&window| !window_is_hidden_frame(window) && window_is_visible(window))
        })
}

/// Waits, for as long as it takes, for process `pid` to exit, and says
/// whether it did: true once it has exited, or when no process has that
/// id (it exited before the wait began); false when the process cannot be
/// waited on, such as one running at a higher integrity level.
pub(crate) fn wait_for_process_exit(pid: u32) -> bool {
    use windows::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: a wait-only open of a plain process id; the handle is closed
    // below.
    let process = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(process) => process,
        Err(error) => return error.code() == ERROR_INVALID_PARAMETER.to_hresult(),
    };
    // SAFETY: the handle opened above, with SYNCHRONIZE access.
    let waited = unsafe { WaitForSingleObject(process, INFINITE) };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(process) };
    waited == WAIT_OBJECT_0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An event the outpost's own handlers took in is stamped with the
    /// microseconds of its observation, its own clock read.
    #[test]
    fn an_observation_is_stamped_from_its_own_clock_read() {
        let timing = verbatim_model::protocol::EventTiming {
            observed_at_us: 6_543_210,
            ..verbatim_model::protocol::EventTiming::default()
        };
        assert_eq!(observed_us(&timing), 6_543_210);
    }
    #[test]
    fn an_event_in_a_console_window_belongs_to_the_consoles_client() {
        // The console host's outpost, 30, reports the window Windows says
        // its client, 20, owns.
        assert_eq!(client_of(CONSOLE_WINDOW_CLASS, 20, 30), Some(20));
    }

    #[test]
    fn an_event_in_any_other_window_belongs_to_the_target() {
        assert_eq!(client_of("Notepad", 20, 30), None);
        assert_eq!(
            client_of(CONSOLE_WINDOW_CLASS, 30, 30),
            None,
            "a console window that is the target's own"
        );
        assert_eq!(
            client_of(CONSOLE_WINDOW_CLASS, 0, 30),
            None,
            "a window whose owner cannot be told"
        );
    }
}
