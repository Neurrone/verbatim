//! The stall that holds the window thread until `release`: with
//! `stall-after-focus-read <n>`, for the tests of the lock per application
//! (`docs/design/focus-pipeline.md`, section 20.7), the `n`th read of a
//! node's `HasKeyboardFocus` property from now, once answered, posts the
//! window a stall that lasts until `release`; with `stall-until-release`,
//! the stall begins at once. The window thread takes no message
//! meanwhile, as a busy or hung application's UI thread does. It prints
//! `stall started` as it begins, and says when a client's call reaches the
//! thread meanwhile: after a focus read, `stall message arrived` once, when
//! the first message is posted or sent to the thread; with
//! `stall-until-release`, `stall message sent` each time a message is sent
//! to it, and with `stall-until-release posted`, also `stall message
//! posted` each time one is posted. Its end is `release`'s own
//! acknowledgement.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetQueueStatus, MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, MsgWaitForMultipleObjectsEx,
    PostMessageW, QS_POSTMESSAGE, QS_SENDMESSAGE, WM_APP,
};
use windows::core::PCWSTR;

/// Posted to the window by the read `stall-after-focus-read` armed: the
/// stall until `release`.
pub(crate) const WM_APP_STALL_UNTIL_RELEASE: u32 = WM_APP + 6;

/// Signaled by `release`; reset as the stall is armed. Manual-reset.
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

/// Readies `release` for a stall armed now.
fn arm_release() {
    // SAFETY: the event lives as long as the process.
    let _ = unsafe { ResetEvent(released_event()) };
}

/// Lets the stall go on: the `release` command, which the stdin thread
/// applies.
pub(crate) fn release() {
    // SAFETY: the event lives as long as the process.
    let _ = unsafe { SetEvent(released_event()) };
}

/// How many `HasKeyboardFocus` reads are still to come before the one
/// after which the window thread stalls; 0 for none armed.
static STALL_AFTER_FOCUS_READ: AtomicU32 = AtomicU32::new(0);

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

/// The stall `stall-until-release` begins at once, on the window thread:
/// it takes no message until `release`, saying each time a message is
/// sent to the thread, as a hung window's callers send it one after
/// another and wait, and, with `posted`, each time one is posted to it, as
/// UI Automation posts a remote operation's arrival. Posts are reported
/// only when asked for: a tree view's window thread was once posted a
/// message of unknown origin during the stall.
pub(crate) fn stall_now_until_release(posted: bool) {
    arm_release();
    crate::window::acknowledge("stall started");
    let released = released_event();
    let kinds = if posted {
        QS_SENDMESSAGE | QS_POSTMESSAGE
    } else {
        QS_SENDMESSAGE
    };
    // SAFETY: the release event, open for the process's life; wakes for it
    // or for a message of `kinds` that reached this thread since the queue
    // was last looked at, which is not taken.
    while unsafe {
        MsgWaitForMultipleObjectsEx(
            Some(&[released]),
            INFINITE,
            kinds,
            MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0),
        )
    } != WAIT_OBJECT_0
    {
        // The kinds of message that arrived since the queue was last
        // looked at, in the low word, which the call then marks as looked
        // at, taking none, so the next wait wakes only for one that arrives
        // after them: without it, the waits went on waking at once once a
        // caller had given up its call.
        // SAFETY: no preconditions.
        let arrived = unsafe { GetQueueStatus(kinds) } & 0xffff;
        if arrived & QS_POSTMESSAGE.0 != 0 {
            crate::window::acknowledge("stall message posted");
        }
        if arrived & QS_SENDMESSAGE.0 != 0 {
            crate::window::acknowledge("stall message sent");
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
