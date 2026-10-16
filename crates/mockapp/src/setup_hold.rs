//! The `hold-setup` command: holds the window thread's next COM modal loop
//! open until `release`, dispatching only the messages other threads send
//! it, so a test can keep UIA's one-time setup inside mockapp in progress
//! for as long as it wants (`docs/design/focus-pipeline.md`, section 20).
//!
//! The first remote operation any client runs in an application makes UIA
//! set itself up there, a `CoCreateInstance` in COM's modal loop on the
//! application's UI thread, during which every new UIA connection from
//! another process fails. A COM message filter registered on the window
//! thread is told of every message that arrives while that thread waits in
//! a modal loop (`MessagePending`); once the hold is armed, the first such
//! call prints `held modal-loop` on stdout and waits there for `release`.
//! A message must arrive for the filter to be called, so while the hold is
//! armed a thread of mockapp's posts the window `WM_NULL` every
//! millisecond; the window procedure's loop takes those outside a modal
//! loop, and they do nothing.

// The `#[implement]`-generated COM object's glue, which this module holds,
// as mockapp's providers' modules allow it.
#![allow(clippy::inline_always, clippy::ref_as_ptr)]

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex, Once};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::{CoRegisterMessageFilter, IMessageFilter, IMessageFilter_Impl};
use windows::Win32::Media::HTASK;
use windows::Win32::System::Com::INTERFACEINFO;
use windows::Win32::UI::WindowsAndMessaging::{
    MSG, MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, MsgWaitForMultipleObjectsEx, PM_NOREMOVE,
    PM_QS_SENDMESSAGE, PeekMessageW, PostMessageW, QS_SENDMESSAGE, WM_NULL,
};
use windows::core::implement;

/// Whether the next modal loop is held, and the condition the posting
/// thread waits on while it is not.
static ARMED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Whether the filter is waiting for `release`, so it takes no second
/// hold from a modal loop it enters while it waits.
static HOLDING: AtomicBool = AtomicBool::new(false);

/// The window `WM_NULL` is posted to.
static TARGET: AtomicIsize = AtomicIsize::new(0);

/// Registers the filter and starts the posting thread, once.
static START: Once = Once::new();

/// `MessagePending`'s answer: go on waiting in the modal loop
/// (`PENDINGMSG_WAITDEFPROCESS`).
const WAIT_DEFAULT_PROCESS: u32 = 2;

/// `HandleInComingCall`'s answer: take the call (`SERVERCALL_ISHANDLED`).
const CALL_HANDLED: u32 = 0;

/// `RetryRejectedCall`'s answer: do not retry (-1).
const NO_RETRY: u32 = u32::MAX;

#[implement(IMessageFilter)]
struct Filter;

impl IMessageFilter_Impl for Filter_Impl {
    fn HandleInComingCall(&self, _: u32, _: HTASK, _: u32, _: *const INTERFACEINFO) -> u32 {
        CALL_HANDLED
    }

    fn RetryRejectedCall(&self, _: HTASK, _: u32, _: u32) -> u32 {
        NO_RETRY
    }

    fn MessagePending(&self, _: HTASK, _: u32, _: u32) -> u32 {
        let armed = std::mem::take(
            &mut *ARMED
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if armed && !HOLDING.swap(true, Ordering::SeqCst) {
            crate::window::acknowledge("held modal-loop");
            while !crate::hits::released() {
                // SAFETY: no handles; wakes for a sent message or after a
                // millisecond.
                unsafe {
                    MsgWaitForMultipleObjectsEx(
                        None,
                        1,
                        QS_SENDMESSAGE,
                        MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0),
                    );
                }
                let mut msg = MSG::default();
                // SAFETY: dispatches the messages sent to this thread, and
                // removes nothing posted.
                let _ = unsafe {
                    PeekMessageW(&raw mut msg, None, 0, 0, PM_NOREMOVE | PM_QS_SENDMESSAGE)
                };
            }
            HOLDING.store(false, Ordering::SeqCst);
        }
        WAIT_DEFAULT_PROCESS
    }
}

/// Arms the hold of the window thread's next COM modal loop: called on the
/// window thread, `hwnd`'s.
///
/// # Errors
///
/// The COM error when the filter cannot be registered.
pub(crate) fn arm(hwnd: HWND) -> windows::core::Result<()> {
    let mut registered = Ok(());
    START.call_once(|| {
        let filter: IMessageFilter = Filter.into();
        // SAFETY: the window thread, an apartment of its own; the filter
        // stays registered for mockapp's life.
        registered = unsafe { CoRegisterMessageFilter(&filter, None) };
        TARGET.store(hwnd.0 as isize, Ordering::SeqCst);
        std::thread::spawn(post_while_armed);
    });
    registered?;
    crate::hits::hold_next_modal_loop();
    *ARMED
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    ARMED.1.notify_all();
    Ok(())
}

/// The posting thread: while the hold is armed, posts the window `WM_NULL`
/// every millisecond, so the filter is called once the thread waits in a
/// modal loop; otherwise waits to be armed.
fn post_while_armed() {
    loop {
        let armed = ARMED
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let armed = ARMED
            .1
            .wait_while(armed, |armed| !*armed)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(armed);
        // SAFETY: a posted message with no pointers, to mockapp's window,
        // which lives as long as the process.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(TARGET.load(Ordering::SeqCst) as *mut _)),
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
            );
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
