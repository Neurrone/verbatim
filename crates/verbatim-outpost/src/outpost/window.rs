//! Window queries only an outpost makes, with local calls only: none of
//! them sends the window a message or calls into another process, so they
//! are safe on any thread and cannot block on a hung application. The
//! queries other roles make too are `verbatim_win32::window`'s.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetForegroundWindow, GetWindowLongPtrW, WS_DISABLED, WS_SYSMENU,
};

use verbatim_model::WindowHandle;

use verbatim_win32::window::served_by;

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

/// `handle` as the model's opaque window handle.
pub(super) fn window_handle(handle: isize) -> WindowHandle {
    WindowHandle(u64::from_ne_bytes(handle.to_ne_bytes()))
}

/// The window the model's `handle` names, as [`window_handle`] made it.
pub(super) fn hwnd_of(handle: WindowHandle) -> isize {
    isize::from_ne_bytes(handle.0.to_ne_bytes())
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

pub(super) use verbatim_win32::window::window_text;

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

pub(super) use verbatim_win32::window::main_window_of;

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
}
