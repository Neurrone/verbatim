//! The low-level keyboard hook thread.
//!
//! A dedicated thread installs a `WH_KEYBOARD_LL` hook and runs a message
//! loop; the hook procedure runs on that same thread, feeds each transition to
//! the pure [`DecisionMachine`], returns `1` to swallow or calls
//! `CallNextHookEx` to pass, and reports what each key caused through one
//! callback ([`KeyReport`]). The code that runs on the thread is in the
//! `hook` module; this one holds what other threads call: [`InputHook`],
//! [`Injector`], and Core's views the thread reads ([`KeyHolding`],
//! [`ForegroundView`]).
//!
//! # The never-block constraint
//!
//! This is the single most important rule in this crate. Windows silently
//! removes a low-level hook whose procedure takes longer than
//! `LowLevelHooksTimeout` (a per-user registry value, 300 ms by default) to
//! return. If that happens the hook stops firing and Verbatim goes deaf to the
//! keyboard with no error. Therefore the hook procedure must never block:
//!
//! - It runs the pure decision machine, which does no I/O and takes
//!   microseconds against a lock-free snapshot of the gesture map.
//! - It reports through callbacks that post to completion ports, the
//!   reducer's and the speech queue's, whose post never waits and takes no
//!   lock; a backlog waits in memory in front of the reducer, as the
//!   communication design keeps every backlog
//!   (`docs/design/communication.md`, section 4.8).
//! - It touches no lock, and makes no call that dispatches sent messages,
//!   which would deliver the next key while this one is half handled.
//!   `cargo xtask ci` checks the hook thread's modules for both
//!   (`docs/design/ownership-input.md`, "The ownership check on the hook").
//!
//! Besides bound gestures the hook reports what a passed key did that the
//! reducer needs to know: an observed gesture (a caret key) and the text a
//! key types. Before each key it tells the decision machine whether Num
//! Lock is on.
//!
//! # Key numbers
//!
//! The hook thread is the one minter of key numbers ([`KeySeq`]): every key
//! press, and every gesture injected for the user ([`Injector`]), takes the
//! next number in the order the thread handles it, and every report of
//! what it caused carries it. A press cancels speech at once, from this
//! thread, while the previous press's speech may still be on its way; the
//! cancel therefore names its press's number, and the speech manager drops
//! speech caused by an earlier press that reaches it after the cancel, as
//! NVDA, which queues a key's cancel in order behind the earlier keys'
//! scripts, never speaks it (`docs/nvda/input.md`, "What a key press does
//! to speech").

mod hook;
mod typed;

use std::collections::HashSet;
use std::io;
use std::os::windows::io::{AsHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use arc_swap::ArcSwapOption;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_QUIT,
};

use verbatim_input::map::SharedGestureMap;
use verbatim_input::state::{DecisionConfig, DecisionMachine, EmittedGesture, KeySpeechEffect};
use verbatim_model::{GestureId, HookForeground, KeySeq, TraceId, WindowHandle};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};

/// The `dwExtraInfo` Verbatim puts on keys it injects for its own purposes
/// (the Control tap that lets it take the foreground). The hook leaves
/// speech alone for them, as NVDA ignores the keys it injects itself; keys
/// injected on a user's behalf, such as the end-to-end harness's, carry no
/// tag and cancel speech like typed ones.
pub const OWN_INPUT_TAG: usize = 0x5642_544D;

/// The high half of the `dwExtraInfo` Verbatim puts on a key it held back
/// and now passes on to the application (`docs/design/key-holding.md`);
/// the low half is the key's place in line ([`HeldStroke::sequence`]), or
/// 0 on the modifiers and releases sent around a held key. The hook passes
/// these keys straight down the hook chain, deciding and reporting
/// nothing, as NVDA ignores the keys its caret scripts send. Keys injected
/// on a user's behalf, such as the end-to-end harness's, carry other
/// values and are held like typed ones.
pub const PASSED_ON_TAG: usize = 0x5642_4B50 << 32;

/// The bits of `dwExtraInfo` that hold [`PASSED_ON_TAG`].
const PASSED_ON_MASK: usize = 0xFFFF_FFFF << 32;

/// The `dwExtraInfo` of a key passed on with place in line `sequence`.
#[must_use]
pub fn passed_on_info(sequence: u32) -> usize {
    PASSED_ON_TAG | sequence as usize
}

