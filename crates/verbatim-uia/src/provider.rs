//! The process that serves a window's UI Automation, read with local calls
//! only (`docs/design/listener-without-connections.md`, section 4.3): its
//! owner by `GetWindowThreadProcessId`, except a console window, whose owner
//! by that call is the console's client and whose provider is its console
//! host. Also where a window's process ends: its parent window owned by
//! another process ([`parent_across_process_boundary`]).

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_PARENT, GetAncestor, GetClassNameW, GetDesktopWindow, GetWindowThreadProcessId,
};

/// The class of a console window, whose provider is the console host, the
/// process of the window's thread, while `GetWindowThreadProcessId` names the
/// console's client as its owner.
const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// The process that serves UIA for the window `hwnd`: its owner by
/// `GetWindowThreadProcessId`, except for a console window, whose owner by
/// that call is the console's client while its provider is the console
/// host. Windows names the client's thread for a console window too, so the
/// host is read from the client instead ([`console_host_of`]). `None` for a
/// window that no longer exists. Local calls only; a console window costs
/// the client's lookup too.
#[must_use]
pub fn provider_process(hwnd: isize) -> Option<u32> {
    let window = HWND(hwnd as *mut _);
    let mut class = [0u16; 32];
    // SAFETY: the buffer outlives the call, which writes at most its
    // length; an invalid handle answers 0.
    let length = unsafe { GetClassNameW(window, &mut class) };
    let mut owner = 0u32;
    // SAFETY: `owner` is a local; an invalid handle answers thread 0.
    let thread = unsafe { GetWindowThreadProcessId(window, Some(&raw mut owner)) };
    if thread == 0 {
        return None;
    }
    let class = usize::try_from(length)
        .ok()
        .and_then(|length| class.get(..length))
        .map(String::from_utf16_lossy);
    if class.as_deref() == Some(CONSOLE_WINDOW_CLASS) {
        return console_host_of(owner).or(Some(owner));
    }
    Some(owner)
}

/// The parent window of `hwnd` across the process boundary: its parent
/// (`GetAncestor` with `GA_PARENT`, which leaves out an owner) when that
/// parent is owned by another process than `hwnd`'s, by
/// `GetWindowThreadProcessId`, as the Settings page's content window is a
/// child of `ApplicationFrameHost`'s frame
/// (`docs/design/listener-without-connections.md`, "The process boundary").
/// `None` for a top-level window, whose parent is the desktop window, for
/// a parent of the same process, and for a window that no longer exists.
/// A walk up stops at `hwnd` when this answers a window. Local calls only.
#[must_use]
pub fn parent_across_process_boundary(hwnd: isize) -> Option<isize> {
    let window = HWND(hwnd as *mut _);
    // SAFETY: GetAncestor tolerates any handle, answering null for an
    // invalid one.
    let parent = unsafe { GetAncestor(window, GA_PARENT) };
    // SAFETY: GetDesktopWindow has no preconditions.
    if parent.0.is_null() || parent == unsafe { GetDesktopWindow() } {
        return None;
    }
    let owner = |window: HWND| {
        let mut process = 0u32;
        // SAFETY: `process` is a local; an invalid handle answers thread 0.
        let thread = unsafe { GetWindowThreadProcessId(window, Some(&raw mut process)) };
        (thread != 0).then_some(process)
    };
    let (own, parents) = (owner(window)?, owner(parent)?);
    (own != parents).then_some(parent.0 as isize)
}

#[link(name = "ntdll")]
unsafe extern "system" {
    /// `ntdll`'s `NtQueryInformationProcess`, for the one information
    /// class the `windows` crate's features here do not reach.
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: u32,
        information: *mut std::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

/// `ProcessConsoleHostProcess`: the console host of a console's client.
const PROCESS_CONSOLE_HOST_PROCESS: u32 = 49;

/// The console host of the console client `client`, the process that
/// serves its console window, or `None` when the client cannot be opened or
/// has no console: `NtQueryInformationProcess`'s `ProcessConsoleHostProcess`,
/// whose answer carries flags in its two low bits. A local call: it sends
/// the console nothing.
#[must_use]
pub fn console_host_of(client: u32) -> Option<u32> {
    // SAFETY: OpenProcess with a query-only right fails safely; the handle
    // is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, client) }.ok()?;
    let mut host = 0usize;
    // SAFETY: `host` is a pointer-sized local, the class's answer; the
    // handle is open with query access.
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            PROCESS_CONSOLE_HOST_PROCESS,
            (&raw mut host).cast(),
            u32::try_from(size_of::<usize>()).unwrap_or(0),
            std::ptr::null_mut(),
        )
    };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(handle) };
    let host = u32::try_from(host & !3).ok()?;
    (status >= 0 && host != 0).then_some(host)
}

#[cfg(test)]
mod tests {
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };
    use windows::core::w;

    use super::*;
    use crate::test_console::Console;

    #[test]
    fn a_console_window_is_served_by_its_console_host_not_its_owner() {
        let console = Console::start();
        let mut owner = 0u32;
        // SAFETY: `owner` is a local; the window is the console's.
        unsafe { GetWindowThreadProcessId(HWND(console.window as *mut _), Some(&raw mut owner)) };
        assert_ne!(owner, console.host, "the console's client owns its window");
        assert_eq!(console_host_of(owner), Some(console.host));
        assert_eq!(provider_process(console.window), Some(console.host));
    }

    #[test]
    fn any_other_window_is_served_by_its_owner() {
        // SAFETY: a predefined class, no parent, menu, or creation data.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("a window of the test's own"),
                WS_OVERLAPPED,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
        }
        .expect("a window of the test's own");
        let served_by = provider_process(window.0 as isize);
        // SAFETY: the window this thread created.
        let _ = unsafe { DestroyWindow(window) };
        assert_eq!(served_by, Some(std::process::id()));
    }
}
