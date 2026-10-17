//! Two ways to hold the window thread until `release`, for the tests of the
//! lock per application (`docs/design/focus-pipeline.md`, section 20.7):
//!
//! - `hold-setup`: the window thread's next COM modal loop waits until
//!   `release`, dispatching only the messages other threads send it, so a
//!   test can keep UIA's one-time setup inside mockapp in progress for as
//!   long as it wants. The first remote operation any client runs in an
//!   application makes UIA set itself up there, a `CoCreateInstance` in
//!   COM's modal loop on the application's UI thread. A COM message filter
//!   registered on the window thread is told of every message that arrives
//!   while that thread waits in a modal loop (`MessagePending`); once the
//!   hold is armed, the first such call prints `held modal-loop` on stdout
//!   and waits there for `release`. A message must arrive for the filter to
//!   be called, so while the hold is armed a thread of mockapp's posts the
//!   window `WM_NULL` every millisecond; the window procedure takes those
//!   outside a modal loop, and they do nothing.
//! - `stall-after-focus-read <n>`: the `n`th read of a node's
//!   `HasKeyboardFocus` property from now, once answered, posts the window a
//!   stall that lasts until
//!   `release`: the window thread takes no message meanwhile, as a busy
//!   application's UI thread does. It prints `stall started` as it begins
//!   and `stall message arrived` once when the first message is posted or
//!   sent to the thread meanwhile: a client's next call, waiting for the
//!   thread. Its end is `release`'s own acknowledgement.

// The `#[implement]`-generated COM object's glue, which this module holds,
// as mockapp's providers' modules allow it.
#![allow(clippy::inline_always, clippy::ref_as_ptr)]

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, Once, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Media::Audio::{CoRegisterMessageFilter, IMessageFilter, IMessageFilter_Impl};
use windows::Win32::Media::HTASK;
use windows::Win32::System::Com::INTERFACEINFO;
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MSG, MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, MsgWaitForMultipleObjectsEx, PM_NOREMOVE,
    PM_QS_SENDMESSAGE, PeekMessageW, PostMessageW, QS_POSTMESSAGE, QS_SENDMESSAGE, WM_APP, WM_NULL,
};
use windows::core::{PCWSTR, implement};

/// Posted to the window by the read `stall-after-focus-read` armed: the
/// stall until `release`.
pub(crate) const WM_APP_STALL_UNTIL_RELEASE: u32 = WM_APP + 6;

/// Signaled by `release`; reset as a hold is armed. Manual-reset.
fn released_event() -> HANDLE {
    static EVENT: OnceLock<OwnedHandle> = OnceLock::new();
    let event = EVENT.get_or_init(|| {
        // SAFETY: an unnamed manual-reset event, not signaled, with no
        // security attributes.
        let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .expect("mockapp's release event");
        // SAFETY: just returned, owned by nothing else.
        unsafe { OwnedHandle::from_raw_handle(event.0) }
    });
    HANDLE(event.as_raw_handle())
}

/// Readies `release` for a hold armed now.
fn arm_release() {
    // SAFETY: the event lives as long as the process.
    let _ = unsafe { ResetEvent(released_event()) };
}

/// Lets a hold of this module's go on: the `release` command, which the
/// stdin thread applies.
pub(crate) fn release() {
    // SAFETY: the event lives as long as the process.
    let _ = unsafe { SetEvent(released_event()) };
}

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

/// How many `HasKeyboardFocus` reads are still to come before the one
/// after which the window thread stalls; 0 for none armed.
static STALL_AFTER_FOCUS_READ: AtomicU32 = AtomicU32::new(0);

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
            let released = released_event();
            loop {
                // SAFETY: the release event, open for the process's life;
                // wakes for it or for a message sent to this thread.
                let woke = unsafe {
                    MsgWaitForMultipleObjectsEx(
                        Some(&[released]),
                        INFINITE,
                        QS_SENDMESSAGE,
                        MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0),
                    )
                };
                if woke == WAIT_OBJECT_0 {
                    break;
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
pub(crate) fn arm_setup_hold(hwnd: HWND) -> windows::core::Result<()> {
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
    arm_release();
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

/// Arms the stall after the `reads`th `HasKeyboardFocus` read from now.
pub(crate) fn arm_stall_after_focus_read(reads: u32) {
    arm_release();
    STALL_AFTER_FOCUS_READ.store(reads, Ordering::SeqCst);
}

/// Called as a node's `HasKeyboardFocus` is read, on the window thread of
/// `hwnd`: posts the stall when this is the read it is armed for, which the
/// window thread takes once the read is answered.
pub(crate) fn focus_read(hwnd: HWND) {
    let counted = STALL_AFTER_FOCUS_READ.try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
        left.checked_sub(1)
    });
    if counted == Ok(1) {
        // SAFETY: a posted message with no pointers, to the window whose
        // node is read.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_APP_STALL_UNTIL_RELEASE, WPARAM(0), LPARAM(0));
        }
    }
}

/// The stall, on the window thread: takes no message until `release`,
/// saying when the first one reaches the thread.
pub(crate) fn stall_until_release() {
    crate::window::acknowledge("stall started");
    let released = released_event();
    // SAFETY: the release event, open for the process's life; wakes for it
    // or for a message posted or sent to this thread, which is not taken.
    let woke = unsafe {
        MsgWaitForMultipleObjectsEx(
            Some(&[released]),
            INFINITE,
            QS_POSTMESSAGE | QS_SENDMESSAGE,
            MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0),
        )
    };
    if woke != WAIT_OBJECT_0 {
        crate::window::acknowledge("stall message arrived");
        // SAFETY: the release event, as above.
        unsafe { WaitForSingleObject(released, INFINITE) };
    }
}