/// The place in line a key passed on carries, when `extra_info` is
/// [`passed_on_info`]'s.
pub(crate) fn passed_on_sequence(extra_info: usize) -> Option<u32> {
    (extra_info & PASSED_ON_MASK == PASSED_ON_TAG)
        .then(|| u32::try_from(extra_info & !PASSED_ON_MASK).unwrap_or(0))
}

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

/// Core's view of the foreground window, as the keyboard hook needs it
/// (architecture section 5): the focus listener learns which window is in
/// front, Core decides from it whether keys may be held back for it and
/// which thread's keyboard layout translates its keys, and the reducer's
/// thread publishes that here after each input ([`ForegroundView::set`]).
/// The hook reads it with one lock-free load and applies it only while the
/// window it names is still the foreground window, as the hook reads it
/// locally; until Core catches up with a change, no key is held and keys
/// are translated with the layout of the thread Windows names as the
/// window's owner. The hook itself hooks no event.
#[derive(Debug, Default)]
pub struct ForegroundView(ArcSwapOption<HookForeground>);

impl ForegroundView {
    /// A view of no foreground window yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes Core's newest view, when it changed.
    pub fn set(&self, view: Option<HookForeground>) {
        if self.get() != view {
            self.0.store(view.map(Arc::new));
        }
    }

    /// The view published last.
    fn get(&self) -> Option<HookForeground> {
        self.0.load().as_deref().copied()
    }
}

/// Whether keys may be held back now, with `foreground` in front: only
/// when Core's `view` is about that window and lets them be.
fn may_hold(foreground: WindowHandle, view: Option<HookForeground>) -> bool {
    view.is_some_and(|view| view.window == foreground && view.holds_keys)
}

/// The key a held or deferred key transition came from, for the
/// application to be sent it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldStroke {
    /// The virtual-key code; `VK_PACKET` for a character typed as Unicode,
    /// whose UTF-16 unit is the scan code.
    pub vk: u16,
    /// The hardware scan code.
    pub scan_code: u32,
    /// Whether it is an extended key (the arrows and the navigation block).
    pub extended: bool,
    /// Its place in the line of keys held back, counted from 1, which the
    /// key carries when it is passed on ([`passed_on_info`]) so the hook
    /// knows it has reached the application.
    pub sequence: u32,
}

/// Carries out a key press's effect on speech, given the press's key
/// number; called on the hook thread, so it must not block, nor make a
/// call that dispatches sent messages (a cross-apartment COM call,
/// `SendMessage`), which would deliver the next key to the hook while this
/// one is still being handled.
pub type SpeechEffectFn = Box<dyn Fn(KeySpeechEffect, KeySeq) + Send>;

/// What the hook thread is given to act on and report keys through, and
/// Core's views it reads: the speech effect, the key reports, which keys
/// are held, and the foreground.
type Effects = (
    SpeechEffectFn,
    KeyReportFn,
    Arc<KeyHolding>,
    Arc<ForegroundView>,
);

/// Receives what each key and each injected input caused, in the order the
/// hook thread handled them; called on the hook thread, so it must not
/// block, nor dispatch sent messages, as [`SpeechEffectFn`] explains.
pub type KeyReportFn = Box<dyn Fn(KeyReport) + Send>;

