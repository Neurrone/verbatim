//! The low-level keyboard hook thread.
//!
//! A dedicated thread installs a `WH_KEYBOARD_LL` hook and runs a message
//! loop; the hook procedure runs on that same thread, feeds each transition to
//! the pure [`DecisionMachine`], returns `1` to swallow or calls
//! `CallNextHookEx` to pass, and forwards emitted gestures down a channel.
//!
//! # The never-block constraint
//!
//! This is the single most important rule in this module. Windows silently
//! removes a low-level hook whose procedure takes longer than
//! `LowLevelHooksTimeout` (a per-user registry value, 300 ms by default) to
//! return. If that happens the hook stops firing and Verbatim goes deaf to the
//! keyboard with no error. Therefore the hook procedure must never block:
//!
//! - It runs the pure decision machine, which does no I/O and takes
//!   microseconds against a lock-free snapshot of the gesture map.
//! - It forwards emitted gestures with a non-blocking
//!   [`try_send`](crossbeam_channel::Sender::try_send) and drops the gesture if
//!   the channel is full, rather than waiting for a slow consumer. A full
//!   channel means the reducer is already backed up; dropping one synthetic
//!   input event is far better than losing the whole keyboard.
//! - It touches no lock that any other thread can hold across a blocking call.
//!
//! Nothing that can wait belongs in the hook procedure. Gesture semantics run
//! elsewhere (the reducer thread), reached only through the channel.
//!
//! Besides bound gestures the hook reports, through a callback that must not
//! block either, what a passed key did that the reducer needs to know: an
//! observed gesture (a caret key) and the text a key types
//! ([`KeyReport`]). Before each key it tells the decision machine whether
//! Num Lock is on.
//!
//! # Key sequence numbers
//!
//! Every key press gets the next key sequence number ([`next_key`]), and
//! everything the press causes is recorded under it by its trace id
//! ([`key_of`]): its gesture, its observed caret key, the text it types. A
//! press cancels speech at once, from this thread, while the previous
//! press's gesture may still be on its way to the reducer; the cancel
//! therefore names its press's number, and the speech manager drops speech
//! caused by an earlier press that reaches it after the cancel, as NVDA,
//! which queues a key's cancel in order behind the earlier keys' scripts,
//! never speaks it (`docs/nvda/input.md`, "What a key press does to
//! speech").

mod typed;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::Sender;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_NUMLOCK};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_QUIT, WM_SYSKEYDOWN,
};

use verbatim_input::map::SharedGestureMap;
use verbatim_input::state::{DecisionConfig, DecisionMachine, EmittedGesture, KeySpeechEffect};

/// The `dwExtraInfo` Verbatim puts on keys it injects for its own purposes
/// (the Control tap that lets it take the foreground). The hook leaves
/// speech alone for them, as NVDA ignores the keys it injects itself; keys
/// injected on a user's behalf, such as the end-to-end harness's, carry no
/// tag and cancel speech like typed ones.
pub const OWN_INPUT_TAG: usize = 0x5642_544D;

/// What the hook sends the gesture router, in the order the keys came.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Routed {
    /// A gesture fired.
    Gesture(EmittedGesture),
    /// An end-to-end harness's numbered key stroke has been handled by the
    /// hook (`verbatim_input::harness`): everything it caused was sent on
    /// before this. The router passes it on behind whatever the stroke's
    /// gestures caused, so the reducer can say when it has handled it.
    Handled(u64),
    /// A control-plane request waiting for Verbatim to be idle, named by its
    /// token: it is answered only after everything sent here before it.
    /// Never sent by the hook itself.
    Barrier(u64),
}

/// Carries out a key press's effect on speech, given the press's key
/// sequence number ([`next_key`]); called on the hook thread, so it must not
/// block, nor make a call that dispatches sent messages (a cross-apartment
/// COM call, `SendMessage`), which would deliver the next key to the hook
/// while this one is still being handled.
pub type SpeechEffectFn = Box<dyn Fn(KeySpeechEffect, u64) + Send>;

/// The last key sequence number given out.
static KEY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// How many traces' key sequence numbers are kept: far more than a key's
/// speech can be in flight behind.
const KEY_ORIGINS_KEPT: usize = 1024;

/// The key sequence number of what recent key presses caused, by trace id,
/// oldest first.
static KEY_ORIGINS: Mutex<VecDeque<(TraceId, u64)>> = Mutex::new(VecDeque::new());

