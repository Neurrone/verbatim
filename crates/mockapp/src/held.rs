//! The stall that holds the window thread until `release`, as a hung
//! application's UI thread answers no client: with
//! `stall-after-focus-read <n>`, for the tests of the lock per application
//! (`docs/design/focus-pipeline.md`, section 20.7), the `n`th read of a
//! node's `HasKeyboardFocus` property from now, once answered, posts the
//! window a stall that lasts until `release`; with `stall-until-release`,
//! the stall begins at once. It prints `stall started` as it begins, and
//! its end is `release`'s own acknowledgement, after which it prints
//! nothing. Its `release` is readied as its command is read, so a
//! `release` written after the command ends it even before it has begun:
//! it then prints nothing at all, and the thread does not stall.
//!
//! The stall holds every message sent to the thread from another thread,
//! to any window on it: mockapp's own, the native controls' (buttons, edit
//! and list views), COM's `OleMainThreadWndClass` and the input method's.
//! A thread hook (`WH_CALLWNDPROC`), which Windows calls on the thread
//! before any window procedure is given a sent message, holds each one
//! until `release` ([`hold_if_stalled`]); the window procedures need do
//! nothing. Meanwhile the thread goes on taking the messages sent to it,
//! each held in turn inside the one before, so held messages are answered
//! in the reverse order of their arrival once `release` comes. Three kinds
//! of sent message are answered and not reported: the system broadcasts
//! Windows sends every top-level window when a setting changes
//! ([`BROADCASTS`]), such as the `WM_SETTINGCHANGE` for
//! `SPI_SETSCREENREADER` when a screen reader starts or stops; a message
//! the thread sends itself, as it may while it answers a broadcast; and a
//! message sent with `SendNotifyMessage`, such as a registered message
//! broadcast to every window (`TaskbarCreated`), since its sender waits for
//! no answer. No posted message is taken.
//!
//! What the stall says while it lasts ([`Reports`]): with
//! `stall-until-release`, `stall message sent <message> to <class>` for
//! each message it holds; with `stall-until-release posted`, and after a
//! focus read, also `stall message posted <id> to <class>` for the first
//! message posted to it whose number is not one of mockapp's own posts
//! (`WM_APP` to `WM_APP + 0xFF`), a timer's or a hot key's, as a remote
//! operation's arrival is posted; and with `stall-until-release quiet`, the
//! thread takes no message at all and says nothing between its two lines.
//!
//! Windows counts a stalled thread's windows as hung (`IsHungAppWindow`)
//! once it has taken no message for 5 seconds, as it does a real hang's,
//! so a sender using `SMTO_ABORTIFHUNG` gives up on them at once from
//! then on. A stall takes a message only when one is sent to it: with
//! nothing sent, `stall-until-release` and `stall-until-release quiet`
//! were each counted hung by the check 5 or 6 seconds into the stall, and
//! not by the one at 4 seconds, in 5 trials of each on a test's desktop
//! (checked once a second, on 2026-10-18).

use std::cell::Cell;
use std::io::Write as _;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::DataExchange::GetClipboardFormatNameW;
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentThreadId, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CWPSTRUCT, CallNextHookEx, GetClassNameW, GetQueueStatus, HC_ACTION, HHOOK, ISMEX_NOTIFY,
    InSendMessageEx, MSG, MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, MsgWaitForMultipleObjectsEx,
    PM_NOREMOVE, PM_QS_POSTMESSAGE, PM_QS_SENDMESSAGE, PeekMessageW, PostMessageW, QS_POSTMESSAGE,
    QS_SENDMESSAGE, SetWindowsHookExW, UnhookWindowsHookEx, WH_CALLWNDPROC, WM_APP, WM_COMPACTING,
    WM_DEVICECHANGE, WM_DEVMODECHANGE, WM_DISPLAYCHANGE, WM_DWMCOLORIZATIONCOLORCHANGED,
    WM_DWMCOMPOSITIONCHANGED, WM_FONTCHANGE, WM_GETOBJECT, WM_HOTKEY, WM_PALETTECHANGED,
    WM_POWERBROADCAST, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WM_THEMECHANGED, WM_TIMECHANGE,
    WM_TIMER, WM_USERCHANGED,
};
use windows::core::PCWSTR;

/// Posted to the window by the read `stall-after-focus-read` armed: the
/// stall until `release`.
pub(crate) const WM_APP_STALL_UNTIL_RELEASE: u32 = WM_APP + 6;