/// What a key the hook handled, or an input injected for the user, caused,
/// reported after the key's effect on speech. Every user input's report
/// carries its key number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyReport {
    /// The key completed an observed gesture (`Decision::observed`), a
    /// caret key whose result the reducer speaks.
    Observed {
        /// The gesture.
        gesture: EmittedGesture,
        /// The key's number.
        key_seq: KeySeq,
        /// Microseconds since the Unix epoch when the hook procedure ran,
        /// the clock outposts stamp what they observe with (`observed_at_us`
        /// and the latency log). The
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
        /// The key's number.
        key_seq: KeySeq,
        /// As [`KeyReport::Observed`]'s.
        pressed_at_us: u64,
        /// The key to send again.
        stroke: HeldStroke,
    },
    /// A key transition the hook would have passed to the application but
    /// held back, because a held key ahead of it has not reached the
    /// application yet: it is passed on after the keys ahead of it, in the
    /// order pressed, never read (`docs/design/key-holding.md`, "Keys
    /// behind a held key"). What the key did is reported as for a key
    /// passed at once, by the other reports, which come before this one.
    Deferred {
        /// The transition to send again.
        stroke: HeldStroke,
        /// Whether it is a press; a release otherwise.
        pressed: bool,
    },
    /// Any other key passed to the application, typing no text, that is not
    /// a modifier or a lock key: the source of a caret key judged by where
    /// the caret landed (`CaretMotion::Other`).
    Passed {
        /// Minted when the key was observed.
        trace_id: TraceId,
        /// The key's number.
        key_seq: KeySeq,
        /// As [`KeyReport::Observed`]'s.
        pressed_at_us: u64,
    },
    /// The key types `text` into the focused application: the source of
    /// `Input::CharacterTyped`. A tab is a tab character and Enter a
    /// carriage return; a dead key types nothing until the key after it.
    Typed {
        /// Minted when the key was observed.
        trace_id: TraceId,
        /// The key's number.
        key_seq: KeySeq,
        /// The text typed.
        text: String,
        /// When the key was pressed, in microseconds on the run's shared
        /// clock.
        pressed_at_us: u64,
    },
    /// A bound gesture fired, a gesture was injected for the user, or a
    /// lock key reached the operating system (its `kb:` gesture), for the
    /// reducer to run or announce.
    Gesture {
        /// The gesture.
        gesture: EmittedGesture,
        /// The key's number, or the injected gesture's.
        key_seq: KeySeq,
        /// As [`KeyReport::Observed`]'s.
        pressed_at_us: u64,
    },
    /// An end-to-end harness's numbered key stroke has been handled by the
    /// hook (`verbatim_input::harness`): everything it caused was reported
    /// before this.
    Handled(u64),
    /// A control-plane request waiting for Verbatim to be idle, named by its
    /// token, injected ([`Injected::Barrier`]): everything injected or
    /// decided before it was reported before this.
    Barrier(u64),
}

/// The private message an injected input travels in to the hook thread,
/// its `LPARAM` the boxed [`Injected`] and its `WPARAM` the hook's
/// injection token: any other process at Verbatim's integrity can post a
/// `WM_APP` to the thread, or to a COM window on it, so the hook takes a
/// box only from a thread message carrying the token, drawn at random once
/// per hook ([`InputHook::start`]).
const WM_INJECTED: u32 = WM_APP;

/// A fresh random injection token, from the process's random hash keys,
/// which the system seeds.
fn injection_token() -> usize {
    use std::hash::BuildHasher;
    let token = std::collections::hash_map::RandomState::new().hash_one(0_u8);
    // `usize` is 64 bits on every target Verbatim builds for.
    usize::try_from(token).unwrap_or(usize::MAX)
}

/// An input injected for the user, handled by the hook thread in its turn
/// among the keys ([`Injector::inject`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Injected {
    /// A bound gesture, numbered as a key press is: it cancels speech
    /// through its number and is reported as [`KeyReport::Gesture`].
    Gesture(GestureId),
    /// An idle waiter's token, reported as [`KeyReport::Barrier`] behind
    /// everything handled before it.
    Barrier(u64),
}

/// Posts inputs injected for the user to the hook thread, from any thread:
/// the control plane's gestures and idle waiters today. Cloneable and
/// sendable. It holds a handle to the hook thread, so the thread's id is
/// not given to another thread while any `Injector` can post to it.
#[derive(Clone, Debug)]
pub struct Injector(Arc<InjectTarget>);

/// The thread an [`Injector`] posts to.
#[derive(Debug)]
struct InjectTarget {
    thread_id: u32,
    /// Sent as the message's `WPARAM` ([`WM_INJECTED`]).
    token: usize,
    /// Keeps the thread object, and so its id, while this lives; closed
    /// when the last [`Injector`] goes.
    _thread: Option<OwnedHandle>,
}

impl Injector {
    /// Posts `injected` to the hook thread, behind every key and input the
    /// thread has not yet taken. Never waits.
    ///
    /// # Errors
    ///
    /// Gives `injected` back when it could not be posted: the hook thread
    /// has gone, or its message queue is full. An injection given back
    /// never runs later.
    pub fn inject(&self, injected: Injected) -> Result<(), Injected> {
        let item = Box::into_raw(Box::new(injected));
        // SAFETY: posting to a thread id is always sound; the pointer is
        // carried, never read by the system, and taken back below or by
        // the hook thread, which owns it once the post succeeds.
        let posted = unsafe {
            PostThreadMessageW(
                self.0.thread_id,
                WM_INJECTED,
                WPARAM(self.0.token),
                LPARAM(item as isize),
            )
        };
        if posted.is_ok() {
            return Ok(());
        }
        // SAFETY: the message was not posted, so the box was never given up.
        Err(*unsafe { Box::from_raw(item) })
    }

