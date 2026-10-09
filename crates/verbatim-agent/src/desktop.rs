//! The desktop's state, for a test to establish and check what it starts
//! from and to leave the desktop as it found it:
//! [`Request::ForegroundInfo`](crate::protocol::Request::ForegroundInfo),
//! [`Request::CloseWindows`](crate::protocol::Request::CloseWindows),
//! [`Request::MinimizeAll`](crate::protocol::Request::MinimizeAll),
//! [`Request::SetForeground`](crate::protocol::Request::SetForeground), and
//! the conditions
//! [`Request::WaitForWindow`](crate::protocol::Request::WaitForWindow)
//! waits for.
//!
//! NVDA's system tests check before every assertion that the window they
//! opened is the foreground window, failing with the foreground window's
//! title and the open windows' titles when it is not, and close the windows
//! they opened with an ordinary close request, waiting for them to go. These
//! are the same operations for Verbatim's suite.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use std::time::Duration;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GW_OWNER, GWL_STYLE, GetClassNameW, GetForegroundWindow, GetWindow,
    GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    PostMessageW, SMTO_ABORTIFHUNG, SW_MINIMIZE, SW_RESTORE, SendMessageTimeoutW,
    SetForegroundWindow, ShowWindow, ShowWindowAsync, WM_CLOSE, WM_COMMAND, WS_MINIMIZEBOX,
};
use windows::core::{BOOL, PWSTR, w};

use crate::protocol::{ForegroundInfo, WindowCondition, WindowInfo};
use crate::wait;

/// The foreground window and every visible, titled, unowned top-level
/// window, each with its title, class, and image name.
#[must_use]
pub fn foreground_info() -> ForegroundInfo {
    // SAFETY: GetForegroundWindow has no preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    ForegroundInfo {
        foreground: (!foreground.is_invalid()).then(|| window_info(foreground)),
        windows: top_level_windows().into_iter().map(window_info).collect(),
    }
}