/// Gives out the next key sequence number, for a key press or a gesture
/// the control plane injects.
pub fn next_key() -> u64 {
    KEY_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1
}

/// Records that what carries `trace` was caused by key press `key`.
pub fn record_key_origin(trace: TraceId, key: u64) {
    let mut origins = KEY_ORIGINS.lock().unwrap_or_else(PoisonError::into_inner);
    if origins.len() == KEY_ORIGINS_KEPT {
        origins.pop_front();
    }
    origins.push_back((trace, key));
}

/// The key sequence number of the key press that caused what carries
/// `trace`, if a recent key press did.
#[must_use]
pub fn key_of(trace: TraceId) -> Option<u64> {
    KEY_ORIGINS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .find(|(recorded, _)| *recorded == trace)
        .map(|&(_, key)| key)
}

/// Receives what a key passed to the application did; called on the hook
/// thread, so it must not block, nor dispatch sent messages, as
/// [`SpeechEffectFn`] explains.
pub type KeyReportFn = Box<dyn Fn(KeyReport) + Send>;
use verbatim_input::{KeyDecision, KeyEvent};
use verbatim_model::TraceId;

/// What a key the hook passed to the application did, reported after the
/// key's effect on speech.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyReport {
    /// The key completed an observed gesture (`Decision::observed`), a
    /// caret key whose result the reducer speaks.
    Observed {
        /// The gesture.
        gesture: EmittedGesture,
        /// Microseconds since the Unix epoch when the hook procedure ran,
        /// the clock outposts stamp what they observe with (in milliseconds
        /// for `observed_at_ms`, in microseconds for the latency log). The
        /// application receives the key only after the hook returns, so
        /// anything an outpost read before this time came before the key.
        /// This is not the key event's own `time`, which counts
        /// milliseconds since startup at the system timer's resolution of
        /// about 16 ms: converted to this clock it could fall after the
        /// application handled the key.
        pressed_at_us: u64,
    },
    /// The key types `text` into the focused application: the source of
    /// `Input::CharacterTyped`. A tab is a tab character and Enter a
    /// carriage return; a dead key types nothing until the key after it.
    Typed {
        /// Minted when the key was observed.
        trace_id: TraceId,
        /// The text typed.
        text: String,
    },
}

/// Per-hook-thread state reached by the hook procedure.
///
/// The hook procedure is a bare `extern "system"` function with no user
/// pointer, so it reaches its state through this thread-local. The procedure
/// only ever runs on the thread that installed the hook, so a thread-local is
/// exactly the right scope and needs no synchronization.
struct HookState {
    machine: DecisionMachine,
    events: Sender<Routed>,
    speech: SpeechEffectFn,
    reports: KeyReportFn,
    typing: typed::Typing,
}