    /// An injector to `thread_id`, holding no handle: for a test.
    #[cfg(test)]
    fn to_thread(thread_id: u32) -> Self {
        Self(Arc::new(InjectTarget {
            thread_id,
            token: 0,
            _thread: None,
        }))
    }
}

/// A running keyboard hook. Dropping it stops the hook thread and unhooks.
#[derive(Debug)]
pub struct InputHook {
    thread_id: u32,
    injector: Injector,
    join: Option<JoinHandle<()>>,
}

impl InputHook {
    /// Installs the low-level keyboard hook on a fresh dedicated thread.
    ///
    /// `config` chooses the Verbatim modifier keys and the double-tap timeout,
    /// and `map` is the lock-free bound-gesture snapshot the hook consults.
    /// `speech` carries out each key press's effect on speech (cancel, or
    /// pause and resume), before anything the press caused is reported, so
    /// speech it causes is never the speech it cancels. `reports` receives,
    /// after the speech effect, everything each key and injected input
    /// caused ([`KeyReport`]). Both run on the hook thread and must never
    /// block (see the crate's never-block constraint). `holding` says which
    /// caret keys are held back, and when, and `foreground` Core's view of
    /// the window in front ([`ForegroundView`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the hook thread cannot be spawned or its handle
    /// duplicated, or `SetWindowsHookExW` fails.
    pub fn start(
        config: DecisionConfig,
        map: SharedGestureMap,
        speech: SpeechEffectFn,
        reports: KeyReportFn,
        holding: Arc<KeyHolding>,
        foreground: Arc<ForegroundView>,
    ) -> io::Result<Self> {
        // The thread reports back either its id (hook installed) or the error
        // that stopped it, so `start` can surface installation failure.
        let (ready_tx, ready_rx) = mpsc::channel::<io::Result<u32>>();
        let token = injection_token();

        let join = thread::Builder::new()
            .name("verbatim-input-hook".to_owned())
            .spawn(move || {
                hook_thread(
                    config,
                    map,
                    (speech, reports, holding, foreground),
                    token,
                    &ready_tx,
                );
            })?;

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => {
                let thread = join.as_handle().try_clone_to_owned();
                let injector = match thread {
                    Ok(thread) => Injector(Arc::new(InjectTarget {
                        thread_id,
                        token,
                        _thread: Some(thread),
                    })),
                    Err(error) => {
                        let hook = Self {
                            thread_id,
                            injector: Injector::detached(),
                            join: Some(join),
                        };
                        drop(hook);
                        return Err(error);
                    }
                };
                Ok(Self {
                    thread_id,
                    injector,
                    join: Some(join),
                })
            }
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

    /// The handle other threads inject inputs for the user through.
    #[must_use]
    pub fn injector(&self) -> Injector {
        self.injector.clone()
    }
}

impl Injector {
    /// An injector that posts to no thread, every injection given back.
    fn detached() -> Self {
        Self(Arc::new(InjectTarget {
            thread_id: 0,
            token: 0,
            _thread: None,
        }))
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
/// messages until `WM_QUIT`, then unhook and drop the injected inputs left.
fn hook_thread(
    config: DecisionConfig,
    map: SharedGestureMap,
    effects: Effects,
    token: usize,
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
    let hook = match unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(hook::keyboard_hook),
            Some(hinstance),
            0,
        )
    } {
        Ok(hook) => hook,
        Err(error) => {
            let _ = ready_tx.send(Err(io::Error::from_raw_os_error(error.code().0)));
            return;
        }
    };

    let (typing, typing_error) = typed::Typing::with_text_services();
    if let Some(error) = typing_error {
        tracing::warn!(%error, "the text services' profile manager could not be made");
    }
    // Install this thread's decision state before announcing readiness, so the
    // procedure never runs against an empty thread-local.
    hook::HOOK_STATE.with(|state| {
        *state.borrow_mut() = Some(hook::HookState::new(
            DecisionMachine::new(config, map),
            effects,
            typing,
            token,
        ));
    });

    // SAFETY: reads a thread-owned integer.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready_tx.send(Ok(thread_id)).is_err() {
        // The starter is gone; unhook and leave.
        // SAFETY: `hook` is the handle we just installed.
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
        hook::HOOK_STATE.with(|state| *state.borrow_mut() = None);
        return;
    }

    hook::pump_messages();

    // SAFETY: `hook` is the handle installed above and not yet unhooked.
    unsafe {
        let _ = UnhookWindowsHookEx(hook);
    }
    hook::drop_injected();
    hook::HOOK_STATE.with(|state| *state.borrow_mut() = None);
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
