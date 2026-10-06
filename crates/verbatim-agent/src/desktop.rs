//! The desktop's state, for a test to establish and check what it starts
//! from and to leave the desktop as it found it:
//! [`Request::ForegroundInfo`](crate::protocol::Request::ForegroundInfo) and
//! [`Request::CloseWindows`](crate::protocol::Request::CloseWindows).
//!
//! NVDA's system tests check before every assertion that the window they
//! opened is the foreground window, failing with the foreground window's
//! title and the open windows' titles when it is not, and close the windows
//! they opened with an ordinary close request, waiting for them to go. These
//! are the same operations for Verbatim's suite.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowTextW,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW, WM_CLOSE,
};
use windows::core::{BOOL, PWSTR};

use crate::protocol::{ForegroundInfo, WindowInfo};

/// How often [`close_windows`] checks whether the windows have gone.
const CLOSE_POLL: Duration = Duration::from_millis(100);

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
/// contains `title_contains`, and waits up to `timeout` for them to go.
/// Returns how many were still open when it gave up; zero means all closed.
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
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = matching()
            .into_iter()
            // SAFETY: IsWindow tolerates any handle.
            .filter(|&window| unsafe { IsWindow(Some(window)) }.as_bool())
            .count();
        if remaining == 0 || Instant::now() >= deadline {
            return u32::try_from(remaining).unwrap_or(u32::MAX);
        }
        thread::sleep(CLOSE_POLL);
    }
}

fn window_info(window: HWND) -> WindowInfo {
    let mut pid = 0u32;
    // SAFETY: tolerates any handle, writing 0 for an invalid one.
    unsafe {
        GetWindowThreadProcessId(window, Some(&raw mut pid));
    }
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
    WindowInfo {
        title: window_text(window),
        class: class_name(window),
        image: image_name(pid).unwrap_or_default(),
        cloaked: read.is_ok() && cloaked != 0,
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

    #[test]
    fn the_foreground_report_lists_visible_windows() {
        // A test process may run with no foreground window at all (a CI
        // runner); the list of windows is all that can be relied on, and
        // every entry it holds has a title.
        let info = foreground_info();
        assert!(info.windows.iter().all(|window| !window.title.is_empty()));
    }
}
