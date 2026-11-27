//! The process that serves a window's UI Automation, read with local calls
//! only (`books/dev/src/explanation/design/listener-without-connections.md`, section 4.3): its
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
    EnumThreadWindows, GA_PARENT, GA_ROOT, GW_OWNER, GetAncestor, GetDesktopWindow, GetWindow,
    IsWindowVisible,
};
use windows::core::BOOL;

use verbatim_model::Pid;

use crate::window_facts::{Raised, Serving, SystemReads, WindowReads};

/// The process that serves UIA for the window `hwnd`, read through `reads`
/// (`SystemReads` for the running system): the one rule every reader, the
/// shell and the tools decide it by ([`Owner::serving_process`]), so a
/// console window is its console host's, never its client's. `raised_by`
/// is the event on the window the caller has, if any ([`Raised`]): a
/// `WinEvent`'s thread, which for a console window is its console host's
/// own, or a UI Automation event's provider process, either naming the
/// host directly. `None` for a window that no longer
/// exists, and for a console window whose host cannot be read. Local
/// calls only: the window's class and owner, and for a console window the
/// thread's `OpenThread` and `GetProcessIdOfThread`, or with no thread
/// given its client's `OpenProcess` and `NtQueryInformationProcess`.
#[must_use]
pub fn serving_process(
    reads: &impl WindowReads,
    hwnd: isize,
    raised_by: Option<Raised>,
) -> Option<Pid> {
    reads
        .owner(hwnd)
        .serving_process(&reads.class(hwnd), reads, raised_by)
}

/// Whether the process `pid` serves UIA for the window `hwnd`: whether it is
/// the window's [`serving_process`], or, for a console window whose host
/// cannot be read, whether the window is one of `pid`'s threads' own, which
/// `EnumThreadWindows` answers by the thread that created the window where
/// `GetWindowThreadProcessId` names the client's. So a console host
/// recognizes its own console window whatever its client, and no other
/// process does. Local calls only; that fallback costs a snapshot of the
/// system's threads.
#[must_use]
pub fn served_by(hwnd: isize, pid: u32) -> bool {
    match serving(hwnd) {
        Some(Serving::Process(process)) => process == pid,
        Some(Serving::UnreadConsole) => threads_own_window(pid, hwnd),
        None => false,
    }
}

/// `hwnd`'s serving process on the running system, a console window whose
/// host cannot be read told apart; `None` for a window that no longer
/// exists.
fn serving(hwnd: isize) -> Option<Serving> {
    SystemReads
        .owner(hwnd)
        .serving(&SystemReads.class(hwnd), &SystemReads, None)
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
/// hidden. That window is answered when its [`serving_process`] is not
/// `hwnd`'s, so a console window above counts as its console host's
/// (`books/dev/src/explanation/design/listener-without-connections.md`, "The process boundary").
/// `None` for a top-level window with no visible owner, for a window above
/// served by the same process, and for a window that no longer exists. A
/// walk up stops at `hwnd` when this answers a window. Local calls only; a
/// console window costs its client's lookup too.
#[must_use]
pub fn parent_across_process_boundary(hwnd: isize) -> Option<isize> {
    let above = raw_view_parent_window(hwnd)?;
    (serving(hwnd)? != serving(above)?).then_some(above)
}

/// The window UIA's raw view goes up to from `hwnd`'s element, as
/// [`parent_across_process_boundary`] finds it, whatever process serves it:
/// its parent window, or, for a top-level window, its owner while the owner
/// is visible. `None` for a top-level window with no visible owner and for
/// a window that no longer exists. Local calls only.
#[must_use]
pub fn raw_view_parent_window(hwnd: isize) -> Option<isize> {
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
    Some(above.0 as isize)
}

/// The parent window of `hwnd` (`GetAncestor` with `GA_PARENT`), 0 for
/// none. A local call.
#[must_use]
pub fn parent_window(hwnd: isize) -> isize {
    // SAFETY: GetAncestor tolerates any handle, answering null for an
    // invalid one.
    unsafe { GetAncestor(HWND(hwnd as *mut _), GA_PARENT) }.0 as isize
}

/// The nearest window of the outermost element of a walk up the raw view,
/// by local calls (`books/dev/src/reference/adr/0001-focus.md`, "When an application's call
/// fails"): a walk whose step to a parent failed goes on
/// from its parent window, as NVDA's parent read falls back to the window
/// hierarchy (`NVDAObjects/UIA/__init__.py` 2142 to 2145,
/// `Window._get_parent`). `nearest` is the first element's nearest window,
/// and `hwnds` each element's own native window handle, from the first
/// outward, 0 for one that is not a window: an element that is a window's
/// own has that window; one with none above one that had its own is
/// hosted, in the raw view, by that window's parent window; and one with
/// none above one with none has the same window as it. 0 when not known.
#[must_use]
pub fn outermost_nearest_window(nearest: isize, hwnds: impl IntoIterator<Item = isize>) -> isize {
    outermost_window(nearest, hwnds).0
}

/// The window plain windows, it included, complete a walk up the raw view
/// from when the walk is cut short, by its bound or its depth limit, by
/// local calls (`books/dev/src/reference/adr/0001-focus.md`, "When an application's call fails"):
/// the outermost element's nearest window
/// ([`outermost_nearest_window`]), or that window's parent window when the
/// outermost element is the window's own element, which the walk has
/// already read, so that the window is not in the chain twice. `nearest`
/// and `hwnds` as for [`outermost_nearest_window`].
#[must_use]
pub fn plain_windows_start(nearest: isize, hwnds: impl IntoIterator<Item = isize>) -> isize {
    match outermost_window(nearest, hwnds) {
        (window, true) => parent_window(window),
        (window, false) => window,
    }
}

/// The outermost element's nearest window, and whether the element is
/// that window's own, for [`outermost_nearest_window`] and
/// [`plain_windows_start`].
fn outermost_window(nearest: isize, hwnds: impl IntoIterator<Item = isize>) -> (isize, bool) {
    let mut window = nearest;
    let mut own = false;
    for hwnd in hwnds {
        if hwnd != 0 {
            window = hwnd;
            own = true;
        } else if own {
            window = parent_window(window);
            own = false;
        }
    }
    (window, own)
}

/// Whether `hwnd` is a top-level window, its own root window (`GetAncestor`
/// with `GA_ROOT`); `false` for a child window and for a window that no
/// longer exists. A local call.
#[must_use]
pub fn is_top_level_window(hwnd: isize) -> bool {
    let window = HWND(hwnd as *mut _);
    // SAFETY: GetAncestor tolerates any handle, answering null for an
    // invalid one.
    hwnd != 0 && unsafe { GetAncestor(window, GA_ROOT) } == window
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

/// The process `thread` belongs to, read with `GetProcessIdOfThread`, a
/// local call; `None` when the thread cannot be opened, such as one that
/// has ended.
pub(crate) fn process_of_thread(thread: u32) -> Option<u32> {
    use windows::Win32::System::Threading::{
        GetProcessIdOfThread, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: OpenThread has no preconditions; the handle is closed below.
    let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread) }.ok()?;
    // SAFETY: an open thread handle with the access the call needs.
    let pid = unsafe { GetProcessIdOfThread(handle) };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(handle) };
    (pid != 0).then_some(pid)
}