/// The posted messages a stall never names, by number: the timer messages,
/// `WM_TIMER` (0x0113) to 0x0118, which `PeekMessageW` makes up for a timer
/// that is due; `WM_HOTKEY` (0x0312); and the messages mockapp posts to
/// itself, `WM_APP` to `WM_APP + 0xFF`. No client's arrival is posted with
/// these numbers.
const UNNAMED_POSTS: [std::ops::RangeInclusive<u32>; 3] = [
    WM_TIMER..=0x0118,
    WM_HOTKEY..=WM_HOTKEY,
    WM_APP..=WM_APP + 0xFF,
];

/// The system broadcasts a stall answers and does not report: the
/// messages Windows documents as sent to every top-level window, on the
/// tests' desktops too, when a setting, the theme, the colours, the
/// display, the fonts, the time, a device, a device's mode, the power
/// state, the desktop window manager's composition or colour, the
/// palette, the signed-in user or the memory left changes, at times no
/// test chooses. It is a known set, not a rule: any
/// other message sent to a stalled thread is held and reported, so a test
/// that does not expect it fails on the line rather than passing over a
/// broadcast missing here.
pub(crate) const BROADCASTS: [u32; 14] = [
    WM_SETTINGCHANGE,
    WM_THEMECHANGED,
    WM_SYSCOLORCHANGE,
    WM_DISPLAYCHANGE,
    WM_FONTCHANGE,
    WM_TIMECHANGE,
    WM_DEVICECHANGE,
    WM_DEVMODECHANGE,
    WM_POWERBROADCAST,
    WM_DWMCOMPOSITIONCHANGED,
    WM_DWMCOLORIZATIONCOLORCHANGED,
    WM_PALETTECHANGED,
    WM_USERCHANGED,
    WM_COMPACTING,
];

/// Signaled by `release`; reset as a stall's command is read, on the
/// stdin thread, which applies `release` too ([`arm_release`]), so a
/// `release` written after the command is never reset. Manual-reset.
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

/// Readies `release` for the stall whose command the stdin thread has just
/// read: called on that thread, before the command is handed to the window
/// thread, so it resets only a `release` written before the command, and a
/// `release` written after it ends the stall even before it has begun.
pub(crate) fn arm_release() {
    // SAFETY: the event lives as long as the process.
    let _ = unsafe { ResetEvent(released_event()) };
}

/// Whether `release` has come.
fn is_released() -> bool {
    // SAFETY: the event lives as long as the process; a wait of no time.
    unsafe { WaitForSingleObject(released_event(), 0) == WAIT_OBJECT_0 }
}

/// Lets the stall go on, and acknowledges it with `applied`: the `release`
/// command, which the stdin thread applies. The event is signaled and the
/// line written under the lock of standard output that [`report`] takes,
/// so no line of the stall's can follow `applied`.
pub(crate) fn release() {
    let _out = std::io::stdout().lock();
    crate::hits::release();
    // SAFETY: the event lives as long as the process.
    let _ = unsafe { SetEvent(released_event()) };
    crate::window::acknowledge("applied");
}

