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
//! events the thread blocks in `GetMessageW`, and a thread timer bounds a
//! wait that never sees its evidence.

use std::cell::Cell;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE,
    EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND,
    EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, GetMessageW, KillTimer, MSG,
    OBJID_WINDOW, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetTimer, WINEVENT_OUTOFCONTEXT,
    WM_APP, WM_TIMER,
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
    let milliseconds = u32::try_from(timeout.as_millis())
        .unwrap_or(u32::MAX)
        .max(1);
    // SAFETY: a thread timer with no window and no callback; it posts
    // WM_TIMER to this thread and is killed below.
    let timer = unsafe { SetTimer(None, 0, milliseconds, None) };
    let held = loop {
        // SAFETY: `msg` is an owned buffer; no window filter.
        let read = unsafe { GetMessageW(&raw mut msg, None, 0, 0) };
        if read.0 <= 0 {
            break met();
        }
        match msg.message {
            WM_TIMER if msg.wParam.0 == timer => break met(),
            WM_CHANGED => {
                WAKE_POSTED.with(|posted| posted.set(false));
                if met() {
                    break true;
                }
            }
            _ => {
                // SAFETY: a message this thread read; dispatching it runs
                // nothing of this module's but the event procedure.
                unsafe {
                    DispatchMessageW(&raw const msg);
                }
            }
        }
    };
    // SAFETY: the timer set above.
    unsafe {
        let _ = KillTimer(None, timer);
    }
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
