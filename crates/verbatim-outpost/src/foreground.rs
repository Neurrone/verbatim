//! The foreground trigger (architecture section 1, risk R2 pre-spawning).
//!
//! A dedicated thread installs a global out-of-context
//! `EVENT_SYSTEM_FOREGROUND` hook and pumps messages, reporting only the
//! `(pid, hwnd)` of each new foreground window. It does no property fetches on
//! this thread; the supervisor uses the pid to retarget its single outpost.

use std::cell::RefCell;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, GetWindowThreadProcessId, MSG,
    PostThreadMessageW, TranslateMessage, WINEVENT_OUTOFCONTEXT, WM_QUIT,
};

use crossbeam_channel::{Sender, unbounded};

/// Called on the trigger thread with the pid and window handle of each new
/// foreground window. Must not block.
pub type ForegroundCallback = Arc<dyn Fn(u32, isize) + Send + Sync>;

thread_local! {
    static CALLBACK: RefCell<Option<ForegroundCallback>> = const { RefCell::new(None) };
}

/// A live foreground hook and its thread. Dropping it unhooks and stops.
pub struct ForegroundTrigger {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl ForegroundTrigger {
    /// Installs the global foreground hook on a dedicated thread, reporting to
    /// `callback`.
    ///
    /// # Panics
    ///
    /// Panics if the trigger thread cannot be spawned, which indicates the
    /// process is out of OS thread resources.
    #[must_use]
    pub fn new(callback: ForegroundCallback) -> Self {
        let (id_tx, id_rx) = unbounded::<u32>();
        let join = thread::Builder::new()
            .name("verbatim-foreground".to_owned())
            .spawn(move || trigger_main(&id_tx, callback))
            .expect("spawn foreground trigger thread");
        let thread_id = id_rx.recv().unwrap_or(0);
        Self {
            thread_id,
            join: Some(join),
        }
    }
}

impl Drop for ForegroundTrigger {
    fn drop(&mut self) {
        // SAFETY: WM_QUIT ends the trigger thread's message loop.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn trigger_main(id_tx: &Sender<u32>, callback: ForegroundCallback) {
    // SAFETY: GetCurrentThreadId is always sound.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = id_tx.send(thread_id);
    CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));

    // SAFETY: a global out-of-context foreground hook with a same-process proc.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };

    let mut message = MSG::default();
    loop {
        // SAFETY: standard message loop over a fully owned MSG.
        let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        // SAFETY: dispatching a fully owned message.
        unsafe {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }

    if !hook.0.is_null() {
        // SAFETY: `hook` came from a successful SetWinEventHook.
        unsafe {
            let _ = UnhookWinEvent(hook);
        }
    }
    CALLBACK.with(|slot| *slot.borrow_mut() = None);
}

unsafe extern "system" fn foreground_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd.0.is_null() {
        return;
    }
    // SAFETY: GetWindowThreadProcessId tolerates any window handle.
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&raw mut pid));
    }
    CALLBACK.with(|slot| {
        if let Some(callback) = slot.borrow().as_ref() {
            callback(pid, hwnd.0 as isize);
        }
    });
}
