//! The process that serves a window's UI Automation, read with local calls
//! only (`docs/design/listener-without-connections.md`, section 4.3): its
//! owner by `GetWindowThreadProcessId`, except a console window, whose owner
//! by that call is the console's client and whose provider is its console
//! host. Also where a window's process ends: its parent window, or a
//! top-level window's visible owner, served by another process
//! ([`parent_across_process_boundary`]).

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GA_PARENT, GW_OWNER, GetAncestor, GetClassNameW, GetDesktopWindow,
    GetWindow, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::BOOL;

/// The class of a console window, whose provider is the console host, the
/// process of the window's thread, while `GetWindowThreadProcessId` names the
/// console's client as its owner.
const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// The process that serves UIA for the window `hwnd`: its owner by
/// `GetWindowThreadProcessId`, except for a console window, whose owner by
/// that call is the console's client while its provider is the console
/// host. Windows names the client's thread for a console window too, so the
/// host is read from the client instead ([`console_host_of`]). `None` for a
/// window that no longer exists, and for a console window whose host cannot
/// be read, such as one whose client runs as another user, so that no
/// outpost takes that window as its application's: never the client's,
/// which would then connect into the console host. Local calls only; a
/// console window costs the client's lookup too.
#[must_use]
pub fn provider_process(hwnd: isize) -> Option<u32> {
    match provider(hwnd)? {
        Provider::Process(process) => Some(process),
        Provider::UnreadConsole => None,
    }
}

/// Whether the process `pid` serves UIA for the window `hwnd`: whether it is
/// the window's [`provider_process`], or, for a console window whose host
/// cannot be read, whether the window is one of `pid`'s threads' own, which
/// `EnumThreadWindows` answers by the thread that created the window where
/// `GetWindowThreadProcessId` names the client's. So a console host
/// recognizes its own console window whatever its client, and no other
/// process does. Local calls only; that fallback costs a snapshot of the
/// system's threads.
#[must_use]
pub fn served_by(hwnd: isize, pid: u32) -> bool {
    match provider(hwnd) {
        Some(Provider::Process(process)) => process == pid,
        Some(Provider::UnreadConsole) => threads_own_window(pid, hwnd),
        None => false,
    }
}

/// A window's provider, as far as it can be read.
#[derive(Debug, PartialEq, Eq)]
enum Provider {
    /// The process that serves the window's UIA.
    Process(u32),
    /// A console window whose console host cannot be read from its client.
    UnreadConsole,
}

/// `hwnd`'s provider, `None` for a window that no longer exists.
fn provider(hwnd: isize) -> Option<Provider> {
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
    Some(provider_of(class.as_deref(), owner, console_host_of))
}

/// The provider of a window of class `class` that `GetWindowThreadProcessId`
/// says `owner` owns: the owner, except for a console window, whose console
/// host `host` reads from the owner, its client.
fn provider_of(class: Option<&str>, owner: u32, host: impl FnOnce(u32) -> Option<u32>) -> Provider {
    if class == Some(CONSOLE_WINDOW_CLASS) {
        host(owner).map_or(Provider::UnreadConsole, Provider::Process)
    } else {
        Provider::Process(owner)
    }
}