/// Sends a close request to every visible top-level window whose title
/// contains `title_contains`, and waits up to `timeout` for them to go,
/// on window events ([`wait::until`]). Returns how many were still open
/// when it gave up; zero means all closed.
#[must_use]
pub fn close_windows(title_contains: &str, timeout: Duration) -> u32 {
    let matching = || -> Vec<HWND> {
        top_level_windows()
            .into_iter()
            .filter(|&window| window_text(window).contains(title_contains))
            .collect()
    };
    for window in matching() {
        // Checked again just before the close: a window destroyed since the
        // enumeration could have its handle reused by an unrelated one.
        if !window_text(window).contains(title_contains) {
            continue;
        }
        // SAFETY: PostMessageW carries no pointer and tolerates a window
        // that has since gone; the title was checked just above.
        unsafe {
            let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    wait::until(|| matching().is_empty(), timeout);
    u32::try_from(matching().len()).unwrap_or(u32::MAX)
}

/// Whether `condition` holds on the desktop now.
#[must_use]
pub fn holds(condition: &WindowCondition) -> bool {
    match condition {
        WindowCondition::Foreground {
            title_contains,
            unsaved,
        } => foreground_info().foreground.is_some_and(|window| {
            !window.cloaked
                && window.title.contains(title_contains.as_str())
                && unsaved.is_none_or(|unsaved| window.title.starts_with('*') == unsaved)
        }),
        WindowCondition::NotForeground { title_contains } => !foreground_info()
            .foreground
            .is_some_and(|window| window.title.contains(title_contains.as_str())),
        WindowCondition::Present { title_contains } => top_level_windows()
            .into_iter()
            .any(|window| window_text(window).contains(title_contains.as_str())),
        WindowCondition::Absent { title_contains } => !top_level_windows()
            .into_iter()
            .any(|window| window_text(window).contains(title_contains.as_str())),
        WindowCondition::AllMinimized => top_level_windows()
            .into_iter()
            .all(|window| !minimizable(window) || is_minimized(window) || is_cloaked(window)),
    }
}

/// Waits up to `timeout` for `condition`, on window events
/// ([`wait::until`]), and returns whether it held and the desktop then.
#[must_use]
pub fn wait_for(condition: &WindowCondition, timeout: Duration) -> (bool, ForegroundInfo) {
    let met = wait::until(|| holds(condition), timeout);
    (met, foreground_info())
}

/// Minimizes every window as the taskbar's Show Desktop command does, and
/// gives the desktop the foreground: the state every scenario starts from.
/// Minimizing leaves the foreground on the window that had it, minimized,
/// so the desktop's window, Program Manager, is then brought forward with
/// `SetForegroundWindow`, injecting no input. The taskbar's command
/// minimizes only the windows the taskbar has taken in, which a window
/// opened moments before, such as the Notepad document a scenario opens
/// just before, may not yet be (found 2026-10-08: that window stayed
/// restored and the run failed), so each window the command is to minimize
/// is also sent a minimize of its own (`ShowWindowAsync`, which never waits
/// on the window's thread). The command is sent and waited for, so each
/// window still restored once the taskbar has handled it is logged, at
/// info, with its class and how long its process has run, before it is
/// minimized directly. Waits up to `timeout` for
/// every window that can be minimized to be ([`WindowCondition::AllMinimized`]),
/// and then for the desktop to hold the foreground. Returns whether both
/// held, and the desktop then.
#[must_use]
pub fn minimize_all(timeout: Duration) -> (bool, ForegroundInfo) {
    /// The taskbar's command that minimizes every window.
    const MINIMIZE_ALL: usize = 419;
    // SAFETY: looks a window up by class; no pointer is kept.
    if let Ok(taskbar) = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) } {
        // SAFETY: a command message carrying no pointer; it returns once
        // the taskbar has handled it, or after the limit if the taskbar
        // does not answer.
        unsafe {
            let _ = SendMessageTimeoutW(
                taskbar,
                WM_COMMAND,
                WPARAM(MINIMIZE_ALL),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                TASKBAR_LIMIT_MS,
                None,
            );
        }
    }
    for window in top_level_windows()
        .into_iter()
        .filter(|&window| minimizable(window) && !is_minimized(window) && !is_cloaked(window))
    {
        let info = window_info(window);
        tracing::info!(
            class = info.class,
            image = info.image,
            title = info.title,
            process_age_ms = process_age_ms(info.pid),
            "a window was still restored once the taskbar's Minimize All was handled; minimized directly"
        );
        // SAFETY: tolerates any handle; the minimize is posted to the
        // window's thread, and its answer is not waited for.
        let _ = unsafe { ShowWindowAsync(window, SW_MINIMIZE) };
    }
    let (minimized, desktop) = wait_for(&WindowCondition::AllMinimized, timeout);
    if !minimized {
        return (false, desktop);
    }
    // SAFETY: looks a window up by class; no pointer is kept.
    let Ok(desktop_window) = (unsafe { FindWindowW(w!("Progman"), None) }) else {
        return (false, foreground_info());
    };
    // SAFETY: a window just found; a stale one fails.
    let _ = unsafe { SetForegroundWindow(desktop_window) };
    wait_for(
        &WindowCondition::Foreground {
            title_contains: window_text(desktop_window),
            unsaved: None,
        },
        timeout,
    )
}

/// Brings `window` to the foreground as clicking its taskbar button does,
/// injecting no input, and returns whether it is the foreground window
/// afterwards.
///
/// A minimized window is restored and then set as the foreground with
/// `SetForegroundWindow`; Windows gives a window it restores from
/// minimized the foreground whatever input came last. A restored window
/// that is not already in front is minimized first, and restored once it
/// is: `SetForegroundWindow` alone succeeds only while the agent may set
/// the foreground, which it may only after injecting the last input
/// (`docs/tooling.md`, "Windows' foreground lock keeps launched
/// applications behind"). msinfo32 is such a window: it ignores the
/// minimized show state it is launched with and opens restored and
/// inactive (found 2026-10-09). Such a window is logged, at info, and the
/// minimize is waited for on window events, up to [`MINIMIZE_LIMIT`].
///
/// `SetForegroundWindow` can return before the window is the foreground
/// window, so the answer is not read at once: it waits on window events,
/// the foreground event among them, for the window to be in front, up to
/// [`FOREGROUND_LIMIT`]. Read straight after the call, the foreground was
/// sometimes still the window before, which took the foreground a moment
/// after the `false` answer (`phase6-design.md`, "Test isolation and the
/// foreground lock", 2026-10-09).
#[must_use]
pub fn set_foreground(window: u64) -> bool {
    let window = HWND(usize::try_from(window).unwrap_or(0) as *mut c_void);
    // SAFETY: GetForegroundWindow has no preconditions.
    let in_front = unsafe { GetForegroundWindow() } == window;
    if !in_front && !is_minimized(window) {
        let info = window_info(window);
        tracing::info!(
            class = info.class,
            image = info.image,
            title = info.title,
            "a window to bring forward was restored and not in front; minimized before it is restored"
        );
        // SAFETY: tolerates any handle; the return value is the window's
        // earlier visibility, not a failure.
        let _ = unsafe { ShowWindow(window, SW_MINIMIZE) };
        if !wait::until(|| is_minimized(window), MINIMIZE_LIMIT) {
            return false;
        }
    }
    if is_minimized(window) {
        // SAFETY: as above.
        let _ = unsafe { ShowWindow(window, SW_RESTORE) };
    }
    // SAFETY: tolerates any handle; a stale one fails. A refusal is not
    // final: a window restored from minimized just above may still be
    // taking the foreground, so the wait below decides.
    let _ = unsafe { SetForegroundWindow(window) };
    wait::until(
        || {
            // SAFETY: GetForegroundWindow has no preconditions.
            let foreground = unsafe { GetForegroundWindow() };
            foreground == window
        },
        FOREGROUND_LIMIT,
    )
}

