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
//! - It forwards emitted gestures down an unbounded channel, whose send
//!   never waits, and [`InputHook::start`] refuses a bounded one. Waiting for
//!   a slow consumer would risk the whole keyboard, and dropping a gesture
//!   when the consumer is backed up would lose a key the user pressed;
//!   neither is acceptable, so a backlog waits in memory in front of the
//!   router instead, as the communication design keeps every backlog in
//!   front of the reducer (`docs/design/communication.md`, section 4.8). A
//!   queued gesture costs about 70 bytes (its 48-byte slot and its
//!   gesture name), and a held key repeats about 30 times a second, so even
//!   a router stalled for a minute holds about 140 kilobytes.
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
use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
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
use verbatim_input::state::{
    Decision, DecisionConfig, DecisionMachine, EmittedGesture, KeySpeechEffect,
};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};

/// The `dwExtraInfo` Verbatim puts on keys it injects for its own purposes
/// (the Control tap that lets it take the foreground). The hook leaves
/// speech alone for them, as NVDA ignores the keys it injects itself; keys
/// injected on a user's behalf, such as the end-to-end harness's, carry no
/// tag and cancel speech like typed ones.
pub const OWN_INPUT_TAG: usize = 0x5642_544D;

/// The `dwExtraInfo` Verbatim puts on a caret key it held back and now
/// passes on to the application (`docs/design/key-holding.md`): the hook
/// passes it, and the modifiers sent with it, straight down the hook chain,
/// deciding and reporting nothing, as NVDA ignores the keys its caret
/// scripts send. Keys injected on a user's behalf, such as the end-to-end
/// harness's, carry other values and are held like typed ones.
pub const PASSED_ON_TAG: usize = 0x5642_4B50;

/// Which caret keys the hook holds back, and whether it holds them now
/// (`docs/design/key-holding.md`). The keys are fixed when the hook starts;
/// the switch is set by the reducer's thread from the focus after each
/// input ([`KeyHolding::set`]) and read by the hook procedure with one
/// atomic load, so the hook never waits for it.
#[derive(Debug)]
pub struct KeyHolding {
    on: AtomicBool,
    keys: HashSet<GestureId>,
}

impl KeyHolding {
    /// Holds `keys`, the caret keys, once switched on.
    #[must_use]
    pub fn new(keys: impl IntoIterator<Item = GestureId>) -> Self {
        Self {
            on: AtomicBool::new(false),
            keys: keys.into_iter().collect(),
        }
    }

