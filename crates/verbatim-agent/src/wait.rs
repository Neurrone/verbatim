//! Waiting for evidence about the desktop's windows, for
//! [`Request::WaitForWindow`](crate::protocol::Request::WaitForWindow) and
//! everything else in the agent that waits for a window to appear, close,
//! or take the foreground.
//!
//! A wait checks its condition once, then installs out-of-context
//! `WinEvent` hooks for the events that can change it (a top-level window
//! created, destroyed, shown, hidden, renamed, cloaked or uncloaked,
//! minimized or restored, or brought to the foreground) and checks again
//! each time one arrives. Nothing is polled and nothing sleeps: between
//! events the thread blocks in `MsgWaitForMultipleObjectsEx` until a
//! message comes or the wait's time runs out. No thread timer bounds it,
//! since `KillTimer` leaves a `WM_TIMER` already in the queue, which ended
//! the next wait at once when a wait ended on an event as its timer fired.

use std::cell::Cell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE,
    EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND,
    EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, OBJID_WINDOW, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
    PostThreadMessageW, QS_ALLINPUT, WINEVENT_OUTOFCONTEXT, WM_APP,
};

/// The message an event hook posts to wake the waiting thread.
const WM_CHANGED: u32 = WM_APP + 1;

thread_local! {
    /// Whether a wake-up is already posted and not yet read, so a burst of
    /// events posts one.
    static WAKE_POSTED: Cell<bool> = const { Cell::new(false) };
}

/// The event ranges a wait listens to.
const RANGES: [(u32, u32); 5] = [
    (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
    (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
    (EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE),
    (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
    (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
];

/// Installed hooks, removed on drop.
struct Hooks(Vec<HWINEVENTHOOK>);

impl Drop for Hooks {
    fn drop(&mut self) {
        for hook in &self.0 {
            // SAFETY: each hook was installed by this thread and is removed
            // once.
            let _ = unsafe { UnhookWinEvent(*hook) };
        }
    }
}

/// A window-level event (not one about a child object) wakes the wait.
unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _window: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW.0 || child != i32::try_from(CHILDID_SELF).unwrap_or(0) {
        return;
    }
    if WAKE_POSTED.with(|posted| posted.replace(true)) {
        return;
    }
    // SAFETY: no preconditions.
    let thread = unsafe { GetCurrentThreadId() };
    // SAFETY: posts a plain message to this thread's own queue.
    unsafe {
        let _ = PostThreadMessageW(thread, WM_CHANGED, WPARAM(0), LPARAM(0));
    }
}

/// Waits up to `timeout` for `met` to return true, checking it now and
/// again after each window event. Returns whether it held when the wait
/// ended.
pub fn until(mut met: impl FnMut() -> bool, timeout: Duration) -> bool {
    if met() {
        return true;
    }
    // Creates this thread's message queue, so the hooks' posts land.
    let mut msg = MSG::default();
    // SAFETY: `msg` is an owned buffer; nothing is removed from the queue.
    unsafe {
        let _ = PeekMessageW(&raw mut msg, None, 0, 0, PM_NOREMOVE);
    }
    let hooks = Hooks(
        RANGES
            .iter()
            .filter_map(|&(first, last)| {
                // SAFETY: `on_event` has the hook procedure's signature and
                // lives for the process; out of context, it runs on this
                // thread while it reads messages.
                let hook = unsafe {
                    SetWinEventHook(
                        first,
                        last,
                        None,
                        Some(on_event),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT,
                    )
                };
                (!hook.is_invalid()).then_some(hook)
            })
            .collect(),
    );
    // Checked again now the hooks are in: a change between the first check
    // and the hooks would otherwise go unseen.
    if met() {
        return true;
    }
    let deadline = Instant::now() + timeout;
    let held = 'wait: loop {
        // Every message waiting is read first: the hooks' events are
        // delivered as they are.
        // SAFETY: `msg` is an owned buffer; no window filter.
        while unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_CHANGED {
                WAKE_POSTED.with(|posted| posted.set(false));
                if met() {
                    break 'wait true;
                }
            } else {
                // SAFETY: a message this thread read; dispatching it runs
                // nothing of this module's but the event procedure.
                unsafe {
                    DispatchMessageW(&raw const msg);
                }
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break met();
        }
        let milliseconds = u32::try_from(left.as_millis()).unwrap_or(u32::MAX).max(1);
        // SAFETY: no handles; returns when a message is in the queue or the
        // time is up, and a message already there counts.
        unsafe {
            let _ =
                MsgWaitForMultipleObjectsEx(None, milliseconds, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    };
    drop(hooks);
    // A wake-up posted after the last read is left for the next wait to
    // read, which treats it as a reason to check.
    WAKE_POSTED.with(|posted| posted.set(false));
    held
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_condition_that_holds_at_once_needs_no_event() {
        assert!(until(|| true, Duration::from_millis(1)));
    }

    #[test]
    fn a_timer_message_left_in_the_queue_does_not_end_a_wait() {
        // A `WM_TIMER` already in the queue, as `KillTimer` leaves one.
        // SAFETY: no preconditions.
        let thread = unsafe { GetCurrentThreadId() };
        let mut msg = MSG::default();
        // SAFETY: creates this thread's queue; nothing is removed.
        unsafe {
            let _ = PeekMessageW(&raw mut msg, None, 0, 0, PM_NOREMOVE);
        }
        for id in 0..8 {
            // SAFETY: a plain message to this thread's own queue.
            unsafe {
                let _ = PostThreadMessageW(
                    thread,
                    windows::Win32::UI::WindowsAndMessaging::WM_TIMER,
                    WPARAM(id),
                    LPARAM(0),
                );
            }
        }
        let timeout = Duration::from_millis(100);
        let started = Instant::now();
        assert!(!until(|| false, timeout));
        assert!(started.elapsed() >= timeout, "{:?}", started.elapsed());
    }

    #[test]
    fn a_condition_that_never_holds_ends_with_the_timeout() {
        let mut checks = 0;
        assert!(!until(
            || {
                checks += 1;
                false
            },
            Duration::from_millis(50)
        ));
        assert!(
            checks >= 3,
            "checked first, after the hooks, and at the end"
        );
    }
}