/// How long [`set_foreground`] waits for a window it minimized to be
/// minimized: well within the client's read timeout.
const MINIMIZE_LIMIT: Duration = Duration::from_secs(5);

/// How long [`set_foreground`] waits for the window to be the foreground
/// window: with [`MINIMIZE_LIMIT`], well within the client's read timeout.
const FOREGROUND_LIMIT: Duration = Duration::from_secs(5);

/// Whether `window` has a minimize box, so Show Desktop minimizes it.
fn minimizable(window: HWND) -> bool {
    // SAFETY: reads a window's style; 0 for an invalid handle.
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) };
    u32::try_from(style).is_ok_and(|style| style & WS_MINIMIZEBOX.0 != 0)
}

/// Whether `window` is cloaked: kept by the window manager but not shown,
/// as a suspended app's window is.
fn is_cloaked(window: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: the out-parameter is a u32, the size DWMWA_CLOAKED writes.
    let read = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast::<c_void>(),
            u32::try_from(size_of::<u32>()).unwrap_or(4),
        )
    };
    read.is_ok() && cloaked != 0
}

/// Whether `window` is minimized.
fn is_minimized(window: HWND) -> bool {
    // SAFETY: tolerates any handle.
    unsafe { IsIconic(window) }.as_bool()
}

/// What a test is told of `window`.
pub(crate) fn window_info(window: HWND) -> WindowInfo {
    let mut pid = 0u32;
    // SAFETY: tolerates any handle, writing 0 for an invalid one.
    unsafe {
        GetWindowThreadProcessId(window, Some(&raw mut pid));
    }
    WindowInfo {
        window: window.0 as u64,
        pid,
        title: window_text(window),
        class: class_name(window),
        image: image_name(pid).unwrap_or_default(),
        cloaked: is_cloaked(window),
        minimized: is_minimized(window),
    }
}

/// The visible, titled, unowned top-level windows, in Z order.
fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn visit(window: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the vector passed below, alive for the call.
        let windows = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        // SAFETY: tolerates any handle.
        let visible = unsafe { IsWindowVisible(window) }.as_bool();
        // SAFETY: as above.
        let candidate = visible && unsafe { GetWindow(window, GW_OWNER) }.is_err();
        if candidate && !window_text(window).is_empty() {
            windows.push(window);
        }
        BOOL(1)
    }
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: `visit` only pushes into the vector, which outlives the
    // synchronous EnumWindows call.
    unsafe {
        let _ = EnumWindows(
            Some(visit),
            LPARAM((&raw mut windows).cast::<c_void>() as isize),
        );
    }
    windows
}

