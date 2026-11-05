//! An out-of-context `WinEvent` hook, as an MSAA client and Verbatim's
//! listener hold them: a thread of its own that pumps messages, so the
//! system can deliver each event to it, and reports each event heard with
//! the time it was heard.

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_QUIT,
};

/// One `WinEvent` heard.
#[derive(Clone, Copy, Debug)]
pub struct WinEvent {
    /// The event's number, such as `EVENT_OBJECT_FOCUS`.
    pub event: u32,
    /// The window it names.
    pub hwnd: isize,
    /// When the hook ran, on [`crate::now_us`]'s clock.
    pub at: u64,
}

thread_local! {
    static SINK: RefCell<Option<Sender<WinEvent>>> = const { RefCell::new(None) };
}

/// A hook running on a thread of its own; dropping it unhooks and ends the
/// thread.
pub struct Hook {
    thread: u32,
    join: Option<JoinHandle<()>>,
}

impl Hook {
    /// Hooks events `min` to `max` from process `pid` (0 for every
    /// process but this one) and answers the hook and its events.
    ///
    /// # Panics
    ///
    /// When the hook cannot be set.
    #[must_use]
    pub fn start(min: u32, max: u32, pid: u32) -> (Self, Receiver<WinEvent>) {
        let (events, heard) = channel();
        let (started, is_started) = channel();
        let join = std::thread::spawn(move || {
            SINK.with(|sink| *sink.borrow_mut() = Some(events));
            // SAFETY: an out-of-context hook with a callback that lives for
            // the whole program, on this thread, which pumps below.
            let hook = unsafe {
                SetWinEventHook(
                    min,
                    max,
                    None,
                    Some(callback),
                    pid,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            assert!(!hook.is_invalid(), "the WinEvent hook is set");
            // SAFETY: no precondition.
            let _ = started.send(unsafe { GetCurrentThreadId() });
            let mut msg = MSG::default();
            // SAFETY: a live structure for each call.
            while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.as_bool() {
                // SAFETY: as above, a message GetMessageW filled.
                let _ = unsafe { TranslateMessage(&raw const msg) };
                // SAFETY: as above.
                unsafe { DispatchMessageW(&raw const msg) };
            }
            // SAFETY: the hook set above, unhooked once on its own thread.
            let _ = unsafe { UnhookWinEvent(hook) };
        });
        let thread = is_started.recv().expect("the hook thread starts");
        (
            Self {
                thread,
                join: Some(join),
            },
            heard,
        )
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // SAFETY: the hook thread's id; it pumps until WM_QUIT.
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

extern "system" fn callback(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let at = crate::now_us();
    SINK.with(|sink| {
        if let Some(sink) = &*sink.borrow() {
            let _ = sink.send(WinEvent {
                event,
                hwnd: hwnd.0 as isize,
                at,
            });
        }
    });
}