/// Writes one of the stall's lines, unless `release` has come: checked
/// under the lock of standard output that [`release`] takes.
fn report(line: &str) {
    let mut out = std::io::stdout().lock();
    if is_released() {
        return;
    }
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// How many `HasKeyboardFocus` reads are still to come before the one
/// after which the window thread stalls; 0 for none armed.
static STALL_AFTER_FOCUS_READ: AtomicU32 = AtomicU32::new(0);

/// Arms the stall after the `reads`th `HasKeyboardFocus` read from now.
pub(crate) fn arm_stall_after_focus_read(reads: u32) {
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

/// What a stall says while it lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reports {
    /// Each message it holds.
    Sends,
    /// Each message it holds, and the first message posted to the thread
    /// that [`first_client_post`] names.
    SendsAndFirstPost,
    /// Nothing, for a test whose evidence comes from elsewhere: the thread
    /// takes no message at all.
    Nothing,
}

thread_local! {
    /// What the stall under way on this thread reports; `None` when there
    /// is none, or once `release` has come.
    static STALL: Cell<Option<Reports>> = const { Cell::new(None) };
    /// Whether the stall is still to name the first message posted to the
    /// thread.
    static FIRST_POST: Cell<bool> = const { Cell::new(false) };
}

/// The stall `stall-until-release` begins at once, on the window thread,
/// and ends at `release`, saying what `reports` asks for. A tree view's
/// window thread was once posted a message of unknown origin during the
/// stall, so posts are reported only when asked for; and only the first,
/// since a remote operation's arrival is posted once or twice.
pub(crate) fn stall_now_until_release(reports: Reports) {
    if begin() {
        stall(reports);
    }
}

/// The stall after a focus read, on the window thread: holds every
/// client's message until `release`, naming each one held and the first
/// posted, as `stall-until-release posted` does.
pub(crate) fn stall_until_release() {
    if begin() {
        stall(Reports::SendsAndFirstPost);
    }
}

/// Begins a stall, printing `stall started`, unless `release` has already
/// come: checked under the lock of standard output that [`release`] takes,
/// so the line never follows `applied`. False when `release` has come, and
/// the thread then goes on with no stall.
fn begin() -> bool {
    let mut out = std::io::stdout().lock();
    if is_released() {
        return false;
    }
    let _ = writeln!(out, "stall started");
    let _ = out.flush();
    true
}

/// Holds the window thread until `release`, as the module documentation
/// describes, saying what `reports` asks for.
fn stall(reports: Reports) {
    let released = released_event();
    if reports == Reports::Nothing {
        // SAFETY: the release event, open for the process's life.
        unsafe { WaitForSingleObject(released, INFINITE) };
        return;
    }
    // SAFETY: no preconditions.
    let thread = unsafe { GetCurrentThreadId() };
    // SAFETY: a hook on this thread alone, whose procedure is this
    // module's; no module handle is needed for one.
    let hook = match unsafe { SetWindowsHookExW(WH_CALLWNDPROC, Some(call_wnd_proc), None, thread) }
    {
        Ok(hook) => hook,
        Err(error) => {
            report(&format!("stall failed: no hook, {error}"));
            // SAFETY: as above.
            unsafe { WaitForSingleObject(released, INFINITE) };
            return;
        }
    };
    STALL.set(Some(reports));
    FIRST_POST.set(reports == Reports::SendsAndFirstPost);
    wait_for_release();
    STALL.set(None);
    unhook(hook);
}

/// Removes the stall's hook.
fn unhook(hook: HHOOK) {
    // SAFETY: the hook `stall` set, removed once, after every call of its
    // procedure has returned.
    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

/// Waits on the window thread until `release`, taking each message sent to
/// the thread, which the hook holds, and naming the first client's post
/// when that is still to do; this runs again inside each held message.
/// Every turn first looks at what already waits in the queue, which
/// marks it as seen, and only then waits for what arrives after, so no
/// message waiting with another goes unseen. A wait that fails is
/// reported, and the thread then takes no message until `release`.
fn wait_for_release() {
    let released = released_event();
    loop {
        if is_released() {
            STALL.set(None);
            return;
        }
        // What waits in the queue, in the high word; the call marks the
        // queue as looked at, taking nothing.
        // SAFETY: no preconditions.
        let waiting = unsafe { GetQueueStatus(QS_SENDMESSAGE | QS_POSTMESSAGE) } >> 16;
        if waiting & QS_SENDMESSAGE.0 != 0 {
            take_sent();
            continue;
        }
        if FIRST_POST.get() && waiting & QS_POSTMESSAGE.0 != 0 {
            note_first_post();
        }
        // SAFETY: the release event, open for the process's life; wakes for
        // it or for a message sent or posted to this thread since the queue
        // was looked at above, which is not taken.
        let woke = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[released]),
                INFINITE,
                QS_SENDMESSAGE | QS_POSTMESSAGE,
                MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0),
            )
        };
        if woke.0 > WAIT_OBJECT_0.0 + 1 {
            let error = windows::core::Error::from_thread();
            report(&format!("stall failed: {woke:?}, {error}"));
            // SAFETY: as above.
            unsafe { WaitForSingleObject(released, INFINITE) };
        }
    }
}