fn window_text(window: HWND) -> String {
    let mut buffer = [0u16; 512];
    // SAFETY: writes at most the buffer's length; 0 for a window with no
    // text or an invalid handle.
    let length = unsafe { GetWindowTextW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
}

fn class_name(window: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: writes at most the buffer's length; 0 for an invalid handle.
    let length = unsafe { GetClassNameW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
}

/// How long the taskbar is given to handle Minimize All.
const TASKBAR_LIMIT_MS: u32 = 5_000;

/// How long process `pid` has run, in milliseconds; `None` when it cannot
/// be read.
fn process_age_ms(pid: u32) -> Option<u64> {
    // SAFETY: a query-only open that fails safely; the handle is closed
    // below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: `handle` is open with query access; the out-parameters are
    // locals.
    let read = unsafe {
        GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(handle) };
    read.ok()?;
    // SAFETY: no preconditions.
    let now = unsafe { GetSystemTimeAsFileTime() };
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    // Filetime ticks are 100 nanoseconds.
    Some(ticks(now).saturating_sub(ticks(created)) / 10_000)
}

/// `pid`'s executable file name, such as `notepad.exe`.
fn image_name(pid: u32) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: a query-only open that fails safely; the handle is closed
    // below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    // SAFETY: `handle` is open with query access; the buffer outlives the
    // call, which writes at most `length` units.
    let read = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(handle) };
    read.ok()?;
    let path = String::from_utf16_lossy(&buffer[..usize::try_from(length).ok()?]);
    Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, DESKTOP_CREATEWINDOW,
        DESKTOP_ENUMERATE, DESKTOP_READOBJECTS, DESKTOP_WRITEOBJECTS, HDESK, SetThreadDesktop,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE,
        WINDOW_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
    };
    use windows::core::{HSTRING, PCWSTR, w};

    #[test]
    fn closing_windows_no_window_matches_reports_none_left() {
        assert_eq!(
            close_windows(
                "verbatim-agent-test-no-window-has-this-title",
                Duration::from_millis(50)
            ),
            0
        );
    }

    /// The class of the test's windows.
    const CLASS: PCWSTR = w!("VerbatimAgentDesktopTest");

    /// The test windows' procedure: the default one.
    unsafe extern "system" fn window_procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: the arguments Windows passed this window procedure.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// Makes a top-level window of the test's class on this thread's
    /// desktop.
    fn make_window(title: PCWSTR, style: WINDOW_STYLE, owner: Option<HWND>) -> HWND {
        // SAFETY: the class is registered, the strings are static, and the
        // owner, when there is one, is a live window of this thread.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS,
                title,
                style,
                0,
                0,
                100,
                100,
                owner,
                None,
                None,
                None,
            )
        }
        .expect("creates a window")
    }

    /// The report lists exactly the visible, titled, unowned top-level
    /// windows. The windows are made on a desktop of the test's own, which
    /// holds nothing else and is never shown, so what is listed does not
    /// depend on the machine and the user's desktop is left alone.
    #[test]
    fn the_foreground_report_lists_visible_windows() {
        let access = DESKTOP_CREATEWINDOW.0
            | DESKTOP_ENUMERATE.0
            | DESKTOP_READOBJECTS.0
            | DESKTOP_WRITEOBJECTS.0;
        let name = format!("verbatim-agent-test-{}", std::process::id());
        // SAFETY: a valid name and no device, mode, or security attributes.
        let desktop = unsafe {
            CreateDesktopW(
                &HSTRING::from(name),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS(0),
                access,
                None,
            )
        }
        .expect("creates a desktop");
        let handle = desktop.0 as usize;
        let (info, listed) = thread::spawn(move || {
            // SAFETY: this new thread has no windows or hooks yet, and the
            // desktop stays open until the thread has ended.
            unsafe { SetThreadDesktop(HDESK(handle as *mut c_void)) }
                .expect("moves to the test's desktop");
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_procedure),
                lpszClassName: CLASS,
                ..WNDCLASSW::default()
            };
            // SAFETY: a class with a valid window procedure and a static name.
            assert_ne!(unsafe { RegisterClassW(&raw const class) }, 0, "registers");
            let listed = make_window(w!("Listed"), WS_OVERLAPPEDWINDOW | WS_VISIBLE, None);
            let windows = [
                listed,
                make_window(PCWSTR::null(), WS_OVERLAPPEDWINDOW | WS_VISIBLE, None),
                make_window(w!("Hidden"), WS_OVERLAPPEDWINDOW, None),
                make_window(w!("Owned"), WS_POPUP | WS_VISIBLE, Some(listed)),
            ];
            let info = foreground_info();
            for window in windows.into_iter().rev() {
                // SAFETY: a window this thread made.
                unsafe { DestroyWindow(window) }.expect("destroys the window");
            }
            (info, listed.0 as u64)
        })
        .join()
        .expect("the window thread ends");
        // SAFETY: the desktop opened above; its only thread has ended.
        unsafe { CloseDesktop(desktop) }.expect("closes the desktop");

        let image = std::env::current_exe()
            .expect("this test's executable")
            .file_name()
            .expect("a file name")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            info,
            ForegroundInfo {
                foreground: None,
                windows: vec![WindowInfo {
                    window: listed,
                    pid: std::process::id(),
                    title: "Listed".to_owned(),
                    class: "VerbatimAgentDesktopTest".to_owned(),
                    image,
                    cloaked: false,
                    minimized: false,
                }],
            }
        );
    }
}