/// The console host of the console client `client`, the process that
/// serves its console window, or `None` when the client cannot be opened or
/// has no console: `NtQueryInformationProcess`'s `ProcessConsoleHostProcess`,
/// whose answer carries flags in its two low bits. A local call: it sends
/// the console nothing. Read only by [`serving_process`]'s rule.
pub(crate) fn console_host_of(client: u32) -> Option<u32> {
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
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_CHILD, WS_OVERLAPPED,
    };
    use windows::core::w;

    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    use super::*;
    use crate::window_facts::node_window;
    use verbatim_test_console::Console;

    #[test]
    fn a_console_window_is_served_by_its_console_host_not_its_owner() {
        Console::with(|console| {
            let mut owner = 0u32;
            // SAFETY: `owner` is a local; the window is the console's.
            unsafe {
                GetWindowThreadProcessId(HWND(console.window as *mut _), Some(&raw mut owner))
            };
            assert_ne!(owner, console.host, "the console's client owns its window");
            assert_eq!(console_host_of(owner), Some(console.host));
            assert_eq!(
                serving_process(&SystemReads, console.window, None),
                Some(Pid(console.host))
            );
        });
    }

    /// A node read in a console window names the console host as the
    /// process its requests go to, so object navigation's next step is
    /// asked of the console host's outpost, which holds the node, and not
    /// of the client's (second 2 of the audit).
    #[test]
    fn a_node_in_a_console_window_names_its_console_host_not_its_client() {
        Console::with(|console| {
            assert_eq!(
                node_window(&SystemReads, console.window).process,
                Some(Pid(console.host))
            );
        });
    }

    #[test]
    fn a_console_window_is_its_console_hosts_threads_own_not_its_clients() {
        Console::with(|console| {
            let mut client = 0u32;
            // SAFETY: `client` is a local; the window is the console's.
            unsafe {
                GetWindowThreadProcessId(HWND(console.window as *mut _), Some(&raw mut client))
            };
            assert!(threads_own_window(console.host, console.window));
            assert!(!threads_own_window(client, console.window));
        });
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
        let served_by = serving_process(&SystemReads, window.0 as isize, None);
        // SAFETY: the window this thread created.
        let _ = unsafe { DestroyWindow(window) };
        assert_eq!(served_by, Some(Pid(std::process::id())));
    }

    /// A walk cut short whose outermost element is its window's own has
    /// read that window, so plain windows complete it from the window's
    /// parent; one whose outermost element has no window of its own, from
    /// its nearest window, which the walk has not read.
    #[test]
    fn plain_windows_start_above_a_window_the_walk_read() {
        // SAFETY: a predefined class, no parent, menu, or creation data.
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("the parent"),
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
        // SAFETY: a predefined class under the window just created.
        let child = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("the child"),
                WS_CHILD,
                0,
                0,
                5,
                5,
                Some(parent),
                None,
                None,
                None,
            )
        }
        .expect("a child window");
        let (parent_hwnd, child_hwnd) = (parent.0 as isize, child.0 as isize);
        let starts = [
            plain_windows_start(child_hwnd, [0, 0]),
            plain_windows_start(child_hwnd, [0, child_hwnd]),
            plain_windows_start(child_hwnd, [child_hwnd, 0]),
        ];
        let outermost = outermost_nearest_window(child_hwnd, [0, child_hwnd]);
        // SAFETY: the window this thread created, its child with it.
        let _ = unsafe { DestroyWindow(parent) };
        assert_eq!(
            starts,
            [child_hwnd, parent_hwnd, parent_hwnd],
            "windowless elements; an ancestor that is the child's own; a windowless one above it"
        );
        assert_eq!(outermost, child_hwnd);
    }
}
