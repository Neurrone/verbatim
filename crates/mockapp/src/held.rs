//! The stall that holds the window thread until `release`: with
//! `stall-after-focus-read <n>`, for the tests of the lock per application
//! (`docs/design/focus-pipeline.md`, section 20.7), the `n`th read of a
//! node's `HasKeyboardFocus` property from now, once answered, posts the
//! window a stall that lasts until `release`; with `stall-until-release`,
//! the stall begins at once. The window thread answers no client
//! meanwhile, as a busy or hung application's UI thread does. It prints
//! `stall started` as it begins, and says when a client's call reaches the
//! thread meanwhile: after a focus read, `stall message arrived` once, when
//! the first message is posted or sent to the thread; with
//! `stall-until-release`, `stall message sent <message> to <class>` for
//! each client's message sent to it, `WM_GETOBJECT` or a registered message
//! such as UI Automation's own, which the window procedure that receives it
//! holds until `release`, while any other message sent to the thread, such
//! as the `WM_SETTINGCHANGE` Windows sends every top-level window when a
//! screen reader starts or stops, is answered and not reported. With
//! `stall-until-release posted`, it also names the first message posted to
//! it, once: `stall message posted <id> to <class>`; with
//! `stall-until-release quiet`, the thread takes no message at all and it
//! says nothing between the two. Its end is `release`'s own
//! acknowledgement.

use std::cell::Cell;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WAIT_EVENT, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::DataExchange::GetClipboardFormatNameW;
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetQueueStatus, MSG, MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS,
    MsgWaitForMultipleObjectsEx, PM_NOREMOVE, PM_QS_POSTMESSAGE, PM_QS_SENDMESSAGE, PeekMessageW,
    PostMessageW, QS_POSTMESSAGE, QS_SENDMESSAGE, QUEUE_STATUS_FLAGS, WM_APP, WM_GETOBJECT,
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

/// What the stall of `stall-until-release` says while it lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reports {
    /// Each client's message sent to the thread.
    Sends,
    /// Each client's message sent to the thread, and the first message
    /// posted to it.
    SendsAndFirstPost,
    /// Nothing, for a test whose evidence comes from elsewhere: the thread
    /// takes no message at all.
    Nothing,
}

thread_local! {
    /// What the stall of `stall-until-release` under way on this thread
    /// reports; `None` when there is none.
    static STALL: Cell<Option<Reports>> = const { Cell::new(None) };
    /// Whether the stall is still to name the first message posted to the
    /// thread.
    static FIRST_POST: Cell<bool> = const { Cell::new(false) };
}

/// The stall `stall-until-release` begins at once, on the window thread,
/// and ends at `release`, saying what `reports` asks for. With
/// [`Reports::Nothing`] the thread takes no message at all. Otherwise it
/// takes each message sent to it, and the window procedures hold a
/// client's ([`hold_if_stalled`]), so a client's call still waits for the
/// thread until `release`, while a message such as the `WM_SETTINGCHANGE`
/// Windows sends every top-level window when a screen reader starts or
/// stops is answered and not reported. With
/// [`Reports::SendsAndFirstPost`] it also names the first message posted
/// to the thread, as a remote operation's arrival is posted, once or
/// twice. Posts are reported only when asked for, since a tree view's
/// window thread was once posted a message of unknown origin during the
/// stall, and only the first, since the second post for an operation comes
/// or not.
pub(crate) fn stall_now_until_release(reports: Reports) {
    arm_release();
    crate::window::acknowledge("stall started");
    STALL.set(Some(reports));
    FIRST_POST.set(reports == Reports::SendsAndFirstPost);
    wait_for_release(reports);
    STALL.set(None);
}