thread_local! {
    static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

/// A running keyboard hook. Dropping it stops the hook thread and unhooks.
#[derive(Debug)]
pub struct InputHook {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl InputHook {
    /// Installs the low-level keyboard hook on a fresh dedicated thread.
    ///
    /// `config` chooses the Verbatim modifier keys and the double-tap timeout,
    /// `map` is the lock-free bound-gesture snapshot the hook consults, and
    /// `events` receives gestures as they fire. The send is non-blocking and
    /// drops on a full channel (see the module's never-block constraint), so a
    /// bounded channel is a fine choice. `speech` carries out each key
    /// press's effect on speech (cancel, or pause and resume), before the
    /// press's gesture is sent, so speech the gesture causes is never the
    /// speech it cancels. `reports` receives, after the speech effect, what
    /// each passed key did ([`KeyReport`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the hook thread cannot be spawned or
    /// `SetWindowsHookExW` fails.
    pub fn start(
        config: DecisionConfig,
        map: SharedGestureMap,
        events: Sender<Routed>,
        speech: SpeechEffectFn,
        reports: KeyReportFn,
    ) -> io::Result<Self> {
        // The thread reports back either its id (hook installed) or the error
        // that stopped it, so `start` can surface installation failure.
        let (ready_tx, ready_rx) = mpsc::channel::<io::Result<u32>>();

        let join = thread::Builder::new()
            .name("verbatim-input-hook".to_owned())
            .spawn(move || hook_thread(config, map, events, (speech, reports), &ready_tx))?;

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                join: Some(join),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(_) => {
                let _ = join.join();
                Err(io::Error::other(
                    "hook thread exited before reporting readiness",
                ))
            }
        }
    }
}

impl Drop for InputHook {
    fn drop(&mut self) {
        // Wake the hook thread's message loop so it unhooks and returns.
        // SAFETY: posting WM_QUIT to a thread id is always sound; the target
        // thread may already have exited, in which case this simply fails.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The body of the hook thread: install the hook, publish readiness, pump
/// messages until `WM_QUIT`, then unhook.
fn hook_thread(
    config: DecisionConfig,
    map: SharedGestureMap,
    events: Sender<Routed>,
    (speech, reports): (SpeechEffectFn, KeyReportFn),
    ready_tx: &mpsc::Sender<io::Result<u32>>,
) {
    // COM for the text services' profile manager, which typing asks which
    // keyboard profile is active (`typed`); it lives as long as the thread
    // and is released before COM is uninitialized.
    let _apartment = Apartment::enter();
    // SAFETY: `GetModuleHandleW(None)` takes no pointer and returns this
    // process's module handle, the standard `hmod` for a low-level hook
    // whose procedure lives in this module; a failure is reported below.
    let hinstance = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => HINSTANCE(module.0),
        Err(error) => {
            let _ = ready_tx.send(Err(io::Error::from_raw_os_error(error.code().0)));
            return;
        }
    };

    // SAFETY: `keyboard_hook` has the required `HOOKPROC` signature and lives
    // for the process lifetime; `hinstance` is this module. On success we own
    // the returned hook and must unhook it before returning.
    let hook =
        match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(hinstance), 0) }
        {
            Ok(hook) => hook,
            Err(error) => {
                let _ = ready_tx.send(Err(io::Error::from_raw_os_error(error.code().0)));
                return;
            }
        };

    // Install this thread's decision state before announcing readiness, so the
    // procedure never runs against an empty thread-local.
    HOOK_STATE.with(|state| {
        *state.borrow_mut() = Some(HookState {
            machine: DecisionMachine::new(config, map),
            events,
            speech,
            reports,
            typing: typed::Typing::with_text_services(),
        });
    });

    // SAFETY: reads a thread-owned integer.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready_tx.send(Ok(thread_id)).is_err() {
        // The starter is gone; unhook and leave.
        // SAFETY: `hook` is the handle we just installed.
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
        HOOK_STATE.with(|state| *state.borrow_mut() = None);
        return;
    }

    pump_messages();

    // SAFETY: `hook` is the handle installed above and not yet unhooked.
    unsafe {
        let _ = UnhookWindowsHookEx(hook);
    }
    HOOK_STATE.with(|state| *state.borrow_mut() = None);
}

/// COM initialized on this thread as a single-threaded apartment, which
/// the hook thread's message loop serves, until dropped.
struct Apartment {
    entered: bool,
}

impl Apartment {
    /// Enters the apartment; a failure is logged, and leaves typing without
    /// the text services.
    fn enter() -> Self {
        // SAFETY: called once at the start of the hook thread, which has
        // not initialized COM.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if let Err(error) = result.ok() {
            tracing::warn!(%error, "COM could not be initialized on the hook thread");
        }
        Self {
            entered: result.is_ok(),
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.entered {
            // SAFETY: balances the successful `CoInitializeEx` on this
            // thread, after every COM object it made was released.
            unsafe { CoUninitialize() };
        }
    }
}

/// Microseconds since the Unix epoch, as outposts stamp their observations.
fn unix_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_micros()).unwrap_or(u64::MAX)
        })
}

/// Runs the message loop until `WM_QUIT` (or a `GetMessageW` error) ends it.
/// A low-level hook only fires while its installing thread pumps messages.
fn pump_messages() {
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a valid, owned message buffer; a null window handle
        // retrieves messages for this thread, including the posted `WM_QUIT`.
        let result = unsafe { GetMessageW(&raw mut msg, None, 0, 0) };
        // `GetMessageW` returns 0 for WM_QUIT and -1 for an error; both end
        // the loop. There is no window, so no dispatch is needed.
        if result.0 <= 0 {
            break;
        }
    }
}

