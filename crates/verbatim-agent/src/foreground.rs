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
//!
//! None of those can displace a cloaked `Windows.UI.Core` window, which the
//! Start menu's search host leaves as the foreground window after it closes
//! (found live). As a last resort, as NVDA's system tests fall back to the
//! task switcher, one Alt+Tab is injected: the shell switches to a real
//! window, after which the call succeeds.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, BringWindowToTop, EnumWindows, GW_OWNER, GetForegroundWindow,
    GetWindow, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible, SW_SHOW,
    SetForegroundWindow, ShowWindow,
};
use windows::core::BOOL;

use crate::process::matching_pids;

/// How long to wait between looking for the window and checking the
/// foreground.
const POLL: Duration = Duration::from_millis(50);

/// Waits, up to [`SWITCH_SETTLE`], until the foreground has left `stuck` and
/// stayed on one window for [`HOLD_CHECK`]: the shell finishes an Alt+Tab
/// switch asynchronously, and forcing the target before it has would let
/// the switch take the foreground back.
fn settle_after_switch(stuck: HWND) {
    let deadline = Instant::now() + SWITCH_SETTLE;
    let mut last = stuck;
    let mut since = Instant::now();
    while Instant::now() < deadline {
        thread::sleep(POLL);
        // SAFETY: GetForegroundWindow has no preconditions.
        let now = unsafe { GetForegroundWindow() };
        if now != last {
            last = now;
            since = Instant::now();
        } else if now != stuck && since.elapsed() >= HOLD_CHECK {
            return;
        }
    }
}

/// How long a window must keep the foreground to count as having it.
const HOLD_CHECK: Duration = Duration::from_millis(300);

/// The longest the shell is given to finish an Alt+Tab switch.
const SWITCH_SETTLE: Duration = Duration::from_secs(3);

/// Waits up to `timeout` for a visible, titled, unowned top-level window
/// belonging to a process whose image name is `image_name` and, when
/// `title_contains` is set, whose title contains it, and brings it to the
/// foreground. Returns whether such a window is the foreground window when
/// this returns; `false` if none appeared in time or Windows refused.
///
/// # Errors
///
/// Returns an error if the process list cannot be read.
pub fn bring_to_foreground(
    image_name: &str,
    title_contains: Option<&str>,
    timeout: Duration,
) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    let mut switched = false;
    loop {
        let pids = matching_pids(image_name)?;
        if let Some(window) = main_window_of(&pids, title_contains) {
            if foreground_is(window) {
                eprintln!("verbatim-agent: {image_name} is already the foreground window");
                return Ok(true);
            }
            force_foreground(window, &pids);
            if holds_foreground(window) {
                eprintln!(
                    "verbatim-agent: {image_name} brought to the foreground{}",
                    if switched { " after an Alt+Tab" } else { "" }
                );
                return Ok(true);
            }
            if !switched {
                switched = true;
                // SAFETY: GetForegroundWindow has no preconditions.
                let stuck = unsafe { GetForegroundWindow() };
                eprintln!(
                    "verbatim-agent: {image_name} refused the foreground (held by window {:?}); trying Alt+Tab",
                    stuck.0
                );
                alt_tab();
                settle_after_switch(stuck);
                continue;
            }
        }
        if Instant::now() >= deadline {
            eprintln!("verbatim-agent: {image_name} did not take the foreground in time");
            return Ok(false);
        }
        thread::sleep(POLL);
    }
}

/// Whether `window` is the foreground window, and still is a moment later:
/// a shell switch still settling (an Alt+Tab) can take the foreground back
/// just after it was given.
fn holds_foreground(window: HWND) -> bool {
    if !foreground_is(window) {
        return false;
    }
    thread::sleep(HOLD_CHECK);
    foreground_is(window)
}

/// Whether `window` is the foreground window.
fn foreground_is(window: HWND) -> bool {
    // SAFETY: GetForegroundWindow has no preconditions.
    (unsafe { GetForegroundWindow() }) == window
}

fn window_pid(window: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: tolerates any handle, writing 0 for an invalid one.
    unsafe {
        GetWindowThreadProcessId(window, Some(&raw mut pid));
    }
    pid
}

