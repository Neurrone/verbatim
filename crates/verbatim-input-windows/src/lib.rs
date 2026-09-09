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

use std::cell::RefCell;
use std::io;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crossbeam_channel::Sender;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_QUIT, WM_SYSKEYDOWN,
};

use verbatim_input::map::SharedGestureMap;
use verbatim_input::state::{DecisionConfig, DecisionMachine, EmittedGesture};
use verbatim_input::{KeyDecision, KeyEvent};

/// Per-hook-thread state reached by the hook procedure.
///
/// The hook procedure is a bare `extern "system"` function with no user
/// pointer, so it reaches its state through this thread-local. The procedure
/// only ever runs on the thread that installed the hook, so a thread-local is
/// exactly the right scope and needs no synchronization.
struct HookState {
    machine: DecisionMachine,
    events: Sender<EmittedGesture>,
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
    /// bounded channel is a fine choice.
    ///
    /// # Errors
    ///
    /// Returns an error if the hook thread cannot be spawned or
    /// `SetWindowsHookExW` fails.
    pub fn start(
        config: DecisionConfig,
        map: SharedGestureMap,
        events: Sender<EmittedGesture>,
    ) -> io::Result<Self> {
        // The thread reports back either its id (hook installed) or the error
        // that stopped it, so `start` can surface installation failure.
        let (ready_tx, ready_rx) = mpsc::channel::<io::Result<u32>>();

        let join = thread::Builder::new()
            .name("verbatim-input-hook".to_owned())
            .spawn(move || hook_thread(config, map, events, &ready_tx))?;

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
    events: Sender<EmittedGesture>,
    ready_tx: &mpsc::Sender<io::Result<u32>>,
) {
    // SAFETY: `GetModuleHandleW(None)` returns this process's module handle,
    // the standard `hmod` for a low-level hook whose procedure lives in this
    // module. It does not fail in practice for the current process.
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
        return;
    }

    pump_messages();

    // SAFETY: `hook` is the handle installed above and not yet unhooked.
    unsafe {
        let _ = UnhookWindowsHookEx(hook);
    }
    HOOK_STATE.with(|state| *state.borrow_mut() = None);
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
            let mut state = state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return KeyDecision::Pass;
            };
            let decision = state.machine.on_key(event, Instant::now());
            if let Some(emitted) = decision.emitted {
                // Never block: drop the gesture if the consumer is backed up.
                let _ = state.events.try_send(emitted);
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