/// The `WH_KEYBOARD_LL` procedure. Runs on the hook thread for every key
/// transition system-wide; see the module's never-block constraint.
///
/// # Safety
///
/// Called by the operating system with `WH_KEYBOARD_LL` conventions: when
/// `code == HC_ACTION`, `lparam` points to a valid [`KBDLLHOOKSTRUCT`]. The
/// function only dereferences it in that case.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION.cast_signed() {
        // SAFETY: for HC_ACTION, `lparam` is a pointer to a KBDLLHOOKSTRUCT
        // owned by the OS for the duration of this call.
        let kbd = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        // The message id (a small constant) and the virtual-key code (0..=254)
        // always fit; a hypothetical out-of-range value degrades to a
        // pass-through, which is the safe default.
        let message = u32::try_from(wparam.0).unwrap_or(0);
        let event = KeyEvent {
            vk: u16::try_from(kbd.vkCode).unwrap_or(0),
            scan_code: kbd.scanCode,
            extended: kbd.flags.contains(LLKHF_EXTENDED),
            injected: kbd.flags.contains(LLKHF_INJECTED),
            pressed: message == WM_KEYDOWN || message == WM_SYSKEYDOWN,
        };

        let decision = HOOK_STATE.with(|state| {
            // A key delivered while another is still being handled, should a
            // callback ever dispatch sent messages, passes through untouched
            // rather than panicking on the borrow, which would abort.
            let Ok(mut state) = state.try_borrow_mut() else {
                return KeyDecision::Pass;
            };
            let Some(state) = state.as_mut() else {
                return KeyDecision::Pass;
            };
            // Num Lock decides what the numpad's operator keys are: the
            // keyboard's own state, read locally before each key.
            // SAFETY: GetKeyState takes any virtual-key code.
            let num_lock = unsafe { GetKeyState(i32::from(VK_NUMLOCK.0)) } & 1 != 0;
            state.machine.set_num_lock(num_lock);
            let decision = state.machine.on_key(event, Instant::now());
            let own = kbd.dwExtraInfo == OWN_INPUT_TAG;
            let key_number = if event.pressed {
                next_key()
            } else {
                KEY_SEQUENCE.load(Ordering::Relaxed)
            };
            if let Some(effect) = decision.speech
                && !own
            {
                (state.speech)(effect, key_number);
            }
            if let Some(emitted) = decision.emitted {
                record_key_origin(emitted.trace_id, key_number);
                // Never block: drop the gesture if the consumer is backed up.
                let _ = state.events.try_send(Routed::Gesture(emitted));
            }
            if let Some(observed) = decision.observed
                && !own
            {
                record_key_origin(observed.trace_id, key_number);
                (state.reports)(KeyReport::Observed {
                    gesture: observed,
                    pressed_at_us: unix_us(),
                });
            }
            if event.pressed
                && decision.decision == KeyDecision::Pass
                && !decision.shared_modifier
                && !own
                && let Some(text) = state.typing.translate(event.vk, kbd.scanCode)
            {
                let trace_id = TraceId::mint();
                record_key_origin(trace_id, key_number);
                (state.reports)(KeyReport::Typed { trace_id, text });
            }
            // A lock key reaching the operating system is reported, for its
            // new state to be announced, as NVDA announces it. The Verbatim
            // modifier passed in share mode is not: the screen reader behind
            // Verbatim decides whether it reaches the operating system, and
            // announces the state itself if it lets it through.
            if event.pressed
                && decision.decision == KeyDecision::Pass
                && !decision.shared_modifier
                && let Some(key) = verbatim_input::ToggleKey::from_vk(event.vk)
            {
                let trace_id = TraceId::mint();
                record_key_origin(trace_id, key_number);
                let _ = state
                    .events
                    .try_send(Routed::Gesture(verbatim_input::EmittedGesture {
                        trace_id,
                        gesture: key.gesture(),
                        repeat: 0,
                    }));
            }
            // A harness's stroke is complete with its last key event: say
            // so behind everything the stroke caused. Never blocking, as
            // for a gesture; a dropped mark leaves the harness's barrier
            // waiting, which fails its test with a named timeout.
            if let Some(numbered) = u64::try_from(kbd.dwExtraInfo)
                .ok()
                .and_then(verbatim_input::harness::decode)
                && numbered.last
            {
                let _ = state.events.try_send(Routed::Handled(numbered.number));
            }
            decision.decision
        });

        if decision == KeyDecision::Swallow {
            return LRESULT(1);
        }
    }

    // SAFETY: passing the transition to the next hook in the chain with the
    // parameters we received is always sound; a null handle is accepted.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_key_caused_is_found_by_its_trace_until_newer_keys_push_it_out() {
        let first = TraceId::mint();
        let key = next_key();
        record_key_origin(first, key);
        assert_eq!(key_of(first), Some(key));
        assert_eq!(key_of(TraceId::mint()), None);
        for _ in 0..KEY_ORIGINS_KEPT {
            record_key_origin(TraceId::mint(), next_key());
        }
        assert_eq!(key_of(first), None);
    }
}
