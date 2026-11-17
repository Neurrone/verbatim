//! The recorder's own `WinEvent` hooks: raw events, unfiltered, both hooks
//! on one thread running a message loop at the focus listener's priority,
//! which numbers its passes as the listener will batch them
//! ([`crate::passes`]). Verbatim's hook code in `verbatim-ia2` filters and
//! normalizes, which a recording of what an application raises must not.

use std::cell::{Cell, RefCell};
use std::sync::mpsc;
use std::thread;

use verbatim_ia2::event_clock;
use verbatim_process::scheduling::{ThreadPriority, prioritize_current_thread};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_MAX, EVENT_MIN, EVENT_OBJECT_FOCUS, GetMessageW, MSG,
    PostThreadMessageW, TranslateMessage, WINEVENT_OUTOFCONTEXT, WM_APP,
};

use crate::passes::Passes;

/// The thread message that ends a pass, its number in the first word.
const WM_PASS: u32 = WM_APP;

/// The focus listener's `WinEvent` thread priority
/// (`verbatim-ia2`'s event thread), so that the passes split as its do.
const WINEVENT_PRIORITY: ThreadPriority = ThreadPriority::Highest;

/// Which hook delivered an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hook {
    /// The target's hook: every event of the target's process but focus.
    Target,
    /// The desktop's hook: focus, for every process.
    Focus,
}

/// One `WinEvent` as the hook delivers it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RawEvent {
    /// The hook that delivered it.
    pub hook: Hook,
    /// The raw event id.
    pub event: u32,
    /// `hwnd`.
    pub hwnd: isize,
    /// `idObject`.
    pub object: i32,
    /// `idChild`.
    pub child: i32,
    /// How long before the callback Windows raised it, on the event clock.
    pub raised_ms_ago: u32,
    /// The thread's pass it arrived in.
    pub pass: u64,
}

/// What the hook thread does with each event: it must not block.
pub(crate) type Sink = Box<dyn Fn(RawEvent)>;

/// What the hook thread does when it cannot post a pass's message, so that
/// the passes after it would be wrong: ends the recording.
pub(crate) type PassFailed = Box<dyn Fn(String)>;

thread_local! {
    /// This thread's sink, set before its hooks are installed.
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    /// What ends the recording when a pass message cannot be posted.
    static PASS_FAILED: RefCell<Option<PassFailed>> = const { RefCell::new(None) };
    /// This thread's passes.
    static PASSES: RefCell<Passes> = RefCell::new(Passes::default());
    /// The target's hook, to tell its events from the focus hook's.
    static TARGET_HOOK: Cell<isize> = const { Cell::new(0) };
}

/// The `WINEVENTPROC` both recorder hooks deliver to.
unsafe extern "system" fn deliver(
    hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    time: u32,
) {
    let hook = if TARGET_HOOK.get() == hook.0 as isize {
        Hook::Target
    } else {
        Hook::Focus
    };
    // The target's own focus events reach the desktop's hook as well, as
    // the listener's one hook receives them once.
    if hook == Hook::Target && event == EVENT_OBJECT_FOCUS {
        return;
    }
    let raised_ms_ago = event_clock().wrapping_sub(time);
    let delivered = PASSES.with(|passes| passes.borrow_mut().event());
    if delivered.opened {
        post_pass(delivered.pass);
    }
    SINK.with(|sink| {
        if let Some(sink) = sink.borrow().as_ref() {
            sink(RawEvent {
                hook,
                event,
                hwnd: hwnd.0 as isize,
                object,
                child,
                raised_ms_ago,
                pass: delivered.pass,
            });
        }
    });
}

/// Posts this thread the message that ends pass `pass`, ending the
/// recording when it cannot.
fn post_pass(pass: u64) {
    // SAFETY: GetCurrentThreadId is always sound.
    let thread = unsafe { GetCurrentThreadId() };
    let posted = usize::try_from(pass)
        .map_err(|error| error.to_string())
        .and_then(|word| {
            // SAFETY: posting a message with no pointer parameters to this
            // thread's own queue.
            unsafe { PostThreadMessageW(thread, WM_PASS, WPARAM(word), LPARAM(0)) }
                .map_err(|error| error.to_string())
        });
    if let Err(error) = posted {
        PASS_FAILED.with(|failed| {
            if let Some(failed) = failed.borrow().as_ref() {
                failed(format!(
                    "pass {pass}'s message could not be posted: {error}"
                ));
            }
        });
    }
}

/// Installs one out-of-context hook of the events from `first` to `last`
/// of process `pid` (every process for zero).
fn hook_events(first: u32, last: u32, pid: u32) -> Result<HWINEVENTHOOK, String> {
    // SAFETY: a null module and the out-of-context flag are the documented
    // combination for a hook whose proc is in this process; `deliver` has
    // the required signature.
    let hook = unsafe {
        SetWinEventHook(
            first,
            last,
            None,
            Some(deliver),
            pid,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if hook.0.is_null() {
        return Err(format!(
            "SetWinEventHook refused events {first:#X} to {last:#X} of process {pid}"
        ));
    }
    Ok(hook)
}

/// Starts the `WinEvent` thread at the listener's priority: it hooks every
/// event of the process `pid`, then focus for every process, out of
/// context, delivering to the sink `make_sink` builds there with each
/// event's pass, and returns once both hooks are in. The thread runs until
/// the process ends.
///
/// # Errors
///
/// Returns a message if the thread cannot start, its priority is refused,
/// or a hook is refused.
pub(crate) fn spawn_hook_thread(
    pid: u32,
    make_sink: impl FnOnce() -> (Sink, PassFailed) + Send + 'static,
) -> Result<(), String> {
    let (hooked, wait) = mpsc::channel::<Result<(), String>>();
    thread::Builder::new()
        .name("verbatim-event-record-winevent".to_owned())
        .spawn(move || {
            if let Err(error) = prioritize_current_thread(WINEVENT_PRIORITY) {
                let _ = hooked.send(Err(format!(
                    "the WinEvent thread's priority was refused: {error}"
                )));
                return;
            }
            let (sink, failed) = make_sink();
            SINK.with(|slot| *slot.borrow_mut() = Some(sink));
            PASS_FAILED.with(|slot| *slot.borrow_mut() = Some(failed));
            // The target's hook comes first, so that a window shown while
            // the desktop's is installed is not missed.
            let installed = hook_events(EVENT_MIN, EVENT_MAX, pid).and_then(|target| {
                TARGET_HOOK.set(target.0 as isize);
                hook_events(EVENT_OBJECT_FOCUS, EVENT_OBJECT_FOCUS, 0)
            });
            let failed = installed.is_err();
            let _ = hooked.send(installed.map(|_| ()));
            if failed {
                return;
            }
            let mut message = MSG::default();
            // SAFETY: `message` is a valid out-parameter; the loop only
            // pumps this thread's own queue.
            while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.as_bool() {
                if message.hwnd.is_invalid() && message.message == WM_PASS {
                    let pass = message.wParam.0 as u64;
                    PASSES.with(|passes| passes.borrow_mut().message(pass));
                    continue;
                }
                // SAFETY: the message just retrieved.
                let _ = unsafe { TranslateMessage(&raw const message) };
                // SAFETY: as above.
                unsafe { DispatchMessageW(&raw const message) };
            }
        })
        .map_err(|error| format!("could not start the WinEvent thread: {error}"))?;
    wait.recv()
        .map_err(|_| "the WinEvent thread ended before it hooked".to_owned())?
}