    /// Switches holding on, while the focus is text whose caret Verbatim
    /// reads, or off.
    pub fn set(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    /// Whether a key that completed `gesture` is held back now.
    fn holds(&self, gesture: &GestureId) -> bool {
        self.on.load(Ordering::Relaxed) && self.keys.contains(gesture)
    }
}

/// The key a held caret key's press came from, for the application to be
/// sent it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldStroke {
    /// The virtual-key code.
    pub vk: u16,
    /// The hardware scan code.
    pub scan_code: u32,
    /// Whether it is an extended key (the arrows and the navigation block).
    pub extended: bool,
}

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
use verbatim_model::{GestureId, TraceId};

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
    /// A caret key the hook held back from the application, because
    /// holding is on ([`KeyHolding`]): the reducer has the caret read, then
    /// has the key sent again ([`HeldStroke`], tagged [`PASSED_ON_TAG`]).
    /// Its release is held back too.
    Held {
        /// The gesture.
        gesture: EmittedGesture,
        /// As [`KeyReport::Observed`]'s.
        pressed_at_us: u64,
        /// The key to send again.
        stroke: HeldStroke,
    },
    /// Any other key passed to the application, typing no text, that is not
    /// a modifier or a lock key: the source of a caret key judged by where
    /// the caret landed (`CaretMotion::Other`).
    Passed {
        /// Minted when the key was observed.
        trace_id: TraceId,
        /// As [`KeyReport::Observed`]'s.
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
    holding: Arc<KeyHolding>,
    /// The keys whose press was held back, whose release is held back too,
    /// so the application never sees a release without its press.
    held_downs: HashSet<(u16, bool)>,
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
    /// `events` receives gestures as they fire. It must be an unbounded
    /// channel, whose send neither waits nor fails while the router runs, so
    /// no key is ever lost (see the module's never-block constraint).
    /// `speech` carries out each key
    /// press's effect on speech (cancel, or pause and resume), before the
    /// press's gesture is sent, so speech the gesture causes is never the
    /// speech it cancels. `reports` receives, after the speech effect, what
    /// each passed or held key did ([`KeyReport`]). `holding` says which
    /// caret keys are held back, and when.
    ///
    /// # Errors
    ///
    /// Returns an error if `events` is bounded, the hook thread cannot be
    /// spawned, or `SetWindowsHookExW` fails.
    pub fn start(
        config: DecisionConfig,
        map: SharedGestureMap,
        events: Sender<Routed>,
        speech: SpeechEffectFn,
        reports: KeyReportFn,
        holding: Arc<KeyHolding>,
    ) -> io::Result<Self> {
        if events.capacity().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the keyboard hook's channel must be unbounded, so that no key is lost",
            ));
        }
        // The thread reports back either its id (hook installed) or the error
        // that stopped it, so `start` can surface installation failure.
        let (ready_tx, ready_rx) = mpsc::channel::<io::Result<u32>>();

        let join = thread::Builder::new()
            .name("verbatim-input-hook".to_owned())
            .spawn(move || {
                hook_thread(config, map, events, (speech, reports, holding), &ready_tx);
            })?;

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

/// The hook thread's priority: it decides each key and hands it on, for
/// microseconds per key, and Windows removes a low-level hook that keeps a
/// key waiting too long (`docs/design/communication.md`, section 6.1).
const HOOK_PRIORITY: ThreadPriority = ThreadPriority::Highest;

/// The body of the hook thread: install the hook, publish readiness, pump
/// messages until `WM_QUIT`, then unhook.
fn hook_thread(
    config: DecisionConfig,
    map: SharedGestureMap,
    events: Sender<Routed>,
    (speech, reports, holding): (SpeechEffectFn, KeyReportFn, Arc<KeyHolding>),
    ready_tx: &mpsc::Sender<io::Result<u32>>,
) {
    prioritize_role("keyboard hook", HOOK_PRIORITY);
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
            holding,
            held_downs: HashSet::new(),
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

    // The console windows' threads, for their keyboard layout (`typed`),
    // kept while the loop runs.
    let consoles = typed::ConsoleWatch::start();
    pump_messages();
    drop(consoles);

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
/// Whether `vk` is a modifier key, Shift, Control, Alt, or Windows, which
/// does nothing alone.
fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

/// Whether `vk`, passed to the application without typing text, may edit
/// the text or move the caret, so the hook reports it
/// ([`KeyReport::Passed`]) and the reducer watches for its evidence: any
/// key but a modifier, which does nothing alone, a lock key, whose new
/// state is announced instead, and a key Windows gives a meaning outside
/// the text (Pause, Print Screen, Sleep, and the browser, volume, media and
/// launch keys). NVDA says nothing for any of these; a watch for one would
/// only wait out its bound.
fn may_edit(vk: u16) -> bool {
    !is_modifier(vk)
        && verbatim_input::ToggleKey::from_vk(vk).is_none()
        && !matches!(vk, 0x13 | 0x2C | 0x5F | 0xA6..=0xB7)
}

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
            decide(state, event, kbd.scanCode, kbd.dwExtraInfo)
        });

        if decision == KeyDecision::Swallow {
            return LRESULT(1);
        }
    }

    // SAFETY: passing the transition to the next hook in the chain with the
    // parameters we received is always sound; a null handle is accepted.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Holds a caret key back while holding is on, and its release with it,