/// The first visible, titled, unowned top-level window of one of `pids`
/// whose title contains `title_contains`, when that is set.
fn main_window_of(pids: &[u32], title_contains: Option<&str>) -> Option<HWND> {
    struct Search<'a> {
        pids: &'a [u32],
        title_contains: Option<&'a str>,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(window: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the search passed below, alive for the call.
        let search = unsafe { &mut *(lparam.0 as *mut Search<'_>) };
        // SAFETY: each call below tolerates any handle.
        let candidate = unsafe { IsWindowVisible(window) }.as_bool()
            // SAFETY: as above.
            && unsafe { GetWindow(window, GW_OWNER) }.is_err()
            // SAFETY: as above.
            && unsafe { GetWindowTextLengthW(window) } > 0;
        if candidate
            && search.pids.contains(&window_pid(window))
            && search
                .title_contains
                .is_none_or(|text| window_title(window).contains(text))
        {
            search.found = Some(window);
            return BOOL(0);
        }
        BOOL(1)
    }
    if pids.is_empty() {
        return None;
    }
    let mut search = Search {
        pids,
        title_contains,
        found: None,
    };
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

/// A window's title.
fn window_title(window: HWND) -> String {
    let mut buffer = [0u16; 512];
    // SAFETY: writes at most the buffer's length.
    let length =
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
}

/// Injects one Alt+Tab, leaving Alt held briefly so the shell completes the
/// switch rather than showing its switcher.
fn alt_tab() {
    send_keys(&[(VK_MENU, false), (VK_TAB, false), (VK_TAB, true)]);
    thread::sleep(Duration::from_millis(150));
    send_keys(&[(VK_MENU, true)]);
}

/// Injects key presses (`true` for a release).
fn send_keys(keys: &[(VIRTUAL_KEY, bool)]) {
    let inputs: Vec<INPUT> = keys
        .iter()
        .map(|&(key, up)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    ..Default::default()
                },
            },
        })
        .collect();
    // SAFETY: `inputs` is a fully initialized slice of INPUT structures,
    // copied by SendInput.
    unsafe {
        SendInput(&inputs, i32::try_from(size_of::<INPUT>()).unwrap_or(0));
    }
}

/// Whether a launch may tap Control, real input, to let the launched
/// program take the foreground past the foreground lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForegroundNudge {
    /// It may: the agent serving a harness, whose launches must come to
    /// the front as a user's would.
    Allowed,
    /// It never does: this crate's own tests, whose launches must not
    /// press a key into a desktop that another program, such as a Verbatim
    /// under an end-to-end run, is using. The launched program comes to
    /// the front only when Windows lets it without input.
    Never,
}

/// Injects a bare Control down-then-up tap, which satisfies the foreground
/// lock's input heuristic before `SetForegroundWindow` (see
/// `verbatim-gui`'s `foreground` module for why Control and not Alt).
fn nudge_foreground_lock() {
    send_keys(&[(VK_CONTROL, false), (VK_CONTROL, true)]);
}

/// Lets the process `pid`, just created and not yet running, take the
/// foreground with its first window, as a program a user starts does. A
/// program the agent starts would otherwise open its window under the
/// foreground lock: refused the foreground, it still announces itself (the
/// console host raises its focus events), and when the harness then brings
/// the window forward no new event says so, so a screen reader that, like
/// NVDA, dropped the refused window's events never hears of it. The right
/// is granted at once when the agent may set the foreground itself, and
/// otherwise after the nudge that lets it, when `nudge` allows one.
pub(crate) fn allow_foreground(pid: u32, nudge: ForegroundNudge) {
    // SAFETY: plain calls taking a process id; failure is reported by the
    // return value alone.
    if unsafe { AllowSetForegroundWindow(pid) }.is_ok() || nudge == ForegroundNudge::Never {
        return;
    }
    nudge_foreground_lock();
    // SAFETY: as above.
    if let Err(error) = unsafe { AllowSetForegroundWindow(pid) } {
        tracing::debug!(pid, %error, "the launched process may not take the foreground");
    }
}

/// Brings `window` to the foreground: directly after the nudge, else while
/// attached to the current foreground thread's input queue, detaching at
/// once. Nothing is done when `window` no longer belongs to one of `pids`:
/// it was destroyed since it was found, and its handle may now name an
/// unrelated window.
fn force_foreground(window: HWND, pids: &[u32]) {
    let mut owner = 0u32;
    // SAFETY: tolerates any handle, writing 0 for an invalid one.
    unsafe { GetWindowThreadProcessId(window, Some(&raw mut owner)) };
    if !pids.contains(&owner) {
        return;
    }
    nudge_foreground_lock();
    // Every call below tolerates a stale handle, and the window's owner was
    // checked just above.
    // SAFETY: plain window calls taking a handle and a command.
    let _ = unsafe { ShowWindow(window, SW_SHOW) };
    // SAFETY: as above.
    let _ = unsafe { BringWindowToTop(window) };
    // SAFETY: as above.
    if unsafe { SetForegroundWindow(window) }.as_bool() {
        return;
    }
    // SAFETY: no preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        return;
    }
    // SAFETY: tolerates any handle, returning 0 for an invalid one.
    let foreground_thread = unsafe { GetWindowThreadProcessId(foreground, None) };
    // SAFETY: no preconditions.
    let our_thread = unsafe { GetCurrentThreadId() };
    if foreground_thread == 0 || foreground_thread == our_thread {
        return;
    }
    // SAFETY: two thread ids; the attachment is undone just below.
    if unsafe { AttachThreadInput(our_thread, foreground_thread, true) }.as_bool() {
        // SAFETY: as for the calls above.
        let _ = unsafe { SetForegroundWindow(window) };
        // SAFETY: as above.
        let _ = unsafe { BringWindowToTop(window) };
        // SAFETY: undoes the attachment made above.
        let _ = unsafe { AttachThreadInput(our_thread, foreground_thread, false) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_with_no_process_reports_not_taken() {
        let taken = bring_to_foreground(
            "verbatim-agent-test-nonexistent-image-name.exe",
            None,
            Duration::from_millis(100),
        )
        .expect("an unmatched name is not an error");
        assert!(!taken);
    }
}