/// Whether the top-level window `hwnd` belongs to one of `pid`'s threads by
/// `EnumThreadWindows`, which goes by the thread that created the window: a
/// console window's console host's. Local calls: a snapshot of the system's
/// threads, and an enumeration of each of `pid`'s.
fn threads_own_window(pid: u32, hwnd: isize) -> bool {
    struct Search {
        window: isize,
        found: bool,
    }
    unsafe extern "system" fn visit(window: HWND, search: LPARAM) -> BOOL {
        // SAFETY: `search` is the search passed below, alive for the call.
        let search = unsafe { &mut *(search.0 as *mut Search) };
        search.found = window.0 as isize == search.window;
        BOOL::from(!search.found)
    }
    // SAFETY: a thread snapshot has no preconditions; the handle is closed
    // below.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }) else {
        return false;
    };
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(size_of::<THREADENTRY32>()).unwrap_or(0),
        ..Default::default()
    };
    let mut search = Search {
        window: hwnd,
        found: false,
    };
    // SAFETY: the snapshot just taken; `entry` has its `dwSize` set.
    let mut has_entry = unsafe { Thread32First(snapshot, &raw mut entry) }.is_ok();
    while has_entry && !search.found {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: `visit` reads only the search, which outlives the
            // synchronous enumeration; it answers an error for a thread
            // with no windows, or when `visit` stopped it.
            let _ = unsafe {
                EnumThreadWindows(
                    entry.th32ThreadID,
                    Some(visit),
                    LPARAM((&raw mut search) as isize),
                )
            };
        }
        // SAFETY: as above; the call overwrites `entry` with the next thread.
        has_entry = unsafe { Thread32Next(snapshot, &raw mut entry) }.is_ok();
    }
    // SAFETY: the snapshot handle, closed once.
    let _ = unsafe { CloseHandle(snapshot) };
    search.found
}

/// The window above `hwnd` across the process boundary, where UIA's raw
/// view goes up from `hwnd`'s element: its parent (`GetAncestor` with
/// `GA_PARENT`), as the Settings page's content window is a child of
/// `ApplicationFrameHost`'s frame, or, for a top-level window, its owner
/// (`GetWindow` with `GW_OWNER`) while the owner is visible, as a dialog
/// is owned by the window that opened it; UIA's raw view puts an owned
/// window under a visible owner and under the desktop when the owner is
/// hidden. That window is answered when its [`provider_process`] is not
/// `hwnd`'s, so a console window above counts as its console host's
/// (`docs/design/listener-without-connections.md`, "The process boundary").
/// `None` for a top-level window with no visible owner, for a window above
/// served by the same process, and for a window that no longer exists. A
/// walk up stops at `hwnd` when this answers a window. Local calls only; a
/// console window costs its client's lookup too.
#[must_use]
pub fn parent_across_process_boundary(hwnd: isize) -> Option<isize> {
    let window = HWND(hwnd as *mut _);
    // SAFETY: GetAncestor tolerates any handle, answering null for an
    // invalid one.
    let parent = unsafe { GetAncestor(window, GA_PARENT) };
    if parent.0.is_null() {
        return None;
    }
    // SAFETY: GetDesktopWindow has no preconditions.
    let above = if parent == unsafe { GetDesktopWindow() } {
        // SAFETY: GetWindow tolerates any handle, answering an error for
        // a window with no owner.
        let owner = unsafe { GetWindow(window, GW_OWNER) }.ok()?;
        // SAFETY: IsWindowVisible tolerates any handle.
        if !unsafe { IsWindowVisible(owner) }.as_bool() {
            return None;
        }
        owner
    } else {
        parent
    };
    let above = above.0 as isize;
    (provider(hwnd)? != provider(above)?).then_some(above)
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
    use verbatim_test_console::Console;

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
    fn a_console_window_whose_host_cannot_be_read_has_no_provider_process() {
        assert_eq!(
            provider_of(Some(CONSOLE_WINDOW_CLASS), 20, |_| None),
            Provider::UnreadConsole
        );
        assert_eq!(
            provider_of(Some(CONSOLE_WINDOW_CLASS), 20, |_| Some(30)),
            Provider::Process(30)
        );
        assert_eq!(
            provider_of(Some("Notepad"), 20, |_| panic!("the host is not read")),
            Provider::Process(20)
        );
    }

    #[test]
    fn a_console_window_is_its_console_hosts_threads_own_not_its_clients() {
        let console = Console::start();
        let mut client = 0u32;
        // SAFETY: `client` is a local; the window is the console's.
        unsafe { GetWindowThreadProcessId(HWND(console.window as *mut _), Some(&raw mut client)) };
        assert!(threads_own_window(console.host, console.window));
        assert!(!threads_own_window(client, console.window));
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
