//! Bringing a launched application's window to the foreground, for
//! [`Request::BringToForeground`](crate::protocol::Request::BringToForeground).
//!
//! Windows refuses the foreground to a newly launched application while its
//! foreground lock is in force: for 200 seconds after the last user input,
//! which includes the keystrokes earlier scenarios inject. The application's
//! window then raises a foreground event but stays behind the current
//! foreground window, and a screen reader that, like NVDA, checks the real
//! foreground window rightly says nothing. A user who launched the
//! application would have it in front, so the harness puts it there with the
//! same technique `verbatim-gui` uses for its own popup: a bare Control tap
//! to satisfy the lock's input heuristic, `SetForegroundWindow`, and, failing
//! that, the call made while attached to the foreground thread's input queue.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VK_CONTROL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GW_OWNER, GetForegroundWindow, GetWindow, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsWindowVisible, SW_SHOW, SetForegroundWindow, ShowWindow,
};
use windows::core::BOOL;

use crate::process::matching_pids;

/// How long to wait between looking for the window and checking the
/// foreground.
const POLL: Duration = Duration::from_millis(50);

/// Waits up to `timeout` for a visible, titled, unowned top-level window
/// belonging to a process whose image name is `image_name`, and brings it to
/// the foreground. Returns whether such a window is the foreground window
/// when this returns; `false` if none appeared in time or Windows refused.
///
/// # Errors
///
/// Returns an error if the process list cannot be read.
pub fn bring_to_foreground(image_name: &str, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let pids = matching_pids(image_name)?;
        if let Some(window) = main_window_of(&pids) {
            if foreground_is(&pids) {
                return Ok(true);
            }
            force_foreground(window);
            if foreground_is(&pids) {
                return Ok(true);
            }
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(POLL);
    }
}

/// Whether the foreground window belongs to one of `pids`.
fn foreground_is(pids: &[u32]) -> bool {
    // SAFETY: GetForegroundWindow has no preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    !foreground.is_invalid() && pids.contains(&window_pid(foreground))
}

fn window_pid(window: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: tolerates any handle, writing 0 for an invalid one.
    unsafe {
        GetWindowThreadProcessId(window, Some(&raw mut pid));
    }
    pid
}

/// The first visible, titled, unowned top-level window of one of `pids`.
fn main_window_of(pids: &[u32]) -> Option<HWND> {
    struct Search<'a> {
        pids: &'a [u32],
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(window: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the search passed below, alive for the call.
        let search = unsafe { &mut *(lparam.0 as *mut Search<'_>) };
        // SAFETY: each call tolerates any handle.
        let candidate = unsafe {
            IsWindowVisible(window).as_bool()
                && GetWindow(window, GW_OWNER).is_err()
                && GetWindowTextLengthW(window) > 0
        };
        if candidate && search.pids.contains(&window_pid(window)) {
            search.found = Some(window);
            return BOOL(0);
        }
        BOOL(1)
    }
    if pids.is_empty() {
        return None;
    }
    let mut search = Search { pids, found: None };
    // SAFETY: `visit` reads only the search state, which outlives the
    // synchronous EnumWindows call. EnumWindows reports an error when the
    // callback stops it early, which is how a match ends the walk.
    unsafe {
        let _ = EnumWindows(
            Some(visit),
            LPARAM((&raw mut search).cast::<c_void>() as isize),
        );
    }
    search.found
}

/// Injects a bare Control down-then-up tap, which satisfies the foreground
/// lock's input heuristic before `SetForegroundWindow` (see
/// `verbatim-gui`'s `foreground` module for why Control and not Alt).
fn nudge_foreground_lock() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_CONTROL,
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    // SAFETY: `inputs` is a fully initialized array of INPUT structures,
    // copied by SendInput.
    unsafe {
        SendInput(&inputs, i32::try_from(size_of::<INPUT>()).unwrap_or(0));
    }
}

/// Brings `window` to the foreground: directly after the nudge, else while
/// attached to the current foreground thread's input queue, detaching at
/// once.
fn force_foreground(window: HWND) {
    nudge_foreground_lock();
    // SAFETY: every call below tolerates a stale handle; the attachment is
    // undone before returning.
    unsafe {
        let _ = ShowWindow(window, SW_SHOW);
        let _ = BringWindowToTop(window);
        if SetForegroundWindow(window).as_bool() {
            return;
        }
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return;
        }
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let our_thread = GetCurrentThreadId();
        if foreground_thread == 0 || foreground_thread == our_thread {
            return;
        }
        if AttachThreadInput(our_thread, foreground_thread, true).as_bool() {
            let _ = SetForegroundWindow(window);
            let _ = BringWindowToTop(window);
            let _ = AttachThreadInput(our_thread, foreground_thread, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_with_no_process_reports_not_taken() {
        let taken = bring_to_foreground(
            "verbatim-agent-test-nonexistent-image-name.exe",
            Duration::from_millis(100),
        )
        .expect("an unmatched name is not an error");
        assert!(!taken);
    }
}
