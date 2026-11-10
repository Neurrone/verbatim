//! The recorder's own `WinEvent` hooks: raw events, unfiltered, each hook
//! on a thread of its own running a message loop. Verbatim's hook code in
//! `verbatim-ia2` filters and normalizes, which a recording of what an
//! application raises must not.

use std::cell::RefCell;
use std::sync::mpsc;
use std::thread;

use verbatim_ia2::event_clock;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage, WINEVENT_OUTOFCONTEXT,
};

/// One `WinEvent` as the hook delivers it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RawEvent {
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
}

/// What a hook thread does with each event: it must not block.
pub(crate) type Sink = Box<dyn Fn(RawEvent)>;

thread_local! {
    /// This thread's sink, set before its hook is installed.
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// The `WINEVENTPROC` every recorder hook delivers to.
unsafe extern "system" fn deliver(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    time: u32,
) {
    SINK.with(|sink| {
        if let Some(sink) = sink.borrow().as_ref() {
            sink(RawEvent {
                event,
                hwnd: hwnd.0 as isize,
                object,
                child,
                raised_ms_ago: event_clock().wrapping_sub(time),
            });
        }
    });
}

/// Starts a thread that hooks the events from `first` to `last` of the
/// process `pid` (every process for zero), out of context, delivering to
/// the sink `make_sink` builds there, and returns once the hook is in. The
/// thread runs until the process ends.
///
/// # Errors
///
/// Returns a message if the thread cannot start or the hook is refused.
pub(crate) fn spawn_hook_thread(
    name: &str,
    (first, last): (u32, u32),
    pid: u32,
    make_sink: impl FnOnce() -> Sink + Send + 'static,
) -> Result<(), String> {
    let (hooked, wait) = mpsc::channel::<Result<(), String>>();
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            SINK.with(|sink| *sink.borrow_mut() = Some(make_sink()));
            // SAFETY: a null module and the out-of-context flag are the
            // documented combination for a hook whose proc is in this
            // process; `deliver` has the required signature.
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
                let _ = hooked.send(Err(format!(
                    "SetWinEventHook refused events {first:#X} to {last:#X} of process {pid}"
                )));
                return;
            }
            let _ = hooked.send(Ok(()));
            let mut message = MSG::default();
            // SAFETY: `message` is a valid out-parameter; the loop only
            // pumps this thread's own queue.
            while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.as_bool() {
                // SAFETY: the message just retrieved.
                let _ = unsafe { TranslateMessage(&raw const message) };
                // SAFETY: as above.
                unsafe { DispatchMessageW(&raw const message) };
            }
        })
        .map_err(|error| format!("could not start a hook thread: {error}"))?;
    wait.recv()
        .map_err(|_| "the hook thread ended before it hooked".to_owned())?
}