/// Waits on the window thread until `release`, taking each message sent to
/// the thread unless `reports` is [`Reports::Nothing`], and naming the
/// first message posted to it when that is still to do. A wait that fails
/// ends the wait, saying why.
fn wait_for_release(reports: Reports) {
    let released = released_event();
    loop {
        let kinds = match reports {
            Reports::Nothing => QUEUE_STATUS_FLAGS(0),
            _ if FIRST_POST.get() => QS_SENDMESSAGE | QS_POSTMESSAGE,
            _ => QS_SENDMESSAGE,
        };
        // SAFETY: the release event, open for the process's life; wakes for
        // it or for a message of `kinds` that reached this thread since the
        // queue was last looked at, which is not taken.
        let woke = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[released]),
                INFINITE,
                kinds,
                MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0),
            )
        };
        if woke == WAIT_OBJECT_0 {
            return;
        }
        if woke != WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
            let error = windows::core::Error::from_thread();
            crate::window::acknowledge(&format!("stall failed: {woke:?}, {error}"));
            return;
        }
        // The kinds of message that arrived since the queue was last looked
        // at, in the low word, and those waiting in it, in the high word;
        // the call marks the queue as looked at, taking nothing, so the
        // next wait wakes only for one that arrives after them.
        // SAFETY: no preconditions.
        let status = unsafe { GetQueueStatus(kinds) };
        let sent_waiting = (status >> 16) & QS_SENDMESSAGE.0 != 0;
        if FIRST_POST.get() && status & QS_POSTMESSAGE.0 != 0 {
            FIRST_POST.set(false);
            crate::window::acknowledge(&first_post(sent_waiting));
        }
        if sent_waiting {
            let mut message = MSG::default();
            // SAFETY: `message` is a local the call writes; it takes each
            // message sent to the thread, calling its window procedure,
            // and nothing posted.
            let _ = unsafe {
                PeekMessageW(
                    &raw mut message,
                    None,
                    0,
                    0,
                    PM_NOREMOVE | PM_QS_SENDMESSAGE,
                )
            };
        }
    }
}

/// Called first by each of mockapp's window procedures: during a stall of
/// `stall-until-release`, holds a client's message, `WM_GETOBJECT` or a
/// registered message such as UI Automation's own, until `release`, saying
/// `stall message sent <message> to <class>` as it arrives, where
/// `<message>` is `WM_GETOBJECT` with its object id, or the registered
/// message's name. Messages sent meanwhile are taken in turn, as the stall
/// takes them. Any other message is left to the procedure.
pub(crate) fn hold_if_stalled(hwnd: HWND, msg: u32, lparam: LPARAM) {
    let Some(reports) = STALL.get() else {
        return;
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "idObject is 32 bits by convention"
    )]
    let message = if msg == WM_GETOBJECT {
        format!("WM_GETOBJECT {}", lparam.0 as i32)
    } else if (0xC000..=0xFFFF).contains(&msg) {
        let mut name = [0u16; 256];
        // SAFETY: a registered message's id names an atom; a local buffer.
        let length = unsafe { GetClipboardFormatNameW(msg, &mut name) };
        let length = usize::try_from(length).unwrap_or(0);
        String::from_utf16_lossy(&name[..length])
    } else {
        return;
    };
    crate::window::acknowledge(&format!(
        "stall message sent {message} to {}",
        class_name(hwnd)
    ));
    wait_for_release(reports);
}

/// The class name of `hwnd`.
fn class_name(hwnd: HWND) -> String {
    let mut class = [0u16; 256];
    // SAFETY: a window handle, which the call checks; a local buffer.
    let length = unsafe { GetClassNameW(hwnd, &mut class) };
    let length = usize::try_from(length).unwrap_or(0);
    String::from_utf16_lossy(&class[..length])
}

/// The stall's line for the first message posted to the window thread:
/// `stall message posted <id> to <class>`, the message and the class of
/// the window it was posted to (`no window` for a thread message), or why
/// it could not be read. It is read with `PeekMessageW`, which takes
/// nothing posted with `PM_NOREMOVE` but, whatever its filter, takes every
/// message sent to the thread before it looks (checked on 2026-10-18: a
/// message sent and waiting was taken, and the call returned no message),
/// so it is read only when no sent message waits (`sent_waiting`); one sent
/// in between is held by its window procedure as the stall would hold it.
fn first_post(sent_waiting: bool) -> String {
    if sent_waiting {
        return "stall message posted, not read: a sent message waits".to_owned();
    }
    let mut message = MSG::default();
    // SAFETY: `message` is a local the call writes; nothing posted is
    // taken.
    let found = unsafe {
        PeekMessageW(
            &raw mut message,
            None,
            0,
            0,
            PM_NOREMOVE | PM_QS_POSTMESSAGE,
        )
    }
    .as_bool();
    if !found {
        return "stall message posted, not read: none found".to_owned();
    }
    let class = if message.hwnd.is_invalid() {
        "no window".to_owned()
    } else {
        class_name(message.hwnd)
    };
    format!("stall message posted {:#06x} to {class}", message.message)
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