/// swallowing both in `decision`; the reducer has the key sent again once
/// the caret has been read. Returns whether this key-down was held.
fn hold(state: &mut HookState, event: KeyEvent, own: bool, decision: &mut Decision) -> bool {
    let stroke = (event.vk, event.extended);
    let held = event.pressed
        && !own
        && decision
            .observed
            .as_ref()
            .is_some_and(|observed| state.holding.holds(&observed.gesture));
    if held {
        state.held_downs.insert(stroke);
        decision.decision = KeyDecision::Swallow;
    } else if !event.pressed && state.held_downs.remove(&stroke) {
        decision.decision = KeyDecision::Swallow;
    }
    held
}

/// Decides one key transition with the hook thread's state, carries out its
/// effect on speech, and sends and reports what it caused; `scan_code` and
/// `extra_info` are the key event's own. Every send is on the router's
/// unbounded channel, so it neither waits nor drops; it fails only once the
/// router has exited, at shutdown, when nothing is left to act on a key.
fn decide(
    state: &mut HookState,
    event: KeyEvent,
    scan_code: u32,
    extra_info: usize,
) -> KeyDecision {
    // A held key Verbatim passes on, with its modifiers, goes straight to
    // the application: it was decided and reported when it was pressed.
    if extra_info == PASSED_ON_TAG {
        return KeyDecision::Pass;
    }
    // Num Lock decides what the numpad's operator keys are: the
    // keyboard's own state, read locally before each key.
    // SAFETY: GetKeyState takes any virtual-key code.
    let num_lock = unsafe { GetKeyState(i32::from(VK_NUMLOCK.0)) } & 1 != 0;
    state.machine.set_num_lock(num_lock);
    let mut decision = state.machine.on_key(event, Instant::now());
    let own = extra_info == OWN_INPUT_TAG;
    let held = hold(state, event, own, &mut decision);
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
        let _ = state.events.send(Routed::Gesture(emitted));
    }
    let was_observed = decision.observed.is_some();
    if let Some(observed) = decision.observed
        && !own
    {
        record_key_origin(observed.trace_id, key_number);
        let pressed_at_us = unix_us();
        (state.reports)(if held {
            KeyReport::Held {
                gesture: observed,
                pressed_at_us,
                stroke: HeldStroke {
                    vk: event.vk,
                    scan_code,
                    extended: event.extended,
                },
            }
        } else {
            KeyReport::Observed {
                gesture: observed,
                pressed_at_us,
            }
        });
    }
    let passed = event.pressed
        && decision.decision == KeyDecision::Pass
        && !decision.shared_modifier
        && !own;
    if passed {
        let pressed_at_us = unix_us();
        let trace_id = TraceId::mint();
        match state.typing.translate(event.vk, scan_code) {
            Some(text) => {
                record_key_origin(trace_id, key_number);
                (state.reports)(KeyReport::Typed { trace_id, text });
            }
            None if !was_observed && may_edit(event.vk) => {
                record_key_origin(trace_id, key_number);
                (state.reports)(KeyReport::Passed {
                    trace_id,
                    pressed_at_us,
                });
            }
            None => {}
        }
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
            .send(Routed::Gesture(verbatim_input::EmittedGesture {
                trace_id,
                gesture: key.gesture(),
                repeat: 0,
            }));
    }
    // A harness's stroke is complete with its last key event: say
    // so behind everything the stroke caused.
    if let Some(numbered) = u64::try_from(extra_info)
        .ok()
        .and_then(verbatim_input::harness::decode)
        && numbered.last
    {
        let _ = state.events.send(Routed::Handled(numbered.number));
    }
    decision.decision
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

    /// A burst of strokes, each Verbatim+V with a harness number, larger
    /// than the 64 gestures the router's channel once held, decided while
    /// nothing reads the channel. It shows that `decide` sends every
    /// gesture, in order; on an unbounded channel it would pass with
    /// `try_send` too, so the guarantee that no key is lost rests on
    /// `a_bounded_channel_is_refused`.
    #[test]
    fn a_burst_larger_than_the_old_bound_is_delivered_in_order_with_none_lost() {
        use verbatim_input::GestureMap;
        use verbatim_input::state::DecisionConfig;
        use verbatim_model::GestureId;

        const CAPS: u16 = 0x14;
        const V: u16 = 0x56;
        const STROKES: u64 = 1_000;
        let key = |vk, pressed| KeyEvent {
            vk,
            scan_code: 0,
            extended: false,
            injected: true,
            pressed,
        };
        let map = GestureMap::new([GestureId::parse("kb:v+verbatim").expect("valid id")]);
        let (events, routed) = crossbeam_channel::unbounded();
        let mut state = HookState {
            machine: DecisionMachine::new(DecisionConfig::default(), map.into_shared()),
            events,
            speech: Box::new(|_, _| {}),
            reports: Box::new(|_| {}),
            typing: typed::Typing::default(),
            holding: Arc::new(KeyHolding::new([])),
            held_downs: HashSet::new(),
        };
        assert_eq!(
            decide(&mut state, key(CAPS, true), 0, 0),
            KeyDecision::Swallow
        );
        for number in 1..=STROKES {
            let last = usize::try_from(verbatim_input::harness::encode(number, true)).unwrap();
            assert_eq!(decide(&mut state, key(V, true), 0, 0), KeyDecision::Swallow);
            assert_eq!(
                decide(&mut state, key(V, false), 0, last),
                KeyDecision::Swallow
            );
        }
        drop(state);

        let mut received = Vec::new();
        for routed in routed {
            received.push(match routed {
                Routed::Gesture(emitted) => format!("{}", emitted.gesture),
                Routed::Handled(number) => format!("handled {number}"),
                Routed::Barrier(token) => format!("barrier {token}"),
            });
        }
        let expected: Vec<String> = (1..=STROKES)
            .flat_map(|number| ["kb:v+verbatim".to_owned(), format!("handled {number}")])
            .collect();
        assert_eq!(received, expected);
    }

    /// The hook refuses a bounded channel, which once full would make the
    /// hook wait or drop the key: this is what guarantees no key is lost.
    #[test]
    fn a_bounded_channel_is_refused() {
        let (events, _routed) = crossbeam_channel::bounded(64);
        let error = InputHook::start(
            verbatim_input::state::DecisionConfig::default(),
            verbatim_input::GestureMap::new([]).into_shared(),
            events,
            Box::new(|_, _| {}),
            Box::new(|_| {}),
            Arc::new(KeyHolding::new([])),
        )
        .expect_err("a bounded channel could drop a key");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    /// A hook state observing Right Arrow and Escape, holding Right Arrow,
    /// whose reports are collected in `reports`.
    fn holding_state(
        reports: &Arc<Mutex<Vec<KeyReport>>>,
    ) -> (
        HookState,
        crossbeam_channel::Receiver<Routed>,
        Arc<KeyHolding>,
    ) {
        use verbatim_input::GestureMap;
        use verbatim_input::state::DecisionConfig;

        let right = GestureId::parse("kb:rightarrow").expect("valid id");
        let escape = GestureId::parse("kb:escape").expect("valid id");
        let map = GestureMap::new([]).with_observed([right.clone(), escape]);
        let holding = Arc::new(KeyHolding::new([right]));
        let (events, routed) = crossbeam_channel::unbounded();
        let sink = Arc::clone(reports);
        let state = HookState {
            machine: DecisionMachine::new(DecisionConfig::default(), map.into_shared()),
            events,
            speech: Box::new(|_, _| {}),
            reports: Box::new(move |report| sink.lock().unwrap().push(report)),
            typing: typed::Typing::default(),
            holding: Arc::clone(&holding),
            held_downs: HashSet::new(),
        };
        (state, routed, holding)
    }

    /// Right Arrow, an extended key, pressed or released.
    fn right_arrow(pressed: bool) -> KeyEvent {
        KeyEvent {
            vk: 0x27,
            scan_code: 0x4D,
            extended: true,
            injected: false,
            pressed,
        }
    }

    /// While holding is on, a caret key in the holding set is held back,
    /// press and release, and reported held with the stroke to send again;
    /// a key observed but not held (Escape) still passes; with holding off
    /// the caret key passes and is reported observed, as before.
    #[test]
    fn a_caret_key_is_held_while_holding_is_on() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let (mut state, _routed, holding) = holding_state(&reports);
        holding.set(true);
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Swallow
        );
        assert_eq!(
            decide(&mut state, right_arrow(false), 0x4D, 0),
            KeyDecision::Swallow
        );
        let escape = |pressed| KeyEvent {
            vk: 0x1B,
            scan_code: 0x01,
            extended: false,
            injected: false,
            pressed,
        };
        assert_eq!(decide(&mut state, escape(true), 0x01, 0), KeyDecision::Pass);
        assert_eq!(
            decide(&mut state, escape(false), 0x01, 0),
            KeyDecision::Pass
        );
        holding.set(false);
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Pass
        );
        assert_eq!(
            decide(&mut state, right_arrow(false), 0x4D, 0),
            KeyDecision::Pass
        );
        let reports = reports.lock().unwrap();
        let kinds: Vec<String> = reports
            .iter()
            .map(|report| match report {
                KeyReport::Held {
                    gesture, stroke, ..
                } => format!(
                    "held {} {:#04x} {:#04x} {}",
                    gesture.gesture, stroke.vk, stroke.scan_code, stroke.extended
                ),
                KeyReport::Observed { gesture, .. } => format!("observed {}", gesture.gesture),
                KeyReport::Passed { .. } => "passed".to_owned(),
                KeyReport::Typed { text, .. } => format!("typed {text}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "held kb:rightarrow 0x27 0x4d true",
                "observed kb:escape",
                "observed kb:rightarrow",
            ]
        );
    }

    /// A key carrying [`PASSED_ON_TAG`], a held key Verbatim passes on,
    /// passes straight through, reported to nobody and sent on to no
    /// router, though holding is on; a key the end-to-end harness injects,
    /// with its stroke number, is the user's and is held.
    #[test]
    fn a_key_passed_on_passes_untouched_and_a_harness_key_is_held() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let (mut state, routed, holding) = holding_state(&reports);
        holding.set(true);
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, PASSED_ON_TAG),
            KeyDecision::Pass
        );
        assert_eq!(
            decide(&mut state, right_arrow(false), 0x4D, PASSED_ON_TAG),
            KeyDecision::Pass
        );
        assert!(reports.lock().unwrap().is_empty());
        assert!(routed.try_recv().is_err());
        let numbered = usize::try_from(verbatim_input::harness::encode(7, true)).unwrap();
        let injected = |pressed| KeyEvent {
            injected: true,
            ..right_arrow(pressed)
        };
        assert_eq!(
            decide(&mut state, injected(true), 0x4D, 0),
            KeyDecision::Swallow
        );
        assert_eq!(
            decide(&mut state, injected(false), 0x4D, numbered),
            KeyDecision::Swallow
        );
        assert!(matches!(
            reports.lock().unwrap().as_slice(),
            [KeyReport::Held { .. }]
        ));
        assert_eq!(routed.try_recv(), Ok(Routed::Handled(7)));
    }

    #[test]
    fn a_key_with_a_meaning_outside_the_text_is_not_watched() {
        // Pause, Print Screen, Sleep, Volume Up, Play/Pause, Launch Mail.
        for vk in [0x13, 0x2C, 0x5F, 0xAF, 0xB3, 0xB4] {
            assert!(!may_edit(vk), "{vk:#04x}");
        }
        // Left Control and Caps Lock, as before.
        for vk in [0xA2, 0x14] {
            assert!(!may_edit(vk), "{vk:#04x}");
        }
        // Keys Verbatim does not know that may move the caret: bash's
        // Control+A and Alt+F, and F3, Find Next.
        for vk in [0x41, 0x46, 0x72] {
            assert!(may_edit(vk), "{vk:#04x}");
        }
    }
}