/// Takes every message sent to the thread, each given to the hook, which
/// holds it.
fn take_sent() {
    let mut message = MSG::default();
    // SAFETY: `message` is a local the call writes; it takes each message
    // sent to the thread, calling its window procedure, and nothing
    // posted.
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

/// Called by Windows on the window thread, during a stall, before a
/// window procedure is given a sent message: holds it when
/// [`hold_if_stalled`] says to, and then hands it on.
///
/// # Safety
///
/// Installed only as a `WH_CALLWNDPROC` hook procedure by [`stall`];
/// Windows passes a live `CWPSTRUCT` in `lparam` for `HC_ACTION`.
unsafe extern "system" fn call_wnd_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if u32::try_from(code) == Ok(HC_ACTION) {
        // SAFETY: for `HC_ACTION`, `lparam` points to the message's
        // `CWPSTRUCT`, live for this call.
        let sent = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        // Nonzero for a message another thread sent, zero for one this
        // thread sent itself (checked on 2026-10-18; the documentation
        // words it the other way round).
        hold_if_stalled(sent, wparam.0 != 0);
    }
    // SAFETY: hands the message on, as every hook procedure must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// During a stall, holds `sent` until `release`, reporting it as the
/// stall's [`Reports`] ask: `stall message sent <message> to <class>`,
/// where `<message>` is `WM_GETOBJECT` with its object id, a registered
/// message's name, or the message's number. Three kinds are answered and
/// not reported: a broadcast of [`BROADCASTS`]; a message this thread
/// sent itself (`from_another_thread` false), as a window procedure
/// answering a broadcast may send to the thread's other windows; and a
/// message another thread sent with `SendNotifyMessage`, which waits for
/// no answer. Messages sent meanwhile are taken in turn, each held inside
/// this one.
fn hold_if_stalled(sent: &CWPSTRUCT, from_another_thread: bool) {
    let Some(reports) = STALL.get() else {
        return;
    };
    if !from_another_thread || BROADCASTS.contains(&sent.message) {
        return;
    }
    // How another thread sent it: `ISMEX_NOTIFY` for `SendNotifyMessage`.
    // `InSendMessageEx` describes the message from another thread that the
    // thread is answering, which, for a message the thread sends itself, is
    // the one it is answering as it sends, so it cannot tell the thread's
    // own sends apart; the hook's `from_another_thread` does (both checked
    // on 2026-10-18).
    // SAFETY: no preconditions; describes the sent message being handled.
    let how = unsafe { InSendMessageEx(None) };
    if how & ISMEX_NOTIFY != 0 {
        return;
    }
    match reports {
        Reports::Sends | Reports::SendsAndFirstPost => report(&format!(
            "stall message sent {} to {}",
            message_name(sent.message, sent.lParam),
            class_name(sent.hwnd)
        )),
        Reports::Nothing => {}
    }
    wait_for_release();
}

/// How the stall names a message: `WM_GETOBJECT` with its object id, a
/// registered message's name, or the message's number.
fn message_name(message: u32, lparam: LPARAM) -> String {
    if message == WM_GETOBJECT {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "idObject is 32 bits by convention"
        )]
        return format!("WM_GETOBJECT {}", lparam.0 as i32);
    }
    if (0xC000..=0xFFFF).contains(&message) {
        let mut name = [0u16; 256];
        // SAFETY: a registered message's id names an atom; a local buffer.
        let length = unsafe { GetClipboardFormatNameW(message, &mut name) };
        let length = usize::try_from(length).unwrap_or(0);
        if length > 0 {
            return String::from_utf16_lossy(&name[..length]);
        }
    }
    format!("{message:#06x}")
}

/// The class name of `hwnd`.
fn class_name(hwnd: HWND) -> String {
    let mut class = [0u16; 256];
    // SAFETY: a window handle, which the call checks; a local buffer.
    let length = unsafe { GetClassNameW(hwnd, &mut class) };
    let length = usize::try_from(length).unwrap_or(0);
    String::from_utf16_lossy(&class[..length])
}

/// Names the first message posted to the window thread, once, if one
/// waits, passing over the numbers in [`UNNAMED_POSTS`], such as
/// mockapp's wake-up for a command written during the stall:
/// `stall message posted <id> to <class>`, the message and the class of
/// the window it was posted to (`no window` for a thread message). It is
/// read with `PeekMessageW`, which takes nothing posted with `PM_NOREMOVE`, but,
/// whatever its filter, takes every message sent to the thread before it
/// looks (checked on 2026-10-18), each held by the hook, inside which the
/// post may already have been named; so whether it is still to name is
/// asked again once the call returns.
fn note_first_post() {
    let Some(posted) = first_client_post() else {
        return;
    };
    if !FIRST_POST.replace(false) {
        return;
    }
    let class = if posted.hwnd.is_invalid() {
        "no window".to_owned()
    } else {
        class_name(posted.hwnd)
    };
    report(&format!(
        "stall message posted {:#06x} to {class}",
        posted.message
    ));
}

/// The earliest message waiting in the queue whose number is not in
/// [`UNNAMED_POSTS`]: of the first in each span of numbers between them,
/// the one posted first by its time.
fn first_client_post() -> Option<MSG> {
    let mut first = 0;
    let mut earliest: Option<MSG> = None;
    let end = 0x1_0000..=0x1_0000;
    for unnamed in UNNAMED_POSTS.iter().chain(std::iter::once(&end)) {
        if first < *unnamed.start()
            && let Some(posted) = peek_posted(first, unnamed.start() - 1)
            && earliest.is_none_or(|earliest| posted.time < earliest.time)
        {
            earliest = Some(posted);
        }
        first = unnamed.end() + 1;
    }
    earliest
}

/// The first posted message waiting in the queue whose number is from
/// `first` to `last`, left there.
fn peek_posted(first: u32, last: u32) -> Option<MSG> {
    let mut message = MSG::default();
    // SAFETY: `message` is a local the call writes; nothing posted is
    // taken.
    unsafe {
        PeekMessageW(
            &raw mut message,
            None,
            first,
            last,
            PM_NOREMOVE | PM_QS_POSTMESSAGE,
        )
    }
    .as_bool()
    .then_some(message)
}
